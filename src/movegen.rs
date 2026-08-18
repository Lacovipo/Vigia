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
// Attack detection.
// ---------------------------------------------------------------------

/// Is `sq` attacked by any piece of `by_color` in the current position?
pub fn is_square_attacked(board: &Board, sq: Square, by_color: Color) -> bool {
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

    let occupied = board.occupied();
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
/// by `generate_castling`, so this filter is redundant-but-harmless there.
///
/// Clones `board` into a scratch copy to make/unmake moves on while testing
/// legality. Called once per search node, so that clone is a real cost;
/// `legal_moves_scratch` below exists for the hot path, which already has a
/// `&mut Board` on hand and can reuse it directly instead.
pub fn generate_legal_moves(board: &Board) -> Vec<Move> {
    let mut working = board.clone();
    legal_moves_scratch(&mut working)
}

/// Same as `generate_legal_moves`, but does its make/unmake legality
/// testing directly on the caller's own board instead of an internal
/// clone. `working` is restored to its original position before returning
/// (every generated move is made and then unmade), so this is transparent
/// to the caller — it just avoids a clone per call in `negamax`/
/// `quiescence`/`search_root`, which already hold a `&mut Board` anyway.
pub(crate) fn legal_moves_scratch(working: &mut Board) -> Vec<Move> {
    let color = working.side_to_move;
    let mut moves = generate_pseudo_legal_moves(working);
    // `retain` in place rather than `filter().collect()`: the latter built a
    // second `Vec` per node on top of the generator's own, and this is the
    // single most frequently called allocation site in the engine.
    moves.retain(|&mv| {
        let undo = working.make_move(mv);
        let legal = !is_in_check(working, color);
        working.unmake_move(mv, undo);
        legal
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
}
