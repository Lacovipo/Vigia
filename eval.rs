use crate::bitboard::Bitboard;
use crate::board::Board;
use crate::kpk;
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
/// positive means White is better, negative means Black is better. A pure
/// king-and-one-pawn ending is answered exactly (see `kpk_exact_score`)
/// instead of through the generic heuristics below, since that specific
/// case is a fully solved endgame, not a judgment call.
pub fn evaluate(board: &Board) -> i32 {
    // Computed once and threaded through, rather than letting each term
    // below recompute it independently (it used to be recomputed 5 times
    // per call: here, plus once each inside mop_up/king_safety/piece_square
    // /pawn_endgame's own scoring functions).
    let phase = game_phase(board);
    if phase == 0 {
        let total_pawns =
            board.pieces_of(Color::White, PieceType::Pawn).count() + board.pieces_of(Color::Black, PieceType::Pawn).count();
        if total_pawns == 1 {
            return kpk_exact_score(board);
        }
    }
    // Shared by king_safety_score/threats_score below, computed once: both
    // need "which squares does each color's pawns/minors/rooks/queens
    // attack", just aggregated differently (see `AttackInfo`'s own docs).
    let occ = board.occupied();
    let white_attacks = attack_info_for(board, Color::White, occ);
    let black_attacks = attack_info_for(board, Color::Black, occ);

    let raw = material_score(board)
        + piece_square_score_with_phase(board, phase)
        + mobility_score(board)
        + pawn_structure_score_with_phase(board, phase)
        + bishop_pair_score(board)
        + bishop_pawns_score(board)
        + king_safety_score_with(board, phase, &white_attacks, &black_attacks)
        + mop_up_score_with_phase(board, phase)
        + rook_file_score(board)
        + rook_on_seventh_score(board)
        + knight_outpost_score(board)
        + pawn_endgame_score(board, phase)
        + tempo_score(board)
        + threats_score_with_attacks(board, &white_attacks, &black_attacks);
    (raw * endgame_scale_factor(board)) / SCALE_FACTOR_NORMAL
}

/// Small, non-tapered bonus for whoever's turn it is: having the move is
/// worth something in almost any position. Every reference engine's HCE
/// checked for this eval pass includes some form of it (Crafty and Ethereal
/// have it explicit; Fruit's is implicit in its endgame-specific code, but
/// the concept still applies generally).
const TEMPO_BONUS: i32 = 12;

fn tempo_score(board: &Board) -> i32 {
    if board.side_to_move == Color::White {
        TEMPO_BONUS
    } else {
        -TEMPO_BONUS
    }
}

/// Decisive bonus for a King+Pawn vs King ending that `kpk::probe` has
/// proven won, comfortably below `search::MATE_SCORE`'s
/// mate-distance-pruning threshold (29000) so it's never mistaken for a
/// forced-mate score, but far above any ordinary positional or material
/// swing so the search always prefers forcing a trade down into a position
/// this certain over any merely-promising alternative.
const KPK_DECISIVE_BONUS: i32 = 2000;

/// Exact evaluation of a position with exactly one pawn and otherwise bare
/// kings, via `kpk::probe`. A proven draw scores a flat `0` — deliberately:
/// this is the direct answer to the classic "an extra pawn in the ending is
/// just won" rule of thumb, which is false often enough (wrong rook pawn,
/// king too far away...) that pretending otherwise would be worse than
/// useless here. A proven win gets `KPK_DECISIVE_BONUS` plus a small,
/// bounded shaping term (pawn advancement and king proximity) so the search
/// still prefers the more efficient winning technique among several
/// choices, without ever reading as anything less than certain.
fn kpk_exact_score(board: &Board) -> i32 {
    let (pawn_color, pawn_sq) = if let Some(sq) = board.pieces_of(Color::White, PieceType::Pawn).lsb() {
        (Color::White, sq)
    } else if let Some(sq) = board.pieces_of(Color::Black, PieceType::Pawn).lsb() {
        (Color::Black, sq)
    } else {
        return 0; // only reachable mid-test with a pawnless FEN
    };
    let strong_king = board.pieces_of(pawn_color, PieceType::King).lsb();
    let weak_king = board.pieces_of(pawn_color.opposite(), PieceType::King).lsb();
    let (strong_king, weak_king) = match (strong_king, weak_king) {
        (Some(s), Some(w)) => (s, w),
        _ => return 0, // only reachable mid-test with a kingless FEN
    };

    let outcome = kpk::probe(pawn_color, strong_king, weak_king, pawn_sq, board.side_to_move);
    let sign = if pawn_color == Color::White { 1 } else { -1 };
    match outcome {
        kpk::Outcome::Draw => 0,
        kpk::Outcome::Win => {
            let advance = match pawn_color {
                Color::White => pawn_sq.rank() as i32,
                Color::Black => 7 - pawn_sq.rank() as i32,
            };
            let shaping = advance * 8 + (7 - chebyshev_distance(strong_king, pawn_sq)) * 4;
            sign * (KPK_DECISIVE_BONUS + shaping)
        }
    }
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
    mop_up_score_with_phase(board, game_phase(board))
}

fn mop_up_score_with_phase(board: &Board, phase: i32) -> i32 {
    if phase > MOPUP_MAX_PHASE {
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

/// Ranks between the king and the nearest pawn in `pawns` on `file` that
/// still lies ahead of the king from `king_color`'s point of view (i.e.
/// could plausibly shield it, or is still advancing toward it) — `None` if
/// there's no such pawn on that file at all. Shared by the shelter and
/// storm terms below: shelter passes the king's own pawns, storm passes
/// the enemy's; only the direction (`king_color`) matters, not whose
/// pawns they are.
fn nearest_pawn_distance_ahead(pawns: Bitboard, file: i32, king_rank: i32, king_color: Color) -> Option<i32> {
    pawns
        .into_iter()
        .filter(|sq| sq.file() as i32 == file)
        .filter_map(|sq| {
            let d = match king_color {
                Color::White => sq.rank() as i32 - king_rank,
                Color::Black => king_rank - sq.rank() as i32,
            };
            (d > 0).then_some(d)
        })
        .min()
}

/// Missing entirely, or too far away to help, is worse than a real
/// distance ever gets scored as (`(5*5*4).min(36) == 36`, same as the cap):
/// this sentinel just lets "no shelter pawn on this file at all" fall
/// straight through the same quadratic formula as a real distance would.
const NO_SHELTER_PAWN_DISTANCE: i32 = 5;
const SHELTER_PENALTY_FACTOR: i32 = 4;
const SHELTER_PENALTY_CAP: i32 = 36;

/// Penalty for how far the king's own nearest pawn on `file` is (bigger,
/// and non-linearly so, the further away or entirely missing): a pawn
/// still on its start square shelters the king almost perfectly, one
/// that's already advanced (or was traded off) leaves real gaps for an
/// enemy piece to walk into.
fn shelter_penalty(own_pawns: Bitboard, file: i32, king_rank: i32, color: Color) -> i32 {
    let distance = nearest_pawn_distance_ahead(own_pawns, file, king_rank, color).unwrap_or(NO_SHELTER_PAWN_DISTANCE);
    (distance * distance * SHELTER_PENALTY_FACTOR).min(SHELTER_PENALTY_CAP)
}

/// Penalty for an enemy pawn advancing toward the king on `file`, indexed
/// by how many ranks away it still is: a pawn already knocking on the
/// king's door is far more dangerous than one that's merely left its own
/// side of the board, and one that hasn't advanced at all yet (or doesn't
/// exist) isn't a storm at all.
#[rustfmt::skip]
const STORM_PENALTY_BY_DISTANCE: [i32; 8] = [40, 40, 24, 12, 4, 0, 0, 0];

fn storm_penalty(enemy_pawns: Bitboard, file: i32, king_rank: i32, color: Color) -> i32 {
    match nearest_pawn_distance_ahead(enemy_pawns, file, king_rank, color) {
        Some(d) => STORM_PENALTY_BY_DISTANCE[(d as usize).min(STORM_PENALTY_BY_DISTANCE.len() - 1)],
        None => 0,
    }
}

/// The king's own square plus its 8 neighbors: the immediate zone an
/// attacking piece needs to bear on to actually threaten the king, as
/// opposed to merely being active somewhere else on the board.
fn king_ring(king_sq: Square) -> Bitboard {
    let file = king_sq.file() as i32;
    let rank = king_sq.rank() as i32;
    let mut ring = Bitboard::EMPTY;
    for df in -1..=1 {
        for dr in -1..=1 {
            let f = file + df;
            let r = rank + dr;
            if (0..8).contains(&f) && (0..8).contains(&r) {
                ring.set(Square::new(f as u8, r as u8));
            }
        }
    }
    ring
}

/// Weight per attacking piece type for the king-ring danger count below,
/// matching the classic (Fruit-style) split: minor pieces count once,
/// rooks twice, queens four times — a queen bearing on the king ring is
/// far more ominous than a knight doing the same.
const KING_ATTACK_WEIGHT_MINOR: i32 = 1;
const KING_ATTACK_WEIGHT_ROOK: i32 = 2;
const KING_ATTACK_WEIGHT_QUEEN: i32 = 4;
/// Squares the king-ring danger penalty (`units² * scale`, capped) is
/// scaled and capped: growing with the *square* of the weighted attacker
/// count, not linearly, is the one piece of consensus every reference
/// engine's HCE checked this session agreed on — a lone attacker near the
/// king is normal, several at once is when a real attack starts, and the
/// danger compounds faster than one-at-a-time addition would suggest.
const KING_DANGER_SCALE: i32 = 2;
const KING_DANGER_CAP: i32 = 150;

fn king_danger_penalty(ring: Bitboard, enemy_attacks: &AttackInfo) -> i32 {
    let units = (enemy_attacks.minors & ring).count() as i32 * KING_ATTACK_WEIGHT_MINOR
        + (enemy_attacks.rooks & ring).count() as i32 * KING_ATTACK_WEIGHT_ROOK
        + (enemy_attacks.queens & ring).count() as i32 * KING_ATTACK_WEIGHT_QUEEN;
    (units * units * KING_DANGER_SCALE).min(KING_DANGER_CAP)
}

/// How exposed `color`'s king is (always >= 0; higher means more
/// dangerous), tapered by `phase` since exposure mostly matters while
/// enough attacking material remains on the board. Three independent
/// signals added together, the first two keyed to the king's own file and
/// its two neighbors: pawn shelter, enemy pawn storm, and a non-linearly
/// scaled count of enemy pieces (`enemy_attacks`, the same `AttackInfo`
/// `threats_score` uses) bearing on the immediate king ring.
fn king_safety_penalty(board: &Board, color: Color, phase: i32, enemy_attacks: &AttackInfo) -> i32 {
    let king_sq = match board.pieces_of(color, PieceType::King).lsb() {
        Some(sq) => sq,
        None => return 0, // only reachable mid-test with a kingless FEN
    };
    let own_pawns = board.pieces_of(color, PieceType::Pawn);
    let enemy_pawns = board.pieces_of(color.opposite(), PieceType::Pawn);
    let king_file = king_sq.file() as i32;
    let king_rank = king_sq.rank() as i32;

    let mut penalty = 0;
    for file in (king_file - 1)..=(king_file + 1) {
        if !(0..8).contains(&file) {
            continue;
        }
        penalty += shelter_penalty(own_pawns, file, king_rank, color);
        penalty += storm_penalty(enemy_pawns, file, king_rank, color);
    }
    penalty += king_danger_penalty(king_ring(king_sq), enemy_attacks);

    (penalty * phase) / MAX_GAME_PHASE
}

pub fn king_safety_score(board: &Board) -> i32 {
    let occ = board.occupied();
    let white_attacks = attack_info_for(board, Color::White, occ);
    let black_attacks = attack_info_for(board, Color::Black, occ);
    king_safety_score_with(board, game_phase(board), &white_attacks, &black_attacks)
}

fn king_safety_score_with(board: &Board, phase: i32, white_attacks: &AttackInfo, black_attacks: &AttackInfo) -> i32 {
    king_safety_penalty(board, Color::Black, phase, white_attacks) - king_safety_penalty(board, Color::White, phase, black_attacks)
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

fn square_is_light(sq: Square) -> bool {
    (sq.file() + sq.rank()) % 2 == 1
}

/// A "bad bishop": own pawns sitting on the same square color as a lone
/// bishop get in its own diagonals' way. Doubled if the pawn is "rammed"
/// (blocked head-on by an enemy pawn directly in front, so it can't ever
/// move aside on its own) rather than just sitting there. Only applies
/// with exactly one bishop of that color — with the pair, the other
/// bishop already covers the opposite color complex, so this penalty
/// isn't the right lens (that's what `bishop_pair_score` rewards instead).
const BAD_BISHOP_PAWN_PENALTY: i32 = 3;
const BAD_BISHOP_RAMMED_PAWN_PENALTY: i32 = 6;

fn bishop_pawns_score_for(board: &Board, color: Color) -> i32 {
    let bishops = board.pieces_of(color, PieceType::Bishop);
    if bishops.count() != 1 {
        return 0;
    }
    let bishop_is_light = square_is_light(bishops.lsb().expect("count() == 1 above guarantees a bishop"));
    let enemy_pawns = board.pieces_of(color.opposite(), PieceType::Pawn);

    let mut score = 0;
    for sq in board.pieces_of(color, PieceType::Pawn) {
        if square_is_light(sq) != bishop_is_light {
            continue;
        }
        let ahead_rank = match color {
            Color::White => sq.rank() as i32 + 1,
            Color::Black => sq.rank() as i32 - 1,
        };
        let rammed =
            (0..8).contains(&ahead_rank) && enemy_pawns.contains(Square::new(sq.file(), ahead_rank as u8));
        score -= if rammed { BAD_BISHOP_RAMMED_PAWN_PENALTY } else { BAD_BISHOP_PAWN_PENALTY };
    }
    score
}

pub fn bishop_pawns_score(board: &Board) -> i32 {
    bishop_pawns_score_for(board, Color::White) - bishop_pawns_score_for(board, Color::Black)
}

// ---------------------------------------------------------------------
// Endgame scale factor: some material balances are "technically" an
// advantage by raw material/positional count but are known, specific
// patterns that are far more drawish in practice than the plain sum of
// the terms above would suggest. Rather than inventing more additive
// bonuses/penalties to fight that (which would fight the *other* terms
// too, in positions where this pattern doesn't apply), this scales the
// whole evaluation down directly — the same technique Stockfish/Ethereal
// use for exactly this purpose. `kpk_exact_score` is unaffected (it
// returns before this is ever computed): that path is already exact, not
// a judgment call that could be systematically too confident.
// ---------------------------------------------------------------------

/// Denominator for `endgame_scale_factor`: `SCALE_FACTOR_NORMAL` itself
/// means "don't scale at all".
const SCALE_FACTOR_NORMAL: i32 = 64;

/// Classic opposite-colored-bishops ending: each side has exactly one
/// bishop, on opposite square colors, and nothing else but kings and
/// pawns. Notoriously drawish even a pawn or two down, since the
/// defending side's bishop can permanently blockade one color complex
/// that the attacker's bishop can never contest — confirmed empirically
/// against Stockfish/Obsidian/Berserk on a hand-built test position (see
/// `compare_eval.sh` in this session's notes) where Vigia gave +87 and the
/// reference engines gave roughly 0. Deliberately a flat scale regardless
/// of how many extra pawns the "better" side has: real engines accept
/// this same simplification (more pawns genuinely do help convert in
/// practice, but modeling that precisely needs more than a hand-picked
/// constant can responsibly capture).
const OPPOSITE_COLORED_BISHOPS_SCALE: i32 = 12;

fn is_pure_opposite_colored_bishops_ending(board: &Board) -> bool {
    for color in [Color::White, Color::Black] {
        if board.pieces_of(color, PieceType::Knight).count() > 0
            || board.pieces_of(color, PieceType::Rook).count() > 0
            || board.pieces_of(color, PieceType::Queen).count() > 0
            || board.pieces_of(color, PieceType::Bishop).count() != 1
        {
            return false;
        }
    }
    let white_bishop = board
        .pieces_of(Color::White, PieceType::Bishop)
        .lsb()
        .expect("checked count() == 1 above");
    let black_bishop = board
        .pieces_of(Color::Black, PieceType::Bishop)
        .lsb()
        .expect("checked count() == 1 above");
    square_is_light(white_bishop) != square_is_light(black_bishop)
}

/// Mirrors `search::is_insufficient_material`'s conservative definition
/// (no pawns/rooks/queens for either side, at most one minor piece on the
/// whole board): that check already makes the search itself score these
/// positions as an exact draw wherever they actually appear in the tree,
/// so this is a belt-and-suspenders match for `evaluate()`/`cmd_eval`
/// being asked about such a position directly, not a case this needs to
/// (or safely could) handle any more precisely — a second minor piece on
/// the stronger side (two bishops, or bishop and knight) can in fact force
/// mate, so this must not overreach into that territory.
fn is_drawn_by_insufficient_material(board: &Board) -> bool {
    for color in [Color::White, Color::Black] {
        if !board.pieces_of(color, PieceType::Pawn).is_empty()
            || !board.pieces_of(color, PieceType::Rook).is_empty()
            || !board.pieces_of(color, PieceType::Queen).is_empty()
        {
            return false;
        }
    }
    let minors = board.pieces_of(Color::White, PieceType::Knight).count()
        + board.pieces_of(Color::White, PieceType::Bishop).count()
        + board.pieces_of(Color::Black, PieceType::Knight).count()
        + board.pieces_of(Color::Black, PieceType::Bishop).count();
    minors <= 1
}

fn endgame_scale_factor(board: &Board) -> i32 {
    if is_drawn_by_insufficient_material(board) {
        0
    } else if is_pure_opposite_colored_bishops_ending(board) {
        OPPOSITE_COLORED_BISHOPS_SCALE
    } else {
        SCALE_FACTOR_NORMAL
    }
}

/// Bonus for a passed pawn (no enemy pawn on its file or an adjacent file
/// can ever stop or capture it), indexed by how many ranks it has already
/// advanced past its own second rank. Grows sharply near promotion.
#[rustfmt::skip]
const PASSED_PAWN_BONUS_BY_ADVANCE: [i32; 8] = [0, 5, 10, 20, 35, 60, 100, 0];
const DOUBLED_PAWN_PENALTY: i32 = 15;
const ISOLATED_PAWN_PENALTY: i32 = 15;
const BACKWARD_PAWN_PENALTY: i32 = 10;
/// Bonus for a pawn defended by another pawn (diagonally behind) or
/// standing shoulder-to-shoulder with one on the same rank (a "phalanx"):
/// both count as the same "connected" concept here rather than as two
/// separate terms, which several reference engines' own HCE treat as
/// close enough in value to not be worth telling apart. Indexed by advance
/// like the passed-pawn table, but flatter: connectedness matters most as
/// a passed pawn nears promotion, which the passed-pawn bonus above
/// already captures on its own.
#[rustfmt::skip]
const PAWN_CONNECTED_BONUS_BY_ADVANCE: [i32; 8] = [0, 3, 4, 6, 12, 20, 30, 0];
/// Weighting for the two kings' distance to a passed pawn, generalized
/// from the pure-KPK-ending case (`pawn_endgame_score`, gated to
/// `game_phase == 0`) to any sufficiently simplified position: escorting
/// the pawn home with your own king matters more than the enemy king
/// merely being far away, so the own-king weight is deliberately larger.
const PASSED_PAWN_OWN_KING_WEIGHT: i32 = 4;
const PASSED_PAWN_ENEMY_KING_WEIGHT: i32 = 2;
const PASSED_PAWN_KING_DISTANCE_MAX_PHASE: i32 = 12;

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

/// A pawn with no neighbor-file pawn at its own rank or behind it (so
/// nothing can ever come up to shield/support it from behind) whose stop
/// square is already controlled by an enemy pawn (so advancing loses it
/// outright). Deliberately simpler than the full textbook definition (no
/// check for whether the file is semi-open toward a rook/queen, which
/// would need piece bitboards this function doesn't receive) — a
/// reasonable simplification for a heuristic term, not the exact KPK
/// oracle. Callers must already know the pawn isn't isolated (has some
/// neighbor-file pawn, just not one positioned to help) before calling
/// this, since an isolated pawn would otherwise also match "no supporter"
/// and get double-penalized under both terms.
fn is_backward_pawn(sq: Square, color: Color, own_pawns: Bitboard, enemy_pawns: Bitboard) -> bool {
    let file = sq.file() as i32;
    let rank = sq.rank() as i32;
    let has_supporter_or_future_supporter = own_pawns.into_iter().any(|other| {
        (other.file() as i32 - file).abs() == 1
            && match color {
                Color::White => other.rank() as i32 <= rank,
                Color::Black => other.rank() as i32 >= rank,
            }
    });
    if has_supporter_or_future_supporter {
        return false;
    }
    let stop_rank = match color {
        Color::White => rank + 1,
        Color::Black => rank - 1,
    };
    if !(0..8).contains(&stop_rank) {
        return false; // already on the back rank: not reachable in practice, but never crash
    }
    let stop_square = Square::new(sq.file(), stop_rank as u8);
    pawn_attack_set(enemy_pawns, color.opposite()).contains(stop_square)
}

fn has_phalanx_partner(sq: Square, own_pawns: Bitboard) -> bool {
    let file = sq.file() as i32;
    [file - 1, file + 1]
        .into_iter()
        .any(|f| (0..8).contains(&f) && own_pawns.contains(Square::new(f as u8, sq.rank())))
}

/// Doubled/isolated/backward penalties plus passed-pawn and connected-pawn
/// bonuses for `pawns`, from `color`'s own perspective (always
/// non-negative-biased upward, i.e. a good structure scores higher
/// regardless of which color is being asked about; the caller subtracts
/// Black's from White's). `own_king`/`enemy_king`/`phase` only feed the
/// passed-pawn king-distance term below `PASSED_PAWN_KING_DISTANCE_MAX_PHASE`.
#[allow(clippy::too_many_arguments)]
fn pawn_structure_score_for(
    pawns: Bitboard,
    enemy_pawns: Bitboard,
    color: Color,
    own_king: Square,
    enemy_king: Square,
    phase: i32,
) -> i32 {
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
        } else if is_backward_pawn(sq, color, pawns, enemy_pawns) {
            score -= BACKWARD_PAWN_PENALTY;
        }

        let advance = match color {
            Color::White => sq.rank(),
            Color::Black => 7 - sq.rank(),
        };
        if is_defended_by_pawn(sq, color, pawns) || has_phalanx_partner(sq, pawns) {
            score += PAWN_CONNECTED_BONUS_BY_ADVANCE[advance as usize];
        }
        if is_passed_pawn(sq, color, enemy_pawns) {
            score += PASSED_PAWN_BONUS_BY_ADVANCE[advance as usize];
            if phase <= PASSED_PAWN_KING_DISTANCE_MAX_PHASE {
                score += chebyshev_distance(enemy_king, sq) * PASSED_PAWN_ENEMY_KING_WEIGHT
                    - chebyshev_distance(own_king, sq) * PASSED_PAWN_OWN_KING_WEIGHT;
            }
        }
    }
    score
}

pub fn pawn_structure_score(board: &Board) -> i32 {
    pawn_structure_score_with_phase(board, game_phase(board))
}

fn pawn_structure_score_with_phase(board: &Board, phase: i32) -> i32 {
    let white_pawns = board.pieces_of(Color::White, PieceType::Pawn);
    let black_pawns = board.pieces_of(Color::Black, PieceType::Pawn);
    let (Some(white_king), Some(black_king)) = (
        board.pieces_of(Color::White, PieceType::King).lsb(),
        board.pieces_of(Color::Black, PieceType::King).lsb(),
    ) else {
        return 0; // only reachable mid-test with a kingless FEN
    };
    pawn_structure_score_for(white_pawns, black_pawns, Color::White, white_king, black_king, phase)
        - pawn_structure_score_for(black_pawns, white_pawns, Color::Black, black_king, white_king, phase)
}

/// Sum of piece-square bonuses, White pieces minus Black pieces. The king
/// is scored separately from the other piece types since its table is
/// tapered by game phase instead of being a single fixed table.
pub fn piece_square_score(board: &Board) -> i32 {
    piece_square_score_with_phase(board, game_phase(board))
}

fn piece_square_score_with_phase(board: &Board, phase: i32) -> i32 {
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

// ---------------------------------------------------------------------
// Threats: is a piece sitting where a cheaper enemy piece attacks it? A
// hand-crafted eval has no idea otherwise — confirmed empirically this
// session (see the session notes / `compare_eval.sh`): a knight hanging to
// an undefended pawn scored -247 for the side about to lose it for free,
// when Stockfish/Obsidian/Berserk/Caissa all saw the position as roughly
// even. Deliberately scoped to the classic "cheaper piece attacks more
// valuable one" patterns only, not full SEE-based hanging-piece detection
// (which would need every piece's own defenders, not just attackers) —
// that's a further refinement to consider later, not required to close
// the gap that was actually measured.
//
// Not built from `piece_mobility_for`'s per-piece attack sets above: those
// are summed per piece (so two pieces attacking the same square each get
// separate mobility credit), which is the wrong shape for "is this square
// attacked at all" — a union bitboard is what threats need instead, and
// re-deriving it here is cheap (a handful of array lookups) next to the
// risk of subtly changing mobility's already-tested behavior to share it.
// ---------------------------------------------------------------------

/// One color's attacked squares, aggregated by attacker type (not counted
/// per piece, just unioned) — exactly what `threats_score` needs to ask
/// "is this square attacked by a pawn/minor/rook/queen at all?".
struct AttackInfo {
    pawns: Bitboard,
    minors: Bitboard,
    rooks: Bitboard,
    queens: Bitboard,
}

fn attack_info_for(board: &Board, color: Color, occ: Bitboard) -> AttackInfo {
    let mut minors = Bitboard::EMPTY;
    for sq in board.pieces_of(color, PieceType::Knight) {
        minors = minors | movegen::knight_attacks(sq);
    }
    for sq in board.pieces_of(color, PieceType::Bishop) {
        minors = minors | movegen::bishop_attacks(sq, occ);
    }
    let mut rooks = Bitboard::EMPTY;
    for sq in board.pieces_of(color, PieceType::Rook) {
        rooks = rooks | movegen::rook_attacks(sq, occ);
    }
    let mut queens = Bitboard::EMPTY;
    for sq in board.pieces_of(color, PieceType::Queen) {
        queens = queens | movegen::queen_attacks(sq, occ);
    }
    AttackInfo {
        pawns: pawn_attack_set(board.pieces_of(color, PieceType::Pawn), color),
        minors,
        rooks,
        queens,
    }
}

const MINOR_ATTACKED_BY_PAWN_PENALTY: i32 = 45;
const ROOK_ATTACKED_BY_LESSER_PENALTY: i32 = 35;
const QUEEN_ATTACKED_BY_LESSER_PENALTY: i32 = 40;
const WEAK_PAWN_PENALTY: i32 = 12;

/// Non-negative: how much trouble `victim_color`'s own pieces are in
/// against `attackers` (the *other* color's `AttackInfo`). The caller
/// combines both directions with the right sign.
fn threat_penalty_for(board: &Board, victim_color: Color, attackers: &AttackInfo) -> i32 {
    let own_pawn_defense = pawn_attack_set(board.pieces_of(victim_color, PieceType::Pawn), victim_color);
    let mut penalty = 0;

    let minors = board.pieces_of(victim_color, PieceType::Knight) | board.pieces_of(victim_color, PieceType::Bishop);
    for sq in minors {
        if attackers.pawns.contains(sq) && !own_pawn_defense.contains(sq) {
            penalty += MINOR_ATTACKED_BY_PAWN_PENALTY;
        }
    }

    for sq in board.pieces_of(victim_color, PieceType::Rook) {
        if attackers.pawns.contains(sq) || attackers.minors.contains(sq) {
            penalty += ROOK_ATTACKED_BY_LESSER_PENALTY;
        }
    }

    for sq in board.pieces_of(victim_color, PieceType::Queen) {
        if attackers.pawns.contains(sq) || attackers.minors.contains(sq) || attackers.rooks.contains(sq) {
            penalty += QUEEN_ATTACKED_BY_LESSER_PENALTY;
        }
    }

    for sq in board.pieces_of(victim_color, PieceType::Pawn) {
        let attacked = attackers.pawns.contains(sq)
            || attackers.minors.contains(sq)
            || attackers.rooks.contains(sq)
            || attackers.queens.contains(sq);
        if attacked && !own_pawn_defense.contains(sq) {
            penalty += WEAK_PAWN_PENALTY;
        }
    }

    penalty
}

pub fn threats_score(board: &Board) -> i32 {
    let occ = board.occupied();
    let white_attacks = attack_info_for(board, Color::White, occ);
    let black_attacks = attack_info_for(board, Color::Black, occ);
    threats_score_with_attacks(board, &white_attacks, &black_attacks)
}

fn threats_score_with_attacks(board: &Board, white_attacks: &AttackInfo, black_attacks: &AttackInfo) -> i32 {
    threat_penalty_for(board, Color::Black, white_attacks) - threat_penalty_for(board, Color::White, black_attacks)
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

/// Bonus for a rook on the 7th rank (2nd from its own perspective) that's
/// actually threatening something: the enemy king cut off on the back
/// rank, or an enemy pawn sitting there to be picked off. A 7th-rank rook
/// with neither condition isn't the classical "pig on the 7th", just a
/// rook that happens to be far advanced — already rewarded by its PST and
/// mobility on their own, no extra bonus needed here.
const ROOK_ON_SEVENTH_BONUS: i32 = 20;

fn rook_on_seventh_score_for(board: &Board, color: Color) -> i32 {
    let (seventh_rank, back_rank) = match color {
        Color::White => (6, 7),
        Color::Black => (1, 0),
    };
    let enemy_king_on_back_rank = board
        .pieces_of(color.opposite(), PieceType::King)
        .lsb()
        .is_some_and(|k| k.rank() == back_rank);
    let enemy_pawn_on_seventh = board
        .pieces_of(color.opposite(), PieceType::Pawn)
        .into_iter()
        .any(|s| s.rank() == seventh_rank);
    if !enemy_king_on_back_rank && !enemy_pawn_on_seventh {
        return 0;
    }
    let rooks_on_seventh = board
        .pieces_of(color, PieceType::Rook)
        .into_iter()
        .filter(|r| r.rank() == seventh_rank)
        .count();
    rooks_on_seventh as i32 * ROOK_ON_SEVENTH_BONUS
}

pub fn rook_on_seventh_score(board: &Board) -> i32 {
    rook_on_seventh_score_for(board, Color::White) - rook_on_seventh_score_for(board, Color::Black)
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

// ---------------------------------------------------------------------
// Pure pawn ending heuristics (two or more pawns total, otherwise bare
// kings). The single-pawn case is answered exactly by `kpk_exact_score`
// instead; everything here is a judgment call layered on top of the
// generic terms above (passed-pawn advance, king PST, ...), for the
// concepts that only really matter once no piece but the kings can
// intervene: races, key squares/opposition, and latent (candidate) passers
// from a pawn majority.
// ---------------------------------------------------------------------

fn queening_square(sq: Square, color: Color) -> Square {
    match color {
        Color::White => Square::new(sq.file(), 7),
        Color::Black => Square::new(sq.file(), 0),
    }
}

/// How many of *this pawn's own* moves it needs to reach the back rank,
/// ignoring interference from either king (that's handled separately by
/// `is_square_rule_catch`): the plain rank distance, minus one tempo if
/// it's still on its starting rank and can use the double step.
fn plies_to_queen(sq: Square, color: Color) -> i32 {
    let (advance, start_rank) = match color {
        Color::White => (7 - sq.rank() as i32, 1),
        Color::Black => (sq.rank() as i32, 6),
    };
    if sq.rank() == start_rank {
        advance - 1
    } else {
        advance
    }
}

/// The passed pawn of `color` closest to queening, if any.
fn best_passer(pawns: Bitboard, enemy_pawns: Bitboard, color: Color) -> Option<Square> {
    pawns
        .filter(|&sq| is_passed_pawn(sq, color, enemy_pawns))
        .min_by_key(|&sq| plies_to_queen(sq, color))
}

/// The classic "rule of the square", generalized with tempo: can
/// `defender_king` reach the pawn's queening square in time to stop it,
/// crediting it an extra step of head start if it's the defender's move?
fn is_square_rule_catch(pawn: Square, pawn_color: Color, defender_king: Square, defender_to_move: bool) -> bool {
    let target = queening_square(pawn, pawn_color);
    let mut king_distance = chebyshev_distance(defender_king, target);
    if defender_to_move {
        king_distance -= 1;
    }
    king_distance <= plies_to_queen(pawn, pawn_color)
}

/// Bonus for having the pawn race clearly won: one side's most dangerous
/// passer is outrunning the defending king (per the square rule) while the
/// other side has no such runner, or, if both do, whichever queens first
/// once whose move it is gets credited. A genuine tie is left for the
/// search to resolve on its own (checks, a defended queening square, etc.
/// can decide it in ways this heuristic can't see).
const RACE_WIN_BONUS: i32 = 120;
const RACE_TEMPO_WEIGHT: i32 = 15;
const RACE_TEMPO_CAP: i32 = 5;

fn pawn_race_score(board: &Board) -> i32 {
    let white_pawns = board.pieces_of(Color::White, PieceType::Pawn);
    let black_pawns = board.pieces_of(Color::Black, PieceType::Pawn);
    let (Some(white_king), Some(black_king)) = (
        board.pieces_of(Color::White, PieceType::King).lsb(),
        board.pieces_of(Color::Black, PieceType::King).lsb(),
    ) else {
        return 0; // only reachable mid-test with a kingless FEN
    };

    let white_best = best_passer(white_pawns, black_pawns, Color::White);
    let black_best = best_passer(black_pawns, white_pawns, Color::Black);
    let white_to_move = board.side_to_move == Color::White;

    let white_runs_free = white_best.is_some_and(|sq| !is_square_rule_catch(sq, Color::White, black_king, !white_to_move));
    let black_runs_free = black_best.is_some_and(|sq| !is_square_rule_catch(sq, Color::Black, white_king, white_to_move));

    match (white_runs_free, black_runs_free) {
        (true, false) => RACE_WIN_BONUS,
        (false, true) => -RACE_WIN_BONUS,
        (false, false) => 0,
        (true, true) => {
            let margin = plies_to_queen(black_best.unwrap(), Color::Black) - plies_to_queen(white_best.unwrap(), Color::White);
            let adjusted = if white_to_move { margin } else { margin - 1 };
            match adjusted.cmp(&0) {
                std::cmp::Ordering::Equal => 0,
                std::cmp::Ordering::Greater => RACE_WIN_BONUS + adjusted.min(RACE_TEMPO_CAP) * RACE_TEMPO_WEIGHT,
                std::cmp::Ordering::Less => -(RACE_WIN_BONUS + (-adjusted).min(RACE_TEMPO_CAP) * RACE_TEMPO_WEIGHT),
            }
        }
    }
}

/// Extra credit, beyond the flat advance-based bonus `pawn_structure_score`
/// already gives every passed pawn, for two properties that matter far
/// more once nothing but a king can ever stop the pawn: being protected by
/// another pawn (the defending king can't approach without walking into
/// that defender), and being an "outside" passer, far from the rest of the
/// pawns (it's an equally strong runner, but it also drags the defending
/// king away from the theater where the other pawns live).
const PROTECTED_PASSER_BONUS: i32 = 20;
const OUTSIDE_PASSER_BONUS_PER_FILE: i32 = 8;
const OUTSIDE_PASSER_MAX_FILES: i32 = 5;

fn outside_passer_bonus(passer: Square, other_pawns: Bitboard) -> i32 {
    if other_pawns.is_empty() {
        return 0;
    }
    let count = other_pawns.count() as i32;
    let file_sum: i32 = other_pawns.map(|sq| sq.file() as i32).sum();
    let avg_file = file_sum / count;
    let distance = (passer.file() as i32 - avg_file).abs();
    (distance - 2).clamp(0, OUTSIDE_PASSER_MAX_FILES) * OUTSIDE_PASSER_BONUS_PER_FILE
}

fn passer_quality_score_for(pawns: Bitboard, enemy_pawns: Bitboard, color: Color) -> i32 {
    let mut score = 0;
    for sq in pawns {
        if !is_passed_pawn(sq, color, enemy_pawns) {
            continue;
        }
        if is_defended_by_pawn(sq, color, pawns) {
            score += PROTECTED_PASSER_BONUS;
        }
        let mut other_own = pawns;
        other_own.clear(sq);
        score += outside_passer_bonus(sq, other_own | enemy_pawns);
    }
    score
}

fn passer_quality_score(board: &Board) -> i32 {
    let white_pawns = board.pieces_of(Color::White, PieceType::Pawn);
    let black_pawns = board.pieces_of(Color::Black, PieceType::Pawn);
    passer_quality_score_for(white_pawns, black_pawns, Color::White) - passer_quality_score_for(black_pawns, white_pawns, Color::Black)
}

/// Rough "key squares" for a pawn that hasn't queened yet: the three
/// squares two ranks ahead of it (clamped to the board), a standard
/// approximation of classical key-square theory that is deliberately not
/// exact about the pawn's most advanced ranks — this is a heuristic for
/// the multi-pawn case, not the exact single-pawn oracle in `kpk.rs`.
fn key_squares(pawn: Square, color: Color) -> [Option<Square>; 3] {
    let target_rank = match color {
        Color::White => (pawn.rank() as i32 + 2).min(7),
        Color::Black => (pawn.rank() as i32 - 2).max(0),
    };
    let file = pawn.file() as i32;
    let mut squares = [None; 3];
    for (i, df) in [-1, 0, 1].into_iter().enumerate() {
        let f = file + df;
        if (0..8).contains(&f) {
            squares[i] = Some(Square::new(f as u8, target_rank as u8));
        }
    }
    squares
}

/// Bonus for the attacking king occupying one of the pawn's key squares,
/// penalty if the defending king got there first (the classical drawing
/// mechanism), halved for a rook pawn since the defending king only needs
/// the corner, not genuine control of specific squares, to hold the draw.
const KEY_SQUARE_BONUS: i32 = 25;

fn key_square_control_score(pawn: Square, color: Color, attacker_king: Square, defender_king: Square) -> i32 {
    let mut score = 0;
    for sq in key_squares(pawn, color).into_iter().flatten() {
        if sq == attacker_king {
            score += KEY_SQUARE_BONUS;
        }
        if sq == defender_king {
            score -= KEY_SQUARE_BONUS;
        }
    }
    if pawn.file() == 0 || pawn.file() == 7 {
        score /= 2;
    }
    score
}

/// Kings squarely facing off on the same file or rank, close enough for it
/// to matter (direct or one-move-removed "distant" opposition): the side
/// that does *not* have to move right now holds the opposition. This is
/// the one place in Vigia's eval where having the move is treated as a
/// potential liability rather than a plus — the zugzwang risk that barely
/// exists anywhere else in chess but is central to king-and-pawn endings.
const OPPOSITION_BONUS: i32 = 15;

fn opposition_score(white_king: Square, black_king: Square, side_to_move: Color) -> i32 {
    let same_file = white_king.file() == black_king.file();
    let same_rank = white_king.rank() == black_king.rank();
    if !same_file && !same_rank {
        return 0;
    }
    let distance = chebyshev_distance(white_king, black_king);
    if distance != 2 && distance != 4 {
        return 0;
    }
    match side_to_move.opposite() {
        Color::White => OPPOSITION_BONUS,
        Color::Black => -OPPOSITION_BONUS,
    }
}

fn key_square_and_opposition_score(board: &Board) -> i32 {
    let white_pawns = board.pieces_of(Color::White, PieceType::Pawn);
    let black_pawns = board.pieces_of(Color::Black, PieceType::Pawn);
    let (Some(white_king), Some(black_king)) = (
        board.pieces_of(Color::White, PieceType::King).lsb(),
        board.pieces_of(Color::Black, PieceType::King).lsb(),
    ) else {
        return 0; // only reachable mid-test with a kingless FEN
    };

    let mut score = opposition_score(white_king, black_king, board.side_to_move);
    if let Some(sq) = best_passer(white_pawns, black_pawns, Color::White) {
        score += key_square_control_score(sq, Color::White, white_king, black_king);
    }
    if let Some(sq) = best_passer(black_pawns, white_pawns, Color::Black) {
        score -= key_square_control_score(sq, Color::Black, black_king, white_king);
    }
    score
}

/// A pawn majority on a wing with no passed pawn there yet is a *candidate*
/// passer: latent potential to create one by force, which matters far more
/// here than with pieces on the board, since nothing but the king can ever
/// stop the pawn that eventually breaks through.
const CANDIDATE_MAJORITY_BONUS: i32 = 15;
const WINGS: [std::ops::Range<u8>; 2] = [0..4, 4..8];

fn wing_majority_score_for(pawns: Bitboard, enemy_pawns: Bitboard, color: Color) -> i32 {
    let mut score = 0;
    for wing in WINGS {
        let own_count = pawns.filter(|sq| wing.contains(&sq.file())).count();
        let enemy_count = enemy_pawns.filter(|sq| wing.contains(&sq.file())).count();
        if own_count <= enemy_count {
            continue;
        }
        let already_passed = pawns
            .filter(|sq| wing.contains(&sq.file()))
            .any(|sq| is_passed_pawn(sq, color, enemy_pawns));
        if !already_passed {
            score += CANDIDATE_MAJORITY_BONUS;
        }
    }
    score
}

fn pawn_majority_score(board: &Board) -> i32 {
    let white_pawns = board.pieces_of(Color::White, PieceType::Pawn);
    let black_pawns = board.pieces_of(Color::Black, PieceType::Pawn);
    wing_majority_score_for(white_pawns, black_pawns, Color::White) - wing_majority_score_for(black_pawns, white_pawns, Color::Black)
}

fn pawn_endgame_score(board: &Board, phase: i32) -> i32 {
    if phase != 0 {
        return 0;
    }
    let total_pawns =
        board.pieces_of(Color::White, PieceType::Pawn).count() + board.pieces_of(Color::Black, PieceType::Pawn).count();
    if total_pawns < 2 {
        return 0; // 0 and 1 pawn are handled elsewhere (trivial draw / exact KPK oracle)
    }
    pawn_race_score(board) + passer_quality_score(board) + key_square_and_opposition_score(board) + pawn_majority_score(board)
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
        // Not 0: White is to move, and `tempo_score` credits that.
        assert_eq!(evaluate(&board), TEMPO_BONUS);
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
        let black_attacks = attack_info_for(&board, Color::Black, board.occupied());
        assert_eq!(king_safety_penalty(&board, Color::White, 0, &black_attacks), 0);
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

    #[test]
    fn evaluate_returns_a_flat_draw_for_a_proven_kpk_draw() {
        // White Ka1, Pe4 (undefended); Black Ke5 to move captures it: a
        // textbook draw by insufficient material, and `evaluate` should say
        // so plainly (see `kpk_exact_score`), not hedge with a small score.
        let board = Board::from_fen("8/8/8/4k3/4P3/8/8/K7 b - - 0 1").unwrap();
        assert_eq!(evaluate(&board), 0);
    }

    #[test]
    fn evaluate_returns_a_decisive_bonus_for_a_proven_kpk_win() {
        // White Ke2, Pe7, Black Ka1 to move: nothing can stop the pawn, and
        // `evaluate` should say so with a score nowhere near an ordinary
        // positional or material swing.
        let board = Board::from_fen("8/4P3/8/8/8/8/4K3/k7 w - - 0 1").unwrap();
        assert!(evaluate(&board) >= KPK_DECISIVE_BONUS);
    }

    #[test]
    fn pawn_race_score_rewards_an_unstoppable_passer_with_no_reply() {
        let board = Board::from_fen("7k/8/8/8/P7/8/8/4K3 w - - 0 1").unwrap();
        assert_eq!(pawn_race_score(&board), RACE_WIN_BONUS);
    }

    #[test]
    fn pawn_race_score_is_silent_when_neither_pawn_is_a_free_runner() {
        let board = Board::from_fen("k7/8/8/4p3/4P3/8/8/K7 w - - 0 1").unwrap();
        assert_eq!(pawn_race_score(&board), 0);
    }

    #[test]
    fn passer_quality_score_rewards_a_protected_passed_pawn() {
        let protected = Board::from_fen("k7/8/8/4P3/3P4/8/8/4K3 w - - 0 1").unwrap();
        let unprotected = Board::from_fen("k7/8/8/4P3/8/8/8/4K3 w - - 0 1").unwrap();
        assert_eq!(passer_quality_score(&protected), PROTECTED_PASSER_BONUS);
        assert_eq!(passer_quality_score(&unprotected), 0);
    }

    #[test]
    fn outside_passer_bonus_grows_with_distance_from_the_rest_of_the_pawns() {
        let cluster = Bitboard::from_square(Square::new(4, 3)); // e4
        let far = outside_passer_bonus(Square::new(0, 4), cluster); // a5, 4 files away
        let near = outside_passer_bonus(Square::new(3, 4), cluster); // d4, 1 file away
        assert!(far > near);
        assert_eq!(near, 0);
    }

    #[test]
    fn key_square_control_score_favors_whichever_king_occupies_it() {
        let pawn = Square::new(4, 3); // e4; key squares are d6/e6/f6
        let attacker_on_key_square = key_square_control_score(pawn, Color::White, Square::new(3, 5), Square::new(0, 0));
        let defender_on_key_square = key_square_control_score(pawn, Color::White, Square::new(0, 0), Square::new(3, 5));
        assert!(attacker_on_key_square > 0);
        assert_eq!(attacker_on_key_square, -defender_on_key_square);
    }

    #[test]
    fn opposition_score_favors_the_side_not_to_move() {
        let white_king = Square::new(4, 3); // e4
        let black_king = Square::new(4, 5); // e6: direct opposition, two files apart
        assert_eq!(opposition_score(white_king, black_king, Color::Black), OPPOSITION_BONUS);
        assert_eq!(opposition_score(white_king, black_king, Color::White), -OPPOSITION_BONUS);
    }

    #[test]
    fn pawn_majority_score_rewards_a_clean_wing_majority() {
        // White has 2 vs 1 on the queenside with neither pawn passed yet
        // (both blocked by the a7 pawn on/adjacent to their files): a
        // candidate passer, worth a bonus purely from the pure-pawn-ending
        // heuristics, on top of anything `pawn_structure_score` already
        // gives the individual pawns.
        let board = Board::from_fen("4k3/p7/8/8/8/8/PP6/4K3 w - - 0 1").unwrap();
        assert_eq!(pawn_majority_score(&board), CANDIDATE_MAJORITY_BONUS);
    }

    #[test]
    fn tempo_score_favors_the_side_to_move() {
        let board = Board::from_fen("4k3/8/8/8/8/8/8/4K3 w - - 0 1").unwrap();
        let mirrored = Board::from_fen("4k3/8/8/8/8/8/8/4K3 b - - 0 1").unwrap();
        assert_eq!(tempo_score(&board), TEMPO_BONUS);
        assert_eq!(tempo_score(&mirrored), -TEMPO_BONUS);
    }

    #[test]
    fn is_backward_pawn_detects_a_pawn_with_no_supporter_and_a_controlled_stop_square() {
        // White d2, with c3/e3 already advanced past it (neither can ever
        // come back to defend it) and a black pawn on e4 controlling d3.
        let own = Bitboard::from_square(Square::new(3, 1)) // d2
            | Bitboard::from_square(Square::new(2, 2)) // c3
            | Bitboard::from_square(Square::new(4, 2)); // e3
        let enemy = Bitboard::from_square(Square::new(4, 3)); // e4
        assert!(is_backward_pawn(Square::new(3, 1), Color::White, own, enemy));
    }

    #[test]
    fn is_backward_pawn_is_false_with_a_same_rank_supporter() {
        // Same stop-square control from e4, but c2/e2 (same rank as d2,
        // not already advanced past it) could still come up and defend it.
        let own = Bitboard::from_square(Square::new(3, 1)) // d2
            | Bitboard::from_square(Square::new(2, 1)) // c2
            | Bitboard::from_square(Square::new(4, 1)); // e2
        let enemy = Bitboard::from_square(Square::new(4, 3)); // e4
        assert!(!is_backward_pawn(Square::new(3, 1), Color::White, own, enemy));
    }

    #[test]
    fn has_phalanx_partner_detects_an_adjacent_pawn_on_the_same_rank() {
        let side_by_side = Bitboard::from_square(Square::new(3, 3)) | Bitboard::from_square(Square::new(4, 3));
        assert!(has_phalanx_partner(Square::new(3, 3), side_by_side));
        let alone = Bitboard::from_square(Square::new(3, 3));
        assert!(!has_phalanx_partner(Square::new(3, 3), alone));
    }

    #[test]
    fn bishop_pawns_score_penalizes_own_pawns_on_the_bishops_color() {
        // Dark-squared bishop on c1; b2/d2 are also dark squares.
        let bad = Board::from_fen("4k3/8/8/8/8/8/1P1P4/2B1K3 w - - 0 1").unwrap();
        // Same bishop, but the pawns are on c2/e2 (light squares) instead.
        let good = Board::from_fen("4k3/8/8/8/8/8/2P1P3/2B1K3 w - - 0 1").unwrap();
        assert_eq!(bishop_pawns_score(&bad), -2 * BAD_BISHOP_PAWN_PENALTY);
        assert_eq!(bishop_pawns_score(&good), 0);
    }

    #[test]
    fn bishop_pawns_score_penalizes_a_rammed_pawn_more() {
        // Same "bad" position as above, plus a black pawn on d3 directly
        // blocking the white d2 pawn head-on.
        let board = Board::from_fen("4k3/8/8/8/8/3p4/1P1P4/2B1K3 w - - 0 1").unwrap();
        assert_eq!(bishop_pawns_score(&board), -BAD_BISHOP_PAWN_PENALTY - BAD_BISHOP_RAMMED_PAWN_PENALTY);
    }

    #[test]
    fn rook_on_seventh_score_requires_a_real_target() {
        let threatening = Board::from_fen("4k3/R7/8/8/8/8/8/4K3 w - - 0 1").unwrap();
        assert_eq!(rook_on_seventh_score(&threatening), ROOK_ON_SEVENTH_BONUS);
        // Same rook on the 7th, but the black king isn't on the back rank
        // and there's no black pawn on the 7th either: no real target.
        let empty_seventh = Board::from_fen("8/R7/4k3/8/8/8/8/4K3 w - - 0 1").unwrap();
        assert_eq!(rook_on_seventh_score(&empty_seventh), 0);
    }

    #[test]
    fn passed_pawn_king_distance_favors_the_closer_escorting_king() {
        // Same passed pawn (a5) and same distant black king (e8) in both;
        // only the white king's distance to the pawn changes.
        let king_close = Board::from_fen("4k3/8/8/P7/8/K7/8/8 w - - 0 1").unwrap();
        let king_far = Board::from_fen("4k3/8/8/P7/8/8/8/7K w - - 0 1").unwrap();
        assert!(pawn_structure_score(&king_close) > pawn_structure_score(&king_far));
    }

    #[test]
    fn opposite_colored_bishops_ending_gets_scaled_down() {
        // White up a pawn with opposite-colored bishops: the exact pattern
        // this session's live comparison against Stockfish/Obsidian/Berserk
        // flagged as overconfident (Vigia gave +87, the reference engines
        // gave roughly 0).
        let board = Board::from_fen("8/5k2/8/3b4/8/3P4/2K2B2/8 w - - 0 1").unwrap();
        let score = evaluate(&board);
        assert!(score.abs() <= 30, "expected a heavily scaled-down score, got {score}");
    }

    #[test]
    fn endgame_scale_factor_leaves_a_same_colored_bishop_ending_unscaled() {
        // Same idea, but both bishops are on dark squares: no opposite-
        // colored-bishops drawishness, so no scaling should apply.
        let same_color = Board::from_fen("8/5k2/3b4/8/8/3P4/2K2B2/8 w - - 0 1").unwrap();
        assert_eq!(endgame_scale_factor(&same_color), SCALE_FACTOR_NORMAL);
    }

    #[test]
    fn endgame_scale_factor_ignores_opposite_bishops_if_other_pieces_remain() {
        // Same opposite-colored bishops, but White also has a rook: not a
        // "pure" OCB ending anymore, so no scaling.
        let board = Board::from_fen("8/5k2/8/3b4/8/3P4/2K2B2/4R3 w - - 0 1").unwrap();
        assert_eq!(endgame_scale_factor(&board), SCALE_FACTOR_NORMAL);
    }

    #[test]
    fn evaluate_flattens_bare_kings_and_a_lone_minor_via_the_scale_factor() {
        // Mirrors `search::is_insufficient_material`: these positions are
        // an exact draw regardless of what material_score/piece_square_score
        // alone would suggest.
        let bare_kings = Board::from_fen("4k3/8/8/8/8/8/8/4K3 w - - 0 1").unwrap();
        assert_eq!(evaluate(&bare_kings), 0);
        let lone_knight = Board::from_fen("4k3/8/8/8/8/8/8/3NK3 w - - 0 1").unwrap();
        assert_eq!(evaluate(&lone_knight), 0);
    }

    #[test]
    fn threats_score_penalizes_an_undefended_minor_attacked_by_a_pawn() {
        // Black knight d5 hanging to the white pawn on e4, with no black
        // pawn defending it. This is the exact pattern that scored -247
        // for the side about to lose the piece before this term existed.
        let hanging = Board::from_fen("4k3/8/8/3n4/4P3/8/8/4K3 w - - 0 1").unwrap();
        assert_eq!(threats_score(&hanging), MINOR_ATTACKED_BY_PAWN_PENALTY);

        // Same attack, but a black pawn on c6 now defends d5: no penalty.
        let defended = Board::from_fen("4k3/8/2p5/3n4/4P3/8/8/4K3 w - - 0 1").unwrap();
        assert_eq!(threats_score(&defended), 0);
    }

    #[test]
    fn threats_score_penalizes_a_rook_attacked_by_a_cheaper_piece() {
        let board = Board::from_fen("4k3/8/8/1n6/3R4/8/8/4K3 w - - 0 1").unwrap();
        assert_eq!(threats_score(&board), -ROOK_ATTACKED_BY_LESSER_PENALTY);
    }

    #[test]
    fn threats_score_penalizes_an_undefended_pawn_under_attack() {
        let board = Board::from_fen("4k3/8/8/2n5/4P3/8/8/4K3 w - - 0 1").unwrap();
        assert_eq!(threats_score(&board), -WEAK_PAWN_PENALTY);
    }

    #[test]
    fn evaluate_partially_corrects_for_a_hanging_knight() {
        // Before `threats_score` existed, this position (white pawn about
        // to win a free knight next move) evaluated around -247/-239:
        // `evaluate` thought Black was clearly better by *more* than the
        // raw material gap alone would suggest. A static term can only
        // ever partially close a "piece is just gone" gap like this one —
        // actually resolving it is what quiescence search's real capture
        // is for, not a static eval pretending to already know the
        // outcome — so this checks the correction is real and in the
        // right direction, not that it fully matches the reference
        // engines' near-even NNUE judgment.
        let board = Board::from_fen("4k3/8/8/3n4/4P3/8/8/4K3 w - - 0 1").unwrap();
        let score = evaluate(&board);
        assert!(
            score > material_score(&board),
            "expected threats_score to improve on raw material alone, got {score}"
        );
    }

    #[test]
    fn shelter_penalty_grows_with_distance_to_the_nearest_pawn() {
        let close = Bitboard::from_square(Square::new(4, 1)); // e2, one rank from a rank-0 king
        let far = Bitboard::from_square(Square::new(4, 3)); // e4, three ranks away
        assert!(shelter_penalty(close, 4, 0, Color::White) < shelter_penalty(far, 4, 0, Color::White));
        assert_eq!(shelter_penalty(Bitboard::EMPTY, 4, 0, Color::White), SHELTER_PENALTY_CAP);
    }

    #[test]
    fn storm_penalty_grows_as_the_enemy_pawn_gets_closer() {
        let close = Bitboard::from_square(Square::new(4, 1)); // e2: one step from a rank-0 king
        let far = Bitboard::from_square(Square::new(4, 5)); // e6: still far off
        assert!(storm_penalty(close, 4, 0, Color::White) > storm_penalty(far, 4, 0, Color::White));
        assert_eq!(storm_penalty(Bitboard::EMPTY, 4, 0, Color::White), 0);
    }

    #[test]
    fn king_danger_penalty_grows_faster_than_linearly_with_attacker_count() {
        let ring = king_ring(Square::new(4, 0)); // e1
        let empty_info = || AttackInfo { pawns: Bitboard::EMPTY, minors: Bitboard::EMPTY, rooks: Bitboard::EMPTY, queens: Bitboard::EMPTY };
        let one_ring_square_attacked =
            AttackInfo { rooks: Bitboard::from_square(Square::new(4, 1)), ..empty_info() }; // e2, in the ring
        let two_ring_squares_attacked = AttackInfo {
            rooks: Bitboard::from_square(Square::new(4, 1)) | Bitboard::from_square(Square::new(3, 1)), // e2, d2
            ..empty_info()
        };
        let one = king_danger_penalty(ring, &one_ring_square_attacked);
        let two = king_danger_penalty(ring, &two_ring_squares_attacked);
        assert!(two > one * 2, "expected super-linear growth: one={one}, two={two}");
    }

    #[test]
    fn king_safety_score_penalizes_real_attackers_bearing_on_the_king_ring() {
        // Same material and same king positions in both: a black queen and
        // rook already lined up on White's king in one, the queen moved
        // off to a harmless square in the other (the rook alone still
        // bears on the file, so this isolates the queen's contribution
        // without an all-or-nothing comparison).
        let exposed = Board::from_fen("1k4r1/8/8/8/7q/8/8/7K w - - 0 1").unwrap();
        let safer = Board::from_fen("1k4r1/8/8/8/q7/8/8/7K w - - 0 1").unwrap();
        assert!(king_safety_score(&exposed) < king_safety_score(&safer));
    }
}
