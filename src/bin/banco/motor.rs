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
        let mut anunciadas: Vec<OpcionAnunciada> = Vec::new();
        while let Some(line) = motor.recv_until(deadline) {
            if let Some(name) = line.strip_prefix("id name ") {
                motor.id_name = name.trim().to_string();
            }
            if let Some(op) = opcion_anunciada(&line) {
                anunciadas.push(op);
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

        // Un `setoption` que el motor no entiende no da error: el protocolo
        // manda ignorarlo, y la tanda seguiría adelante midiendo una
        // configuración que no es la pedida. Aquí ya pasó con `UseNNUE`, que
        // los binarios anteriores a 0.29 no tienen: la red se quedaba apagada
        // y el experimento comparaba otra cosa. Se aborta antes de jugar nada,
        // y se miran las dos mitades de la línea: el nombre y el valor.
        let faltan = opciones_no_anunciadas(&anunciadas, opciones);
        if !faltan.is_empty() {
            return Err(format!(
                "{etiqueta}: pide opciones UCI que el motor no anuncia: {}. Un 'setoption' \
                 desconocido se ignora en silencio y la tanda mediría otra cosa. El motor \
                 anuncia: {}",
                faltan.join(", "),
                if anunciadas.is_empty() {
                    "ninguna".to_string()
                } else {
                    anunciadas.iter().map(|o| o.nombre.as_str()).collect::<Vec<_>>().join(", ")
                },
            ));
        }
        let malos: Vec<String> = opciones
            .iter()
            .filter_map(|(nombre, valor)| {
                anunciadas.iter().find(|o| o.nombre.eq_ignore_ascii_case(nombre))?.reproche(valor)
            })
            .collect();
        if !malos.is_empty() {
            return Err(format!(
                "{etiqueta}: valores que este motor no va a aceptar: {}. Un valor que no encaja \
                 con el tipo anunciado tampoco da error: el motor lo descarta y deja la opción \
                 como estaba —o peor, la pone en 'false'—, y la tanda mediría otra cosa.",
                malos.join("; "),
            ));
        }
        for (nombre, valor) in opciones {
            motor.send(&linea_setoption(&anunciadas, nombre, valor))?;
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

/// Una opción tal como el motor la anunció en el saludo.
struct OpcionAnunciada {
    nombre: String,
    tipo: TipoOpcion,
}

/// Lo que el protocolo permite declarar. `Otro` son `string`, `button` y
/// cualquier cosa que no se reconozca: su valor no se puede juzgar aquí.
enum TipoOpcion {
    Check,
    Spin { min: Option<i64>, max: Option<i64> },
    Combo { valores: Vec<String> },
    Otro,
}

impl OpcionAnunciada {
    /// Qué tiene de malo este valor, si algo. `None` es «adelante».
    ///
    /// Una diferencia de mayúsculas no es un error: es una grafía, y se arregla
    /// sola en `valor_para_el_cable`. Lo que sí se rechaza es un valor que no
    /// corresponde a nada de lo anunciado, porque ahí el fichero de experimento
    /// dice una cosa y la tanda mediría otra.
    fn reproche(&self, valor: &str) -> Option<String> {
        let nombre = &self.nombre;
        match &self.tipo {
            // Un motor resuelve el `check` comparando con "true" a secas —el de
            // Vigía lo hace—, así que un '1' no se ignora: apaga la opción.
            TipoOpcion::Check
                if !valor.eq_ignore_ascii_case("true") && !valor.eq_ignore_ascii_case("false") =>
            {
                Some(format!("{nombre} = '{valor}' (es 'check': solo vale 'true' o 'false')"))
            }
            TipoOpcion::Spin { min, max } => match valor.parse::<i64>() {
                Err(_) => Some(format!("{nombre} = '{valor}' (es 'spin': tiene que ser un entero)")),
                Ok(n) if min.is_some_and(|m| n < m) || max.is_some_and(|m| n > m) => Some(format!(
                    "{nombre} = {n} (fuera del rango anunciado {}..{})",
                    min.map(|m| m.to_string()).unwrap_or_else(|| "-".into()),
                    max.map(|m| m.to_string()).unwrap_or_else(|| "-".into()),
                )),
                Ok(_) => None,
            },
            TipoOpcion::Combo { valores }
                if !valores.iter().any(|v| v.eq_ignore_ascii_case(valor)) =>
            {
                Some(format!(
                    "{nombre} = '{valor}' (es 'combo'; el motor anuncia: {})",
                    valores.join(", ")
                ))
            }
            _ => None,
        }
    }

    /// El valor tal como conviene mandarlo: la grafía canónica del protocolo
    /// para un `check` y la que el motor anunció para un `combo`.
    ///
    /// Mismo motivo que con el nombre: admitir la diferencia de caja en la
    /// comprobación no sirve de nada si luego se envía una grafía que el motor
    /// despacha de forma exacta y descarta en silencio.
    fn valor_para_el_cable(&self, valor: &str) -> String {
        match &self.tipo {
            TipoOpcion::Check => valor.to_ascii_lowercase(),
            TipoOpcion::Combo { valores } => valores
                .iter()
                .find(|v| v.eq_ignore_ascii_case(valor))
                .cloned()
                .unwrap_or_else(|| valor.to_string()),
            _ => valor.to_string(),
        }
    }
}

/// Lee una línea `option name <nombre> type <tipo> ...` del saludo.
///
/// El nombre puede llevar espacios (`Clear Hash`, `UCI_LimitStrength`), así
/// que se corta por el ` type ` que exige el protocolo y no por el primer
/// espacio. Los valores de un `combo` también pueden llevarlos, de ahí que se
/// acumulen hasta la siguiente palabra clave en vez de tomar solo la siguiente.
fn opcion_anunciada(line: &str) -> Option<OpcionAnunciada> {
    let resto = line.strip_prefix("option name ")?;
    let (nombre, cola) = match resto.find(" type ") {
        Some(i) => (&resto[..i], &resto[i + " type ".len()..]),
        None => (resto, ""),
    };
    let nombre = nombre.trim();
    if nombre.is_empty() {
        return None;
    }
    let tokens: Vec<&str> = cola.split_whitespace().collect();
    let numero = |clave: &str| {
        tokens
            .iter()
            .position(|t| *t == clave)
            .and_then(|i| tokens.get(i + 1))
            .and_then(|t| t.parse::<i64>().ok())
    };
    let tipo = match tokens.first().copied() {
        Some("check") => TipoOpcion::Check,
        Some("spin") => TipoOpcion::Spin { min: numero("min"), max: numero("max") },
        Some("combo") => TipoOpcion::Combo { valores: valores_de_combo(&tokens[1..]) },
        _ => TipoOpcion::Otro,
    };
    Some(OpcionAnunciada { nombre: nombre.to_string(), tipo })
}

/// Los `var` de un `combo`, cada uno hasta la siguiente palabra clave.
fn valores_de_combo(tokens: &[&str]) -> Vec<String> {
    let mut valores: Vec<String> = Vec::new();
    let mut actual: Option<String> = None;
    for t in tokens {
        match *t {
            "var" => {
                valores.extend(actual.take().filter(|v| !v.is_empty()));
                actual = Some(String::new());
            }
            "default" | "min" | "max" => {
                valores.extend(actual.take().filter(|v| !v.is_empty()));
            }
            palabra => {
                if let Some(v) = actual.as_mut() {
                    if !v.is_empty() {
                        v.push(' ');
                    }
                    v.push_str(palabra);
                }
            }
        }
    }
    valores.extend(actual.filter(|v| !v.is_empty()));
    valores
}

/// Opciones que pide la configuración y el motor no anunció.
///
/// Se comparan sin distinguir mayúsculas: el protocolo no obliga a nada y los
/// motores no se ponen de acuerdo, y un falso positivo aquí aborta una tanda
/// de horas por una diferencia de caja. Esa tolerancia solo es segura porque
/// lo que se envía después es la grafía anunciada (`grafia_anunciada`).
fn opciones_no_anunciadas<'a>(
    anunciadas: &[OpcionAnunciada],
    pedidas: &'a [(String, String)],
) -> Vec<&'a str> {
    pedidas
        .iter()
        .map(|(nombre, _)| nombre.as_str())
        .filter(|nombre| !anunciadas.iter().any(|a| a.nombre.eq_ignore_ascii_case(nombre)))
        .collect()
}

/// La línea `setoption` tal como conviene mandarla: con la grafía que el motor
/// anunció, no con la del fichero de experimento.
///
/// La comprobación de nombres y valores admite diferencias de mayúsculas a
/// propósito —el protocolo no obliga a nada y abortar una tanda de horas por
/// una diferencia de caja sería peor—, pero el despacho de un motor real puede
/// ser exacto: el de Vigía lo es. Sin esto, `usennue` pasaría la comprobación y
/// se perdería igual, que es justo el fallo silencioso que se quería cerrar.
fn linea_setoption(anunciadas: &[OpcionAnunciada], nombre: &str, valor: &str) -> String {
    match anunciadas.iter().find(|o| o.nombre.eq_ignore_ascii_case(nombre)) {
        Some(op) => {
            format!("setoption name {} value {}", op.nombre, op.valor_para_el_cable(valor))
        }
        // Sin anunciar no se llega hasta aquí: lo aborta `opciones_no_anunciadas`.
        None => format!("setoption name {nombre} value {valor}"),
    }
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
        let nombre = |line| opcion_anunciada(line).map(|o| o.nombre);
        assert_eq!(nombre("option name Clear Hash type button").as_deref(), Some("Clear Hash"));
        assert_eq!(
            nombre("option name Hash type spin default 16 min 1 max 1024").as_deref(),
            Some("Hash")
        );
        // Sin ` type ` no es una línea legal, pero tampoco se pierde el nombre.
        assert_eq!(nombre("option name Ponder").as_deref(), Some("Ponder"));
        assert!(nombre("id name Vigia 0.31").is_none());
        assert!(nombre("option name  type check default false").is_none());
    }

    #[test]
    fn an_option_the_engine_never_advertised_is_caught_before_playing() {
        let anunciadas = vec![
            opcion_anunciada("option name Hash type spin default 16 min 1 max 1024").unwrap(),
            opcion_anunciada("option name Threads type spin default 1 min 1 max 16").unwrap(),
        ];
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
    fn what_travels_on_the_wire_is_the_spelling_the_engine_advertised() {
        // La comprobación admite 'usennue', pero un motor que despache con un
        // `match` exacto —el de Vigía— ignoraría ese `setoption` en silencio y
        // la tanda mediría la configuración de al lado.
        let anunciadas = vec![
            opcion_anunciada("option name UseNNUE type check default true").unwrap(),
            opcion_anunciada("option name Hash type spin default 16 min 1 max 1024").unwrap(),
            opcion_anunciada(
                "option name NNUEInstructions type combo default auto var auto var avx2 var portable",
            )
            .unwrap(),
        ];
        let linea = |n, v| linea_setoption(&anunciadas, n, v);
        assert_eq!(linea("usennue", "true"), "setoption name UseNNUE value true");
        assert_eq!(linea("HASH", "32"), "setoption name Hash value 32");
        // El valor también: 'True' es lo que quiso decir quien lo escribió, pero
        // el motor compara con "true" y dejaría la red apagada.
        assert_eq!(linea("UseNNUE", "True"), "setoption name UseNNUE value true");
        // Y el `var` de un combo va con la grafía del motor, no con la del fichero.
        assert_eq!(
            linea("NNUEInstructions", "AVX2"),
            "setoption name NNUEInstructions value avx2"
        );
        // Lo que no anunció se envía tal cual; ese caso ya ha abortado antes.
        assert_eq!(linea("Ponder", "true"), "setoption name Ponder value true");
    }

    #[test]
    fn a_value_the_engine_would_silently_drop_is_caught_before_playing() {
        let check = opcion_anunciada("option name UseNNUE type check default true").unwrap();
        // '1' no se ignora: el motor compara con "true" y deja la red APAGADA.
        assert!(check.reproche("1").is_some());
        assert!(check.reproche("si").is_some());
        assert!(check.reproche("true").is_none());
        assert!(check.reproche("false").is_none());
        // 'True' es una grafía, no un error: pasa, y viaja como "true".
        assert!(check.reproche("True").is_none());
        assert_eq!(check.valor_para_el_cable("True"), "true");

        let spin = opcion_anunciada("option name Hash type spin default 16 min 1 max 1024").unwrap();
        assert!(spin.reproche("32").is_none());
        assert!(spin.reproche("dos").is_some());
        assert!(spin.reproche("0").is_some());
        assert!(spin.reproche("2048").is_some());

        let combo =
            opcion_anunciada("option name NNUEInstructions type combo default auto var auto var avx2 var portable")
                .unwrap();
        assert!(combo.reproche("avx2").is_none());
        assert!(combo.reproche("auto").is_none());
        assert!(combo.reproche("AVX2").is_none());
        assert_eq!(combo.valor_para_el_cable("AVX2"), "avx2");
        assert!(combo.reproche("avx512").is_some());

        // De un `string` o un `button` no se puede juzgar el valor.
        let libre = opcion_anunciada("option name SyzygyPath type string default <empty>").unwrap();
        assert!(libre.reproche("C:/lo/que/sea").is_none());
    }

    #[test]
    fn the_values_of_a_combo_can_carry_spaces() {
        let op = opcion_anunciada("option name Estilo type combo default Muy lento var Muy lento var Rapido")
            .unwrap();
        assert!(op.reproche("Muy lento").is_none());
        assert!(op.reproche("Rapido").is_none());
        assert!(op.reproche("Muy").is_some());
    }

    #[test]
    fn pgn_comments_use_pawn_units() {
        assert_eq!(Score::Cp(134).to_pgn_comment(), "+1.34");
        assert_eq!(Score::Cp(-7).to_pgn_comment(), "-0.07");
        assert_eq!(Score::Mate(-3).to_pgn_comment(), "M-3");
    }
}
