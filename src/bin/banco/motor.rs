//! Un proceso UCI visto desde el árbitro.
//!
//! El banco nunca se fía de lo que el motor diga sobre el estado de la
//! partida (si hay mate, si son tablas): solo le pide jugadas. Todo lo
//! demás lo decide el árbitro con las reglas reales. Este módulo se limita
//! a hablar el protocolo y a devolver, por cada jugada, lo que el motor
//! anunció: la jugada, su puntuación, profundidad, nodos y tiempo real
//! consumido.
//!
//! El tiempo lo mide el árbitro con su propio reloj, no el `time` que
//! reporta el motor: en control de tiempo real, quien tiene que decidir si
//! alguien se ha pasado es el árbitro.

use std::io::{BufRead, BufReader, Write};
use std::path::Path;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::thread;
use std::time::{Duration, Instant};

/// Puntuación anunciada por el motor para la jugada que acaba de elegir.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Score {
    Cp(i32),
    /// Jugadas hasta el mate, con signo: positivo si mata quien mueve.
    Mate(i32),
}

impl Score {
    /// Valor en centipeones para comparar contra los umbrales de
    /// adjudicación. Un mate se trata como una ventaja mayor que cualquier
    /// umbral razonable, conservando el signo.
    pub fn as_cp(self) -> i32 {
        match self {
            Score::Cp(cp) => cp,
            Score::Mate(n) if n >= 0 => 1_000_000,
            Score::Mate(_) => -1_000_000,
        }
    }

    pub fn to_pgn_comment(self) -> String {
        match self {
            Score::Cp(cp) => format!("{:+.2}", cp as f64 / 100.0),
            Score::Mate(n) => format!("M{n}"),
        }
    }
}

/// Lo que devuelve el motor al pedirle una jugada.
#[derive(Clone, Debug)]
pub struct Respuesta {
    pub uci: String,
    /// `None` cuando el motor devolvió `bestmove` sin haber anunciado
    /// ningún `info ... score` utilizable. No es un detalle cosmético: una
    /// ventana de adjudicación que ignore estos huecos y una que los trate
    /// como corte dan resultados distintos. Aquí cortan (ver `arbitro`).
    pub score: Option<Score>,
    pub depth: Option<u32>,
    pub nodes: Option<u64>,
    pub elapsed: Duration,
}

/// Cómo se le pide al motor que piense.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Limite {
    /// Reproducible entre máquinas y entre ejecuciones: el resultado no
    /// depende de la carga del equipo. Es el modo por defecto del banco.
    Nodos(u64),
    Movetime(u64),
    Profundidad(u32),
    /// Control de tiempo real; los relojes los lleva el árbitro.
    Reloj { wtime_ms: u64, btime_ms: u64, winc_ms: u64, binc_ms: u64 },
}

pub struct Motor {
    pub etiqueta: String,
    pub id_name: String,
    child: Child,
    stdin: ChildStdin,
    rx: Receiver<String>,
    handshake_timeout: Duration,
}

impl Motor {
    /// Lanza el proceso, hace el saludo UCI y aplica las opciones.
    ///
    /// `cwd` es el directorio de trabajo del proceso: un motor puede abrir
    /// ficheros relativos (tablas, redes, configuración) y ejecutarlo desde
    /// otro sitio cambiaría en silencio lo que se está midiendo.
    pub fn lanzar(
        ruta: &Path,
        etiqueta: &str,
        cwd: Option<&Path>,
        opciones: &[(String, String)],
        handshake_timeout: Duration,
    ) -> Result<Motor, String> {
        let mut cmd = Command::new(ruta);
        cmd.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null());
        if let Some(dir) = cwd {
            cmd.current_dir(dir);
        }
        let mut child = cmd
            .spawn()
            .map_err(|e| format!("{etiqueta}: no se pudo lanzar '{}': {e}", ruta.display()))?;
        let stdin = child.stdin.take().expect("stdin conectado");
        let stdout = child.stdout.take().expect("stdout conectado");

        // Hilo lector: convierte el `read_line` bloqueante en un canal, para
        // poder esperar una línea concreta con plazo en vez de quedarse
        // colgado contra un motor que ha dejado de responder.
        let (tx, rx) = mpsc::channel();
        thread::spawn(move || {
            let mut reader = BufReader::new(stdout);
            let mut line = String::new();
            loop {
                line.clear();
                match reader.read_line(&mut line) {
                    Ok(0) | Err(_) => break,
                    Ok(_) => {
                        if tx.send(line.trim_end().to_string()).is_err() {
                            break;
                        }
                    }
                }
            }
        });

        let mut motor = Motor {
            etiqueta: etiqueta.to_string(),
            id_name: String::new(),
            child,
            stdin,
            rx,
            handshake_timeout,
        };

        motor.send("uci")?;
        let deadline = Instant::now() + handshake_timeout;
        let mut vio_uciok = false;
        let mut anunciadas: Vec<String> = Vec::new();
        while let Some(line) = motor.recv_until(deadline) {
            if let Some(name) = line.strip_prefix("id name ") {
                motor.id_name = name.trim().to_string();
            }
            if let Some(nombre) = nombre_anunciado(&line) {
                anunciadas.push(nombre.to_string());
            }
            if line == "uciok" {
                vio_uciok = true;
                break;
            }
        }
        if !vio_uciok {
            return Err(format!("{etiqueta}: no respondió 'uciok' en {handshake_timeout:?}"));
        }
        if motor.id_name.is_empty() {
            motor.id_name = format!("(sin id name) {}", ruta.display());
        }

        // Un `setoption` con un nombre que el motor no conoce no da error:
        // el protocolo manda ignorarlo, y la tanda seguiría adelante midiendo
        // una configuración que no es la pedida. Aquí ya pasó con `UseNNUE`,
        // que los binarios anteriores a 0.29 no tienen: la red se quedaba
        // apagada y el experimento comparaba otra cosa. Se aborta antes de
        // jugar nada.
        let faltan = opciones_no_anunciadas(&anunciadas, opciones);
        if !faltan.is_empty() {
            return Err(format!(
                "{etiqueta}: pide opciones UCI que el motor no anuncia: {}. Un 'setoption' \
                 desconocido se ignora en silencio y la tanda mediría otra cosa. El motor \
                 anuncia: {}",
                faltan.join(", "),
                if anunciadas.is_empty() { "ninguna".to_string() } else { anunciadas.join(", ") },
            ));
        }
        for (nombre, valor) in opciones {
            motor.send(&format!("setoption name {nombre} value {valor}"))?;
        }
        motor.sincronizar()?;
        Ok(motor)
    }

    fn send(&mut self, cmd: &str) -> Result<(), String> {
        writeln!(self.stdin, "{cmd}")
            .and_then(|()| self.stdin.flush())
            .map_err(|e| format!("{}: fallo escribiendo '{cmd}': {e}", self.etiqueta))
    }

    fn recv_until(&self, deadline: Instant) -> Option<String> {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return None;
        }
        // Agotar el plazo y que el motor haya muerto se tratan igual: en
        // ambos casos no hay línea, y quien decide qué hacer es el árbitro.
        self.rx.recv_timeout(remaining).ok()
    }

    /// `isready`/`readyok`.
    pub fn sincronizar(&mut self) -> Result<(), String> {
        self.drenar();
        self.send("isready")?;
        let deadline = Instant::now() + self.handshake_timeout;
        while let Some(line) = self.recv_until(deadline) {
            if line == "readyok" {
                return Ok(());
            }
        }
        Err(format!("{}: no respondió 'readyok'", self.etiqueta))
    }

    pub fn nueva_partida(&mut self) -> Result<(), String> {
        self.send("ucinewgame")?;
        self.sincronizar()
    }

    /// Descarta líneas pendientes de un comando anterior. Sin esto, un
    /// `info` rezagado podría leerse como si fuera de la búsqueda actual y
    /// contaminar la puntuación que ve la adjudicación.
    fn drenar(&self) {
        while self.rx.try_recv().is_ok() {}
    }

    /// Pide una jugada. `fen_inicial` es `None` para la posición inicial.
    ///
    /// `plazo` es el tiempo total de reloj que el árbitro está dispuesto a
    /// esperar por `bestmove`. Lo calcula el árbitro, no este módulo: en
    /// control de tiempo depende de cuánto le queda **al bando que mueve**,
    /// y aquí no se sabe quién mueve.
    ///
    /// Devuelve `Ok(None)` si el motor no contestó dentro del plazo: eso lo
    /// resuelve el árbitro como incomparecencia (pérdida), no como fallo
    /// del banco.
    pub fn pedir_jugada(
        &mut self,
        fen_inicial: Option<&str>,
        jugadas: &[String],
        limite: Limite,
        plazo: Duration,
    ) -> Result<Option<Respuesta>, String> {
        self.drenar();
        let mut position = match fen_inicial {
            Some(fen) => format!("position fen {fen}"),
            None => "position startpos".to_string(),
        };
        if !jugadas.is_empty() {
            position.push_str(" moves ");
            position.push_str(&jugadas.join(" "));
        }
        self.send(&position)?;

        let go = match limite {
            Limite::Nodos(n) => format!("go nodes {n}"),
            Limite::Movetime(ms) => format!("go movetime {ms}"),
            Limite::Profundidad(d) => format!("go depth {d}"),
            Limite::Reloj { wtime_ms, btime_ms, winc_ms, binc_ms } => format!(
                "go wtime {wtime_ms} btime {btime_ms} winc {winc_ms} binc {binc_ms}"
            ),
        };

        let inicio = Instant::now();
        self.send(&go)?;
        let deadline = inicio + plazo;

        let mut score = None;
        let mut depth = None;
        let mut nodes = None;
        while let Some(line) = self.recv_until(deadline) {
            if let Some(resto) = line.strip_prefix("bestmove ") {
                let uci = resto.split_whitespace().next().unwrap_or_default().to_string();
                if uci.is_empty() {
                    return Ok(None);
                }
                return Ok(Some(Respuesta { uci, score, depth, nodes, elapsed: inicio.elapsed() }));
            }
            if line.starts_with("info ") {
                if let Some(info) = parse_info(&line) {
                    if let Some(s) = info.score {
                        score = Some(s);
                    }
                    if let Some(d) = info.depth {
                        depth = Some(d);
                    }
                    if let Some(n) = info.nodes {
                        nodes = Some(n);
                    }
                }
            }
        }
        Ok(None)
    }
}

impl Drop for Motor {
    fn drop(&mut self) {
        let _ = self.send("quit");
        for _ in 0..100 {
            if matches!(self.child.try_wait(), Ok(Some(_))) {
                return;
            }
            thread::sleep(Duration::from_millis(10));
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Nombre de una línea `option name <nombre> type <tipo> ...` del saludo.
///
/// El nombre puede llevar espacios (`Clear Hash`, `UCI_LimitStrength`), así
/// que se corta por el ` type ` que exige el protocolo y no por el primer
/// espacio.
fn nombre_anunciado(line: &str) -> Option<&str> {
    let resto = line.strip_prefix("option name ")?;
    let nombre = match resto.find(" type ") {
        Some(i) => &resto[..i],
        None => resto,
    };
    let nombre = nombre.trim();
    (!nombre.is_empty()).then_some(nombre)
}

/// Opciones que pide la configuración y el motor no anunció.
///
/// Se comparan sin distinguir mayúsculas: el protocolo no obliga a nada y los
/// motores no se ponen de acuerdo, y un falso positivo aquí aborta una tanda
/// de horas por una diferencia de caja.
fn opciones_no_anunciadas<'a>(anunciadas: &[String], pedidas: &'a [(String, String)]) -> Vec<&'a str> {
    pedidas
        .iter()
        .map(|(nombre, _)| nombre.as_str())
        .filter(|nombre| !anunciadas.iter().any(|a| a.eq_ignore_ascii_case(nombre)))
        .collect()
}

#[derive(Default)]
struct Info {
    score: Option<Score>,
    depth: Option<u32>,
    nodes: Option<u64>,
}

/// Extrae de una línea `info` lo que le interesa al banco.
///
/// Se descartan por completo:
/// - las líneas de `multipv` distinto de 1, cuya puntuación no es la de la
///   posición sino la de una variante alternativa;
/// - las puntuaciones marcadas `lowerbound`/`upperbound`, que son cotas de
///   una ventana de aspiración a medio resolver y no una evaluación.
///
/// Descartar de más es seguro: el hueco se propaga como `score: None` y
/// corta la ventana de adjudicación. Aceptar de más no lo es.
fn parse_info(line: &str) -> Option<Info> {
    let tokens: Vec<&str> = line.split_whitespace().collect();
    let mut info = Info::default();
    let mut i = 0;
    while i < tokens.len() {
        match tokens[i] {
            "multipv" => {
                if tokens.get(i + 1) != Some(&"1") {
                    return None;
                }
                i += 2;
            }
            "depth" => {
                info.depth = tokens.get(i + 1).and_then(|t| t.parse().ok());
                i += 2;
            }
            "nodes" => {
                info.nodes = tokens.get(i + 1).and_then(|t| t.parse().ok());
                i += 2;
            }
            "score" => {
                let kind = tokens.get(i + 1).copied();
                let value: Option<i32> = tokens.get(i + 2).and_then(|t| t.parse().ok());
                let bounded = matches!(tokens.get(i + 3), Some(&"lowerbound") | Some(&"upperbound"));
                info.score = match (kind, value, bounded) {
                    (Some("cp"), Some(v), false) => Some(Score::Cp(v)),
                    (Some("mate"), Some(v), false) => Some(Score::Mate(v)),
                    _ => None,
                };
                i += 3;
            }
            _ => i += 1,
        }
    }
    Some(info)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_a_centipawn_score_with_depth_and_nodes() {
        let info = parse_info("info depth 12 score cp -34 nodes 91234 nps 1 time 5 pv e2e4").unwrap();
        assert_eq!(info.score, Some(Score::Cp(-34)));
        assert_eq!(info.depth, Some(12));
        assert_eq!(info.nodes, Some(91234));
    }

    #[test]
    fn reads_a_mate_score_with_its_sign() {
        let info = parse_info("info depth 20 score mate -3 pv e2e4").unwrap();
        assert_eq!(info.score, Some(Score::Mate(-3)));
    }

    #[test]
    fn a_bounded_score_is_not_taken_as_an_evaluation() {
        let info = parse_info("info depth 8 score cp 500 lowerbound pv e2e4").unwrap();
        assert_eq!(info.score, None);
    }

    #[test]
    fn a_secondary_multipv_line_is_discarded_whole() {
        assert!(parse_info("info multipv 2 depth 9 score cp 20 pv d2d4").is_none());
        let primary = parse_info("info multipv 1 depth 9 score cp 20 pv d2d4").unwrap();
        assert_eq!(primary.score, Some(Score::Cp(20)));
    }

    #[test]
    fn an_info_line_without_a_score_leaves_the_hole_visible() {
        let info = parse_info("info depth 3 nodes 100 time 1").unwrap();
        assert_eq!(info.score, None);
        assert_eq!(info.depth, Some(3));
    }

    #[test]
    fn a_pv_containing_keyword_like_tokens_does_not_confuse_the_parser() {
        // El pv va al final y sus tokens son jugadas, pero un parser que
        // busque "depth" en cualquier sitio se comería un "depth" tardío.
        let info = parse_info("info depth 4 score cp 15 pv e2e4 e7e5 g1f3").unwrap();
        assert_eq!(info.depth, Some(4));
        assert_eq!(info.score, Some(Score::Cp(15)));
    }

    #[test]
    fn mate_scores_dominate_any_adjudication_threshold() {
        assert!(Score::Mate(5).as_cp() > 1000);
        assert!(Score::Mate(-5).as_cp() < -1000);
        assert_eq!(Score::Cp(-250).as_cp(), -250);
    }

    #[test]
    fn an_option_name_with_spaces_survives_the_handshake() {
        assert_eq!(nombre_anunciado("option name Clear Hash type button"), Some("Clear Hash"));
        assert_eq!(nombre_anunciado("option name Hash type spin default 16 min 1 max 1024"), Some("Hash"));
        // Sin ` type ` no es una línea legal, pero tampoco se pierde el nombre.
        assert_eq!(nombre_anunciado("option name Ponder"), Some("Ponder"));
        assert_eq!(nombre_anunciado("id name Vigia 0.31"), None);
        assert_eq!(nombre_anunciado("option name  type check default false"), None);
    }

    #[test]
    fn an_option_the_engine_never_advertised_is_caught_before_playing() {
        let anunciadas = vec!["Hash".to_string(), "Threads".to_string()];
        let pedidas = vec![
            ("Hash".to_string(), "32".to_string()),
            ("usennue".to_string(), "true".to_string()),
        ];
        // El nombre desconocido sale, y solo él: la caja no cuenta.
        assert_eq!(opciones_no_anunciadas(&anunciadas, &pedidas), vec!["usennue"]);
        let solo_conocidas = vec![("threads".to_string(), "1".to_string())];
        assert!(opciones_no_anunciadas(&anunciadas, &solo_conocidas).is_empty());
    }

    #[test]
    fn pgn_comments_use_pawn_units() {
        assert_eq!(Score::Cp(134).to_pgn_comment(), "+1.34");
        assert_eq!(Score::Cp(-7).to_pgn_comment(), "-0.07");
        assert_eq!(Score::Mate(-3).to_pgn_comment(), "M-3");
    }
}
