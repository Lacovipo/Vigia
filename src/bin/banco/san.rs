//! Notación algebraica estándar (SAN).
//!
//! El motor habla UCI (`e2e4`), pero un PGN que no esté en SAN no lo abre
//! ningún visor, y clasificar derrotas mirando `g1f3 b8c6` a mano no es
//! viable. La conversión se hace contra la lista de jugadas legales reales,
//! así que la desambiguación sale correcta por construcción en vez de por
//! reglas aproximadas.

use vigia::board::Board;
use vigia::movegen;
use vigia::types::{Move, PieceType};

fn letra(kind: PieceType) -> &'static str {
    match kind {
        PieceType::Pawn => "",
        PieceType::Knight => "N",
        PieceType::Bishop => "B",
        PieceType::Rook => "R",
        PieceType::Queen => "Q",
        PieceType::King => "K",
    }
}

/// Convierte una jugada **legal en `board`** a SAN, con sufijo `+`/`#`.
///
/// Si la jugada no es legal en la posición dada, devuelve su forma UCI: es
/// una situación que no debería darse, y degradar a UCI deja el PGN
/// inspeccionable en vez de abortar una tanda de horas por un detalle de
/// formato.
pub fn to_san(board: &Board, mv: Move) -> String {
    let legales = movegen::generate_legal_moves(board);
    if !legales.contains(&mv) {
        return mv.to_string();
    }
    let Some(pieza) = board.piece_at(mv.from) else {
        return mv.to_string();
    };

    let mut san = if mv.flag.is_castle() {
        // El enroque corto lleva el rey hacia la columna g; el largo, hacia
        // la c. Distinguir por columna destino evita depender del flag en
        // variantes donde el rey no parte de e1.
        if mv.to.file() > mv.from.file() {
            "O-O".to_string()
        } else {
            "O-O-O".to_string()
        }
    } else if pieza.kind == PieceType::Pawn {
        let mut s = String::new();
        if mv.is_capture() {
            s.push((b'a' + mv.from.file()) as char);
            s.push('x');
        }
        s.push_str(&mv.to.to_string());
        if let Some(promo) = mv.promotion() {
            s.push('=');
            s.push_str(letra(promo));
        }
        s
    } else {
        let mut s = letra(pieza.kind).to_string();
        s.push_str(&desambiguacion(board, &legales, mv, pieza.kind));
        if mv.is_capture() {
            s.push('x');
        }
        s.push_str(&mv.to.to_string());
        s
    };

    let mut despues = board.clone();
    despues.make_move(mv);
    let sin_respuesta = movegen::generate_legal_moves(&despues).is_empty();
    let en_jaque = movegen::is_in_check(&despues, despues.side_to_move);
    if en_jaque {
        san.push(if sin_respuesta { '#' } else { '+' });
    }
    san
}

/// Parte mínima de la casilla de origen que hace única a la jugada entre
/// las de la misma pieza al mismo destino: nada, la columna, la fila o la
/// casilla completa, en ese orden de preferencia.
fn desambiguacion(board: &Board, legales: &[Move], mv: Move, kind: PieceType) -> String {
    let rivales: Vec<Move> = legales
        .iter()
        .copied()
        .filter(|otra| {
            otra.to == mv.to
                && otra.from != mv.from
                && board.piece_at(otra.from).map(|p| p.kind) == Some(kind)
        })
        .collect();
    if rivales.is_empty() {
        return String::new();
    }
    if rivales.iter().all(|o| o.from.file() != mv.from.file()) {
        return ((b'a' + mv.from.file()) as char).to_string();
    }
    if rivales.iter().all(|o| o.from.rank() != mv.from.rank()) {
        return ((b'1' + mv.from.rank()) as char).to_string();
    }
    mv.from.to_string()
}

/// Quita de un SAN todo lo que no cambia qué jugada es: sufijos de jaque y
/// anotaciones (`!`, `?`, `!?`...). Sirve para comparar la jugada del motor
/// con el `bm` de una suite EPD, que puede venir con o sin adornos.
pub fn normalizar_san(san: &str) -> String {
    san.chars()
        .filter(|c| !matches!(c, '+' | '#' | '!' | '?'))
        .collect()
}

/// Busca en `board` la jugada legal cuyo SAN normalizado coincide con
/// `texto`. Acepta también la forma UCI, porque las suites EPD del mundo
/// real mezclan ambas.
pub fn desde_texto(board: &Board, texto: &str) -> Option<Move> {
    let objetivo = normalizar_san(texto);
    let legales = movegen::generate_legal_moves(board);
    legales
        .iter()
        .find(|&&m| normalizar_san(&to_san(board, m)) == objetivo)
        .or_else(|| legales.iter().find(|&&m| m.to_string() == texto))
        .copied()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn san_de(fen: &str, uci: &str) -> String {
        let board = Board::from_fen(fen).unwrap();
        let mv = movegen::generate_legal_moves(&board)
            .into_iter()
            .find(|m| m.to_string() == uci)
            .unwrap_or_else(|| panic!("{uci} no es legal en {fen}"));
        to_san(&board, mv)
    }

    const INICIAL: &str = "rnbqkbnr/pppppppp/8/8/8/8/PPPPPPPP/RNBQKBNR w KQkq - 0 1";

    #[test]
    fn a_quiet_pawn_move_is_just_its_destination() {
        assert_eq!(san_de(INICIAL, "e2e4"), "e4");
    }

    #[test]
    fn a_knight_move_carries_its_letter() {
        assert_eq!(san_de(INICIAL, "g1f3"), "Nf3");
    }

    #[test]
    fn a_pawn_capture_names_the_departure_file() {
        assert_eq!(
            san_de("rnbqkbnr/ppp1pppp/8/3p4/4P3/8/PPPP1PPP/RNBQKBNR w KQkq d6 0 2", "e4d5"),
            "exd5"
        );
    }

    #[test]
    fn en_passant_looks_like_any_other_pawn_capture() {
        assert_eq!(
            san_de("rnbqkbnr/ppp1p1pp/8/3pPp2/8/8/PPPP1PPP/RNBQKBNR w KQkq f6 0 3", "e5f6"),
            "exf6"
        );
    }

    #[test]
    fn castling_uses_the_o_notation_on_both_sides() {
        let fen = "r3k2r/8/8/8/8/8/8/R3K2R w KQkq - 0 1";
        assert_eq!(san_de(fen, "e1g1"), "O-O");
        assert_eq!(san_de(fen, "e1c1"), "O-O-O");
    }

    #[test]
    fn promotion_appends_the_new_piece() {
        assert_eq!(san_de("4k3/2P5/8/8/8/8/8/4K3 w - - 0 1", "c7c8q"), "c8=Q+");
        assert_eq!(san_de("4k3/2P5/8/8/8/8/8/4K3 w - - 0 1", "c7c8n"), "c8=N");
    }

    #[test]
    fn a_promotion_with_capture_keeps_the_file_prefix() {
        assert_eq!(san_de("3rk3/2P5/8/8/8/8/8/4K3 w - - 0 1", "c7d8q"), "cxd8=Q+");
    }

    #[test]
    fn two_knights_reaching_one_square_are_told_apart_by_file() {
        let fen = "4k3/8/8/8/8/2N1N3/8/4K3 w - - 0 1";
        assert_eq!(san_de(fen, "c3d5"), "Ncd5");
        assert_eq!(san_de(fen, "e3d5"), "Ned5");
    }

    #[test]
    fn two_rooks_on_one_file_are_told_apart_by_rank() {
        let fen = "4k3/8/8/8/R7/8/8/R3K3 w - - 0 1";
        assert_eq!(san_de(fen, "a1a2"), "R1a2");
        assert_eq!(san_de(fen, "a4a2"), "R4a2");
    }

    #[test]
    fn three_queens_may_need_the_whole_square() {
        // Damas en a1, a5 y e1, todas con e5 a tiro: la de a1 comparte
        // columna con a5 y fila con e1, así que ni columna ni fila bastan y
        // hace falta la casilla entera. Las otras dos sí se distinguen con
        // media casilla, cada una por un lado distinto.
        let fen = "6k1/8/8/Q7/8/8/8/Q3Q2K w - - 0 1";
        assert_eq!(san_de(fen, "a1e5"), "Qa1e5");
        assert_eq!(san_de(fen, "a5e5"), "Q5e5");
        assert_eq!(san_de(fen, "e1e5"), "Qee5");
    }

    #[test]
    fn a_check_gets_a_plus_and_a_mate_gets_a_hash() {
        assert_eq!(
            san_de("r1bqkbnr/pppp1ppp/2n5/4p2Q/2B1P3/8/PPPP1PPP/RNB1K1NR w KQkq - 0 1", "h5f7"),
            "Qxf7#"
        );
        assert_eq!(san_de("4k3/8/8/8/8/8/8/R3K3 w Q - 0 1", "a1a8"), "Ra8+");
    }

    #[test]
    fn normalizing_drops_decorations_but_not_the_move() {
        assert_eq!(normalizar_san("Qxf7#"), "Qxf7");
        assert_eq!(normalizar_san("Nf3!?"), "Nf3");
        assert_eq!(normalizar_san("e4"), "e4");
    }

    #[test]
    fn a_move_can_be_looked_up_from_san_or_from_uci() {
        let board = Board::from_fen(INICIAL).unwrap();
        let por_san = desde_texto(&board, "Nf3").unwrap();
        let por_uci = desde_texto(&board, "g1f3").unwrap();
        assert_eq!(por_san, por_uci);
        assert_eq!(por_san.to_string(), "g1f3");
        assert!(desde_texto(&board, "Nf6").is_none());
    }

    #[test]
    fn every_legal_move_from_a_busy_position_round_trips() {
        // La red de seguridad de verdad: si la desambiguación se quedara
        // corta, dos jugadas distintas producirían el mismo SAN y el PGN
        // sería ambiguo. Se exige que todos los SAN de la posición sean
        // distintos y que cada uno se resuelva de vuelta a su jugada.
        let fen = "r3k2r/p1ppqpb1/bn2pnp1/3PN3/1p2P3/2N2Q1p/PPPBBPPP/R3K2R w KQkq - 0 1";
        let board = Board::from_fen(fen).unwrap();
        let legales = movegen::generate_legal_moves(&board);
        let mut vistos: Vec<String> = Vec::new();
        for mv in legales {
            let san = to_san(&board, mv);
            assert!(!vistos.contains(&san), "SAN repetido: {san}");
            assert_eq!(desde_texto(&board, &san), Some(mv), "no vuelve: {san}");
            vistos.push(san);
        }
    }
}
