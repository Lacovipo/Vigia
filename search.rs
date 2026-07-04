use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use crate::board::Board;
use crate::eval;
use crate::movegen;
use crate::types::{Color, Move, MoveFlag, PieceType, Square};
use crate::zobrist;

pub const MATE_SCORE: i32 = 30_000;
const MATE_THRESHOLD: i32 = MATE_SCORE - 1_000;
const INF: i32 = 1_000_000;
const MAX_PLY: u32 = 128;
const DEFAULT_TT_SIZE_MB: usize = 64;
/// Depth reduction applied to the verification search after a null move.
const NULL_MOVE_REDUCTION: u32 = 2;
/// Null-move pruning only pays off once there's enough depth left to still
/// search meaningfully after the reduction.
const NULL_MOVE_MIN_DEPTH: u32 = 3;
/// Half-width, in centipawns, of the aspiration window built around the
/// previous iteration's score.
const ASPIRATION_WINDOW: i32 = 25;
/// Late move reductions only kick in with enough depth left to still
/// search meaningfully after the reduction...
const LMR_MIN_DEPTH: u32 = 3;
/// ...and only past the first few moves, which are searched at full depth
/// since move ordering should have put the most promising ones first.
const LMR_FULL_DEPTH_MOVES: usize = 4;
const LMR_REDUCTION: u32 = 1;
/// Futility pruning only applies at these shallow "frontier" depths: any
/// deeper and a quiet move that looks bad right now has too much room left
/// to turn into something real.
const FUTILITY_MAX_DEPTH: u32 = 2;
/// How far behind alpha the static eval is still allowed to be before a
/// quiet move at that depth gets skipped without being searched at all,
/// indexed by depth (index 0 is unused, since depth 0 never reaches this
/// code — it goes to quiescence instead).
const FUTILITY_MARGIN: [i32; (FUTILITY_MAX_DEPTH + 1) as usize] = [0, 200, 300];
/// Safety margin added on top of the captured piece's value when delta
/// pruning in quiescence search, so a capture isn't skipped just because
/// it's a few centipawns short (piece-square/mobility swings could still
/// make up the difference).
const DELTA_PRUNING_MARGIN: i32 = 200;
/// How many centipawns below the root's best exact score a move can still
/// be while remaining a candidate for random selection: small enough that
/// picking one over the other is noise-level, but enough to give the
/// engine some variety instead of always playing the single best line.
const ROOT_TIE_EPSILON: i32 = 4;
/// Razoring only fires this shallow: any deeper and dismissing the whole
/// move loop on a quiescence-search verdict is too risky.
const RAZOR_MAX_DEPTH: u32 = 3;
const RAZOR_MARGIN_PER_DEPTH: i32 = 300;
/// Reverse futility pruning (static null-move pruning) only fires this
/// shallow, same reasoning as razoring: at higher depths the static eval
/// alone isn't a trustworthy enough stand-in for a real search.
const RFP_MAX_DEPTH: u32 = 8;
const RFP_MARGIN_PER_DEPTH: i32 = 75;
/// Internal iterative reduction only kicks in deep enough that treating
/// the node as one ply shallower is still meaningfully faster than just
/// eating the cost of the weaker move ordering.
const IIR_MIN_DEPTH: u32 = 4;
/// Singular extensions only bother verifying at real depth: the
/// verification search itself costs nodes, and at shallow depth that cost
/// isn't worth it relative to just searching everything normally.
const SINGULAR_MIN_DEPTH: u32 = 6;
/// The TT entry's own depth must be within this many plies of the current
/// depth to trust it enough to drive a singular-extension decision.
const SINGULAR_TT_DEPTH_MARGIN: u32 = 3;
const SINGULAR_MARGIN_PER_DEPTH: i32 = 2;
/// Correction history: a table of learned corrections to the static eval,
/// keyed by pawn structure (since the pawn skeleton is what our simple HCE
/// is most likely to misjudge systematically — e.g. an isolated pawn that
/// turns out fine in a specific piece configuration this eval doesn't
/// model). Size is a power of two so indexing is a plain AND-mask.
const CORRECTION_HISTORY_SIZE: usize = 16384;
/// Clamp on the learned correction itself, in centipawns: keeps a string
/// of unlucky results from making the eval wildly overconfident in either
/// direction.
const CORRECTION_MAX: i32 = 300;
/// Denominator of the exponential-moving-average update: higher-depth
/// results move the stored correction further per update (up to this
/// weight cap out of the total), since they're more trustworthy.
const CORRECTION_WEIGHT_SCALE: i32 = 32;
const CORRECTION_WEIGHT_CAP: i32 = 16;

#[derive(Clone, Default)]
pub struct SearchLimits {
    pub max_depth: Option<u32>,
    pub move_time_ms: Option<u64>,
    pub white_time_ms: Option<u64>,
    pub black_time_ms: Option<u64>,
    pub white_inc_ms: Option<u64>,
    pub black_inc_ms: Option<u64>,
    pub infinite: bool,
    /// Hard node-count budget (UCI `go nodes`): checked at the same
    /// granularity as the `stop` flag/deadline, so it's an approximate
    /// ceiling, not an exact cutoff.
    pub max_nodes: Option<u64>,
    /// Restricts the root move loop to exactly these moves (UCI `go
    /// searchmoves`), in the order the GUI listed them. `None` means every
    /// legal root move is considered, as usual.
    pub search_moves: Option<Vec<Move>>,
}

#[derive(Clone, Default)]
pub struct SearchResult {
    pub best_move: Option<Move>,
    pub score: i32,
    pub depth: u32,
    pub nodes: u64,
    /// The principal variation, root move first, reconstructed by walking
    /// the TT's stored best move from each position along the line. May be
    /// shorter than `depth` (a TT slot along the line got overwritten by
    /// an unrelated position, or a repetition/cycle was hit) but always
    /// starts with `best_move` when `best_move` is `Some`.
    pub pv: Vec<Move>,
}

/// Which side of the true score a stored evaluation represents, relative to
/// the alpha/beta window that was searched when it was stored.
#[derive(Clone, Copy, PartialEq, Eq)]
enum TtFlag {
    Exact,
    Lower, // the true score is >= the stored score (a beta cutoff occurred)
    Upper, // the true score is <= the stored score (every move failed low)
}

#[derive(Clone, Copy)]
struct TtEntry {
    key: u64,
    depth: u8,
    score: i32,
    flag: TtFlag,
    best_move: Option<Move>,
}

/// Fixed-size hash table of positions seen during search, keyed by the
/// incrementally-maintained Zobrist hash. Shallower entries are overwritten
/// by deeper ones; a same-key entry is always refreshed.
struct TranspositionTable {
    entries: Vec<Option<TtEntry>>,
    mask: u64,
}

impl TranspositionTable {
    fn new(size_mb: usize) -> Self {
        let entry_size = std::mem::size_of::<Option<TtEntry>>();
        let count = ((size_mb * 1024 * 1024) / entry_size)
            .next_power_of_two()
            .max(1);
        TranspositionTable { entries: vec![None; count], mask: (count - 1) as u64 }
    }

    fn clear(&mut self) {
        self.entries.fill(None);
    }

    fn probe(&self, key: u64) -> Option<TtEntry> {
        self.entries[(key & self.mask) as usize].filter(|e| e.key == key)
    }

    fn store(&mut self, key: u64, depth: u8, score: i32, flag: TtFlag, best_move: Option<Move>) {
        let slot = &mut self.entries[(key & self.mask) as usize];
        let should_replace = match slot {
            Some(existing) => existing.key != key || existing.depth <= depth,
            None => true,
        };
        if should_replace {
            *slot = Some(TtEntry { key, depth, score, flag, best_move });
        }
    }
}

/// Reconstructs the principal variation by repeatedly following the TT's
/// stored best move from `board` onward. Each step re-validates the move
/// against the actual legal move list (a stale/colliding TT slot could
/// otherwise hand back a move that no longer applies) and a cycle guard
/// stops the walk if it ever revisits a hash, so a corrupted or
/// transposition-heavy table can only shorten the PV, never loop forever
/// or fabricate an illegal line.
fn extract_pv(tt: &TranspositionTable, board: &Board, max_len: u32) -> Vec<Move> {
    let mut pv = Vec::new();
    let mut current = board.clone();
    let mut seen_hashes = std::collections::HashSet::new();
    while (pv.len() as u32) < max_len && seen_hashes.insert(current.hash) {
        let Some(mv) = tt.probe(current.hash).and_then(|e| e.best_move) else {
            break;
        };
        if !movegen::generate_legal_moves(&current).contains(&mv) {
            break;
        }
        current.make_move(mv);
        pv.push(mv);
    }
    pv
}

/// Transposition table shared across an entire game, not just a single
/// `search()` call: reused move after move (via [`Tt::clone_handle`]/`Arc`
/// on the caller's side) so that transpositions discovered on one move stay
/// useful on the next, and cleared only when the caller knows the game
/// itself has restarted (UCI `ucinewgame`).
pub struct Tt(Mutex<TranspositionTable>);

impl Tt {
    pub fn new(size_mb: usize) -> Self {
        Tt(Mutex::new(TranspositionTable::new(size_mb)))
    }

    pub fn clear(&self) {
        self.0.lock().unwrap().clear();
    }
}

impl Default for Tt {
    fn default() -> Self {
        Tt::new(DEFAULT_TT_SIZE_MB)
    }
}

/// Mate scores encode "distance to mate from the current node", so they
/// must be shifted by `ply` when crossing the TT boundary: stored relative
/// to the node they were computed at, converted back relative to whichever
/// node re-reads them later.
fn score_to_tt(score: i32, ply: u32) -> i32 {
    if score >= MATE_THRESHOLD {
        score + ply as i32
    } else if score <= -MATE_THRESHOLD {
        score - ply as i32
    } else {
        score
    }
}

fn score_from_tt(score: i32, ply: u32) -> i32 {
    if score >= MATE_THRESHOLD {
        score - ply as i32
    } else if score <= -MATE_THRESHOLD {
        score + ply as i32
    } else {
        score
    }
}

struct Context<'a> {
    nodes: u64,
    stop: &'a AtomicBool,
    /// Hard cutoff: search must never run past this, checked every 2048
    /// nodes just like the `stop` flag. The looser "soft" limit that
    /// decides whether to *start* another iterative-deepening depth lives
    /// in `search()` itself, not here, since it's only ever checked
    /// between depths.
    hard_deadline: Option<Instant>,
    max_nodes: Option<u64>,
    /// UCI `go searchmoves`: when set, the root move loop only considers
    /// these moves. Irrelevant below the root, where every reply must
    /// still be searched normally.
    search_moves: Option<Vec<Move>>,
    path: Vec<u64>,
    aborted: bool,
    tt: &'a mut TranspositionTable,
    /// Up to two killer (non-capture, beta-cutoff-causing) moves per ply.
    killers: Vec<[Option<Move>; 2]>,
    /// History heuristic score per [from][to], boosted on quiet cutoffs.
    history: [[i32; 64]; 64],
    /// Static eval at each ply visited so far on the current path, used to
    /// compute the "improving" flag (None while that ply was in check).
    static_evals: Vec<Option<i32>>,
    /// (piece, to-square) of the move that was played to reach each ply,
    /// so a node can look up what its parent just did. Sized to MAX_PLY+1
    /// since the last node before the ply cap still needs a slot to record
    /// into for its own children.
    moves_played: Vec<Option<(PieceType, Square)>>,
    /// Continuation history: how well a (previous move, this move) pair
    /// has performed historically, flattened as
    /// [prev_piece][prev_to][piece][to]. The classic complement to the
    /// flat history table — some quiet moves are only good as a *reply* to
    /// a specific previous move (e.g. recapturing), which a from/to-only
    /// table can't express.
    cont_history: Vec<i32>,
    /// Learned static-eval correction per pawn-structure hash bucket. See
    /// `CORRECTION_HISTORY_SIZE` and `Context::correction_score`.
    pawn_correction: Vec<i32>,
}

/// Number of PieceType variants, used to size/index `Context::cont_history`.
const PIECE_TYPE_COUNT: usize = 6;

fn cont_history_index(prev_piece: PieceType, prev_to: Square, piece: PieceType, to: Square) -> usize {
    ((prev_piece as usize * 64 + prev_to.0 as usize) * PIECE_TYPE_COUNT + piece as usize) * 64 + to.0 as usize
}

/// Zobrist hash of just the pawn structure (both colors), used to key the
/// correction history. Recomputed on demand from the pawn bitboards rather
/// than maintained incrementally like `Board::hash` — cheap relative to
/// the rest of a static eval call (at most 16 XORs), and keeps this
/// search-only concern out of `Board`.
fn pawn_hash(board: &Board) -> u64 {
    let mut hash = 0u64;
    for sq in board.pieces_of(Color::White, PieceType::Pawn) {
        hash ^= zobrist::piece_square_key(Color::White, PieceType::Pawn, sq);
    }
    for sq in board.pieces_of(Color::Black, PieceType::Pawn) {
        hash ^= zobrist::piece_square_key(Color::Black, PieceType::Pawn, sq);
    }
    hash
}

impl Context<'_> {
    fn should_stop(&mut self) -> bool {
        if self.aborted {
            return true;
        }
        if self.nodes % 2048 == 0 {
            if self.stop.load(Ordering::Relaxed) {
                self.aborted = true;
                return true;
            }
            if let Some(deadline) = self.hard_deadline {
                if Instant::now() >= deadline {
                    self.aborted = true;
                    return true;
                }
            }
            if let Some(max_nodes) = self.max_nodes {
                if self.nodes >= max_nodes {
                    self.aborted = true;
                    return true;
                }
            }
        }
        false
    }

    fn is_repetition(&self, hash: u64) -> bool {
        self.path.iter().filter(|&&h| h == hash).count() >= 2
    }

    fn store_killer(&mut self, ply: u32, mv: Move) {
        let slot = &mut self.killers[ply as usize];
        if slot[0] != Some(mv) {
            slot[1] = slot[0];
            slot[0] = Some(mv);
        }
    }

    fn killers_at(&self, ply: u32) -> [Option<Move>; 2] {
        self.killers[ply as usize]
    }

    fn bump_history(&mut self, mv: Move, depth: u32) {
        self.history[mv.from.0 as usize][mv.to.0 as usize] += (depth * depth) as i32;
    }

    /// Halves every history/continuation-history entry. Called once per
    /// iterative-deepening depth (see `search()`): without this, scores
    /// from shallow early iterations would keep accumulating forever
    /// alongside deeper, more trustworthy ones, making move ordering
    /// slower to react to what the current depth is actually finding.
    fn decay_history(&mut self) {
        for row in &mut self.history {
            for v in row {
                *v /= 2;
            }
        }
        for v in &mut self.cont_history {
            *v /= 2;
        }
    }

    fn record_move_played(&mut self, ply: u32, piece: PieceType, to: Square) {
        self.moves_played[ply as usize] = Some((piece, to));
    }

    fn prev_move_at(&self, ply: u32) -> Option<(PieceType, Square)> {
        self.moves_played[ply as usize]
    }

    fn cont_history_score(&self, ply: u32, piece: PieceType, to: Square) -> i32 {
        match self.prev_move_at(ply) {
            Some((prev_piece, prev_to)) => self.cont_history[cont_history_index(prev_piece, prev_to, piece, to)],
            None => 0,
        }
    }

    fn bump_cont_history(&mut self, ply: u32, piece: PieceType, to: Square, depth: u32) {
        if let Some((prev_piece, prev_to)) = self.prev_move_at(ply) {
            self.cont_history[cont_history_index(prev_piece, prev_to, piece, to)] += (depth * depth) as i32;
        }
    }

    fn correction_score(&self, pawn_hash: u64) -> i32 {
        self.pawn_correction[pawn_hash as usize & (CORRECTION_HISTORY_SIZE - 1)]
    }

    /// Nudges the stored correction for `pawn_hash` toward `error` (how
    /// far this node's real search score ended up from its static eval),
    /// via an exponential moving average weighted by how trustworthy this
    /// particular result was (deeper searches move it further).
    fn update_correction(&mut self, pawn_hash: u64, error: i32, depth: u32) {
        let idx = pawn_hash as usize & (CORRECTION_HISTORY_SIZE - 1);
        let weight = (depth as i32 + 1).min(CORRECTION_WEIGHT_CAP);
        let entry = &mut self.pawn_correction[idx];
        let blended = *entry * (CORRECTION_WEIGHT_SCALE - weight) + error * weight;
        *entry = (blended / CORRECTION_WEIGHT_SCALE).clamp(-CORRECTION_MAX, CORRECTION_MAX);
    }
}

/// Soft/hard time budget for one `go` call. The soft deadline gates
/// whether iterative deepening starts another depth at all (a fresh depth
/// can easily take several times longer than the last one, so it's not
/// worth starting one so late that it would badly overrun); the hard
/// deadline is the absolute cutoff enforced mid-search by `should_stop`.
/// For a fixed `movetime`, both are the same instant: the caller asked for
/// an exact budget, not a target.
struct TimeBudget {
    soft: Instant,
    hard: Instant,
}

fn compute_time_budget(limits: &SearchLimits, side: Color, start: Instant) -> Option<TimeBudget> {
    if limits.infinite {
        return None;
    }
    if let Some(mt) = limits.move_time_ms {
        let deadline = start + Duration::from_millis(mt);
        return Some(TimeBudget { soft: deadline, hard: deadline });
    }
    let (time_left, inc) = match side {
        Color::White => (limits.white_time_ms, limits.white_inc_ms.unwrap_or(0)),
        Color::Black => (limits.black_time_ms, limits.black_inc_ms.unwrap_or(0)),
    };
    time_left.map(|t| {
        let raw_budget = t / 20 + inc / 2;
        let safe_cap = t.saturating_sub(50);
        let soft_ms = raw_budget.min(safe_cap).max(1);
        let hard_ms = (soft_ms * 3).min(safe_cap).max(soft_ms);
        TimeBudget {
            soft: start + Duration::from_millis(soft_ms),
            hard: start + Duration::from_millis(hard_ms),
        }
    })
}

/// Runs iterative deepening from `board`'s position until `limits`/`stop`
/// say to quit, calling `on_iteration` after every completed depth so the
/// caller can report UCI `info` lines as the search progresses. `tt` is
/// caller-owned so it can persist across moves within the same game instead
/// of being rebuilt from scratch on every call. `game_history` is the
/// Zobrist hash of every position actually reached earlier in the real
/// game (not including `board` itself), so `Context::is_repetition` can
/// recognize a line that repeats a position from before this `go` call —
/// not just one that repeats within the search tree currently being
/// explored.
pub fn search(
    board: &Board,
    limits: SearchLimits,
    stop: &AtomicBool,
    tt: &Tt,
    game_history: &[u64],
    mut on_iteration: impl FnMut(&SearchResult, Duration),
) -> SearchResult {
    let start = Instant::now();
    let mut working = board.clone();
    let budget = compute_time_budget(&limits, working.side_to_move, start);
    let max_depth = limits
        .max_depth
        .unwrap_or(if limits.infinite || budget.is_some() { MAX_PLY } else { 6 })
        .clamp(1, MAX_PLY);

    let mut path = Vec::with_capacity(game_history.len() + 1);
    path.extend_from_slice(game_history);
    path.push(working.hash);

    let mut tt_guard = tt.0.lock().unwrap();
    let mut ctx = Context {
        nodes: 0,
        stop,
        hard_deadline: budget.as_ref().map(|b| b.hard),
        max_nodes: limits.max_nodes,
        search_moves: limits.search_moves.clone(),
        path,
        aborted: false,
        tt: &mut tt_guard,
        killers: vec![[None, None]; MAX_PLY as usize],
        history: [[0; 64]; 64],
        static_evals: vec![None; MAX_PLY as usize],
        moves_played: vec![None; (MAX_PLY + 1) as usize],
        cont_history: vec![0; PIECE_TYPE_COUNT * 64 * PIECE_TYPE_COUNT * 64],
        pawn_correction: vec![0; CORRECTION_HISTORY_SIZE],
    };

    let mut result = SearchResult::default();

    for depth in 1..=max_depth {
        // Don't even start a deeper iteration once past the soft budget:
        // the next depth is typically several times more expensive than
        // the last, so starting late just means overshooting further
        // before the hard deadline catches it.
        if depth > 1 {
            if let Some(b) = &budget {
                if Instant::now() >= b.soft {
                    break;
                }
            }
            ctx.decay_history();
        }

        let (score, best_move) = search_root_with_aspiration(&mut working, depth, result.score, &mut ctx);

        if best_move.is_none() {
            // No legal moves at the root at all: checkmate or stalemate.
            result = SearchResult { best_move: None, score, depth, nodes: ctx.nodes, pv: Vec::new() };
            break;
        }
        if ctx.aborted && depth > 1 {
            break; // discard the unfinished iteration, keep the previous one
        }

        let pv = extract_pv(ctx.tt, &working, depth);
        result = SearchResult { best_move, score, depth, nodes: ctx.nodes, pv };
        on_iteration(&result, start.elapsed());

        if ctx.aborted || score.abs() >= MATE_THRESHOLD {
            break;
        }
    }
    result
}

/// Searches `depth` with a narrow window centered on `prev_score` (the
/// previous iteration's result), re-searching with the full (-INF, INF)
/// window on the rare occasions the guess was wrong. Most iterations stay
/// inside the narrow window, which lets alpha-beta prune far more
/// aggressively than a wide-open window would.
fn search_root_with_aspiration(board: &mut Board, depth: u32, prev_score: i32, ctx: &mut Context) -> (i32, Option<Move>) {
    if depth <= 2 {
        return search_root(board, depth, -INF, INF, ctx);
    }
    let alpha = prev_score.saturating_sub(ASPIRATION_WINDOW).max(-INF);
    let beta = prev_score.saturating_add(ASPIRATION_WINDOW).min(INF);
    let (score, best_move) = search_root(board, depth, alpha, beta, ctx);
    if !ctx.aborted && (score <= alpha || score >= beta) {
        return search_root(board, depth, -INF, INF, ctx);
    }
    (score, best_move)
}

fn search_root(board: &mut Board, depth: u32, alpha_init: i32, beta: i32, ctx: &mut Context) -> (i32, Option<Move>) {
    let mut moves = movegen::generate_legal_moves(board);
    if let Some(restrict_to) = &ctx.search_moves {
        // If none of the requested moves are actually legal here, fall
        // back to the full legal list rather than reporting a spurious
        // checkmate/stalemate below.
        let restricted: Vec<Move> = moves.iter().copied().filter(|m| restrict_to.contains(m)).collect();
        if !restricted.is_empty() {
            moves = restricted;
        }
    }
    if moves.is_empty() {
        let score = if movegen::is_in_check(board, board.side_to_move) {
            -MATE_SCORE
        } else {
            0
        };
        return (score, None);
    }

    let tt_move = ctx.tt.probe(board.hash).and_then(|e| e.best_move);
    let mut ordered = moves;
    order_moves_full(board, &mut ordered, tt_move, [None, None], ctx, 0);

    let in_check = movegen::is_in_check(board, board.side_to_move);
    let child_depth = depth - 1 + if in_check { 1 } else { 0 };

    let mut alpha = alpha_init;
    let mut best_score = -INF;
    let mut best_move = ordered[0];
    // Every root move whose score came from a genuine full-window search
    // (the first move, or any later move whose null-window scout beat
    // alpha and got re-searched) rather than just a fail-low scout bound.
    // At the end, picking randomly among the ones within a few centipawns
    // of the best is what gives the engine some variety between otherwise
    // near-equal moves instead of always playing the exact same one —
    // exact ties alone turn out to be rare even from the start position,
    // since mobility/PST almost always break them by a point or two.
    let mut exact_candidates: Vec<(Move, i32)> = Vec::new();

    for (move_index, mv) in ordered.into_iter().enumerate() {
        let moved_piece = board.piece_at(mv.from).map(|p| p.kind).unwrap_or(PieceType::Pawn);
        let undo = board.make_move(mv);
        ctx.path.push(board.hash);
        ctx.record_move_played(1, moved_piece, mv.to);
        let (score, is_exact) = if move_index == 0 {
            (-negamax(board, child_depth, 1, -beta, -alpha, ctx, None), true)
        } else {
            let scout = -negamax(board, child_depth, 1, -alpha - 1, -alpha, ctx, None);
            if scout > alpha && !ctx.aborted {
                (-negamax(board, child_depth, 1, -beta, -alpha, ctx, None), true)
            } else {
                (scout, false)
            }
        };
        ctx.path.pop();
        board.unmake_move(mv, undo);

        if is_exact {
            exact_candidates.push((mv, score));
        }
        if score > best_score {
            best_score = score;
            best_move = mv;
        }
        if best_score > alpha {
            alpha = best_score;
        }

        if ctx.aborted {
            break;
        }
    }

    let near_best: Vec<Move> = exact_candidates
        .into_iter()
        .filter(|&(_, score)| best_score - score <= ROOT_TIE_EPSILON)
        .map(|(mv, _)| mv)
        .collect();
    if near_best.len() > 1 {
        best_move = near_best[random_index(near_best.len())];
    }

    if !ctx.aborted {
        ctx.tt.store(board.hash, depth as u8, score_to_tt(best_score, 0), TtFlag::Exact, Some(best_move));
    }

    (best_score, Some(best_move))
}

/// Tiny, dependency-free PRNG (seeded once from the system clock, stepped
/// with the same splitmix64 mixing function `zobrist.rs` uses for its
/// constants) used only to break exact ties among root moves. Not
/// cryptographic and not meant to be: its entire job is move variety.
fn random_index(len: usize) -> usize {
    use std::sync::atomic::AtomicU64;
    use std::time::{SystemTime, UNIX_EPOCH};

    static STATE: AtomicU64 = AtomicU64::new(0);

    let mut seed = STATE.load(Ordering::Relaxed);
    if seed == 0 {
        seed = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(0x9E37_79B9_7F4A_7C15)
            | 1;
    }
    seed = seed.wrapping_add(0x9E37_79B9_7F4A_7C15);
    STATE.store(seed, Ordering::Relaxed);

    let mut z = seed;
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^= z >> 31;

    (z % len as u64) as usize
}

fn negamax(board: &mut Board, depth: u32, ply: u32, mut alpha: i32, beta: i32, ctx: &mut Context, excluded: Option<Move>) -> i32 {
    ctx.nodes += 1;
    if ctx.should_stop() {
        return 0;
    }

    if board.halfmove_clock >= 100 || ctx.is_repetition(board.hash) || is_insufficient_material(board) {
        return 0;
    }

    // Check extensions (below) let a single line of forced checks push ply
    // past what `depth` alone would predict; bail out to a static eval
    // rather than recurse further, which both bounds worst-case recursion
    // and keeps `ctx.killers_at(ply)` in bounds (it's sized to MAX_PLY).
    if ply >= MAX_PLY {
        return eval::evaluate_relative(board);
    }

    let alpha_orig = alpha;
    // While verifying a singular-extension candidate (`excluded` is set),
    // this same position's TT entry reflects the *unrestricted* move set
    // — including the very move being excluded — so it can't be used as a
    // cutoff for this restricted search. `tt_move` (and the entry, for the
    // singular-extension check below) are still read normally either way.
    let tt_entry = ctx.tt.probe(board.hash);
    let tt_move = tt_entry.and_then(|e| e.best_move);
    if excluded.is_none() {
        if let Some(entry) = tt_entry {
            if entry.depth as u32 >= depth {
                let score = score_from_tt(entry.score, ply);
                match entry.flag {
                    TtFlag::Exact => return score,
                    TtFlag::Lower => {
                        if score >= beta {
                            return score;
                        }
                        if score > alpha {
                            alpha = score;
                        }
                    }
                    TtFlag::Upper => {
                        if score <= alpha {
                            return score;
                        }
                    }
                }
            }
        }
    }

    if depth == 0 {
        return quiescence(board, alpha, beta, ply, ctx);
    }

    let in_check = movegen::is_in_check(board, board.side_to_move);

    // Static eval of this node, corrected by the learned pawn-structure
    // adjustment (see `Context::correction_score`), cached per ply so
    // later plies can compare against it (that's what "improving" means
    // below), and reused by razoring/RFP/futility instead of recomputing.
    // None while in check: the static eval is meaningless there and none
    // of these pruning techniques apply to check positions anyway.
    let node_pawn_hash = if in_check { None } else { Some(pawn_hash(board)) };
    // Kept separate from `static_eval` (the corrected version used for
    // pruning below): the correction update at the end of this node needs
    // the gap between the search result and the *raw* eval, not the
    // already-corrected one, or successive updates would partly correct
    // against themselves instead of converging on the true bias.
    let raw_eval = node_pawn_hash.map(|_| eval::evaluate_relative(board));
    let static_eval = match (raw_eval, node_pawn_hash) {
        (Some(re), Some(ph)) => Some(re + ctx.correction_score(ph)),
        _ => None,
    };
    ctx.static_evals[ply as usize] = static_eval;

    // "Improving": is this node's static eval better than the same side's
    // static eval two plies ago (the last time it was to move)? If so,
    // pruning margins below can afford to be a bit more conservative,
    // since the position seems to be getting better on its own already.
    let improving = match (static_eval, ply >= 2) {
        (Some(se), true) => ctx.static_evals[ply as usize - 2].is_some_and(|prev| se > prev),
        _ => false,
    };

    // Razoring: if the static eval is so far below alpha that no ordinary
    // move could plausibly make up the difference, a quiescence search is
    // enough to confirm that instead of the full move loop. Falls through
    // to normal search on the rare chance the quiescence score clears
    // alpha anyway (a tactical shot the static eval didn't see coming).
    if let Some(se) = static_eval {
        if depth <= RAZOR_MAX_DEPTH && se + RAZOR_MARGIN_PER_DEPTH * depth as i32 <= alpha {
            let razor_score = quiescence(board, alpha, beta, ply, ctx);
            if razor_score <= alpha {
                return razor_score;
            }
        }
    }

    // Reverse futility pruning (a.k.a. static null-move pruning): if the
    // static eval already beats beta by more than a depth-scaled margin,
    // assume a real search would too and cut here without exploring any
    // moves at all. The margin shrinks by one depth's worth when the
    // position isn't improving, since a stagnant/worsening eval is a
    // weaker signal that the true score is really this high.
    if let Some(se) = static_eval {
        if depth <= RFP_MAX_DEPTH && beta < MATE_THRESHOLD {
            let margin = RFP_MARGIN_PER_DEPTH * (depth as i32 - if improving { 0 } else { 1 }).max(0);
            if se - margin >= beta {
                return se;
            }
        }
    }

    // Null-move pruning: if we can skip a move entirely and the opponent
    // still can't beat beta, our real move is likely to be even better, so
    // this position is probably not worth searching further. Guarded
    // against check (illegal to "pass" then) and against zugzwang-prone
    // positions (only king and pawns left), where giving up the move can
    // artificially flip the evaluation.
    if !in_check
        && depth >= NULL_MOVE_MIN_DEPTH
        && beta < MATE_THRESHOLD
        && has_non_pawn_material(board, board.side_to_move)
    {
        let undo = board.make_null_move();
        ctx.path.push(board.hash);
        let score = -negamax(board, depth - 1 - NULL_MOVE_REDUCTION, ply + 1, -beta, -beta + 1, ctx, None);
        ctx.path.pop();
        board.unmake_null_move(undo);
        if !ctx.aborted && score >= beta {
            return beta;
        }
    }

    // Internal iterative reduction: with no TT move to try first, this
    // node's move ordering is probably weaker than usual (no proven-good
    // move to search ahead of everything else), so treat it as one ply
    // shallower for the rest of this node — child_depth, LMR/futility
    // margins, and the depth this node's own result gets stored under all
    // follow from `depth` below. A later, deeper visit to this position
    // will have a TT move by then and search at full strength.
    let depth = if tt_move.is_none() && depth >= IIR_MIN_DEPTH { depth - 1 } else { depth };

    let moves = movegen::generate_legal_moves(board);
    if moves.is_empty() {
        return if in_check { -MATE_SCORE + ply as i32 } else { 0 };
    }

    let killers = ctx.killers_at(ply);
    let mut ordered = moves;
    order_moves_full(board, &mut ordered, tt_move, killers, ctx, ply);

    // Check extension: a position where the side to move is in check is
    // forcing (few replies, tactics often hiding just beyond the horizon),
    // so search it one ply deeper instead of letting `depth` run out here.
    let child_depth = depth - 1 + if in_check { 1 } else { 0 };

    // Singular extensions: if a reduced-depth search of every move *except*
    // the TT move can't even get close to the TT move's own score, the TT
    // move is probably the only thing holding this position together (a
    // forced sequence) — worth searching one ply deeper. Guarded by
    // `excluded.is_none()` so the verification search itself (which visits
    // this same position with the TT move excluded) can't recursively
    // trigger another one.
    let mut tt_move_extension = 0;
    if excluded.is_none() {
        if let (Some(mv), Some(entry)) = (tt_move, tt_entry) {
            let tt_score = score_from_tt(entry.score, ply);
            if depth >= SINGULAR_MIN_DEPTH
                && entry.depth as u32 + SINGULAR_TT_DEPTH_MARGIN >= depth
                && entry.flag != TtFlag::Upper
                && tt_score.abs() < MATE_THRESHOLD
            {
                let singular_beta = tt_score - SINGULAR_MARGIN_PER_DEPTH * depth as i32;
                let verification_depth = depth.saturating_sub(1) / 2;
                let score = negamax(board, verification_depth, ply, singular_beta - 1, singular_beta, ctx, Some(mv));
                if !ctx.aborted && score < singular_beta {
                    tt_move_extension = 1;
                }
            }
        }
    }

    let mut best_score = -INF;
    let mut best_move = ordered[0];

    // Futility pruning: at these shallow depths, if the static eval is
    // already so far below alpha that no quiet move could plausibly close
    // the gap, skip searching quiet moves altogether (captures/promotions
    // can still swing the material count enough to matter, so they're
    // exempt). Disabled near mate scores, where the static eval is
    // meaningless and pruning could hide a forced mate.
    let futility_eval = static_eval.filter(|_| depth <= FUTILITY_MAX_DEPTH && alpha > -MATE_THRESHOLD && beta < MATE_THRESHOLD);

    for (move_index, mv) in ordered.into_iter().enumerate() {
        if Some(mv) == excluded {
            continue;
        }

        let is_quiet = !mv.is_capture() && mv.promotion().is_none();

        if move_index > 0 && is_quiet {
            if let Some(se) = futility_eval {
                if se + FUTILITY_MARGIN[depth as usize] <= alpha {
                    continue;
                }
            }
        }

        let moved_piece = board.piece_at(mv.from).map(|p| p.kind).unwrap_or(PieceType::Pawn);
        let mv_child_depth = if Some(mv) == tt_move { child_depth + tt_move_extension } else { child_depth };
        let undo = board.make_move(mv);
        ctx.path.push(board.hash);
        ctx.record_move_played(ply + 1, moved_piece, mv.to);

        // Late move reductions: moves this far down an already-good
        // ordering, that are quiet and not a reply to/giver of check, are
        // unlikely to be the best move, so search them shallower first and
        // only pay for a full-depth re-search if they beat alpha anyway.
        let can_reduce = move_index >= LMR_FULL_DEPTH_MOVES
            && depth >= LMR_MIN_DEPTH
            && is_quiet
            && !in_check
            && !movegen::is_in_check(board, board.side_to_move);
        let reduction = if can_reduce { LMR_REDUCTION } else { 0 };

        let score = if move_index == 0 {
            -negamax(board, mv_child_depth, ply + 1, -beta, -alpha, ctx, None)
        } else {
            let reduced_depth = mv_child_depth.saturating_sub(reduction);
            let mut s = -negamax(board, reduced_depth, ply + 1, -alpha - 1, -alpha, ctx, None);
            if reduction > 0 && s > alpha && !ctx.aborted {
                s = -negamax(board, mv_child_depth, ply + 1, -alpha - 1, -alpha, ctx, None);
            }
            if s > alpha && s < beta && !ctx.aborted {
                s = -negamax(board, mv_child_depth, ply + 1, -beta, -alpha, ctx, None);
            }
            s
        };
        ctx.path.pop();
        board.unmake_move(mv, undo);

        if score > best_score {
            best_score = score;
            best_move = mv;
        }
        if best_score > alpha {
            alpha = best_score;
        }
        if alpha >= beta {
            if !mv.is_capture() {
                ctx.store_killer(ply, mv);
                ctx.bump_history(mv, depth);
                ctx.bump_cont_history(ply, moved_piece, mv.to, depth);
            }
            break;
        }
        if ctx.aborted {
            break;
        }
    }

    // Neither the TT store nor the correction-history update below applies
    // to a singular-extension verification search: it explored a
    // deliberately restricted move set (the TT move excluded) at a
    // half-depth window, so its result isn't a valid bound for this
    // position's *real* value and would only pollute both tables.
    if !ctx.aborted && excluded.is_none() {
        // Feed this node's outcome back into the correction history: how
        // far off was the static eval from what the search actually
        // found? Skipped near mate scores, where that gap is meaningless
        // (it reflects distance-to-mate, not a misjudged position).
        if let (Some(re), Some(ph)) = (raw_eval, node_pawn_hash) {
            if best_score.abs() < MATE_THRESHOLD {
                ctx.update_correction(ph, best_score - re, depth);
            }
        }

        let flag = if best_score <= alpha_orig {
            TtFlag::Upper
        } else if best_score >= beta {
            TtFlag::Lower
        } else {
            TtFlag::Exact
        };
        ctx.tt.store(board.hash, depth as u8, score_to_tt(best_score, ply), flag, Some(best_move));
    }

    best_score
}

fn quiescence(board: &mut Board, mut alpha: i32, beta: i32, ply: u32, ctx: &mut Context) -> i32 {
    ctx.nodes += 1;
    if ctx.should_stop() {
        return 0;
    }
    if ply >= MAX_PLY {
        return eval::evaluate_relative(board);
    }

    let in_check = movegen::is_in_check(board, board.side_to_move);

    // When in check there is no "stand pat": the side to move might be
    // getting mated, and refusing to at least try every evasion (not just
    // captures) would let a mate hiding at the search horizon evaluate as
    // an ordinary, possibly favorable, static score instead.
    let stand_pat = if in_check {
        -MATE_SCORE + ply as i32
    } else {
        eval::evaluate_relative(board)
    };
    if !in_check {
        if stand_pat >= beta {
            return stand_pat;
        }
        if stand_pat > alpha {
            alpha = stand_pat;
        }
    }

    let mut moves: Vec<Move> = if in_check {
        // In check: every legal reply is a candidate, not just captures —
        // a quiet evasion can be the only way out of a mating net.
        movegen::generate_legal_moves(board)
    } else {
        // Skip captures that lose material outright (negative SEE): they
        // essentially never help resolve a tactical sequence, and reading
        // them out wastes a large fraction of quiescence search's node
        // budget. Quiet promotions are included too (not just captures):
        // a pawn one push from queening is exactly the kind of "loud" move
        // that must not go unresolved at the horizon.
        movegen::generate_legal_moves(board)
            .into_iter()
            .filter(|m| (m.is_capture() || m.promotion().is_some()) && movegen::static_exchange_eval(board, *m) >= 0)
            .collect()
    };

    if in_check && moves.is_empty() {
        // No legal evasion: this is checkmate, exactly `ply` plies from the
        // root that called into this quiescence chain.
        return -MATE_SCORE + ply as i32;
    }

    order_moves_in_place(board, &mut moves);

    for mv in moves {
        // Delta pruning: even winning this capture outright can't close
        // enough of the gap to alpha to matter, so don't bother reading it
        // out. Promotions are exempt since the extra promoted-piece value
        // isn't accounted for by `capture_victim_value` alone. Doesn't
        // apply while in check: every move here is a forced evasion, not
        // an optional tactical shot to prune away.
        if !in_check && mv.promotion().is_none() && stand_pat + capture_victim_value(board, mv) + DELTA_PRUNING_MARGIN <= alpha {
            continue;
        }

        let undo = board.make_move(mv);
        let score = -quiescence(board, -beta, -alpha, ply + 1, ctx);
        board.unmake_move(mv, undo);

        if score > alpha {
            alpha = score;
        }
        if alpha >= beta || ctx.aborted {
            break;
        }
    }
    alpha
}

/// True if `color` has any piece besides pawns and the king, i.e. it is
/// safe to try a null move without risking a zugzwang position where
/// "passing" looks better than every legal move.
fn has_non_pawn_material(board: &Board, color: Color) -> bool {
    PieceType::ALL
        .iter()
        .any(|&kind| !matches!(kind, PieceType::Pawn | PieceType::King) && !board.pieces_of(color, kind).is_empty())
}

/// A position where no sequence of legal moves, played by either side no
/// matter how badly, could ever produce checkmate: no pawns left to
/// promote into fresh material, and at most one minor piece on the whole
/// board. This stays deliberately conservative — two knights, two bishops,
/// or a bishop pair can force mate in at least some lines, so those are
/// left for the search to work out on its own rather than risk misjudging
/// a position that's actually still winnable as an automatic draw.
fn is_insufficient_material(board: &Board) -> bool {
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

fn capture_victim_value(board: &Board, mv: Move) -> i32 {
    if mv.flag == MoveFlag::EnPassant {
        eval::piece_value(crate::types::PieceType::Pawn)
    } else {
        board.piece_at(mv.to).map(|p| eval::piece_value(p.kind)).unwrap_or(0)
    }
}

/// MVV-LVA-ish move ordering: captures first (biggest victim / smallest
/// attacker first), quiet moves after in whatever order they were generated.
/// Used by quiescence search, which never sees killers, TT moves or history.
fn move_order_score(board: &Board, mv: Move) -> i32 {
    if mv.is_capture() {
        let victim = capture_victim_value(board, mv);
        let attacker = board.piece_at(mv.from).map(|p| eval::piece_value(p.kind)).unwrap_or(0);
        10_000 + victim * 10 - attacker
    } else {
        0
    }
}

fn order_moves_in_place(board: &Board, moves: &mut [Move]) {
    moves.sort_by_key(|&m| std::cmp::Reverse(move_order_score(board, m)));
}

/// Full move ordering for the main search: the transposition-table move
/// first (it was good enough to be stored, so try it before anything
/// else), then captures ranked by MVV-LVA and nudged by their SEE value,
/// then killer moves, then quiet moves ranked by flat history plus
/// continuation history (how well this move has done specifically as a
/// reply to whatever move preceded it — a from/to-only table can't tell
/// "good in general" from "good only as a recapture here"). Captures that
/// actually lose material (negative SEE, e.g. a "capture" that just hangs
/// the piece to a defender) are pushed below every quiet move instead:
/// MVV-LVA alone can't tell a winning capture from a losing one, since it
/// doesn't know whether the target square is defended.
fn move_order_score_full(board: &Board, mv: Move, tt_move: Option<Move>, killers: [Option<Move>; 2], ctx: &Context, ply: u32) -> i32 {
    if tt_move == Some(mv) {
        return 1_000_000;
    }
    if mv.is_capture() {
        let see = movegen::static_exchange_eval(board, mv);
        if see >= 0 {
            let victim = capture_victim_value(board, mv);
            let attacker = board.piece_at(mv.from).map(|p| eval::piece_value(p.kind)).unwrap_or(0);
            return 100_000 + victim * 10 - attacker + see;
        }
        return see - 50_000;
    }
    if killers[0] == Some(mv) {
        return 90_000;
    }
    if killers[1] == Some(mv) {
        return 89_000;
    }
    let piece = board.piece_at(mv.from).map(|p| p.kind).unwrap_or(PieceType::Pawn);
    ctx.history[mv.from.0 as usize][mv.to.0 as usize] + ctx.cont_history_score(ply, piece, mv.to)
}

fn order_moves_full(board: &Board, moves: &mut [Move], tt_move: Option<Move>, killers: [Option<Move>; 2], ctx: &Context, ply: u32) {
    moves.sort_by_key(|&m| std::cmp::Reverse(move_order_score_full(board, m, tt_move, killers, ctx, ply)));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::board::Board;

    fn search_to_depth(fen: &str, depth: u32) -> SearchResult {
        let board = Board::from_fen(fen).unwrap();
        let stop = AtomicBool::new(false);
        let tt = Tt::new(1);
        let limits = SearchLimits { max_depth: Some(depth), ..Default::default() };
        search(&board, limits, &stop, &tt, &[], |_, _| {})
    }

    #[test]
    fn finds_mate_in_one() {
        // Scholar's mate: 1.e4 e5 2.Bc4 Nc6 3.Qh5 Nf6??, White to move.
        // Qxf7# is mate because the bishop on c4 defends f7 too, so the
        // king cannot recapture.
        let result = search_to_depth(
            "r1bqkb1r/pppp1ppp/2n2n2/4p2Q/2B1P3/8/PPPP1PPP/RN2KBNR w KQkq - 4 4",
            3,
        );
        assert_eq!(result.best_move.map(|m| m.to_string()), Some("h5f7".to_string()));
        assert!(result.score >= MATE_SCORE - 10);
    }

    #[test]
    fn returns_a_move_in_a_lopsided_endgame() {
        let board = Board::from_fen("7k/8/8/8/8/2K5/8/3Q4 w - - 0 1").unwrap();
        let stop = AtomicBool::new(false);
        let tt = Tt::new(1);
        let limits = SearchLimits { max_depth: Some(3), ..Default::default() };
        let result = search(&board, limits, &stop, &tt, &[], |_, _| {});
        assert!(result.best_move.is_some());
    }

    #[test]
    fn returns_none_at_checkmated_root() {
        let result = search_to_depth("4R1k1/5ppp/8/8/8/8/8/4K3 b - - 0 1", 3);
        assert_eq!(result.best_move, None);
        assert_eq!(result.score, -MATE_SCORE);
    }

    #[test]
    fn returns_a_legal_move_from_startpos() {
        let result = search_to_depth(crate::board::STARTPOS_FEN, 3);
        let board = Board::start_pos();
        let legal = movegen::generate_legal_moves(&board);
        let played = result.best_move.expect("startpos siempre tiene jugadas legales");
        assert!(legal.contains(&played));
    }

    #[test]
    fn stops_promptly_when_stop_flag_is_set() {
        let board = Board::start_pos();
        let stop = AtomicBool::new(true); // already stopped
        let tt = Tt::new(1);
        let limits = SearchLimits { max_depth: Some(64), ..Default::default() };
        let result = search(&board, limits, &stop, &tt, &[], |_, _| {});
        // Even with the flag pre-set, depth 1 always completes so we still
        // get a legal move back. How many depths finish before the node
        // counter crosses the stop-flag check granularity depends on how
        // efficient move ordering/pruning are (TT and PVS make each depth
        // cheaper), so pin the assertion to "aborted well before the depth
        // limit" rather than to a specific depth.
        assert!(result.best_move.is_some());
        assert!(result.depth < 64);
    }

    #[test]
    fn respects_a_tight_wtime_budget_without_wild_overrun() {
        // wtime 200ms -> soft ~10ms, hard ~30ms (see compute_time_budget).
        // The hard deadline must actually be enforced, not just the soft
        // one (which only gates *starting* a new depth).
        let board = Board::start_pos();
        let stop = AtomicBool::new(false);
        let tt = Tt::new(1);
        let limits = SearchLimits { white_time_ms: Some(200), ..Default::default() };
        let start = Instant::now();
        let result = search(&board, limits, &stop, &tt, &[], |_, _| {});
        assert!(result.best_move.is_some());
        assert!(start.elapsed() < Duration::from_millis(500));
    }

    #[test]
    fn respects_a_short_movetime_budget() {
        let board = Board::start_pos();
        let stop = AtomicBool::new(false);
        let tt = Tt::new(1);
        let limits = SearchLimits {
            move_time_ms: Some(50),
            ..Default::default()
        };
        let start = Instant::now();
        let result = search(&board, limits, &stop, &tt, &[], |_, _| {});
        assert!(result.best_move.is_some());
        assert!(start.elapsed() < Duration::from_millis(1000));
    }

    #[test]
    fn has_non_pawn_material_detects_only_king_and_pawns() {
        let kp_only = Board::from_fen("4k3/8/8/8/8/8/4P3/4K3 w - - 0 1").unwrap();
        assert!(!has_non_pawn_material(&kp_only, Color::White));
        let with_knight = Board::from_fen("4k3/8/8/8/8/8/4P3/3NK3 w - - 0 1").unwrap();
        assert!(has_non_pawn_material(&with_knight, Color::White));
    }

    #[test]
    fn is_insufficient_material_detects_bare_kings_and_lone_minors() {
        let bare_kings = Board::from_fen("4k3/8/8/8/8/8/8/4K3 w - - 0 1").unwrap();
        assert!(is_insufficient_material(&bare_kings));
        let lone_knight = Board::from_fen("4k3/8/8/8/8/8/8/3NK3 w - - 0 1").unwrap();
        assert!(is_insufficient_material(&lone_knight));
        let lone_bishop = Board::from_fen("4k3/8/8/8/8/8/8/3BK3 w - - 0 1").unwrap();
        assert!(is_insufficient_material(&lone_bishop));
        // Two bishops (or any other pairing of minors) can, at least in
        // some lines, force checkmate, so they must not be swept into the
        // same automatic draw.
        let two_bishops = Board::from_fen("4k3/8/8/8/8/8/8/2B1KB2 w - - 0 1").unwrap();
        assert!(!is_insufficient_material(&two_bishops));
        let with_pawn = Board::from_fen("4k3/8/8/8/8/8/4P3/4K3 w - - 0 1").unwrap();
        assert!(!is_insufficient_material(&with_pawn));
    }

    #[test]
    fn search_scores_bare_kings_as_a_dead_draw() {
        let result = search_to_depth("4k3/8/8/8/8/8/8/4K3 w - - 0 1", 4);
        assert_eq!(result.score, 0);
    }

    #[test]
    fn search_recognizes_that_capturing_the_last_pawn_leaves_insufficient_material() {
        // White Ka1, Pe4 (undefended); Black Ke5 to move. This is a proven
        // draw (see eval::kpk_exact_score) however Black plays it, but
        // before this fix, a line that captured the pawn and then wandered
        // a few more plies through bare-king king-square-table noise could
        // leak a small nonzero score instead of a clean draw.
        let result = search_to_depth("8/8/8/4k3/4P3/8/8/K7 b - - 0 1", 4);
        assert_eq!(result.score, 0);
    }

    #[test]
    fn deeper_search_agrees_with_shallower_search_on_a_forced_mate() {
        // The TT/PVS/null-move/LMR machinery must not corrupt a forced
        // line: whatever depth we search a known mate-in-2 to, it should
        // still find it.
        let fen = "6k1/5ppp/8/8/8/8/8/R3R1K1 w - - 0 1";
        for depth in [3, 4, 5, 6, 8] {
            let result = search_to_depth(fen, depth);
            assert!(
                result.score >= MATE_SCORE - 10,
                "depth {depth} did not find the forced mate, score = {}",
                result.score
            );
        }
    }

    #[test]
    fn finding_the_same_position_twice_via_tt_does_not_change_the_best_move() {
        // Play the same position through the search twice (as would happen
        // via transposition) and make sure the TT-cached result is
        // consistent. The *score* must always match exactly (it doesn't
        // depend on the root's random tie-break among near-equal moves,
        // only `best_move` does — see `search_root`'s `near_best`
        // selection), so that's the real TT-consistency invariant;
        // `best_move` itself is only checked for legality, since a
        // genuine near-tie at the root can legitimately resolve to a
        // different (equally good) move on each call.
        let board = Board::start_pos();
        let stop = AtomicBool::new(false);
        let tt = Tt::new(1);
        let limits = SearchLimits { max_depth: Some(4), ..Default::default() };
        let first = search(&board, limits.clone(), &stop, &tt, &[], |_, _| {});
        let second = search(&board, limits, &stop, &tt, &[], |_, _| {});
        assert_eq!(first.score, second.score);

        let legal = movegen::generate_legal_moves(&board);
        for result in [&first, &second] {
            let mv = result.best_move.expect("startpos siempre tiene jugadas legales");
            assert!(legal.contains(&mv));
        }
    }

    #[test]
    fn cont_history_index_is_distinct_per_combination() {
        let a = cont_history_index(PieceType::Pawn, Square::new(0, 0), PieceType::Knight, Square::new(2, 1));
        let b = cont_history_index(PieceType::Pawn, Square::new(0, 0), PieceType::Knight, Square::new(2, 2));
        let c = cont_history_index(PieceType::Queen, Square::new(0, 0), PieceType::Knight, Square::new(2, 1));
        let d = cont_history_index(PieceType::Pawn, Square::new(0, 1), PieceType::Knight, Square::new(2, 1));
        let e = cont_history_index(PieceType::Pawn, Square::new(0, 0), PieceType::Bishop, Square::new(2, 1));
        let all = [a, b, c, d, e];
        for i in 0..all.len() {
            for j in (i + 1)..all.len() {
                assert_ne!(all[i], all[j], "indices {i} and {j} collided");
            }
        }
        assert!(all.iter().all(|&i| i < PIECE_TYPE_COUNT * 64 * PIECE_TYPE_COUNT * 64));
    }

    #[test]
    fn pawn_hash_depends_only_on_pawn_structure() {
        let a = Board::from_fen("4k3/8/8/8/8/8/4P3/4K3 w - - 0 1").unwrap();
        let same_pawns_extra_queen = Board::from_fen("4k3/8/8/8/8/8/4P3/4KQ2 w - - 0 1").unwrap();
        assert_eq!(pawn_hash(&a), pawn_hash(&same_pawns_extra_queen));

        let pawn_moved = Board::from_fen("4k3/8/8/8/8/8/3P4/4K3 w - - 0 1").unwrap();
        assert_ne!(pawn_hash(&a), pawn_hash(&pawn_moved));
    }

    #[test]
    fn random_index_always_returns_zero_for_a_single_candidate() {
        for _ in 0..20 {
            assert_eq!(random_index(1), 0);
        }
    }

    #[test]
    fn random_index_stays_in_range_and_actually_varies() {
        let mut seen = std::collections::HashSet::new();
        for _ in 0..200 {
            let i = random_index(1000);
            assert!(i < 1000);
            seen.insert(i);
        }
        assert!(seen.len() > 1, "random_index never produced more than one distinct value in 200 draws");
    }

    #[test]
    fn near_tied_root_moves_give_the_engine_some_opening_variety() {
        // At depth 3 from startpos, several opening moves land within
        // ROOT_TIE_EPSILON of each other (verified empirically), so a
        // fresh Tt each time should occasionally surface more than one of
        // them across repeated searches instead of always the same move.
        let board = Board::start_pos();
        let limits = SearchLimits { max_depth: Some(3), ..Default::default() };
        let mut seen = std::collections::HashSet::new();
        for _ in 0..30 {
            let stop = AtomicBool::new(false);
            let tt = Tt::new(1);
            let result = search(&board, limits.clone(), &stop, &tt, &[], |_, _| {});
            seen.insert(result.best_move.map(|m| m.to_string()));
        }
        assert!(seen.len() > 1, "expected more than one distinct opening move across 30 searches, got {seen:?}");
    }

    #[test]
    fn reusing_the_same_tt_across_two_searches_still_finds_the_forced_mate() {
        // Simulates the real usage pattern (one Tt handle kept alive across
        // an entire game): search the same position twice through the same
        // table without clearing it in between, and make sure stale
        // entries from the first pass don't corrupt the second.
        let board = Board::from_fen("6k1/5ppp/8/8/8/8/8/R3R1K1 w - - 0 1").unwrap();
        let stop = AtomicBool::new(false);
        let tt = Tt::new(1);
        let limits = SearchLimits { max_depth: Some(5), ..Default::default() };
        let _ = search(&board, limits.clone(), &stop, &tt, &[], |_, _| {});
        let second = search(&board, limits, &stop, &tt, &[], |_, _| {});
        assert!(second.score >= MATE_SCORE - 10);
    }

    #[test]
    fn quiescence_recognizes_a_back_rank_mate_at_the_search_horizon() {
        // Black's queen on h8 is completely boxed in by its own king (g8)
        // and pawn (h7) — zero legal moves — so Ra1-a8 is a genuine mate
        // despite black being materially way ahead (queen + 3 pawns vs a
        // lone rook). At depth 1 the mating move's child node has no depth
        // left and lands directly in quiescence with black in check (see
        // Autorrevisión Hallazgo 1): quiescence must recognize that as
        // checkmate instead of falling back to a stand-pat eval that
        // favors black's material edge.
        let result = search_to_depth("6kq/5ppp/8/8/8/8/8/R5K1 w - - 0 1", 1);
        assert_eq!(result.best_move.map(|m| m.to_string()), Some("a1a8".to_string()));
        assert!(result.score >= MATE_SCORE - 10, "expected a mate score, got {}", result.score);
    }

    #[test]
    fn decay_history_halves_bumped_entries() {
        let stop = AtomicBool::new(false);
        let tt = Tt::new(1);
        let mut tt_guard = tt.0.lock().unwrap();
        let mut ctx = Context {
            nodes: 0,
            stop: &stop,
            hard_deadline: None,
            max_nodes: None,
            search_moves: None,
            path: vec![0],
            aborted: false,
            tt: &mut tt_guard,
            killers: vec![[None, None]; MAX_PLY as usize],
            history: [[0; 64]; 64],
            static_evals: vec![None; MAX_PLY as usize],
            moves_played: vec![None; (MAX_PLY + 1) as usize],
            cont_history: vec![0; PIECE_TYPE_COUNT * 64 * PIECE_TYPE_COUNT * 64],
            pawn_correction: vec![0; CORRECTION_HISTORY_SIZE],
        };
        let mv = Move::new(Square::new(4, 1), Square::new(4, 3), MoveFlag::DoublePawnPush);
        ctx.bump_history(mv, 10);
        let before = ctx.history[mv.from.0 as usize][mv.to.0 as usize];
        assert!(before > 0);
        ctx.decay_history();
        assert_eq!(ctx.history[mv.from.0 as usize][mv.to.0 as usize], before / 2);
    }

    #[test]
    fn search_result_reports_a_multi_move_principal_variation() {
        // Depth 4 from startpos should yield a PV several moves deep, not
        // just the root's best move: GUIs/analysis tools expect the full
        // line, not a single-move stub.
        let result = search_to_depth(crate::board::STARTPOS_FEN, 4);
        assert_eq!(result.pv.first(), result.best_move.as_ref());
        assert!(result.pv.len() > 1, "expected a multi-move PV, got {:?}", result.pv);

        // The PV must actually be a legal line: replaying it move by move
        // from startpos should never hit an illegal move.
        let mut board = Board::start_pos();
        for mv in &result.pv {
            let legal = movegen::generate_legal_moves(&board);
            assert!(legal.contains(mv), "PV move {mv} illegal in position {}", board.to_fen());
            board.make_move(*mv);
        }
    }

    #[test]
    fn negamax_treats_a_position_from_before_this_go_call_as_a_repetition() {
        // A lone extra queen makes the point unmissable: without
        // recognizing the repetition, this position's material edge would
        // make negamax return a large positive score for White instead of
        // the draw a threefold repetition actually forces.
        let mut board = Board::from_fen("4k3/8/8/8/8/8/8/3QK3 w - - 0 1").unwrap();
        let earlier_hash = board.hash;
        // Shuffle both kings out and back, landing on the exact same
        // position two full moves later — the same way it would happen
        // for real via `position ... moves ...` before this `go` began.
        let m1 = Move::new(Square::new(4, 0), Square::new(5, 0), MoveFlag::Quiet); // Ke1-f1
        let m2 = Move::new(Square::new(4, 7), Square::new(5, 7), MoveFlag::Quiet); // Ke8-f8
        let m3 = Move::new(Square::new(5, 0), Square::new(4, 0), MoveFlag::Quiet); // Kf1-e1
        let m4 = Move::new(Square::new(5, 7), Square::new(4, 7), MoveFlag::Quiet); // Kf8-e8
        board.make_move(m1);
        board.make_move(m2);
        board.make_move(m3);
        board.make_move(m4);
        assert_eq!(board.hash, earlier_hash, "the four king shuffles should land back on the exact same position");

        let stop = AtomicBool::new(false);
        let tt = Tt::new(1);
        let mut tt_guard = tt.0.lock().unwrap();
        // This is exactly what `search()` builds from `game_history` plus
        // the current position: `earlier_hash` reappearing here represents
        // a repetition that happened before this `go`, not one discovered
        // within the tree currently being searched.
        let mut ctx = Context {
            nodes: 0,
            stop: &stop,
            hard_deadline: None,
            max_nodes: None,
            search_moves: None,
            path: vec![earlier_hash],
            aborted: false,
            tt: &mut tt_guard,
            killers: vec![[None, None]; MAX_PLY as usize],
            history: [[0; 64]; 64],
            static_evals: vec![None; MAX_PLY as usize],
            moves_played: vec![None; (MAX_PLY + 1) as usize],
            cont_history: vec![0; PIECE_TYPE_COUNT * 64 * PIECE_TYPE_COUNT * 64],
            pawn_correction: vec![0; CORRECTION_HISTORY_SIZE],
        };
        ctx.path.push(board.hash);

        let score = negamax(&mut board, 2, 1, -INF, INF, &mut ctx, None);
        assert_eq!(score, 0, "expected the repetition to be scored as an immediate draw, got {score}");
    }

    #[test]
    fn clearing_the_tt_does_not_break_the_next_search() {
        let board = Board::start_pos();
        let stop = AtomicBool::new(false);
        let tt = Tt::new(1);
        let limits = SearchLimits { max_depth: Some(3), ..Default::default() };
        let _ = search(&board, limits.clone(), &stop, &tt, &[], |_, _| {});
        tt.clear();
        let after_clear = search(&board, limits, &stop, &tt, &[], |_, _| {});
        assert!(after_clear.best_move.is_some());
    }
}
