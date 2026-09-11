//! Atalaya-256: Vigía's NNUE evaluation.
//!
//! The design, and every number in this file, is argued in
//! `docs/PlanNNUE.md`. The comments here only explain why the *code* is
//! shaped the way it is — which is the part a refactor loses without a single
//! test failing.
//!
//! `772 -> 2x256 -> 1`: flat piece-square features seen from each side, plus
//! the four castling rights, feed one `i16` accumulator per perspective, then
//! a squared clipped ReLU, then an output layer picked by piece count.
//!
//! **The invariant the whole design rests on:** no feature depends on more
//! than one piece, so a move changes at most a handful of features and the
//! accumulator is never rebuilt from scratch during a search. A king move
//! costs exactly what a pawn move costs. The only full refresh is at the root
//! of each search, once per thread.
//!
//! Every constant below has a twin in `tools/nnue/netfmt.py`, and the
//! golden-vector test keeps the two from disagreeing in silence: a feature
//! index that differs between trainer and engine trains a perfectly good
//! network over a permuted input space, and the only symptom is that the
//! engine plays worse.

use std::sync::OnceLock;

use crate::board::Board;
use crate::eval;
use crate::sha256::Sha256;
use crate::types::{CastlingRights, Color, Move, MoveFlag, Piece, PieceType, Square};

/// Inputs per perspective: 768 piece-square features plus 4 castling rights.
pub const FEATURES: usize = 772;
const PIECE_FEATURES: usize = 768;
/// Accumulator width per perspective.
pub const HIDDEN: usize = 256;
/// Output buckets stored in the file. A network may use fewer (`n_buckets`);
/// the unused ones stay in the file as zeros so its size never changes.
pub const MAX_BUCKETS: usize = 8;
/// Activation clamp, and the scale of the feature-transformer weights.
///
/// 127 and not 255: with 127 the square still fits a signed `i16`, so the
/// whole activation stays eight lanes wide per SSE2 vector. With 255 it would
/// have to be unpacked to `i32`, doubling the vectors in the hottest loop of
/// the evaluation. That is an SSE2 decision; with AVX2 it must be re-measured.
const QA: i16 = 127;
const QB_LOG2: u32 = 5;
/// `QA² · QB / 16`: turns the output sum back into centipawns.
const OUTPUT_DIVISOR: i32 = 32_258;
const HALF_OUTPUT_DIVISOR: i32 = OUTPUT_DIVISOR / 2;
/// At most 32 pieces and 4 castling rights are active at once.
const MAX_ACTIVE_FEATURES: usize = 36;
/// The largest value the activation can take: `(127 · 127) >> 4`.
const MAX_ACTIVATION: i64 = ((QA as i64) * (QA as i64)) >> 4;

const HEADER_LEN: usize = 128;
const MAGIC: &[u8; 8] = b"VIGIANN1";
/// The architecture packed into one word, so a file built for another
/// topology or quantization is refused on load instead of being read as
/// garbage. Packed the same way in `netfmt.py`.
pub const ARCH: u32 = ((FEATURES as u32) << 22) | ((HIDDEN as u32) << 12) | ((QA as u32) << 5) | QB_LOG2;
const WEIGHTS_LEN: usize = FEATURES * HIDDEN * 2 + HIDDEN * 2 + MAX_BUCKETS * 2 * HIDDEN * 2 + MAX_BUCKETS * 4;

const RIGHTS: [u8; 4] = [
    CastlingRights::WHITE_KINGSIDE,
    CastlingRights::WHITE_QUEENSIDE,
    CastlingRights::BLACK_KINGSIDE,
    CastlingRights::BLACK_QUEENSIDE,
];

/// Whether the HCE's three endgame dampers (lone minor 4/64, opposite
/// bishops 12/64, small pawnless edge 16/64) apply to the network's output.
///
/// Off, deliberately. Those constants were calibrated against the HCE's own
/// errors, and a network may well be getting right exactly the positions they
/// flatten. Turning them on is its own experiment (phase 6 of the plan).
/// Insufficient material still zeroes the score either way: that one is a
/// rule of the game, not a calibration.
const ENDGAME_DAMPERS: bool = false;

// ---------------------------------------------------------------- features

/// Feature index of `piece` on `square`, seen from `perspective`.
///
/// Two operations, no table. The square is mirrored vertically for Black, and
/// the piece is "ours" or "theirs" relative to whoever is looking rather than
/// white or black, which is what lets both perspectives share one weight
/// table. The king is `kind = 5`: one more piece, not an index into anything.
#[inline(always)]
fn piece_feature(perspective: Color, piece: Piece, square: Square) -> usize {
    let p = perspective as usize;
    let s = (square.0 as usize) ^ (56 * p);
    let theirs = (piece.color as usize != p) as usize;
    theirs * 384 + piece.kind as usize * 64 + s
}

/// Feature index of one castling right (a single bit of `CastlingRights`).
///
/// The only thing about a position that 768 flat piece-square features cannot
/// see by any route: a king on g1 with a rook on f1 is the same position as a
/// king on g1 that can still castle. It changes at most once per side per
/// game and depends on nothing else, so it keeps the invariant.
#[inline(always)]
fn castling_feature(perspective: Color, right: u8) -> usize {
    let owner = if right & (CastlingRights::WHITE_KINGSIDE | CastlingRights::WHITE_QUEENSIDE) != 0 {
        Color::White
    } else {
        Color::Black
    };
    let kingside = right & (CastlingRights::WHITE_KINGSIDE | CastlingRights::BLACK_KINGSIDE) != 0;
    PIECE_FEATURES + ((owner != perspective) as usize) * 2 + kingside as usize
}

fn for_each_active_feature(board: &Board, perspective: Color, mut visit: impl FnMut(usize)) {
    for color in [Color::White, Color::Black] {
        for kind in PieceType::ALL {
            for square in board.pieces_of(color, kind) {
                visit(piece_feature(perspective, Piece::new(color, kind), square));
            }
        }
    }
    for right in RIGHTS {
        if board.castling.has(right) {
            visit(castling_feature(perspective, right));
        }
    }
}

/// Every active feature of `board` seen from `perspective`, sorted.
///
/// Allocates, so it is for tooling and tests only. The search never asks for
/// this: it keeps its accumulators up to date move by move instead.
pub fn active_features(board: &Board, perspective: Color) -> Vec<usize> {
    let mut features = Vec::with_capacity(MAX_ACTIVE_FEATURES);
    for_each_active_feature(board, perspective, |feature| features.push(feature));
    features.sort_unstable();
    features
}

// ------------------------------------------------------------- the network

pub struct Net {
    /// `FEATURES × HIDDEN`, laid out `[feature][dimension]`: the accumulator
    /// update reads one contiguous row per changed feature.
    ft_weights: Box<[i16]>,
    ft_biases: Box<[i16]>,
    /// `MAX_BUCKETS × 2·HIDDEN`, laid out `[bucket][j]`, with the side to
    /// move's half first.
    out_weights: Box<[i16]>,
    out_biases: [i32; MAX_BUCKETS],
    /// Piece count (0..=32) to output bucket. Read from the file rather than
    /// compiled in: the trainer merges buckets it has too little data for, and
    /// the engine does not need to know the rule, only the 33 bytes.
    bucket_of: [u8; 33],
    n_buckets: u8,
    /// The sigmoid scale the network was trained with. Documentary: nothing in
    /// the engine evaluates with it, since the labels are Vigía's own
    /// centipawns and the network's output lands in centipawns by construction.
    k: u16,
}

/// Decoded one value at a time into a fresh heap allocation.
///
/// Not a `transmute`: `include_bytes!` promises no alignment, so that would be
/// undefined behaviour as well as the first `unsafe` in a `src/` that has none.
/// And not a `Box::new([[i16; 256]; 772])` either, which builds 386 KiB on the
/// stack before moving it and overflows the stack in a debug build.
fn decode_i16(bytes: &[u8]) -> Box<[i16]> {
    bytes.chunks_exact(2).map(|pair| i16::from_le_bytes([pair[0], pair[1]])).collect()
}

impl Net {
    /// Parses and validates a network file. Everything that can be checked
    /// without evaluating anything is checked here, and a file that fails any
    /// of it is refused rather than half-trusted.
    pub fn from_bytes(bytes: &[u8]) -> Result<Net, String> {
        if bytes.len() != HEADER_LEN + WEIGHTS_LEN {
            return Err(format!(
                "network file is {} bytes, the compiled architecture needs {}",
                bytes.len(),
                HEADER_LEN + WEIGHTS_LEN
            ));
        }
        let (header, weights) = bytes.split_at(HEADER_LEN);
        if &header[..8] != MAGIC {
            return Err("not a Vigía network file (bad magic)".into());
        }
        let arch = u32::from_le_bytes(header[8..12].try_into().unwrap());
        if arch != ARCH {
            return Err(format!("network architecture {arch:#010x} does not match the compiled {ARCH:#010x}"));
        }
        let k = u16::from_le_bytes(header[12..14].try_into().unwrap());
        let n_buckets = header[14];
        if n_buckets == 0 || n_buckets as usize > MAX_BUCKETS {
            return Err(format!("{n_buckets} output buckets, expected 1..={MAX_BUCKETS}"));
        }
        let bucket_of: [u8; 33] = header[15..48].try_into().unwrap();
        if let Some(pieces) = bucket_of.iter().position(|&bucket| bucket >= n_buckets) {
            return Err(format!(
                "the bucket table sends {pieces} pieces to bucket {}, but the network has {n_buckets}",
                bucket_of[pieces]
            ));
        }
        let declared: [u8; 32] = header[88..120].try_into().unwrap();
        let mut hasher = Sha256::new();
        hasher.update(weights);
        if hasher.finish() != declared {
            return Err("the weights do not match the sha256 in the header".into());
        }
        if header[120..].iter().any(|&byte| byte != 0) {
            return Err("reserved header bytes are not zero".into());
        }

        let (ft_weights, rest) = weights.split_at(FEATURES * HIDDEN * 2);
        let (ft_biases, rest) = rest.split_at(HIDDEN * 2);
        let (out_weights, out_biases_bytes) = rest.split_at(MAX_BUCKETS * 2 * HIDDEN * 2);
        let mut out_biases = [0i32; MAX_BUCKETS];
        for (bias, chunk) in out_biases.iter_mut().zip(out_biases_bytes.chunks_exact(4)) {
            *bias = i32::from_le_bytes(chunk.try_into().unwrap());
        }

        let net = Net {
            ft_weights: decode_i16(ft_weights),
            ft_biases: decode_i16(ft_biases),
            out_weights: decode_i16(out_weights),
            out_biases,
            bucket_of,
            n_buckets,
            k,
        };
        net.check_accumulator_bounds()?;
        net.check_output_bounds()?;
        Ok(net)
    }

    #[inline(always)]
    fn feature_row(&self, feature: usize) -> &[i16; HIDDEN] {
        self.ft_weights[feature * HIDDEN..(feature + 1) * HIDDEN].try_into().unwrap()
    }

    #[inline(always)]
    fn output_row(&self, bucket: usize) -> &[i16; 2 * HIDDEN] {
        self.out_weights[bucket * 2 * HIDDEN..(bucket + 1) * 2 * HIDDEN].try_into().unwrap()
    }

    #[inline(always)]
    fn bucket_for(&self, board: &Board) -> usize {
        self.bucket_of[board.occupied().count() as usize] as usize
    }

    pub fn n_buckets(&self) -> usize {
        self.n_buckets as usize
    }

    pub fn k(&self) -> u16 {
        self.k
    }

    /// T1 of the plan: no set of active features can overflow the `i16`
    /// accumulator. At most 36 are active at once, so for every dimension the
    /// 36 largest weights of each sign, plus the bias, have to stay in range.
    ///
    /// Stockfish checks nothing here and trusts its trainer. It costs a few
    /// milliseconds once, and an overflow inside a 12,000-game match would be
    /// silent and practically impossible to trace back.
    pub fn check_accumulator_bounds(&self) -> Result<(), String> {
        let mut column = Vec::with_capacity(FEATURES);
        for dimension in 0..HIDDEN {
            column.clear();
            column.extend((0..FEATURES).map(|feature| self.ft_weights[feature * HIDDEN + dimension] as i64));
            column.sort_unstable();
            let bias = self.ft_biases[dimension] as i64;
            let highest: i64 = column.iter().rev().take(MAX_ACTIVE_FEATURES).filter(|&&w| w > 0).sum();
            let lowest: i64 = column.iter().take(MAX_ACTIVE_FEATURES).filter(|&&w| w < 0).sum();
            if bias + highest > i16::MAX as i64 || bias + lowest < i16::MIN as i64 {
                return Err(format!(
                    "accumulator dimension {dimension} can overflow i16 (bias {bias}, range {lowest}..{highest})"
                ));
            }
        }
        Ok(())
    }

    /// T2 of the plan: the output sum cannot overflow `i32`, rounding
    /// included, whatever the accumulators hold.
    pub fn check_output_bounds(&self) -> Result<(), String> {
        for bucket in 0..self.n_buckets as usize {
            let weights: i64 = self.output_row(bucket).iter().map(|&w| (w as i64).abs()).sum();
            let worst = weights * MAX_ACTIVATION + (self.out_biases[bucket] as i64).abs() + HALF_OUTPUT_DIVISOR as i64;
            if worst > i32::MAX as i64 {
                return Err(format!("output bucket {bucket} can overflow i32 (worst case {worst})"));
            }
        }
        Ok(())
    }
}

/// The network compiled into the binary.
///
/// Embedded with `include_bytes!` and not read from a file next to the
/// executable, and not for convenience: the bench signs every experiment with
/// the sha256 of the binary, so with the network outside it two runs could be
/// "the same binary" playing with different networks, and the bench's
/// reproducibility would break without a single warning.
static EMBEDDED: &[u8] = include_bytes!("../nets/material-256.bin");

pub fn embedded() -> &'static Net {
    static NET: OnceLock<Net> = OnceLock::new();
    NET.get_or_init(|| {
        Net::from_bytes(EMBEDDED).unwrap_or_else(|e| panic!("the network compiled into this binary is invalid: {e}"))
    })
}

// ------------------------------------------------------------ the kernels
//
// THE THREE WRITING RULES. They are not style: between them they are worth
// 60 % to 400 % of the evaluation's speed, they are measured (§4.5 of the
// plan), and a refactor that breaks one fails no functional test.
//
// R1. The accumulator loops are zipped iterators, never indices, and copying
//     the parent into the child is fused into the first update. Indexing costs
//     up to 2.7x; copy-then-modify adds ~10 ns per move for nothing.
// R2. HIDDEN is a compile-time constant and the slices are `&[i16; HIDDEN]`,
//     not `&[i16]`, so LLVM knows the length and emits no scalar prologue.
// R3. The output reduction is `sum += (t as i32) * (w as i32)` with `t` and `w`
//     both `i16`. That exact shape becomes `pmaddwd`. With the operands
//     already `i32` LLVM emits `pmulld`, which is SSE4.1 and this binary does
//     not have it: it would be emulated with `pmuludq` and shuffles at three or
//     four times the cost, and nothing would fail.
//
// The additions wrap on purpose. The bounds only cover the final accumulator:
// an intermediate sum inside one update may briefly count one feature too many
// before its removal lands. Wrapping arithmetic still arrives at the right
// value, is what a debug build needs in order not to panic there, and compiles
// to the same `paddw`/`psubw` in release.

#[inline(always)]
fn add_row(destination: &mut [i16; HIDDEN], row: &[i16; HIDDEN]) {
    for (d, r) in destination.iter_mut().zip(row) {
        *d = d.wrapping_add(*r);
    }
}

#[inline(always)]
fn sub_row(destination: &mut [i16; HIDDEN], row: &[i16; HIDDEN]) {
    for (d, r) in destination.iter_mut().zip(row) {
        *d = d.wrapping_sub(*r);
    }
}

#[inline(always)]
fn screlu_dot(accumulator: &[i16; HIDDEN], weights: &[i16; HIDDEN]) -> i32 {
    let mut sum = 0i32;
    for (&a, &w) in accumulator.iter().zip(weights) {
        let v = a.clamp(0, QA);
        let t = (v * v) >> 4; // at most 1,008: stays i16, even in a debug build
        sum += (t as i32) * (w as i32);
    }
    sum
}

/// Rounds half away from zero, by division. An arithmetic `>>` floors
/// towards minus infinity and would break `eval(p) == -eval(colour-flip of
/// p)`, which the search relies on. A division by a constant is a
/// multiply-high and a shift, once per evaluation.
#[inline(always)]
fn divide_rounding(sum: i32) -> i32 {
    if sum >= 0 {
        (sum + HALF_OUTPUT_DIVISOR) / OUTPUT_DIVISOR
    } else {
        (sum - HALF_OUTPUT_DIVISOR) / OUTPUT_DIVISOR
    }
}

// ------------------------------------------------------- the accumulators

const SLOT: usize = 2 * HIDDEN;

/// One accumulator pair per ply, owned by a search thread's `Context` and
/// never by `Board`: `Board` is cloned per thread, in perft and by the bench's
/// referee, and has no business carrying 129 KiB around.
///
/// An array indexed by ply, not a stack with push and pop. Slot `ply + 1` is
/// always written from slot `ply` and the move about to be played, so there is
/// nothing to undo: `unmake_move` never has to hear about the network, and an
/// early return from anywhere in the search cannot leave the state
/// inconsistent, because every node overwrites its own slot from its parent's.
pub(crate) struct Accumulators {
    slots: Box<[i16]>,
}

impl Accumulators {
    pub(crate) fn new(plies: usize) -> Accumulators {
        Accumulators { slots: vec![0; plies * SLOT].into_boxed_slice() }
    }

    #[inline(always)]
    fn half(&self, ply: usize, perspective: Color) -> &[i16; HIDDEN] {
        let start = ply * SLOT + perspective as usize * HIDDEN;
        self.slots[start..start + HIDDEN].try_into().unwrap()
    }

    /// Rebuilds slot `ply` from nothing. Called once per search per thread, at
    /// the root; the design guarantees nothing else ever needs it.
    pub(crate) fn refresh(&mut self, net: &Net, board: &Board, ply: usize) {
        for perspective in [Color::White, Color::Black] {
            let start = ply * SLOT + perspective as usize * HIDDEN;
            let destination: &mut [i16; HIDDEN] = (&mut self.slots[start..start + HIDDEN]).try_into().unwrap();
            destination.copy_from_slice(&net.ft_biases);
            for_each_active_feature(board, perspective, |feature| add_row(destination, net.feature_row(feature)));
        }
    }

    /// Slot `ply + 1` for a null move: the pieces and the castling rights have
    /// not changed, so neither has either perspective.
    pub(crate) fn copy_parent(&mut self, ply: usize) {
        let (parents, children) = self.slots.split_at_mut((ply + 1) * SLOT);
        children[..SLOT].copy_from_slice(&parents[ply * SLOT..]);
    }

    /// Slot `ply + 1` from slot `ply` and `mv`. **Must be called before
    /// `board.make_move(mv)`**: it reads the moving piece, the captured piece
    /// and the castling rights from the position the move is played from. That
    /// keeps it a pure function of `(board, mv)`, independent of `Undo`'s
    /// private fields.
    ///
    /// And once per `make_move`, never once per recursive call: a single move
    /// can feed up to three `negamax` calls (the LMR scout, the full-depth
    /// re-search, the full-window re-search), and hooking the update into the
    /// recursion would apply it several times without anything noticing.
    pub(crate) fn push(&mut self, net: &Net, board: &Board, mv: Move, ply: usize) {
        let moving = board.piece_at(mv.from).expect("nnue push: no piece on the origin square");
        let placed = Piece::new(moving.color, mv.flag.promotion_piece().unwrap_or(moving.kind));
        let captured = if mv.flag.is_capture() {
            let square = if mv.flag == MoveFlag::EnPassant {
                Square::new(mv.to.file(), mv.from.rank())
            } else {
                mv.to
            };
            board.piece_at(square).map(|piece| (piece, square))
        } else {
            None
        };
        let castle_rook = mv.flag.is_castle().then(|| {
            let (from, to) = Board::castle_rook_squares(moving.color, mv.flag);
            (Piece::new(moving.color, PieceType::Rook), from, to)
        });
        // Rights only ever disappear during a move, never appear.
        let lost_rights = board.castling.0 & !Board::next_castling_rights(board.castling, mv.from, mv.to, moving).0;

        let (parents, children) = self.slots.split_at_mut((ply + 1) * SLOT);
        let parent = &parents[ply * SLOT..];
        let child = &mut children[..SLOT];
        for perspective in [Color::White, Color::Black] {
            let p = perspective as usize;
            let source: &[i16; HIDDEN] = parent[p * HIDDEN..(p + 1) * HIDDEN].try_into().unwrap();
            let destination: &mut [i16; HIDDEN] = (&mut child[p * HIDDEN..(p + 1) * HIDDEN]).try_into().unwrap();

            // R1: the copy from the parent and the one change every move has
            // (the piece leaving its square and arriving on another) in one pass.
            let added = net.feature_row(piece_feature(perspective, placed, mv.to));
            let removed = net.feature_row(piece_feature(perspective, moving, mv.from));
            for (d, ((s, a), r)) in destination.iter_mut().zip(source.iter().zip(added).zip(removed)) {
                *d = s.wrapping_add(*a).wrapping_sub(*r);
            }

            if let Some((piece, square)) = captured {
                sub_row(destination, net.feature_row(piece_feature(perspective, piece, square)));
            }
            if let Some((rook, from, to)) = castle_rook {
                add_row(destination, net.feature_row(piece_feature(perspective, rook, to)));
                sub_row(destination, net.feature_row(piece_feature(perspective, rook, from)));
            }
            for right in RIGHTS {
                if lost_rights & right != 0 {
                    sub_row(destination, net.feature_row(castling_feature(perspective, right)));
                }
            }
        }
    }

    /// The raw output sum for slot `ply`, before dividing back to centipawns.
    #[inline(always)]
    fn output_sum(&self, net: &Net, ply: usize, side_to_move: Color, bucket: usize) -> i32 {
        let (us, them) = net.output_row(bucket).split_at(HIDDEN);
        net.out_biases[bucket]
            + screlu_dot(self.half(ply, side_to_move), us.try_into().unwrap())
            + screlu_dot(self.half(ply, side_to_move.opposite()), them.try_into().unwrap())
    }

    /// The network's opinion from White's side, before the endgame scale.
    #[inline(always)]
    fn raw_white(&self, net: &Net, board: &Board, ply: usize) -> i32 {
        let stm = divide_rounding(self.output_sum(net, ply, board.side_to_move, net.bucket_for(board)));
        if board.side_to_move == Color::White {
            stm
        } else {
            -stm
        }
    }

    /// The full evaluation from White's side, like `eval::evaluate`: the KPK
    /// oracle in front, the network, and the endgame scale behind it.
    pub(crate) fn evaluate_white(&self, net: &Net, board: &Board, ply: usize) -> i32 {
        if let Some(exact) = kpk_shortcut(board) {
            return exact;
        }
        let raw = self.raw_white(net, board, ply);
        (raw * scale_factor(board, raw)) / 64
    }

    /// What the search calls: the evaluation from the side to move's point of
    /// view, like `eval::evaluate_relative`.
    #[inline(always)]
    pub(crate) fn evaluate(&self, net: &Net, board: &Board, ply: usize) -> i32 {
        let white = self.evaluate_white(net, board, ply);
        if board.side_to_move == Color::White {
            white
        } else {
            -white
        }
    }

    #[cfg(test)]
    fn slot(&self, ply: usize) -> &[i16] {
        &self.slots[ply * SLOT..(ply + 1) * SLOT]
    }

    /// Whether slot `ply` holds exactly what a refresh of `board` would. Test
    /// builds only: the search asserts it at every evaluation.
    #[cfg(test)]
    pub(crate) fn matches_refresh(&self, net: &Net, board: &Board, ply: usize) -> bool {
        let mut scratch = Accumulators::new(1);
        scratch.refresh(net, board, 0);
        self.slot(ply) == scratch.slot(0)
    }
}

/// The one piece of the old evaluation kept *in front of* the network. A
/// king-and-one-pawn ending is answered by retrograde analysis, not by an
/// opinion, and no network distilled from self-play is going to match "this
/// is a certain draw" with the wrong rook pawn. Cheapest test first.
#[inline(always)]
fn kpk_shortcut(board: &Board) -> Option<i32> {
    let pawns =
        board.pieces_of(Color::White, PieceType::Pawn).count() + board.pieces_of(Color::Black, PieceType::Pawn).count();
    (pawns == 1 && eval::game_phase(board) == 0).then(|| eval::kpk_exact_score(board))
}

/// Out of 64, applied behind the network. See `ENDGAME_DAMPERS`.
#[inline(always)]
fn scale_factor(board: &Board, raw: i32) -> i32 {
    if ENDGAME_DAMPERS {
        eval::endgame_scale_factor(board, raw)
    } else if eval::is_drawn_by_insufficient_material(board) {
        0
    } else {
        64
    }
}

/// What the UCI `eval` command reports about the network for one position.
pub(crate) struct Explanation {
    /// Which path answered: `kpk_exact` or `network`.
    pub path: &'static str,
    /// The network's output from White's side, before the endgame scale.
    pub network_white: i32,
    pub bucket: usize,
    /// Out of 64.
    pub scale: i32,
    /// What the search sees, from White's side.
    pub total_white: i32,
}

pub(crate) fn explain(net: &Net, board: &Board) -> Explanation {
    let mut accumulators = Accumulators::new(1);
    accumulators.refresh(net, board, 0);
    let bucket = net.bucket_for(board);
    let network_white = accumulators.raw_white(net, board, 0);
    match kpk_shortcut(board) {
        Some(exact) => Explanation { path: "kpk_exact", network_white, bucket, scale: 64, total_white: exact },
        None => {
            let scale = scale_factor(board, network_white);
            Explanation { path: "network", network_white, bucket, scale, total_white: network_white * scale / 64 }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::movegen;

    const KIWIPETE: &str = "r3k2r/p1ppqpb1/bn2pnp1/3PN3/1p2P3/2N2Q1p/PPPBBPPP/R3K2R w KQkq - 0 1";

    /// Positions chosen for what breaks incremental updates: castling on both
    /// wings, en passant, promotions with and without capture, and rooks
    /// captured on their home corners (which destroys a right the capturing
    /// side does not own).
    const WALK_POSITIONS: [(&str, u32); 7] = [
        ("rnbqkbnr/pppppppp/8/8/8/8/PPPPPPPP/RNBQKBNR w KQkq - 0 1", 3),
        (KIWIPETE, 3),
        ("8/2p5/3p4/KP5r/1R3p1k/8/4P1P1/8 w - - 0 1", 4),
        ("r3k2r/Pppp1ppp/1b3nbN/nP6/BBP1P3/q4N2/Pp1P2PP/R2Q1RK1 w kq - 0 1", 3),
        ("rnbq1k1r/pp1Pbppp/2p5/8/2B5/8/PPP1NnPP/RNBQK2R w KQ - 1 8", 3),
        ("r3k2r/8/8/8/8/8/8/R3K2R w KQkq - 0 1", 3),
        ("rnbqkbnr/ppp1p1pp/8/3pPp2/8/8/PPPP1PPP/RNBQKBNR w KQkq f6 0 3", 3),
    ];

    struct Rng(u64);

    impl Rng {
        fn next(&mut self) -> u64 {
            let mut x = self.0;
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            self.0 = x;
            x
        }

        fn symmetric(&mut self, bound: i32) -> i32 {
            (self.next() % (2 * bound as u64 + 1)) as i32 - bound
        }
    }

    /// Serializes a network exactly as `netfmt.py` does, so every test below
    /// goes through the real parser instead of around it.
    fn net_bytes(
        ft_weights: &[i16],
        ft_biases: &[i16],
        out_weights: &[i16],
        out_biases: &[i32; MAX_BUCKETS],
        bucket_of: &[u8; 33],
        n_buckets: u8,
    ) -> Vec<u8> {
        let mut weights = Vec::with_capacity(WEIGHTS_LEN);
        for value in ft_weights.iter().chain(ft_biases).chain(out_weights) {
            weights.extend_from_slice(&value.to_le_bytes());
        }
        for value in out_biases {
            weights.extend_from_slice(&value.to_le_bytes());
        }
        assert_eq!(weights.len(), WEIGHTS_LEN);
        let mut hasher = Sha256::new();
        hasher.update(&weights);

        let mut bytes = Vec::with_capacity(HEADER_LEN + WEIGHTS_LEN);
        bytes.extend_from_slice(MAGIC);
        bytes.extend_from_slice(&ARCH.to_le_bytes());
        bytes.extend_from_slice(&400u16.to_le_bytes());
        bytes.push(n_buckets);
        bytes.extend_from_slice(bucket_of);
        bytes.extend_from_slice(&[0; 32]);
        bytes.extend_from_slice(&0u64.to_le_bytes());
        bytes.extend_from_slice(&hasher.finish());
        bytes.extend_from_slice(&[0; 8]);
        assert_eq!(bytes.len(), HEADER_LEN);
        bytes.extend_from_slice(&weights);
        bytes
    }

    fn default_bucket_of() -> [u8; 33] {
        let mut table = [0u8; 33];
        for (pieces, bucket) in table.iter_mut().enumerate() {
            *bucket = (pieces.saturating_sub(1) / 4).min(MAX_BUCKETS - 1) as u8;
        }
        table
    }

    /// Every weight different and small enough to satisfy both bounds. It
    /// exercises every feature row, every output bucket and the clamp at both
    /// ends, which the hand-built material network does not.
    fn random_net_bytes(seed: u64) -> Vec<u8> {
        let mut rng = Rng(seed);
        let ft_weights: Vec<i16> = (0..FEATURES * HIDDEN).map(|_| rng.symmetric(90) as i16).collect();
        let ft_biases: Vec<i16> = (0..HIDDEN).map(|_| rng.symmetric(60) as i16).collect();
        let out_weights: Vec<i16> = (0..MAX_BUCKETS * 2 * HIDDEN).map(|_| rng.symmetric(60) as i16).collect();
        let mut out_biases = [0i32; MAX_BUCKETS];
        for bias in &mut out_biases {
            *bias = rng.symmetric(5000);
        }
        net_bytes(&ft_weights, &ft_biases, &out_weights, &out_biases, &default_bucket_of(), MAX_BUCKETS as u8)
    }

    fn random_net(seed: u64) -> Net {
        Net::from_bytes(&random_net_bytes(seed)).unwrap()
    }

    #[test]
    fn a_file_for_another_architecture_or_a_malformed_one_is_refused() {
        let good = random_net_bytes(1);
        assert!(Net::from_bytes(&good).is_ok());

        let refused = |bytes: &[u8], what: &str| {
            assert!(Net::from_bytes(bytes).is_err(), "{what} should have been refused");
        };
        refused(&good[..good.len() - 1], "a truncated file");
        let mut bad = good.clone();
        bad[0] = b'X';
        refused(&bad, "a wrong magic");
        let mut bad = good.clone();
        bad[9] ^= 0x10;
        refused(&bad, "another architecture");
        let mut bad = good.clone();
        bad[14] = 0;
        refused(&bad, "zero output buckets");
        let mut bad = good.clone();
        bad[14] = MAX_BUCKETS as u8 + 1;
        refused(&bad, "more buckets than the file holds");
        let mut bad = good.clone();
        bad[14] = 2; // the default table uses buckets up to 7
        refused(&bad, "a bucket table pointing past n_buckets");
        let mut bad = good.clone();
        bad[127] = 1;
        refused(&bad, "non-zero reserved bytes");
    }

    #[test]
    fn weights_that_do_not_match_the_hash_in_the_header_are_refused() {
        let mut bytes = random_net_bytes(2);
        bytes[HEADER_LEN + 1000] ^= 1;
        match Net::from_bytes(&bytes) {
            Ok(_) => panic!("a corrupted weight must be refused"),
            Err(e) => assert!(e.contains("sha256"), "{e}"),
        }
    }

    #[test]
    fn a_network_that_could_overflow_is_refused_on_load() {
        let mut rng = Rng(3);
        let out_biases = [0i32; MAX_BUCKETS];

        // T1: one accumulator dimension whose 36 largest weights sum past i16.
        let mut ft_weights: Vec<i16> = (0..FEATURES * HIDDEN).map(|_| rng.symmetric(10) as i16).collect();
        for feature in 0..MAX_ACTIVE_FEATURES {
            ft_weights[feature * HIDDEN + 7] = 1000;
        }
        let bytes = net_bytes(
            &ft_weights,
            &[0; HIDDEN],
            &vec![1; MAX_BUCKETS * 2 * HIDDEN],
            &out_biases,
            &default_bucket_of(),
            MAX_BUCKETS as u8,
        );
        assert!(Net::from_bytes(&bytes).is_err(), "T1 must refuse this network");

        // T2: output weights large enough for the sum to leave i32.
        let bytes = net_bytes(
            &vec![0; FEATURES * HIDDEN],
            &[0; HIDDEN],
            &vec![i16::MAX; MAX_BUCKETS * 2 * HIDDEN],
            &out_biases,
            &default_bucket_of(),
            MAX_BUCKETS as u8,
        );
        assert!(Net::from_bytes(&bytes).is_err(), "T2 must refuse this network");
    }

    #[test]
    fn the_embedded_network_loads_and_can_overflow_nothing() {
        let net = embedded();
        net.check_accumulator_bounds().unwrap();
        net.check_output_bounds().unwrap();
    }

    #[test]
    fn feature_indices_are_the_documented_ones() {
        // Pinned against literal numbers, not just against netfmt.py: if both
        // halves changed the same way, the golden vector alone would not see it.
        let white_king = Piece::new(Color::White, PieceType::King);
        assert_eq!(piece_feature(Color::White, white_king, Square::new(4, 0)), 5 * 64 + 4);
        assert_eq!(piece_feature(Color::Black, white_king, Square::new(4, 0)), 384 + 5 * 64 + 60);
        let black_pawn = Piece::new(Color::Black, PieceType::Pawn);
        assert_eq!(piece_feature(Color::Black, black_pawn, Square::new(0, 6)), 8);
        assert_eq!(piece_feature(Color::White, black_pawn, Square::new(0, 6)), 384 + 48);
        assert_eq!(castling_feature(Color::White, CastlingRights::WHITE_KINGSIDE), 769);
        assert_eq!(castling_feature(Color::White, CastlingRights::BLACK_QUEENSIDE), 770);
        assert_eq!(castling_feature(Color::Black, CastlingRights::BLACK_KINGSIDE), 769);
        assert_eq!(castling_feature(Color::Black, CastlingRights::WHITE_QUEENSIDE), 770);
    }

    #[test]
    fn the_accumulator_is_the_bias_plus_the_rows_of_the_active_features() {
        let net = random_net(6);
        let board = Board::from_fen(KIWIPETE).unwrap();
        let mut accumulators = Accumulators::new(1);
        accumulators.refresh(&net, &board, 0);
        for perspective in [Color::White, Color::Black] {
            let features = active_features(&board, perspective);
            assert_eq!(features.len(), board.occupied().count() as usize + 4, "32 pieces and 4 castling rights");
            let mut expected = net.ft_biases.to_vec();
            for feature in features {
                for (e, w) in expected.iter_mut().zip(net.feature_row(feature)) {
                    *e += *w;
                }
            }
            assert_eq!(&accumulators.half(0, perspective)[..], &expected[..]);
        }
    }

    fn walk(
        net: &Net,
        board: &mut Board,
        accumulators: &mut Accumulators,
        scratch: &mut Accumulators,
        ply: usize,
        depth: u32,
    ) {
        scratch.refresh(net, board, 0);
        assert_eq!(
            accumulators.slot(ply),
            scratch.slot(0),
            "incremental accumulator drifted from a refresh at {}",
            board.to_fen()
        );
        if depth == 0 {
            return;
        }
        for mv in movegen::legal_moves_scratch(board) {
            accumulators.push(net, board, mv, ply);
            let undo = board.make_move(mv);
            walk(net, board, accumulators, scratch, ply + 1, depth - 1);
            board.unmake_move(mv, undo);
        }
    }

    #[test]
    fn the_incremental_accumulator_matches_a_refresh_at_every_node() {
        // A full tree, compared at every node rather than at the leaves: a
        // wrong delta that a later move happens to cancel would pass at the
        // leaves and still poison every evaluation in between.
        let net = random_net(4);
        let mut accumulators = Accumulators::new(8);
        let mut scratch = Accumulators::new(1);
        for (fen, depth) in WALK_POSITIONS {
            let mut board = Board::from_fen(fen).unwrap();
            accumulators.refresh(&net, &board, 0);
            walk(&net, &mut board, &mut accumulators, &mut scratch, 0, depth);
        }
    }

    #[test]
    fn a_null_move_leaves_the_accumulator_as_it_was() {
        let net = random_net(5);
        let mut board = Board::from_fen(KIWIPETE).unwrap();
        let mut accumulators = Accumulators::new(2);
        accumulators.refresh(&net, &board, 0);
        accumulators.copy_parent(0);
        let undo = board.make_null_move();
        let mut scratch = Accumulators::new(1);
        scratch.refresh(&net, &board, 0);
        assert_eq!(accumulators.slot(1), scratch.slot(0));
        board.unmake_null_move(undo);
    }

    fn mirror_fen(fen: &str) -> String {
        let fields: Vec<&str> = fen.split_whitespace().collect();
        let swap = |c: char| if c.is_ascii_uppercase() { c.to_ascii_lowercase() } else { c.to_ascii_uppercase() };
        let placement: Vec<String> = fields[0].split('/').rev().map(|rank| rank.chars().map(swap).collect()).collect();
        let side = if fields[1] == "w" { "b" } else { "w" };
        let castling: String = if fields[2] == "-" {
            "-".to_string()
        } else {
            let flipped: Vec<char> = fields[2].chars().map(swap).collect();
            "KQkq".chars().filter(|c| flipped.contains(c)).collect()
        };
        let en_passant = if fields[3] == "-" {
            "-".to_string()
        } else {
            let mut chars = fields[3].chars();
            let file = chars.next().unwrap();
            let rank = chars.next().unwrap();
            format!("{file}{}", if rank == '3' { '6' } else { '3' })
        };
        format!("{} {side} {castling} {en_passant} {} {}", placement.join("/"), fields[4], fields[5])
    }

    fn collect_positions(board: &mut Board, depth: u32, out: &mut Vec<String>) {
        out.push(board.to_fen());
        if depth == 0 {
            return;
        }
        for mv in movegen::legal_moves_scratch(board) {
            let undo = board.make_move(mv);
            collect_positions(board, depth - 1, out);
            board.unmake_move(mv, undo);
        }
    }

    #[test]
    fn the_evaluation_is_antisymmetric_under_a_colour_flip() {
        // Holds for any weights at all, which is what makes it a real test of
        // the code rather than of the network: the perspective mirroring, the
        // "ours/theirs" split, the castling rights and the rounding by division
        // all have to be right for it to pass.
        let net = random_net(7);
        let mut fens = Vec::new();
        for fen in [
            KIWIPETE,
            "8/2p5/3p4/KP5r/1R3p1k/8/4P1P1/8 w - - 0 1",
            "r3k2r/Pppp1ppp/1b3nbN/nP6/BBP1P3/q4N2/Pp1P2PP/R2Q1RK1 w kq - 0 1",
            "8/8/8/4k3/8/8/4P3/4K3 w - - 0 1",
        ] {
            collect_positions(&mut Board::from_fen(fen).unwrap(), 2, &mut fens);
        }
        let mut accumulators = Accumulators::new(1);
        for fen in &fens {
            let board = Board::from_fen(fen).unwrap();
            let mirrored = Board::from_fen(&mirror_fen(fen)).unwrap();
            accumulators.refresh(&net, &board, 0);
            let score = accumulators.evaluate_white(&net, &board, 0);
            accumulators.refresh(&net, &mirrored, 0);
            let mirrored_score = accumulators.evaluate_white(&net, &mirrored, 0);
            assert_eq!(score, -mirrored_score, "{fen} scores {score}, its colour flip scores {mirrored_score}");
        }
    }

    #[test]
    fn the_material_network_scores_material_exactly_as_computed_by_hand() {
        // Redoes, independently and in Rust, the arithmetic that
        // tools/nnue/material_net.py builds the network from.
        const UNITS: [(PieceType, i32); 5] = [
            (PieceType::Pawn, 2),
            (PieceType::Knight, 5),
            (PieceType::Bishop, 5),
            (PieceType::Rook, 8),
            (PieceType::Queen, 14),
        ];
        let units = |board: &Board, color: Color| -> i32 {
            UNITS.iter().map(|&(kind, value)| board.pieces_of(color, kind).count() as i32 * value).sum()
        };
        let screlu = |a: i32| {
            let v = a.clamp(0, QA as i32);
            (v * v) >> 4
        };

        let net = embedded();
        let mut accumulators = Accumulators::new(1);
        for fen in [
            "rnbqkbnr/pppppppp/8/8/8/8/PPPPPPPP/RNBQKBNR w KQkq - 0 1",
            KIWIPETE,
            "4k3/8/8/8/8/8/8/3QK3 w - - 0 1",
            "4k3/8/8/8/8/8/8/3QK3 b - - 0 1",
            "6k1/8/8/8/8/8/8/QQQQQ2K w - - 0 1",
            "4k3/pp6/8/8/8/8/PPP5/4K3 w - - 0 1",
            "r3k3/8/8/8/8/8/5B2/2B1K3 b - - 0 1",
        ] {
            let board = Board::from_fen(fen).unwrap();
            let stm = board.side_to_move;
            let d = units(&board, stm) - units(&board, stm.opposite());
            let sum = 64 * 2016 * (screlu(64 + d) - screlu(64 - d));
            let stm_cp = divide_rounding(sum);
            let expected = if stm == Color::White { stm_cp } else { -stm_cp };
            accumulators.refresh(net, &board, 0);
            assert_eq!(accumulators.evaluate_white(net, &board, 0), expected, "{fen}: material difference {d} units");
        }
    }

    #[test]
    fn insufficient_material_zeroes_the_network_and_kpk_answers_before_it() {
        let net = embedded();
        let mut accumulators = Accumulators::new(1);

        // K+N vs K: the material network alone says a knight up, the rule says draw.
        let knight = Board::from_fen("8/8/8/4k3/8/8/8/3NK3 w - - 0 1").unwrap();
        accumulators.refresh(net, &knight, 0);
        assert_ne!(accumulators.raw_white(net, &knight, 0), 0);
        assert_eq!(accumulators.evaluate_white(net, &knight, 0), 0);

        // K+P vs K goes to the retrograde oracle and never reaches the network.
        let kpk = Board::from_fen("8/8/8/4k3/8/8/4P3/4K3 w - - 0 1").unwrap();
        accumulators.refresh(net, &kpk, 0);
        assert_eq!(accumulators.evaluate_white(net, &kpk, 0), eval::kpk_exact_score(&kpk));
    }
}
