//! Orquestador del match: reparte parejas entre workers, decide cuándo
//! parar y deja el experimento por escrito.
//!
//! # La regla que hace que esto sea un SPRT de verdad
//!
//! Con varios workers las parejas terminan desordenadas. Si el LLR se
//! evaluara sobre "todo lo que hay hecho", la decisión dependería de qué
//! worker acabó antes, es decir, de la carga de la máquina: dos ejecuciones
//! idénticas podrían parar en sitios distintos y ninguna sería reproducible.
//!
//! Por eso el LLR se evalúa **solo sobre el prefijo contiguo** de parejas
//! completadas: 0, 1, 2, ... sin huecos. Una pareja que termina antes de
//! tiempo espera en memoria a que se cierre el hueco anterior. Cuando el
//! prefijo cruza una frontera, la tanda para y las parejas especulativas
//! que ya se hubieran calculado más allá **se descartan**: entraron en el
//! cálculo de nadie y guardarlas invitaría a mirarlas.
//!
//! # Ficheros
//!
//! - `manifiesto.json`: qué se ejecutó exactamente, con hashes y firma.
//! - `parejas.jsonl`: una línea por pareja, en orden, con todas las
//!   jugadas. Es la **única fuente de verdad**; se vacía a disco después de
//!   cada pareja, así que un corte de luz cuesta como mucho una pareja.
//! - `resumen.json` y `partidas.pgn`: derivados, regenerables en cualquier
//!   momento con `banco informe`.

use std::collections::BTreeMap;
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use std::sync::mpsc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use vigia::types::Color;

use crate::arbitro::{self, ConfigPartida, Final, Partida, Resultado};
use crate::config::Config;
use crate::json::Json;
use crate::libro::{self, Apertura};
use crate::motor::{Limite, Motor, Score};
use crate::pgn;
use crate::sha256;
use crate::stats::{Decision, Pentanomial, Sprt};

pub const BANCO_VERSION: &str = "banco/1.0";

// ---------------------------------------------------------------- resultados

#[derive(Clone, Debug)]
pub struct PartidaJugada {
    pub candidato_con_blancas: bool,
    pub partida: Partida,
}

impl PartidaJugada {
    /// Puntos del candidato en esta partida.
    pub fn puntos_candidato(&self) -> f64 {
        let color = if self.candidato_con_blancas { Color::White } else { Color::Black };
        self.partida.resultado.puntos(color)
    }
}

#[derive(Clone, Debug)]
pub struct Pareja {
    pub indice: usize,
    pub fen: String,
    pub juegos: [PartidaJugada; 2],
}

impl Pareja {
    pub fn cajon(&self) -> usize {
        Pentanomial::bin_of(self.juegos[0].puntos_candidato(), self.juegos[1].puntos_candidato())
    }
}

#[derive(Default)]
pub struct Acumulado {
    pub pentanomial: Pentanomial,
    pub ganadas: u64,
    pub tablas: u64,
    pub perdidas: u64,
    pub anomalas: u64,
    pub plies: u64,
    pub finales: BTreeMap<String, u64>,
}

impl Acumulado {
    pub fn añadir(&mut self, pareja: &Pareja) {
        self.pentanomial
            .add_pair(pareja.juegos[0].puntos_candidato(), pareja.juegos[1].puntos_candidato());
        for juego in &pareja.juegos {
            match juego.puntos_candidato() {
                p if p > 0.75 => self.ganadas += 1,
                p if p < 0.25 => self.perdidas += 1,
                _ => self.tablas += 1,
            }
            if juego.partida.final_partida.es_anomalo() {
                self.anomalas += 1;
            }
            self.plies += juego.partida.jugadas.len() as u64;
            *self
                .finales
                .entry(juego.partida.final_partida.as_str().to_string())
                .or_insert(0) += 1;
        }
    }

    pub fn partidas(&self) -> u64 {
        self.ganadas + self.tablas + self.perdidas
    }
}

// --------------------------------------------------------------- preparación

pub struct Preparado {
    pub cfg: Config,
    pub aperturas: Vec<Apertura>,
    pub manifiesto: Json,
    pub firma: String,
}

/// Resuelve el libro, sella los binarios y construye el manifiesto.
pub fn preparar(cfg: Config) -> Result<Preparado, String> {
    let origen = libro::cargar(&cfg.libro.fichero)?;
    let mut aperturas = origen.aperturas.clone();
    if cfg.libro.barajar {
        libro::barajar(&mut aperturas, cfg.libro.semilla);
    }
    if aperturas.len() < cfg.partidas.max_parejas {
        return Err(format!(
            "el libro '{}' tiene {} posiciones únicas ({} líneas leídas) y se piden {} parejas. \
             Repetir aperturas devolvería la correlación que las parejas eliminan: o se amplía el \
             libro o se baja 'partidas.max_parejas' a {}.",
            origen.ruta, origen.unicas, origen.leidas, cfg.partidas.max_parejas, origen.unicas
        ));
    }
    aperturas.truncate(cfg.partidas.max_parejas);

    let sha_candidato = sha256::hash_file(&cfg.candidato.ruta)?;
    let sha_base = sha256::hash_file(&cfg.base.ruta)?;

    // Un arranque de prueba de cada motor: confirma que responden UCI antes
    // de comprometer horas de CPU, recoge su `id name` para el manifiesto y
    // las etiquetas del PGN, y comprueba que los dos entienden el límite de
    // búsqueda de la misma manera.
    let (id_candidato, nodos_candidato) = sondear(&cfg, true)?;
    let (id_base, nodos_base) = sondear(&cfg, false)?;
    if let Limite::Nodos(pedidos) = cfg.limite {
        comprobar_limite_de_nodos(pedidos, &cfg.candidato.nombre, nodos_candidato)?;
        comprobar_limite_de_nodos(pedidos, &cfg.base.nombre, nodos_base)?;
    }

    let mut manifiesto = Json::obj();
    manifiesto.set("esquema", crate::config::ESQUEMA.into());
    manifiesto.set("banco", BANCO_VERSION.into());
    manifiesto.set("id", cfg.id.as_str().into());
    manifiesto.set("generado", marca_de_tiempo().into());
    manifiesto.set("config", cfg.normalizado.clone());

    let mut motores = Json::obj();
    for (clave, motor_cfg, sha, id_name) in [
        ("candidato", &cfg.candidato, &sha_candidato, &id_candidato),
        ("base", &cfg.base, &sha_base, &id_base),
    ] {
        let mut m = Json::obj();
        m.set("ruta", motor_cfg.ruta.display().to_string().into());
        m.set("sha256", sha.as_str().into());
        m.set("id_name", id_name.as_str().into());
        motores.set(clave, m);
    }
    manifiesto.set("motores", motores);

    let mut libro_json = Json::obj();
    libro_json.set("fichero", origen.ruta.as_str().into());
    libro_json.set("sha256", origen.sha256.as_str().into());
    libro_json.set("lineas", origen.leidas.into());
    libro_json.set("unicas", origen.unicas.into());
    libro_json.set("usadas", aperturas.len().into());
    manifiesto.set("libro", libro_json);

    let mut entorno = Json::obj();
    entorno.set("so", std::env::consts::OS.into());
    entorno.set("arquitectura", std::env::consts::ARCH.into());
    entorno.set("cpus_disponibles", cpus_disponibles().into());
    entorno.set("reproducible", cfg.es_reproducible().into());
    manifiesto.set("entorno", entorno);

    let firma = calcular_firma(&cfg.normalizado, &sha_candidato, &sha_base, &origen.sha256);
    manifiesto.set("firma", firma.as_str().into());

    Ok(Preparado { cfg, aperturas, manifiesto, firma })
}

/// Arranca el motor, devuelve su `id name` y —si el límite es por nodos—
/// cuántos nodos dice haber visitado al pedirle exactamente ese límite
/// desde la posición inicial.
fn sondear(cfg: &Config, candidato: bool) -> Result<(String, Option<u64>), String> {
    let m = if candidato { &cfg.candidato } else { &cfg.base };
    let mut motor = Motor::lanzar(&m.ruta, &m.nombre, m.cwd.as_deref(), &m.opciones, cfg.saludo())?;
    let id_name = motor.id_name.clone();
    let nodos = match cfg.limite {
        Limite::Nodos(n) => motor
            .pedir_jugada(Some(arbitro::FEN_INICIAL), &[], Limite::Nodos(n), cfg.margen())?
            .and_then(|r| r.nodes),
        _ => None,
    };
    Ok((id_name, nodos))
}

/// Fracción del límite pedido por debajo de la cual se considera que el
/// motor no respeta `go nodes`.
///
/// Un motor que comprueba el contador cada cierto número de nodos siempre
/// se pasa un poco o se queda muy cerca; quedarse en la mitad significa que
/// está usando otro criterio de parada. Umbral generoso: se trata de
/// detectar un incumplimiento claro, no de exigir precisión al nodo.
const FRACCION_MINIMA_DE_NODOS: f64 = 0.5;

/// El control por nodos solo es una comparación justa si **los dos** motores
/// lo respetan.
///
/// Esto no es hipotético: la release 0.24 de Vigía anuncia `bestmove` tras
/// unos 4.500 nodos cuando se le piden 25.000, y enfrentada así a un binario
/// que sí los respeta pierde el 92 % de las partidas. El resultado parece
/// una mejora de +400 Elo y no mide nada: mide que un bando pensó cuatro
/// veces menos. Contra un binario antiguo hay que usar `movetime` o
/// `profundidad`.
fn comprobar_limite_de_nodos(pedidos: u64, nombre: &str, medidos: Option<u64>) -> Result<(), String> {
    let Some(medidos) = medidos else {
        return Err(format!(
            "'{nombre}' no informa de 'nodes' en sus líneas 'info', así que no se puede comprobar              que respete 'go nodes'. Usa 'movetime_ms' o 'profundidad' en 'busqueda'."
        ));
    };
    let minimo = (pedidos as f64 * FRACCION_MINIMA_DE_NODOS) as u64;
    if medidos >= minimo {
        return Ok(());
    }
    Err(format!(
        "'{nombre}' no respeta el límite por nodos: se le pidieron {pedidos} nodos desde la \
         posición inicial y anunció la jugada tras {medidos} ({:.0} % de lo pedido).\n\
         Enfrentar así dos motores no compara su fuerza, compara cuánto piensa cada uno. \
         Usa 'movetime_ms' (justo, pero dependiente de la máquina) o 'profundidad' \
         (reproducible, pero favorece al motor que hace más trabajo por nodo) en 'busqueda'.",
        medidos as f64 * 100.0 / pedidos as f64
    ))
}

/// La firma deja fuera los ajustes que no cambian **qué** se mide, solo
/// cómo se reparte el trabajo: número de workers, márgenes de espera y
/// cuántas parejas se piden. Así se puede reanudar una tanda con más o
/// menos CPUs libres, o alargarla si terminó sin decidir, sin perder lo ya
/// jugado; pero tocar un umbral, una opción UCI o un binario invalida la
/// continuación, que es justo lo que debe pasar.
fn calcular_firma(normalizado: &Json, sha_cand: &str, sha_base: &str, sha_libro: &str) -> String {
    let mut firmable = normalizado.clone();
    if let Some(partidas) = firmable.get_mut("partidas") {
        partidas.remove("workers");
        partidas.remove("margen_ms");
        partidas.remove("saludo_ms");
        partidas.remove("max_parejas");
    }
    let material = format!(
        "{BANCO_VERSION}|{}|{sha_cand}|{sha_base}|{sha_libro}",
        firmable.to_compact()
    );
    sha256::hash_bytes(material.as_bytes())
}

fn cpus_disponibles() -> u64 {
    std::thread::available_parallelism().map(|n| n.get() as u64).unwrap_or(1)
}

// ------------------------------------------------------------------ ejecución

pub struct Salida {
    pub acumulado: Acumulado,
    pub decision: Decision,
}

pub fn ejecutar(prep: &Preparado) -> Result<Salida, String> {
    let cfg = &prep.cfg;
    let dir = &cfg.salida;
    let ruta_manifiesto = dir.join("manifiesto.json");
    let ruta_parejas = dir.join("parejas.jsonl");

    let mut hechas: Vec<Pareja> = Vec::new();
    if dir.exists() {
        if !ruta_manifiesto.exists() {
            return Err(format!(
                "'{}' ya existe pero no tiene manifiesto.json: no es una tanda del banco. \
                 Elige otro 'salida' o borra el directorio a mano.",
                dir.display()
            ));
        }
        let previo = leer_json(&ruta_manifiesto)?;
        let firma_previa = previo.get("firma").and_then(|v| v.as_str()).unwrap_or("");
        if firma_previa != prep.firma {
            return Err(format!(
                "'{}' contiene otra tanda: la firma no coincide.\n  guardada: {firma_previa}\n  \
                 actual:   {}\nHan cambiado los binarios, el libro o algún parámetro que sí afecta \
                 a la medición. Mezclar ambas sería inventarse los datos.",
                dir.display(),
                prep.firma
            ));
        }
        hechas = leer_parejas(&ruta_parejas)?;
        let acumulado_previo = {
            let mut a = Acumulado::default();
            for p in &hechas {
                a.añadir(p);
            }
            a
        };
        let llr = acumulado_previo.pentanomial.llr(&cfg.sprt);
        if cfg.sprt.decide(llr).is_final() {
            return Err(format!(
                "'{}' ya terminó con decisión '{}' tras {} parejas. Alargar una prueba secuencial \
                 después de que haya decidido invalida su nivel de significación: si hace falta \
                 más evidencia, se abre un experimento nuevo con otro 'id' y otro libro.",
                dir.display(),
                cfg.sprt.decide(llr).as_str(),
                hechas.len()
            ));
        }
        if !hechas.is_empty() {
            println!("Reanudando: {} parejas ya jugadas, LLR = {llr:+.4}", hechas.len());
        }
    } else {
        std::fs::create_dir_all(dir)
            .map_err(|e| format!("no se pudo crear '{}': {e}", dir.display()))?;
    }
    escribir(&ruta_manifiesto, &prep.manifiesto.to_pretty())?;

    let ya = hechas.len();
    if ya >= prep.aperturas.len() {
        println!("No queda nada por jugar: {ya} parejas de {}.", prep.aperturas.len());
        return finalizar(prep, hechas, Duration::ZERO);
    }

    anunciar(prep, ya);

    let mut fichero_parejas = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&ruta_parejas)
        .map_err(|e| format!("no se pudo abrir '{}': {e}", ruta_parejas.display()))?;

    let aperturas = Arc::new(prep.aperturas.clone());
    let siguiente = Arc::new(AtomicUsize::new(ya));
    let parar = Arc::new(AtomicBool::new(false));
    let tope = prep.aperturas.len();

    let cfg_partida = ConfigPartida {
        limite: cfg.limite,
        max_plies: cfg.partidas.max_plies,
        margen: cfg.margen(),
        adjudicacion: cfg.adjudicacion,
    };

    let (tx, rx) = mpsc::channel::<Mensaje>();
    let inicio = std::time::Instant::now();
    let mut handles = Vec::new();
    for _ in 0..cfg.partidas.workers {
        let tx = tx.clone();
        let aperturas = Arc::clone(&aperturas);
        let siguiente = Arc::clone(&siguiente);
        let parar = Arc::clone(&parar);
        let cand = cfg.candidato.clone();
        let base = cfg.base.clone();
        let saludo = cfg.saludo();
        handles.push(thread::spawn(move || {
            trabajar(&tx, &aperturas, &siguiente, &parar, tope, &cand, &base, saludo, &cfg_partida);
        }));
    }
    drop(tx);

    let mut acumulado = Acumulado::default();
    for p in &hechas {
        acumulado.añadir(p);
    }
    let mut pendientes: BTreeMap<usize, Pareja> = BTreeMap::new();
    let mut esperado = ya;
    let mut error: Option<String> = None;
    let mut llr;
    let mut decision;

    // `recv()` en vez de `for ... in rx`: el bucle no puede consumir el
    // receptor, porque cerrarlo explícitamente después es lo que avisa a
    // los workers de que la tanda ha terminado.
    'recepcion: while let Ok(mensaje) = rx.recv() {
        match mensaje {
            Mensaje::Fallo(indice, motivo) => {
                error = Some(format!("pareja {indice}: {motivo}"));
                parar.store(true, Ordering::Relaxed);
                break 'recepcion;
            }
            Mensaje::Lista(pareja) => {
                pendientes.insert(pareja.indice, *pareja);
                while let Some(pareja) = pendientes.remove(&esperado) {
                    escribir_linea(&mut fichero_parejas, &pareja_a_json(&pareja).to_compact())?;
                    acumulado.añadir(&pareja);
                    hechas.push(pareja);
                    esperado += 1;

                    llr = acumulado.pentanomial.llr(&cfg.sprt);
                    decision = cfg.sprt.decide(llr);
                    progreso(&acumulado, esperado, tope, llr, decision, inicio.elapsed());
                    if decision.is_final() {
                        parar.store(true, Ordering::Relaxed);
                        break 'recepcion;
                    }
                }
            }
        }
    }

    parar.store(true, Ordering::Relaxed);
    drop(rx);
    for handle in handles {
        let _ = handle.join();
    }
    if !pendientes.is_empty() {
        println!(
            "({} parejas especulativas descartadas: se calcularon más allá del punto de parada \
             y no entran en la decisión)",
            pendientes.len()
        );
    }
    if let Some(motivo) = error {
        return Err(format!(
            "la tanda se detuvo por un fallo de comunicación con un motor: {motivo}\n\
             Lo ya escrito en parejas.jsonl es válido y la tanda se puede reanudar."
        ));
    }

    finalizar(prep, hechas, inicio.elapsed())
}

use std::thread;

enum Mensaje {
    Lista(Box<Pareja>),
    Fallo(usize, String),
}

#[allow(clippy::too_many_arguments)]
fn trabajar(
    tx: &mpsc::Sender<Mensaje>,
    aperturas: &[Apertura],
    siguiente: &AtomicUsize,
    parar: &AtomicBool,
    tope: usize,
    cand: &crate::config::MotorCfg,
    base: &crate::config::MotorCfg,
    saludo: Duration,
    cfg_partida: &ConfigPartida,
) {
    let mut candidato = match Motor::lanzar(&cand.ruta, &cand.nombre, cand.cwd.as_deref(), &cand.opciones, saludo) {
        Ok(m) => m,
        Err(e) => {
            let _ = tx.send(Mensaje::Fallo(usize::MAX, e));
            return;
        }
    };
    let mut rival = match Motor::lanzar(&base.ruta, &base.nombre, base.cwd.as_deref(), &base.opciones, saludo) {
        Ok(m) => m,
        Err(e) => {
            let _ = tx.send(Mensaje::Fallo(usize::MAX, e));
            return;
        }
    };

    loop {
        if parar.load(Ordering::Relaxed) {
            return;
        }
        let indice = siguiente.fetch_add(1, Ordering::Relaxed);
        if indice >= tope {
            return;
        }
        let fen = aperturas[indice].fen.clone();

        let mut juegos = Vec::with_capacity(2);
        // Las dos partidas de la pareja las juega el mismo worker: la
        // pareja es la unidad estadística y no puede quedar partida a la
        // mitad entre dos hilos.
        for candidato_con_blancas in [true, false] {
            if let Err(e) = candidato.nueva_partida().and_then(|()| rival.nueva_partida()) {
                let _ = tx.send(Mensaje::Fallo(indice, e));
                return;
            }
            let resultado = if candidato_con_blancas {
                arbitro::jugar_partida(&mut candidato, &mut rival, &fen, cfg_partida)
            } else {
                arbitro::jugar_partida(&mut rival, &mut candidato, &fen, cfg_partida)
            };
            match resultado {
                Ok(partida) => juegos.push(PartidaJugada { candidato_con_blancas, partida }),
                Err(e) => {
                    let _ = tx.send(Mensaje::Fallo(indice, e));
                    return;
                }
            }
        }

        let pareja = Pareja {
            indice,
            fen,
            juegos: [juegos[0].clone(), juegos[1].clone()],
        };
        if tx.send(Mensaje::Lista(Box::new(pareja))).is_err() {
            return; // el orquestador ya cerró: la tanda ha decidido.
        }
    }
}

fn anunciar(prep: &Preparado, ya: usize) {
    let cfg = &prep.cfg;
    let (lo, hi) = cfg.sprt.bounds();
    println!("=== {} ===", cfg.id);
    println!("candidato : {} ({})", cfg.candidato.nombre, cfg.candidato.ruta.display());
    println!("base      : {} ({})", cfg.base.nombre, cfg.base.ruta.display());
    println!("control   : {}{}", cfg.control(), if cfg.es_reproducible() { " (reproducible)" } else { " (depende de la máquina)" });
    println!(
        "libro     : {} — {} posiciones, semilla {}",
        cfg.libro.fichero.display(),
        prep.aperturas.len(),
        cfg.libro.semilla
    );
    println!(
        "SPRT      : H0 = {:+.1} Elo, H1 = {:+.1} Elo, alpha = {}, beta = {} → fronteras [{lo:+.3}, {hi:+.3}]",
        cfg.sprt.elo0, cfg.sprt.elo1, cfg.sprt.alpha, cfg.sprt.beta
    );
    println!(
        "recursos  : {} worker(s) → hasta {} CPUs ocupadas de {} disponibles",
        cfg.partidas.workers,
        cfg.partidas.workers,
        cpus_disponibles()
    );
    println!("salida    : {}", cfg.salida.display());
    println!("por jugar : {} parejas ({} partidas)", prep.aperturas.len() - ya, (prep.aperturas.len() - ya) * 2);
    println!();
}

fn progreso(
    acumulado: &Acumulado,
    parejas: usize,
    tope: usize,
    llr: f64,
    decision: Decision,
    transcurrido: Duration,
) {
    let elo = acumulado
        .pentanomial
        .elo_estimate()
        .and_then(|e| e.elo)
        .map(|v| format!("{v:+.1}"))
        .unwrap_or_else(|| "  n/d".to_string());
    let restantes = tope.saturating_sub(parejas);
    let eta = if parejas > 0 && restantes > 0 {
        let por_pareja = transcurrido.as_secs_f64() / parejas as f64;
        format!(" eta {}", duracion_corta(Duration::from_secs_f64(por_pareja * restantes as f64)))
    } else {
        String::new()
    };
    println!(
        "[{parejas:>4}/{tope}] {}-{}-{} (G-P-T)  Elo {elo}  LLR {llr:+.4} [{}]{eta}",
        acumulado.ganadas, acumulado.perdidas, acumulado.tablas, decision.as_str()
    );
}

fn duracion_corta(d: Duration) -> String {
    let s = d.as_secs();
    if s >= 3600 {
        format!("{}h{:02}m", s / 3600, (s % 3600) / 60)
    } else if s >= 60 {
        format!("{}m{:02}s", s / 60, s % 60)
    } else {
        format!("{s}s")
    }
}

fn finalizar(prep: &Preparado, parejas: Vec<Pareja>, duracion: Duration) -> Result<Salida, String> {
    let cfg = &prep.cfg;
    let mut acumulado = Acumulado::default();
    for p in &parejas {
        acumulado.añadir(p);
    }
    let llr = acumulado.pentanomial.llr(&cfg.sprt);
    let decision = cfg.sprt.decide(llr);

    let resumen = resumen_json(prep, &acumulado, llr, decision, duracion);
    escribir(&cfg.salida.join("resumen.json"), &resumen.to_pretty())?;
    escribir(&cfg.salida.join("partidas.pgn"), &pgn_de(prep, &parejas))?;

    imprimir_resumen(&cfg.id, &acumulado, &cfg.sprt, llr, decision);
    Ok(Salida { acumulado, decision })
}

fn resumen_json(
    prep: &Preparado,
    acumulado: &Acumulado,
    llr: f64,
    decision: Decision,
    duracion: Duration,
) -> Json {
    let cfg = &prep.cfg;
    let mut j = Json::obj();
    j.set("id", cfg.id.as_str().into());
    j.set("firma", prep.firma.as_str().into());
    j.set("generado", marca_de_tiempo().into());
    j.set("parejas", acumulado.pentanomial.pairs().into());
    j.set("partidas", acumulado.partidas().into());

    let mut wdl = Json::obj();
    wdl.set("ganadas", acumulado.ganadas.into());
    wdl.set("tablas", acumulado.tablas.into());
    wdl.set("perdidas", acumulado.perdidas.into());
    j.set("candidato", wdl);

    j.set(
        "pentanomial",
        Json::Arr(acumulado.pentanomial.bins.iter().map(|&b| Json::from(b)).collect()),
    );

    if let Some(est) = acumulado.pentanomial.elo_estimate() {
        let mut e = Json::obj();
        e.set("puntuacion", est.score.into());
        e.set("error_tipico", est.standard_error.into());
        e.set("elo", est.elo.map(Json::from).unwrap_or(Json::Null));
        e.set("elo_ic95_bajo", est.elo_low.map(Json::from).unwrap_or(Json::Null));
        e.set("elo_ic95_alto", est.elo_high.map(Json::from).unwrap_or(Json::Null));
        e.set("los", est.los.into());
        j.set("estimacion", e);
    }

    let (lo, hi) = cfg.sprt.bounds();
    let mut s = Json::obj();
    s.set("llr", llr.into());
    s.set("frontera_baja", lo.into());
    s.set("frontera_alta", hi.into());
    s.set("decision", decision.as_str().into());
    j.set("sprt", s);

    let mut finales = Json::obj();
    for (clave, valor) in &acumulado.finales {
        finales.set(clave, (*valor).into());
    }
    j.set("finales", finales);
    j.set("anomalas", acumulado.anomalas.into());
    j.set(
        "plies_por_partida",
        if acumulado.partidas() > 0 {
            Json::from(acumulado.plies as f64 / acumulado.partidas() as f64)
        } else {
            Json::Null
        },
    );
    j.set("segundos", duracion.as_secs_f64().into());
    j
}

fn imprimir_resumen(
    id: &str,
    acumulado: &Acumulado,
    sprt: &Sprt,
    llr: f64,
    decision: Decision,
) {
    let (lo, hi) = sprt.bounds();
    println!();
    println!("--- {id} ---");
    println!(
        "Partidas: {}  ({} parejas)",
        acumulado.partidas(),
        acumulado.pentanomial.pairs()
    );
    println!(
        "Candidato: {} ganadas, {} tablas, {} perdidas",
        acumulado.ganadas, acumulado.tablas, acumulado.perdidas
    );
    println!(
        "Pentanomial [LL, LD, LW/DD, DW, WW]: {:?}",
        acumulado.pentanomial.bins
    );
    match acumulado.pentanomial.elo_estimate() {
        Some(est) => {
            println!("Puntuación: {:.2} %", est.score * 100.0);
            match (est.elo, est.elo_low, est.elo_high) {
                (Some(e), Some(l), Some(h)) => println!(
                    "Elo: {e:+.1}  (IC 95 %: {l:+.1} … {h:+.1})   LOS: {:.1} %",
                    est.los * 100.0
                ),
                _ => println!("Elo: no estimable (resultado en un extremo)"),
            }
        }
        None => println!("Sin datos suficientes para estimar."),
    }
    println!("LLR: {llr:+.4}   fronteras [{lo:+.3}, {hi:+.3}]   →  {}", decision.as_str());
    println!("{}", interpretacion(sprt, decision));
    if acumulado.anomalas > 0 {
        println!();
        println!(
            "AVISO: {} partida(s) terminaron de forma anómala (incomparecencia o jugada ilegal). \
             Eso no es un resultado de ajedrez: hay un motor que falla o una máquina saturada, y \
             la cifra de Elo de arriba no es interpretable hasta explicarlo. Mira 'finales' en \
             resumen.json y las partidas correspondientes en partidas.pgn.",
            acumulado.anomalas
        );
    }
    let sin_jugadas = acumulado.finales.get("tope_de_plies").copied().unwrap_or(0);
    if sin_jugadas * 5 > acumulado.partidas() {
        println!();
        println!(
            "Nota: {sin_jugadas} partidas llegaron al tope de plies y se anotaron como tablas. \
             Con esa proporción conviene revisar 'partidas.max_plies' o la adjudicación de tablas."
        );
    }
}

fn interpretacion(sprt: &Sprt, decision: Decision) -> String {
    match decision {
        Decision::AceptaH1 => format!(
            "Evidencia a favor de H1 ({:+.1} Elo) frente a H0 ({:+.1} Elo): el cambio se acepta.",
            sprt.elo1, sprt.elo0
        ),
        Decision::AceptaH0 => format!(
            "Evidencia a favor de H0 ({:+.1} Elo) frente a H1 ({:+.1} Elo): el cambio no llega a \
             lo que se le pedía. No demuestra que empeore, demuestra que no mejora lo suficiente.",
            sprt.elo0, sprt.elo1
        ),
        Decision::Continuar => {
            "Sin decisión: no se ha cruzado ninguna frontera. No es 'casi'; es que aún no se sabe. \
             Ampliar el libro y seguir, o abrir un experimento nuevo con más parejas."
                .to_string()
        }
    }
}

// ------------------------------------------------------------- persistencia

fn pgn_de(prep: &Preparado, parejas: &[Pareja]) -> String {
    let cfg = &prep.cfg;
    let fecha = fecha_pgn();
    let mut out = String::new();
    for pareja in parejas {
        for (n, juego) in pareja.juegos.iter().enumerate() {
            let (blancas, negras) = if juego.candidato_con_blancas {
                (cfg.candidato.nombre.clone(), cfg.base.nombre.clone())
            } else {
                (cfg.base.nombre.clone(), cfg.candidato.nombre.clone())
            };
            let meta = pgn::Meta {
                evento: cfg.id.clone(),
                sitio: format!("banco ({})", std::env::consts::OS),
                fecha: fecha.clone(),
                ronda: format!("{}.{}", pareja.indice + 1, n + 1),
                blancas,
                negras,
                control: cfg.control(),
            };
            out.push_str(&pgn::escribir(&juego.partida, &meta));
        }
    }
    out
}

fn pareja_a_json(pareja: &Pareja) -> Json {
    let mut j = Json::obj();
    j.set("indice", pareja.indice.into());
    j.set("fen", pareja.fen.as_str().into());
    j.set("cajon", pareja.cajon().into());
    let juegos: Vec<Json> = pareja
        .juegos
        .iter()
        .map(|juego| {
            let mut g = Json::obj();
            g.set("candidato_blancas", juego.candidato_con_blancas.into());
            g.set("resultado", juego.partida.resultado.pgn().into());
            g.set("final", juego.partida.final_partida.as_str().into());
            let jugadas: Vec<Json> = juego
                .partida
                .jugadas
                .iter()
                .map(|m| {
                    let mut x = Json::obj();
                    x.set("uci", m.uci.as_str().into());
                    x.set("san", m.san.as_str().into());
                    match m.score {
                        Some(Score::Cp(cp)) => {
                            x.set("cp", cp.into());
                        }
                        Some(Score::Mate(n)) => {
                            x.set("mate", n.into());
                        }
                        None => {}
                    }
                    if let Some(d) = m.depth {
                        x.set("d", u64::from(d).into());
                    }
                    x.set("ms", m.ms.into());
                    x
                })
                .collect();
            g.set("jugadas", Json::Arr(jugadas));
            g
        })
        .collect();
    j.set("juegos", Json::Arr(juegos));
    j
}

fn pareja_desde_json(j: &Json, linea: usize) -> Result<Pareja, String> {
    let err = |m: &str| format!("parejas.jsonl:{linea}: {m}");
    let indice = j
        .get("indice")
        .and_then(|v| v.as_u64())
        .ok_or_else(|| err("falta 'indice'"))? as usize;
    let fen = j
        .get("fen")
        .and_then(|v| v.as_str())
        .ok_or_else(|| err("falta 'fen'"))?
        .to_string();
    let juegos_json = j
        .get("juegos")
        .and_then(|v| v.as_array())
        .ok_or_else(|| err("falta 'juegos'"))?;
    if juegos_json.len() != 2 {
        return Err(err("una pareja son exactamente dos partidas"));
    }

    let mut juegos = Vec::with_capacity(2);
    for g in juegos_json {
        let candidato_con_blancas = g
            .get("candidato_blancas")
            .and_then(|v| v.as_bool())
            .ok_or_else(|| err("falta 'candidato_blancas'"))?;
        let resultado = match g.get("resultado").and_then(|v| v.as_str()) {
            Some("1-0") => Resultado::GananBlancas,
            Some("0-1") => Resultado::GananNegras,
            Some("1/2-1/2") => Resultado::Tablas,
            _ => return Err(err("'resultado' desconocido")),
        };
        let final_partida = final_desde_str(g.get("final").and_then(|v| v.as_str()).unwrap_or(""))
            .ok_or_else(|| err("'final' desconocido"))?;
        let jugadas = g
            .get("jugadas")
            .and_then(|v| v.as_array())
            .ok_or_else(|| err("falta 'jugadas'"))?
            .iter()
            .map(|x| arbitro::JugadaRegistrada {
                uci: x.get("uci").and_then(|v| v.as_str()).unwrap_or("0000").to_string(),
                san: x.get("san").and_then(|v| v.as_str()).unwrap_or("?").to_string(),
                score: match (x.get("cp").and_then(|v| v.as_i64()), x.get("mate").and_then(|v| v.as_i64())) {
                    (Some(cp), _) => Some(Score::Cp(cp as i32)),
                    (_, Some(m)) => Some(Score::Mate(m as i32)),
                    _ => None,
                },
                depth: x.get("d").and_then(|v| v.as_u64()).map(|d| d as u32),
                ms: x.get("ms").and_then(|v| v.as_u64()).unwrap_or(0),
            })
            .collect();
        juegos.push(PartidaJugada {
            candidato_con_blancas,
            partida: Partida {
                resultado,
                final_partida,
                fen_inicial: fen.clone(),
                jugadas,
            },
        });
    }

    let pareja = Pareja { indice, fen, juegos: [juegos[0].clone(), juegos[1].clone()] };
    // El cajón se recalcula siempre; el guardado solo sirve para detectar
    // un fichero manipulado o corrompido.
    if let Some(guardado) = j.get("cajon").and_then(|v| v.as_u64()) {
        if guardado as usize != pareja.cajon() {
            return Err(err(&format!(
                "el cajón guardado ({guardado}) no coincide con los resultados ({})",
                pareja.cajon()
            )));
        }
    }
    Ok(pareja)
}

fn final_desde_str(s: &str) -> Option<Final> {
    let todos = [
        Final::Mate,
        Final::Ahogado,
        Final::CincuentaJugadas,
        Final::RepeticionTriple,
        Final::MaterialInsuficiente,
        Final::AbandonoAdjudicado,
        Final::TablasAdjudicadas,
        Final::TopeDePlies,
        Final::Incomparecencia,
        Final::JugadaIlegal,
        Final::TiempoAgotado,
    ];
    todos.into_iter().find(|f| f.as_str() == s)
}

pub fn leer_parejas(ruta: &Path) -> Result<Vec<Pareja>, String> {
    if !ruta.exists() {
        return Ok(Vec::new());
    }
    let texto = std::fs::read_to_string(ruta)
        .map_err(|e| format!("no se pudo leer '{}': {e}", ruta.display()))?;
    let mut parejas = Vec::new();
    for (n, linea) in texto.lines().enumerate() {
        let linea = linea.trim();
        if linea.is_empty() {
            continue;
        }
        let json = crate::json::parse(linea).map_err(|e| format!("parejas.jsonl:{}: {e}", n + 1))?;
        let pareja = pareja_desde_json(&json, n + 1)?;
        if pareja.indice != parejas.len() {
            return Err(format!(
                "parejas.jsonl:{}: se esperaba la pareja {} y hay la {}. El fichero debe ser un \
                 prefijo contiguo; un hueco significa que la decisión secuencial no se puede \
                 reconstruir.",
                n + 1,
                parejas.len(),
                pareja.indice
            ));
        }
        parejas.push(pareja);
    }
    Ok(parejas)
}

fn leer_json(ruta: &Path) -> Result<Json, String> {
    let texto = std::fs::read_to_string(ruta)
        .map_err(|e| format!("no se pudo leer '{}': {e}", ruta.display()))?;
    crate::json::parse(&texto).map_err(|e| format!("{}: {e}", ruta.display()))
}

fn escribir(ruta: &Path, contenido: &str) -> Result<(), String> {
    let mut f = File::create(ruta)
        .map_err(|e| format!("no se pudo escribir '{}': {e}", ruta.display()))?;
    f.write_all(contenido.as_bytes())
        .map_err(|e| format!("no se pudo escribir '{}': {e}", ruta.display()))
}

/// Cada pareja se vacía a disco en cuanto se cierra. UCI no da forma de
/// atrapar un Ctrl+C limpiamente desde aquí, así que la garantía es esta:
/// lo que está en el fichero está completo y se puede reanudar.
fn escribir_linea(f: &mut File, linea: &str) -> Result<(), String> {
    writeln!(f, "{linea}").map_err(|e| format!("no se pudo anotar la pareja: {e}"))?;
    f.flush().map_err(|e| format!("no se pudo vaciar el fichero de parejas: {e}"))
}

// ---------------------------------------------------------------- informe

/// Recalcula resumen y PGN a partir de `parejas.jsonl`, verificando de paso
/// que el fichero es un prefijo contiguo y coherente.
pub fn informe(dir: &Path) -> Result<(), String> {
    let manifiesto = leer_json(&dir.join("manifiesto.json"))?;
    let config_json = manifiesto
        .get("config")
        .ok_or_else(|| "el manifiesto no lleva 'config'".to_string())?;
    let parejas = leer_parejas(&dir.join("parejas.jsonl"))?;

    let sprt = Sprt {
        elo0: campo_f64(config_json, "sprt", "elo0")?,
        elo1: campo_f64(config_json, "sprt", "elo1")?,
        alpha: campo_f64(config_json, "sprt", "alpha")?,
        beta: campo_f64(config_json, "sprt", "beta")?,
    };
    let id = manifiesto.get("id").and_then(|v| v.as_str()).unwrap_or("(sin id)");

    let mut acumulado = Acumulado::default();
    for p in &parejas {
        acumulado.añadir(p);
    }
    let llr = acumulado.pentanomial.llr(&sprt);
    let decision = sprt.decide(llr);
    imprimir_resumen(id, &acumulado, &sprt, llr, decision);

    // Reconstruir el PGN necesita los nombres y el control, que están en la
    // configuración normalizada del propio manifiesto.
    let nombre = |cual: &str| {
        config_json
            .get(cual)
            .and_then(|m| m.get("nombre"))
            .and_then(|v| v.as_str())
            .unwrap_or(cual)
            .to_string()
    };
    let control = config_json
        .get("busqueda")
        .map(|b| match b.get("tipo").and_then(|v| v.as_str()) {
            Some("nodos") => format!("{} nodos", b.get("nodos").and_then(|v| v.as_u64()).unwrap_or(0)),
            Some("movetime") => format!("{} ms/jugada", b.get("movetime_ms").and_then(|v| v.as_u64()).unwrap_or(0)),
            Some("profundidad") => format!("profundidad {}", b.get("profundidad").and_then(|v| v.as_u64()).unwrap_or(0)),
            _ => "reloj".to_string(),
        })
        .unwrap_or_else(|| "?".to_string());

    let fecha = fecha_pgn();
    let mut out = String::new();
    for pareja in &parejas {
        for (n, juego) in pareja.juegos.iter().enumerate() {
            let (blancas, negras) = if juego.candidato_con_blancas {
                (nombre("candidato"), nombre("base"))
            } else {
                (nombre("base"), nombre("candidato"))
            };
            let meta = pgn::Meta {
                evento: id.to_string(),
                sitio: format!("banco ({})", std::env::consts::OS),
                fecha: fecha.clone(),
                ronda: format!("{}.{}", pareja.indice + 1, n + 1),
                blancas,
                negras,
                control: control.clone(),
            };
            out.push_str(&pgn::escribir(&juego.partida, &meta));
        }
    }
    escribir(&dir.join("partidas.pgn"), &out)?;
    println!();
    println!("Regenerado {}", dir.join("partidas.pgn").display());
    Ok(())
}

fn campo_f64(config: &Json, objeto: &str, clave: &str) -> Result<f64, String> {
    config
        .get(objeto)
        .and_then(|o| o.get(clave))
        .and_then(|v| v.as_f64())
        .ok_or_else(|| format!("el manifiesto no lleva 'config.{objeto}.{clave}'"))
}

// ------------------------------------------------------------------- fechas

fn segundos_desde_epoca() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

/// Algoritmo civil-from-days de Howard Hinnant. Se escribe a mano porque el
/// banco no tiene dependencias y `std` no sabe convertir un instante en una
/// fecha del calendario.
fn fecha_civil(dias: i64) -> (i64, u32, u32) {
    let z = dias + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

pub fn fecha_pgn() -> String {
    let segundos = segundos_desde_epoca();
    let (y, m, d) = fecha_civil((segundos / 86_400) as i64);
    format!("{y:04}.{m:02}.{d:02}")
}

pub fn marca_de_tiempo() -> String {
    let segundos = segundos_desde_epoca();
    let (y, m, d) = fecha_civil((segundos / 86_400) as i64);
    let resto = segundos % 86_400;
    format!(
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z",
        resto / 3600,
        (resto % 3600) / 60,
        resto % 60
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::arbitro::JugadaRegistrada;

    fn partida(resultado: Resultado, final_partida: Final) -> Partida {
        Partida {
            resultado,
            final_partida,
            fen_inicial: arbitro::FEN_INICIAL.to_string(),
            jugadas: vec![JugadaRegistrada {
                uci: "e2e4".to_string(),
                san: "e4".to_string(),
                score: Some(Score::Cp(25)),
                depth: Some(9),
                ms: 40,
            }],
        }
    }

    fn pareja(indice: usize, r1: Resultado, r2: Resultado) -> Pareja {
        Pareja {
            indice,
            fen: arbitro::FEN_INICIAL.to_string(),
            juegos: [
                PartidaJugada { candidato_con_blancas: true, partida: partida(r1, Final::Mate) },
                PartidaJugada { candidato_con_blancas: false, partida: partida(r2, Final::Mate) },
            ],
        }
    }

    #[test]
    fn the_candidate_scores_from_the_side_it_actually_played() {
        // Gana el blanco en las dos partidas: el candidato gana la primera
        // (jugaba con blancas) y pierde la segunda.
        let p = pareja(0, Resultado::GananBlancas, Resultado::GananBlancas);
        assert_eq!(p.juegos[0].puntos_candidato(), 1.0);
        assert_eq!(p.juegos[1].puntos_candidato(), 0.0);
        assert_eq!(p.cajon(), 2);
    }

    #[test]
    fn sweeping_both_games_lands_in_the_top_bin() {
        let p = pareja(0, Resultado::GananBlancas, Resultado::GananNegras);
        assert_eq!(p.cajon(), 4);
        let q = pareja(0, Resultado::GananNegras, Resultado::GananBlancas);
        assert_eq!(q.cajon(), 0);
    }

    #[test]
    fn a_pair_round_trips_through_its_json_line() {
        let p = pareja(7, Resultado::Tablas, Resultado::GananNegras);
        let texto = pareja_a_json(&p).to_compact();
        let vuelta = pareja_desde_json(&crate::json::parse(&texto).unwrap(), 1).unwrap();
        assert_eq!(vuelta.indice, 7);
        assert_eq!(vuelta.cajon(), p.cajon());
        assert_eq!(vuelta.juegos[0].partida.jugadas[0].san, "e4");
        assert_eq!(vuelta.juegos[1].partida.final_partida, Final::Mate);
    }

    #[test]
    fn a_tampered_bin_is_detected_when_reading_back() {
        let p = pareja(0, Resultado::Tablas, Resultado::Tablas);
        let texto = pareja_a_json(&p).to_compact().replace("\"cajon\":2", "\"cajon\":4");
        let err = pareja_desde_json(&crate::json::parse(&texto).unwrap(), 3).unwrap_err();
        assert!(err.contains("no coincide"), "{err}");
    }

    #[test]
    fn every_termination_name_survives_a_round_trip() {
        for f in [
            Final::Mate,
            Final::Ahogado,
            Final::CincuentaJugadas,
            Final::RepeticionTriple,
            Final::MaterialInsuficiente,
            Final::AbandonoAdjudicado,
            Final::TablasAdjudicadas,
            Final::TopeDePlies,
            Final::Incomparecencia,
            Final::JugadaIlegal,
            Final::TiempoAgotado,
        ] {
            assert_eq!(final_desde_str(f.as_str()), Some(f), "{}", f.as_str());
        }
        assert_eq!(final_desde_str("inventado"), None);
    }

    #[test]
    fn the_accumulator_counts_games_from_the_candidate_side() {
        let mut a = Acumulado::default();
        a.añadir(&pareja(0, Resultado::GananBlancas, Resultado::GananNegras)); // 1 + 1
        a.añadir(&pareja(1, Resultado::Tablas, Resultado::Tablas)); // ½ + ½
        a.añadir(&pareja(2, Resultado::GananNegras, Resultado::GananBlancas)); // 0 + 0
        assert_eq!(a.ganadas, 2);
        assert_eq!(a.tablas, 2);
        assert_eq!(a.perdidas, 2);
        assert_eq!(a.pentanomial.bins, [1, 0, 1, 0, 1]);
        assert_eq!(a.partidas(), 6);
    }

    #[test]
    fn the_signature_ignores_worker_count_but_not_the_search_limit() {
        let base = r#"{
            "esquema": 1, "id": "x",
            "candidato": {"nombre": "c", "ruta": "a.exe"},
            "base": {"nombre": "b", "ruta": "b.exe"},
            "libro": {"fichero": "l.epd", "semilla": 1},
            "busqueda": {"nodos": 3000},
            "partidas": {"max_parejas": 8},
            "sprt": {"elo0": 0.0, "elo1": 5.0, "alpha": 0.05, "beta": 0.05},
            "salida": "out"
        }"#;
        let firma_de = |texto: &str| {
            let cfg = crate::config::desde_texto(texto).unwrap();
            calcular_firma(&cfg.normalizado, "sha_c", "sha_b", "sha_l")
        };
        let original = firma_de(base);
        // Cambiar workers, márgenes o el número de parejas no cambia qué se
        // mide: la reanudación debe seguir siendo válida.
        let mas_workers = base.replace(
            "\"partidas\": {\"max_parejas\": 8}",
            "\"partidas\": {\"max_parejas\": 64, \"workers\": 6, \"margen_ms\": 30000}",
        );
        assert_eq!(firma_de(&mas_workers), original);
        // Cambiar el control de búsqueda o una hipótesis sí.
        assert_ne!(firma_de(&base.replace("3000", "4000")), original);
        assert_ne!(firma_de(&base.replace("\"elo1\": 5.0", "\"elo1\": 3.0")), original);
        assert_ne!(firma_de(&base.replace("\"semilla\": 1", "\"semilla\": 2")), original);
        // Y cambiar un binario, evidentemente.
        let cfg = crate::config::desde_texto(base).unwrap();
        assert_ne!(calcular_firma(&cfg.normalizado, "otro", "sha_b", "sha_l"), original);
    }

    #[test]
    fn dates_are_converted_without_a_calendar_library() {
        assert_eq!(fecha_civil(0), (1970, 1, 1));
        assert_eq!(fecha_civil(19_723), (2024, 1, 1));
        assert_eq!(fecha_civil(19_782), (2024, 2, 29)); // año bisiesto
        assert_eq!(fecha_civil(20_683), (2026, 8, 18));
    }

    #[test]
    fn short_durations_read_as_something_a_human_can_use() {
        assert_eq!(duracion_corta(Duration::from_secs(45)), "45s");
        assert_eq!(duracion_corta(Duration::from_secs(125)), "2m05s");
        assert_eq!(duracion_corta(Duration::from_secs(7_320)), "2h02m");
    }
}
