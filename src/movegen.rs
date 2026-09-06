use crate::bitboard::Bitboard;
use crate::board::Board;
use crate::eval;
use crate::magic;
use crate::types::{CastlingRights, Color, Move, MoveFlag, PieceType, Square};

// ---------------------------------------------------------------------
// Precomputed attack tables, built once at compile time.
// ---------------------------------------------------------------------

const fn knight_attacks_from(sq: u8) -> u64 {
    const DELTAS: [(i32, i32); 8] = [
        (1, 2),
        (2, 1),
        (2, -1),
        (1, -2),
        (-1, -2),
        (-2, -1),
        (-2, 1),
        (-1, 2),
    ];
    let file = (sq % 8) as i32;
    let rank = (sq / 8) as i32;
    let mut bb: u64 = 0;
    let mut i = 0;
    while i < 8 {
        let (df, dr) = DELTAS[i];
        let f = file + df;
        let r = rank + dr;
        if f >= 0 && f < 8 && r >= 0 && r < 8 {
            bb |= 1u64 << (r * 8 + f);
        }
        i += 1;
    }
    bb
}

const fn king_attacks_from(sq: u8) -> u64 {
    const DELTAS: [(i32, i32); 8] = [
        (-1, -1),
        (-1, 0),
        (-1, 1),
        (0, -1),
        (0, 1),
        (1, -1),
        (1, 0),
        (1, 1),
    ];
    let file = (sq % 8) as i32;
    let rank = (sq / 8) as i32;
    let mut bb: u64 = 0;
    let mut i = 0;
    while i < 8 {
        let (df, dr) = DELTAS[i];
        let f = file + df;
        let r = rank + dr;
        if f >= 0 && f < 8 && r >= 0 && r < 8 {
            bb |= 1u64 << (r * 8 + f);
        }
        i += 1;
    }
    bb
}

const fn pawn_attacks_from(sq: u8, white: bool) -> u64 {
    let dr: i32 = if white { 1 } else { -1 };
    let file = (sq % 8) as i32;
    let rank = (sq / 8) as i32;
    let mut bb: u64 = 0;
    let mut i = 0;
    const FILE_DELTAS: [i32; 2] = [-1, 1];
    while i < 2 {
        let f = file + FILE_DELTAS[i];
        let r = rank + dr;
        if f >= 0 && f < 8 && r >= 0 && r < 8 {
            bb |= 1u64 << (r * 8 + f);
        }
        i += 1;
    }
    bb
}

const fn build_knight_table() -> [u64; 64] {
    let mut table = [0u64; 64];
    let mut sq = 0;
    while sq < 64 {
        table[sq] = knight_attacks_from(sq as u8);
        sq += 1;
    }
    table
}

const fn build_king_table() -> [u64; 64] {
    let mut table = [0u64; 64];
    let mut sq = 0;
    while sq < 64 {
        table[sq] = king_attacks_from(sq as u8);
        sq += 1;
    }
    table
}

const KNIGHT_ATTACKS: [u64; 64] = build_knight_table();
const KING_ATTACKS: [u64; 64] = build_king_table();

const fn build_pawn_table(white: bool) -> [u64; 64] {
    let mut table = [0u64; 64];
    let mut sq = 0;
    while sq < 64 {
        table[sq] = pawn_attacks_from(sq as u8, white);
        sq += 1;
    }
    table
}

const WHITE_PAWN_ATTACKS: [u64; 64] = build_pawn_table(true);
const BLACK_PAWN_ATTACKS: [u64; 64] = build_pawn_table(false);

// ---------------------------------------------------------------------
// Sliding attack generation.
//
// Delegated to `magic`, which answers with one multiply, one shift and one
// lookup. The classical ray walk this replaced is still here, as
// `classical_reference` below, and the tests check the two against each
// other over every occupancy that can reach the tables: the point of magic
// bitboards is that they change the cost of the answer and not the answer.
// ---------------------------------------------------------------------

#[inline(always)]
pub fn bishop_attacks(sq: Square, occupied: Bitboard) -> Bitboard {
    magic::bishop_attacks(sq, occupied)
}

#[inline(always)]
pub fn rook_attacks(sq: Square, occupied: Bitboard) -> Bitboard {
    magic::rook_attacks(sq, occupied)
}

#[inline(always)]
pub fn queen_attacks(sq: Square, occupied: Bitboard) -> Bitboard {
    magic::bishop_attacks(sq, occupied) | magic::rook_attacks(sq, occupied)
}

pub fn knight_attacks(sq: Square) -> Bitboard {
    Bitboard(KNIGHT_ATTACKS[sq.0 as usize])
}

pub fn king_attacks(sq: Square) -> Bitboard {
    Bitboard(KING_ATTACKS[sq.0 as usize])
}

// ---------------------------------------------------------------------
// Ray geometry: which squares lie between two others, and which whole line
// they share. Both tables exist so that legality can be answered by
// arithmetic instead of by playing the move -- see `check_info`.
// ---------------------------------------------------------------------

/// The eight ray directions as `(file, rank)` deltas.
const RAY_DIRS: [(i32, i32); 8] = [(0, 1), (0, -1), (1, 0), (-1, 0), (1, 1), (1, -1), (-1, 1), (-1, -1)];

/// Squares strictly between `a` and `b` when the two share a rank, file or
/// diagonal; empty otherwise, and empty for adjacent squares.
const fn between_from(a: u8, b: u8) -> u64 {
    if a == b {
        return 0;
    }
    let file = (a % 8) as i32;
    let rank = (a / 8) as i32;
    let mut i = 0;
    while i < 8 {
        let (df, dr) = RAY_DIRS[i];
        let mut f = file + df;
        let mut r = rank + dr;
        let mut walked: u64 = 0;
        while f >= 0 && f < 8 && r >= 0 && r < 8 {
            let sq = (r * 8 + f) as u32;
            if sq as u8 == b {
                return walked;
            }
            walked |= 1u64 << sq;
            f += df;
            r += dr;
        }
        i += 1;
    }
    0
}

/// Every square of the line through `a` and `b`, `a` itself included, when
/// the two share one; empty otherwise. A pinned piece may move anywhere on
/// this line and nowhere else: its own king closes one end and the pinner
/// the other, so the squares past either of them are unreachable anyway.
const fn line_from(a: u8, b: u8) -> u64 {
    if a == b {
        return 0;
    }
    let file = (a % 8) as i32;
    let rank = (a / 8) as i32;
    let mut i = 0;
    while i < 8 {
        let (df, dr) = RAY_DIRS[i];
        let mut f = file + df;
        let mut r = rank + dr;
        let mut found = false;
        while f >= 0 && f < 8 && r >= 0 && r < 8 {
            if (r * 8 + f) as u8 == b {
                found = true;
                break;
            }
            f += df;
            r += dr;
        }
        if found {
            let mut bb = 1u64 << a;
            let mut f = file + df;
            let mut r = rank + dr;
            while f >= 0 && f < 8 && r >= 0 && r < 8 {
                bb |= 1u64 << (r * 8 + f) as u32;
                f += df;
                r += dr;
            }
            let mut f = file - df;
            let mut r = rank - dr;
            while f >= 0 && f < 8 && r >= 0 && r < 8 {
                bb |= 1u64 << (r * 8 + f) as u32;
                f -= df;
                r -= dr;
            }
            return bb;
        }
        i += 1;
    }
    0
}

const fn build_between() -> [[u64; 64]; 64] {
    let mut table = [[0u64; 64]; 64];
    let mut a = 0usize;
    while a < 64 {
        let mut b = 0usize;
        while b < 64 {
            table[a][b] = between_from(a as u8, b as u8);
            b += 1;
        }
        a += 1;
    }
    table
}

const fn build_line() -> [[u64; 64]; 64] {
    let mut table = [[0u64; 64]; 64];
    let mut a = 0usize;
    while a < 64 {
        let mut b = 0usize;
        while b < 64 {
            table[a][b] = line_from(a as u8, b as u8);
            b += 1;
        }
        a += 1;
    }
    table
}

// 32 KB each, built at compile time and read in place. `static` rather than
// `const` on purpose: a `const` of this size is a value, not a place, and
// every indexing could materialise a fresh copy of the whole table.
static BETWEEN: [[u64; 64]; 64] = build_between();
static LINE: [[u64; 64]; 64] = build_line();

#[inline(always)]
fn between(a: Square, b: Square) -> Bitboard {
    Bitboard(BETWEEN[a.0 as usize][b.0 as usize])
}

#[inline(always)]
fn line(a: Square, b: Square) -> Bitboard {
    Bitboard(LINE[a.0 as usize][b.0 as usize])
}

// ---------------------------------------------------------------------
// Attack detection.
// ---------------------------------------------------------------------

/// Is `sq` attacked by any piece of `by_color`, judged against an occupancy
/// that need not be the board's own?
///
/// The parameter is there for king moves in the legality filter: a king has
/// to be tested on its destination with *itself* taken off the board, or it
/// would block the very ray it is running along and a retreat straight back
/// from a checking rook would come out looking safe.
fn attacked_with_occ(board: &Board, sq: Square, by_color: Color, occupied: Bitboard) -> bool {
    if !(Bitboard(KNIGHT_ATTACKS[sq.0 as usize]) & board.pieces_of(by_color, PieceType::Knight)).is_empty() {
        return true;
    }
    if !(Bitboard(KING_ATTACKS[sq.0 as usize]) & board.pieces_of(by_color, PieceType::King)).is_empty() {
        return true;
    }
    // Squares from which a `by_color` pawn would attack `sq` are exactly the
    // attack pattern of the opposite-color pawn standing on `sq`.
    let pawn_table = match by_color {
        Color::White => &BLACK_PAWN_ATTACKS,
        Color::Black => &WHITE_PAWN_ATTACKS,
    };
    if !(Bitboard(pawn_table[sq.0 as usize]) & board.pieces_of(by_color, PieceType::Pawn)).is_empty() {
        return true;
    }

    let diagonal_attackers = board.pieces_of(by_color, PieceType::Bishop) | board.pieces_of(by_color, PieceType::Queen);
    if !(bishop_attacks(sq, occupied) & diagonal_attackers).is_empty() {
        return true;
    }
    let orthogonal_attackers = board.pieces_of(by_color, PieceType::Rook) | board.pieces_of(by_color, PieceType::Queen);
    if !(rook_attacks(sq, occupied) & orthogonal_attackers).is_empty() {
        return true;
    }
    false
}

/// Is `sq` attacked by any piece of `by_color` in the current position?
pub fn is_square_attacked(board: &Board, sq: Square, by_color: Color) -> bool {
    attacked_with_occ(board, sq, by_color, board.occupied())
}

/// Whether `color`'s king is under attack. A position with no such king is
/// impossible through the public API (`Board::from_fen` requires exactly one
/// king per side and rejects a side-to-move that could capture the other
/// king), so this answers `false` rather than panicking: a search thread
/// dying on a hand-built position is a worse failure mode than one extra
/// branch on a path that never fires in a real game.
pub fn is_in_check(board: &Board, color: Color) -> bool {
    match board.pieces_of(color, PieceType::King).lsb() {
        Some(king_sq) => is_square_attacked(board, king_sq, color.opposite()),
        None => false,
    }
}

/// Does `mv`, played by the side to move, leave the *opponent's* king in
/// check? Answers exactly what `make_move` + `is_in_check` + `unmake_move`
/// would, without touching the board — which is what makes it affordable in
/// the pruning decisions that happen before a move is played at all
/// (`negamax` must not discard a checking move as "just another quiet move"
/// past the late-move-pruning threshold, and paying a make/unmake per
/// candidate to find out would defeat the point of pruning).
///
/// Handles the three moves that touch more squares than `from`/`to`: en
/// passant vacates the captured pawn's square as well, castling relocates a
/// rook that may itself deliver the check, and a promotion checks with the
/// piece it becomes rather than with a pawn. `gives_check_matches_make_move`
/// pins all of that against the make/unmake answer over whole move trees.
pub fn gives_check(board: &Board, mv: Move) -> bool {
    let us = board.side_to_move;
    let Some(king_sq) = board.pieces_of(us.opposite(), PieceType::King).lsb() else {
        return false;
    };
    let Some(moving) = board.piece_at(mv.from) else {
        return false;
    };

    // Occupancy as it will be after the move.
    let mut occ = board.occupied();
    let mut vacated = Bitboard::from_square(mv.from);
    occ.clear(mv.from);
    occ.set(mv.to);
    if mv.flag == MoveFlag::EnPassant {
        occ.clear(Square::new(mv.to.file(), mv.from.rank()));
    }
    let mut castled_rook = None;
    if mv.flag.is_castle() {
        let (rook_from, rook_to) = Board::castle_rook_squares(us, mv.flag);
        occ.clear(rook_from);
        occ.set(rook_to);
        vacated = vacated | Bitboard::from_square(rook_from);
        castled_rook = Some(rook_to);
    }

    // Direct check by whatever now stands on the destination square.
    let placed = mv.flag.promotion_piece().unwrap_or(moving.kind);
    let direct = match placed {
        PieceType::Pawn => {
            let table = match us {
                Color::White => &WHITE_PAWN_ATTACKS,
                Color::Black => &BLACK_PAWN_ATTACKS,
            };
            Bitboard(table[mv.to.0 as usize]).contains(king_sq)
        }
        PieceType::Knight => knight_attacks(mv.to).contains(king_sq),
        PieceType::Bishop => bishop_attacks(mv.to, occ).contains(king_sq),
        PieceType::Rook => rook_attacks(mv.to, occ).contains(king_sq),
        PieceType::Queen => queen_attacks(mv.to, occ).contains(king_sq),
        PieceType::King => false, // a king can never attack the other king
    };
    if direct {
        return true;
    }
    if let Some(rook_to) = castled_rook {
        if rook_attacks(rook_to, occ).contains(king_sq) {
            return true;
        }
    }

    // Discovered check: any of our sliders that still stands where it was
    // (hence `& !vacated`) and now sees the king through the new occupancy.
    let diagonal = (board.pieces_of(us, PieceType::Bishop) | board.pieces_of(us, PieceType::Queen)) & !vacated;
    if !(bishop_attacks(king_sq, occ) & diagonal).is_empty() {
        return true;
    }
    let orthogonal = (board.pieces_of(us, PieceType::Rook) | board.pieces_of(us, PieceType::Queen)) & !vacated;
    if !(rook_attacks(king_sq, occ) & orthogonal).is_empty() {
        return true;
    }
    false
}

// ---------------------------------------------------------------------
// Pseudo-legal move generation.
// ---------------------------------------------------------------------

fn push_pawn_move(from: Square, to: Square, is_promotion: bool, is_capture: bool, moves: &mut Vec<Move>) {
    if is_promotion {
        let flags = if is_capture {
            [
                MoveFlag::PromoCaptureKnight,
                MoveFlag::PromoCaptureBishop,
                MoveFlag::PromoCaptureRook,
                MoveFlag::PromoCaptureQueen,
            ]
        } else {
            [
                MoveFlag::PromoKnight,
                MoveFlag::PromoBishop,
                MoveFlag::PromoRook,
                MoveFlag::PromoQueen,
            ]
        };
        for flag in flags {
            moves.push(Move::new(from, to, flag));
        }
    } else {
        let flag = if is_capture { MoveFlag::Capture } else { MoveFlag::Quiet };
        moves.push(Move::new(from, to, flag));
    }
}

fn generate_pawn_moves(board: &Board, color: Color, occ: Bitboard, opp: Bitboard, moves: &mut Vec<Move>) {
    let (dir, start_rank, promo_rank): (i8, u8, u8) = match color {
        Color::White => (1, 1, 7),
        Color::Black => (-1, 6, 0),
    };
    let pawn_attack_table = match color {
        Color::White => &WHITE_PAWN_ATTACKS,
        Color::Black => &BLACK_PAWN_ATTACKS,
    };

    for from in board.pieces_of(color, PieceType::Pawn) {
        let to_rank = from.rank() as i8 + dir;
        let single_to = Square::new(from.file(), to_rank as u8);

        if !occ.contains(single_to) {
            push_pawn_move(from, single_to, to_rank as u8 == promo_rank, false, moves);

            if from.rank() == start_rank {
                let double_to = Square::new(from.file(), (from.rank() as i8 + 2 * dir) as u8);
                if !occ.contains(double_to) {
                    moves.push(Move::new(from, double_to, MoveFlag::DoublePawnPush));
                }
            }
        }

        for to in Bitboard(pawn_attack_table[from.0 as usize]) {
            if opp.contains(to) {
                push_pawn_move(from, to, to.rank() == promo_rank, true, moves);
            } else if Some(to) == board.en_passant {
                moves.push(Move::new(from, to, MoveFlag::EnPassant));
            }
        }
    }
}

fn generate_knight_moves(board: &Board, color: Color, own: Bitboard, occ: Bitboard, moves: &mut Vec<Move>) {
    for from in board.pieces_of(color, PieceType::Knight) {
        for to in Bitboard(KNIGHT_ATTACKS[from.0 as usize]) & !own {
            let flag = if occ.contains(to) { MoveFlag::Capture } else { MoveFlag::Quiet };
            moves.push(Move::new(from, to, flag));
        }
    }
}

fn generate_king_moves(board: &Board, color: Color, own: Bitboard, occ: Bitboard, moves: &mut Vec<Move>) {
    for from in board.pieces_of(color, PieceType::King) {
        for to in Bitboard(KING_ATTACKS[from.0 as usize]) & !own {
            let flag = if occ.contains(to) { MoveFlag::Capture } else { MoveFlag::Quiet };
            moves.push(Move::new(from, to, flag));
        }
    }
}

fn generate_sliding_moves(
    board: &Board,
    color: Color,
    own: Bitboard,
    occ: Bitboard,
    kind: PieceType,
    moves: &mut Vec<Move>,
) {
    let attacks_fn: fn(Square, Bitboard) -> Bitboard = match kind {
        PieceType::Bishop => bishop_attacks,
        PieceType::Rook => rook_attacks,
        PieceType::Queen => queen_attacks,
        _ => unreachable!("generate_sliding_moves solo admite piezas deslizantes"),
    };
    for from in board.pieces_of(color, kind) {
        for to in attacks_fn(from, occ) & !own {
            let flag = if occ.contains(to) { MoveFlag::Capture } else { MoveFlag::Quiet };
            moves.push(Move::new(from, to, flag));
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn try_add_castle(
    board: &Board,
    moves: &mut Vec<Move>,
    right: u8,
    king_from: Square,
    king_to: Square,
    must_be_empty: &[Square],
    must_not_be_attacked: &[Square],
    flag: MoveFlag,
    opponent: Color,
) {
    if !board.castling.has(right) {
        return;
    }
    let occ = board.occupied();
    if must_be_empty.iter().any(|&sq| occ.contains(sq)) {
        return;
    }
    if must_not_be_attacked
        .iter()
        .any(|&sq| is_square_attacked(board, sq, opponent))
    {
        return;
    }
    moves.push(Move::new(king_from, king_to, flag));
}

fn generate_castling(board: &Board, moves: &mut Vec<Move>) {
    let color = board.side_to_move;
    let opponent = color.opposite();
    let rank = match color {
        Color::White => 0,
        Color::Black => 7,
    };
    let king_from = Square::new(4, rank);
    let (kingside_right, queenside_right) = match color {
        Color::White => (CastlingRights::WHITE_KINGSIDE, CastlingRights::WHITE_QUEENSIDE),
        Color::Black => (CastlingRights::BLACK_KINGSIDE, CastlingRights::BLACK_QUEENSIDE),
    };

    try_add_castle(
        board,
        moves,
        kingside_right,
        king_from,
        Square::new(6, rank),
        &[Square::new(5, rank), Square::new(6, rank)],
        &[Square::new(4, rank), Square::new(5, rank), Square::new(6, rank)],
        MoveFlag::KingCastle,
        opponent,
    );
    try_add_castle(
        board,
        moves,
        queenside_right,
        king_from,
        Square::new(2, rank),
        &[Square::new(1, rank), Square::new(2, rank), Square::new(3, rank)],
        &[Square::new(4, rank), Square::new(3, rank), Square::new(2, rank)],
        MoveFlag::QueenCastle,
        opponent,
    );
}

pub fn generate_pseudo_legal_moves(board: &Board) -> Vec<Move> {
    let mut moves = Vec::with_capacity(48);
    let color = board.side_to_move;
    let own = board.color_occupied(color);
    let opp = board.color_occupied(color.opposite());
    let occ = own | opp;

    generate_pawn_moves(board, color, occ, opp, &mut moves);
    generate_knight_moves(board, color, own, occ, &mut moves);
    generate_sliding_moves(board, color, own, occ, PieceType::Bishop, &mut moves);
    generate_sliding_moves(board, color, own, occ, PieceType::Rook, &mut moves);
    generate_sliding_moves(board, color, own, occ, PieceType::Queen, &mut moves);
    generate_king_moves(board, color, own, occ, &mut moves);
    generate_castling(board, &mut moves);

    moves
}

/// Generates only fully legal moves: pseudo-legal moves that do not leave
/// the mover's own king in check. Castling moves are already fully vetted
/// by `generate_castling`, so the filter lets them straight through.
///
/// Clones `board` into a scratch copy because en passant -- and only en
/// passant -- is still decided by playing it. `legal_moves_scratch` below
/// is the same function without the clone, for the hot path, which already
/// holds a `&mut Board` of its own.
pub fn generate_legal_moves(board: &Board) -> Vec<Move> {
    let mut working = board.clone();
    legal_moves_scratch(&mut working)
}

/// King square, checking pieces and absolutely pinned pieces of the side to
/// move: everything a legality test needs, computed once per node instead of
/// once per candidate move.
#[derive(Clone, Copy)]
struct CheckInfo {
    king_sq: Square,
    checkers: Bitboard,
    pinned: Bitboard,
}

/// Builds the `CheckInfo` of `us`, or `None` when `us` has no king on the
/// board. That position is unreachable through `Board::from_fen` (which
/// demands exactly one king per side), and the `None` exists for the same
/// reason `is_in_check` answers `false` there: a search thread dying on a
/// hand-built position is a worse failure mode than one extra branch.
fn check_info(board: &Board, us: Color) -> Option<CheckInfo> {
    let king_sq = board.pieces_of(us, PieceType::King).lsb()?;
    let them = us.opposite();
    let occupied = board.occupied();

    let pawn_table = match them {
        Color::White => &BLACK_PAWN_ATTACKS,
        Color::Black => &WHITE_PAWN_ATTACKS,
    };
    let diagonal = board.pieces_of(them, PieceType::Bishop) | board.pieces_of(them, PieceType::Queen);
    let orthogonal = board.pieces_of(them, PieceType::Rook) | board.pieces_of(them, PieceType::Queen);

    // No king term: a king can never attack the other one, `from_fen`
    // rejects adjacent kings, and `make_move` never produces them.
    let checkers = (Bitboard(KNIGHT_ATTACKS[king_sq.0 as usize]) & board.pieces_of(them, PieceType::Knight))
        | (Bitboard(pawn_table[king_sq.0 as usize]) & board.pieces_of(them, PieceType::Pawn))
        | (bishop_attacks(king_sq, occupied) & diagonal)
        | (rook_attacks(king_sq, occupied) & orthogonal);

    // Snipers are the enemy sliders that would see the king on an empty
    // board. Exactly one piece standing between a sniper and the king means
    // that piece is pinned -- and only ours can be, since moving theirs is
    // not a move we get to make.
    let ours = board.color_occupied(us);
    let snipers =
        (bishop_attacks(king_sq, Bitboard::EMPTY) & diagonal) | (rook_attacks(king_sq, Bitboard::EMPTY) & orthogonal);
    let mut pinned = Bitboard::EMPTY;
    for sniper in snipers {
        let blockers = between(king_sq, sniper) & occupied;
        if blockers.count() == 1 {
            pinned = pinned | (blockers & ours);
        }
    }

    Some(CheckInfo { king_sq, checkers, pinned })
}

/// Is `mv` legal, answered from `info` without touching the board?
///
/// Equivalent to make/unmake plus `is_in_check` for every move except en
/// passant, which the caller keeps sending down the old path.
fn legal_by_pins(board: &Board, mv: Move, info: &CheckInfo, us: Color) -> bool {
    if mv.from == info.king_sq {
        // Castling already passed `try_add_castle`, which tests the origin
        // square along with the two the king crosses. Emptying e1/h1/a1
        // cannot uncover a ray onto f1/g1/c1/d1 that did not already pass
        // through e1, and e1 is one of the squares tested.
        if mv.flag.is_castle() {
            return true;
        }
        let mut occupied = board.occupied();
        occupied.clear(info.king_sq);
        // The piece being captured, if any, stays in the occupancy: it sits
        // on the destination square itself, where it blocks nothing that
        // reaches that square, and it cannot be attacking the square it
        // stands on.
        return !attacked_with_occ(board, mv.to, us.opposite(), occupied);
    }

    // Double check: no interposition and no capture answers both attacks at
    // once, so only the king may move.
    if info.checkers.count() > 1 {
        return false;
    }

    // A pinned piece may travel along the line it is pinned on and nowhere
    // else. That covers capturing the pinner and shuffling towards the king.
    if info.pinned.contains(mv.from) && !line(info.king_sq, mv.from).contains(mv.to) {
        return false;
    }

    // Single check: capture the checker or step into the line. `BETWEEN` is
    // empty for a checking pawn (adjacent) and for a checking knight (a
    // knight is never aligned with a square it attacks), so for those two
    // the mask collapses to "capture it", which is exactly right.
    if let Some(checker) = info.checkers.lsb() {
        if !(between(info.king_sq, checker) | Bitboard::from_square(checker)).contains(mv.to) {
            return false;
        }
    }

    true
}

/// Same as `generate_legal_moves`, but filters in place on the caller's own
/// board instead of on an internal clone. `working` comes back exactly as it
/// went in, so this is transparent to the caller -- it just avoids a clone
/// per call in `negamax`/`quiescence`/`search_root`, which already hold a
/// `&mut Board` anyway.
///
/// Until 0.28 this played and took back every pseudo-legal move just to ask
/// whether it left the king in check. Measured on the twelve positions of
/// `banco velocidad`, that filter was 27.9 % of the time spent per node --
/// paid in full even in quiescence, which builds the whole legal list before
/// the `stand_pat >= beta` cutoff that is its most frequent exit.
pub(crate) fn legal_moves_scratch(working: &mut Board) -> Vec<Move> {
    let us = working.side_to_move;
    let mut moves = generate_pseudo_legal_moves(working);
    let Some(info) = check_info(working, us) else {
        // No king of ours: `is_in_check` answers `false` for such a
        // position, so the filter this replaced kept every move. Keep that.
        return moves;
    };
    // `retain` in place rather than `filter().collect()`: the latter built a
    // second `Vec` per node on top of the generator's own, and this is the
    // single most frequently called allocation site in the engine.
    moves.retain(|&mv| {
        if mv.flag == MoveFlag::EnPassant {
            // The one move that empties a square it never touches. It can
            // uncover the king along a rank through the captured pawn and
            // the capturing one at once, which no mask taken before the
            // move can express, so it keeps paying make/unmake. It is well
            // under 1 % of the moves generated: measured by doubling the
            // filter for king and en passant moves alone, the two together
            // are ~2 % of the time per node.
            let undo = working.make_move(mv);
            let legal = !is_in_check(working, us);
            working.unmake_move(mv, undo);
            legal
        } else {
            legal_by_pins(working, mv, &info, us)
        }
    });
    moves
}

// ---------------------------------------------------------------------
// Static exchange evaluation (SEE): "if I capture here, and both sides
// keep recapturing with their least valuable attacker, who comes out
// ahead?" Used by search to skip clearly-losing captures instead of
// wasting time reading them out, and to rank captures more accurately
// than MVV-LVA alone (which doesn't know a capture is defended).
// ---------------------------------------------------------------------

/// All pieces of either color attacking `sq` against a possibly
/// hypothetical `occupied` bitboard, restricted to still-present pieces.
/// Sliding attacks are recomputed against `occupied` so that removing a
/// piece during a simulated exchange correctly reveals x-ray attackers
/// behind it.
fn attackers_to(board: &Board, sq: Square, occupied: Bitboard) -> Bitboard {
    let knights = board.pieces_of(Color::White, PieceType::Knight) | board.pieces_of(Color::Black, PieceType::Knight);
    let kings = board.pieces_of(Color::White, PieceType::King) | board.pieces_of(Color::Black, PieceType::King);
    let mut attackers = Bitboard(KNIGHT_ATTACKS[sq.0 as usize]) & knights;
    attackers = attackers | (Bitboard(KING_ATTACKS[sq.0 as usize]) & kings);
    // A square is attacked by a color's pawn from exactly the squares the
    // opposite-color pawn attack table would list for `sq`.
    attackers = attackers | (Bitboard(BLACK_PAWN_ATTACKS[sq.0 as usize]) & board.pieces_of(Color::White, PieceType::Pawn));
    attackers = attackers | (Bitboard(WHITE_PAWN_ATTACKS[sq.0 as usize]) & board.pieces_of(Color::Black, PieceType::Pawn));

    let diagonal_sliders = board.pieces_of(Color::White, PieceType::Bishop)
        | board.pieces_of(Color::Black, PieceType::Bishop)
        | board.pieces_of(Color::White, PieceType::Queen)
        | board.pieces_of(Color::Black, PieceType::Queen);
    attackers = attackers | (bishop_attacks(sq, occupied) & diagonal_sliders);

    let orthogonal_sliders = board.pieces_of(Color::White, PieceType::Rook)
        | board.pieces_of(Color::Black, PieceType::Rook)
        | board.pieces_of(Color::White, PieceType::Queen)
        | board.pieces_of(Color::Black, PieceType::Queen);
    attackers = attackers | (rook_attacks(sq, occupied) & orthogonal_sliders);

    attackers & occupied
}

fn least_valuable_attacker(board: &Board, color: Color, attackers: Bitboard) -> Option<(Square, PieceType)> {
    for kind in PieceType::ALL {
        if let Some(sq) = (attackers & board.pieces_of(color, kind)).lsb() {
            return Some((sq, kind));
        }
    }
    None
}

/// Longest exchange this can model: 32 pieces on the board is an absolute
/// ceiling on how many captures can happen on one square, and in practice a
/// swap sequence past a handful of plies is already vanishingly rare.
const SEE_MAX_EXCHANGES: usize = 32;

/// Value a pawn gains by reaching the last rank during the swap sequence.
/// The recapture chain always promotes to a queen: SEE is a bound on how
/// the exchange can go, and no defender ever benefits from assuming its
/// opponent underpromotes.
fn see_promotion_gain() -> i32 {
    eval::piece_value(PieceType::Queen) - eval::piece_value(PieceType::Pawn)
}

fn is_promotion_rank(sq: Square, color: Color) -> bool {
    match color {
        Color::White => sq.rank() == 7,
        Color::Black => sq.rank() == 0,
    }
}

/// Net material change (in centipawns, from the mover's perspective) of
/// playing capture `mv` and then letting both sides recapture on that
/// square with their least valuable attacker, for as long as it's
/// profitable to keep capturing. Intended for capture moves only; legality
/// of intermediate recaptures (e.g. a pinned piece "recapturing") is not
/// checked, which is the standard, well-tested simplification every engine
/// using this classic swap algorithm makes.
pub fn static_exchange_eval(board: &Board, mv: Move) -> i32 {
    let to = mv.to;
    let mover_color = board.side_to_move;

    let mut occupied = board.occupied();
    occupied.clear(mv.from); // the moving piece vacates its origin square

    let captured_value = if mv.flag == MoveFlag::EnPassant {
        let captured_sq = Square::new(to.file(), mv.from.rank());
        occupied.clear(captured_sq);
        eval::piece_value(PieceType::Pawn)
    } else {
        board.piece_at(to).map(|p| eval::piece_value(p.kind)).unwrap_or(0)
    };

    // A promotion changes the net material swing of this move by the gap
    // between the promoted piece and the pawn that vacated `from`, and it
    // also means the piece now sitting on `to` (what the first recapture,
    // if any, would actually win) is the promoted piece, not a pawn.
    let promotion_gain = mv
        .promotion()
        .map(|kind| eval::piece_value(kind) - eval::piece_value(PieceType::Pawn))
        .unwrap_or(0);
    let mut attacker_value = match mv.promotion() {
        Some(kind) => eval::piece_value(kind),
        None => board.piece_at(mv.from).map(|p| eval::piece_value(p.kind)).unwrap_or(0),
    };

    // Fixed-size stack array rather than a `Vec`: SEE runs for every capture
    // during move ordering *and* again in the quiescence filter, so a heap
    // allocation here was one malloc/free per capture per node.
    let mut gains = [0i32; SEE_MAX_EXCHANGES];
    gains[0] = captured_value + promotion_gain;
    let mut gains_len = 1usize;
    let mut side = mover_color.opposite();

    while gains_len < SEE_MAX_EXCHANGES {
        let attackers = attackers_to(board, to, occupied) & board.color_occupied(side) & occupied;
        let Some((attacker_sq, kind)) = least_valuable_attacker(board, side, attackers) else {
            break;
        };
        // A king can never itself be recaptured, so it may only join the
        // exchange when the opponent has no attacker left bearing on the
        // square once the king steps in (x-rays through the king's own
        // origin square included): otherwise the "recapture" would be
        // moving into check, which the real move generator never allows —
        // and since piece_value(King) == 0, letting the swap continue past
        // it would score the follow-up "capture of the king" as a free,
        // harmless trade instead of an illegal line.
        if kind == PieceType::King {
            let mut occupied_after = occupied;
            occupied_after.clear(attacker_sq);
            let opponent_attackers =
                attackers_to(board, to, occupied_after) & board.color_occupied(side.opposite());
            if !opponent_attackers.is_empty() {
                break;
            }
        }
        // A pawn recapturing onto the last rank promotes, which the swap
        // used to ignore entirely: only the *initial* move's promotion was
        // priced in. That made `1R2k3/P7/8/8/1r6/8/8/4K3 b - - 0 1` score
        // ...Rxb8 as an even trade when axb8=Q answers it, roughly 800 cp
        // the other way.
        let mut gain = attacker_value - gains[gains_len - 1];
        let mut next_attacker_value = eval::piece_value(kind);
        if kind == PieceType::Pawn && is_promotion_rank(to, side) {
            let promo = see_promotion_gain();
            gain += promo;
            next_attacker_value += promo;
        }
        gains[gains_len] = gain;
        gains_len += 1;
        occupied.clear(attacker_sq);
        attacker_value = next_attacker_value;
        side = side.opposite();
    }

    for i in (0..gains_len - 1).rev() {
        gains[i] = -i32::max(-gains[i], gains[i + 1]);
    }
    gains[0]
}

// ---------------------------------------------------------------------
// Perft: exhaustive leaf-node count, used to validate the move generator
// against known reference values.
// ---------------------------------------------------------------------

pub fn perft(board: &mut Board, depth: u32) -> u64 {
    if depth == 0 {
        return 1;
    }
    let moves = legal_moves_scratch(board);
    if depth == 1 {
        return moves.len() as u64;
    }
    let mut nodes = 0;
    for mv in moves {
        let undo = board.make_move(mv);
        nodes += perft(board, depth - 1);
        board.unmake_move(mv, undo);
    }
    nodes
}

/// The ray-walking slider implementation that `magic` replaced, kept as an
/// independent oracle. It arrives at the attack set a different way — one
/// precomputed ray per direction, then the first blocker on it found with
/// `lsb`/`msb` and its own ray XORed away — so agreeing with the magic
/// tables is real evidence about both, not a tautology. Test-only: nothing
/// in the engine calls it any more.
#[cfg(test)]
mod classical_reference {
    use crate::bitboard::Bitboard;
    use crate::types::Square;

    const fn ray_from(sq: u8, df: i32, dr: i32) -> u64 {
        let mut file = (sq % 8) as i32 + df;
        let mut rank = (sq / 8) as i32 + dr;
        let mut bb: u64 = 0;
        while file >= 0 && file < 8 && rank >= 0 && rank < 8 {
            bb |= 1u64 << (rank * 8 + file);
            file += df;
            rank += dr;
        }
        bb
    }

    const fn build_ray_table(df: i32, dr: i32) -> [u64; 64] {
        let mut table = [0u64; 64];
        let mut sq = 0;
        while sq < 64 {
            table[sq] = ray_from(sq as u8, df, dr);
            sq += 1;
        }
        table
    }

    const NORTH: [u64; 64] = build_ray_table(0, 1);
    const SOUTH: [u64; 64] = build_ray_table(0, -1);
    const EAST: [u64; 64] = build_ray_table(1, 0);
    const WEST: [u64; 64] = build_ray_table(-1, 0);
    const NORTH_EAST: [u64; 64] = build_ray_table(1, 1);
    const NORTH_WEST: [u64; 64] = build_ray_table(-1, 1);
    const SOUTH_EAST: [u64; 64] = build_ray_table(1, -1);
    const SOUTH_WEST: [u64; 64] = build_ray_table(-1, -1);

    fn positive_ray_attacks(table: &[u64; 64], sq: Square, occupied: Bitboard) -> Bitboard {
        let ray = Bitboard(table[sq.0 as usize]);
        match (ray & occupied).lsb() {
            Some(blocker) => ray ^ Bitboard(table[blocker.0 as usize]),
            None => ray,
        }
    }

    fn negative_ray_attacks(table: &[u64; 64], sq: Square, occupied: Bitboard) -> Bitboard {
        let ray = Bitboard(table[sq.0 as usize]);
        match (ray & occupied).msb() {
            Some(blocker) => ray ^ Bitboard(table[blocker.0 as usize]),
            None => ray,
        }
    }

    pub fn bishop_attacks(sq: Square, occupied: Bitboard) -> Bitboard {
        positive_ray_attacks(&NORTH_EAST, sq, occupied)
            | positive_ray_attacks(&NORTH_WEST, sq, occupied)
            | negative_ray_attacks(&SOUTH_EAST, sq, occupied)
            | negative_ray_attacks(&SOUTH_WEST, sq, occupied)
    }

    pub fn rook_attacks(sq: Square, occupied: Bitboard) -> Bitboard {
        positive_ray_attacks(&NORTH, sq, occupied)
            | negative_ray_attacks(&SOUTH, sq, occupied)
            | positive_ray_attacks(&EAST, sq, occupied)
            | negative_ray_attacks(&WEST, sq, occupied)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::board::STARTPOS_FEN;

    const KIWIPETE_FEN: &str = "r3k2r/p1ppqpb1/bn2pnp1/3PN3/1p2P3/2N2Q1p/PPPBBPPP/R3K2R w KQkq - 0 1";
    const POSITION3_FEN: &str = "8/2p5/3p4/KP5r/1R3p1k/8/4P1P1/8 w - - 0 1";
    const POSITION4_FEN: &str = "r3k2r/Pppp1ppp/1b3nbN/nP6/BBP1P3/q4N2/Pp1P2PP/R2Q1RK1 w kq - 0 1";
    const POSITION5_FEN: &str = "rnbq1k1r/pp1Pbppp/2p5/8/2B5/8/PPP1NnPP/RNBQK2R w KQ - 1 8";
    const POSITION6_FEN: &str = "r4rk1/1pp1qppp/p1np1n2/2b1p1B1/2B1P1b1/P1NP1N2/1PP1QPPP/R4RK1 w - - 0 10";

    /// Magic lookups against the ray-walking implementation they replaced,
    /// over occupancies dense enough to exercise blockers on every ray. If
    /// these two ever disagree, the search changes what it visits and the
    /// change stops being a pure speed change.
    #[test]
    fn magic_sliders_agree_with_the_classical_ray_walk() {
        // xorshift64*, fixed seed: a disagreement has to be reproducible.
        let mut state = 0x9e37_79b9_7f4a_7c15u64;
        let mut next = move || {
            state ^= state >> 12;
            state ^= state << 25;
            state ^= state >> 27;
            state.wrapping_mul(0x2545_F491_4F6C_DD1D)
        };
        for sq in 0..64u8 {
            let square = Square(sq);
            for _ in 0..256 {
                // Three densities: a nearly empty board, a normal one, and
                // one crowded enough that most rays stop after a step.
                for occupied in [
                    Bitboard(next() & next() & next()),
                    Bitboard(next() & next()),
                    Bitboard(next()),
                ] {
                    assert_eq!(
                        bishop_attacks(square, occupied),
                        classical_reference::bishop_attacks(square, occupied),
                        "alfil en {sq}"
                    );
                    assert_eq!(
                        rook_attacks(square, occupied),
                        classical_reference::rook_attacks(square, occupied),
                        "torre en {sq}"
                    );
                    assert_eq!(
                        queen_attacks(square, occupied),
                        classical_reference::bishop_attacks(square, occupied)
                            | classical_reference::rook_attacks(square, occupied),
                        "dama en {sq}"
                    );
                }
            }
        }
    }

    #[test]
    fn startpos_has_20_legal_moves() {
        let board = Board::start_pos();
        assert_eq!(generate_legal_moves(&board).len(), 20);
    }

    #[test]
    fn perft_startpos() {
        let mut board = Board::start_pos();
        assert_eq!(perft(&mut board, 1), 20);
        assert_eq!(perft(&mut board, 2), 400);
        assert_eq!(perft(&mut board, 3), 8_902);
        assert_eq!(perft(&mut board, 4), 197_281);
        assert_eq!(board.to_fen(), STARTPOS_FEN);
    }

    #[test]
    #[ignore = "lento: ejecutar manualmente con --release cuando se necesite mayor confianza"]
    fn perft_startpos_deep() {
        let mut board = Board::start_pos();
        assert_eq!(perft(&mut board, 5), 4_865_609);
    }

    #[test]
    fn perft_kiwipete() {
        let mut board = Board::from_fen(KIWIPETE_FEN).unwrap();
        assert_eq!(perft(&mut board, 1), 48);
        assert_eq!(perft(&mut board, 2), 2_039);
        assert_eq!(perft(&mut board, 3), 97_862);
    }

    #[test]
    #[ignore = "lento: ejecutar manualmente con --release cuando se necesite mayor confianza"]
    fn perft_kiwipete_deep() {
        let mut board = Board::from_fen(KIWIPETE_FEN).unwrap();
        assert_eq!(perft(&mut board, 4), 4_085_603);
    }

    #[test]
    fn perft_position3() {
        let mut board = Board::from_fen(POSITION3_FEN).unwrap();
        assert_eq!(perft(&mut board, 1), 14);
        assert_eq!(perft(&mut board, 2), 191);
        assert_eq!(perft(&mut board, 3), 2_812);
        assert_eq!(perft(&mut board, 4), 43_238);
    }

    #[test]
    fn perft_position4() {
        let mut board = Board::from_fen(POSITION4_FEN).unwrap();
        assert_eq!(perft(&mut board, 1), 6);
        assert_eq!(perft(&mut board, 2), 264);
        assert_eq!(perft(&mut board, 3), 9_467);
    }

    #[test]
    fn perft_position5() {
        let mut board = Board::from_fen(POSITION5_FEN).unwrap();
        assert_eq!(perft(&mut board, 1), 44);
        assert_eq!(perft(&mut board, 2), 1_486);
        assert_eq!(perft(&mut board, 3), 62_379);
    }

    #[test]
    fn perft_position6() {
        let mut board = Board::from_fen(POSITION6_FEN).unwrap();
        assert_eq!(perft(&mut board, 1), 46);
        assert_eq!(perft(&mut board, 2), 2_079);
        assert_eq!(perft(&mut board, 3), 89_890);
    }

    #[test]
    fn checkmate_has_no_legal_moves_and_king_in_check() {
        let board = Board::from_fen("4R1k1/5ppp/8/8/8/8/8/4K3 b - - 0 1").unwrap();
        assert!(generate_legal_moves(&board).is_empty());
        assert!(is_in_check(&board, Color::Black));
    }

    #[test]
    fn stalemate_has_no_legal_moves_and_king_not_in_check() {
        let board = Board::from_fen("k7/8/1Q6/8/8/8/8/7K b - - 0 1").unwrap();
        assert!(generate_legal_moves(&board).is_empty());
        assert!(!is_in_check(&board, Color::Black));
    }

    #[test]
    fn castling_unavailable_through_attacked_square() {
        // Black rook on f8 controls the open f-file down to f1, the square
        // the white king must cross to castle kingside (but not e1 itself,
        // so the king is not in check and queenside remains available).
        let board = Board::from_fen("4kr2/8/8/8/8/8/8/R3K2R w KQ - 0 1").unwrap();
        let moves = generate_legal_moves(&board);
        assert!(!moves.iter().any(|m| m.flag == MoveFlag::KingCastle));
        assert!(moves.iter().any(|m| m.flag == MoveFlag::QueenCastle));
    }

    #[test]
    fn en_passant_move_is_generated() {
        let board =
            Board::from_fen("rnbqkbnr/ppp1pppp/8/3pP3/8/8/PPPP1PPP/RNBQKBNR w KQkq d6 0 3").unwrap();
        let moves = generate_legal_moves(&board);
        assert!(moves
            .iter()
            .any(|m| m.flag == MoveFlag::EnPassant && m.to == Square::new(3, 5)));
    }

    fn assert_hash_consistent(board: &Board) {
        assert_eq!(
            board.hash,
            board.compute_hash_from_scratch(),
            "el hash Zobrist incremental no coincide con el recalculado, FEN: {}",
            board.to_fen()
        );
    }

    fn walk_and_check_hashes(board: &mut Board, depth: u32) {
        assert_hash_consistent(board);
        if depth == 0 {
            return;
        }
        for mv in generate_legal_moves(board) {
            let undo = board.make_move(mv);
            walk_and_check_hashes(board, depth - 1);
            board.unmake_move(mv, undo);
            assert_hash_consistent(board);
        }
    }

    #[test]
    fn zobrist_hash_matches_recomputation_through_move_tree() {
        let mut board = Board::start_pos();
        walk_and_check_hashes(&mut board, 3);

        let mut board = Board::from_fen(KIWIPETE_FEN).unwrap();
        walk_and_check_hashes(&mut board, 2);
    }

    #[test]
    fn pinned_piece_cannot_move_and_expose_king() {
        // White king e1, white bishop e2 pinned by black rook e8 along the e-file.
        let board = Board::from_fen("4r1k1/8/8/8/8/8/4B3/4K3 w - - 0 1").unwrap();
        let moves = generate_legal_moves(&board);
        assert!(!moves.iter().any(|m| m.from == Square::new(4, 1) && m.to.file() != 4));
    }

    #[test]
    fn see_of_capturing_an_undefended_pawn_is_just_the_pawn() {
        let board = Board::from_fen("4k3/8/8/3p4/8/8/8/3QK3 w - - 0 1").unwrap();
        let mv = Move::new(Square::new(3, 0), Square::new(3, 4), MoveFlag::Capture); // Qd1xd5
        assert_eq!(static_exchange_eval(&board, mv), 100);
    }

    #[test]
    fn see_of_a_queen_taking_a_pawn_defended_by_a_pawn_is_very_negative() {
        // Qd1xd5, but the pawn on d5 is defended by a black pawn on e6:
        // after exd5 White has traded a queen for a pawn.
        let board = Board::from_fen("4k3/8/4p3/3p4/8/8/8/3QK3 w - - 0 1").unwrap();
        let mv = Move::new(Square::new(3, 0), Square::new(3, 4), MoveFlag::Capture);
        assert!(static_exchange_eval(&board, mv) < -700);
    }

    #[test]
    fn see_of_capturing_an_undefended_rook_is_a_free_rook() {
        let board = Board::from_fen("4k3/8/8/3r4/8/8/8/3RK3 w - - 0 1").unwrap();
        let mv = Move::new(Square::new(3, 0), Square::new(3, 4), MoveFlag::Capture);
        assert_eq!(static_exchange_eval(&board, mv), 500);
    }

    #[test]
    fn see_of_a_quiet_promotion_includes_the_new_queens_value() {
        // b7-b8=Q on an empty, undefended square: a plain pawn push nets
        // the full queen-minus-pawn value, not zero (there's no capture to
        // report from `piece_at(to)` alone).
        let board = Board::from_fen("4k3/1P6/8/8/8/8/8/4K3 w - - 0 1").unwrap();
        let mv = Move::new(Square::new(1, 6), Square::new(1, 7), MoveFlag::PromoQueen);
        assert_eq!(
            static_exchange_eval(&board, mv),
            eval::piece_value(PieceType::Queen) - eval::piece_value(PieceType::Pawn)
        );
    }

    #[test]
    fn see_of_a_promotion_capture_recaptured_by_a_rook_accounts_for_the_new_queen() {
        // axb8=Q, but the new queen on b8 is immediately recaptured by a
        // black rook on b5 down the b-file. The queen's bonus and its loss
        // cancel out algebraically, leaving just rook-for-pawn.
        let board = Board::from_fen("1r1k4/P7/8/1r6/8/8/8/4K3 w - - 0 1").unwrap();
        let mv = Move::new(Square::new(0, 6), Square::new(1, 7), MoveFlag::PromoCaptureQueen);
        let expected = eval::piece_value(PieceType::Rook) - eval::piece_value(PieceType::Pawn);
        assert_eq!(static_exchange_eval(&board, mv), expected);
    }

    #[test]
    fn see_does_not_let_the_king_recapture_on_a_defended_square() {
        // Qa2xd5 wins a pawn "defended" only by the black king — but
        // White's rook on d1 also bears on d5, so Kxd5 would be moving into
        // check and the pawn is actually free. The old swap loop let the
        // king recapture anyway (and, with piece_value(King) == 0, scored
        // the follow-up capture of the king as a harmless trade), so this
        // came out as losing the queen instead of winning a clean pawn.
        let board = Board::from_fen("8/8/3k4/3p4/8/8/Q7/3RK3 w - - 0 1").unwrap();
        let mv = Move::new(Square::new(0, 1), Square::new(3, 4), MoveFlag::Capture); // Qa2xd5
        assert_eq!(static_exchange_eval(&board, mv), eval::piece_value(PieceType::Pawn));
    }

    #[test]
    fn see_still_lets_the_king_recapture_when_the_square_is_otherwise_undefended() {
        // Same position minus the d1 rook: now Kxd5 is perfectly legal, and
        // grabbing the pawn really does trade the queen for it.
        let board = Board::from_fen("8/8/3k4/3p4/8/8/Q7/4K3 w - - 0 1").unwrap();
        let mv = Move::new(Square::new(0, 1), Square::new(3, 4), MoveFlag::Capture);
        assert_eq!(
            static_exchange_eval(&board, mv),
            eval::piece_value(PieceType::Pawn) - eval::piece_value(PieceType::Queen)
        );
    }

    #[test]
    fn see_of_an_even_rook_trade_is_zero() {
        // Rd1xd5, but the black rook on d5 is defended by another black
        // rook behind it on d8: after Rxd5 Rxd5 both sides gave up a rook.
        let board = Board::from_fen("3rk3/8/8/3r4/8/8/8/3RK3 w - - 0 1").unwrap();
        let mv = Move::new(Square::new(3, 0), Square::new(3, 4), MoveFlag::Capture);
        assert_eq!(static_exchange_eval(&board, mv), 0);
    }

    /// Walks every legal move of every position down to `depth` and checks
    /// `gives_check` against the make/unmake answer it replaces.
    fn assert_gives_check_agrees(board: &mut Board, depth: u32) {
        for mv in legal_moves_scratch(board) {
            let predicted = gives_check(board, mv);
            let undo = board.make_move(mv);
            let actual = is_in_check(board, board.side_to_move);
            assert_eq!(
                predicted,
                actual,
                "gives_check disagreed on {mv} in {}",
                {
                    board.unmake_move(mv, undo);
                    board.to_fen()
                }
            );
            if depth > 1 {
                assert_gives_check_agrees(board, depth - 1);
            }
            board.unmake_move(mv, undo);
        }
    }

    #[test]
    fn gives_check_matches_make_move() {
        // The reference positions, chosen for exactly the cases a
        // square-arithmetic check test gets wrong: Kiwipete for castling
        // and pins, position 3 for en passant and discovered checks along a
        // rank, position 4 for promotions.
        let cases = [
            ("rnbqkbnr/pppppppp/8/8/8/8/PPPPPPPP/RNBQKBNR w KQkq - 0 1", 3),
            ("r3k2r/p1ppqpb1/bn2pnp1/3PN3/1p2P3/2N2Q1p/PPPBBPPP/R3K2R w KQkq - 0 1", 3),
            ("8/2p5/3p4/KP5r/1R3p1k/8/4P1P1/8 w - - 0 1", 4),
            ("r3k2r/Pppp1ppp/1b3nbN/nP6/BBP1P3/q4N2/Pp1P2PP/R2Q1RK1 w kq - 0 1", 3),
            ("rnbq1k1r/pp1Pbppp/2p5/8/2B5/8/PPP1NnPP/RNBQK2R w KQ - 1 8", 3),
        ];
        for (fen, depth) in cases {
            let mut board = Board::from_fen(fen).unwrap();
            assert_gives_check_agrees(&mut board, depth);
        }
    }

    #[test]
    fn see_prices_a_promotion_that_happens_during_the_recapture_chain() {
        // Black rook takes the rook on b8; White answers axb8=Q. The swap
        // used to price that answering pawn as a pawn — it only ever
        // accounted for a promotion made by the *initial* move — and
        // reported the whole sequence as an even trade.
        let board = Board::from_fen("1R2k3/P7/8/8/1r6/8/8/4K3 b - - 0 1").unwrap();
        let rxb8 = Move::new(Square::new(1, 3), Square::new(1, 7), MoveFlag::Capture);
        let see = static_exchange_eval(&board, rxb8);
        assert!(
            see < -700,
            "...Rxb8 axb8=Q loses a rook for a rook plus a new queen; SEE said {see}"
        );
    }

    #[test]
    fn see_of_a_pawn_recapture_short_of_the_last_rank_is_unaffected() {
        // Same shape one rank lower, where no promotion is involved: the
        // promotion handling must not leak into ordinary recaptures.
        let board = Board::from_fen("4k3/8/8/8/1r6/1R6/P7/4K3 b - - 0 1").unwrap();
        let rxb3 = Move::new(Square::new(1, 3), Square::new(1, 2), MoveFlag::Capture);
        let see = static_exchange_eval(&board, rxb3);
        assert_eq!(see, eval::piece_value(PieceType::Rook) - eval::piece_value(PieceType::Rook));
    }

    /// The legality filter as it stood until 0.28: play the move, ask
    /// whether it left our own king in check, take it back. Kept here as
    /// the oracle `legal_by_pins` has to agree with.
    fn legal_moves_by_make_unmake(working: &mut Board) -> Vec<Move> {
        let color = working.side_to_move;
        let mut moves = generate_pseudo_legal_moves(working);
        moves.retain(|&mv| {
            let undo = working.make_move(mv);
            let legal = !is_in_check(working, color);
            working.unmake_move(mv, undo);
            legal
        });
        moves
    }

    /// Walks every legal move down to `depth` comparing the two filters
    /// move by move and in order, not as sets. The order is part of what
    /// has to hold: `legal_moves_scratch` filters with `retain`, the search
    /// orders that list, and a permuted list would be a different search.
    fn assert_legality_agrees(board: &mut Board, depth: u32) {
        let by_pins = legal_moves_scratch(board);
        let by_make_unmake = legal_moves_by_make_unmake(board);
        assert_eq!(by_pins, by_make_unmake, "legality disagreed in {}", board.to_fen());
        if depth > 1 {
            for mv in by_pins {
                let undo = board.make_move(mv);
                assert_legality_agrees(board, depth - 1);
                board.unmake_move(mv, undo);
            }
        }
    }

    #[test]
    fn legality_by_pins_matches_the_make_unmake_filter_it_replaced() {
        // Perft already counts these trees, but it only compares totals: two
        // errors of opposite sign in the same subtree cancel out and the
        // count still matches. This compares the lists themselves, which is
        // the property the search actually depends on.
        let cases = [
            ("rnbqkbnr/pppppppp/8/8/8/8/PPPPPPPP/RNBQKBNR w KQkq - 0 1", 4),
            (KIWIPETE_FEN, 3),
            (POSITION3_FEN, 4),
            (POSITION4_FEN, 3),
            (POSITION5_FEN, 3),
            (POSITION6_FEN, 3),
            // En passant with both kings on the fourth rank's line of fire.
            ("8/8/8/8/k1pP3R/8/8/4K3 b - d3 0 1", 3),
            // Four white pieces pinned at once, one per ray family: Re6 on
            // the file, Nd4 on the rank, Bf5 and the d3 pawn on the two
            // diagonals.
            ("4r3/7b/4R3/5B2/r2NK3/3P4/8/1b5k w - - 0 1", 3),
            // Castling available to both sides while under fire.
            ("r3k2r/pppq1ppp/2n1bn2/3pp3/3PP3/2N1BN2/PPPQ1PPP/R3K2R w KQkq - 0 1", 3),
        ];
        for (fen, depth) in cases {
            let mut board = Board::from_fen(fen).unwrap();
            assert_legality_agrees(&mut board, depth);
        }
    }

    #[test]
    fn a_king_may_not_retreat_along_the_ray_of_the_rook_checking_it() {
        // The single most likely way to get this wrong: testing the
        // destination for attacks with the king still standing on its
        // origin, where it blocks the very ray it is fleeing along. The
        // rook on e1 checks the king on e5; with the king left in the
        // occupancy it stops the ray at e5 and Ke6 comes out looking safe,
        // when it is just the same check one square further away.
        let mut board = Board::from_fen("8/8/8/4k3/8/8/8/K3R3 b - - 0 1").unwrap();
        let moves = legal_moves_scratch(&mut board);
        let (e4, e6) = (Square::new(4, 3), Square::new(4, 5));
        assert!(
            !moves.iter().any(|mv| mv.to == e6),
            "Ke5-e6 runs away down the rook's own file and must not be legal: {moves:?}"
        );
        assert!(
            !moves.iter().any(|mv| mv.to == e4),
            "Ke5-e4 walks towards the rook on the same file and must not be legal: {moves:?}"
        );
        assert_eq!(moves.len(), 6, "the six squares off the e-file are the legal ones: {moves:?}");
        assert_eq!(moves, legal_moves_by_make_unmake(&mut board));
    }

    #[test]
    fn a_double_check_leaves_only_king_moves() {
        // Rook on e1 down the file, knight on f6: nothing blocks or
        // captures both, so every legal move has to start on e8.
        let mut board = Board::from_fen("4k3/8/5N2/8/8/8/8/4RK2 b - - 0 1").unwrap();
        let moves = legal_moves_scratch(&mut board);
        let e8 = Square::new(4, 7);
        assert!(!moves.is_empty(), "the king still has squares to run to");
        assert!(
            moves.iter().all(|mv| mv.from == e8),
            "only the king may answer a double check: {moves:?}"
        );
        assert_eq!(moves, legal_moves_by_make_unmake(&mut board));
    }

    #[test]
    fn en_passant_that_uncovers_the_king_along_the_rank_is_rejected() {
        // c4xd3 e.p. empties c4 *and* d4 at once, leaving the black king on
        // a4 in line with the rook on h4. This is the case no mask taken
        // before the move can express, and the reason en passant keeps
        // paying make/unmake.
        let mut board = Board::from_fen("8/8/8/8/k1pP3R/8/8/4K3 b - d3 0 1").unwrap();
        let moves = legal_moves_scratch(&mut board);
        assert!(
            !moves.iter().any(|mv| mv.flag == MoveFlag::EnPassant),
            "the en passant capture uncovers the king and must not be legal: {moves:?}"
        );
        assert_eq!(moves, legal_moves_by_make_unmake(&mut board));
    }

    #[test]
    fn en_passant_that_captures_the_checking_pawn_is_allowed() {
        // The white pawn double-pushed to d4 giving check from there. The
        // reply e4xd3 e.p. answers it by removing the checker -- and the
        // captured pawn is not on the destination square, so the
        // "capture the checker or block" mask would reject it. Only the
        // en passant branch coming first keeps this move alive.
        let mut board = Board::from_fen("8/8/8/4k3/3Pp3/8/8/4K3 b - d3 0 1").unwrap();
        let moves = legal_moves_scratch(&mut board);
        assert!(
            moves.iter().any(|mv| mv.flag == MoveFlag::EnPassant),
            "exd3 e.p. captures the checking pawn and must be legal: {moves:?}"
        );
        assert_eq!(moves, legal_moves_by_make_unmake(&mut board));
    }

    #[test]
    fn a_pinned_piece_may_still_move_along_its_pin() {
        // White king d1, white rook d4, black queen d8: the rook is pinned
        // on the d-file and may travel it, capturing the pinner included,
        // but may not leave it.
        let mut board = Board::from_fen("3q3k/8/8/8/3R4/8/8/3K4 w - - 0 1").unwrap();
        let moves = legal_moves_scratch(&mut board);
        let d4 = Square::new(3, 3);
        let d8 = Square::new(3, 7);
        assert!(
            moves.iter().any(|mv| mv.from == d4 && mv.to == d8),
            "Rxd8 captures the pinner along the pin and must be legal: {moves:?}"
        );
        assert!(
            moves.iter().filter(|mv| mv.from == d4).all(|mv| mv.to.file() == 3),
            "the pinned rook must not leave the d-file: {moves:?}"
        );
        assert_eq!(moves, legal_moves_by_make_unmake(&mut board));
    }

    #[test]
    fn between_and_line_agree_with_walking_the_ray() {
        // The two tables in one sweep, against the definitions in prose.
        for a in 0..64u8 {
            for b in 0..64u8 {
                let (sa, sb) = (Square(a), Square(b));
                let aligned = a != b
                    && (sa.file() == sb.file()
                        || sa.rank() == sb.rank()
                        || (sa.file() as i32 - sb.file() as i32).abs()
                            == (sa.rank() as i32 - sb.rank() as i32).abs());
                assert_eq!(
                    !line(sa, sb).is_empty(),
                    aligned,
                    "LINE[{a}][{b}] should be non-empty exactly when the two are aligned"
                );
                if !aligned {
                    assert!(between(sa, sb).is_empty(), "BETWEEN[{a}][{b}] must be empty when not aligned");
                    continue;
                }
                // Everything between is on the shared line, and neither
                // endpoint is in it.
                assert!(!between(sa, sb).contains(sa) && !between(sa, sb).contains(sb));
                assert_eq!(between(sa, sb) & line(sa, sb), between(sa, sb));
                assert!(line(sa, sb).contains(sa) && line(sa, sb).contains(sb));
                assert_eq!(between(sa, sb), between(sb, sa), "between is symmetric");
                assert_eq!(line(sa, sb), line(sb, sa), "the line through two squares is symmetric");
                // A rook or bishop on `a` with the squares between occupied
                // by nothing reaches `b`; with them occupied it does not,
                // unless the two are adjacent.
                let occupied = between(sa, sb);
                let attacks = if sa.file() == sb.file() || sa.rank() == sb.rank() {
                    rook_attacks(sa, occupied)
                } else {
                    bishop_attacks(sa, occupied)
                };
                assert_eq!(
                    attacks.contains(sb),
                    occupied.is_empty(),
                    "BETWEEN[{a}][{b}] must be exactly the squares that block the ray"
                );
            }
        }
    }
}
