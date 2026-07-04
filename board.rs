use std::fmt;

use crate::bitboard::Bitboard;
use crate::types::{CastlingRights, Color, Move, MoveFlag, Piece, PieceType, Square};
use crate::zobrist;

pub const STARTPOS_FEN: &str = "rnbqkbnr/pppppppp/8/8/8/8/PPPPPPPP/RNBQKBNR w KQkq - 0 1";

/// Drops any castling right whose king/rook aren't actually on their home
/// squares. A FEN can claim a right that no longer matches the board (e.g.
/// a hand-edited or adversarial position missing the corner rook); trusting
/// it blindly would let `generate_castling` offer the move and then panic
/// in `make_move` when the expected rook isn't there to relocate.
fn sanitize_castling_rights(rights: CastlingRights, mailbox: &[Option<Piece>; 64]) -> CastlingRights {
    let is_piece = |sq: Square, color: Color, kind: PieceType| {
        matches!(mailbox[sq.0 as usize], Some(p) if p.color == color && p.kind == kind)
    };
    let mut rights = rights;
    let white_king_home = is_piece(Square::new(4, 0), Color::White, PieceType::King);
    let black_king_home = is_piece(Square::new(4, 7), Color::Black, PieceType::King);
    if !(white_king_home && is_piece(Square::H1, Color::White, PieceType::Rook)) {
        rights.remove(CastlingRights::WHITE_KINGSIDE);
    }
    if !(white_king_home && is_piece(Square::A1, Color::White, PieceType::Rook)) {
        rights.remove(CastlingRights::WHITE_QUEENSIDE);
    }
    if !(black_king_home && is_piece(Square::H8, Color::Black, PieceType::Rook)) {
        rights.remove(CastlingRights::BLACK_KINGSIDE);
    }
    if !(black_king_home && is_piece(Square::A8, Color::Black, PieceType::Rook)) {
        rights.remove(CastlingRights::BLACK_QUEENSIDE);
    }
    rights
}

#[derive(Clone)]
pub struct Board {
    pieces: [[Bitboard; 6]; 2],
    mailbox: [Option<Piece>; 64],
    pub side_to_move: Color,
    pub castling: CastlingRights,
    pub en_passant: Option<Square>,
    pub halfmove_clock: u16,
    pub fullmove_number: u16,
    /// Zobrist hash of the position, maintained incrementally. Two boards
    /// with the same hash are the same position for repetition/TT purposes
    /// (modulo the astronomically unlikely case of a hash collision).
    pub hash: u64,
}

/// Everything needed to reverse a `make_move` call.
#[derive(Clone, Copy)]
pub struct Undo {
    captured: Option<Piece>,
    capture_square: Square,
    prev_castling: CastlingRights,
    prev_en_passant: Option<Square>,
    prev_halfmove_clock: u16,
    prev_hash: u64,
}

/// Everything needed to reverse a `make_null_move` call.
#[derive(Clone, Copy)]
pub struct NullUndo {
    prev_en_passant: Option<Square>,
    prev_hash: u64,
}

impl Board {
    pub fn start_pos() -> Board {
        Board::from_fen(STARTPOS_FEN).expect("la FEN de la posición inicial debe ser válida")
    }

    pub fn from_fen(fen: &str) -> Result<Board, String> {
        let fields: Vec<&str> = fen.split_whitespace().collect();
        if fields.len() < 4 {
            return Err(format!(
                "FEN inválida: se esperaban al menos 4 campos, se obtuvieron {}",
                fields.len()
            ));
        }

        let mut pieces = [[Bitboard::EMPTY; 6]; 2];
        let mut mailbox = [None; 64];

        let ranks: Vec<&str> = fields[0].split('/').collect();
        if ranks.len() != 8 {
            return Err(format!(
                "FEN inválida: se esperaban 8 filas separadas por '/', se obtuvieron {}",
                ranks.len()
            ));
        }
        for (rank_from_top, rank_str) in ranks.iter().enumerate() {
            let rank = 7 - rank_from_top as u8;
            let mut file = 0u8;
            for c in rank_str.chars() {
                if let Some(skip) = c.to_digit(10) {
                    file += skip as u8;
                } else {
                    let piece =
                        Piece::from_char(c).ok_or_else(|| format!("carácter de pieza inválido: '{c}'"))?;
                    if file >= 8 {
                        return Err("FEN inválida: una fila describe más de 8 columnas".to_string());
                    }
                    let sq = Square::new(file, rank);
                    pieces[piece.color as usize][piece.kind as usize].set(sq);
                    mailbox[sq.0 as usize] = Some(piece);
                    file += 1;
                }
            }
            if file != 8 {
                return Err("FEN inválida: una fila no suma 8 columnas".to_string());
            }
        }

        let side_to_move = match fields[1] {
            "w" => Color::White,
            "b" => Color::Black,
            other => return Err(format!("color activo inválido: '{other}'")),
        };

        if pieces[Color::White as usize][PieceType::King as usize].count() != 1 {
            return Err("FEN inválida: se requiere exactamente un rey blanco".to_string());
        }
        if pieces[Color::Black as usize][PieceType::King as usize].count() != 1 {
            return Err("FEN inválida: se requiere exactamente un rey negro".to_string());
        }

        let castling = sanitize_castling_rights(CastlingRights::from_fen_str(fields[2]), &mailbox);

        let en_passant = match fields[3] {
            "-" => None,
            s => Some(
                s.parse::<Square>()
                    .map_err(|_| format!("casilla al paso inválida: '{s}'"))?,
            ),
        };

        let halfmove_clock = fields.get(4).and_then(|s| s.parse().ok()).unwrap_or(0);
        let fullmove_number = fields.get(5).and_then(|s| s.parse().ok()).unwrap_or(1);

        let mut board = Board {
            pieces,
            mailbox,
            side_to_move,
            castling,
            en_passant,
            halfmove_clock,
            fullmove_number,
            hash: 0,
        };
        board.hash = board.compute_hash_from_scratch();
        Ok(board)
    }

    /// Recomputes the Zobrist hash from the current board state in O(64).
    /// Used to seed `hash` on construction and, in tests, to check that the
    /// incremental updates in make/unmake never drift from the true value.
    pub fn compute_hash_from_scratch(&self) -> u64 {
        let mut hash = 0u64;
        for sq in 0..64u8 {
            if let Some(piece) = self.mailbox[sq as usize] {
                hash ^= zobrist::piece_square_key(piece.color, piece.kind, Square(sq));
            }
        }
        if self.side_to_move == Color::Black {
            hash ^= zobrist::side_to_move_key();
        }
        hash ^= zobrist::castling_key(self.castling);
        hash ^= zobrist::en_passant_key(self.en_passant);
        hash
    }

    pub fn to_fen(&self) -> String {
        let mut s = String::new();
        for rank_from_top in 0..8u8 {
            let rank = 7 - rank_from_top;
            let mut empty = 0u8;
            for file in 0..8u8 {
                match self.piece_at(Square::new(file, rank)) {
                    Some(p) => {
                        if empty > 0 {
                            s.push_str(&empty.to_string());
                            empty = 0;
                        }
                        s.push(p.to_char());
                    }
                    None => empty += 1,
                }
            }
            if empty > 0 {
                s.push_str(&empty.to_string());
            }
            if rank_from_top != 7 {
                s.push('/');
            }
        }
        s.push(' ');
        s.push(match self.side_to_move {
            Color::White => 'w',
            Color::Black => 'b',
        });
        s.push(' ');
        s.push_str(&self.castling.to_fen_str());
        s.push(' ');
        match self.en_passant {
            Some(sq) => s.push_str(&sq.to_string()),
            None => s.push('-'),
        }
        s.push(' ');
        s.push_str(&self.halfmove_clock.to_string());
        s.push(' ');
        s.push_str(&self.fullmove_number.to_string());
        s
    }

    pub fn piece_at(&self, sq: Square) -> Option<Piece> {
        self.mailbox[sq.0 as usize]
    }

    pub fn pieces_of(&self, color: Color, kind: PieceType) -> Bitboard {
        self.pieces[color as usize][kind as usize]
    }

    pub fn color_occupied(&self, color: Color) -> Bitboard {
        self.pieces[color as usize]
            .iter()
            .fold(Bitboard::EMPTY, |acc, &bb| acc | bb)
    }

    pub fn occupied(&self) -> Bitboard {
        self.color_occupied(Color::White) | self.color_occupied(Color::Black)
    }

    fn add_piece(&mut self, piece: Piece, sq: Square) {
        self.pieces[piece.color as usize][piece.kind as usize].set(sq);
        self.mailbox[sq.0 as usize] = Some(piece);
        self.hash ^= zobrist::piece_square_key(piece.color, piece.kind, sq);
    }

    fn remove_piece(&mut self, piece: Piece, sq: Square) {
        self.pieces[piece.color as usize][piece.kind as usize].clear(sq);
        self.mailbox[sq.0 as usize] = None;
        self.hash ^= zobrist::piece_square_key(piece.color, piece.kind, sq);
    }

    fn castle_rook_squares(color: Color, flag: MoveFlag) -> (Square, Square) {
        match (color, flag) {
            (Color::White, MoveFlag::KingCastle) => (Square::H1, Square::new(5, 0)),
            (Color::White, MoveFlag::QueenCastle) => (Square::A1, Square::new(3, 0)),
            (Color::Black, MoveFlag::KingCastle) => (Square::H8, Square::new(5, 7)),
            (Color::Black, MoveFlag::QueenCastle) => (Square::A8, Square::new(3, 7)),
            _ => unreachable!("castle_rook_squares called with a non-castling flag"),
        }
    }

    fn update_castling_rights(&mut self, from: Square, to: Square, moving: Piece) {
        let before = self.castling;
        if moving.kind == PieceType::King {
            match moving.color {
                Color::White => self
                    .castling
                    .remove(CastlingRights::WHITE_KINGSIDE | CastlingRights::WHITE_QUEENSIDE),
                Color::Black => self
                    .castling
                    .remove(CastlingRights::BLACK_KINGSIDE | CastlingRights::BLACK_QUEENSIDE),
            }
        }
        for sq in [from, to] {
            if sq == Square::A1 {
                self.castling.remove(CastlingRights::WHITE_QUEENSIDE);
            } else if sq == Square::H1 {
                self.castling.remove(CastlingRights::WHITE_KINGSIDE);
            } else if sq == Square::A8 {
                self.castling.remove(CastlingRights::BLACK_QUEENSIDE);
            } else if sq == Square::H8 {
                self.castling.remove(CastlingRights::BLACK_KINGSIDE);
            }
        }
        self.hash ^= zobrist::castling_key(before) ^ zobrist::castling_key(self.castling);
    }

    fn set_en_passant(&mut self, new_en_passant: Option<Square>) {
        self.hash ^= zobrist::en_passant_key(self.en_passant);
        self.en_passant = new_en_passant;
        self.hash ^= zobrist::en_passant_key(self.en_passant);
    }

    /// Mechanically applies `mv`, assumed to be at least pseudo-legal for the
    /// side to move. Legality (check safety) is the move generator's job, not
    /// this method's.
    pub fn make_move(&mut self, mv: Move) -> Undo {
        let moving = self
            .piece_at(mv.from)
            .expect("make_move: no hay pieza en la casilla de origen");
        let color = self.side_to_move;

        let prev_castling = self.castling;
        let prev_en_passant = self.en_passant;
        let prev_halfmove_clock = self.halfmove_clock;
        let prev_hash = self.hash;

        let capture_square = if mv.flag == MoveFlag::EnPassant {
            Square::new(mv.to.file(), mv.from.rank())
        } else {
            mv.to
        };
        let captured = if mv.flag.is_capture() {
            self.piece_at(capture_square)
        } else {
            None
        };
        if let Some(cap) = captured {
            self.remove_piece(cap, capture_square);
        }

        self.remove_piece(moving, mv.from);
        let placed_kind = mv.flag.promotion_piece().unwrap_or(moving.kind);
        self.add_piece(Piece::new(moving.color, placed_kind), mv.to);

        if mv.flag.is_castle() {
            let (rook_from, rook_to) = Self::castle_rook_squares(color, mv.flag);
            let rook = self
                .piece_at(rook_from)
                .expect("make_move: falta la torre para enrocar");
            self.remove_piece(rook, rook_from);
            self.add_piece(rook, rook_to);
        }

        self.update_castling_rights(mv.from, mv.to, moving);

        let new_en_passant = if mv.flag == MoveFlag::DoublePawnPush {
            Some(Square::new(mv.from.file(), (mv.from.rank() + mv.to.rank()) / 2))
        } else {
            None
        };
        self.set_en_passant(new_en_passant);

        self.halfmove_clock = if moving.kind == PieceType::Pawn || captured.is_some() {
            0
        } else {
            self.halfmove_clock + 1
        };

        if color == Color::Black {
            self.fullmove_number += 1;
        }

        self.side_to_move = color.opposite();
        self.hash ^= zobrist::side_to_move_key();

        Undo {
            captured,
            capture_square,
            prev_castling,
            prev_en_passant,
            prev_halfmove_clock,
            prev_hash,
        }
    }

    pub fn unmake_move(&mut self, mv: Move, undo: Undo) {
        let color = self.side_to_move.opposite();
        self.side_to_move = color;

        let placed = self
            .piece_at(mv.to)
            .expect("unmake_move: la casilla de destino está vacía");
        let moving_kind = if mv.flag.is_promotion() {
            PieceType::Pawn
        } else {
            placed.kind
        };
        self.remove_piece(placed, mv.to);
        self.add_piece(Piece::new(color, moving_kind), mv.from);

        if let Some(cap) = undo.captured {
            self.add_piece(cap, undo.capture_square);
        }

        if mv.flag.is_castle() {
            let (rook_from, rook_to) = Self::castle_rook_squares(color, mv.flag);
            let rook = self
                .piece_at(rook_to)
                .expect("unmake_move: falta la torre para deshacer el enroque");
            self.remove_piece(rook, rook_to);
            self.add_piece(rook, rook_from);
        }

        self.castling = undo.prev_castling;
        self.en_passant = undo.prev_en_passant;
        self.halfmove_clock = undo.prev_halfmove_clock;
        if color == Color::Black {
            self.fullmove_number -= 1;
        }
        self.hash = undo.prev_hash;
    }

    /// Passes the turn to the opponent without moving any piece. Used for
    /// static mobility evaluation and, later, null-move search pruning.
    pub fn make_null_move(&mut self) -> NullUndo {
        let prev_en_passant = self.en_passant;
        let prev_hash = self.hash;
        self.set_en_passant(None);
        self.side_to_move = self.side_to_move.opposite();
        self.hash ^= zobrist::side_to_move_key();
        NullUndo { prev_en_passant, prev_hash }
    }

    pub fn unmake_null_move(&mut self, undo: NullUndo) {
        self.side_to_move = self.side_to_move.opposite();
        self.en_passant = undo.prev_en_passant;
        self.hash = undo.prev_hash;
    }
}

impl fmt::Display for Board {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        for rank_from_top in 0..8u8 {
            let rank = 7 - rank_from_top;
            write!(f, "{} ", rank + 1)?;
            for file in 0..8u8 {
                let c = self
                    .piece_at(Square::new(file, rank))
                    .map(|p| p.to_char())
                    .unwrap_or('.');
                write!(f, "{c} ")?;
            }
            writeln!(f)?;
        }
        writeln!(f, "  a b c d e f g h")?;
        write!(f, "{}", self.to_fen())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn startpos_fen_round_trips() {
        let board = Board::start_pos();
        assert_eq!(board.to_fen(), STARTPOS_FEN);
    }

    #[test]
    fn from_fen_rejects_garbage() {
        assert!(Board::from_fen("not a fen").is_err());
        assert!(Board::from_fen("8/8/8/8/8/8/8 w - - 0 1").is_err()); // only 7 ranks
        assert!(Board::from_fen("8/8/8/8/8/8/8/9 w - - 0 1").is_err()); // bad rank
    }

    #[test]
    fn from_fen_rejects_positions_without_exactly_one_king_per_side() {
        assert!(Board::from_fen("8/8/8/8/8/8/8/8 w - - 0 1").is_err()); // no kings at all
        assert!(Board::from_fen("4k3/8/8/8/8/8/8/8 w - - 0 1").is_err()); // no white king
        assert!(Board::from_fen("4kk2/8/8/8/8/8/8/4K3 w - - 0 1").is_err()); // two black kings
    }

    #[test]
    fn from_fen_drops_castling_rights_with_no_rook_on_the_corner() {
        // King on e1 with a claimed kingside right, but no rook on h1: the
        // right must be stripped so `generate_castling`/`make_move` never
        // try to relocate a rook that isn't there.
        let board = Board::from_fen("4k3/8/8/8/8/8/8/4K3 w K - 0 1").unwrap();
        assert_eq!(board.castling, CastlingRights::default());
    }

    #[test]
    fn from_fen_keeps_castling_rights_that_match_the_board() {
        let board = Board::from_fen(STARTPOS_FEN).unwrap();
        assert_eq!(board.castling, CastlingRights::from_fen_str("KQkq"));
    }

    #[test]
    fn make_and_unmake_double_pawn_push() {
        let mut board = Board::start_pos();
        let mv = Move::new(Square::new(4, 1), Square::new(4, 3), MoveFlag::DoublePawnPush);
        let undo = board.make_move(mv);
        assert_eq!(
            board.to_fen(),
            "rnbqkbnr/pppppppp/8/8/4P3/8/PPPP1PPP/RNBQKBNR b KQkq e3 0 1"
        );
        board.unmake_move(mv, undo);
        assert_eq!(board.to_fen(), STARTPOS_FEN);
    }

    #[test]
    fn make_and_unmake_kingside_castle() {
        let mut board =
            Board::from_fen("r3k2r/8/8/8/8/8/8/R3K2R w KQkq - 0 1").unwrap();
        let mv = Move::new(Square::new(4, 0), Square::new(6, 0), MoveFlag::KingCastle);
        let undo = board.make_move(mv);
        assert_eq!(board.to_fen(), "r3k2r/8/8/8/8/8/8/R4RK1 b kq - 1 1");
        board.unmake_move(mv, undo);
        assert_eq!(board.to_fen(), "r3k2r/8/8/8/8/8/8/R3K2R w KQkq - 0 1");
    }

    #[test]
    fn make_and_unmake_en_passant() {
        let mut board =
            Board::from_fen("rnbqkbnr/ppp1pppp/8/3pP3/8/8/PPPP1PPP/RNBQKBNR w KQkq d6 0 3").unwrap();
        let mv = Move::new(Square::new(4, 4), Square::new(3, 5), MoveFlag::EnPassant);
        let undo = board.make_move(mv);
        assert_eq!(
            board.to_fen(),
            "rnbqkbnr/ppp1pppp/3P4/8/8/8/PPPP1PPP/RNBQKBNR b KQkq - 0 3"
        );
        board.unmake_move(mv, undo);
        assert_eq!(
            board.to_fen(),
            "rnbqkbnr/ppp1pppp/8/3pP3/8/8/PPPP1PPP/RNBQKBNR w KQkq d6 0 3"
        );
    }

    #[test]
    fn make_and_unmake_promotion_capture() {
        let mut board = Board::from_fen("r3k3/1P6/8/8/8/8/8/4K3 w - - 0 1").unwrap();
        let mv = Move::new(
            Square::new(1, 6),
            Square::new(0, 7),
            MoveFlag::PromoCaptureQueen,
        );
        let undo = board.make_move(mv);
        assert_eq!(board.to_fen(), "Q3k3/8/8/8/8/8/8/4K3 b - - 0 1");
        board.unmake_move(mv, undo);
        assert_eq!(board.to_fen(), "r3k3/1P6/8/8/8/8/8/4K3 w - - 0 1");
    }

    #[test]
    fn castling_rights_are_lost_on_rook_capture() {
        let mut board = Board::from_fen("r3k2r/8/8/8/8/8/6q1/R3K2R b KQkq - 0 1").unwrap();
        let mv = Move::new(Square::new(6, 1), Square::H1, MoveFlag::Capture);
        board.make_move(mv);
        assert_eq!(board.castling, CastlingRights::from_fen_str("Qkq"));
    }
}
