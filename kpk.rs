//! Exact win/draw oracle for the elementary King+Pawn vs King endgame (one
//! side has exactly one pawn and otherwise a bare king; the other side has
//! only a king). Unlike the rest of `eval.rs`, which scores positions with
//! heuristics, this is a genuinely *solved* sub-game: the outcome is
//! computed once via retrograde analysis over every reachable coordinate
//! and cached, so `probe` answers with certainty rather than a guess.
//!
//! This module is intentionally self-contained (it only depends on
//! `types::{Color, Square}`, not on `Board` or `movegen`): the reduced
//! three-piece game has much simpler rules than full chess, and hand-rolling
//! them here avoids cloning a whole `Board` for each of the ~200,000
//! coordinates the analysis visits.

use std::sync::OnceLock;

use crate::types::{Color, Square};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Outcome {
    /// The stronger side (the one with the pawn) can force promotion or
    /// checkmate against any defense.
    Win,
    /// The weaker side can prevent the pawn from ever queening, forever
    /// (by capturing it, by stalemate, or by an unresolvable fortress).
    Draw,
}

/// After canonicalization (see `probe`) the pawn always belongs to "White"
/// advancing toward rank 8, and by board symmetry its file is always a-d
/// (a final on the c-file is a mirror image of one on the f-file), so only
/// 4 files x 6 ranks (2-7; a pawn can never sit on rank 1 or 8) of pawn
/// squares are ever indexed.
const PAWN_FILES: usize = 4;
const PAWN_RANKS: usize = 6;
const PAWN_SQUARES: usize = PAWN_FILES * PAWN_RANKS;
const BOARD_SQUARES: usize = 64;
const TABLE_SIZE: usize = PAWN_SQUARES * BOARD_SQUARES * BOARD_SQUARES * 2;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Status {
    Unknown,
    Win,
    Draw,
}

/// A legal transition out of a state. `Win`/`Draw` are absorbing results
/// reached the instant a pawn promotes or is captured, without needing a
/// coordinate in the table (promoting leaves this pawn-only state space
/// entirely; capturing leaves king-vs-king, trivially drawn).
enum Transition {
    Win,
    Draw,
    Next { wk: Square, bk: Square, pawn: Square, strong_to_move: bool },
}

fn chebyshev(a: Square, b: Square) -> i32 {
    let file_dist = (a.file() as i32 - b.file() as i32).abs();
    let rank_dist = (a.rank() as i32 - b.rank() as i32).abs();
    file_dist.max(rank_dist)
}

const KING_DELTAS: [(i32, i32); 8] = [
    (-1, -1), (-1, 0), (-1, 1),
    (0, -1), (0, 1),
    (1, -1), (1, 0), (1, 1),
];

fn king_neighbors(sq: Square) -> impl Iterator<Item = Square> {
    let file = sq.file() as i32;
    let rank = sq.rank() as i32;
    KING_DELTAS.into_iter().filter_map(move |(df, dr)| {
        let f = file + df;
        let r = rank + dr;
        ((0..8).contains(&f) && (0..8).contains(&r)).then(|| Square::new(f as u8, r as u8))
    })
}

/// Squares a canonical (upward-moving) pawn on `pawn` attacks diagonally.
fn pawn_attacks(pawn: Square) -> [Option<Square>; 2] {
    let file = pawn.file() as i32;
    let rank = pawn.rank() as i32 + 1;
    let mut out = [None, None];
    if rank < 8 {
        if file - 1 >= 0 {
            out[0] = Some(Square::new((file - 1) as u8, rank as u8));
        }
        if file + 1 < 8 {
            out[1] = Some(Square::new((file + 1) as u8, rank as u8));
        }
    }
    out
}

fn pawn_attacks_contains(pawn: Square, sq: Square) -> bool {
    pawn_attacks(pawn).into_iter().flatten().any(|s| s == sq)
}

fn pawn_index(pawn: Square) -> usize {
    (pawn.rank() as usize - 1) * PAWN_FILES + pawn.file() as usize
}

fn square_from_pawn_index(idx: usize) -> Square {
    let file = (idx % PAWN_FILES) as u8;
    let rank = (idx / PAWN_FILES) as u8 + 1;
    Square::new(file, rank)
}

fn state_index(wk: Square, bk: Square, pawn: Square, strong_to_move: bool) -> usize {
    ((pawn_index(pawn) * BOARD_SQUARES + wk.0 as usize) * BOARD_SQUARES + bk.0 as usize) * 2
        + strong_to_move as usize
}

/// Is this coordinate a position that could actually occur in a real game?
/// Kings can never coincide or stand adjacent to each other, neither king
/// can stand on the pawn's square, and the weak king can never be "in
/// check" from the pawn on a turn where it is the strong side who is about
/// to move (that would mean the weak side's own last move left its king in
/// check, which is illegal).
fn is_valid(wk: Square, bk: Square, pawn: Square, strong_to_move: bool) -> bool {
    if wk == bk || wk == pawn || bk == pawn {
        return false;
    }
    if chebyshev(wk, bk) <= 1 {
        return false;
    }
    if strong_to_move && pawn_attacks_contains(pawn, bk) {
        return false;
    }
    true
}

/// Candidate moves for the strong side (king or pawn) at `(wk, bk, pawn)`.
/// The strong king can never be left with zero legal moves here: the weak
/// king can only ever exclude a couple of its up-to-8 neighbors (it can't
/// come within a chebyshev distance of 1 of the strong king by definition
/// of a valid state), so at least one king move always survives even in a
/// corner, regardless of what the single pawn is doing.
fn strong_transitions(wk: Square, bk: Square, pawn: Square) -> Vec<Transition> {
    let mut moves = Vec::with_capacity(8);
    for dest in king_neighbors(wk) {
        if dest == pawn || chebyshev(dest, bk) <= 1 {
            continue;
        }
        moves.push(Transition::Next { wk: dest, bk, pawn, strong_to_move: false });
    }

    let (file, rank) = (pawn.file(), pawn.rank());
    if rank < 7 {
        let single = Square::new(file, rank + 1);
        if single != wk && single != bk {
            if single.rank() == 7 {
                moves.push(Transition::Win);
            } else {
                moves.push(Transition::Next { wk, bk, pawn: single, strong_to_move: false });
            }
            if rank == 1 {
                let double = Square::new(file, rank + 2);
                if double != wk && double != bk {
                    moves.push(Transition::Next { wk, bk, pawn: double, strong_to_move: false });
                }
            }
        }
    }
    moves
}

/// Candidate moves for the weak side (king only) at `(wk, bk, pawn)`.
fn weak_transitions(wk: Square, bk: Square, pawn: Square) -> Vec<Transition> {
    let mut moves = Vec::with_capacity(8);
    for dest in king_neighbors(bk) {
        if chebyshev(dest, wk) <= 1 {
            continue;
        }
        if dest == pawn {
            moves.push(Transition::Draw); // captures the pawn: king vs king, insufficient material
            continue;
        }
        if pawn_attacks_contains(pawn, dest) {
            continue; // would move into check from the pawn
        }
        moves.push(Transition::Next { wk, bk: dest, pawn, strong_to_move: true });
    }
    moves
}

/// Resolves `moves` against the current `status` table. Returns
/// `Status::Unknown` if the result can't be determined yet (some
/// unresolved move could still flip it). An empty move list should be
/// unreachable in a legal state (see `strong_transitions` and the base
/// pass in `build_table`, which classifies all weak states with zero
/// moves directly), but is treated as a safe `Draw` rather than panicking
/// if it ever occurred, since a search-facing oracle must never crash.
fn classify(moves: &[Transition], status: &[Status], strong_to_move: bool) -> Status {
    if moves.is_empty() {
        return Status::Draw;
    }
    let mut any_unknown = false;
    if strong_to_move {
        for mv in moves {
            match mv {
                Transition::Win => return Status::Win,
                Transition::Draw => {}
                Transition::Next { wk, bk, pawn, strong_to_move: next } => {
                    match status[state_index(*wk, *bk, *pawn, *next)] {
                        Status::Win => return Status::Win,
                        Status::Unknown => any_unknown = true,
                        Status::Draw => {}
                    }
                }
            }
        }
        if any_unknown { Status::Unknown } else { Status::Draw }
    } else {
        for mv in moves {
            match mv {
                Transition::Draw => return Status::Draw,
                Transition::Win => {}
                Transition::Next { wk, bk, pawn, strong_to_move: next } => {
                    match status[state_index(*wk, *bk, *pawn, *next)] {
                        Status::Draw => return Status::Draw,
                        Status::Unknown => any_unknown = true,
                        Status::Win => {}
                    }
                }
            }
        }
        if any_unknown { Status::Unknown } else { Status::Win }
    }
}

/// Builds the full table via a fixed-point relaxation: repeatedly sweep
/// every still-unknown state and resolve what can be resolved, until a full
/// sweep changes nothing. Any state left unknown at that point is a Draw by
/// construction (this is a reachability game for the strong side: if it
/// cannot force a win in finitely many moves, the weak side can avoid one
/// forever). This is not the fastest possible construction (a real
/// retrograde pass with a predecessor list would converge in one pass
/// instead of several sweeps over the whole table), but the whole table is
/// under 200,000 states and only built once per process, so the simpler,
/// easier-to-verify sweep is the right trade-off here.
fn build_table() -> Vec<bool> {
    let mut status = vec![Status::Unknown; TABLE_SIZE];

    // Base pass: classify every weak-to-move state with zero legal moves.
    for pawn_idx in 0..PAWN_SQUARES {
        let pawn = square_from_pawn_index(pawn_idx);
        for wk_i in 0u8..64 {
            let wk = Square(wk_i);
            for bk_i in 0u8..64 {
                let bk = Square(bk_i);
                if !is_valid(wk, bk, pawn, false) {
                    continue;
                }
                if weak_transitions(wk, bk, pawn).is_empty() {
                    let idx = state_index(wk, bk, pawn, false);
                    status[idx] = if pawn_attacks_contains(pawn, bk) { Status::Win } else { Status::Draw };
                }
            }
        }
    }

    loop {
        let mut changed = false;
        for pawn_idx in 0..PAWN_SQUARES {
            let pawn = square_from_pawn_index(pawn_idx);
            for wk_i in 0u8..64 {
                let wk = Square(wk_i);
                for bk_i in 0u8..64 {
                    let bk = Square(bk_i);
                    for &strong_to_move in &[true, false] {
                        if !is_valid(wk, bk, pawn, strong_to_move) {
                            continue;
                        }
                        let idx = state_index(wk, bk, pawn, strong_to_move);
                        if status[idx] != Status::Unknown {
                            continue;
                        }
                        let moves = if strong_to_move {
                            strong_transitions(wk, bk, pawn)
                        } else {
                            weak_transitions(wk, bk, pawn)
                        };
                        let result = classify(&moves, &status, strong_to_move);
                        if result != Status::Unknown {
                            status[idx] = result;
                            changed = true;
                        }
                    }
                }
            }
        }
        if !changed {
            break;
        }
    }

    status.into_iter().map(|s| s == Status::Win).collect()
}

static TABLE: OnceLock<Vec<bool>> = OnceLock::new();

fn table() -> &'static Vec<bool> {
    TABLE.get_or_init(build_table)
}

/// Exact outcome of the King+Pawn vs King endgame where `pawn_color` owns
/// the only pawn on the board. `strong_king`/`weak_king` are that pawn's
/// side and the bare side's kings respectively (not White's/Black's).
///
/// Internally canonicalizes to "White's pawn, advancing toward rank 8": if
/// `pawn_color` is Black the whole position is mirrored vertically first
/// (`sq.0 ^ 56`, the same transform `eval.rs` already uses for piece-square
/// tables), and if the pawn ends up on a file e-h it is then mirrored
/// horizontally too, since a pawn ending on file c is a mirror image of one
/// on file f. Neither transform changes the outcome, only which of the
/// (otherwise identical) table entries answers the query.
pub fn probe(pawn_color: Color, strong_king: Square, weak_king: Square, pawn: Square, side_to_move: Color) -> Outcome {
    let flip_v = |sq: Square| if pawn_color == Color::Black { Square(sq.0 ^ 56) } else { sq };
    let mut wk = flip_v(strong_king);
    let mut bk = flip_v(weak_king);
    let mut pawn = flip_v(pawn);

    if pawn.file() >= 4 {
        let flip_h = |sq: Square| Square::new(7 - sq.file(), sq.rank());
        wk = flip_h(wk);
        bk = flip_h(bk);
        pawn = flip_h(pawn);
    }

    let strong_to_move = side_to_move == pawn_color;
    if table()[state_index(wk, bk, pawn, strong_to_move)] {
        Outcome::Win
    } else {
        Outcome::Draw
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn free_uncontested_pawn_one_step_from_promotion_is_won() {
        let outcome = probe(Color::White, Square::new(4, 1), Square::new(0, 0), Square::new(4, 6), Color::White);
        assert_eq!(outcome, Outcome::Win);
    }

    #[test]
    fn weak_king_too_far_to_catch_a_running_passer_is_won() {
        // Pawn on a4 needs at most 4 pushes to queen; the defending king on
        // h8 is 7 files away from the queening square, hopelessly outside
        // the square of the pawn no matter whose move it is.
        let outcome = probe(Color::White, Square::new(4, 3), Square::new(7, 7), Square::new(0, 3), Color::Black);
        assert_eq!(outcome, Outcome::Win);
    }

    #[test]
    fn weak_king_captures_an_undefended_pawn_for_a_draw() {
        let outcome = probe(Color::White, Square::new(0, 0), Square::new(4, 4), Square::new(4, 3), Color::Black);
        assert_eq!(outcome, Outcome::Draw);
    }

    #[test]
    fn king_and_pawn_stalemate_in_the_corner_is_a_draw() {
        // The textbook trap: White Ka6/Pa7, Black Ka8 to move. Every
        // king move is either occupied-and-defended (a7), adjacent to the
        // white king (b7), or attacked by the pawn (b8): stalemate.
        let outcome = probe(Color::White, Square::new(0, 5), Square::new(0, 7), Square::new(0, 6), Color::Black);
        assert_eq!(outcome, Outcome::Draw);
    }

    #[test]
    fn mirroring_the_pawns_color_gives_the_same_result() {
        let white_pawn = probe(Color::White, Square::new(4, 1), Square::new(0, 0), Square::new(4, 6), Color::White);
        let black_pawn = probe(Color::Black, Square::new(4, 6), Square::new(0, 7), Square::new(4, 1), Color::Black);
        assert_eq!(white_pawn, black_pawn);
    }

    #[test]
    fn mirroring_the_board_horizontally_gives_the_same_result() {
        let original = probe(Color::White, Square::new(4, 3), Square::new(7, 7), Square::new(0, 3), Color::Black);
        let mirrored = probe(Color::White, Square::new(3, 3), Square::new(0, 7), Square::new(7, 3), Color::Black);
        assert_eq!(original, mirrored);
    }
}
