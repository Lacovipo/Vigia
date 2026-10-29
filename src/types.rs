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

    /// Parses the FEN castling token strictly: `-` for no rights, otherwise
    /// a non-empty combination of `KQkq` without repeats (any order).
    /// `None` for anything else — the old behavior of just extracting
    /// whichever recognized letters appeared meant garbage like `KXq`
    /// passed through silently as if it were a valid FEN.
    pub fn from_fen_str(s: &str) -> Option<CastlingRights> {
        if s == "-" {
            return Some(CastlingRights(0));
        }
        if s.is_empty() {
            return None;
        }
        let mut rights = 0u8;
        for c in s.chars() {
            let bit = match c {
                'K' => Self::WHITE_KINGSIDE,
                'Q' => Self::WHITE_QUEENSIDE,
                'k' => Self::BLACK_KINGSIDE,
                'q' => Self::BLACK_QUEENSIDE,
                _ => return None,
            };
            if rights & bit != 0 {
                return None; // duplicated right
            }
            rights |= bit;
        }
        Some(CastlingRights(rights))
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

/// Capacity of a [`MoveList`]. The most *legal* moves any position offers is
/// 218; the pseudo-legal generator can exceed that only by what a pin or a
/// check takes away, so 320 leaves a wide margin.
pub const MAX_MOVES: usize = 320;

/// A list of moves held entirely on the stack.
///
/// The search builds one per node, and quiescence (most of the nodes) builds
/// one too. As a `Vec` that was a heap allocation and its release at every
/// node: 35–45 ns each against ~12 ns for the array on this machine
/// (micro-benchmark kept in `docs/MejorasPendientes.md`, 0.37), out of a node
/// that costs ~590 ns. The storage is zero-initialised so the type needs no
/// `unsafe`; that costs one 960-byte memset even for an empty list, so build
/// one only where a list is really needed.
#[derive(Clone)]
pub struct MoveList {
    moves: [Move; MAX_MOVES],
    len: usize,
}

impl MoveList {
    const FILLER: Move = Move { from: Square(0), to: Square(0), flag: MoveFlag::Quiet };

    #[inline(always)]
    pub fn new() -> MoveList {
        MoveList { moves: [Self::FILLER; MAX_MOVES], len: 0 }
    }

    /// Appends `mv`. Past [`MAX_MOVES`] it panics (index out of range) rather
    /// than silently dropping a move; no position can get there.
    #[inline(always)]
    pub fn push(&mut self, mv: Move) {
        self.moves[self.len] = mv;
        self.len += 1;
    }

    /// Keeps, in order, the moves for which `keep` is true. `keep` runs once
    /// per move, front to back, exactly like `Vec::retain`: callers rely on
    /// that when the test plays and takes back moves on a board.
    pub fn retain<F: FnMut(&Move) -> bool>(&mut self, mut keep: F) {
        let mut write = 0;
        for read in 0..self.len {
            let mv = self.moves[read];
            if keep(&mv) {
                self.moves[write] = mv;
                write += 1;
            }
        }
        self.len = write;
    }

    pub fn clear(&mut self) {
        self.len = 0;
    }
}

impl Default for MoveList {
    fn default() -> MoveList {
        MoveList::new()
    }
}

impl std::ops::Deref for MoveList {
    type Target = [Move];

    #[inline(always)]
    fn deref(&self) -> &[Move] {
        &self.moves[..self.len]
    }
}

impl std::ops::DerefMut for MoveList {
    #[inline(always)]
    fn deref_mut(&mut self) -> &mut [Move] {
        &mut self.moves[..self.len]
    }
}

impl PartialEq for MoveList {
    fn eq(&self, other: &MoveList) -> bool {
        **self == **other
    }
}

impl PartialEq<Vec<Move>> for MoveList {
    fn eq(&self, other: &Vec<Move>) -> bool {
        **self == other[..]
    }
}

impl fmt::Debug for MoveList {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.debug_list().entries(self.iter()).finish()
    }
}

impl<'a> IntoIterator for &'a MoveList {
    type Item = &'a Move;
    type IntoIter = std::slice::Iter<'a, Move>;

    fn into_iter(self) -> Self::IntoIter {
        self.iter()
    }
}

/// By-value iterator over a [`MoveList`] (it takes the list with it).
pub struct MoveListIntoIter {
    list: MoveList,
    pos: usize,
}

impl Iterator for MoveListIntoIter {
    type Item = Move;

    #[inline]
    fn next(&mut self) -> Option<Move> {
        if self.pos < self.list.len {
            let mv = self.list.moves[self.pos];
            self.pos += 1;
            Some(mv)
        } else {
            None
        }
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        let left = self.list.len - self.pos;
        (left, Some(left))
    }
}

impl ExactSizeIterator for MoveListIntoIter {}

impl IntoIterator for MoveList {
    type Item = Move;
    type IntoIter = MoveListIntoIter;

    fn into_iter(self) -> MoveListIntoIter {
        MoveListIntoIter { list: self, pos: 0 }
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
    fn castling_rights_from_fen_str_rejects_garbage_and_duplicates() {
        assert!(CastlingRights::from_fen_str("KXq").is_none());
        assert!(CastlingRights::from_fen_str("").is_none());
        assert!(CastlingRights::from_fen_str("KK").is_none());
        assert!(CastlingRights::from_fen_str("-q").is_none());
        assert_eq!(CastlingRights::from_fen_str("-"), Some(CastlingRights(0)));
        assert_eq!(
            CastlingRights::from_fen_str("KQkq").map(|r| r.to_fen_str()),
            Some("KQkq".to_string())
        );
    }

    #[test]
    fn move_display_includes_promotion_suffix() {
        let mv = Move::new(Square::new(4, 6), Square::new(4, 7), MoveFlag::PromoQueen);
        assert_eq!(mv.to_string(), "e7e8q");
    }

    fn sample_moves(n: usize) -> Vec<Move> {
        (0..n).map(|i| Move::new(Square((i % 64) as u8), Square(((i * 7 + 3) % 64) as u8), MoveFlag::Quiet)).collect()
    }

    #[test]
    fn move_list_behaves_like_a_vec_of_moves() {
        let expected = sample_moves(40);
        let mut list = MoveList::new();
        assert!(list.is_empty());
        for &mv in &expected {
            list.push(mv);
        }
        assert_eq!(list.len(), 40);
        assert_eq!(list[7], expected[7]);
        assert_eq!(list.to_vec(), expected);
        assert!(list.contains(&expected[39]));
        // By value, by reference and through the slice all agree.
        assert_eq!(list.clone().into_iter().collect::<Vec<_>>(), expected);
        assert_eq!((&list).into_iter().copied().collect::<Vec<_>>(), expected);
        assert_eq!(list.clone().into_iter().len(), 40);
    }

    #[test]
    fn move_list_retain_keeps_order_and_visits_each_move_once_in_order() {
        let all = sample_moves(100);
        let mut list = MoveList::new();
        for &mv in &all {
            list.push(mv);
        }
        let mut seen = Vec::new();
        list.retain(|&mv| {
            seen.push(mv);
            mv.from.0 % 3 == 0
        });
        assert_eq!(seen, all, "keep must run once per move, front to back");
        let kept: Vec<Move> = all.iter().copied().filter(|m| m.from.0 % 3 == 0).collect();
        assert_eq!(list.to_vec(), kept);
        list.clear();
        assert!(list.is_empty());
    }

    #[test]
    fn move_list_holds_the_largest_legal_position() {
        // 218 is the known maximum of legal moves; the capacity must clear it
        // with room for the pseudo-legal extras.
        let mut list = MoveList::new();
        for mv in sample_moves(MAX_MOVES) {
            list.push(mv);
        }
        assert_eq!(list.len(), MAX_MOVES);
        const { assert!(MAX_MOVES >= 218 + 64) };
    }
}
