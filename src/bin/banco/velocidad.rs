//! Banco de velocidad: nodos y nodos/segundo a profundidad fija.
//!
//! No todas las mejoras se validan con Elo. Un cambio de **solo velocidad**
//! —magic bitboards, un generador más rápido, una reordenación de
//! estructuras— tiene una propiedad que el SPRT no sabe aprovechar: no debe
//! cambiar ni una sola decisión de la búsqueda. Eso es comprobable
//! directamente y con una muestra de doce posiciones, en segundos, en vez
//! de con miles de partidas.
//!
//! El criterio:
//!
//! - **Los nodos por posición deben ser idénticos.** Si difieren, el cambio
//!   no es de solo velocidad: altera lo que la búsqueda visita, y entonces
//!   sí hace falta pasarlo por el SPRT.
//! - **Los nodos/segundo deben subir.** Esa es la mejora.
//!
//! Un cambio que mueva los nodos y además sea más rápido puede seguir
//! siendo bueno; simplemente no se puede dar por bueno aquí.

use std::path::Path;
use std::time::Duration;

use crate::motor::{Limite, Motor};

/// Posiciones del banco. Mezcla apertura, medio juego táctico, posiciones
/// cerradas y finales, porque el reparto de tiempo entre generación de
/// jugadas, evaluación y quiescencia es muy distinto en cada fase y una
/// mejora puede ayudar en una y estorbar en otra.
pub const POSICIONES: &[(&str, &str)] = &[
    ("inicial", "rnbqkbnr/pppppppp/8/8/8/8/PPPPPPPP/RNBQKBNR w KQkq - 0 1"),
    ("kiwipete", "r3k2r/p1ppqpb1/bn2pnp1/3PN3/1p2P3/2N2Q1p/PPPBBPPP/R3K2R w KQkq - 0 1"),
    ("final-torres", "8/2p5/3p4/KP5r/1R3p1k/8/4P1P1/8 w - - 0 1"),
    ("promocion", "n1n5/PPPk4/8/8/8/8/4Kppp/5N1N b - - 0 1"),
    ("siciliana", "r1bqkb1r/pp3ppp/2n1pn2/3p4/3P4/2NBPN2/PP3PPP/R1BQK2R w KQkq - 0 8"),
    ("cerrada", "r1bq1rk1/pp2nppp/2n1p3/2ppP3/3P4/P1PB1N2/2P2PPP/R1BQK2R w KQ - 1 10"),
    ("india-rey", "r1bq1rk1/ppp1npbp/3p1np1/3Pp3/2P1P3/2N2N1P/PP2BPP1/R1BQ1RK1 b - - 0 9"),
    ("mediojuego-táctico", "2rq1rk1/pb1nbppp/1p2pn2/2p5/2BP4/1QN1PN2/PP3PPP/R1B2RK1 w - - 0 12"),
    ("damas-cambiadas", "r4rk1/1bp2ppp/p1np1n2/1p2p3/4P3/1BPP1N1P/PP3PP1/R1B2RK1 w - - 0 13"),
    ("final-alfiles", "8/5pk1/6p1/3b3p/3P4/2B2P1P/6PK/8 w - - 0 40"),
    ("final-peones", "8/1p3pp1/p6p/3k4/3P4/P4P1P/1P4P1/4K3 w - - 0 35"),
    ("torre-y-peones", "8/5pk1/4p1p1/3pP2p/r2P3P/5PK1/6P1/2R5 w - - 0 33"),
];

pub struct Medida {
    pub nombre: String,
    pub nodos: u64,
    pub ms: u64,
    pub jugada: String,
}

impl Medida {
    pub fn nps(&self) -> f64 {
        if self.ms == 0 {
            return 0.0;
        }
        self.nodos as f64 * 1000.0 / self.ms as f64
    }
}

pub fn medir(
    ruta: &Path,
    nombre: &str,
    opciones: &[(String, String)],
    profundidad: u32,
    margen: Duration,
) -> Result<Vec<Medida>, String> {
    let mut motor = Motor::lanzar(ruta, nombre, None, opciones, Duration::from_secs(10))?;
    let mut medidas = Vec::new();
    for (etiqueta, fen) in POSICIONES {
        // Tabla limpia en cada posición: si no, lo que se mide es el orden
        // en que se recorrieron, no la velocidad del motor.
        motor.nueva_partida()?;
        let respuesta = motor
            .pedir_jugada(Some(fen), &[], Limite::Profundidad(profundidad), margen)?
            .ok_or_else(|| format!("{nombre}: no contestó en '{etiqueta}' dentro del plazo"))?;
        medidas.push(Medida {
            nombre: (*etiqueta).to_string(),
            nodos: respuesta.nodes.unwrap_or(0),
            ms: respuesta.elapsed.as_millis() as u64,
            jugada: respuesta.uci,
        });
    }
    Ok(medidas)
}

pub fn total_nodos(medidas: &[Medida]) -> u64 {
    medidas.iter().map(|m| m.nodos).sum()
}

pub fn total_ms(medidas: &[Medida]) -> u64 {
    medidas.iter().map(|m| m.ms).sum()
}

pub fn nps_global(medidas: &[Medida]) -> f64 {
    let ms = total_ms(medidas);
    if ms == 0 {
        return 0.0;
    }
    total_nodos(medidas) as f64 * 1000.0 / ms as f64
}

/// Diferencias de nodos posición a posición. Vacío = el cambio no tocó lo
/// que la búsqueda visita.
pub fn divergencias(a: &[Medida], b: &[Medida]) -> Vec<(String, u64, u64)> {
    a.iter()
        .zip(b.iter())
        .filter(|(x, y)| x.nodos != y.nodos)
        .map(|(x, y)| (x.nombre.clone(), x.nodos, y.nodos))
        .collect()
}

pub fn imprimir_una(nombre: &str, medidas: &[Medida], profundidad: u32) {
    println!("{nombre} — profundidad {profundidad}");
    println!("{:<22} {:>12} {:>9} {:>12}  jugada", "posición", "nodos", "ms", "nodos/s");
    for m in medidas {
        println!(
            "{:<22} {:>12} {:>9} {:>12.0}  {}",
            m.nombre,
            m.nodos,
            m.ms,
            m.nps(),
            m.jugada
        );
    }
    println!(
        "{:<22} {:>12} {:>9} {:>12.0}",
        "TOTAL",
        total_nodos(medidas),
        total_ms(medidas),
        nps_global(medidas)
    );
}

pub fn imprimir_comparacion(
    nombre_a: &str,
    a: &[Medida],
    nombre_b: &str,
    b: &[Medida],
    profundidad: u32,
) {
    println!();
    println!("=== Comparación a profundidad {profundidad} ===");
    println!("A = {nombre_a}");
    println!("B = {nombre_b}");
    println!();
    println!(
        "{:<22} {:>13} {:>13} {:>9}",
        "posición", "nodos/s A", "nodos/s B", "A/B"
    );
    for (x, y) in a.iter().zip(b.iter()) {
        let ratio = if y.nps() > 0.0 { x.nps() / y.nps() } else { 0.0 };
        println!(
            "{:<22} {:>13.0} {:>13.0} {:>8.2}x",
            x.nombre,
            x.nps(),
            y.nps(),
            ratio
        );
    }
    let (na, nb) = (nps_global(a), nps_global(b));
    let ratio = if nb > 0.0 { na / nb } else { 0.0 };
    println!("{:<22} {:>13.0} {:>13.0} {:>8.2}x", "GLOBAL", na, nb, ratio);

    println!();
    let divs = divergencias(a, b);
    if divs.is_empty() {
        println!(
            "Nodos idénticos en las {} posiciones: el cambio es de solo velocidad y el \
             resultado de arriba es la mejora limpia.",
            a.len()
        );
        println!(
            "Veredicto: {} ({:+.1} % de nodos/segundo)",
            if ratio > 1.0 { "más rápido" } else { "más lento" },
            (ratio - 1.0) * 100.0
        );
    } else {
        println!(
            "AVISO: los nodos difieren en {} de {} posiciones. Esto NO es un cambio de solo \
             velocidad: altera lo que la búsqueda visita, así que la comparación de nodos/segundo \
             no basta para darlo por bueno y hay que pasarlo por 'banco sprt'.",
            divs.len(),
            a.len()
        );
        for (nombre, na, nb) in divs.iter().take(10) {
            println!("  {nombre:<22} A = {na:>12}   B = {nb:>12}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use vigia::board::Board;
    use vigia::movegen;

    #[test]
    fn every_bench_position_is_a_legal_playable_position() {
        for (nombre, fen) in POSICIONES {
            let board = Board::from_fen(fen)
                .unwrap_or_else(|e| panic!("'{nombre}' no es una FEN válida: {e}"));
            assert!(
                !movegen::generate_legal_moves(&board).is_empty(),
                "'{nombre}' no tiene jugadas legales: no sirve para medir velocidad"
            );
        }
    }

    #[test]
    fn bench_position_names_are_unique() {
        let mut nombres: Vec<&str> = POSICIONES.iter().map(|(n, _)| *n).collect();
        nombres.sort_unstable();
        let antes = nombres.len();
        nombres.dedup();
        assert_eq!(nombres.len(), antes, "hay nombres repetidos en el banco");
    }

    fn medida(nombre: &str, nodos: u64, ms: u64) -> Medida {
        Medida { nombre: nombre.to_string(), nodos, ms, jugada: "e2e4".to_string() }
    }

    #[test]
    fn identical_node_counts_report_no_divergence() {
        let a = vec![medida("x", 1000, 10), medida("y", 2000, 20)];
        let b = vec![medida("x", 1000, 8), medida("y", 2000, 16)];
        assert!(divergencias(&a, &b).is_empty());
        assert!(nps_global(&a) < nps_global(&b));
    }

    #[test]
    fn a_changed_node_count_is_reported_with_both_values() {
        let a = vec![medida("x", 1000, 10), medida("y", 2000, 20)];
        let b = vec![medida("x", 1000, 10), medida("y", 2100, 20)];
        assert_eq!(divergencias(&a, &b), vec![("y".to_string(), 2000, 2100)]);
    }

    #[test]
    fn nps_is_zero_rather_than_infinite_when_no_time_elapsed() {
        assert_eq!(medida("x", 1000, 0).nps(), 0.0);
        assert_eq!(nps_global(&[medida("x", 1000, 0)]), 0.0);
    }
}
