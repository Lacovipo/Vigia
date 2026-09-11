//! `generador`: los datos de la red NNUE, sacados de Vigía mismo.
//!
//! ```text
//! generador indices --semilla N --salida tools/nnue/indices.txt
//! generador datos   --aperturas <fichero> --excluir banco/libros --posiciones N
//!                   --semilla N --salida <directorio> [--nodos 25000] [--hilos 1]
//! ```
//!
//! **`indices`** escribe el sentido A del vector dorado (§7.1 de
//! `docs/PlanNNUE.md`): 4.096 posiciones y, por cada una, la lista ordenada de
//! rasgos activos de las dos perspectivas, calculada con el código del motor.
//! `tools/nnue/golden.py` los recalcula con su propia implementación y falla si
//! difiere una sola entrada.
//!
//! **`datos`** genera el corpus de entrenamiento (fase 2 del plan): Vigía juega
//! contra sí mismo con un presupuesto de nodos, sin adjudicación, hasta que la
//! partida termina de verdad, y cada posición queda anotada con la puntuación
//! de su búsqueda, la jugada elegida y el resultado final.
//!
//! Las aperturas salen de un fichero de posiciones (una FEN o EPD por línea),
//! no de jugadas al azar desde la inicial como proponía el plan. Con 3,8
//! millones de posiciones realistas cada partida empieza en una distinta, y lo
//! que da información a la red, según la medición de §5.5 del plan, es la
//! variedad entre partidas. Jugadas al azar dan aperturas que no aparecen en
//! ninguna partida real: capacidad de la red gastada en posiciones que nunca va
//! a ver. Las etiquetas siguen siendo de Vigía; la apertura solo decide dónde
//! empieza cada partida.
//!
//! **Las posiciones de los libros del banco se excluyen.** Esos libros se
//! muestrearon del mismo fichero, y entrenar sobre ellos sería entrenar sobre
//! las aperturas con las que luego se mide la red.

use std::collections::HashSet;
use std::fs::File;
use std::io::{BufWriter, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use vigia::bitboard::Bitboard;
use vigia::board::Board;
use vigia::eval;
use vigia::movegen;
use vigia::nnue;
use vigia::rules::{self, GameEnd};
use vigia::search::{self, SearchLimits, Tt};
use vigia::sha256::Sha256;
use vigia::types::{Color, Move, PieceType};

const AYUDA: &str = "\
generador — datos de la red NNUE de Vigía

USO
  generador indices --semilla N --salida <fichero>
      Posiciones de referencia y sus rasgos activos (sentido A del vector
      dorado). Ver docs/PlanNNUE.md §7.1.

  generador datos --aperturas <fichero> --excluir <directorio> --posiciones N
                  --semilla N --salida <directorio> [--nodos N] [--hilos N] [--fen si]
      Corpus de entrenamiento: autojuego a nodos fijos, sin adjudicación.
      --excluir apunta a los libros del banco, cuyas posiciones no se usan.
      Cada hilo ocupa una CPU y escribe hilo-NN.bin y hce-NN.bin; con --fen si,
      también fen-NN.txt con la FEN de cada registro, para verificar el formato.";

/// xorshift64*, para que la misma semilla dé siempre los mismos ficheros.
struct Rng(u64);

impl Rng {
    fn new(semilla: u64) -> Rng {
        Rng(semilla.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1)
    }

    fn siguiente(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    fn menor_que(&mut self, n: usize) -> usize {
        (self.siguiente() % n as u64) as usize
    }
}

/// Opciones `--clave valor`, cada una una sola vez.
fn opciones(args: &[String]) -> Result<Vec<(String, String)>, String> {
    let mut pares: Vec<(String, String)> = Vec::new();
    let mut i = 0;
    while i < args.len() {
        let clave = args[i].strip_prefix("--").ok_or_else(|| format!("se esperaba una opción '--algo', hay '{}'", args[i]))?;
        let valor = args.get(i + 1).ok_or_else(|| format!("'--{clave}' necesita un valor"))?;
        if pares.iter().any(|(k, _)| k == clave) {
            return Err(format!("'--{clave}' aparece dos veces"));
        }
        pares.push((clave.to_string(), valor.clone()));
        i += 2;
    }
    Ok(pares)
}

fn valor<'a>(pares: &'a [(String, String)], clave: &str) -> Option<&'a str> {
    pares.iter().find(|(k, _)| k == clave).map(|(_, v)| v.as_str())
}

fn numero(pares: &[(String, String)], clave: &str, defecto: Option<u64>) -> Result<u64, String> {
    match valor(pares, clave) {
        Some(v) => v.parse().map_err(|_| format!("'--{clave} {v}' no es un número")),
        None => defecto.ok_or_else(|| format!("falta --{clave}")),
    }
}

fn comprobar_claves(pares: &[(String, String)], validas: &[&str]) -> Result<(), String> {
    match pares.iter().find(|(k, _)| !validas.contains(&k.as_str())) {
        Some((k, _)) => Err(format!("opción desconocida '--{k}'\n\n{AYUDA}")),
        None => Ok(()),
    }
}

// ------------------------------------------------------------------ indices

/// Cupos por categoría, en orden de especificidad: una posición que encaja en
/// varias va a la primera con sitio. Suman 4.096 con las 8 del libro de humo.
const CATEGORIAS: [(&str, usize); 6] = [
    ("al_paso", 512),
    ("coronacion", 512),
    ("final", 1024),
    ("enroque", 512),
    ("desequilibrio", 504),
    ("general", 1024),
];
const MAX_PARTIDAS: usize = 500_000;
const MAX_PLIES: usize = 300;
const RANGO_2: u64 = 0x0000_0000_0000_FF00;
const RANGO_7: u64 = 0x00FF_0000_0000_0000;

fn encaja(categoria: &str, board: &Board) -> bool {
    match categoria {
        "al_paso" => board.en_passant.is_some(),
        "coronacion" => {
            !(board.pieces_of(Color::White, PieceType::Pawn) & Bitboard(RANGO_7)).is_empty()
                || !(board.pieces_of(Color::Black, PieceType::Pawn) & Bitboard(RANGO_2)).is_empty()
        }
        "final" => board.occupied().count() <= 8,
        "enroque" => board.castling.0 != 0,
        "desequilibrio" => eval::material_score(board).abs() >= 300,
        _ => true,
    }
}

/// Colocación, turno, enroques y al paso: lo que identifica una posición. Los
/// relojes no cuentan. Es la misma identidad que usa el banco para sus libros.
fn identidad(fen: &str) -> String {
    fen.split_whitespace().take(4).collect::<Vec<_>>().join(" ")
}

fn linea_indices(categoria: &str, board: &Board) -> String {
    let rasgos = |perspectiva| {
        nnue::active_features(board, perspectiva).iter().map(|f| f.to_string()).collect::<Vec<_>>().join(" ")
    };
    format!("{categoria}|{}|{}|{}", board.to_fen(), rasgos(Color::White), rasgos(Color::Black))
}

fn indices(args: &[String]) -> Result<(), String> {
    let pares = opciones(args)?;
    comprobar_claves(&pares, &["semilla", "salida"])?;
    let semilla = numero(&pares, "semilla", None)?;
    let salida = valor(&pares, "salida").ok_or("falta --salida")?.to_string();

    let mut rng = Rng::new(semilla);
    let mut vistas = HashSet::new();
    let mut cubos: Vec<Vec<String>> = vec![Vec::new(); CATEGORIAS.len()];
    let llenas = |cubos: &Vec<Vec<String>>| cubos.iter().zip(CATEGORIAS).all(|(c, (_, cupo))| c.len() >= cupo);

    let mut partidas = 0;
    while !llenas(&cubos) && partidas < MAX_PARTIDAS {
        partidas += 1;
        let mut board = Board::start_pos();
        for _ in 0..MAX_PLIES {
            let jugadas = movegen::generate_legal_moves(&board);
            if jugadas.is_empty() || eval::is_insufficient_material(&board) {
                break;
            }
            let capturas: Vec<_> = jugadas.iter().copied().filter(|m| m.is_capture()).collect();
            let jugada = if !capturas.is_empty() && rng.siguiente().is_multiple_of(2) {
                capturas[rng.menor_que(capturas.len())]
            } else {
                jugadas[rng.menor_que(jugadas.len())]
            };
            board.make_move(jugada);

            let fen = board.to_fen();
            if !vistas.insert(fen.split_whitespace().take(3).collect::<Vec<_>>().join(" ")) {
                continue;
            }
            for (cubo, (categoria, cupo)) in cubos.iter_mut().zip(CATEGORIAS) {
                if cubo.len() < cupo && encaja(categoria, &board) {
                    cubo.push(linea_indices(categoria, &board));
                    break;
                }
            }
        }
    }
    for (cubo, (categoria, cupo)) in cubos.iter().zip(CATEGORIAS) {
        if cubo.len() < cupo {
            return Err(format!("tras {partidas} partidas, '{categoria}' solo tiene {} de {cupo}", cubo.len()));
        }
    }

    let humo = std::fs::read_to_string("banco/libros/humo.epd").map_err(|e| format!("banco/libros/humo.epd: {e}"))?;
    let mut lineas_humo = Vec::new();
    for fen in humo.lines().map(str::trim).filter(|l| !l.is_empty() && !l.starts_with('#')) {
        let board = Board::from_fen(fen).map_err(|e| format!("humo.epd: {fen}: {e}"))?;
        lineas_humo.push(linea_indices("humo", &board));
    }

    let mut texto = format!(
        "# Rasgos activos de la red NNUE, calculados por Vigía: el sentido A del vector dorado.\n\
         # generador indices --semilla {semilla} — {partidas} partidas aleatorias.\n\
         # Una línea por posición: categoría|fen|rasgos vistos por las blancas|rasgos vistos por las negras\n"
    );
    for l in cubos.iter().flatten().chain(&lineas_humo) {
        texto.push_str(l);
        texto.push('\n');
    }
    std::fs::write(&salida, texto).map_err(|e| format!("{salida}: {e}"))?;
    let total = cubos.iter().map(Vec::len).sum::<usize>() + lineas_humo.len();
    println!("{salida}: {total} posiciones tras {partidas} partidas");
    Ok(())
}

// -------------------------------------------------------------------- datos

/// Registro fijo de 32 bytes por posición, §5.3 del plan, todo little-endian:
///
/// | bytes  | campo |
/// |--------|-------|
/// | 0..8   | u64 casillas ocupadas |
/// | 8..24  | un nibble por casilla ocupada en orden de bit ascendente, el bajo primero: tipo (0..5 = P N B R Q K) más 8 si es negra |
/// | 24     | meta: bit 0 mueven negras, bits 1-4 enroques (K Q k q), bit 5 hay al paso, bit 6 en jaque, bit 7 búsqueda incompleta |
/// | 25     | casilla al paso, o 64 |
/// | 26..28 | i16 puntuación de la búsqueda, desde el bando que mueve |
/// | 28..30 | u16 jugada elegida: origen, destino << 6, tipo << 12 (`MoveFlag`) |
/// | 30     | resultado para el bando que mueve: 0 pierde, 1 tablas, 2 gana |
/// | 31     | ply dentro de la partida, saturado a 255 |
///
/// Los bits 6 y 7 de `meta` no estaban en el plan. Dos de sus filtros —la
/// posición en jaque y la búsqueda incompleta— no se pueden deducir después
/// desde Python sin reimplementar ajedrez, y el entrenador no reimplementa
/// ajedrez. Se escribe todo en bruto y se filtra al entrenar: así probar otro
/// filtro no obliga a regenerar diez horas de datos.
const REGISTRO: usize = 32;
const CABECERA: usize = 128;
const MAGIA: &[u8; 8] = b"VIGIADT1";
const VERSION_FORMATO: u32 = 1;
/// Sin adjudicación: la partida acaba por mate, ahogado, repetición, 50
/// jugadas, material insuficiente o este tope, que cuenta como tablas.
const MAX_PLIES_PARTIDA: usize = 300;
/// Jugadas al azar tras la apertura del fichero, entre 0 y este número: un
/// poco más de variedad sin salirse de posiciones realistas.
const PLIES_ALEATORIOS_MAX: usize = 2;
/// Una apertura que la primera búsqueda ya ve decidida se descarta: produciría
/// una partida trivial.
const LIMITE_APERTURA_CP: i32 = 400;
const HASH_MB: usize = 16;

fn codificar(board: &Board, puntuacion: i32, jugada: Move, ply: usize, en_jaque: bool, completa: bool) -> [u8; REGISTRO] {
    let mut r = [0u8; REGISTRO];
    let ocupadas = board.occupied();
    r[0..8].copy_from_slice(&ocupadas.0.to_le_bytes());
    for (i, casilla) in ocupadas.enumerate() {
        let pieza = board.piece_at(casilla).expect("una casilla ocupada tiene que tener pieza");
        let nibble = pieza.kind as u8 | ((pieza.color as u8) << 3);
        r[8 + i / 2] |= nibble << (4 * (i % 2));
    }
    let mut meta = (board.side_to_move as u8) | (board.castling.0 << 1);
    if board.en_passant.is_some() {
        meta |= 1 << 5;
    }
    if en_jaque {
        meta |= 1 << 6;
    }
    if !completa {
        meta |= 1 << 7;
    }
    r[24] = meta;
    r[25] = board.en_passant.map_or(64, |casilla| casilla.0);
    let cp = puntuacion.clamp(i16::MIN as i32, i16::MAX as i32) as i16;
    r[26..28].copy_from_slice(&cp.to_le_bytes());
    let codigo = jugada.from.0 as u16 | ((jugada.to.0 as u16) << 6) | ((jugada.flag as u16) << 12);
    r[28..30].copy_from_slice(&codigo.to_le_bytes());
    // r[30], el resultado, se escribe cuando termina la partida.
    r[31] = ply.min(255) as u8;
    r
}

/// Cuatro bytes por registro, en paralelo: la escala de final que el motor
/// aplica a la red en esa posición, y el número de piezas. Lo escribe Rust con
/// el código del motor para que el entrenador no tenga que restatear la regla.
fn registro_hce(board: &Board) -> [u8; 4] {
    [nnue::training_scale(board), board.occupied().count() as u8, 0, 0]
}

fn cabecera(nodos: u64, semilla: u64, hilo: usize, sha_aperturas: &[u8; 32], sha_generador: &[u8; 32], registros: u64) -> [u8; CABECERA] {
    let mut h = [0u8; CABECERA];
    h[0..8].copy_from_slice(MAGIA);
    h[8..12].copy_from_slice(&VERSION_FORMATO.to_le_bytes());
    h[12..16].copy_from_slice(&(nodos as u32).to_le_bytes());
    h[16..24].copy_from_slice(&semilla.to_le_bytes());
    h[24..28].copy_from_slice(&(hilo as u32).to_le_bytes());
    h[28..60].copy_from_slice(sha_aperturas);
    h[60..92].copy_from_slice(sha_generador);
    h[92..100].copy_from_slice(&registros.to_le_bytes());
    h
}

fn sha256(bytes: &[u8]) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    hasher.finish()
}

/// El fichero de aperturas entero en memoria y el inicio de cada línea útil.
/// Un `Vec<String>` con 3,8 millones de cadenas cuesta varias veces más.
struct Aperturas {
    texto: String,
    inicios: Vec<u32>,
}

impl Aperturas {
    fn cargar(ruta: &Path) -> Result<(Aperturas, [u8; 32]), String> {
        let bytes = std::fs::read(ruta).map_err(|e| format!("{}: {e}", ruta.display()))?;
        if bytes.len() > u32::MAX as usize {
            return Err(format!("{}: más de 4 GB, no cabe en los índices de línea", ruta.display()));
        }
        let sha = sha256(&bytes);
        let texto = String::from_utf8(bytes).map_err(|_| format!("{}: no es UTF-8", ruta.display()))?;
        let mut inicios = Vec::new();
        let mut inicio = 0usize;
        for linea in texto.split_inclusive('\n') {
            let contenido = linea.trim();
            if !contenido.is_empty() && !contenido.starts_with('#') {
                inicios.push(inicio as u32);
            }
            inicio += linea.len();
        }
        if inicios.is_empty() {
            return Err(format!("{}: no hay ninguna posición", ruta.display()));
        }
        Ok((Aperturas { texto, inicios }, sha))
    }

    fn len(&self) -> usize {
        self.inicios.len()
    }

    fn fen(&self, i: usize) -> Option<String> {
        let linea = self.texto[self.inicios[i] as usize..].lines().next()?.trim();
        let campos: Vec<&str> = linea.split_whitespace().collect();
        match campos.len() {
            4 | 5 => Some(format!("{} 0 1", campos[..4].join(" "))),
            n if n >= 6 => Some(campos[..6].join(" ")),
            _ => None,
        }
    }
}

fn identidades_excluidas(directorio: &Path) -> Result<HashSet<String>, String> {
    let mut excluidas = HashSet::new();
    let entradas = std::fs::read_dir(directorio).map_err(|e| format!("{}: {e}", directorio.display()))?;
    for entrada in entradas {
        let ruta = entrada.map_err(|e| e.to_string())?.path();
        if ruta.extension().and_then(|e| e.to_str()) != Some("epd") {
            continue;
        }
        let texto = std::fs::read_to_string(&ruta).map_err(|e| format!("{}: {e}", ruta.display()))?;
        for linea in texto.lines().map(str::trim).filter(|l| !l.is_empty() && !l.starts_with('#')) {
            excluidas.insert(identidad(linea));
        }
    }
    Ok(excluidas)
}

/// Una partida de autojuego completa desde `board`. `None` si la apertura no
/// sirve: ya terminada, o decidida desde la primera búsqueda.
/// Registros, anexos de `hce` y, si se pidieron, las FEN de cada registro.
type Partida = (Vec<[u8; REGISTRO]>, Vec<[u8; 4]>, Vec<String>);

fn jugar_partida(mut board: Board, nodos: u64, tt: &Tt, stop: &AtomicBool, con_fen: bool) -> Option<Partida> {
    let mut historia = vec![board.hash];
    if rules::game_end(&board, &historia).is_some() {
        return None;
    }
    // Cada partida empieza con la tabla vacía: así su contenido depende solo
    // de la semilla y no del orden en que el hilo jugó las anteriores.
    tt.clear();

    let mut registros = Vec::with_capacity(160);
    let mut hce = Vec::with_capacity(160);
    let mut fens = Vec::new();
    let mut fin = None;
    for ply in 0..MAX_PLIES_PARTIDA {
        let limites = SearchLimits { max_nodes: Some(nodos), ..Default::default() };
        // La búsqueda añade ella misma la posición actual a su camino, así que
        // recibe la historia sin ella, igual que desde UCI.
        let resultado = search::search(&board, limites, stop, tt, &historia[..historia.len() - 1], |_, _| {});
        let jugada = resultado.best_move?;
        if ply == 0 && resultado.score.abs() >= LIMITE_APERTURA_CP {
            return None;
        }
        let en_jaque = movegen::is_in_check(&board, board.side_to_move);
        registros.push(codificar(&board, resultado.score, jugada, ply, en_jaque, resultado.complete));
        hce.push(registro_hce(&board));
        if con_fen {
            fens.push(board.to_fen());
        }

        board.make_move(jugada);
        historia.push(board.hash);
        if let Some(causa) = rules::game_end(&board, &historia) {
            fin = Some((causa, board.side_to_move));
            break;
        }
    }

    // Resultado desde las blancas: +1 ganan, 0 tablas (incluido el tope), -1 pierden.
    let blancas: i8 = match fin {
        Some((GameEnd::Checkmate, Color::White)) => -1,
        Some((GameEnd::Checkmate, Color::Black)) => 1,
        _ => 0,
    };
    for registro in &mut registros {
        let para_quien_mueve = if registro[24] & 1 == 0 { blancas } else { -blancas };
        registro[30] = (para_quien_mueve + 1) as u8;
    }
    Some((registros, hce, fens))
}

struct Configuracion {
    nodos: u64,
    semilla: u64,
    cupo: u64,
    salida: PathBuf,
    sha_aperturas: [u8; 32],
    sha_generador: [u8; 32],
    con_fen: bool,
}

#[derive(Default)]
struct Resumen {
    registros: u64,
    partidas: u64,
    descartadas: u64,
}

fn trabajador(
    hilo: usize,
    cfg: &Configuracion,
    aperturas: &Aperturas,
    excluidas: &HashSet<String>,
    contador: &AtomicU64,
) -> Result<Resumen, String> {
    let mut rng = Rng::new(cfg.semilla ^ (hilo as u64 + 1).wrapping_mul(0xD1B5_4A32_D192_ED03));
    let tt = Tt::new(HASH_MB);
    let stop = AtomicBool::new(false);

    let ruta_datos = cfg.salida.join(format!("hilo-{hilo:02}.bin"));
    let ruta_hce = cfg.salida.join(format!("hce-{hilo:02}.bin"));
    let abrir = |ruta: &Path| File::create(ruta).map(BufWriter::new).map_err(|e| format!("{}: {e}", ruta.display()));
    let mut datos = abrir(&ruta_datos)?;
    let mut hce = abrir(&ruta_hce)?;
    let mut fens = if cfg.con_fen { Some(abrir(&cfg.salida.join(format!("fen-{hilo:02}.txt")))?) } else { None };
    let provisional = cabecera(cfg.nodos, cfg.semilla, hilo, &cfg.sha_aperturas, &cfg.sha_generador, 0);
    let error = |e: std::io::Error| e.to_string();
    datos.write_all(&provisional).map_err(error)?;

    let mut resumen = Resumen::default();
    while resumen.registros < cfg.cupo {
        let Some(fen) = aperturas.fen(rng.menor_que(aperturas.len())) else { continue };
        if excluidas.contains(&identidad(&fen)) {
            continue;
        }
        let Ok(mut board) = Board::from_fen(&fen) else { continue };
        for _ in 0..rng.menor_que(PLIES_ALEATORIOS_MAX + 1) {
            let jugadas = movegen::generate_legal_moves(&board);
            if jugadas.is_empty() {
                break;
            }
            board.make_move(jugadas[rng.menor_que(jugadas.len())]);
        }
        match jugar_partida(board, cfg.nodos, &tt, &stop, cfg.con_fen) {
            None => resumen.descartadas += 1,
            Some((registros, anexos, lineas)) => {
                for (registro, anexo) in registros.iter().zip(&anexos) {
                    datos.write_all(registro).map_err(error)?;
                    hce.write_all(anexo).map_err(error)?;
                }
                if let Some(f) = fens.as_mut() {
                    for linea in &lineas {
                        writeln!(f, "{linea}").map_err(error)?;
                    }
                }
                resumen.registros += registros.len() as u64;
                resumen.partidas += 1;
                contador.fetch_add(registros.len() as u64, Ordering::Relaxed);
            }
        }
    }

    // La cabecera se reescribe al final con el número real de registros.
    let mut datos = datos.into_inner().map_err(|e| e.to_string())?;
    datos.seek(SeekFrom::Start(0)).map_err(error)?;
    datos
        .write_all(&cabecera(cfg.nodos, cfg.semilla, hilo, &cfg.sha_aperturas, &cfg.sha_generador, resumen.registros))
        .map_err(error)?;
    hce.flush().map_err(error)?;
    if let Some(mut f) = fens {
        f.flush().map_err(error)?;
    }
    Ok(resumen)
}

fn datos(args: &[String]) -> Result<(), String> {
    let pares = opciones(args)?;
    comprobar_claves(&pares, &["aperturas", "excluir", "posiciones", "semilla", "salida", "nodos", "hilos", "fen"])?;
    let ruta_aperturas = PathBuf::from(valor(&pares, "aperturas").ok_or("falta --aperturas")?);
    let excluir = PathBuf::from(valor(&pares, "excluir").ok_or("falta --excluir")?);
    let posiciones = numero(&pares, "posiciones", None)?;
    let semilla = numero(&pares, "semilla", None)?;
    let salida = PathBuf::from(valor(&pares, "salida").ok_or("falta --salida")?);
    let nodos = numero(&pares, "nodos", Some(25_000))?;
    let hilos = numero(&pares, "hilos", Some(1))? as usize;
    if hilos == 0 || posiciones == 0 {
        return Err("--hilos y --posiciones tienen que ser mayores que cero".into());
    }

    let inicio = Instant::now();
    let (aperturas, sha_aperturas) = Aperturas::cargar(&ruta_aperturas)?;
    let excluidas = identidades_excluidas(&excluir)?;
    let yo = std::env::current_exe().map_err(|e| e.to_string())?;
    let sha_generador = sha256(&std::fs::read(&yo).map_err(|e| format!("{}: {e}", yo.display()))?);
    std::fs::create_dir_all(&salida).map_err(|e| format!("{}: {e}", salida.display()))?;
    let cfg = Configuracion { nodos, semilla, cupo: posiciones.div_ceil(hilos as u64), salida, sha_aperturas, sha_generador, con_fen: valor(&pares, "fen") == Some("si") };
    println!(
        "{} aperturas, {} posiciones de los libros del banco excluidas, {hilos} hilos a {nodos} nodos, {} registros por hilo (cargado en {:.1} s)",
        aperturas.len(),
        excluidas.len(),
        cfg.cupo,
        inicio.elapsed().as_secs_f64()
    );

    let contador = AtomicU64::new(0);
    let terminados = AtomicUsize::new(0);
    let inicio = Instant::now();
    let objetivo = cfg.cupo * hilos as u64;
    let resultados: Vec<Result<Resumen, String>> = std::thread::scope(|s| {
        let (cfg, aperturas, excluidas, contador, terminados) = (&cfg, &aperturas, &excluidas, &contador, &terminados);
        let manejadores: Vec<_> = (0..hilos)
            .map(|hilo| {
                s.spawn(move || {
                    let resumen = trabajador(hilo, cfg, aperturas, excluidas, contador);
                    terminados.fetch_add(1, Ordering::Relaxed);
                    resumen
                })
            })
            .collect();
        let mut ultimo = Instant::now();
        while terminados.load(Ordering::Relaxed) < hilos {
            std::thread::sleep(Duration::from_millis(250));
            if ultimo.elapsed() >= Duration::from_secs(60) {
                ultimo = Instant::now();
                let hechos = contador.load(Ordering::Relaxed);
                let horas = inicio.elapsed().as_secs_f64() / 3600.0;
                let ritmo = hechos as f64 / horas.max(1e-9);
                let restante = objetivo.saturating_sub(hechos) as f64 / ritmo.max(1e-9);
                println!("  {hechos} de {objetivo} registros, {ritmo:.0} por hora, quedan {restante:.1} h");
            }
        }
        manejadores.into_iter().map(|m| m.join().unwrap_or_else(|_| Err("un hilo ha muerto".into()))).collect()
    });

    let mut total = Resumen::default();
    for resultado in resultados {
        let r = resultado?;
        total.registros += r.registros;
        total.partidas += r.partidas;
        total.descartadas += r.descartadas;
    }
    let horas = inicio.elapsed().as_secs_f64() / 3600.0;
    println!(
        "{} registros en {} partidas ({} aperturas descartadas), {:.1} plies por partida, {:.0} registros por hora por hilo, {:.2} h",
        total.registros,
        total.partidas,
        total.descartadas,
        total.registros as f64 / total.partidas.max(1) as f64,
        total.registros as f64 / horas.max(1e-9) / hilos as f64,
        horas
    );
    Ok(())
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let resultado = match args.first().map(String::as_str) {
        Some("indices") => indices(&args[1..]),
        Some("datos") => datos(&args[1..]),
        _ => Err(AYUDA.to_string()),
    };
    match resultado {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::from(2)
        }
    }
}
