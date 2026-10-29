//! Perfil de tiempo por fases (característica `perfil`, ver `src/perfil.rs`).
//!
//! Busca las doce posiciones de `banco velocidad` a profundidad fija, con la
//! red encendida y un hilo, mientras un muestreador anota en qué fase está la
//! búsqueda, y al final imprime el reparto. Los nodos salen idénticos a los del
//! motor normal: las guardas de fase no tocan la búsqueda.
//!
//! ```text
//! cargo build --release --features perfil --bin perfil --target-dir target-perfil -j 2
//! target-perfil/release/perfil.exe --profundidad 12 --rondas 3
//! ```

use std::sync::atomic::AtomicBool;
use std::time::{Duration, Instant};

use vigia::board::Board;
use vigia::perfil;
use vigia::search::{self, SearchLimits, Tt};

const POSICIONES: [(&str, &str); 12] = [
    ("inicial", "rnbqkbnr/pppppppp/8/8/8/8/PPPPPPPP/RNBQKBNR w KQkq - 0 1"),
    ("kiwipete", "r3k2r/p1ppqpb1/bn2pnp1/3PN3/1p2P3/2N2Q1p/PPPBBPPP/R3K2R w KQkq - 0 1"),
    ("final-torres", "8/2p5/3p4/KP5r/1R3p1k/8/4P1P1/8 w - - 0 1"),
    ("promocion", "n1n5/PPPk4/8/8/8/8/4Kppp/5N1N b - - 0 1"),
    ("siciliana", "r1bqkb1r/pp3ppp/2n1pn2/3p4/3P4/2NBPN2/PP3PPP/R1BQK2R w KQkq - 0 8"),
    ("cerrada", "r1bq1rk1/pp2nppp/2n1p3/2ppP3/3P4/P1PB1N2/2P2PPP/R1BQK2R w KQ - 1 10"),
    ("india-rey", "r1bq1rk1/ppp1npbp/3p1np1/3Pp3/2P1P3/2N2N1P/PP2BPP1/R1BQ1RK1 b - - 0 9"),
    ("mediojuego-tactico", "2rq1rk1/pb1nbppp/1p2pn2/2p5/2BP4/1QN1PN2/PP3PPP/R1B2RK1 w - - 0 12"),
    ("damas-cambiadas", "r4rk1/1bp2ppp/p1np1n2/1p2p3/4P3/1BPP1N1P/PP3PP1/R1B2RK1 w - - 0 13"),
    ("final-alfiles", "8/5pk1/6p1/3b3p/3P4/2B2P1P/6PK/8 w - - 0 40"),
    ("final-peones", "8/1p3pp1/p6p/3k4/3P4/P4P1P/1P4P1/4K3 w - - 0 35"),
    ("torre-y-peones", "8/5pk1/4p1p1/3pP2p/r2P3P/5PK1/6P1/2R5 w - - 0 33"),
];

fn valor(args: &[String], nombre: &str, defecto: u64) -> u64 {
    args.iter()
        .position(|a| a == nombre)
        .and_then(|i| args.get(i + 1))
        .and_then(|v| v.parse().ok())
        .unwrap_or(defecto)
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let profundidad = valor(&args, "--profundidad", 12) as u32;
    let hash_mb = valor(&args, "--hash", 32) as usize;
    let rondas = valor(&args, "--rondas", 1);
    let periodo_us = valor(&args, "--periodo-us", 20);

    let tt = Tt::new(hash_mb);
    let stop = AtomicBool::new(false);
    let mut nodos = 0u64;

    let muestreador = perfil::start_sampler(Duration::from_micros(periodo_us));
    let inicio = Instant::now();
    for _ in 0..rondas {
        for (_nombre, fen) in POSICIONES {
            tt.clear();
            let tablero = Board::from_fen(fen).expect("FEN de la tabla");
            let limites = SearchLimits { max_depth: Some(profundidad), use_nnue: true, ..Default::default() };
            nodos += search::search(&tablero, limites, &stop, &tt, &[], |_, _| {}).nodes;
        }
    }
    let transcurrido = inicio.elapsed();
    let cuentas = muestreador.finish();

    let total: u64 = cuentas.iter().sum();
    println!(
        "profundidad {profundidad}, {rondas} ronda(s): {nodos} nodos en {:.2} s ({:.0} nodos/s), {total} muestras",
        transcurrido.as_secs_f64(),
        nodos as f64 / transcurrido.as_secs_f64()
    );
    let mut orden: Vec<usize> = (0..perfil::PHASES).collect();
    orden.sort_by_key(|&i| std::cmp::Reverse(cuentas[i]));
    for i in orden {
        println!("{:>6.2} %  {}", 100.0 * cuentas[i] as f64 / total.max(1) as f64, perfil::NAMES[i]);
    }
}
