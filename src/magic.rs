//! Magic bitboards: sliding attacks as one multiply, one shift and one
//! table lookup, replacing the classical loop over four rays that has to
//! find the first blocker on each of them.
//!
//! The idea is a perfect hash per square. Only the squares *between* the
//! slider and the board edge can block it — whatever sits on the edge
//! square itself never changes where the piece stops — so a rook on a1
//! cares about at most 12 squares and a bishop about at most 9. Masking
//! the occupancy down to those squares leaves at most 4096 distinct inputs
//! per square, and a magic is a multiplier that maps every one of them,
//! after shifting the product down to the top `bits` bits, onto a distinct
//! slot of a per-square table. Two different occupancies are allowed to
//! share a slot when they produce the *same* attack set (a constructive
//! collision); what the search below rules out is two occupancies with
//! different attacks landing on the same index.
//!
//! Everything here is built at compile time: `MAGIC_*` are the multipliers
//! found once by the seeded search in the tests (see
//! `magics_are_reproducible_from_the_documented_seed`), and the attack
//! tables are filled by `const fn` from those multipliers, so the running
//! engine pays no initialisation and needs no lazy statics on the hot path.

use crate::bitboard::Bitboard;
use crate::types::Square;

/// Step vectors as `(file, rank)` deltas.
const ROOK_DIRS: [(i32, i32); 4] = [(0, 1), (0, -1), (1, 0), (-1, 0)];
const BISHOP_DIRS: [(i32, i32); 4] = [(1, 1), (1, -1), (-1, 1), (-1, -1)];

/// Sum over the 64 squares of `2^bits`: how many table slots the whole
/// piece type needs. Written out as constants rather than computed so the
/// array types below are plain literals; `table_size` asserts them.
const BISHOP_TABLE_SIZE: usize = 5248;
const ROOK_TABLE_SIZE: usize = 102400;

/// Squares whose contents can block a slider standing on this square,
/// excluding the edge square each ray ends on.
const fn blocker_mask(sq: u8, dirs: &[(i32, i32); 4]) -> u64 {
    let file = (sq % 8) as i32;
    let rank = (sq / 8) as i32;
    let mut bb: u64 = 0;
    let mut i = 0;
    while i < 4 {
        let (df, dr) = dirs[i];
        let mut f = file + df;
        let mut r = rank + dr;
        // Stop one short of the edge: a blocker there is invisible to the
        // attack set, so leaving it out of the mask halves the table for
        // every extra edge square the ray reaches.
        while f + df >= 0 && f + df < 8 && r + dr >= 0 && r + dr < 8 {
            bb |= 1u64 << (r * 8 + f) as u32;
            f += df;
            r += dr;
        }
        i += 1;
    }
    bb
}

/// Attack set of a slider on `sq` against `occupied`, walked ray by ray.
/// This is the reference definition of what a sliding piece attacks; the
/// magic tables are filled from it and the tests check the lookups back
/// against it.
const fn walk_attacks(sq: u8, occupied: u64, dirs: &[(i32, i32); 4]) -> u64 {
    let file = (sq % 8) as i32;
    let rank = (sq / 8) as i32;
    let mut bb: u64 = 0;
    let mut i = 0;
    while i < 4 {
        let (df, dr) = dirs[i];
        let mut f = file + df;
        let mut r = rank + dr;
        while f >= 0 && f < 8 && r >= 0 && r < 8 {
            let bit = 1u64 << (r * 8 + f) as u32;
            bb |= bit;
            // The blocker itself is attacked (it may be capturable); what
            // lies behind it is not.
            if occupied & bit != 0 {
                break;
            }
            f += df;
            r += dr;
        }
        i += 1;
    }
    bb
}

/// Per-square hash parameters. Kept as one 24-byte record so a lookup
/// touches a single cache line for the square and then one for the table.
#[derive(Clone, Copy)]
struct Magic {
    mask: u64,
    magic: u64,
    /// Where this square's slots begin inside the shared attack table.
    offset: u32,
    /// `64 - mask.count_ones()`, precomputed.
    shift: u32,
}

impl Magic {
    const EMPTY: Magic = Magic {
        mask: 0,
        magic: 0,
        offset: 0,
        shift: 0,
    };

    #[inline(always)]
    const fn index(&self, occupied: u64) -> usize {
        (((occupied & self.mask).wrapping_mul(self.magic)) >> self.shift) as usize
            + self.offset as usize
    }
}

const fn build_magics(magics: &[u64; 64], dirs: &[(i32, i32); 4]) -> [Magic; 64] {
    let mut out = [Magic::EMPTY; 64];
    let mut sq = 0usize;
    let mut offset = 0u32;
    while sq < 64 {
        let mask = blocker_mask(sq as u8, dirs);
        let bits = mask.count_ones();
        out[sq] = Magic {
            mask,
            magic: magics[sq],
            offset,
            shift: 64 - bits,
        };
        offset += 1u32 << bits;
        sq += 1;
    }
    out
}

/// Fills a whole piece type's table by enumerating, for each square, every
/// subset of its blocker mask (carry-rippler: `sub = (sub - mask) & mask`
/// cycles through all of them exactly once) and writing the walked attack
/// set at the slot the magic maps it to.
const fn build_table<const N: usize>(
    magics: &[Magic; 64],
    dirs: &[(i32, i32); 4],
) -> [u64; N] {
    let mut table = [0u64; N];
    let mut sq = 0usize;
    while sq < 64 {
        let m = magics[sq];
        let mut sub = 0u64;
        loop {
            table[m.index(sub)] = walk_attacks(sq as u8, sub, dirs);
            sub = sub.wrapping_sub(m.mask) & m.mask;
            if sub == 0 {
                break;
            }
        }
        sq += 1;
    }
    table
}

const BISHOP_MAGIC: [Magic; 64] = build_magics(&BISHOP_MAGICS, &BISHOP_DIRS);
const ROOK_MAGIC: [Magic; 64] = build_magics(&ROOK_MAGICS, &ROOK_DIRS);

static BISHOP_ATTACKS: [u64; BISHOP_TABLE_SIZE] =
    build_table::<BISHOP_TABLE_SIZE>(&BISHOP_MAGIC, &BISHOP_DIRS);
static ROOK_ATTACKS: [u64; ROOK_TABLE_SIZE] =
    build_table::<ROOK_TABLE_SIZE>(&ROOK_MAGIC, &ROOK_DIRS);

#[inline(always)]
pub fn bishop_attacks(sq: Square, occupied: Bitboard) -> Bitboard {
    Bitboard(BISHOP_ATTACKS[BISHOP_MAGIC[sq.0 as usize].index(occupied.0)])
}

#[inline(always)]
pub fn rook_attacks(sq: Square, occupied: Bitboard) -> Bitboard {
    Bitboard(ROOK_ATTACKS[ROOK_MAGIC[sq.0 as usize].index(occupied.0)])
}

// ---------------------------------------------------------------------
// The multipliers.
//
// Found by the seeded search reproduced in the tests below: a xorshift64*
// stream seeded with 0x0007_2C6D_1A3B_5F91, drawing sparse candidates
// (`rand & rand & rand`, which is what makes a magic likely to spread the
// masked bits apart) and keeping the first one that hashes every occupancy
// of the square without a destructive collision. Bishops are searched
// first, squares a1..h8, then rooks; that order is what makes the numbers
// below reproducible bit for bit.
// ---------------------------------------------------------------------

const BISHOP_MAGICS: [u64; 64] = [
    0x2008100888090120, 0x0122101206004800,
    0x0422348902010020, 0x4014124200700010,
    0x0044504103001080, 0x000a091008040001,
    0x0000424820088480, 0x080025040a094004,
    0x0009062084011200, 0x0002202220810100,
    0x049004111a020a00, 0x2092222082000280,
    0x8488040420040000, 0x20000090100812c0,
    0x8092014218602808, 0x0011004204906884,
    0xc610044490904100, 0x2014f01810041040,
    0x01101a1102420040, 0x0010200844004001,
    0x0208106101400004, 0x100600290080a424,
    0x0045000208210400, 0x140080044200b023,
    0x2020104120024224, 0x1102020021280e00,
    0x0004010022080300, 0x2a04822018020020,
    0x0041001001004002, 0x2052920001080204,
    0x010119010400b804, 0x0120808000320801,
    0x014842082010a000, 0x0008901000841480,
    0x400a0082085003a6, 0x0400208020080200,
    0xa0810104000a0020, 0x1890901300088095,
    0x0410014100520080, 0x2000c40080090080,
    0x00141c2108001400, 0x046280b030000840,
    0x2a00840049080800, 0x0400084200800800,
    0x0025200414000044, 0x8420481000202110,
    0x240404008c200a01, 0x021000a200800040,
    0x18020290a8383000, 0x2800412488204000,
    0x0004121084040000, 0x0000900042021000,
    0x1600840620820020, 0x0640411082008040,
    0x1020051042004008, 0x4008420802002110,
    0x3880808050122080, 0x1400004404010800,
    0x0000024042009000, 0x2800042320840440,
    0x1100000230c20220, 0x8000005020880129,
    0x2020230210010500, 0x4204101040448080,
];

const ROOK_MAGICS: [u64; 64] = [
    0x0080018050224000, 0x4040001000200040,
    0x0100081020004100, 0x2080080004801000,
    0x0900040800021100, 0x0200041002000108,
    0x8a800c8002004100, 0x0100010010204082,
    0x0000801020804000, 0x0812002042008100,
    0x0004802000100080, 0x2804808088001000,
    0x0000808008000400, 0x0921000900040002,
    0x0001000200040100, 0x0082000204004081,
    0x2040008000804030, 0x2100848020004002,
    0x0420008080100020, 0x1010828010020800,
    0x8200808008000400, 0x4202010100040008,
    0x2200240010012288, 0x4120120011104094,
    0x40c0400080002088, 0x4340002100408108,
    0x0010008080182001, 0x0c08100100200900,
    0x8000080080800400, 0x0c06040080800200,
    0x140200a200082401, 0x4400010200288044,
    0x0080002001400040, 0x0010002004404000,
    0x0001a002c1001100, 0x0250080080801001,
    0x0084000800800480, 0x1204800400800201,
    0x1382000802000401, 0x4221000041000082,
    0x0500209040008002, 0x0041008040010020,
    0xa100408200220010, 0x1223041000210008,
    0x0050080005010010, 0x0102000810020004,
    0x0080020801040010, 0x0a18004084020001,
    0x0009400660800080, 0x0000824001200480,
    0x2100200100144100, 0x3008021002088080,
    0x0000080010050100, 0x1010020080040080,
    0x0400088201304400, 0x0200004100a40e00,
    0x4400401100208001, 0x0040110482220142,
    0x0080104408200101, 0x010a080410002101,
    0x0601007800100205, 0x118a001004080102,
    0x02001010c8010204, 0x0001904100882402,
];

#[cfg(test)]
mod tests {
    use super::*;

    /// Walks the carry-rippler sequence of every subset of `mask`.
    fn subsets(mask: u64) -> Vec<u64> {
        let mut out = Vec::with_capacity(1 << mask.count_ones());
        let mut sub = 0u64;
        loop {
            out.push(sub);
            sub = sub.wrapping_sub(mask) & mask;
            if sub == 0 {
                break;
            }
        }
        out
    }

    /// The property the whole scheme rests on: for every square and every
    /// occupancy the tables can be reached with, the lookup answers exactly
    /// what walking the rays answers. Exhaustive — all 107 648 entries —
    /// which also proves no magic collides destructively, since a collision
    /// would have overwritten one of the two attack sets involved.
    #[test]
    fn every_reachable_occupancy_matches_the_walked_attack_set() {
        for sq in 0..64u8 {
            let square = Square(sq);
            for (magics, dirs, lookup) in [
                (
                    &BISHOP_MAGIC,
                    &BISHOP_DIRS,
                    bishop_attacks as fn(Square, Bitboard) -> Bitboard,
                ),
                (&ROOK_MAGIC, &ROOK_DIRS, rook_attacks as fn(Square, Bitboard) -> Bitboard),
            ] {
                let mask = magics[sq as usize].mask;
                for occupied in subsets(mask) {
                    assert_eq!(
                        lookup(square, Bitboard(occupied)).0,
                        walk_attacks(sq, occupied, dirs),
                        "casilla {sq}, ocupación {occupied:#018x}"
                    );
                }
            }
        }
    }

    /// Pieces off the relevant squares — anything on the edge the ray ends
    /// on, or anywhere off the ray — must not change the answer. That is
    /// what lets the table be indexed by the masked occupancy alone.
    #[test]
    fn occupancy_outside_the_mask_does_not_change_the_answer() {
        let mut rng = Xorshift(0x1234_5678_9abc_def1);
        for sq in 0..64u8 {
            let square = Square(sq);
            for _ in 0..64 {
                let noise = rng.next();
                let bishop_mask = BISHOP_MAGIC[sq as usize].mask;
                assert_eq!(
                    bishop_attacks(square, Bitboard(noise & bishop_mask)),
                    bishop_attacks(square, Bitboard(noise)),
                );
                let rook_mask = ROOK_MAGIC[sq as usize].mask;
                assert_eq!(
                    rook_attacks(square, Bitboard(noise & rook_mask)),
                    rook_attacks(square, Bitboard(noise)),
                );
            }
        }
    }

    #[test]
    fn table_sizes_are_exactly_what_the_masks_require() {
        let bishop: usize = (0..64)
            .map(|sq| 1usize << BISHOP_MAGIC[sq].mask.count_ones())
            .sum();
        let rook: usize = (0..64).map(|sq| 1usize << ROOK_MAGIC[sq].mask.count_ones()).sum();
        assert_eq!(bishop, BISHOP_TABLE_SIZE);
        assert_eq!(rook, ROOK_TABLE_SIZE);
        // Every slot is claimed by exactly one square, so the last offset
        // plus its own span has to close the table with nothing left over.
        assert_eq!(
            BISHOP_MAGIC[63].offset as usize + (1 << BISHOP_MAGIC[63].mask.count_ones()),
            BISHOP_TABLE_SIZE
        );
        assert_eq!(
            ROOK_MAGIC[63].offset as usize + (1 << ROOK_MAGIC[63].mask.count_ones()),
            ROOK_TABLE_SIZE
        );
    }

    #[test]
    fn a_rook_on_a1_sees_the_whole_file_and_rank_on_an_empty_board() {
        let attacks = rook_attacks(Square::A1, Bitboard::EMPTY);
        assert_eq!(attacks.count(), 14);
        assert!(attacks.contains(Square::new(0, 7)));
        assert!(attacks.contains(Square::new(7, 0)));
        assert!(!attacks.contains(Square::new(1, 1)));
    }

    #[test]
    fn a_blocker_hides_what_is_behind_it_but_is_itself_attacked() {
        let blocker = Square::new(0, 3); // a4
        let attacks = rook_attacks(Square::A1, Bitboard::from_square(blocker));
        assert!(attacks.contains(blocker));
        assert!(!attacks.contains(Square::new(0, 4)));
    }

    // -----------------------------------------------------------------
    // Provenance of the multipliers.
    // -----------------------------------------------------------------

    /// xorshift64*, so the search below depends on nothing but its seed.
    struct Xorshift(u64);

    impl Xorshift {
        fn next(&mut self) -> u64 {
            let mut x = self.0;
            x ^= x >> 12;
            x ^= x << 25;
            x ^= x >> 27;
            self.0 = x;
            x.wrapping_mul(0x2545_F491_4F6C_DD1D)
        }

        /// ANDing three draws leaves roughly eight bits set. A magic works
        /// by scattering the masked bits into the top of the product
        /// without them interfering, and a sparse multiplier is far likelier
        /// to manage that than a dense one.
        fn sparse(&mut self) -> u64 {
            self.next() & self.next() & self.next()
        }
    }

    fn find_magic(sq: u8, dirs: &[(i32, i32); 4], rng: &mut Xorshift) -> u64 {
        let mask = blocker_mask(sq, dirs);
        let bits = mask.count_ones();
        let shift = 64 - bits;
        let occupancies = subsets(mask);
        let reference: Vec<u64> = occupancies
            .iter()
            .map(|&occ| walk_attacks(sq, occ, dirs))
            .collect();

        let mut table = vec![0u64; 1 << bits];
        // Generation stamps instead of clearing the table on every
        // candidate: the search tries thousands of them.
        let mut stamp = vec![0u32; 1 << bits];
        let mut era = 0u32;
        loop {
            let candidate = rng.sparse();
            // Cheap reject: a candidate that doesn't move at least six bits
            // of the mask into the top byte practically never hashes
            // cleanly, and finding that out the slow way costs a full pass.
            if mask.wrapping_mul(candidate).wrapping_shr(56).count_ones() < 6 {
                continue;
            }
            era += 1;
            let mut ok = true;
            for (i, &occ) in occupancies.iter().enumerate() {
                let idx = (occ.wrapping_mul(candidate) >> shift) as usize;
                if stamp[idx] != era {
                    stamp[idx] = era;
                    table[idx] = reference[i];
                } else if table[idx] != reference[i] {
                    ok = false;
                    break;
                }
            }
            if ok {
                return candidate;
            }
        }
    }

    /// The constants in this file are not borrowed and not magic-in-the-
    /// mystical-sense: this rederives all 128 of them from the seed and
    /// order documented above them, and they have to come out identical.
    #[test]
    fn magics_are_reproducible_from_the_documented_seed() {
        let mut rng = Xorshift(0x0007_2C6D_1A3B_5F91);
        for sq in 0..64u8 {
            assert_eq!(
                find_magic(sq, &BISHOP_DIRS, &mut rng),
                BISHOP_MAGICS[sq as usize],
                "magic de alfil de la casilla {sq}"
            );
        }
        for sq in 0..64u8 {
            assert_eq!(
                find_magic(sq, &ROOK_DIRS, &mut rng),
                ROOK_MAGICS[sq as usize],
                "magic de torre de la casilla {sq}"
            );
        }
    }
}
