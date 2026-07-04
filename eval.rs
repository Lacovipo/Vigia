use crate::bitboard::Bitboard;
use crate::board::Board;
use crate::movegen;
use crate::types::{Color, PieceType, Square};

/// Centipawn values, indexed like `PieceType`'s discriminant. The king has
/// no material value: it is always present on both sides in equal number,
/// so it never contributes to the material difference.
const PIECE_VALUES: [i32; 6] = [100, 320, 330, 500, 900, 0];

/// Piece-square tables, one per `PieceType`, laid out with index 0 = a1 and
/// index 63 = h8 (matching `Square`'s own encoding) so they apply to White
/// pieces unchanged; a Black piece looks up `square ^ 56`, which mirrors the
/// rank while keeping the file, i.e. the same table viewed from Black's side
/// of the board. Values are classic "simplified evaluation function"
/// centipawn bonuses (Michniewski-style): a cheap, well-tested starting
/// point for piece placement, not tuned specifically for this engine.
#[rustfmt::skip]
const PAWN_PST: [i32; 64] = [
      0,   0,   0,   0,   0,   0,   0,   0,
      5,  10,  10, -20, -20,  10,  10,   5,
      5,  -5, -10,   0,   0, -10,  -5,   5,
      0,   0,   0,  20,  20,   0,   0,   0,
      5,   5,  10,  25,  25,  10,   5,   5,
     10,  10,  20,  30,  30,  20,  10,  10,
     50,  50,  50,  50,  50,  50,  50,  50,
      0,   0,   0,   0,   0,   0,   0,   0,
];
#[rustfmt::skip]
const KNIGHT_PST: [i32; 64] = [
    -50, -40, -30, -30, -30, -30, -40, -50,
    -40, -20,   0,   5,   5,   0, -20, -40,
    -30,   5,  10,  15,  15,  10,   5, -30,
    -30,   0,  15,  20,  20,  15,   0, -30,
    -30,   5,  15,  20,  20,  15,   5, -30,
    -30,   0,  10,  15,  15,  10,   0, -30,
    -40, -20,   0,   0,   0,   0, -20, -40,
    -50, -40, -30, -30, -30, -30, -40, -50,
];
#[rustfmt::skip]
const BISHOP_PST: [i32; 64] = [
    -20, -10, -10, -10, -10, -10, -10, -20,
    -10,   5,   0,   0,   0,   0,   5, -10,
    -10,  10,  10,  10,  10,  10,  10, -10,
    -10,   0,  10,  10,  10,  10,   0, -10,
    -10,   5,   5,  10,  10,   5,   5, -10,
    -10,   0,   5,  10,  10,   5,   0, -10,
    -10,   0,   0,   0,   0,   0,   0, -10,
    -20, -10, -10, -10, -10, -10, -10, -20,
];
#[rustfmt::skip]
const ROOK_PST: [i32; 64] = [
      0,   0,   0,   5,   5,   0,   0,   0,
     -5,   0,   0,   0,   0,   0,   0,  -5,
     -5,   0,   0,   0,   0,   0,   0,  -5,
     -5,   0,   0,   0,   0,   0,   0,  -5,
     -5,   0,   0,   0,   0,   0,   0,  -5,
     -5,   0,   0,   0,   0,   0,   0,  -5,
      5,  10,  10,  10,  10,  10,  10,   5,
      0,   0,   0,   0,   0,   0,   0,   0,
];
#[rustfmt::skip]
const QUEEN_PST: [i32; 64] = [
    -20, -10, -10,  -5,  -5, -10, -10, -20,
    -10,   0,   5,   0,   0,   0,   0, -10,
    -10,   5,   5,   5,   5,   5,   0, -10,
      0,   0,   5,   5,   5,   5,   0,  -5,
     -5,   0,   5,   5,   5,   5,   0,  -5,
    -10,   0,   5,   5,   5,   5,   0, -10,
    -10,   0,   0,   0,   0,   0,   0, -10,
    -20, -10, -10,  -5,  -5, -10, -10, -20,
];
/// King safety in the middlegame: stay behind the pawn shield, away from
/// the center. Tapered against `KING_PST_EG` below by `game_phase`, so this
/// table's influence fades out as material comes off the board.
#[rustfmt::skip]
const KING_PST_MG: [i32; 64] = [
     20,  30,  10,   0,   0,  10,  30,  20,
     20,  20,   0,   0,   0,   0,  20,  20,
    -10, -20, -20, -20, -20, -20, -20, -10,
    -20, -30, -30, -40, -40, -30, -30, -20,
    -30, -40, -40, -50, -50, -40, -40, -30,
    -30, -40, -40, -50, -50, -40, -40, -30,
    -30, -40, -40, -50, -50, -40, -40, -30,
    -30, -40, -40, -50, -50, -40, -40, -30,
];
/// King activity in the endgame: with queens and most other pieces gone,
/// mating danger drops and the king becomes a strong piece that wants to
/// walk toward the center, not hide in the corner.
#[rustfmt::skip]
const KING_PST_EG: [i32; 64] = [
    -50, -30, -30, -30, -30, -30, -30, -50,
    -30, -30,   0,   0,   0,   0, -30, -30,
    -30, -10,  20,  30,  30,  20, -10, -30,
    -30, -10,  30,  40,  40,  30, -10, -30,
    -30, -10,  30,  40,  40,  30, -10, -30,
    -30, -10,  20,  30,  30,  20, -10, -30,
    -30, -20, -10,   0,   0, -10, -20, -30,
    -50, -40, -30, -20, -20, -30, -40, -50,
];

fn pst_table(kind: PieceType) -> &'static [i32; 64] {
    match kind {
        PieceType::Pawn => &PAWN_PST,
        PieceType::Knight => &KNIGHT_PST,
        PieceType::Bishop => &BISHOP_PST,
        PieceType::Rook => &ROOK_PST,
        PieceType::Queen => &QUEEN_PST,
        PieceType::King => &KING_PST_MG,
    }
}

fn table_value(table: &[i32; 64], color: Color, sq: Square) -> i32 {
    let index = match color {
        Color::White => sq.0 as usize,
        Color::Black => (sq.0 ^ 56) as usize,
    };
    table[index]
}

fn pst_value(kind: PieceType, color: Color, sq: Square) -> i32 {
    table_value(pst_table(kind), color, sq)
}

/// Non-pawn, non-king material weight used to interpolate between the
/// middlegame and endgame king tables: 24 at full material (both sides'
/// starting complement of knights/bishops/rooks/queens), down to 0 once
/// only kings and pawns remain.
const MAX_GAME_PHASE: i32 = 24;
const PHASE_WEIGHTS: [i32; 6] = [0, 1, 1, 2, 4, 0]; // Pawn, Knight, Bishop, Rook, Queen, King

fn game_phase(board: &Board) -> i32 {
    let mut phase = 0;
    for kind in PieceType::ALL {
        let weight = PHASE_WEIGHTS[kind as usize];
        if weight == 0 {
            continue;
        }
        let count = board.pieces_of(Color::White, kind).count() + board.pieces_of(Color::Black, kind).count();
        phase += weight * count as i32;
    }
    phase.min(MAX_GAME_PHASE)
}

fn tapered_king_score(board: &Board, color: Color, phase: i32) -> i32 {
    let sq = match board.pieces_of(color, PieceType::King).lsb() {
        Some(sq) => sq,
        None => return 0, // only reachable mid-test with a kingless FEN
    };
    let mg = table_value(&KING_PST_MG, color, sq);
    let eg = table_value(&KING_PST_EG, color, sq);
    (mg * phase + eg * (MAX_GAME_PHASE - phase)) / MAX_GAME_PHASE
}

/// Static evaluation of `board`, in centipawns from White's perspective:
/// positive means White is better, negative means Black is better.
pub fn evaluate(board: &Board) -> i32 {
    material_score(board)
        + piece_square_score(board)
        + mobility_score(board)
        + pawn_structure_score(board)
        + bishop_pair_score(board)
        + king_safety_score(board)
        + mop_up_score(board)
        + rook_file_score(board)
        + knight_outpost_score(board)
}

/// Only kicks in once the position is both clearly winning for one side
/// and fairly simplified (few pieces left besides pawns): the classic
/// "basic mate" technique of pushing the lone/overwhelmed king to the
/// edge while walking your own king in to help, expressed as an eval bonus
/// rather than dedicated tablebase code. Below either threshold the search
/// already handles the position fine on ordinary material/positional
/// terms, and this would just add noise.
const MOPUP_MAX_PHASE: i32 = 12;
const MOPUP_MATERIAL_THRESHOLD: i32 = 400;

fn distance_from_center(sq: Square) -> i32 {
    let file = sq.file() as i32;
    let rank = sq.rank() as i32;
    let file_dist = (file - 3).abs().min((file - 4).abs());
    let rank_dist = (rank - 3).abs().min((rank - 4).abs());
    file_dist + rank_dist
}

fn chebyshev_distance(a: Square, b: Square) -> i32 {
    let file_dist = (a.file() as i32 - b.file() as i32).abs();
    let rank_dist = (a.rank() as i32 - b.rank() as i32).abs();
    file_dist.max(rank_dist)
}

pub fn mop_up_score(board: &Board) -> i32 {
    if game_phase(board) > MOPUP_MAX_PHASE {
        return 0;
    }
    let material = material_score(board);
    if material.abs() < MOPUP_MATERIAL_THRESHOLD {
        return 0;
    }

    let winning_color = if material > 0 { Color::White } else { Color::Black };
    let losing_color = winning_color.opposite();
    let (winning_king, losing_king) = match (
        board.pieces_of(winning_color, PieceType::King).lsb(),
        board.pieces_of(losing_color, PieceType::King).lsb(),
    ) {
        (Some(w), Some(l)) => (w, l),
        _ => return 0, // only reachable mid-test with a kingless FEN
    };

    let push_to_edge = distance_from_center(losing_king) * 10;
    let escort = (7 - chebyshev_distance(winning_king, losing_king)) * 4;
    let bonus = push_to_edge + escort;

    if winning_color == Color::White {
        bonus
    } else {
        -bonus
    }
}

/// Penalty for each file among the king's own file and its two
/// neighbors that has no pawn of the king's color on it: a fully open
/// file (no pawns of either color) is the most dangerous since a rook or
/// queen can walk straight down it, a semi-open one (only enemy pawns)
/// still lets enemy rooks pressure it without their own pawn in the way.
const KING_OPEN_FILE_PENALTY: i32 = 25;
const KING_SEMI_OPEN_FILE_PENALTY: i32 = 15;

/// How exposed `color`'s king is (always >= 0; higher means more
/// dangerous), tapered by `phase` since an open file next to the king
/// mostly matters while queens/rooks are still around to exploit it.
fn king_safety_penalty(board: &Board, color: Color, phase: i32) -> i32 {
    let king_sq = match board.pieces_of(color, PieceType::King).lsb() {
        Some(sq) => sq,
        None => return 0, // only reachable mid-test with a kingless FEN
    };
    let own_pawns = board.pieces_of(color, PieceType::Pawn);
    let enemy_pawns = board.pieces_of(color.opposite(), PieceType::Pawn);
    let king_file = king_sq.file() as i32;

    let mut penalty = 0;
    for file in (king_file - 1)..=(king_file + 1) {
        if !(0..8).contains(&file) {
            continue;
        }
        let own_on_file = own_pawns.into_iter().any(|sq| sq.file() as i32 == file);
        if own_on_file {
            continue;
        }
        let enemy_on_file = enemy_pawns.into_iter().any(|sq| sq.file() as i32 == file);
        penalty += if enemy_on_file { KING_SEMI_OPEN_FILE_PENALTY } else { KING_OPEN_FILE_PENALTY };
    }
    (penalty * phase) / MAX_GAME_PHASE
}

pub fn king_safety_score(board: &Board) -> i32 {
    let phase = game_phase(board);
    king_safety_penalty(board, Color::Black, phase) - king_safety_penalty(board, Color::White, phase)
}

/// Two bishops covering both square colors between them are worth more
/// than the sum of two lone bishops (between them they can never be
/// blocked out of a whole color complex), so this is scored on top of the
/// bishops' own piece-square value rather than folded into it.
const BISHOP_PAIR_BONUS: i32 = 30;

pub fn bishop_pair_score(board: &Board) -> i32 {
    let mut score = 0;
    if board.pieces_of(Color::White, PieceType::Bishop).count() >= 2 {
        score += BISHOP_PAIR_BONUS;
    }
    if board.pieces_of(Color::Black, PieceType::Bishop).count() >= 2 {
        score -= BISHOP_PAIR_BONUS;
    }
    score
}

/// Bonus for a passed pawn (no enemy pawn on its file or an adjacent file
/// can ever stop or capture it), indexed by how many ranks it has already
/// advanced past its own second rank. Grows sharply near promotion.
#[rustfmt::skip]
const PASSED_PAWN_BONUS_BY_ADVANCE: [i32; 8] = [0, 5, 10, 20, 35, 60, 100, 0];
const DOUBLED_PAWN_PENALTY: i32 = 15;
const ISOLATED_PAWN_PENALTY: i32 = 15;

fn is_passed_pawn(sq: Square, color: Color, enemy_pawns: Bitboard) -> bool {
    let file = sq.file() as i32;
    for enemy_sq in enemy_pawns {
        if (enemy_sq.file() as i32 - file).abs() > 1 {
            continue;
        }
        let blocks_or_defends = match color {
            Color::White => enemy_sq.rank() > sq.rank(),
            Color::Black => enemy_sq.rank() < sq.rank(),
        };
        if blocks_or_defends {
            return false;
        }
    }
    true
}

/// Doubled/isolated penalties plus passed-pawn bonuses for `pawns`, from
/// `color`'s own perspective (always non-negative-biased upward, i.e. a
/// good structure scores higher regardless of which color is being asked
/// about; the caller subtracts Black's from White's).
fn pawn_structure_score_for(pawns: Bitboard, enemy_pawns: Bitboard, color: Color) -> i32 {
    let mut file_counts = [0i32; 8];
    for sq in pawns {
        file_counts[sq.file() as usize] += 1;
    }

    let mut score = 0;
    for sq in pawns {
        let file = sq.file() as usize;
        if file_counts[file] > 1 {
            score -= DOUBLED_PAWN_PENALTY;
        }
        let has_neighbor_file =
            (file > 0 && file_counts[file - 1] > 0) || (file < 7 && file_counts[file + 1] > 0);
        if !has_neighbor_file {
            score -= ISOLATED_PAWN_PENALTY;
        }
        if is_passed_pawn(sq, color, enemy_pawns) {
            let advance = match color {
                Color::White => sq.rank(),
                Color::Black => 7 - sq.rank(),
            };
            score += PASSED_PAWN_BONUS_BY_ADVANCE[advance as usize];
        }
    }
    score
}

pub fn pawn_structure_score(board: &Board) -> i32 {
    let white_pawns = board.pieces_of(Color::White, PieceType::Pawn);
    let black_pawns = board.pieces_of(Color::Black, PieceType::Pawn);
    pawn_structure_score_for(white_pawns, black_pawns, Color::White)
        - pawn_structure_score_for(black_pawns, white_pawns, Color::Black)
}

/// Sum of piece-square bonuses, White pieces minus Black pieces. The king
/// is scored separately from the other piece types since its table is
/// tapered by game phase instead of being a single fixed table.
pub fn piece_square_score(board: &Board) -> i32 {
    let mut score = 0;
    for kind in PieceType::ALL {
        if kind == PieceType::King {
            continue;
        }
        for sq in board.pieces_of(Color::White, kind) {
            score += pst_value(kind, Color::White, sq);
        }
        for sq in board.pieces_of(Color::Black, kind) {
            score -= pst_value(kind, Color::Black, sq);
        }
    }
    let phase = game_phase(board);
    score += tapered_king_score(board, Color::White, phase) - tapered_king_score(board, Color::Black, phase);
    score
}

/// Same evaluation, but from the perspective of the side to move: positive
/// always means "good for whoever moves next". This is what negamax search
/// needs at its leaves.
pub fn evaluate_relative(board: &Board) -> i32 {
    let score = evaluate(board);
    if board.side_to_move == Color::White {
        score
    } else {
        -score
    }
}

pub fn piece_value(kind: PieceType) -> i32 {
    PIECE_VALUES[kind as usize]
}

pub fn material_score(board: &Board) -> i32 {
    let mut score = 0;
    for kind in PieceType::ALL {
        let value = PIECE_VALUES[kind as usize];
        let white_count = board.pieces_of(Color::White, kind).count() as i32;
        let black_count = board.pieces_of(Color::Black, kind).count() as i32;
        score += value * (white_count - black_count);
    }
    score
}

/// Files a color's pawns attack, computed directly from the pawn bitboard
/// with shifts rather than a per-square table lookup (standard
/// chess-programming-wiki formula): a pawn on file A/H can't attack
/// further off the left/right edge, hence the file masks.
const NOT_FILE_A: u64 = 0xFEFE_FEFE_FEFE_FEFE;
const NOT_FILE_H: u64 = 0x7F7F_7F7F_7F7F_7F7F;

fn pawn_attack_set(pawns: Bitboard, color: Color) -> Bitboard {
    let p = pawns.0;
    let bits = match color {
        Color::White => ((p & NOT_FILE_A) << 7) | ((p & NOT_FILE_H) << 9),
        Color::Black => ((p & NOT_FILE_H) >> 7) | ((p & NOT_FILE_A) >> 9),
    };
    Bitboard(bits)
}

/// Centipawns per reachable "safe" square (not attacked by an enemy pawn,
/// not occupied by one of the piece's own side), indexed like
/// `PieceType`'s discriminant. Pawns and kings aren't counted: pawn
/// "mobility" isn't meaningful in this sense, and king mobility is mostly
/// noise. Queen mobility is weighted low on purpose — queens start with a
/// huge raw move count almost everywhere, so weighting it like a rook
/// would make the eval overvalue early queen excursions.
const MOBILITY_WEIGHTS: [i32; 6] = [0, 4, 5, 2, 1, 0];

/// Per-piece-type mobility, counting only moves to squares an enemy pawn
/// doesn't attack — the standard refinement over a flat legal/pseudo-legal
/// move count (which treats a knight hop into a square that's instantly
/// recapturable by a pawn the same as a hop to a genuinely useful one).
/// Also considerably cheaper than generating actual move lists: this only
/// ever looks up attack bitboards and counts bits, no `Move` objects, no
/// pawn/king/castling generation at all.
pub fn mobility_score(board: &Board) -> i32 {
    let occ = board.occupied();
    let white_pawn_attacks = pawn_attack_set(board.pieces_of(Color::White, PieceType::Pawn), Color::White);
    let black_pawn_attacks = pawn_attack_set(board.pieces_of(Color::Black, PieceType::Pawn), Color::Black);

    piece_mobility_for(board, Color::White, occ, black_pawn_attacks)
        - piece_mobility_for(board, Color::Black, occ, white_pawn_attacks)
}

fn piece_mobility_for(board: &Board, color: Color, occ: Bitboard, enemy_pawn_attacks: Bitboard) -> i32 {
    let own = board.color_occupied(color);
    let safe_squares = !enemy_pawn_attacks & !own;

    let mut score = 0;
    for sq in board.pieces_of(color, PieceType::Knight) {
        score += MOBILITY_WEIGHTS[PieceType::Knight as usize] * (movegen::knight_attacks(sq) & safe_squares).count() as i32;
    }
    for sq in board.pieces_of(color, PieceType::Bishop) {
        score += MOBILITY_WEIGHTS[PieceType::Bishop as usize] * (movegen::bishop_attacks(sq, occ) & safe_squares).count() as i32;
    }
    for sq in board.pieces_of(color, PieceType::Rook) {
        score += MOBILITY_WEIGHTS[PieceType::Rook as usize] * (movegen::rook_attacks(sq, occ) & safe_squares).count() as i32;
    }
    for sq in board.pieces_of(color, PieceType::Queen) {
        score += MOBILITY_WEIGHTS[PieceType::Queen as usize] * (movegen::queen_attacks(sq, occ) & safe_squares).count() as i32;
    }
    score
}

/// Bonus for a rook on a file with no pawn of its own color on it: an open
/// file (no pawns of either color) lets it contest the file from turn
/// one, a semi-open one (only enemy pawns) still gives it a target and a
/// path in, just with a bit more resistance.
const ROOK_OPEN_FILE_BONUS: i32 = 20;
const ROOK_SEMI_OPEN_FILE_BONUS: i32 = 10;

fn rook_file_score_for(board: &Board, color: Color) -> i32 {
    let own_pawns = board.pieces_of(color, PieceType::Pawn);
    let enemy_pawns = board.pieces_of(color.opposite(), PieceType::Pawn);
    let mut score = 0;
    for sq in board.pieces_of(color, PieceType::Rook) {
        let file = sq.file();
        if own_pawns.into_iter().any(|s| s.file() == file) {
            continue;
        }
        let enemy_on_file = enemy_pawns.into_iter().any(|s| s.file() == file);
        score += if enemy_on_file { ROOK_SEMI_OPEN_FILE_BONUS } else { ROOK_OPEN_FILE_BONUS };
    }
    score
}

pub fn rook_file_score(board: &Board) -> i32 {
    rook_file_score_for(board, Color::White) - rook_file_score_for(board, Color::Black)
}

/// A knight that's both defended by one of its own pawns and can never be
/// challenged by an enemy pawn (none on an adjacent file that hasn't
/// already passed it) is a classic "outpost": hard to dislodge and often
/// worth more than its piece-square value alone suggests. Restricted to
/// advanced squares (the knight's own 4th/5th/6th rank) since a "safe"
/// knight sitting at home is not what this term is meant to reward.
const KNIGHT_OUTPOST_BONUS: i32 = 20;

fn is_defended_by_pawn(sq: Square, color: Color, own_pawns: Bitboard) -> bool {
    let (file, rank) = (sq.file() as i32, sq.rank() as i32);
    let defender_rank = match color {
        Color::White => rank - 1,
        Color::Black => rank + 1,
    };
    if !(0..8).contains(&defender_rank) {
        return false;
    }
    [file - 1, file + 1]
        .into_iter()
        .filter(|&f| (0..8).contains(&f))
        .any(|f| own_pawns.contains(Square::new(f as u8, defender_rank as u8)))
}

fn is_outpost_square(sq: Square, color: Color, enemy_pawns: Bitboard) -> bool {
    let file = sq.file() as i32;
    !enemy_pawns.into_iter().any(|enemy_sq| {
        if (enemy_sq.file() as i32 - file).abs() > 1 {
            return false;
        }
        match color {
            Color::White => enemy_sq.rank() > sq.rank(),
            Color::Black => enemy_sq.rank() < sq.rank(),
        }
    })
}

fn knight_outpost_score_for(board: &Board, color: Color) -> i32 {
    let own_pawns = board.pieces_of(color, PieceType::Pawn);
    let enemy_pawns = board.pieces_of(color.opposite(), PieceType::Pawn);
    let (advanced_lo, advanced_hi) = match color {
        Color::White => (3, 5),
        Color::Black => (2, 4),
    };
    let mut score = 0;
    for sq in board.pieces_of(color, PieceType::Knight) {
        if !(advanced_lo..=advanced_hi).contains(&sq.rank()) {
            continue;
        }
        if is_defended_by_pawn(sq, color, own_pawns) && is_outpost_square(sq, color, enemy_pawns) {
            score += KNIGHT_OUTPOST_BONUS;
        }
    }
    score
}

pub fn knight_outpost_score(board: &Board) -> i32 {
    knight_outpost_score_for(board, Color::White) - knight_outpost_score_for(board, Color::Black)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn startpos_is_exactly_balanced() {
        let board = Board::start_pos();
        assert_eq!(material_score(&board), 0);
        assert_eq!(piece_square_score(&board), 0);
        assert_eq!(mobility_score(&board), 0);
        assert_eq!(evaluate(&board), 0);
    }

    #[test]
    fn pst_favors_a_centralized_knight_over_a_cornered_one() {
        let centralized = Board::from_fen("4k3/8/8/3N4/8/8/8/4K3 w - - 0 1").unwrap();
        let cornered = Board::from_fen("4k3/8/8/8/8/8/8/N3K3 w - - 0 1").unwrap();
        assert!(piece_square_score(&centralized) > piece_square_score(&cornered));
    }

    #[test]
    fn game_phase_is_max_at_full_material_and_zero_with_only_kings_and_pawns() {
        assert_eq!(game_phase(&Board::start_pos()), MAX_GAME_PHASE);
        let kp_only = Board::from_fen("4k3/8/8/8/8/8/8/4K3 w - - 0 1").unwrap();
        assert_eq!(game_phase(&kp_only), 0);
    }

    #[test]
    fn tapered_king_score_prefers_the_corner_at_full_phase_and_the_center_at_zero_phase() {
        let king_on_e4 = Board::from_fen("4k3/8/8/8/4K3/8/8/8 w - - 0 1").unwrap();
        let middlegame_like = tapered_king_score(&king_on_e4, Color::White, MAX_GAME_PHASE);
        let endgame_like = tapered_king_score(&king_on_e4, Color::White, 0);
        assert!(
            endgame_like > middlegame_like,
            "a centralized king should score better as the endgame table takes over"
        );
    }

    #[test]
    fn pst_mirrors_correctly_between_colors() {
        // A White knight on d5 should score exactly like a Black knight on
        // d4, since d4/d5 are mirror images across the board's center.
        let white_knight = Board::from_fen("4k3/8/8/3N4/8/8/8/4K3 w - - 0 1").unwrap();
        let black_knight = Board::from_fen("4k3/8/8/8/3n4/8/8/4K3 b - - 0 1").unwrap();
        assert_eq!(piece_square_score(&white_knight), -piece_square_score(&black_knight));
    }

    #[test]
    fn white_up_a_queen_scores_around_a_queen() {
        let board = Board::from_fen("4k3/8/8/8/8/8/8/4KQ2 w - - 0 1").unwrap();
        assert_eq!(material_score(&board), 900);
        // Mobility swings the total further in White's favor, never against it.
        assert!(evaluate(&board) >= 900);
    }

    #[test]
    fn black_up_a_rook_scores_negative() {
        let board = Board::from_fen("4kr2/8/8/8/8/8/8/4K3 b - - 0 1").unwrap();
        assert_eq!(material_score(&board), -500);
        assert!(evaluate(&board) <= -500);
    }

    #[test]
    fn evaluate_relative_flips_sign_for_black_to_move() {
        let board = Board::from_fen("4k3/8/8/8/8/8/8/4KQ2 w - - 0 1").unwrap();
        let mirrored = Board::from_fen("4kq2/8/8/8/8/8/8/4K3 b - - 0 1").unwrap();
        assert_eq!(evaluate(&board), -evaluate(&mirrored));
        assert!(evaluate_relative(&board) > 0);
        assert!(evaluate_relative(&mirrored) > 0);
    }

    #[test]
    fn side_with_more_room_to_move_scores_higher() {
        // White queen centralized on an open board vs. a black queen boxed
        // into the corner by two of its own pawns.
        let cramped = Board::from_fen("q3k3/pp6/8/8/3Q4/8/8/4K3 w - - 0 1").unwrap();
        assert!(mobility_score(&cramped) > 0);
    }

    #[test]
    fn bishop_pair_beats_two_lone_bishops_of_the_same_kind() {
        let pair = Board::from_fen("4k3/8/8/8/8/8/8/2B1KB2 w - - 0 1").unwrap();
        assert_eq!(bishop_pair_score(&pair), BISHOP_PAIR_BONUS);
        let one_bishop = Board::from_fen("4k3/8/8/8/8/8/8/3BK3 w - - 0 1").unwrap();
        assert_eq!(bishop_pair_score(&one_bishop), 0);
    }

    #[test]
    fn passed_pawn_outranks_a_pawn_blocked_by_an_enemy_pawn_ahead() {
        let passed = Board::from_fen("4k3/8/8/4P3/8/8/8/4K3 w - - 0 1").unwrap();
        let blocked = Board::from_fen("4k3/4p3/8/4P3/8/8/8/4K3 w - - 0 1").unwrap();
        assert!(pawn_structure_score(&passed) > pawn_structure_score(&blocked));
    }

    #[test]
    fn doubled_pawns_score_worse_than_the_same_pawns_spread_out() {
        let doubled = Board::from_fen("4k3/8/8/8/4P3/8/4P3/4K3 w - - 0 1").unwrap();
        let spread = Board::from_fen("4k3/8/8/8/4P3/8/3P4/4K3 w - - 0 1").unwrap();
        assert!(pawn_structure_score(&doubled) < pawn_structure_score(&spread));
    }

    #[test]
    fn isolated_pawn_scores_worse_than_one_with_a_neighbor() {
        let isolated = Board::from_fen("4k3/8/8/8/8/8/2P1P3/4K3 w - - 0 1").unwrap();
        let supported = Board::from_fen("4k3/8/8/8/8/8/2PPP3/4K3 w - - 0 1").unwrap();
        assert!(pawn_structure_score(&isolated) < pawn_structure_score(&supported));
    }

    #[test]
    fn king_behind_an_open_file_is_penalized_with_material_still_on_the_board() {
        // Same king position (g1) and same total pawns, but White's is
        // missing the g-file pawn (open file right next to the castled
        // king) in one FEN and missing an unrelated a-file pawn in the
        // other. Both have full material elsewhere, so the game phase is
        // the same and the difference is purely the open file next to
        // the king.
        let open_file_by_king = Board::from_fen(
            "rnbqkbnr/pppppppp/8/8/8/8/PPPPPP1P/RNBQ1RK1 w kq - 0 1",
        )
        .unwrap();
        let open_file_elsewhere = Board::from_fen(
            "rnbqkbnr/pppppppp/8/8/8/8/1PPPPPPP/RNBQ1RK1 w kq - 0 1",
        )
        .unwrap();
        assert!(king_safety_score(&open_file_by_king) < king_safety_score(&open_file_elsewhere));
    }

    #[test]
    fn king_safety_penalty_fades_out_at_zero_game_phase() {
        let king_sq = Square::new(6, 0); // g1
        let board = Board::from_fen("4k3/8/8/8/8/8/8/6K1 w - - 0 1").unwrap();
        assert_eq!(board.pieces_of(Color::White, PieceType::King).lsb(), Some(king_sq));
        assert_eq!(king_safety_penalty(&board, Color::White, 0), 0);
    }

    #[test]
    fn mop_up_prefers_the_losing_king_cornered_over_centralized() {
        // White up a whole queen (KQK), Black king either boxed into a
        // corner or standing in the center: the corner should score
        // higher for White's mop-up bonus.
        let cornered = Board::from_fen("7k/8/8/8/8/8/8/3QK3 w - - 0 1").unwrap();
        let centralized = Board::from_fen("4k3/8/8/8/8/8/8/3QK3 w - - 0 1").unwrap();
        assert!(mop_up_score(&cornered) > mop_up_score(&centralized));
    }

    #[test]
    fn mop_up_prefers_the_escorting_king_closer() {
        let kings_close = Board::from_fen("7k/8/8/8/8/6K1/8/3Q4 w - - 0 1").unwrap();
        let kings_far = Board::from_fen("7k/8/8/8/8/8/8/3QK3 w - - 0 1").unwrap();
        assert!(mop_up_score(&kings_close) > mop_up_score(&kings_far));
    }

    #[test]
    fn mop_up_is_silent_with_full_material_even_up_a_queen() {
        let board =
            Board::from_fen("rnb1kbnr/pppppppp/8/8/8/8/PPPPPPPP/RNBQKBNR w KQkq - 0 1").unwrap();
        // Black is missing its queen: material.abs() clears the threshold,
        // but the position is nowhere near simplified enough (full phase).
        assert_eq!(mop_up_score(&board), 0);
    }

    #[test]
    fn safe_mobility_excludes_squares_attacked_by_enemy_pawns() {
        // A White knight on d4 reaches b5 among its 8 squares in an
        // otherwise empty board; with a Black pawn added on c6 (which
        // attacks b5 and d5), that square stops counting as safe.
        let no_pawns = Board::from_fen("4k3/8/8/8/3N4/8/8/4K3 w - - 0 1").unwrap();
        let with_attacker = Board::from_fen("4k3/8/2p5/8/3N4/8/8/4K3 w - - 0 1").unwrap();
        assert!(mobility_score(&no_pawns) > mobility_score(&with_attacker));
    }

    #[test]
    fn rook_on_open_file_beats_one_blocked_by_its_own_pawn() {
        let open = Board::from_fen("4k3/8/8/8/8/8/8/R3K3 w - - 0 1").unwrap();
        let blocked = Board::from_fen("4k3/8/8/8/8/8/P7/R3K3 w - - 0 1").unwrap();
        assert!(rook_file_score(&open) > rook_file_score(&blocked));
    }

    #[test]
    fn rook_on_fully_open_file_beats_one_on_a_semi_open_file() {
        let fully_open = Board::from_fen("4k3/8/8/8/8/8/8/R3K3 w - - 0 1").unwrap();
        let semi_open = Board::from_fen("p3k3/8/8/8/8/8/8/R3K3 w - - 0 1").unwrap();
        assert!(rook_file_score(&fully_open) > rook_file_score(&semi_open));
        assert!(rook_file_score(&semi_open) > 0);
    }

    #[test]
    fn knight_outpost_beats_a_knight_a_pawn_could_still_challenge() {
        // White knight e5, defended by a pawn on d4: an outpost as long as
        // no Black pawn on the d/e/f files can ever reach a square that
        // attacks e5. Adding a Black pawn on f6 (still able to advance and
        // capture on e5) removes the bonus.
        let outpost = Board::from_fen("4k3/8/8/4N3/3P4/8/8/4K3 w - - 0 1").unwrap();
        let challenged = Board::from_fen("4k3/8/5p2/4N3/3P4/8/8/4K3 w - - 0 1").unwrap();
        assert!(knight_outpost_score(&outpost) > knight_outpost_score(&challenged));
        assert_eq!(knight_outpost_score(&challenged), 0);
    }
}
