//! Exportación a PGN.
//!
//! El PGN no es un adorno: es la única forma práctica de contestar "vale,
//! el candidato pierde 3 Elo, ¿pero *por qué*?". Se escriben la puntuación
//! y la profundidad de cada jugada como comentario, con el mismo formato
//! que usa cutechess-cli, para poder abrir el fichero en cualquier visor y
//! ver dónde se torció la partida.

use crate::arbitro::{Partida, FEN_INICIAL};

pub struct Meta {
    pub evento: String,
    pub sitio: String,
    pub fecha: String,
    pub ronda: String,
    pub blancas: String,
    pub negras: String,
    pub control: String,
}

/// Ancho al que se parten las líneas de jugadas. El estándar PGN pide no
/// pasar de 80 columnas.
const ANCHO: usize = 80;

pub fn escribir(partida: &Partida, meta: &Meta) -> String {
    let mut out = String::new();
    let etiqueta = |out: &mut String, clave: &str, valor: &str| {
        out.push_str(&format!("[{clave} \"{}\"]\n", valor.replace('\\', "\\\\").replace('"', "\\\"")));
    };

    etiqueta(&mut out, "Event", &meta.evento);
    etiqueta(&mut out, "Site", &meta.sitio);
    etiqueta(&mut out, "Date", &meta.fecha);
    etiqueta(&mut out, "Round", &meta.ronda);
    etiqueta(&mut out, "White", &meta.blancas);
    etiqueta(&mut out, "Black", &meta.negras);
    etiqueta(&mut out, "Result", partida.resultado.pgn());
    if partida.fen_inicial != FEN_INICIAL {
        etiqueta(&mut out, "SetUp", "1");
        etiqueta(&mut out, "FEN", &partida.fen_inicial);
    }
    etiqueta(&mut out, "TimeControl", &meta.control);
    etiqueta(&mut out, "PlyCount", &partida.jugadas.len().to_string());
    etiqueta(&mut out, "Termination", partida.final_partida.as_str());
    out.push('\n');

    let (mut numero, mut toca_blancas) = inicio_de_numeracion(&partida.fen_inicial);
    let mut piezas: Vec<String> = Vec::new();
    for (i, jugada) in partida.jugadas.iter().enumerate() {
        if toca_blancas {
            piezas.push(format!("{numero}."));
        } else if i == 0 {
            // Una apertura que arranca con las negras necesita el "12..."
            // explícito o el PGN no se puede releer.
            piezas.push(format!("{numero}..."));
        }
        piezas.push(jugada.san.clone());
        let score = jugada
            .score
            .map(|s| s.to_pgn_comment())
            .unwrap_or_else(|| "?".to_string());
        let depth = jugada.depth.map(|d| d.to_string()).unwrap_or_else(|| "?".to_string());
        piezas.push(format!("{{{score}/{depth} {:.3}s}}", jugada.ms as f64 / 1000.0));

        if !toca_blancas {
            numero += 1;
        }
        toca_blancas = !toca_blancas;
    }
    piezas.push(partida.resultado.pgn().to_string());

    let mut linea = String::new();
    for pieza in piezas {
        if !linea.is_empty() && linea.len() + 1 + pieza.len() > ANCHO {
            out.push_str(&linea);
            out.push('\n');
            linea.clear();
        }
        if !linea.is_empty() {
            linea.push(' ');
        }
        linea.push_str(&pieza);
    }
    if !linea.is_empty() {
        out.push_str(&linea);
        out.push('\n');
    }
    out.push('\n');
    out
}

/// Número de jugada y bando al que le toca, leídos de la FEN de partida.
/// Ante una FEN rara se cae a "1, blancas": un PGN con numeración fea es
/// preferible a abortar la escritura de una tanda entera.
fn inicio_de_numeracion(fen: &str) -> (u32, bool) {
    let campos: Vec<&str> = fen.split_whitespace().collect();
    let toca_blancas = campos.get(1).copied().unwrap_or("w") != "b";
    let numero = campos.get(5).and_then(|t| t.parse::<u32>().ok()).unwrap_or(1);
    (numero, toca_blancas)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::arbitro::{Final, JugadaRegistrada, Resultado};
    use crate::motor::Score;

    fn meta() -> Meta {
        Meta {
            evento: "banco".to_string(),
            sitio: "local".to_string(),
            fecha: "2026.08.18".to_string(),
            ronda: "1.1".to_string(),
            blancas: "Candidato".to_string(),
            negras: "Base".to_string(),
            control: "3000 nodos".to_string(),
        }
    }

    fn jugada(san: &str, score: Option<Score>, depth: Option<u32>) -> JugadaRegistrada {
        JugadaRegistrada {
            uci: "0000".to_string(),
            san: san.to_string(),
            score,
            depth,
            ms: 120,
        }
    }

    #[test]
    fn a_game_from_the_start_needs_no_fen_tag() {
        let partida = Partida {
            resultado: Resultado::GananBlancas,
            final_partida: Final::Mate,
            fen_inicial: FEN_INICIAL.to_string(),
            jugadas: vec![
                jugada("e4", Some(Score::Cp(30)), Some(10)),
                jugada("e5", Some(Score::Cp(-25)), Some(10)),
                jugada("Qh5", Some(Score::Cp(35)), Some(11)),
            ],
        };
        let texto = escribir(&partida, &meta());
        assert!(!texto.contains("[FEN"), "no debería llevar etiqueta FEN");
        assert!(texto.contains("1. e4 {+0.30/10 0.120s} e5"));
        assert!(texto.contains("2. Qh5"));
        assert!(texto.contains("[Result \"1-0\"]"));
        assert!(texto.trim_end().ends_with("1-0"));
    }

    #[test]
    fn a_game_from_a_book_position_carries_setup_and_fen() {
        let fen = "r1bqkbnr/pppp1ppp/2n5/4p3/2B1P3/5N2/PPPP1PPP/RNBQK2R b KQkq - 4 4";
        let partida = Partida {
            resultado: Resultado::Tablas,
            final_partida: Final::TablasAdjudicadas,
            fen_inicial: fen.to_string(),
            jugadas: vec![jugada("Nf6", Some(Score::Cp(5)), Some(12))],
        };
        let texto = escribir(&partida, &meta());
        assert!(texto.contains("[SetUp \"1\"]"));
        assert!(texto.contains(&format!("[FEN \"{fen}\"]")));
        // Arranca con negras en la jugada 4.
        assert!(texto.contains("4... Nf6"), "{texto}");
        assert!(texto.contains("[Termination \"tablas_adjudicadas\"]"));
    }

    #[test]
    fn a_move_without_score_is_written_as_a_question_mark() {
        let partida = Partida {
            resultado: Resultado::GananNegras,
            final_partida: Final::Incomparecencia,
            fen_inicial: FEN_INICIAL.to_string(),
            jugadas: vec![jugada("e4", None, None)],
        };
        let texto = escribir(&partida, &meta());
        assert!(texto.contains("{?/? 0.120s}"), "{texto}");
    }

    #[test]
    fn move_text_lines_stay_within_the_pgn_width() {
        let jugadas: Vec<JugadaRegistrada> = (0..60)
            .map(|i| jugada(if i % 2 == 0 { "Nf3" } else { "Nf6" }, Some(Score::Cp(12)), Some(9)))
            .collect();
        let partida = Partida {
            resultado: Resultado::Tablas,
            final_partida: Final::TopeDePlies,
            fen_inicial: FEN_INICIAL.to_string(),
            jugadas,
        };
        let texto = escribir(&partida, &meta());
        for linea in texto.lines() {
            assert!(linea.len() <= ANCHO, "línea de {} caracteres: {linea}", linea.len());
        }
    }

    #[test]
    fn move_numbering_starts_from_the_fen_counter() {
        assert_eq!(inicio_de_numeracion(FEN_INICIAL), (1, true));
        assert_eq!(
            inicio_de_numeracion("r1bqkbnr/pppp1ppp/2n5/4p3/2B1P3/5N2/PPPP1PPP/RNBQK2R b KQkq - 4 12"),
            (12, false)
        );
        assert_eq!(inicio_de_numeracion("basura"), (1, true));
    }

    #[test]
    fn quotes_in_an_engine_name_do_not_break_the_tag() {
        let mut m = meta();
        m.blancas = "Vigia \"dev\"".to_string();
        let partida = Partida {
            resultado: Resultado::Tablas,
            final_partida: Final::Ahogado,
            fen_inicial: FEN_INICIAL.to_string(),
            jugadas: vec![],
        };
        let texto = escribir(&partida, &m);
        assert!(texto.contains(r#"[White "Vigia \"dev\""]"#), "{texto}");
    }
}
