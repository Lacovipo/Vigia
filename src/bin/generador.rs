//! `generador`: los datos de la red NNUE, sacados de Vigía mismo.
//!
//! Hoy hace una sola cosa, la primera que necesita el plan:
//!
//! ```text
//! generador indices --semilla N --salida tools/nnue/indices.txt
//! ```
//!
//! Escribe el **sentido A** del vector dorado (§7.1 de `docs/PlanNNUE.md`):
//! 4.096 posiciones y, por cada una, la lista ordenada de rasgos activos de las
//! dos perspectivas, calculada con el código del motor. `tools/nnue/golden.py`
//! recalcula esos índices con su propia implementación y falla si difiere una
//! sola entrada. Así el índice de rasgo, la orientación de la perspectiva, la
//! distinción propia/ajena y los cuatro rasgos de enroque quedan anclados por
//! el lado que no puede fabricar el entrenador.
//!
//! La generación de corpus de la fase 2 vivirá aquí también.
//!
//! Las posiciones salen de partidas aleatorias desde la inicial con una semilla
//! fija, sesgadas hacia las capturas para llegar a finales, y repartidas en
//! categorías con cupo: si un caso no está en el dorado, no está probado.

use std::collections::HashSet;
use std::process::ExitCode;

use vigia::bitboard::Bitboard;
use vigia::board::Board;
use vigia::eval;
use vigia::movegen;
use vigia::nnue;
use vigia::types::{Color, PieceType};

const AYUDA: &str = "\
generador — datos de la red NNUE de Vigía

USO
  generador indices --semilla N --salida <fichero>
      Posiciones de referencia y sus rasgos activos (sentido A del vector
      dorado). Ver docs/PlanNNUE.md §7.1.";

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

/// xorshift64*, para que la misma semilla dé siempre el mismo fichero.
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

fn clave(fen: &str) -> String {
    // Los rasgos solo ven colocación, turno y enroques; los relojes no.
    fen.split_whitespace().take(3).collect::<Vec<_>>().join(" ")
}

fn linea(categoria: &str, board: &Board) -> String {
    let rasgos = |perspectiva| {
        nnue::active_features(board, perspectiva).iter().map(|f| f.to_string()).collect::<Vec<_>>().join(" ")
    };
    format!("{categoria}|{}|{}|{}", board.to_fen(), rasgos(Color::White), rasgos(Color::Black))
}

fn indices(args: &[String]) -> Result<(), String> {
    let mut semilla = None;
    let mut salida = None;
    let mut i = 0;
    while i < args.len() {
        let valor = args.get(i + 1).ok_or_else(|| format!("'{}' necesita un valor", args[i]))?;
        match args[i].as_str() {
            "--semilla" => semilla = Some(valor.parse::<u64>().map_err(|_| format!("'--semilla {valor}' no es un número"))?),
            "--salida" => salida = Some(valor.clone()),
            otra => return Err(format!("opción desconocida '{otra}'\n\n{AYUDA}")),
        }
        i += 2;
    }
    let semilla = semilla.ok_or("falta --semilla")?;
    let salida = salida.ok_or("falta --salida")?;

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

            if !vistas.insert(clave(&board.to_fen())) {
                continue;
            }
            for (cubo, (categoria, cupo)) in cubos.iter_mut().zip(CATEGORIAS) {
                if cubo.len() < cupo && encaja(categoria, &board) {
                    cubo.push(linea(categoria, &board));
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
        lineas_humo.push(linea("humo", &board));
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

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let resultado = match args.first().map(String::as_str) {
        Some("indices") => indices(&args[1..]),
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
