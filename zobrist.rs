use crate::types::{CastlingRights, Color, PieceType, Square};

const fn splitmix64(seed: u64) -> u64 {
    let mut z = seed.wrapping_add(0x9E3779B97F4A7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58476D1CE4E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D049BB133111EB);
    z ^ (z >> 31)
}

const SEED: u64 = 0x51_0F_A2_9B_71_CE_44_9D;

/// Deterministic, well-distributed 64-bit constant for a given index.
/// Every Zobrist key used by this engine is derived from a distinct index,
/// so this one function is the sole source of "randomness".
const fn key_at(index: u64) -> u64 {
    splitmix64(SEED ^ index.wrapping_mul(0x9E3779B97F4A7C15).wrapping_add(1))
}

const fn build_piece_square_keys() -> [[[u64; 64]; 6]; 2] {
    let mut table = [[[0u64; 64]; 6]; 2];
    let mut color = 0;
    while color < 2 {
        let mut piece = 0;
        while piece < 6 {
            let mut sq = 0;
            while sq < 64 {
                let index = (color * 6 * 64 + piece * 64 + sq) as u64;
                table[color][piece][sq] = key_at(index);
                sq += 1;
            }
            piece += 1;
        }
        color += 1;
    }
    table
}

const fn build_en_passant_file_keys() -> [u64; 8] {
    let mut table = [0u64; 8];
    let mut file = 0;
    while file < 8 {
        table[file] = key_at(773 + file as u64);
        file += 1;
    }
    table
}

const PIECE_SQUARE_KEYS: [[[u64; 64]; 6]; 2] = build_piece_square_keys();
const SIDE_TO_MOVE_KEY_VALUE: u64 = key_at(768);
const CASTLING_KEYS: [u64; 4] = [key_at(769), key_at(770), key_at(771), key_at(772)];
const EN_PASSANT_FILE_KEYS: [u64; 8] = build_en_passant_file_keys();

pub fn piece_square_key(color: Color, kind: PieceType, sq: Square) -> u64 {
    PIECE_SQUARE_KEYS[color as usize][kind as usize][sq.0 as usize]
}

pub fn side_to_move_key() -> u64 {
    SIDE_TO_MOVE_KEY_VALUE
}

pub fn castling_key(rights: CastlingRights) -> u64 {
    const BITS: [u8; 4] = [
        CastlingRights::WHITE_KINGSIDE,
        CastlingRights::WHITE_QUEENSIDE,
        CastlingRights::BLACK_KINGSIDE,
        CastlingRights::BLACK_QUEENSIDE,
    ];
    let mut key = 0u64;
    for (i, &bit) in BITS.iter().enumerate() {
        if rights.has(bit) {
            key ^= CASTLING_KEYS[i];
        }
    }
    key
}

pub fn en_passant_key(en_passant: Option<Square>) -> u64 {
    match en_passant {
        Some(sq) => EN_PASSANT_FILE_KEYS[sq.file() as usize],
        None => 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn piece_square_keys_are_pairwise_distinct() {
        let mut seen = std::collections::HashSet::new();
        for &color in &[Color::White, Color::Black] {
            for kind in PieceType::ALL {
                for sq in 0..64u8 {
                    let key = piece_square_key(color, kind, Square(sq));
                    assert!(seen.insert(key), "clave Zobrist duplicada");
                }
            }
        }
    }

    #[test]
    fn castling_key_is_symmetric_difference() {
        let none = CastlingRights::from_fen_str("-");
        let all = CastlingRights::from_fen_str("KQkq");
        assert_eq!(castling_key(none), 0);
        assert_ne!(castling_key(all), 0);
        assert_eq!(castling_key(none) ^ castling_key(all), castling_key(all));
    }
}
