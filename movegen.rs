use crate::bitboard::Bitboard;
use crate::board::Board;
use crate::eval;
use crate::types::{CastlingRights, Color, Move, MoveFlag, PieceType, Square};

// ---------------------------------------------------------------------
// Precomputed attack tables, built once at compile time.
// ---------------------------------------------------------------------

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
// Sliding attack generation (classical ray/blocker technique).
// ---------------------------------------------------------------------

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

pub fn queen_attacks(sq: Square, occupied: Bitboard) -> Bitboard {
    bishop_attacks(sq, occupied) | rook_attacks(sq, occupied)
}

pub fn knight_attacks(sq: Square) -> Bitboard {
    Bitboard(KNIGHT_ATTACKS[sq.0 as usize])
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

pub fn is_in_check(board: &Board, color: Color) -> bool {
    let king_sq = board
        .pieces_of(color, PieceType::King)
        .lsb()
        .expect("is_in_check: no hay rey en el tablero");
    is_square_attacked(board, king_sq, color.opposite())
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
    generate_pseudo_legal_moves(working)
        .into_iter()
        .filter(|&mv| {
            let undo = working.make_move(mv);
            let legal = !is_in_check(working, color);
            working.unmake_move(mv, undo);
            legal
        })
        .collect()
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

    let mut gains = vec![captured_value + promotion_gain];
    let mut side = mover_color.opposite();

    loop {
        let attackers = attackers_to(board, to, occupied) & board.color_occupied(side) & occupied;
        let Some((attacker_sq, kind)) = least_valuable_attacker(board, side, attackers) else {
            break;
        };
        gains.push(attacker_value - gains[gains.len() - 1]);
        occupied.clear(attacker_sq);
        attacker_value = eval::piece_value(kind);
        side = side.opposite();
    }

    for i in (0..gains.len() - 1).rev() {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::board::STARTPOS_FEN;

    const KIWIPETE_FEN: &str = "r3k2r/p1ppqpb1/bn2pnp1/3PN3/1p2P3/2N2Q1p/PPPBBPPP/R3K2R w KQkq - 0 1";
    const POSITION3_FEN: &str = "8/2p5/3p4/KP5r/1R3p1k/8/4P1P1/8 w - - 0 1";
    const POSITION4_FEN: &str = "r3k2r/Pppp1ppp/1b3nbN/nP6/BBP1P3/q4N2/Pp1P2PP/R2Q1RK1 w kq - 0 1";
    const POSITION5_FEN: &str = "rnbq1k1r/pp1Pbppp/2p5/8/2B5/8/PPP1NnPP/RNBQK2R w KQ - 1 8";
    const POSITION6_FEN: &str = "r4rk1/1pp1qppp/p1np1n2/2b1p1B1/2B1P1b1/P1NP1N2/1PP1QPPP/R4RK1 w - - 0 10";

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
    fn see_of_an_even_rook_trade_is_zero() {
        // Rd1xd5, but the black rook on d5 is defended by another black
        // rook behind it on d8: after Rxd5 Rxd5 both sides gave up a rook.
        let board = Board::from_fen("3rk3/8/8/3r4/8/8/8/3RK3 w - - 0 1").unwrap();
        let mv = Move::new(Square::new(3, 0), Square::new(3, 4), MoveFlag::Capture);
        assert_eq!(static_exchange_eval(&board, mv), 0);
    }
}
