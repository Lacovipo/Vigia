use std::fmt;
use std::ops::{BitAnd, BitOr, BitXor, Not};

use crate::types::Square;

#[derive(Clone, Copy, PartialEq, Eq, Default)]
pub struct Bitboard(pub u64);

impl Bitboard {
    pub const EMPTY: Bitboard = Bitboard(0);

    pub fn from_square(sq: Square) -> Bitboard {
        Bitboard(1u64 << sq.0)
    }

    pub fn is_empty(self) -> bool {
        self.0 == 0
    }

    pub fn contains(self, sq: Square) -> bool {
        self.0 & (1u64 << sq.0) != 0
    }

    pub fn set(&mut self, sq: Square) {
        self.0 |= 1u64 << sq.0;
    }

    pub fn clear(&mut self, sq: Square) {
        self.0 &= !(1u64 << sq.0);
    }

    pub fn count(self) -> u32 {
        self.0.count_ones()
    }

    pub fn lsb(self) -> Option<Square> {
        if self.0 == 0 {
            None
        } else {
            Some(Square(self.0.trailing_zeros() as u8))
        }
    }

    pub fn msb(self) -> Option<Square> {
        if self.0 == 0 {
            None
        } else {
            Some(Square(63 - self.0.leading_zeros() as u8))
        }
    }

    pub fn pop_lsb(&mut self) -> Option<Square> {
        let sq = self.lsb()?;
        self.0 &= self.0 - 1;
        Some(sq)
    }
}

/// Consuming a `Bitboard` as an iterator yields its set squares, lowest first.
impl Iterator for Bitboard {
    type Item = Square;

    fn next(&mut self) -> Option<Square> {
        self.pop_lsb()
    }
}

impl BitOr for Bitboard {
    type Output = Bitboard;
    fn bitor(self, rhs: Bitboard) -> Bitboard {
        Bitboard(self.0 | rhs.0)
    }
}

impl BitAnd for Bitboard {
    type Output = Bitboard;
    fn bitand(self, rhs: Bitboard) -> Bitboard {
        Bitboard(self.0 & rhs.0)
    }
}

impl BitXor for Bitboard {
    type Output = Bitboard;
    fn bitxor(self, rhs: Bitboard) -> Bitboard {
        Bitboard(self.0 ^ rhs.0)
    }
}

impl Not for Bitboard {
    type Output = Bitboard;
    fn not(self) -> Bitboard {
        Bitboard(!self.0)
    }
}

impl fmt::Debug for Bitboard {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        for rank_from_top in 0..8u8 {
            let rank = 7 - rank_from_top;
            for file in 0..8u8 {
                let sq = Square::new(file, rank);
                write!(f, "{}", if self.contains(sq) { '1' } else { '.' })?;
            }
            writeln!(f)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn set_and_contains() {
        let mut bb = Bitboard::EMPTY;
        assert!(bb.is_empty());
        bb.set(Square::new(4, 3));
        assert!(bb.contains(Square::new(4, 3)));
        assert!(!bb.contains(Square::new(0, 0)));
        assert_eq!(bb.count(), 1);
    }

    #[test]
    fn msb_and_lsb_of_multi_bit_board() {
        let mut bb = Bitboard::EMPTY;
        bb.set(Square::new(0, 0));
        bb.set(Square::new(4, 4));
        bb.set(Square::new(7, 7));
        assert_eq!(bb.lsb(), Some(Square::new(0, 0)));
        assert_eq!(bb.msb(), Some(Square::new(7, 7)));
    }

    #[test]
    fn clear_removes_square() {
        let mut bb = Bitboard::from_square(Square::A1);
        bb.clear(Square::A1);
        assert!(bb.is_empty());
    }

    #[test]
    fn iterator_yields_all_set_squares_in_order() {
        let mut bb = Bitboard::EMPTY;
        bb.set(Square::new(2, 0));
        bb.set(Square::new(5, 0));
        bb.set(Square::new(0, 1));
        let squares: Vec<Square> = bb.into_iter().collect();
        assert_eq!(
            squares,
            vec![Square::new(2, 0), Square::new(5, 0), Square::new(0, 1)]
        );
    }

    #[test]
    fn bitwise_ops_work() {
        let a = Bitboard::from_square(Square::new(0, 0));
        let b = Bitboard::from_square(Square::new(1, 0));
        let both = a | b;
        assert_eq!(both.count(), 2);
        assert_eq!((both & a), a);
        assert!((a & b).is_empty());
    }
}
