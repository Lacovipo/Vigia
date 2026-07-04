use std::fmt;
use std::str::FromStr;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[repr(u8)]
pub enum Color {
    White = 0,
    Black = 1,
}

impl Color {
    pub fn opposite(self) -> Color {
        match self {
            Color::White => Color::Black,
            Color::Black => Color::White,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[repr(u8)]
pub enum PieceType {
    Pawn = 0,
    Knight = 1,
    Bishop = 2,
    Rook = 3,
    Queen = 4,
    King = 5,
}

impl PieceType {
    pub const ALL: [PieceType; 6] = [
        PieceType::Pawn,
        PieceType::Knight,
        PieceType::Bishop,
        PieceType::Rook,
        PieceType::Queen,
        PieceType::King,
    ];
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Piece {
    pub color: Color,
    pub kind: PieceType,
}

impl Piece {
    pub fn new(color: Color, kind: PieceType) -> Piece {
        Piece { color, kind }
    }

    pub fn to_char(self) -> char {
        let c = match self.kind {
            PieceType::Pawn => 'p',
            PieceType::Knight => 'n',
            PieceType::Bishop => 'b',
            PieceType::Rook => 'r',
            PieceType::Queen => 'q',
            PieceType::King => 'k',
        };
        if self.color == Color::White {
            c.to_ascii_uppercase()
        } else {
            c
        }
    }

    pub fn from_char(c: char) -> Option<Piece> {
        let color = if c.is_ascii_uppercase() {
            Color::White
        } else {
            Color::Black
        };
        let kind = match c.to_ascii_lowercase() {
            'p' => PieceType::Pawn,
            'n' => PieceType::Knight,
            'b' => PieceType::Bishop,
            'r' => PieceType::Rook,
            'q' => PieceType::Queen,
            'k' => PieceType::King,
            _ => return None,
        };
        Some(Piece { color, kind })
    }
}

/// A board square, indexed 0..64 in little-endian rank-file order (a1=0, b1=1, ..., h8=63).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Square(pub u8);

impl Square {
    pub const A1: Square = Square(0);
    pub const H1: Square = Square(7);
    pub const A8: Square = Square(56);
    pub const H8: Square = Square(63);

    pub const fn new(file: u8, rank: u8) -> Square {
        Square(rank * 8 + file)
    }

    pub fn file(self) -> u8 {
        self.0 % 8
    }

    pub fn rank(self) -> u8 {
        self.0 / 8
    }
}

impl fmt::Display for Square {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        let file = (b'a' + self.file()) as char;
        let rank = (b'1' + self.rank()) as char;
        write!(f, "{file}{rank}")
    }
}

impl FromStr for Square {
    type Err = ();

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let bytes = s.as_bytes();
        if bytes.len() != 2 {
            return Err(());
        }
        let (file, rank) = (bytes[0], bytes[1]);
        if !(b'a'..=b'h').contains(&file) || !(b'1'..=b'8').contains(&rank) {
            return Err(());
        }
        Ok(Square::new(file - b'a', rank - b'1'))
    }
}

/// Castling rights as a 4-bit set: white kingside/queenside, black kingside/queenside.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct CastlingRights(pub u8);

impl CastlingRights {
    pub const WHITE_KINGSIDE: u8 = 0b0001;
    pub const WHITE_QUEENSIDE: u8 = 0b0010;
    pub const BLACK_KINGSIDE: u8 = 0b0100;
    pub const BLACK_QUEENSIDE: u8 = 0b1000;

    pub fn has(self, right: u8) -> bool {
        self.0 & right != 0
    }

    pub fn remove(&mut self, rights: u8) {
        self.0 &= !rights;
    }

    pub fn from_fen_str(s: &str) -> CastlingRights {
        let mut rights = 0u8;
        if s.contains('K') {
            rights |= Self::WHITE_KINGSIDE;
        }
        if s.contains('Q') {
            rights |= Self::WHITE_QUEENSIDE;
        }
        if s.contains('k') {
            rights |= Self::BLACK_KINGSIDE;
        }
        if s.contains('q') {
            rights |= Self::BLACK_QUEENSIDE;
        }
        CastlingRights(rights)
    }

    pub fn to_fen_str(self) -> String {
        let mut s = String::new();
        if self.has(Self::WHITE_KINGSIDE) {
            s.push('K');
        }
        if self.has(Self::WHITE_QUEENSIDE) {
            s.push('Q');
        }
        if self.has(Self::BLACK_KINGSIDE) {
            s.push('k');
        }
        if self.has(Self::BLACK_QUEENSIDE) {
            s.push('q');
        }
        if s.is_empty() {
            s.push('-');
        }
        s
    }
}

/// Encodes the special-move information for a `Move` using the classic
/// 4-bit chess-programming-wiki scheme, as a Rust enum so every case must
/// be handled explicitly wherever moves are made/unmade.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum MoveFlag {
    Quiet,
    DoublePawnPush,
    KingCastle,
    QueenCastle,
    Capture,
    EnPassant,
    PromoKnight,
    PromoBishop,
    PromoRook,
    PromoQueen,
    PromoCaptureKnight,
    PromoCaptureBishop,
    PromoCaptureRook,
    PromoCaptureQueen,
}

impl MoveFlag {
    pub fn is_capture(self) -> bool {
        matches!(
            self,
            MoveFlag::Capture
                | MoveFlag::EnPassant
                | MoveFlag::PromoCaptureKnight
                | MoveFlag::PromoCaptureBishop
                | MoveFlag::PromoCaptureRook
                | MoveFlag::PromoCaptureQueen
        )
    }

    pub fn is_promotion(self) -> bool {
        self.promotion_piece().is_some()
    }

    pub fn is_castle(self) -> bool {
        matches!(self, MoveFlag::KingCastle | MoveFlag::QueenCastle)
    }

    pub fn promotion_piece(self) -> Option<PieceType> {
        match self {
            MoveFlag::PromoKnight | MoveFlag::PromoCaptureKnight => Some(PieceType::Knight),
            MoveFlag::PromoBishop | MoveFlag::PromoCaptureBishop => Some(PieceType::Bishop),
            MoveFlag::PromoRook | MoveFlag::PromoCaptureRook => Some(PieceType::Rook),
            MoveFlag::PromoQueen | MoveFlag::PromoCaptureQueen => Some(PieceType::Queen),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Move {
    pub from: Square,
    pub to: Square,
    pub flag: MoveFlag,
}

impl Move {
    pub fn new(from: Square, to: Square, flag: MoveFlag) -> Move {
        Move { from, to, flag }
    }

    pub fn is_capture(self) -> bool {
        self.flag.is_capture()
    }

    pub fn promotion(self) -> Option<PieceType> {
        self.flag.promotion_piece()
    }
}

impl fmt::Display for Move {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{}{}", self.from, self.to)?;
        if let Some(p) = self.promotion() {
            let c = match p {
                PieceType::Knight => 'n',
                PieceType::Bishop => 'b',
                PieceType::Rook => 'r',
                PieceType::Queen => 'q',
                _ => unreachable!("promotion_piece never returns Pawn or King"),
            };
            write!(f, "{c}")?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn square_round_trips_through_string() {
        for &s in &["a1", "e4", "h8", "d5"] {
            let sq: Square = s.parse().unwrap();
            assert_eq!(sq.to_string(), s);
        }
    }

    #[test]
    fn square_rejects_garbage() {
        assert!("i9".parse::<Square>().is_err());
        assert!("a".parse::<Square>().is_err());
        assert!("".parse::<Square>().is_err());
    }

    #[test]
    fn square_new_matches_file_rank() {
        let sq = Square::new(4, 3); // e4
        assert_eq!(sq.file(), 4);
        assert_eq!(sq.rank(), 3);
        assert_eq!(sq.to_string(), "e4");
    }

    #[test]
    fn piece_char_round_trips() {
        for &c in &['P', 'n', 'B', 'r', 'Q', 'k'] {
            let p = Piece::from_char(c).unwrap();
            assert_eq!(p.to_char(), c);
        }
        assert!(Piece::from_char('x').is_none());
    }

    #[test]
    fn move_display_includes_promotion_suffix() {
        let mv = Move::new(Square::new(4, 6), Square::new(4, 7), MoveFlag::PromoQueen);
        assert_eq!(mv.to_string(), "e7e8q");
    }
}
