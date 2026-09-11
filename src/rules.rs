//! The rules that end a game, in one place.
//!
//! The bench's referee and the NNUE data generator both play games to the
//! end, and they have to agree with each other and with the engine about when
//! a game is over. A generator that stopped at a different point than the
//! referee would label its positions with results the bench never produces.
//! Like `eval::is_insufficient_material`, which three diverging copies once
//! disagreed about, this lives in the library exactly once.

use crate::board::Board;
use crate::eval;
use crate::movegen;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum GameEnd {
    /// The side to move is checkmated.
    Checkmate,
    Stalemate,
    FiftyMoves,
    InsufficientMaterial,
    /// The current position has appeared for the third time in the game.
    Repetition,
}

/// Whether the game is over in `board`, and why. `history` holds the hash of
/// every position of the game so far, `board` included.
///
/// The order matters. A mate on the board beats any draw: the fifty-move
/// counter can run out on the very move that delivers mate, and the mate
/// stands. So the legal moves are looked at first, and the draw rules after.
pub fn game_end(board: &Board, history: &[u64]) -> Option<GameEnd> {
    if movegen::generate_legal_moves(board).is_empty() {
        return Some(if movegen::is_in_check(board, board.side_to_move) {
            GameEnd::Checkmate
        } else {
            GameEnd::Stalemate
        });
    }
    if board.halfmove_clock >= 100 {
        return Some(GameEnd::FiftyMoves);
    }
    if eval::is_insufficient_material(board) {
        return Some(GameEnd::InsufficientMaterial);
    }
    if history.iter().filter(|&&hash| hash == board.hash).count() >= 3 {
        return Some(GameEnd::Repetition);
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn checkmate_is_detected_from_the_position_alone() {
        let board = Board::from_fen("r1bqkbnr/pppp1Qpp/2n5/4p3/2B1P3/8/PPPP1PPP/RNB1K1NR b KQkq - 0 1").unwrap();
        assert_eq!(game_end(&board, &[board.hash]), Some(GameEnd::Checkmate));
    }

    #[test]
    fn stalemate_is_not_a_checkmate() {
        let board = Board::from_fen("7k/5Q2/6K1/8/8/8/8/8 b - - 0 1").unwrap();
        assert_eq!(game_end(&board, &[board.hash]), Some(GameEnd::Stalemate));
    }

    #[test]
    fn the_fifty_move_rule_fires_at_a_hundred_halfmoves() {
        let board = Board::from_fen("4k3/8/8/8/8/8/4P3/4K3 w - - 100 80").unwrap();
        assert_eq!(game_end(&board, &[board.hash]), Some(GameEnd::FiftyMoves));
        let board = Board::from_fen("4k3/8/8/8/8/8/4P3/4K3 w - - 99 80").unwrap();
        assert_eq!(game_end(&board, &[board.hash]), None);
    }

    #[test]
    fn checkmate_beats_the_fifty_move_rule() {
        // Back-rank mate with the fifty-move counter already exhausted.
        let board = Board::from_fen("R5k1/5ppp/8/8/8/8/8/6K1 b - - 100 90").unwrap();
        assert_eq!(game_end(&board, &[board.hash]), Some(GameEnd::Checkmate));
    }

    #[test]
    fn a_third_repetition_ends_the_game_and_a_second_does_not() {
        let board = Board::from_fen("4k3/8/8/8/8/8/4P3/4K3 w - - 10 30").unwrap();
        assert_eq!(game_end(&board, &[board.hash, 1, board.hash, 2, board.hash]), Some(GameEnd::Repetition));
        assert_eq!(game_end(&board, &[board.hash, 1, board.hash]), None);
    }

    #[test]
    fn bare_kings_end_the_game() {
        let board = Board::from_fen("4k3/8/8/8/8/8/8/4K3 w - - 0 1").unwrap();
        assert_eq!(game_end(&board, &[board.hash]), Some(GameEnd::InsufficientMaterial));
    }
}
