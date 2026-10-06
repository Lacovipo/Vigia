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
    /// Nodos de **una** búsqueda: lo que se compara entre motores para exigir
    /// que el árbol sea el mismo.
    pub nodos: u64,
    /// Nodos de todas las búsquedas que suman `ms`. Igual a `nodos` salvo en
    /// las mediciones pareadas, que hacen dos por motor y posición.
    pub nodos_medidos: u64,
    /// En microsegundos: hay posiciones que a profundidad 12 duran 50 ms, y con
    /// milisegundos enteros cada medición llevaría ±2 % solo de redondeo.
    pub micros: u64,
    pub jugada: String,
}

impl Medida {
    pub fn nps(&self) -> f64 {
        if self.micros == 0 {
            return 0.0;
        }
        self.nodos_medidos as f64 * 1_000_000.0 / self.micros as f64
    }

    pub fn ms(&self) -> u64 {
        self.micros / 1000
    }
}

fn una_posicion(
    motor: &mut Motor,
    nombre: &str,
    etiqueta: &str,
    fen: &str,
    profundidad: u32,
    margen: Duration,
) -> Result<Medida, String> {
    // Tabla limpia en cada posición: si no, lo que se mide es el orden
    // en que se recorrieron, no la velocidad del motor.
    motor.nueva_partida()?;
    let respuesta = motor
        .pedir_jugada(Some(fen), &[], Limite::Profundidad(profundidad), margen)?
        .ok_or_else(|| format!("{nombre}: no contestó en '{etiqueta}' dentro del plazo"))?;
    let nodos = respuesta.nodes.unwrap_or(0);
    Ok(Medida {
        nombre: etiqueta.to_string(),
        nodos,
        nodos_medidos: nodos,
        micros: respuesta.elapsed.as_micros() as u64,
        jugada: respuesta.uci,
    })
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
        medidas.push(una_posicion(&mut motor, nombre, etiqueta, fen, profundidad, margen)?);
    }
    Ok(medidas)
}

/// El orden de las ocho mediciones de una posición: `true` = le toca a A.
///
/// Dos bloques, A-B-B-A y B-A-A-B (y al revés en las posiciones impares). Con
/// eso cada motor ocupa en cada posición los cuatro papeles posibles:
///
/// - mide una vez **antes** y una vez **después** que el otro, lo que reparte
///   el coste de ir segundo (frecuencia del núcleo que ya ha bajado, una
///   ráfaga de carga que empezó durante el primero);
/// - y es una vez **el de dentro** —sus dos búsquedas seguidas, la segunda con
///   la caché y el predictor de saltos ya entrenados en ese mismo árbol— y una
///   vez **el de fuera**, que entra frío las dos veces.
///
/// Lo segundo no es un refinamiento: con un solo bloque A-B-B-A por posición,
/// un binario contra su propia copia daba de 1,016 a 1,031 en vez de 1,00.
fn bloque(indice: usize) -> [bool; 8] {
    const ABBA: [bool; 4] = [true, false, false, true];
    const BAAB: [bool; 4] = [false, true, true, false];
    let (primero, segundo) = if indice.is_multiple_of(2) { (ABBA, BAAB) } else { (BAAB, ABBA) };
    let mut orden = [false; 8];
    orden[..4].copy_from_slice(&primero);
    orden[4..].copy_from_slice(&segundo);
    orden
}

/// Mide los dos motores **posición a posición, en bloques A-B-B-A y B-A-A-B**.
///
/// Tres versiones de esto han medido mal, y conviene tener las tres delante:
///
/// 1. **A entero y luego B entero** (hasta 0.34). En una máquina que no está en
///    reposo, una carga que aparece a mitad se la cobra entera a uno de los dos:
///    +41,2 %, +13,6 %, +40,4 % y +49,9 % para el mismo par de binarios.
/// 2. **Intercalado, una medición por motor y posición** (unas horas de 0.34).
///    Parecía bien —banda de 1,06 a 1,07 en tres pasadas— y tenía un sesgo fijo
///    del 6 %: el mismo binario contra su copia daba 0,94. Cada posición sale
///    con ±15 % de ruido si se mide una sola vez, el total era nodos sumados
///    entre tiempo sumado, y una sola posición pesada decidía la cifra; en esa
///    posición siempre medía segundo el mismo motor. Una banda estrecha no es
///    ausencia de sesgo.
/// 3. **Esta**: ocho mediciones por posición en dos bloques cruzados
///    (`bloque`), tiempos en microsegundos, y el veredicto sale de la media
///    geométrica de las relaciones por posición, con todas pesando lo mismo.
///    Comprobada como se comprueba un instrumento: un binario contra su copia
///    tiene que dar 1,00.
///
/// No elimina el ruido —nada lo hace en una máquina compartida—; lo reparte, y
/// la dispersión por posición queda impresa para que se vea cuánto hay.
pub fn medir_pareados(
    ruta_a: &Path,
    nombre_a: &str,
    ruta_b: &Path,
    nombre_b: &str,
    opciones: &[(String, String)],
    profundidad: u32,
    margen: Duration,
) -> Result<(Vec<Medida>, Vec<Medida>), String> {
    let saludo = Duration::from_secs(10);
    let mut motor_a = Motor::lanzar(ruta_a, nombre_a, None, opciones, saludo)?;
    let mut motor_b = Motor::lanzar(ruta_b, nombre_b, None, opciones, saludo)?;
    let mut medidas_a = Vec::with_capacity(POSICIONES.len());
    let mut medidas_b = Vec::with_capacity(POSICIONES.len());
    for (i, (etiqueta, fen)) in POSICIONES.iter().enumerate() {
        let mut de_a: Vec<Medida> = Vec::with_capacity(4);
        let mut de_b: Vec<Medida> = Vec::with_capacity(4);
        for le_toca_a_a in bloque(i) {
            if le_toca_a_a {
                de_a.push(una_posicion(&mut motor_a, nombre_a, etiqueta, fen, profundidad, margen)?);
            } else {
                de_b.push(una_posicion(&mut motor_b, nombre_b, etiqueta, fen, profundidad, margen)?);
            }
        }
        medidas_a.push(fundir(de_a)?);
        medidas_b.push(fundir(de_b)?);
    }
    Ok((medidas_a, medidas_b))
}

/// Todas las mediciones de un motor en una posición, en una: los tiempos se
/// suman y los nodos también, de modo que `nps()` es el del conjunto. Si los
/// nodos no coinciden entre ellas, el motor no es determinista a profundidad
/// fija y este banco no puede medirlo.
fn fundir(varias: Vec<Medida>) -> Result<Medida, String> {
    let mut resto = varias.into_iter();
    let mut unida = resto.next().ok_or("faltan mediciones")?;
    for otra in resto {
        if otra.nodos != unida.nodos {
            return Err(format!(
                "'{}': el mismo motor visitó {} nodos y luego {} en la misma posición y a la misma \
                 profundidad. No es determinista, y un banco de velocidad que exige nodos idénticos no \
                 puede medirlo.",
                unida.nombre, unida.nodos, otra.nodos
            ));
        }
        unida.nodos_medidos += otra.nodos_medidos;
        unida.micros += otra.micros;
    }
    Ok(unida)
}

/// Media geométrica de las relaciones de nodos/segundo A/B por posición, y la
/// menor y la mayor. Todas las posiciones pesan lo mismo: con nodos sumados
/// entre tiempo sumado, la más pesada decide sola el resultado.
pub fn relacion_geometrica(a: &[Medida], b: &[Medida]) -> Option<(f64, f64, f64)> {
    let relaciones: Vec<f64> = a
        .iter()
        .zip(b.iter())
        .filter(|(x, y)| x.nps() > 0.0 && y.nps() > 0.0)
        .map(|(x, y)| x.nps() / y.nps())
        .collect();
    if relaciones.is_empty() {
        return None;
    }
    let media = (relaciones.iter().map(|r| r.ln()).sum::<f64>() / relaciones.len() as f64).exp();
    let menor = relaciones.iter().copied().fold(f64::INFINITY, f64::min);
    let mayor = relaciones.iter().copied().fold(0.0, f64::max);
    Some((media, menor, mayor))
}

/// Lo que dicen varias pasadas juntas: la media geométrica de sus relaciones
/// A/B, entre cuáles se movieron, y el error típico de esa media.
///
/// El error típico es lo que hace falta para leer la cifra: una media de
/// 0,985 con un error de 0,004 dice que A es más lento; la misma media con un
/// error de 0,012 no dice nada.
pub fn resumen_de_pasadas(relaciones: &[f64]) -> String {
    let n = relaciones.len() as f64;
    let logs: Vec<f64> = relaciones.iter().filter(|r| **r > 0.0).map(|r| r.ln()).collect();
    if logs.len() < 2 {
        return "hacen falta al menos dos pasadas con medida para resumir".to_string();
    }
    let media = logs.iter().sum::<f64>() / n;
    let varianza = logs.iter().map(|l| (l - media).powi(2)).sum::<f64>() / (n - 1.0);
    let error = (varianza / n).sqrt();
    let menor = relaciones.iter().copied().fold(f64::INFINITY, f64::min);
    let mayor = relaciones.iter().copied().fold(0.0, f64::max);
    format!(
        "A/B = {:.4}x  ({:+.2} % de nodos/segundo), error típico ±{:.2} %, pasadas de {:.3}x a {:.3}x",
        media.exp(),
        (media.exp() - 1.0) * 100.0,
        error * 100.0,
        menor,
        mayor
    )
}

pub fn total_nodos(medidas: &[Medida]) -> u64 {
    medidas.iter().map(|m| m.nodos).sum()
}

pub fn total_ms(medidas: &[Medida]) -> u64 {
    medidas.iter().map(|m| m.micros).sum::<u64>() / 1000
}

pub fn nps_global(medidas: &[Medida]) -> f64 {
    let micros: u64 = medidas.iter().map(|m| m.micros).sum();
    if micros == 0 {
        return 0.0;
    }
    medidas.iter().map(|m| m.nodos_medidos).sum::<u64>() as f64 * 1_000_000.0 / micros as f64
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
            m.ms(),
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
    println!("{:<22} {:>13.0} {:>13.0} {:>8.2}x", "GLOBAL", na, nb, if nb > 0.0 { na / nb } else { 0.0 });
    // El veredicto no sale del global, que es nodos sumados entre tiempo sumado
    // y lo decide la posición más pesada, sino de la media geométrica de las
    // relaciones, con todas las posiciones pesando lo mismo.
    let (ratio, menor, mayor) = relacion_geometrica(a, b).unwrap_or((0.0, 0.0, 0.0));
    println!(
        "{:<22} {:>27} {:>8.3}x   (por posición: de {:.2}x a {:.2}x)",
        "MEDIA GEOMÉTRICA", "", ratio, menor, mayor
    );

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

    #[test]
    fn every_block_puts_each_engine_in_all_four_roles() {
        for indice in 0..POSICIONES.len() {
            let b = bloque(indice);
            // Cuatro mediciones por motor.
            assert_eq!(b.iter().filter(|a| **a).count(), 4);
            for mitad in [&b[..4], &b[4..]] {
                // Cada mitad es un bloque en espejo: quien abre, cierra, y el
                // otro mide las dos del medio seguidas.
                assert_eq!(mitad[0], mitad[3]);
                assert_eq!(mitad[1], mitad[2]);
                assert_ne!(mitad[0], mitad[1]);
            }
            // Y las dos mitades se cruzan: el que fue «el de fuera» en la
            // primera es «el de dentro» en la segunda. Sin esto, un binario
            // contra su copia no da 1,00.
            assert_ne!(b[0], b[4]);
        }
        // Quién abre se alterna de una posición a la siguiente.
        assert_ne!(bloque(0)[0], bloque(1)[0]);
    }

    #[test]
    fn the_measurements_of_an_engine_are_merged_and_must_agree_on_the_nodes() {
        let unida = fundir(vec![medida("x", 1000, 10), medida("x", 1000, 30), medida("x", 1000, 20)]).unwrap();
        assert_eq!((unida.nodos, unida.nodos_medidos, unida.ms()), (1000, 3000, 60));
        assert_eq!(unida.nps(), 50_000.0);
        // Un motor que no repite sus nodos no se puede medir aquí.
        assert!(fundir(vec![medida("x", 1000, 10), medida("x", 1001, 10)]).is_err());
        assert!(fundir(Vec::new()).is_err());
    }

    #[test]
    fn several_passes_report_their_mean_and_how_far_to_trust_it() {
        // Cuatro pasadas idénticas: media exacta y error cero.
        let r = resumen_de_pasadas(&[0.98, 0.98, 0.98, 0.98]);
        assert!(r.contains("0.9800x") && r.contains("-2.00 %") && r.contains("±0.00 %"), "{r}");
        // Dispersas alrededor de 1: la media no se aparta, y el error lo dice.
        let r = resumen_de_pasadas(&[0.97, 1.03, 0.98, 1.02]);
        assert!(r.contains("de 0.970x a 1.030x"), "{r}");
        assert!(!r.contains("±0.00 %"), "{r}");
        // Con una sola no hay dispersión que contar.
        assert!(resumen_de_pasadas(&[1.0]).contains("al menos dos"));
    }

    #[test]
    fn the_verdict_weighs_every_position_the_same() {
        // Una posición enorme en la que A va un 20 % más lento y tres pequeñas
        // en las que va igual. Con nodos sumados entre tiempo sumado manda la
        // grande; la media geométrica dice lo que pasa en el conjunto.
        let a = vec![medida("grande", 1_000_000, 1250), medida("p1", 1000, 10), medida("p2", 1000, 10), medida("p3", 1000, 10)];
        let b = vec![medida("grande", 1_000_000, 1000), medida("p1", 1000, 10), medida("p2", 1000, 10), medida("p3", 1000, 10)];
        assert!((nps_global(&a) / nps_global(&b) - 0.80).abs() < 0.01);
        let (media, menor, mayor) = relacion_geometrica(&a, &b).unwrap();
        assert!((media - 0.8f64.powf(0.25)).abs() < 1e-9, "{media}");
        assert!((menor - 0.8).abs() < 1e-9 && (mayor - 1.0).abs() < 1e-9);
        assert!(relacion_geometrica(&[], &[]).is_none());
    }

    fn medida(nombre: &str, nodos: u64, ms: u64) -> Medida {
        Medida { nombre: nombre.to_string(), nodos, nodos_medidos: nodos, micros: ms * 1000, jugada: "e2e4".to_string() }
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
