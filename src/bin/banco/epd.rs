//! Suites EPD: no-regresión táctica y posicional.
//!
//! Complementa al SPRT en el eje que este mide mal. Un cambio en la
//! evaluación puede salir neutro en Elo y haber roto la detección de un
//! motivo táctico concreto; el Elo lo diluye entre cientos de partidas y
//! una suite lo señala con el dedo.
//!
//! **No es una puerta de calidad.** Ninguna suite mide fuerza: acertar más
//! posiciones de una lista no implica jugar mejor, y hay cambios buenos que
//! bajan el marcador. Sirve para dos cosas: detectar un derrumbe (de 180 a
//! 90 aciertos es un fallo, no ruido) y dar posiciones concretas que
//! examinar cuando el SPRT dice que algo empeoró y no se sabe por qué.

use std::path::Path;
use std::time::Duration;

use vigia::board::Board;
use vigia::types::Move;

use crate::motor::{Limite, Motor};
use crate::san;

pub struct Caso {
    pub fen: String,
    pub id: String,
    /// Jugadas que se consideran correctas (`bm`).
    pub mejores: Vec<String>,
    /// Jugadas que se consideran erróneas (`am`).
    pub evitar: Vec<String>,
}

/// Lee un fichero EPD con operaciones `bm`, `am` e `id`.
pub fn cargar(ruta: &Path) -> Result<Vec<Caso>, String> {
    let texto = std::fs::read_to_string(ruta)
        .map_err(|e| format!("no se pudo leer '{}': {e}", ruta.display()))?;
    let mut casos = Vec::new();
    for (n, linea) in texto.lines().enumerate() {
        let linea = linea.trim();
        if linea.is_empty() || linea.starts_with('#') {
            continue;
        }
        match parsear(linea) {
            Some(caso) => casos.push(caso),
            None => {
                return Err(format!(
                    "{}:{}: línea EPD sin posición reconocible o sin 'bm'/'am': '{linea}'",
                    ruta.display(),
                    n + 1
                ))
            }
        }
    }
    if casos.is_empty() {
        return Err(format!("'{}' no contiene ningún caso", ruta.display()));
    }
    Ok(casos)
}

fn parsear(linea: &str) -> Option<Caso> {
    let mut partes = linea.splitn(2, ';');
    let cabeza = partes.next()?.trim();
    let cola = partes.next().unwrap_or("");

    // La cabecera lleva la posición y, pegadas, la primera tanda de
    // operaciones: "4k3/... w - - bm Nf3" (EPD no usa relojes).
    let tokens: Vec<&str> = cabeza.split_whitespace().collect();
    if tokens.len() < 4 || !tokens[0].contains('/') {
        return None;
    }
    let fen = format!("{} {} {} {} 0 1", tokens[0], tokens[1], tokens[2], tokens[3]);
    Board::from_fen(&fen).ok()?;

    let resto_cabeza = tokens[4..].join(" ");
    let operaciones: Vec<&str> = std::iter::once(resto_cabeza.as_str())
        .chain(cola.split(';'))
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .collect();

    let mut mejores = Vec::new();
    let mut evitar = Vec::new();
    let mut id = String::new();
    for op in operaciones {
        let mut it = op.splitn(2, char::is_whitespace);
        let clave = it.next().unwrap_or("");
        let valor = it.next().unwrap_or("").trim();
        match clave {
            "bm" => mejores.extend(valor.split_whitespace().map(str::to_string)),
            "am" => evitar.extend(valor.split_whitespace().map(str::to_string)),
            "id" => id = valor.trim_matches('"').to_string(),
            _ => {}
        }
    }
    if mejores.is_empty() && evitar.is_empty() {
        return None;
    }
    if id.is_empty() {
        id = fen.split_whitespace().next().unwrap_or("?").to_string();
    }
    Some(Caso { fen, id, mejores, evitar })
}

pub struct Fallo {
    pub id: String,
    pub fen: String,
    pub esperado: String,
    pub jugada: String,
}

pub struct Informe {
    pub aciertos: usize,
    pub total: usize,
    pub fallos: Vec<Fallo>,
    pub ms: u64,
}

pub fn ejecutar(
    ruta_motor: &Path,
    nombre: &str,
    opciones: &[(String, String)],
    casos: &[Caso],
    limite: Limite,
    margen: Duration,
) -> Result<Informe, String> {
    let mut motor = Motor::lanzar(ruta_motor, nombre, None, opciones, Duration::from_secs(10))?;
    let mut aciertos = 0;
    let mut fallos = Vec::new();
    let mut ms = 0u64;

    for caso in casos {
        motor.nueva_partida()?;
        let plazo = match limite {
            Limite::Movetime(t) => Duration::from_millis(t) + margen,
            _ => margen,
        };
        let respuesta = motor
            .pedir_jugada(Some(&caso.fen), &[], limite, plazo)?
            .ok_or_else(|| format!("{nombre}: no contestó en el caso '{}'", caso.id))?;
        ms += respuesta.elapsed.as_millis() as u64;

        let board = Board::from_fen(&caso.fen)
            .map_err(|e| format!("caso '{}': FEN inválida ({e})", caso.id))?;
        let jugada_san = san::desde_texto(&board, &respuesta.uci)
            .map(|m: Move| san::to_san(&board, m))
            .unwrap_or_else(|| respuesta.uci.clone());

        let resuelve = |lista: &[String]| -> bool {
            lista.iter().any(|texto| {
                san::desde_texto(&board, texto).map(|m| m.to_string()) == Some(respuesta.uci.clone())
            })
        };
        let bien = if !caso.mejores.is_empty() {
            resuelve(&caso.mejores)
        } else {
            !resuelve(&caso.evitar)
        };

        if bien {
            aciertos += 1;
        } else {
            let esperado = if caso.mejores.is_empty() {
                format!("evitar {}", caso.evitar.join(" "))
            } else {
                caso.mejores.join(" ")
            };
            fallos.push(Fallo {
                id: caso.id.clone(),
                fen: caso.fen.clone(),
                esperado,
                jugada: jugada_san,
            });
        }
    }
    Ok(Informe { aciertos, total: casos.len(), fallos, ms })
}

pub fn imprimir(nombre: &str, informe: &Informe, detalle: usize) {
    println!();
    println!(
        "{nombre}: {}/{} aciertos ({:.1} %) en {:.1} s",
        informe.aciertos,
        informe.total,
        informe.aciertos as f64 * 100.0 / informe.total.max(1) as f64,
        informe.ms as f64 / 1000.0
    );
    if informe.fallos.is_empty() {
        return;
    }
    println!("Fallos ({} en total, se listan hasta {detalle}):", informe.fallos.len());
    for fallo in informe.fallos.iter().take(detalle) {
        println!("  {:<24} esperaba {:<14} jugó {}", fallo.id, fallo.esperado, fallo.jugada);
        println!("    {}", fallo.fen);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_classic_epd_line_is_parsed_whole() {
        let caso = parsear(
            "1k1r4/pp1b1R2/3q2pp/4p3/2B5/4Q3/PPP2B2/2K5 b - - bm Qd1+; id \"BK.01\";",
        )
        .unwrap();
        assert_eq!(caso.id, "BK.01");
        assert_eq!(caso.mejores, vec!["Qd1+"]);
        assert!(caso.fen.ends_with(" 0 1"));
        assert!(caso.evitar.is_empty());
    }

    #[test]
    fn several_accepted_moves_are_all_kept() {
        let caso = parsear("4k3/8/8/8/8/8/8/R3K2R w KQ - bm O-O Ra8+; id \"dos\";").unwrap();
        assert_eq!(caso.mejores, vec!["O-O", "Ra8+"]);
    }

    #[test]
    fn an_avoid_move_line_works_without_a_best_move() {
        let caso = parsear("4k3/8/8/8/8/8/8/R3K2R w KQ - am Ra8+;").unwrap();
        assert!(caso.mejores.is_empty());
        assert_eq!(caso.evitar, vec!["Ra8+"]);
    }

    #[test]
    fn a_line_without_bm_or_am_is_not_a_case() {
        assert!(parsear("4k3/8/8/8/8/8/8/4K3 w - - id \"solo id\";").is_none());
    }

    #[test]
    fn an_illegal_position_is_not_accepted_as_a_case() {
        assert!(parsear("8/8/8/8/8/8/8/8 w - - bm e4;").is_none());
    }

    #[test]
    fn a_case_without_id_gets_one_from_its_position() {
        let caso = parsear("4k3/8/8/8/8/8/8/R3K2R w KQ - bm O-O;").unwrap();
        assert_eq!(caso.id, "4k3/8/8/8/8/8/8/R3K2R");
    }

    #[test]
    fn the_expected_move_resolves_against_the_real_position() {
        // Comprobación de que `bm` en SAN se puede casar con la jugada UCI
        // del motor: es todo el mecanismo de puntuación de la suite.
        let caso = parsear(
            "r1bqkbnr/pppp1ppp/2n5/4p3/2B1P3/5N2/PPPP1PPP/RNBQK2R b KQkq - bm Nf6; id \"dos caballos\";",
        )
        .unwrap();
        let board = Board::from_fen(&caso.fen).unwrap();
        let esperada = san::desde_texto(&board, &caso.mejores[0]).unwrap();
        assert_eq!(esperada.to_string(), "g8f6");
        // Y la forma UCI del mismo movimiento tiene que resolver igual.
        assert_eq!(san::desde_texto(&board, "g8f6"), Some(esperada));
    }
}
