use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use crate::board::Board;
use crate::eval;
use crate::movegen;
use crate::types::{Color, Move, MoveFlag, PieceType, Square};
use crate::zobrist;

pub const MATE_SCORE: i32 = 30_000;
/// Scores at or beyond this magnitude encode "forced mate in N plies", not
/// centipawns. `pub(crate)` so `uci.rs` can classify scores for `info score
/// mate` without re-deriving the constant by hand.
pub(crate) const MATE_THRESHOLD: i32 = MATE_SCORE - 1_000;
const INF: i32 = 1_000_000;
const MAX_PLY: u32 = 128;
pub(crate) const DEFAULT_TT_SIZE_MB: usize = 64;
/// Assumed number of moves remaining, used to divide up the clock when
/// `go` gives a time budget without `movestogo` (sudden death, where there
/// is no real number to use instead).
const DEFAULT_MOVES_DIVISOR: u64 = 20;
/// Base depth reduction applied to the verification search after a null
/// move; the actual reduction adds a depth-scaled and an eval-margin-scaled
/// term on top of this (see the null-move block in `negamax`).
const NULL_MOVE_REDUCTION: u32 = 2;
/// Null-move pruning only pays off once there's enough depth left to still
/// search meaningfully after the reduction.
const NULL_MOVE_MIN_DEPTH: u32 = 3;
/// Half-width, in centipawns, of the aspiration window built around the
/// previous iteration's score. The reference engines surveyed use 6-16 and
/// only start aspirating at depth 4-6 (below that, iteration scores still
/// jump around too much for a narrow window to pay off); 16 sits at the
/// cautious end of that range.
const ASPIRATION_WINDOW: i32 = 16;
/// Depths at or below this are searched with the full window instead.
const ASPIRATION_MIN_DEPTH: u32 = 4;
/// Late move reductions only kick in with enough depth left to still
/// search meaningfully after the reduction...
const LMR_MIN_DEPTH: u32 = 3;
/// ...and only past the first few moves, which are searched at full depth
/// since move ordering should have put the most promising ones first.
const LMR_FULL_DEPTH_MOVES: usize = 4;
/// Coefficients of the log-scaled LMR formula (see `lmr_reduction`): every
/// reference engine surveyed uses `base + ln(depth)·ln(move_index)/divisor`
/// with base ≈ 0.25-1.4 and divisor ≈ 2.1-3.1; these sit on the
/// conservative (less reduction) side of that range, fitting a move
/// ordering that's decent but not top-engine grade.
const LMR_BASE: f64 = 0.75;
const LMR_DIVISOR: f64 = 2.5;
/// Late move *pruning*: past this many quiet moves at a shallow depth, the
/// rest are skipped outright instead of merely reduced — with sensible
/// ordering, a quiet move this far down the list at low depth essentially
/// never turns out best. `(3 + d²)`, halved when not improving, is the
/// consensus threshold across the reference engines surveyed.
const LMP_MAX_DEPTH: u32 = 8;

fn lmp_threshold(depth: u32, improving: bool) -> usize {
    ((3 + depth * depth) / if improving { 1 } else { 2 }) as usize
}

/// Saturation bound for history/continuation-history scores under the
/// "gravity" update rule (see `gravity`): the de facto standard value in
/// every reference engine surveyed.
const HISTORY_MAX: i32 = 16384;

/// How strongly one search result moves a history entry, scaled by depth
/// (deeper results are worth more) and capped so a single very deep hit
/// can't swamp everything learned before it.
fn history_bonus(depth: u32) -> i32 {
    (300 * depth as i32 - 250).clamp(0, 2500)
}

/// The standard history update: moves the entry toward ±`HISTORY_MAX` by
/// `bonus`, with a pull-back proportional to how saturated the entry
/// already is. Keeps every entry in `[-HISTORY_MAX, HISTORY_MAX]` without
/// explicit clamping and makes recent results gradually displace old ones —
/// replacing both the old unbounded `+= depth²` growth and the once-per-
/// iteration halving that compensated for it.
fn gravity(entry: &mut i32, bonus: i32) {
    *entry += bonus - *entry * bonus.abs() / HISTORY_MAX;
}

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
/// How often the node budget (`go nodes`) is reconciled against the counter
/// shared by every Lazy SMP thread. Much finer than the 2048-node poll used
/// for the clock and the `stop` flag, because a node budget is usually a
/// measurement tool where overshooting by thousands of nodes defeats the
/// purpose — and because each thread used to get the *whole* budget to
/// itself, so `go nodes N` with 4 threads really searched about `4N`.
const NODE_BUDGET_CHECK_INTERVAL: u64 = 64;
/// General-purpose poll interval for the clock and the `stop` flag.
const STOP_CHECK_INTERVAL: u64 = 2048;
/// Cap on how far a chain of check extensions may push past the nominal
/// depth, as a multiple of the iteration's own depth. Without it a long
/// series of forced checks can extend all the way to `MAX_PLY` with no
/// relation at all to the depth that was asked for.
const CHECK_EXTENSION_PLY_FACTOR: u32 = 2;

#[derive(Clone, Default)]
pub struct SearchLimits {
    pub max_depth: Option<u32>,
    pub move_time_ms: Option<u64>,
    pub white_time_ms: Option<u64>,
    pub black_time_ms: Option<u64>,
    pub white_inc_ms: Option<u64>,
    pub black_inc_ms: Option<u64>,
    /// UCI `go movestogo`: how many moves remain before the game clock
    /// resets (classical tournament-style time controls, e.g. "40 moves in
    /// 90 minutes"). `None` means sudden death — no known reset point, so
    /// `compute_time_budget` falls back to a fixed assumed move count.
    pub moves_to_go: Option<u32>,
    pub infinite: bool,
    /// Hard node-count budget (UCI `go nodes`): checked at the same
    /// granularity as the `stop` flag/deadline, so it's an approximate
    /// ceiling, not an exact cutoff.
    pub max_nodes: Option<u64>,
    /// Restricts the root move loop to exactly these moves (UCI `go
    /// searchmoves`), in the order the GUI listed them. `None` means every
    /// legal root move is considered, as usual.
    pub search_moves: Option<Vec<Move>>,
    /// UCI `go ponder`: parsed here purely so all of `go`'s flags live in
    /// one place, but unused by `search`/`search_inner` themselves — the
    /// time budget is computed identically whether or not this is set (see
    /// `Engine::pondering` in `uci.rs` for why that's correct), and
    /// `bestmove` timing is an orchestration concern the search algorithm
    /// itself doesn't need to know about.
    pub ponder: bool,
    /// Break near-exact ties among root moves at random instead of always
    /// playing the first one found (UCI option `Variety`, off by default).
    /// Off is the stronger and the *measurable* setting: with it on, node
    /// counts and match results stop being reproducible for the same
    /// position and depth, which is exactly what a strength harness needs.
    pub variety: bool,
    /// UCI `go mate N`: look for a forced mate in at most N moves. Sets the
    /// depth budget to `2*N` plies when no explicit `depth` was given —
    /// enough to see any mate in N, which takes `2*N - 1` plies with the
    /// searching side to move — and the iterative-deepening loop already
    /// stops as soon as any forced mate is found (see `search_inner`), so
    /// no extra stop condition is needed here.
    pub mate_in: Option<u32>,
}

#[derive(Clone, Default)]
pub struct SearchResult {
    pub best_move: Option<Move>,
    pub score: i32,
    pub depth: u32,
    pub nodes: u64,
    /// The principal variation, root move first, reconstructed by walking
    /// the TT's stored best move from each position along the line. May be
    /// shorter than `depth` (quiescence ends the line at the horizon) but
    /// always starts with `best_move` when `best_move` is `Some`.
    pub pv: Vec<Move>,
    /// False when the iteration that produced this was cut short. Only ever
    /// possible for the very first iteration a thread runs — later ones are
    /// discarded outright in favour of the previous complete result — but
    /// that one still has to be published, because the caller needs *some*
    /// legal move to play. Its `score`, `depth` and `pv` describe an
    /// unfinished search and must not be compared against another thread's
    /// completed one.
    pub complete: bool,
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
    /// Which search "generation" (see `TranspositionTable::generation`)
    /// stored this entry, so a new search can tell a deep-but-stale entry
    /// left over from several moves ago apart from a deep entry from its
    /// own, current search.
    generation: u8,
    /// The fifty-move counter the score was computed under, saturated at
    /// 100. The Zobrist key deliberately excludes it (two positions that
    /// differ only in this counter *are* the same position for repetition
    /// purposes), but the same placement is worth something quite different
    /// at clock 0 and at clock 99, so a stored score can only be reused as
    /// a bound when the counter cannot change the verdict — see
    /// `tt_score_is_clock_compatible`.
    halfmove_clock: u8,
}

/// Can the fifty-move rule fire anywhere inside a subtree of `depth` plies
/// starting from this counter? The `2 *` allows for extensions and the
/// constant for a quiescence tail; both are deliberately generous, since
/// being wrong here means reusing a score that the rule invalidated.
fn fifty_move_rule_in_reach(clock: u32, depth: u8) -> bool {
    clock + 2 * depth as u32 + 8 >= 100
}

/// Whether `entry`'s score may be used as a bound for a position whose
/// fifty-move counter is `board_clock`. Same counter is always fine; a
/// different one only when the rule is out of reach for both.
fn tt_score_is_clock_compatible(entry: &TtEntry, board_clock: u16) -> bool {
    let now = u32::from(board_clock).min(100);
    let stored = u32::from(entry.halfmove_clock);
    now == stored || (!fifty_move_rule_in_reach(now, entry.depth) && !fifty_move_rule_in_reach(stored, entry.depth))
}

/// Fixed-size hash table of positions seen during search, keyed by the
/// incrementally-maintained Zobrist hash. Within the same search, shallower
/// entries are overwritten by deeper ones; across searches, `generation`
/// lets a new search's entries win regardless of depth (see `store`).
struct TranspositionTable {
    entries: Vec<Option<TtEntry>>,
    mask: u64,
    /// Bumped once per `search()` call (i.e. once per UCI `go`), not once
    /// per node — see `store`. Wraps around after 256 searches, which only
    /// costs a slightly worse replacement choice for that one collision,
    /// never a correctness problem: `probe` always re-validates the full
    /// 64-bit key regardless of generation.
    generation: u8,
}

/// Largest power of two `<= n`, or 1 if `n == 0`.
fn floor_power_of_two(n: usize) -> usize {
    if n == 0 {
        1
    } else {
        1usize << (usize::BITS - 1 - n.leading_zeros())
    }
}

/// How many `Option<TtEntry>` slots `TranspositionTable::new(size_mb)`
/// would allocate, without actually allocating them. Exists as its own
/// function so tests can check the size math at large requested sizes
/// (including `MAX_HASH_MB`) without the multi-hundred-MB-or-more real
/// allocation that would otherwise imply — `TranspositionTable::new` itself
/// just calls this.
pub(crate) fn slot_count_for(size_mb: usize) -> usize {
    let entry_size = std::mem::size_of::<Option<TtEntry>>();
    let raw_count = (size_mb * 1024 * 1024) / entry_size;
    // The `& mask` indexing in `probe`/`store` needs an exact power of two,
    // but rounding *up* to one (as `next_power_of_two` would) can nearly
    // double the actual memory used right above almost every power-of-two
    // boundary — e.g. requesting 4096 MB used to allocate 6 GB. Rounding
    // *down* instead means the table never uses more than the requested
    // budget, only occasionally somewhat less.
    floor_power_of_two(raw_count).max(1)
}

impl TranspositionTable {
    fn new(size_mb: usize) -> Self {
        let count = slot_count_for(size_mb);
        TranspositionTable { entries: vec![None; count], mask: (count - 1) as u64, generation: 0 }
    }

    fn clear(&mut self) {
        self.entries.fill(None);
        self.generation = 0;
    }

    /// Marks the start of a new search: without this, a deep entry stored
    /// several moves ago (the table is kept alive for the whole game, see
    /// `Tt`) would keep winning the depth-preferred comparison in `store`
    /// forever, permanently blocking that slot from ever reflecting the
    /// current position even though the old entry is no longer relevant to
    /// where the game actually is now.
    fn new_search(&mut self) {
        self.generation = self.generation.wrapping_add(1);
    }

    fn probe(&self, key: u64) -> Option<TtEntry> {
        self.entries[(key & self.mask) as usize].filter(|e| e.key == key)
    }

    fn store(&mut self, key: u64, depth: u8, score: i32, flag: TtFlag, best_move: Option<Move>, halfmove_clock: u8) {
        let generation = self.generation;
        let slot = &mut self.entries[(key & self.mask) as usize];
        let should_replace = match slot {
            Some(existing) => existing.key != key || existing.generation != generation || existing.depth <= depth,
            None => true,
        };
        if should_replace {
            *slot = Some(TtEntry { key, depth, score, flag, best_move, generation, halfmove_clock });
        }
    }
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

    /// Number of slots in the table (a power of two derived from the size
    /// in MB `Tt::new` was given). Exists so callers — chiefly the UCI
    /// `setoption name Hash` handler and its tests — can confirm a resize
    /// actually took effect, without exposing the table's internal layout.
    pub fn capacity(&self) -> usize {
        self.0.lock().unwrap().entries.len()
    }

    /// Locks just long enough to bump the generation counter — see
    /// `TranspositionTable::new_search`. Must be called exactly once per
    /// real `go`, not once per search thread (Fase 4): the generation marks
    /// "a new search started", and bumping it once per helper thread would
    /// make sibling threads' own fresh entries look mutually stale and
    /// evict each other instead of reinforcing one another through the
    /// shared table.
    fn new_search(&self) {
        self.0.lock().unwrap().new_search();
    }

    /// Locks just long enough to read one slot. Deliberately *not* held for
    /// the whole search (unlike before Fase 4, when the caller kept a
    /// `MutexGuard` for the entire iterative-deepening loop): with several
    /// search threads sharing one `Tt`, a lock held that long would
    /// serialize them completely, leaving Lazy SMP no faster than a single
    /// thread.
    fn probe(&self, key: u64) -> Option<TtEntry> {
        self.0.lock().unwrap().probe(key)
    }

    /// Locks just long enough to write one slot — see `probe` above for why
    /// per-call locking (not one lock for the whole search) matters once
    /// more than one thread shares this table.
    fn store(&self, key: u64, depth: u8, score: i32, flag: TtFlag, best_move: Option<Move>, halfmove_clock: u8) {
        self.0.lock().unwrap().store(key, depth, score, flag, best_move, halfmove_clock);
    }

    /// Marks the start of a real `go`. Public to the crate so the Lazy SMP
    /// coordinator in `uci.rs` can bump the generation *before* launching
    /// any worker: doing it from inside the main search thread let a helper
    /// that started first write entries under the previous generation, which
    /// the bump then immediately marked stale.
    pub(crate) fn begin_search(&self) {
        self.new_search();
    }
}

/// The fifty-move counter as stored in a TT entry.
fn tt_clock(board: &Board) -> u8 {
    u16::min(board.halfmove_clock, 100) as u8
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
    /// Index into `path` before which repetition detection must not look.
    /// Raised past the current path length while searching below a null
    /// move: the positions in that subtree are not reachable by legal play
    /// from anything recorded earlier, so matching them against real
    /// positions produces phantom repetitions (two consecutive null moves
    /// reproduce the hash from two plies up exactly, which used to score a
    /// null-move verification as an immediate draw).
    path_start: usize,
    aborted: bool,
    /// Node counter shared by every Lazy SMP thread, used only to enforce
    /// `go nodes` against one global budget instead of one per thread.
    shared_nodes: &'a AtomicU64,
    /// How much of `nodes` has already been added to `shared_nodes`.
    published_nodes: u64,
    /// Depth of the iteration currently running, for the check-extension cap.
    root_depth: u32,
    /// UCI option `Variety` — see `SearchLimits::variety`.
    variety: bool,
    /// Shared, not exclusively borrowed: with Lazy SMP (Fase 4), several
    /// threads each hold their own `Context` but the same `Tt`, locking it
    /// only for the duration of each individual `probe`/`store` call rather
    /// than for a whole search.
    tt: &'a Tt,
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
    /// True at plies reached by a null move, so the child can refuse to play
    /// a second one (see `path_start`).
    null_at_ply: Vec<bool>,
    /// Triangular principal-variation table: the line from ply `p` onward
    /// lives in `pv[p * MAX_PLY ..][.. pv_len[p]]`. Propagated upward as the
    /// search runs instead of being reconstructed afterwards by walking the
    /// TT, which could (and did) print a line that contradicted the score it
    /// was reported with — a shared table under Lazy SMP has no obligation
    /// to still hold the entries the finished iteration passed through.
    pv: Vec<Move>,
    pv_len: Vec<usize>,
    /// Scratch buffer reused by `order_moves_full`, so ordering a node's
    /// moves doesn't allocate once per node.
    order_buffer: Vec<(i32, u16, Move)>,
}

/// Number of PieceType variants, used to size/index `Context::cont_history`.
const PIECE_TYPE_COUNT: usize = 6;

fn cont_history_index(prev_piece: PieceType, prev_to: Square, piece: PieceType, to: Square) -> usize {
    ((prev_piece as usize * 64 + prev_to.0 as usize) * PIECE_TYPE_COUNT + piece as usize) * 64 + to.0 as usize
}

/// Zobrist hash of just the pawn structure (both colors) *plus the side to
/// move*, used to key the correction history. Recomputed on demand from the
/// pawn bitboards rather than maintained incrementally like `Board::hash` —
/// cheap relative to the rest of a static eval call (at most 16 XORs), and
/// keeps this search-only concern out of `Board`.
///
/// The side to move belongs in the key even though it is not part of the
/// pawn structure: corrections are learned and consumed in the perspective
/// of whoever is to move, so a White-to-move node and a Black-to-move node
/// with the same skeleton would otherwise share a bucket and write
/// opposite-signed errors into it, cancelling out or actively reinforcing
/// the error instead of correcting it.
fn pawn_hash(board: &Board) -> u64 {
    let mut hash = 0u64;
    for sq in board.pieces_of(Color::White, PieceType::Pawn) {
        hash ^= zobrist::piece_square_key(Color::White, PieceType::Pawn, sq);
    }
    for sq in board.pieces_of(Color::Black, PieceType::Pawn) {
        hash ^= zobrist::piece_square_key(Color::Black, PieceType::Pawn, sq);
    }
    if board.side_to_move == Color::Black {
        hash ^= zobrist::side_to_move_key();
    }
    hash
}

impl Context<'_> {
    fn should_stop(&mut self) -> bool {
        if self.aborted {
            return true;
        }
        if let Some(max_nodes) = self.max_nodes {
            if self.nodes.is_multiple_of(NODE_BUDGET_CHECK_INTERVAL) {
                let delta = self.nodes - self.published_nodes;
                let total = self.shared_nodes.fetch_add(delta, Ordering::Relaxed) + delta;
                self.published_nodes = self.nodes;
                if total >= max_nodes {
                    self.aborted = true;
                    return true;
                }
            }
        }
        if self.nodes.is_multiple_of(STOP_CHECK_INTERVAL) {
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
        }
        false
    }

    /// Resets the PV line recorded at `ply`. Called on entry to every node,
    /// so a node that returns early (TT cutoff, draw, quiescence) leaves an
    /// empty line behind rather than a stale sibling's.
    fn clear_pv(&mut self, ply: u32) {
        self.pv_len[ply as usize] = 0;
    }

    /// Records `mv` followed by whatever line the child at `ply + 1` found.
    fn update_pv(&mut self, ply: u32, mv: Move) {
        let p = ply as usize;
        let span = MAX_PLY as usize;
        let child_len = self.pv_len[p + 1].min(span - 1);
        let base = p * span;
        let child_base = (p + 1) * span;
        self.pv[base] = mv;
        for i in 0..child_len {
            self.pv[base + 1 + i] = self.pv[child_base + i];
        }
        self.pv_len[p] = child_len + 1;
    }

    fn pv_line(&self, ply: u32) -> Vec<Move> {
        let p = ply as usize;
        let base = p * MAX_PLY as usize;
        self.pv[base..base + self.pv_len[p]].to_vec()
    }

    /// Twofold, not threefold, *by design*: `path` already contains the
    /// current position once (pushed by the caller), so `>= 2` fires on the
    /// first re-occurrence, whether the earlier one came from the real game
    /// history or from the search path itself. Scoring that first repetition
    /// as an immediate draw is the standard treatment in serious engines
    /// (Stockfish behaves the same way for any repetition with at least one
    /// occurrence inside the search): if the side to move can force one
    /// repetition, it can force another, so burning depth to confirm the
    /// third occurrence buys nothing. The selfplay harness, by contrast,
    /// arbitrates *finished games* with the official threefold rule — that
    /// asymmetry is intentional (heuristic inside the tree, real rule at
    /// the table), not a disagreement to be "fixed".
    ///
    /// Scans backwards two plies at a time and only as far as the fifty-move
    /// counter allows, rather than counting occurrences across the whole
    /// vector: a position can only repeat one where the same side was to
    /// move (any other has a different hash anyway), and never across the
    /// last capture or pawn move, which is exactly what that counter
    /// measures. That bound is what makes the check affordable in quiescence
    /// too, where the path used not to be tracked at all — leaving a
    /// perpetual built out of check evasions invisible.
    fn is_repetition(&self, hash: u64, halfmove_clock: u16) -> bool {
        let end = self.path.len();
        if end < 3 {
            return false;
        }
        let lowest = self.path_start.max(end.saturating_sub(halfmove_clock as usize + 1));
        let mut i = end - 3;
        if i < lowest {
            return false;
        }
        loop {
            if self.path[i] == hash {
                return true;
            }
            if i < lowest + 2 {
                return false;
            }
            i -= 2;
        }
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

    /// Applies a (positive or negative) gravity update to the flat history
    /// entry for `mv` — see `gravity` for why this replaces plain addition.
    fn update_history(&mut self, mv: Move, bonus: i32) {
        gravity(&mut self.history[mv.from.0 as usize][mv.to.0 as usize], bonus);
    }

    fn record_move_played(&mut self, ply: u32, piece: PieceType, to: Square) {
        self.moves_played[ply as usize] = Some((piece, to));
        self.null_at_ply[ply as usize] = false;
    }

    /// Marks `ply` as having been reached by a null move: no move was really
    /// played, so continuation history has no parent move to key on, and the
    /// child must not try a second null.
    fn record_null_played(&mut self, ply: u32) {
        self.moves_played[ply as usize] = None;
        self.null_at_ply[ply as usize] = true;
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

    fn update_cont_history(&mut self, ply: u32, piece: PieceType, to: Square, bonus: i32) {
        if let Some((prev_piece, prev_to)) = self.prev_move_at(ply) {
            gravity(&mut self.cont_history[cont_history_index(prev_piece, prev_to, piece, to)], bonus);
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
    // With `movestogo` we know exactly how many moves are left before the
    // clock resets, which is a much better divisor than the fixed
    // sudden-death fallback: e.g. with 5 moves left, spending a fifth of
    // the remaining time now is correct, whereas assuming 20 moves left
    // would starve the search on each of those 5 moves for no reason.
    let divisor = limits.moves_to_go.map(|m| u64::from(m.max(1))).unwrap_or(DEFAULT_MOVES_DIVISOR);
    time_left.map(|t| {
        let raw_budget = t / divisor + inc / 2;
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
    on_iteration: impl FnMut(&SearchResult, Duration),
) -> SearchResult {
    let shared_nodes = AtomicU64::new(0);
    let shared = SharedSearchState { stop, tt, nodes: &shared_nodes };
    search_inner(board, limits, &shared, game_history, SearchRole::MAIN, on_iteration)
}

/// The state one `go` shares across all of its Lazy SMP threads: the abort
/// flag they watch, the transposition table they cooperate through, and the
/// node counter a `go nodes` budget is measured against. Bundled so
/// `search_inner` takes a handful of arguments rather than a dozen.
pub(crate) struct SharedSearchState<'a> {
    pub stop: &'a AtomicBool,
    pub tt: &'a Tt,
    pub nodes: &'a AtomicU64,
}

/// Distinguishes the "main" search thread from a Lazy SMP helper thread
/// (Fase 4) within `search_inner` — see `uci.rs`'s `spawn_search`, the only
/// place that ever constructs one directly rather than via `SearchRole::MAIN`.
pub(crate) struct SearchRole {
    /// Staggers a helper thread's first iteration (classic Lazy SMP "helper
    /// threads with small depth perturbations") so that, at any wall-clock
    /// instant, different threads tend to be exploring different depths.
    /// `search_inner` clamps this to `max_depth`, so a helper still runs its
    /// loop body at least once even if `go depth` asked for less than its
    /// stagger.
    pub start_depth: u32,
    /// Must be `true` for exactly one thread per real `go`: the TT
    /// generation (see `TranspositionTable::new_search`) marks "a new
    /// search started", not "a new thread started". Bumping it once per
    /// helper would make sibling threads' own fresh entries look mutually
    /// stale and evict each other instead of reinforcing one another
    /// through the shared table.
    pub bump_generation: bool,
}

impl SearchRole {
    pub(crate) const MAIN: SearchRole = SearchRole { start_depth: 1, bump_generation: true };
}

/// The actual iterative-deepening loop `search` wraps. Exposed separately
/// (`pub(crate)`, not `pub`) so the Lazy SMP orchestration in `uci.rs` can
/// run several of these concurrently against one shared `tt` — see
/// `SearchRole` for what distinguishes a helper thread's call from the
/// main thread's.
pub(crate) fn search_inner(
    board: &Board,
    limits: SearchLimits,
    shared: &SharedSearchState,
    game_history: &[u64],
    role: SearchRole,
    mut on_iteration: impl FnMut(&SearchResult, Duration),
) -> SearchResult {
    let SharedSearchState { stop, tt, nodes: shared_nodes } = *shared;
    let start = Instant::now();
    let mut working = board.clone();
    let budget = compute_time_budget(&limits, working.side_to_move, start);
    // A bare `go nodes N` is a budget just like a clock or `infinite`: it
    // used to fall through to the six-ply default instead, so `go nodes
    // 1000000` finished at depth 6 having searched about 4,000 nodes.
    let unbounded = limits.infinite || budget.is_some() || limits.max_nodes.is_some();
    let max_depth = limits
        .max_depth
        .or(limits.mate_in.map(|n| 2 * n))
        .unwrap_or(if unbounded { MAX_PLY } else { 6 })
        .clamp(1, MAX_PLY);
    let start_depth = role.start_depth.min(max_depth);

    let mut path = Vec::with_capacity(game_history.len() + 1);
    path.extend_from_slice(game_history);
    path.push(working.hash);

    if role.bump_generation {
        tt.new_search();
    }
    let mut ctx = Context {
        nodes: 0,
        stop,
        hard_deadline: budget.as_ref().map(|b| b.hard),
        max_nodes: limits.max_nodes,
        search_moves: limits.search_moves.clone(),
        path,
        path_start: 0,
        aborted: false,
        shared_nodes,
        published_nodes: 0,
        root_depth: start_depth,
        variety: limits.variety,
        tt,
        killers: vec![[None, None]; MAX_PLY as usize],
        history: [[0; 64]; 64],
        static_evals: vec![None; MAX_PLY as usize],
        moves_played: vec![None; (MAX_PLY + 1) as usize],
        cont_history: vec![0; PIECE_TYPE_COUNT * 64 * PIECE_TYPE_COUNT * 64],
        pawn_correction: vec![0; CORRECTION_HISTORY_SIZE],
        null_at_ply: vec![false; (MAX_PLY + 1) as usize],
        pv: vec![Move::new(Square(0), Square(0), MoveFlag::Quiet); ((MAX_PLY + 1) * MAX_PLY) as usize],
        pv_len: vec![0; (MAX_PLY + 2) as usize],
        order_buffer: Vec::with_capacity(64),
    };

    let mut result = SearchResult::default();

    for depth in start_depth..=max_depth {
        // Don't even start a deeper iteration once past the soft budget:
        // the next depth is typically several times more expensive than
        // the last, so starting late just means overshooting further
        // before the hard deadline catches it.
        if depth > start_depth {
            if let Some(b) = &budget {
                if Instant::now() >= b.soft {
                    break;
                }
            }
        }

        ctx.root_depth = depth;
        let (score, best_move) = search_root_with_aspiration(&mut working, depth, result.score, &mut ctx);

        if best_move.is_none() {
            // No legal moves at the root at all: checkmate or stalemate.
            result = SearchResult { best_move: None, score, depth, nodes: ctx.nodes, pv: Vec::new(), complete: true };
            break;
        }
        if ctx.aborted && depth > start_depth {
            break; // discard the unfinished iteration, keep the previous one
        }

        let pv = ctx.pv_line(0);
        result = SearchResult { best_move, score, depth, nodes: ctx.nodes, pv, complete: !ctx.aborted };
        on_iteration(&result, start.elapsed());

        if ctx.aborted || score.abs() >= MATE_THRESHOLD {
            break;
        }
    }
    result
}

/// Searches `depth` with a window centered on `prev_score` (the previous
/// iteration's result), widening progressively on whichever side fails
/// instead of jumping straight to the full (-INF, INF) window: the true
/// score is usually still close to `prev_score` even when the first guess
/// misses, so doubling the margin on the failing side and re-searching
/// converges in a couple of cheap retries far more often than it needs a
/// full-width search, which throws away all of alpha-beta's pruning.
/// `search_root` is fail-soft (returns the actual score found, not just
/// `alpha`/`beta` clamped), so each retry re-centers on real information
/// instead of blindly repeating the same guess.
fn search_root_with_aspiration(board: &mut Board, depth: u32, prev_score: i32, ctx: &mut Context) -> (i32, Option<Move>) {
    if depth <= ASPIRATION_MIN_DEPTH {
        return search_root(board, depth, -INF, INF, ctx);
    }

    let mut delta = ASPIRATION_WINDOW;
    let mut alpha = prev_score.saturating_sub(delta).max(-INF);
    let mut beta = prev_score.saturating_add(delta).min(INF);

    loop {
        let (score, best_move) = search_root(board, depth, alpha, beta, ctx);
        if ctx.aborted || (score > alpha && score < beta) {
            return (score, best_move);
        }
        delta = delta.saturating_mul(2);
        if score <= alpha {
            alpha = score.saturating_sub(delta).max(-INF);
        } else {
            beta = score.saturating_add(delta).min(INF);
        }
    }
}

fn search_root(board: &mut Board, depth: u32, alpha_init: i32, beta: i32, ctx: &mut Context) -> (i32, Option<Move>) {
    ctx.clear_pv(0);
    let mut moves = movegen::legal_moves_scratch(board);
    if let Some(restrict_to) = &ctx.search_moves {
        // `parse_go_limits` only ever produces a non-empty list (a
        // `searchmoves` naming nothing legal is dropped there rather than
        // silently searching everything), so this filter always leaves at
        // least the requested moves; the emptiness guard stays as a
        // belt-and-braces against a caller building limits by hand.
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

        if is_exact && ctx.variety {
            exact_candidates.push((mv, score));
        }
        if score > best_score {
            best_score = score;
            best_move = mv;
            if !ctx.aborted {
                ctx.update_pv(0, mv);
            }
        }
        if best_score > alpha {
            alpha = best_score;
        }

        if ctx.aborted {
            break;
        }
        // Fail-high against the aspiration window: the caller is going to
        // widen and search this depth again, so reading out the remaining
        // root moves under a window we already know is wrong is pure cost.
        if alpha >= beta {
            break;
        }
    }

    if !ctx.aborted {
        // With an aspiration window (`alpha_init`/`beta` narrower than the
        // full window), a fail-low/fail-high result is only a bound, and
        // storing it as Exact would hand other probes — including Lazy SMP
        // siblings sharing this table — a score the search never actually
        // proved. Same classification `negamax` uses for its own store.
        let flag = if best_score <= alpha_init {
            TtFlag::Upper
        } else if best_score >= beta {
            TtFlag::Lower
        } else {
            TtFlag::Exact
        };
        // Stored before the variety pick below, deliberately: the table (and
        // therefore the next iteration's move ordering, and every Lazy SMP
        // sibling) must record the move that actually earned `best_score`,
        // not a near-equal alternative chosen for cosmetic variety.
        ctx.tt.store(board.hash, depth as u8, score_to_tt(best_score, 0), flag, Some(best_move), tt_clock(board));
    }

    if ctx.variety {
        if let Some(picked) = pick_near_best(&exact_candidates, best_score) {
            if picked != best_move {
                best_move = picked;
                // The propagated line belongs to the move that was actually
                // best; report only the move being played rather than a PV
                // that starts with a different move than `bestmove`.
                ctx.pv_len[0] = 1;
                ctx.pv[0] = picked;
            }
        }
    }

    (best_score, Some(best_move))
}

/// Picks at random among the root moves whose genuine (full-window) score is
/// within `ROOT_TIE_EPSILON` of the best, or `None` when fewer than two
/// qualify and there is nothing to choose between. Only ever called with the
/// `Variety` option on; the default is to play the best move every time,
/// which is both stronger and the only way node counts and match results
/// stay reproducible.
fn pick_near_best(candidates: &[(Move, i32)], best_score: i32) -> Option<Move> {
    let near_best: Vec<Move> = candidates
        .iter()
        .filter(|&&(_, score)| best_score - score <= ROOT_TIE_EPSILON)
        .map(|&(mv, _)| mv)
        .collect();
    (near_best.len() > 1).then(|| near_best[random_index(near_best.len())])
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

/// Log-scaled late move reduction, precomputed once into a flat table (the
/// formula needs `ln`, which isn't `const fn`): reductions grow slowly with
/// both remaining depth and how far down the move ordering the move sits,
/// so a barely-late move at low depth still gets the old flat 1-ply
/// reduction while a very late move at high depth can be reduced by 4+ —
/// the single biggest node-count lever in every reference engine surveyed.
fn lmr_reduction(depth: u32, move_index: usize) -> u32 {
    const TABLE_DEPTH: usize = (MAX_PLY + 1) as usize;
    const TABLE_MOVES: usize = 64;
    static TABLE: std::sync::OnceLock<Vec<u8>> = std::sync::OnceLock::new();
    let table = TABLE.get_or_init(|| {
        let mut t = vec![0u8; TABLE_DEPTH * TABLE_MOVES];
        for (d, chunk) in t.chunks_mut(TABLE_MOVES).enumerate().skip(1) {
            for (m, entry) in chunk.iter_mut().enumerate().skip(1) {
                let r = LMR_BASE + (d as f64).ln() * (m as f64).ln() / LMR_DIVISOR;
                *entry = r.max(0.0) as u8;
            }
        }
        t
    });
    let d = (depth as usize).min(TABLE_DEPTH - 1);
    let m = move_index.min(TABLE_MOVES - 1);
    table[d * TABLE_MOVES + m] as u32
}

fn negamax(board: &mut Board, depth: u32, ply: u32, mut alpha: i32, beta: i32, ctx: &mut Context, excluded: Option<Move>) -> i32 {
    ctx.nodes += 1;
    if ctx.should_stop() {
        return 0;
    }

    // Check extensions (below) let a single line of forced checks push ply
    // past what `depth` alone would predict; bail out to a static eval
    // rather than recurse further, which both bounds worst-case recursion
    // and keeps `ctx.killers_at(ply)` in bounds (it's sized to MAX_PLY).
    if ply >= MAX_PLY {
        return eval::evaluate_relative(board);
    }
    ctx.clear_pv(ply);

    if let Some(score) = terminal_draw_score(board, ply, ctx) {
        return score;
    }

    let alpha_orig = alpha;
    // While verifying a singular-extension candidate (`excluded` is set),
    // this same position's TT entry reflects the *unrestricted* move set
    // — including the very move being excluded — so it can't be used as a
    // cutoff for this restricted search. `tt_move` (and the entry, for the
    // singular-extension check below) are still read normally either way.
    let is_pv_node = beta > alpha + 1;
    let tt_entry = ctx.tt.probe(board.hash);
    let tt_move = tt_entry.and_then(|e| e.best_move);
    // Never cut off on a stored score inside the principal variation. The
    // score would be right, but the line would end there: the PV is
    // propagated as the search runs now, and a node that returns without
    // searching a move has no line to hand its parent. (This is also the
    // standard treatment — a PV node is worth searching out properly.)
    if excluded.is_none() && !is_pv_node {
        if let Some(entry) = tt_entry {
            // The move is always reusable for ordering; only the *score* is
            // gated on the fifty-move counter being compatible.
            if entry.depth as u32 >= depth && tt_score_is_clock_compatible(&entry, board.halfmove_clock) {
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
        // `quiescence_inner`, not `quiescence`: this node has already been
        // counted above, and going through the counting wrapper made every
        // frontier node show up twice in `nodes`/`nps` and in the `go nodes`
        // budget.
        return quiescence_inner(board, alpha, beta, ply, ctx);
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
    // moves at all. The margin is one depth's worth *smaller* when the
    // position is improving: a rising eval makes it likelier that a real
    // search would confirm the cutoff, so less evidence is demanded. It
    // used to be the other way round, which made the cut easier precisely
    // when the signal was weakest (and, at depth 1 without improvement,
    // demanded no margin at all).
    if let Some(se) = static_eval {
        if depth <= RFP_MAX_DEPTH && beta < MATE_THRESHOLD {
            let margin = RFP_MARGIN_PER_DEPTH * (depth as i32 - if improving { 1 } else { 0 }).max(0);
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
    // artificially flip the evaluation. Only tried when the static eval
    // already clears beta (if it doesn't, handing the opponent a free move
    // is hardly going to), and the reduction grows with both depth and how
    // far above beta the eval sits — the flat R=2 this replaces was the
    // single most conservative pruning setting left in the engine, and the
    // dynamic form is unanimous across the reference engines surveyed.
    // `!ctx.null_at_ply[ply]`: never two nulls in a row. Two consecutive
    // passes reproduce the hash from two plies up exactly (the side-to-move
    // key cancels out), so with both pushed onto `path` the verification
    // search used to see a repetition and score itself as a draw. The tempo
    // term breaks the antisymmetry that would otherwise make the two null
    // conditions mutually exclusive, so this really can happen.
    if !in_check
        && !ctx.null_at_ply[ply as usize]
        && depth >= NULL_MOVE_MIN_DEPTH
        && beta < MATE_THRESHOLD
        && static_eval.is_some_and(|se| se >= beta)
        && has_non_pawn_material(board, board.side_to_move)
    {
        let se = static_eval.expect("guarded by is_some_and above");
        let reduction = NULL_MOVE_REDUCTION + depth / 3 + (((se - beta) / 200) as u32).min(3);
        let undo = board.make_null_move();
        // Nothing below a null move is reachable by legal play from the
        // positions recorded so far, so the repetition window restarts here
        // and the artificial position itself is never recorded at all.
        let prev_path_start = ctx.path_start;
        ctx.path_start = ctx.path.len();
        ctx.record_null_played(ply + 1);
        let score = -negamax(board, depth.saturating_sub(1 + reduction), ply + 1, -beta, -beta + 1, ctx, None);
        ctx.path_start = prev_path_start;
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

    let moves = movegen::legal_moves_scratch(board);
    if moves.is_empty() {
        return if in_check { -MATE_SCORE + ply as i32 } else { 0 };
    }

    let killers = ctx.killers_at(ply);
    let mut ordered = moves;
    order_moves_full(board, &mut ordered, tt_move, killers, ctx, ply);

    // Check extension: a position where the side to move is in check is
    // forcing (few replies, tactics often hiding just beyond the horizon),
    // so search it one ply deeper instead of letting `depth` run out here.
    // Capped by ply so a long forced sequence of checks can't keep extending
    // all the way to `MAX_PLY` with no relation to the depth asked for.
    let extend_check = in_check && ply < CHECK_EXTENSION_PLY_FACTOR * ctx.root_depth;
    let child_depth = depth - 1 + if extend_check { 1 } else { 0 };

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

    // Quiet moves already searched at this node without producing a cutoff,
    // remembered so that if a *later* quiet move does cut off, these get the
    // mirror-image history penalty (malus): they were ordered ahead of the
    // move that actually worked, and that's exactly the ordering mistake the
    // history tables exist to unlearn. Without the malus, history scores
    // only ever grow and ordering stops distinguishing good from merely old.
    let mut tried_quiets: Vec<(Move, PieceType)> = Vec::new();
    // Whether any move was really searched. Only ever 0 in a singular
    // verification search, where the TT move (which move ordering puts
    // first) is the excluded one and everything else can be pruned away —
    // returning `-INF` from there made `score < singular_beta` trivially
    // true and handed out the extension for free.
    let mut moves_searched = 0u32;

    for (move_index, mv) in ordered.into_iter().enumerate() {
        if Some(mv) == excluded {
            continue;
        }

        let is_quiet = !mv.is_capture() && mv.promotion().is_none();
        // Counted over quiet moves actually searched, not over the whole
        // ordered list: with the list led by captures, "the 3rd move" and
        // "the 3rd quiet move" are very different things, and in a tactical
        // position with eight plausible captures the old form pruned
        // *every* quiet move at depth <= 2.
        let quiets_tried = tried_quiets.len();

        // Whether this move checks the opponent, answered without playing
        // it (see `movegen::gives_check`). LMR already refused to reduce a
        // checking move; LMP and futility used to discard them outright,
        // which can throw away a quiet mate sitting past the threshold.
        let gives_check = movegen::gives_check(board, mv);

        if is_quiet && !gives_check && !in_check && alpha > -MATE_THRESHOLD && beta < MATE_THRESHOLD {
            // Late move pruning: at shallow depth, once this many quiet
            // moves have been searched without a cutoff, skip the rest.
            let lmp = depth <= LMP_MAX_DEPTH && quiets_tried >= lmp_threshold(depth, improving);
            let futile =
                move_index > 0 && futility_eval.is_some_and(|se| se + FUTILITY_MARGIN[depth as usize] <= alpha);
            if lmp || futile {
                continue;
            }
        }

        let moved_piece = board.piece_at(mv.from).map(|p| p.kind).unwrap_or(PieceType::Pawn);
        let mv_child_depth = if Some(mv) == tt_move { child_depth + tt_move_extension } else { child_depth };
        let undo = board.make_move(mv);
        ctx.path.push(board.hash);
        ctx.record_move_played(ply + 1, moved_piece, mv.to);
        moves_searched += 1;

        // Late move reductions: moves this far down an already-good
        // ordering, that are quiet and not a reply to/giver of check, are
        // unlikely to be the best move, so search them shallower first and
        // only pay for a full-depth re-search if they beat alpha anyway.
        // The reduction is log-scaled in depth and move index (see
        // `lmr_reduction`), clamped so the reduced search always keeps at
        // least 1 ply (dropping straight into quiescence would make the
        // scout blind to any quiet refutation).
        let can_reduce = move_index >= LMR_FULL_DEPTH_MOVES && depth >= LMR_MIN_DEPTH && is_quiet && !in_check && !gives_check;
        let reduction = if can_reduce {
            lmr_reduction(depth, move_index).min(mv_child_depth.saturating_sub(1))
        } else {
            0
        };

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
            if is_pv_node && score > alpha && score < beta && !ctx.aborted {
                ctx.update_pv(ply, mv);
            }
        }
        if best_score > alpha {
            alpha = best_score;
        }
        if alpha >= beta {
            let bonus = history_bonus(depth);
            if is_quiet {
                ctx.store_killer(ply, mv);
                ctx.update_history(mv, bonus);
                ctx.update_cont_history(ply, moved_piece, mv.to, bonus);
            }
            // Malus applies whatever cut the node off, capture or not: the
            // quiet moves searched before it were still ordered ahead of the
            // move that worked, and that is exactly the ordering mistake the
            // history tables exist to unlearn. Restricting the malus to
            // quiet cutoffs let quiet moves that keep failing in tactical
            // positions hold on to inflated scores indefinitely.
            for &(tried_mv, tried_piece) in &tried_quiets {
                ctx.update_history(tried_mv, -bonus);
                ctx.update_cont_history(ply, tried_piece, tried_mv.to, -bonus);
            }
            break;
        }
        if ctx.aborted {
            break;
        }
        if is_quiet {
            tried_quiets.push((mv, moved_piece));
        }
    }

    if moves_searched == 0 {
        // Everything was excluded or pruned: report a fail-low bound rather
        // than the `-INF` sentinel, which is not a score any caller can use.
        return alpha;
    }

    // Neither the TT store nor the correction-history update below applies
    // to a singular-extension verification search: it explored a
    // deliberately restricted move set (the TT move excluded) at a
    // half-depth window, so its result isn't a valid bound for this
    // position's *real* value and would only pollute both tables.
    if !ctx.aborted && excluded.is_none() {
        let flag = if best_score <= alpha_orig {
            TtFlag::Upper
        } else if best_score >= beta {
            TtFlag::Lower
        } else {
            TtFlag::Exact
        };

        // Feed this node's outcome back into the correction history: how
        // far off was the static eval from what the search actually
        // found? Skipped near mate scores, where that gap is meaningless
        // (it reflects distance-to-mate, not a misjudged position), and
        // when `best_score` is only a bound pointing the wrong way
        // relative to the raw eval: a fail-high proves the true score is
        // *at least* `best_score`, so it only measures the eval's error
        // once it already exceeds the eval (symmetrically for fail-low) —
        // learning from the other cases would feed the table a gap the
        // search never established.
        if let (Some(re), Some(ph)) = (raw_eval, node_pawn_hash) {
            let bound_usable = match flag {
                TtFlag::Exact => true,
                TtFlag::Lower => best_score > re,
                TtFlag::Upper => best_score < re,
            };
            if bound_usable && best_score.abs() < MATE_THRESHOLD {
                ctx.update_correction(ph, best_score - re, depth);
            }
        }

        ctx.tt.store(board.hash, depth as u8, score_to_tt(best_score, ply), flag, Some(best_move), tt_clock(board));
    }

    best_score
}

/// Draw-by-rule score for this node, or `None` if it should be searched
/// normally. A checkmate already on the board takes precedence over every
/// draw claim: the game ends the instant the king has no escape, whatever
/// the fifty-move counter says. Scoring the counter first turned `Qg7#` at
/// halfmove clock 100 into a draw, throwing away a won game in a legal
/// position.
///
/// The mate test only runs on the rare path where a draw rule actually
/// fired, so ordinary nodes still pay nothing but the three cheap checks.
fn terminal_draw_score(board: &mut Board, ply: u32, ctx: &Context) -> Option<i32> {
    let fifty_move = board.halfmove_clock >= 100;
    if !fifty_move && !ctx.is_repetition(board.hash, board.halfmove_clock) && !eval::is_insufficient_material(board) {
        return None;
    }
    // Insufficient material cannot be mate (there is nothing to mate with),
    // and a repetition means this exact position had legal continuations
    // before; only the fifty-move counter can coincide with a real mate.
    if fifty_move && movegen::is_in_check(board, board.side_to_move) && movegen::legal_moves_scratch(board).is_empty() {
        return Some(-MATE_SCORE + ply as i32);
    }
    Some(0)
}

/// Counting entry point to the quiescence search. `negamax` reaches
/// quiescence through `quiescence_inner` instead, having already counted the
/// frontier node itself.
fn quiescence(board: &mut Board, alpha: i32, beta: i32, ply: u32, ctx: &mut Context) -> i32 {
    ctx.nodes += 1;
    quiescence_inner(board, alpha, beta, ply, ctx)
}

fn quiescence_inner(board: &mut Board, mut alpha: i32, beta: i32, ply: u32, ctx: &mut Context) -> i32 {
    if ctx.should_stop() {
        return 0;
    }
    if ply >= MAX_PLY {
        return eval::evaluate_relative(board);
    }
    ctx.clear_pv(ply);
    // Quiescence only plays captures (material strictly drops each time) or,
    // while in check, forced evasions — so a real repetition inside this
    // recursion is essentially impossible and isn't tracked here, but the
    // fifty-move counter and insufficient material can both still be
    // crossed by a capture or a non-capture evasion a few plies into a
    // check-evasion chain, exactly like in `negamax`.
    //
    // Both move-generation branches below use `legal_moves_scratch` (make/
    // unmake on the caller's own board) rather than `generate_legal_moves`,
    // which clones the board per call — quiescence nodes are the majority
    // of any search, so that clone was the single most-executed allocation
    // in the engine (a Fase 3 oversight, fixed in this pass).
    if let Some(score) = terminal_draw_score(board, ply, ctx) {
        return score;
    }

    let in_check = movegen::is_in_check(board, board.side_to_move);

    // The full legal list is generated first and only then filtered down to
    // the noisy moves, so that "no captures worth reading out" and "no legal
    // move at all" stay distinguishable. Conflating them made quiescence
    // score a stalemate as an ordinary static evaluation: from
    // `8/8/8/8/8/8/2Q5/k2K4 w - - 0 1` the engine used to *choose* Kd2,
    // stalemating Black, and announce more than ten pawns of advantage.
    let mut moves = movegen::legal_moves_scratch(board);
    if moves.is_empty() {
        return if in_check { -MATE_SCORE + ply as i32 } else { 0 };
    }

    // When in check there is no "stand pat": the side to move might be
    // getting mated, and refusing to at least try every evasion (not just
    // captures) would let a mate hiding at the search horizon evaluate as
    // an ordinary, possibly favorable, static score instead.
    let stand_pat = if in_check {
        -MATE_SCORE + ply as i32
    } else {
        eval::evaluate_relative(board)
    };
    let mut best_score = stand_pat;
    if !in_check {
        if stand_pat >= beta {
            return stand_pat;
        }
        if stand_pat > alpha {
            alpha = stand_pat;
        }
        // Skip captures that lose material outright (negative SEE): they
        // essentially never help resolve a tactical sequence, and reading
        // them out wastes a large fraction of quiescence search's node
        // budget. Promotions are kept whatever SEE says — a pawn one push
        // from queening is exactly the kind of move that must not go
        // unresolved at the horizon, and SEE prices the promoted piece
        // poorly (see the note in `static_exchange_eval`).
        moves.retain(|m| {
            m.promotion().is_some() || (m.is_capture() && movegen::static_exchange_eval(board, *m) >= 0)
        });
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
        ctx.path.push(board.hash);
        let score = -quiescence(board, -beta, -alpha, ply + 1, ctx);
        ctx.path.pop();
        board.unmake_move(mv, undo);

        if score > best_score {
            best_score = score;
        }
        if score > alpha {
            alpha = score;
        }
        if alpha >= beta || ctx.aborted {
            break;
        }
    }
    // Fail-soft, matching `negamax`: returning `alpha` instead threw away
    // the difference between "failed low by a little" and "failed low by a
    // queen", which is information the parent's TT entry could have used.
    best_score
}

/// True if `color` has any piece besides pawns and the king, i.e. it is
/// safe to try a null move without risking a zugzwang position where
/// "passing" looks better than every legal move.
fn has_non_pawn_material(board: &Board, color: Color) -> bool {
    PieceType::ALL
        .iter()
        .any(|&kind| !matches!(kind, PieceType::Pawn | PieceType::King) && !board.pieces_of(color, kind).is_empty())
}

fn capture_victim_value(board: &Board, mv: Move) -> i32 {
    if mv.flag == MoveFlag::EnPassant {
        eval::piece_value(crate::types::PieceType::Pawn)
    } else {
        board.piece_at(mv.to).map(|p| eval::piece_value(p.kind)).unwrap_or(0)
    }
}

/// How much material a promotion adds beyond the pawn that vacated the
/// square. Zero for every other move.
fn promotion_gain(mv: Move) -> i32 {
    mv.promotion()
        .map(|kind| eval::piece_value(kind) - eval::piece_value(PieceType::Pawn))
        .unwrap_or(0)
}

/// MVV-LVA-ish move ordering: captures and promotions first (biggest gain /
/// smallest attacker first), quiet moves after in whatever order they were
/// generated. Used by quiescence search, which never sees killers, TT moves
/// or history.
///
/// The promotion term matters more than it looks: the generator emits
/// knight, bishop, rook, queen in that order, so without it a quiet
/// promotion to a queen scored the same 0 as any other quiet move and,
/// under a stable sort, ended up *behind* the three underpromotions.
fn move_order_score(board: &Board, mv: Move) -> i32 {
    let gain = promotion_gain(mv);
    if mv.is_capture() || gain > 0 {
        let victim = capture_victim_value(board, mv) + gain;
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
    let gain = promotion_gain(mv);
    if mv.is_capture() || gain > 0 {
        let see = movegen::static_exchange_eval(board, mv);
        if see >= 0 {
            let victim = capture_victim_value(board, mv) + gain;
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

/// Scores every move once into a scratch buffer owned by the `Context` and
/// sorts that, instead of letting the sort call the key function repeatedly:
/// `move_order_score_full` runs SEE for every capture, which is far too
/// expensive to recompute during comparisons. This replaces
/// `sort_by_cached_key`, which did cache the keys correctly but allocated a
/// fresh table at every node of the main search; the buffer is taken out of
/// the context and put back so the allocation survives from node to node.
///
/// The original index is part of the sort key, so moves with equal scores
/// keep generation order exactly as a stable sort would — otherwise the
/// large block of quiet moves that all score 0 early in a search would be
/// permuted arbitrarily.
fn order_moves_full(board: &Board, moves: &mut [Move], tt_move: Option<Move>, killers: [Option<Move>; 2], ctx: &mut Context, ply: u32) {
    let mut buf = std::mem::take(&mut ctx.order_buffer);
    buf.clear();
    buf.extend(
        moves
            .iter()
            .enumerate()
            .map(|(i, &m)| (move_order_score_full(board, m, tt_move, killers, ctx, ply), i as u16, m)),
    );
    buf.sort_unstable_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
    for (dst, src) in moves.iter_mut().zip(buf.iter()) {
        *dst = src.2;
    }
    ctx.order_buffer = buf;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::board::Board;

    /// A `Context` wired up the way `search_inner` builds one, for the tests
    /// that drive `negamax`/`search_root` directly instead of going through
    /// `search()`.
    fn test_context<'a>(
        stop: &'a AtomicBool,
        tt: &'a Tt,
        shared_nodes: &'a AtomicU64,
        root_hash: u64,
    ) -> Context<'a> {
        Context {
            nodes: 0,
            stop,
            hard_deadline: None,
            max_nodes: None,
            search_moves: None,
            path: vec![root_hash],
            path_start: 0,
            aborted: false,
            shared_nodes,
            published_nodes: 0,
            root_depth: 1,
            variety: false,
            tt,
            killers: vec![[None, None]; MAX_PLY as usize],
            history: [[0; 64]; 64],
            static_evals: vec![None; MAX_PLY as usize],
            moves_played: vec![None; (MAX_PLY + 1) as usize],
            cont_history: vec![0; PIECE_TYPE_COUNT * 64 * PIECE_TYPE_COUNT * 64],
            pawn_correction: vec![0; CORRECTION_HISTORY_SIZE],
            null_at_ply: vec![false; (MAX_PLY + 1) as usize],
            pv: vec![Move::new(Square(0), Square(0), MoveFlag::Quiet); ((MAX_PLY + 1) * MAX_PLY) as usize],
            pv_len: vec![0; (MAX_PLY + 2) as usize],
            order_buffer: Vec::new(),
        }
    }

    fn search_to_depth(fen: &str, depth: u32) -> SearchResult {
        let board = Board::from_fen(fen).unwrap();
        let stop = AtomicBool::new(false);
        let tt = Tt::new(1);
        let limits = SearchLimits { max_depth: Some(depth), ..Default::default() };
        search(&board, limits, &stop, &tt, &[], |_, _| {})
    }

    #[test]
    fn slot_count_never_exceeds_the_requested_hash_budget() {
        // Regression test: rounding the entry count *up* to a power of two
        // (instead of down) used to be able to nearly double the actual
        // memory used right above a power-of-two boundary — e.g. a 4096 MB
        // request used to allocate a real 6 GB, which crashed. Checked via
        // the pure `slot_count_for` (no real allocation) across a spread of
        // sizes up to `MAX_HASH_MB`-scale, since the boundary depends on
        // `size_of::<Option<TtEntry>>()`, which changes whenever `TtEntry`'s
        // fields do.
        let entry_size = std::mem::size_of::<Option<TtEntry>>();
        for size_mb in [1, 4, 16, 64, 256, 1024, 4096] {
            let used_bytes = slot_count_for(size_mb) * entry_size;
            assert!(
                used_bytes <= size_mb * 1024 * 1024,
                "Hash={size_mb}MB used {used_bytes} bytes, over budget"
            );
        }
    }

    #[test]
    fn new_actually_allocates_the_slot_count_that_slot_count_for_computes() {
        // A real (but small) allocation, to confirm `TranspositionTable::new`
        // is actually wired to `slot_count_for` and not just that the math
        // function alone is correct.
        let table = TranspositionTable::new(4);
        assert_eq!(table.entries.len(), slot_count_for(4));
    }

    #[test]
    fn store_keeps_a_deeper_entry_over_a_shallower_one_within_the_same_generation() {
        let mut table = TranspositionTable::new(1);
        table.store(42, 10, 100, TtFlag::Exact, None, 0);
        table.store(42, 1, 200, TtFlag::Exact, None, 0); // no new_search() in between
        let entry = table.probe(42).unwrap();
        assert_eq!(entry.depth, 10);
        assert_eq!(entry.score, 100);
    }

    #[test]
    fn store_lets_a_new_generation_overwrite_a_deeper_entry_from_an_older_search() {
        // Without aging, a deep entry stored several `go`s ago would keep
        // winning this depth-preferred comparison forever, since the table
        // is kept alive for the whole game (see `Tt`) — even after the
        // game has moved well past where that entry is still relevant.
        let mut table = TranspositionTable::new(1);
        table.store(42, 10, 100, TtFlag::Exact, None, 0);

        table.new_search();
        table.store(42, 1, 200, TtFlag::Exact, None, 0);

        let entry = table.probe(42).unwrap();
        assert_eq!(entry.depth, 1);
        assert_eq!(entry.score, 200);
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
    fn aspiration_search_converges_when_the_initial_guess_is_wildly_off() {
        // Queen vs bare king: the real score here is far above zero (easily
        // >500cp), so seeding `prev_score` at 0 (as if the position were
        // balanced) guarantees the first narrow window fails high and keeps
        // failing for a couple of retries, exercising the progressive
        // widening loop instead of a single lucky guess.
        let mut board = Board::from_fen("7k/8/8/8/8/2K5/8/3Q4 w - - 0 1").unwrap();
        let stop = AtomicBool::new(false);
        let tt = Tt::new(1);
        let shared_nodes = AtomicU64::new(0);
        let mut ctx = test_context(&stop, &tt, &shared_nodes, board.hash);
        let (score, best_move) = search_root_with_aspiration(&mut board, 4, 0, &mut ctx);
        assert!(best_move.is_some());
        assert!(score > 500, "expected a large positive score reflecting White's material edge, got {score}");
    }

    #[test]
    fn search_root_stores_a_bound_flag_when_the_window_fails_high() {
        // Queen-up position searched against a window far below its real
        // score (as an aspiration retry would): search_root must fail high
        // and store the result as a Lower bound. Regression test: it used
        // to store Exact unconditionally, handing other probes (including
        // Lazy SMP siblings) a score the search never actually proved.
        let mut board = Board::from_fen("7k/8/8/8/8/2K5/8/3Q4 w - - 0 1").unwrap();
        let stop = AtomicBool::new(false);
        let tt = Tt::new(1);
        let shared_nodes = AtomicU64::new(0);
        let mut ctx = test_context(&stop, &tt, &shared_nodes, board.hash);
        let (score, best_move) = search_root(&mut board, 3, -50, 50, &mut ctx);
        assert!(best_move.is_some());
        assert!(score >= 50, "expected a fail-high against this narrow window, got {score}");
        let entry = tt.probe(board.hash).expect("search_root must store a root entry");
        assert!(matches!(entry.flag, TtFlag::Lower), "a fail-high root result must be stored as a Lower bound");
    }

    #[test]
    fn go_mate_finds_a_mate_within_its_own_depth_budget() {
        // Scholar's mate position with `mate_in` as the only limit: the
        // implied depth budget (2*N) must be enough to find the mate in 1,
        // and the existing found-a-mate break stops the search right there.
        let board = Board::from_fen(
            "r1bqkb1r/pppp1ppp/2n2n2/4p2Q/2B1P3/8/PPPP1PPP/RN2KBNR w KQkq - 4 4",
        )
        .unwrap();
        let stop = AtomicBool::new(false);
        let tt = Tt::new(1);
        let limits = SearchLimits { mate_in: Some(1), ..Default::default() };
        let result = search(&board, limits, &stop, &tt, &[], |_, _| {});
        assert_eq!(result.best_move.map(|m| m.to_string()), Some("h5f7".to_string()));
        assert!(result.score >= MATE_SCORE - 10);
        assert!(result.depth <= 2, "mate_in 1 must cap the depth budget at 2 plies, got {}", result.depth);
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
    fn movestogo_divides_the_clock_by_moves_left_instead_of_the_sudden_death_default() {
        let limits = SearchLimits { white_time_ms: Some(10_000), moves_to_go: Some(5), ..Default::default() };
        let start = Instant::now();
        let budget = compute_time_budget(&limits, Color::White, start).unwrap();
        // 10s / 5 moves left = 2s soft budget, vs. 10s / 20 (sudden-death
        // default) = 500ms: movestogo must make the engine think in terms
        // of "spend a fifth of the clock now", not "spend a twentieth".
        let soft_ms = budget.soft.duration_since(start).as_millis();
        assert!((1_900..=2_100).contains(&soft_ms), "soft budget was {soft_ms}ms, expected ~2000ms");
    }

    #[test]
    fn movestogo_of_one_budgets_close_to_the_whole_remaining_clock() {
        // The last move before the clock resets: there's no reason to save
        // anything for "later" moves in this period, since there are none.
        let limits = SearchLimits { white_time_ms: Some(10_000), moves_to_go: Some(1), ..Default::default() };
        let start = Instant::now();
        let budget = compute_time_budget(&limits, Color::White, start).unwrap();
        let soft_ms = budget.soft.duration_since(start).as_millis();
        assert!(soft_ms > 9_000, "soft budget was {soft_ms}ms, expected close to the full 10s");
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
        use crate::eval::is_insufficient_material;
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
        // via transposition) and make sure the second, TT-warm run is still
        // coherent.
        //
        // Exact score equality is deliberately *not* asserted. It used to
        // hold only because a PV node would take a stored cutoff and replay
        // the first search's verdict move for move; now that PV nodes are
        // searched out properly (so the propagated line can't be truncated
        // by a cutoff), the warm run mixes in bounds proved at other depths
        // and can legitimately land a few centipawns away. What must hold is
        // that both runs return a legal move, that the reported line starts
        // with it, and that neither drifts into nonsense.
        let board = Board::start_pos();
        let stop = AtomicBool::new(false);
        let tt = Tt::new(1);
        let limits = SearchLimits { max_depth: Some(4), ..Default::default() };
        let first = search(&board, limits.clone(), &stop, &tt, &[], |_, _| {});
        let second = search(&board, limits, &stop, &tt, &[], |_, _| {});

        let legal = movegen::generate_legal_moves(&board);
        for result in [&first, &second] {
            let mv = result.best_move.expect("startpos siempre tiene jugadas legales");
            assert!(legal.contains(&mv));
            assert_eq!(result.pv.first(), Some(&mv));
            assert!(
                result.score.abs() < 100,
                "the opening position is close to balanced; got {}",
                result.score
            );
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
        // The tie-break is unit-tested directly rather than through a whole
        // search: whether any given position at any given depth happens to
        // produce two root moves within ROOT_TIE_EPSILON is a property of
        // the evaluation, not of this mechanism, and pinning it made the
        // test fail every time a term was retuned.
        let a = Move::new(Square::new(4, 1), Square::new(4, 3), MoveFlag::DoublePawnPush);
        let b = Move::new(Square::new(3, 1), Square::new(3, 3), MoveFlag::DoublePawnPush);
        let c = Move::new(Square::new(1, 0), Square::new(2, 2), MoveFlag::Quiet);

        assert_eq!(pick_near_best(&[(a, 20)], 20), None, "a single candidate leaves nothing to choose");
        assert_eq!(
            pick_near_best(&[(a, 20), (b, 20 - ROOT_TIE_EPSILON - 1)], 20),
            None,
            "a candidate outside the epsilon must not become a choice"
        );

        let tied = [(a, 20), (b, 20 - ROOT_TIE_EPSILON), (c, 19)];
        let mut seen = std::collections::HashSet::new();
        for _ in 0..200 {
            let picked = pick_near_best(&tied, 20).expect("three candidates are all within the epsilon");
            assert!([a, b, c].contains(&picked), "must only ever pick one of the tied candidates");
            seen.insert(picked.to_string());
        }
        assert!(seen.len() > 1, "the tie-break never varied across 200 draws");
    }

    #[test]
    fn the_default_configuration_plays_the_same_move_every_time() {
        // The flip side of the test above: with `Variety` off (the default)
        // the same position at the same depth must produce the same move
        // *and* the same node count, or no strength measurement taken with
        // this engine means anything.
        let board = Board::start_pos();
        let limits = SearchLimits { max_depth: Some(4), ..Default::default() };
        let mut seen = std::collections::HashSet::new();
        for _ in 0..5 {
            let stop = AtomicBool::new(false);
            let tt = Tt::new(1);
            let result = search(&board, limits.clone(), &stop, &tt, &[], |_, _| {});
            seen.insert((result.best_move.map(|m| m.to_string()), result.nodes));
        }
        assert_eq!(seen.len(), 1, "search must be deterministic with Variety off, got {seen:?}");
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
    fn quiescence_recognizes_insufficient_material_as_a_draw() {
        // K+B vs K: without its own insufficient-material check, quiescence
        // would fall through to a stand-pat eval favoring White by roughly
        // a bishop's worth of material, instead of recognizing this exact
        // position (reachable mid-capture-chain, not just at a `negamax`
        // node) as the dead draw it actually is.
        let mut board = Board::from_fen("4k3/8/8/8/8/8/8/3BK3 w - - 0 1").unwrap();
        let stop = AtomicBool::new(false);
        let tt = Tt::new(1);
        let shared_nodes = AtomicU64::new(0);
        let mut ctx = test_context(&stop, &tt, &shared_nodes, board.hash);
        assert_eq!(quiescence(&mut board, -INF, INF, 0, &mut ctx), 0);
    }

    #[test]
    fn gravity_saturates_history_within_its_bound() {
        // Hammering the same entry with the maximum bonus forever must
        // asymptote below HISTORY_MAX, never overflow past it — that's the
        // whole point of the gravity form over plain addition.
        let mut entry = 0;
        for _ in 0..1000 {
            gravity(&mut entry, history_bonus(64));
        }
        assert!(entry > 0 && entry <= HISTORY_MAX, "entry drifted out of range: {entry}");

        // And a string of penalties pulls it back down (and saturates on
        // the negative side the same way).
        for _ in 0..1000 {
            gravity(&mut entry, -history_bonus(64));
        }
        assert!((-HISTORY_MAX..0).contains(&entry), "entry drifted out of range: {entry}");
    }

    #[test]
    fn lmr_reduction_grows_with_depth_and_move_index() {
        // Late move at high depth must be reduced strictly more than an
        // early move at low depth, and the flat-1 behavior survives at the
        // shallow end where the old constant applied.
        assert!(lmr_reduction(3, 4) >= 1);
        assert!(lmr_reduction(12, 20) > lmr_reduction(3, 4));
        // Out-of-table indices clamp instead of panicking.
        assert_eq!(lmr_reduction(500, 500), lmr_reduction(MAX_PLY, 63));
    }

    #[test]
    fn lmp_threshold_is_never_below_the_full_depth_move_count() {
        // LMP must never prune before the first few ordered moves have been
        // tried — otherwise a node could fail low without ever really
        // looking at its best candidates.
        for depth in 1..=LMP_MAX_DEPTH {
            for improving in [false, true] {
                assert!(lmp_threshold(depth, improving) >= 2, "depth {depth} improving {improving}");
            }
        }
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
        // The path is built exactly as `search()` builds it from
        // `game_history`: every position reached along the way, not only the
        // one that repeats. `is_repetition` walks back two plies at a time
        // (only same-side-to-move positions can match), so the intermediate
        // positions have to really be there.
        let mut history = vec![earlier_hash];
        for mv in [m1, m2, m3, m4] {
            board.make_move(mv);
            history.push(board.hash);
        }
        assert_eq!(board.hash, earlier_hash, "the four king shuffles should land back on the exact same position");

        let stop = AtomicBool::new(false);
        let tt = Tt::new(1);
        // `earlier_hash` reappearing here represents a repetition from
        // before this `go`, not one discovered within the tree being
        // searched now.
        let shared_nodes = AtomicU64::new(0);
        let mut ctx = test_context(&stop, &tt, &shared_nodes, earlier_hash);
        ctx.path = history;

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

    #[test]
    fn a_mate_on_the_hundredth_halfmove_is_a_mate_and_not_a_draw() {
        // `Qg7#` with the fifty-move counter at 99: the move takes it to
        // 100, but a mate on the board ends the game whatever the counter
        // says. The draw rules used to be checked first and returned 0,
        // throwing away a won game in a perfectly legal position.
        let result = search_to_depth("7k/5Q2/6K1/8/8/8/8/8 w - - 99 1", 1);
        assert!(
            result.score >= MATE_THRESHOLD,
            "expected a mate score at halfmove clock 99, got {}",
            result.score
        );
        assert_eq!(result.best_move.map(|m| m.to_string()), Some("f7g7".to_string()));

        // One ply earlier the same mate must of course still be a mate.
        let earlier = search_to_depth("7k/5Q2/6K1/8/8/8/8/8 w - - 98 1", 1);
        assert!(earlier.score >= MATE_THRESHOLD);
    }

    #[test]
    fn a_quiet_position_at_the_fifty_move_limit_is_still_a_draw() {
        // The mate check above must not turn every fifty-move draw into a
        // search: a queen up, but the counter has run out.
        let board = Board::from_fen("4k3/8/8/8/8/8/8/3QK3 b - - 100 1").unwrap();
        let stop = AtomicBool::new(false);
        let tt = Tt::new(1);
        let shared_nodes = AtomicU64::new(0);
        let mut ctx = test_context(&stop, &tt, &shared_nodes, board.hash);
        let mut board = board;
        assert_eq!(negamax(&mut board, 3, 1, -INF, INF, &mut ctx, None), 0);
    }

    #[test]
    fn quiescence_recognizes_a_stalemate_instead_of_scoring_the_material() {
        // `8/8/8/8/8/8/2Q5/k2K4 w - - 0 1`: Kd2 stalemates Black. At depth 1
        // the resulting position lands directly in quiescence, which used to
        // see "no captures worth reading out", fall back to the static
        // evaluation and report the queen as a full advantage — so the
        // engine actively *chose* to stalemate its opponent.
        let result = search_to_depth("8/8/8/8/8/8/2Q5/k2K4 w - - 0 1", 1);
        assert_ne!(
            result.best_move.map(|m| m.to_string()),
            Some("d1d2".to_string()),
            "the engine must not walk into a stalemate it can see"
        );
    }

    #[test]
    fn a_transposition_table_score_is_not_reused_across_incompatible_clocks() {
        // Same placement, fifty-move counter 98 vs 0. The entry stored for
        // the first is a draw-by-rule verdict; reusing it for the second
        // turned a won queen ending into 0 cp.
        let near_limit = Board::from_fen("4k3/8/8/8/8/8/8/3QK3 w - - 98 1").unwrap();
        let fresh = Board::from_fen("4k3/8/8/8/8/8/8/3QK3 w - - 0 1").unwrap();
        assert_eq!(near_limit.hash, fresh.hash, "the two differ only in a field the hash excludes, by design");

        let stop = AtomicBool::new(false);
        let tt = Tt::new(1);
        let limits = SearchLimits { max_depth: Some(3), ..Default::default() };
        let _ = search(&near_limit, limits.clone(), &stop, &tt, &[], |_, _| {});
        let after = search(&fresh, limits, &stop, &tt, &[], |_, _| {});
        assert!(
            after.score > 300,
            "a queen up with a fresh clock must not inherit the near-limit draw score, got {}",
            after.score
        );
    }

    #[test]
    fn the_reported_line_always_starts_with_the_move_being_played() {
        // The PV used to be reconstructed after the fact by walking the
        // shared TT, which could hand back a line that had nothing to do
        // with the score it was printed next to.
        for fen in [
            "rnbqkbnr/pppppppp/8/8/8/8/PPPPPPPP/RNBQKBNR w KQkq - 0 1",
            "r1bqkb1r/pppp1ppp/2n2n2/4p2Q/2B1P3/8/PPPP1PPP/RN2KBNR w KQkq - 4 4",
            "8/8/8/8/8/8/2Q5/k2K4 w - - 0 1",
        ] {
            let result = search_to_depth(fen, 5);
            let best = result.best_move.expect("every one of these positions has a legal move");
            assert_eq!(result.pv.first(), Some(&best), "PV head must equal bestmove in {fen}");
            // And every move in the line must be playable in turn.
            let mut board = Board::from_fen(fen).unwrap();
            for mv in &result.pv {
                assert!(movegen::generate_legal_moves(&board).contains(mv), "illegal PV move {mv} in {fen}");
                board.make_move(*mv);
            }
        }
    }
}
