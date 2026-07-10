use std::io::{self, BufRead, Write};
use std::str::SplitWhitespace;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use crate::board::Board;
use crate::eval;
use crate::movegen;
use crate::search;

const ENGINE_NAME: &str = "Vigia 0.22.0";
const ENGINE_AUTHOR: &str = "Vigia Team";
const MIN_HASH_MB: usize = 1;
/// Kept well below the multi-GB ceilings engines built for supercomputer
/// analysis use: this is a single-threaded engine with no need for a huge
/// table to see the benefit, and a `setoption` requesting a multi-GB table
/// is a real crash risk, not just an academic one — a request for 4096 MB
/// (this constant's previous value) reliably failed to allocate during
/// testing despite the machine having double-digit GB of free RAM, most
/// likely from contiguous-address-space fragmentation rather than a true
/// lack of memory. 1 GB is comfortably clear of that failure mode while
/// still being far more than this engine's search meaningfully benefits
/// from.
const MAX_HASH_MB: usize = 1024;
const MIN_THREADS: usize = 1;
/// A cap, not a recommendation: Lazy SMP here shares one `Tt` behind a
/// single `Mutex` (see `search::Tt`), not a lock-free table, so contention
/// on that lock — not core count — is what eventually limits how much
/// more searching more threads actually buys. 16 is comfortably inside
/// where that tradeoff still pays off on typical consumer hardware; a
/// lock-free table would be the natural next step before raising it.
const MAX_THREADS: usize = 16;

pub struct Engine {
    debug: bool,
    board: Board,
    /// Zobrist hash of every position reached so far in the current game
    /// (via `position ... moves ...`), including the current `board`
    /// itself as the last entry — passed to `search::search` so it can
    /// recognize a repetition against the real game, not just the search
    /// tree. Rebuilt from scratch each `position` command, since that
    /// command always fully replaces the board state.
    history: Vec<u64>,
    stop_flag: Arc<AtomicBool>,
    search_thread: Option<thread::JoinHandle<()>>,
    /// Kept alive across the whole game (not rebuilt per `go`), so
    /// transpositions found while thinking about one move are still there
    /// on the next. Cleared explicitly on `ucinewgame`.
    tt: Arc<search::Tt>,
    /// Number of Lazy SMP search threads `go` spawns (see `spawn_search`).
    /// 1 by default, matching the engine's behavior before Fase 4.
    threads: usize,
    /// Set for the duration of a `go ponder` search (cleared by `ponderhit`,
    /// or implicitly by the next `go`). The search itself runs exactly as
    /// it would for a real move — the time budget computed from `wtime`/
    /// `btime` is already correct for pondering, since our own clock isn't
    /// ticking while the opponent thinks regardless of how long that takes
    /// — this flag only gates *when `bestmove` is announced*: `spawn_search`
    /// withholds it until this becomes `false` (via `ponderhit`) or
    /// `stop_flag` becomes `true` (the opponent played something else),
    /// even if the search itself already finished on its own.
    pondering: Arc<AtomicBool>,
}

impl Engine {
    fn new() -> Self {
        let board = Board::start_pos();
        Engine {
            history: vec![board.hash],
            debug: false,
            board,
            stop_flag: Arc::new(AtomicBool::new(false)),
            search_thread: None,
            tt: Arc::new(search::Tt::default()),
            threads: MIN_THREADS,
            pondering: Arc::new(AtomicBool::new(false)),
        }
    }
}

pub fn run() {
    let stdin = io::stdin();
    let mut stdout = io::stdout();
    let mut engine = Engine::new();

    for line in stdin.lock().lines() {
        let line = match line {
            Ok(l) => l,
            Err(_) => break,
        };
        if !handle_command(&line, &mut engine, &mut stdout) {
            break;
        }
    }
}

/// Returns false when the engine should quit.
fn handle_command(line: &str, engine: &mut Engine, out: &mut impl Write) -> bool {
    let mut tokens = line.split_whitespace();
    let cmd = match tokens.next() {
        Some(c) => c,
        None => return true,
    };

    match cmd {
        "uci" => cmd_uci(out),
        "isready" => cmd_isready(out),
        "debug" => cmd_debug(engine, tokens),
        "setoption" => cmd_setoption(engine, tokens),
        "ucinewgame" => cmd_ucinewgame(engine),
        "position" => cmd_position(engine, tokens),
        "go" => cmd_go(engine, tokens),
        "stop" => cmd_stop(engine),
        "ponderhit" => cmd_ponderhit(engine),
        "eval" => cmd_eval(engine, out),
        "quit" => {
            join_search_thread(engine);
            return false;
        }
        // Unknown or not-yet-supported commands are ignored, never fatal.
        _ => {}
    }
    true
}

fn cmd_uci(out: &mut impl Write) {
    let _ = writeln!(out, "id name {ENGINE_NAME}");
    let _ = writeln!(out, "id author {ENGINE_AUTHOR}");
    let _ = writeln!(
        out,
        "option name Hash type spin default {} min {MIN_HASH_MB} max {MAX_HASH_MB}",
        search::DEFAULT_TT_SIZE_MB
    );
    let _ = writeln!(out, "option name Clear Hash type button");
    let _ = writeln!(
        out,
        "option name Threads type spin default {MIN_THREADS} min {MIN_THREADS} max {MAX_THREADS}"
    );
    let _ = writeln!(out, "option name Ponder type check default false");
    let _ = writeln!(out, "uciok");
    let _ = out.flush();
}

fn cmd_isready(out: &mut impl Write) {
    let _ = writeln!(out, "readyok");
    let _ = out.flush();
}

/// Prints the static evaluation of the current position with no search at
/// all — the same debugging extension (unofficial but near-universal)
/// supported by Stockfish, Berserk, Obsidian and friends under this exact
/// command name, which makes it trivial to compare Vigia's hand-crafted
/// number against theirs on an identical `position`.
fn cmd_eval(engine: &Engine, out: &mut impl Write) {
    let cp = eval::evaluate(&engine.board);
    let _ = writeln!(out, "Evaluation: {cp} (white side)");
    let _ = out.flush();
}

fn cmd_debug(engine: &mut Engine, mut tokens: SplitWhitespace) {
    match tokens.next() {
        Some("on") => engine.debug = true,
        Some("off") => engine.debug = false,
        _ => {}
    }
}

/// `setoption name <id> [value <x>]`: both `<id>` and `<x>` may contain
/// spaces (e.g. `Clear Hash`), so this can't just match on the second
/// token — it has to find the literal `value` keyword (if any) and join
/// everything on either side of it back into words.
fn cmd_setoption(engine: &mut Engine, tokens: SplitWhitespace) {
    let tokens: Vec<&str> = tokens.collect();
    if tokens.first() != Some(&"name") {
        return;
    }
    let value_idx = tokens.iter().position(|&t| t == "value");
    let name = tokens[1..value_idx.unwrap_or(tokens.len())].join(" ");
    let value = value_idx.map(|i| tokens[i + 1..].join(" "));

    match name.as_str() {
        "Hash" => {
            if let Some(mb) = value.and_then(|v| v.parse::<usize>().ok()) {
                join_search_thread(engine);
                engine.tt = Arc::new(search::Tt::new(mb.clamp(MIN_HASH_MB, MAX_HASH_MB)));
            }
        }
        "Clear Hash" => {
            join_search_thread(engine);
            engine.tt.clear();
        }
        "Threads" => {
            if let Some(n) = value.and_then(|v| v.parse::<usize>().ok()) {
                engine.threads = n.clamp(MIN_THREADS, MAX_THREADS);
            }
        }
        _ => {}
    }
}

fn cmd_ucinewgame(engine: &mut Engine) {
    join_search_thread(engine);
    engine.board = Board::start_pos();
    engine.history.clear();
    engine.history.push(engine.board.hash);
    engine.tt.clear();
}

fn cmd_position(engine: &mut Engine, tokens: SplitWhitespace) {
    join_search_thread(engine);

    let tokens: Vec<&str> = tokens.collect();

    let (board_tokens, move_tokens): (&[&str], &[&str]) =
        match tokens.iter().position(|&t| t == "moves") {
            Some(idx) => (&tokens[..idx], &tokens[idx + 1..]),
            None => (&tokens[..], &[]),
        };

    let board = match board_tokens.first() {
        Some(&"startpos") => Board::start_pos(),
        Some(&"fen") => match Board::from_fen(&board_tokens[1..].join(" ")) {
            Ok(b) => b,
            Err(_) => return, // FEN mal formada: se ignora el comando, se conserva la posición actual.
        },
        _ => return,
    };
    engine.board = board;
    engine.history.clear();
    engine.history.push(engine.board.hash);

    for &mv_str in move_tokens {
        let legal = movegen::generate_legal_moves(&engine.board);
        match legal.into_iter().find(|m| m.to_string() == mv_str) {
            Some(mv) => {
                engine.board.make_move(mv);
                engine.history.push(engine.board.hash);
            }
            None => break, // jugada ilegal o mal formada: se detiene ahí, sin fallar.
        }
    }
}

/// Every `go` subcommand keyword, used to know where an unbounded-length
/// argument list (currently just `searchmoves`) ends: as soon as the next
/// token is one of these, it's the start of a new subcommand, not another
/// move.
const GO_KEYWORDS: &[&str] = &[
    "searchmoves", "ponder", "wtime", "btime", "winc", "binc", "movestogo", "depth", "nodes", "mate", "movetime",
    "infinite",
];

fn parse_go_limits(tokens: SplitWhitespace, board: &Board) -> search::SearchLimits {
    let tokens: Vec<&str> = tokens.collect();
    let mut limits = search::SearchLimits::default();
    let mut i = 0;
    while i < tokens.len() {
        match tokens[i] {
            "depth" => {
                limits.max_depth = tokens.get(i + 1).and_then(|s| s.parse().ok());
                i += 2;
            }
            "movetime" => {
                limits.move_time_ms = tokens.get(i + 1).and_then(|s| s.parse().ok());
                i += 2;
            }
            "wtime" => {
                limits.white_time_ms = tokens.get(i + 1).and_then(|s| s.parse().ok());
                i += 2;
            }
            "btime" => {
                limits.black_time_ms = tokens.get(i + 1).and_then(|s| s.parse().ok());
                i += 2;
            }
            "winc" => {
                limits.white_inc_ms = tokens.get(i + 1).and_then(|s| s.parse().ok());
                i += 2;
            }
            "binc" => {
                limits.black_inc_ms = tokens.get(i + 1).and_then(|s| s.parse().ok());
                i += 2;
            }
            "movestogo" => {
                limits.moves_to_go = tokens.get(i + 1).and_then(|s| s.parse().ok());
                i += 2;
            }
            "nodes" => {
                limits.max_nodes = tokens.get(i + 1).and_then(|s| s.parse().ok());
                i += 2;
            }
            "infinite" => {
                limits.infinite = true;
                i += 1;
            }
            "ponder" => {
                limits.ponder = true;
                i += 1;
            }
            "searchmoves" => {
                let legal = movegen::generate_legal_moves(board);
                i += 1;
                let mut restrict = Vec::new();
                while i < tokens.len() && !GO_KEYWORDS.contains(&tokens[i]) {
                    if let Some(mv) = legal.iter().find(|m| m.to_string() == tokens[i]) {
                        restrict.push(*mv);
                    }
                    i += 1;
                }
                limits.search_moves = Some(restrict);
            }
            // mate: not supported yet, skip safely.
            _ => i += 1,
        }
    }
    limits
}

fn cmd_go(engine: &mut Engine, tokens: SplitWhitespace) {
    join_search_thread(engine);

    let limits = parse_go_limits(tokens, &engine.board);
    engine.stop_flag.store(false, Ordering::Relaxed);
    // Reset for this `go`, not just set on a ponder one: an ordinary `go`
    // right after a ponder search must not leave a stale `true` behind.
    engine.pondering.store(limits.ponder, Ordering::Relaxed);
    let signals = SearchSignals { stop: Arc::clone(&engine.stop_flag), pondering: Arc::clone(&engine.pondering) };
    let tt = Arc::clone(&engine.tt);
    let board = engine.board.clone();
    // Everything before the current position; `search` appends the
    // current position's own hash to this as the start of its path.
    let game_history = engine.history[..engine.history.len().saturating_sub(1)].to_vec();

    engine.search_thread = Some(spawn_search(board, limits, signals, tt, game_history, engine.threads, io::stdout()));
}

/// How often `spawn_search` polls `pondering`/`stop` while withholding
/// `bestmove` during a finished ponder search (see `spawn_search`'s doc
/// comment). Human/GUI-timescale event, so a few milliseconds of extra
/// latency between `ponderhit` and `bestmove` is unnoticeable; a plain
/// sleep loop is simplest and avoids adding a condvar just for this.
const PONDER_POLL_INTERVAL: Duration = Duration::from_millis(5);

/// The two flags a running search watches, bundled together purely to keep
/// `spawn_search`'s argument count down — every caller that needs one needs
/// the other. `stop`: abort immediately (`cmd_stop`, or `join_search_thread`
/// reclaiming the thread for the next command). `pondering`: see
/// `Engine::pondering` and `spawn_search`.
#[derive(Clone)]
struct SearchSignals {
    stop: Arc<AtomicBool>,
    pondering: Arc<AtomicBool>,
}

/// Runs the search on background thread(s) so the main UCI loop stays free
/// to read `stop`/`isready` while the engine "thinks", and writes `info`
/// lines plus the final `bestmove` to `output` as they become available.
///
/// With `threads > 1` this is Lazy SMP: every thread searches the same
/// position independently, sharing only `tt` (so a discovery any one of
/// them makes can cut off the others' work too), while each keeps its own
/// killers/history/continuation-history tables — sharing those as well
/// would need its own synchronization for comparatively little benefit
/// over the shared `tt` alone, and is not attempted here. Helper threads
/// (every thread but the first) stagger their starting depth and stay
/// silent (no `info` lines — interleaving several depth streams into one
/// UCI output would confuse a GUI); the final `bestmove` comes from
/// whichever thread reached the deepest completed iteration once every
/// thread has stopped, with the main thread winning ties.
///
/// If `pondering` is set (UCI `go ponder`), the search itself runs exactly
/// as normal — its time budget is already correct for pondering without
/// any changes (see `Engine::pondering`) — but `bestmove` is withheld
/// until `pondering` clears (`ponderhit`) or `stop_flag` is set (the
/// opponent played something else), even if the search already finished
/// on its own while still waiting on one of those.
fn spawn_search(
    board: Board,
    limits: search::SearchLimits,
    signals: SearchSignals,
    tt: Arc<search::Tt>,
    game_history: Vec<u64>,
    threads: usize,
    mut output: impl Write + Send + 'static,
) -> thread::JoinHandle<()> {
    thread::spawn(move || {
        let start = Instant::now();
        let helpers: Vec<thread::JoinHandle<search::SearchResult>> = (1..threads)
            .map(|i| {
                let board = board.clone();
                let limits = limits.clone();
                let stop_flag = Arc::clone(&signals.stop);
                let tt = Arc::clone(&tt);
                let game_history = game_history.clone();
                let role = search::SearchRole { start_depth: 1 + (i as u32 % 2), bump_generation: false };
                thread::spawn(move || search::search_inner(&board, limits, &stop_flag, &tt, &game_history, role, |_, _| {}))
            })
            .collect();

        let main_result = search::search(&board, limits, &signals.stop, &tt, &game_history, |res, elapsed| {
            print_info(&mut output, res, elapsed);
        });

        let mut best = main_result.clone();
        for handle in helpers {
            if let Ok(result) = handle.join() {
                if result.depth > best.depth {
                    best = result;
                }
            }
        }
        // A helper thread only ever wins by reaching a deeper completed
        // iteration than the main thread's own last reported `info` line;
        // print one more so the GUI's last-seen depth/score/pv matches the
        // move actually played, instead of trailing behind it.
        if best.depth > main_result.depth {
            print_info(&mut output, &best, start.elapsed());
        }

        while signals.pondering.load(Ordering::Relaxed) && !signals.stop.load(Ordering::Relaxed) {
            thread::sleep(PONDER_POLL_INTERVAL);
        }

        let mv = best.best_move.map(|m| m.to_string()).unwrap_or_else(|| "0000".to_string());
        if let Some(ponder_mv) = best.pv.get(1) {
            let _ = writeln!(output, "bestmove {mv} ponder {ponder_mv}");
        } else {
            let _ = writeln!(output, "bestmove {mv}");
        }
        let _ = output.flush();
    })
}

fn print_info(output: &mut impl Write, result: &search::SearchResult, elapsed: Duration) {
    let nps = if elapsed.as_secs_f64() > 0.0 {
        (result.nodes as f64 / elapsed.as_secs_f64()) as u64
    } else {
        0
    };
    let score_str = if result.score.abs() >= search::MATE_SCORE - 1000 {
        let plies_to_mate = search::MATE_SCORE - result.score.abs();
        let moves_to_mate = (plies_to_mate + 1) / 2;
        let signed = if result.score > 0 { moves_to_mate } else { -moves_to_mate };
        format!("mate {signed}")
    } else {
        format!("cp {}", result.score)
    };
    let pv = if result.pv.is_empty() {
        result.best_move.map(|m| m.to_string()).unwrap_or_default()
    } else {
        result.pv.iter().map(|m| m.to_string()).collect::<Vec<_>>().join(" ")
    };
    let _ = writeln!(
        output,
        "info depth {} score {} nodes {} nps {} time {} pv {}",
        result.depth,
        score_str,
        result.nodes,
        nps,
        elapsed.as_millis(),
        pv
    );
    let _ = output.flush();
}

fn join_search_thread(engine: &mut Engine) {
    if let Some(handle) = engine.search_thread.take() {
        engine.stop_flag.store(true, Ordering::Relaxed);
        let _ = handle.join();
    }
}

fn cmd_stop(engine: &mut Engine) {
    engine.stop_flag.store(true, Ordering::Relaxed);
}

/// The opponent played the move we were pondering on: let the (already
/// correctly time-budgeted, see `Engine::pondering`) search announce
/// `bestmove` whenever it's ready instead of withholding it forever.
fn cmd_ponderhit(engine: &mut Engine) {
    engine.pondering.store(false, Ordering::Relaxed);
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    fn run_command(cmd: &str) -> (String, bool) {
        let mut engine = Engine::new();
        let mut out = Vec::new();
        let keep_going = handle_command(cmd, &mut engine, &mut out);
        (String::from_utf8(out).unwrap(), keep_going)
    }

    #[test]
    fn uci_replies_with_uciok() {
        let (out, keep_going) = run_command("uci");
        assert!(out.contains("id name"));
        assert!(out.contains("uciok"));
        assert!(keep_going);
    }

    #[test]
    fn uci_advertises_hash_clear_hash_and_threads_options() {
        let (out, _) = run_command("uci");
        assert!(out.contains("option name Hash type spin default 64 min 1 max 1024"));
        assert!(out.contains("option name Clear Hash type button"));
        assert!(out.contains("option name Threads type spin default 1 min 1 max 16"));
    }

    #[test]
    fn setoption_hash_resizes_the_transposition_table() {
        let mut engine = Engine::new();
        let default_capacity = engine.tt.capacity();
        let mut out = Vec::new();
        handle_command("setoption name Hash value 1", &mut engine, &mut out);
        // 1 MB is smaller than the 64 MB default, so the slot count must
        // shrink; the exact number depends on entry layout, so this checks
        // the resize actually happened rather than pinning a magic number.
        assert!(engine.tt.capacity() < default_capacity);
    }

    #[test]
    fn setoption_hash_clamps_an_out_of_range_value() {
        let mut engine = Engine::new();
        let mut out = Vec::new();
        // Requesting far more than MAX_HASH_MB must not panic or allocate
        // an unbounded table; it should clamp instead. Compared against the
        // pure `search::slot_count_for` math rather than a second real
        // `Tt::new(MAX_HASH_MB)`, so this test needs only the one real
        // allocation `cmd_setoption` itself makes, not two.
        handle_command("setoption name Hash value 999999999", &mut engine, &mut out);
        assert_eq!(engine.tt.capacity(), search::slot_count_for(MAX_HASH_MB));
    }

    #[test]
    fn setoption_clear_hash_does_not_crash() {
        let mut engine = Engine::new();
        let mut out = Vec::new();
        let keep_going = handle_command("setoption name Clear Hash", &mut engine, &mut out);
        assert!(keep_going);
    }

    #[test]
    fn setoption_threads_sets_the_thread_count() {
        let mut engine = Engine::new();
        let mut out = Vec::new();
        assert_eq!(engine.threads, 1);
        let keep_going = handle_command("setoption name Threads value 4", &mut engine, &mut out);
        assert_eq!(engine.threads, 4);
        assert!(keep_going);
    }

    #[test]
    fn setoption_threads_clamps_an_out_of_range_value() {
        let mut engine = Engine::new();
        let mut out = Vec::new();
        handle_command("setoption name Threads value 999", &mut engine, &mut out);
        assert_eq!(engine.threads, MAX_THREADS);
        handle_command("setoption name Threads value 0", &mut engine, &mut out);
        assert_eq!(engine.threads, MIN_THREADS);
    }

    #[test]
    fn setoption_unknown_name_is_ignored_without_crashing() {
        let (out, keep_going) = run_command("setoption name MultiPV value 4");
        assert_eq!(out, "");
        assert!(keep_going);
    }

    #[test]
    fn isready_replies_readyok() {
        let (out, keep_going) = run_command("isready");
        assert_eq!(out, "readyok\n");
        assert!(keep_going);
    }

    #[test]
    fn quit_stops_the_loop() {
        let (out, keep_going) = run_command("quit");
        assert_eq!(out, "");
        assert!(!keep_going);
    }

    #[test]
    fn eval_reports_the_static_score_of_the_current_position() {
        let (out, keep_going) = run_command("eval");
        // Not 0 at startpos: White is to move, and the eval's tempo term
        // credits that (see `eval::TEMPO_BONUS`).
        assert_eq!(out, "Evaluation: 12 (white side)\n");
        assert!(keep_going);
    }

    #[test]
    fn unknown_command_is_ignored_without_crashing() {
        let (out, keep_going) = run_command("this is not a uci command");
        assert_eq!(out, "");
        assert!(keep_going);
    }

    #[test]
    fn empty_line_is_ignored() {
        let (out, keep_going) = run_command("");
        assert_eq!(out, "");
        assert!(keep_going);
    }

    #[test]
    fn position_with_moves_advances_the_board() {
        let mut engine = Engine::new();
        let mut out = Vec::new();
        handle_command("position startpos moves e2e4 e7e5", &mut engine, &mut out);
        assert_eq!(engine.board.side_to_move, crate::types::Color::White);
        assert_eq!(
            engine.board.to_fen(),
            "rnbqkbnr/pppp1ppp/8/4p3/4P3/8/PPPP1PPP/RNBQKBNR w KQkq e6 0 2"
        );
    }

    #[test]
    fn position_tracks_real_game_history_for_repetition_detection() {
        let mut engine = Engine::new();
        let mut out = Vec::new();
        handle_command("position startpos moves g1f3 g8f6 f3g1 f6g8", &mut engine, &mut out);
        // Back to the startpos position after two full moves of shuffling
        // knights out and back: the history must contain the starting
        // hash twice (once as the very first entry, once again now),
        // which is exactly what lets `search` recognize this as a
        // repetition it wouldn't see by searching the tree alone.
        assert_eq!(engine.history.len(), 5);
        assert_eq!(engine.history[0], engine.history[4]);
        assert_eq!(engine.history[4], engine.board.hash);
    }

    #[test]
    fn ucinewgame_resets_the_tracked_history() {
        let mut engine = Engine::new();
        let mut out = Vec::new();
        handle_command("position startpos moves e2e4 e7e5", &mut engine, &mut out);
        assert_eq!(engine.history.len(), 3);
        handle_command("ucinewgame", &mut engine, &mut out);
        assert_eq!(engine.history, vec![engine.board.hash]);
    }

    #[test]
    fn position_with_illegal_move_does_not_crash_and_stops_applying() {
        let mut engine = Engine::new();
        let mut out = Vec::new();
        handle_command("position startpos moves e2e4 e7e4 g8f6", &mut engine, &mut out);
        // e7e4 isn't legal for black; the board should stop right after e2e4.
        assert_eq!(
            engine.board.to_fen(),
            "rnbqkbnr/pppppppp/8/8/4P3/8/PPPP1PPP/RNBQKBNR b KQkq e3 0 1"
        );
    }

    #[test]
    fn position_with_malformed_fen_is_ignored_without_crashing() {
        let mut engine = Engine::new();
        let mut out = Vec::new();
        let before = engine.board.to_fen();
        handle_command("position fen not a real fen", &mut engine, &mut out);
        assert_eq!(engine.board.to_fen(), before);
    }

    #[test]
    fn go_searchmoves_restricts_the_root_to_the_requested_moves() {
        let board = Board::start_pos();
        let limits = parse_go_limits("depth 1 searchmoves e2e4 g1f3".split_whitespace(), &board);
        let restricted = limits.search_moves.expect("searchmoves should populate search_moves");
        let restricted_str: Vec<String> = restricted.iter().map(|m| m.to_string()).collect();
        assert_eq!(restricted_str, vec!["e2e4".to_string(), "g1f3".to_string()]);
    }

    #[test]
    fn go_searchmoves_ignores_illegal_moves_in_the_list() {
        let board = Board::start_pos();
        let limits = parse_go_limits("searchmoves e2e5 g1f3 depth 3".split_whitespace(), &board);
        let restricted = limits.search_moves.expect("searchmoves should populate search_moves");
        // e2e5 isn't a legal opening move, so only g1f3 survives; "depth"
        // must correctly end the searchmoves list rather than being
        // swallowed as another (illegal, and thus dropped) move token.
        assert_eq!(restricted.iter().map(|m| m.to_string()).collect::<Vec<_>>(), vec!["g1f3".to_string()]);
        assert_eq!(limits.max_depth, Some(3));
    }

    #[test]
    fn go_nodes_is_parsed_into_max_nodes() {
        let board = Board::start_pos();
        let limits = parse_go_limits("nodes 12345".split_whitespace(), &board);
        assert_eq!(limits.max_nodes, Some(12345));
    }

    #[test]
    fn go_movestogo_is_parsed_into_moves_to_go() {
        let board = Board::start_pos();
        let limits = parse_go_limits("wtime 60000 movestogo 20".split_whitespace(), &board);
        assert_eq!(limits.white_time_ms, Some(60_000));
        assert_eq!(limits.moves_to_go, Some(20));
    }

    #[test]
    fn go_ponder_is_parsed_into_the_ponder_flag_alongside_other_limits() {
        let board = Board::start_pos();
        let limits = parse_go_limits("ponder wtime 60000 btime 60000".split_whitespace(), &board);
        assert!(limits.ponder);
        assert_eq!(limits.white_time_ms, Some(60_000));
    }

    #[test]
    fn uci_advertises_the_ponder_option() {
        let (out, _) = run_command("uci");
        assert!(out.contains("option name Ponder type check default false"));
    }

    #[derive(Clone, Default)]
    struct SharedBuf(Arc<Mutex<Vec<u8>>>);

    impl Write for SharedBuf {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(buf);
            Ok(buf.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn go_from_startpos_returns_one_of_the_legal_opening_moves() {
        let board = Board::start_pos();
        let legal = movegen::generate_legal_moves(&board);
        let buf = SharedBuf::default();
        let handle = spawn_search(
            board,
            search::SearchLimits { max_depth: Some(1), ..Default::default() },
            SearchSignals { stop: Arc::new(AtomicBool::new(false)), pondering: Arc::new(AtomicBool::new(false)) },
            Arc::new(search::Tt::new(1)),
            Vec::new(),
            1,
            buf.clone(),
        );
        handle.join().unwrap();
        let out = String::from_utf8(buf.0.lock().unwrap().clone()).unwrap();
        let played = out
            .lines()
            .find_map(|l| l.strip_prefix("bestmove "))
            .and_then(|rest| rest.split_whitespace().next())
            .expect("se esperaba una línea bestmove");
        assert!(legal.iter().any(|m| m.to_string() == played));
    }

    #[test]
    fn go_from_checkmate_replies_null_move() {
        let board = Board::from_fen("4R1k1/5ppp/8/8/8/8/8/4K3 b - - 0 1").unwrap();
        let buf = SharedBuf::default();
        let handle = spawn_search(
            board,
            search::SearchLimits { max_depth: Some(3), ..Default::default() },
            SearchSignals { stop: Arc::new(AtomicBool::new(false)), pondering: Arc::new(AtomicBool::new(false)) },
            Arc::new(search::Tt::new(1)),
            Vec::new(),
            1,
            buf.clone(),
        );
        handle.join().unwrap();
        let out = String::from_utf8(buf.0.lock().unwrap().clone()).unwrap();
        assert!(out.lines().any(|l| l == "bestmove 0000"));
    }

    #[test]
    fn stop_then_quit_joins_the_running_search_without_hanging() {
        let mut engine = Engine::new();
        let mut out = Vec::new();
        handle_command("go infinite", &mut engine, &mut out);
        assert!(engine.search_thread.is_some());
        handle_command("stop", &mut engine, &mut out);
        handle_command("quit", &mut engine, &mut out);
        assert!(engine.search_thread.is_none());
    }

    #[test]
    fn go_with_several_threads_still_returns_a_legal_move() {
        let board = Board::start_pos();
        let legal = movegen::generate_legal_moves(&board);
        let buf = SharedBuf::default();
        let handle = spawn_search(
            board,
            search::SearchLimits { max_depth: Some(4), ..Default::default() },
            SearchSignals { stop: Arc::new(AtomicBool::new(false)), pondering: Arc::new(AtomicBool::new(false)) },
            Arc::new(search::Tt::new(1)),
            Vec::new(),
            4,
            buf.clone(),
        );
        handle.join().unwrap();
        let out = String::from_utf8(buf.0.lock().unwrap().clone()).unwrap();
        let played = out
            .lines()
            .find_map(|l| l.strip_prefix("bestmove "))
            .and_then(|rest| rest.split_whitespace().next())
            .expect("se esperaba una línea bestmove");
        assert!(legal.iter().any(|m| m.to_string() == played));
    }

    #[test]
    fn stop_then_quit_joins_a_multi_threaded_search_without_hanging() {
        let mut engine = Engine::new();
        let mut out = Vec::new();
        handle_command("setoption name Threads value 4", &mut engine, &mut out);
        handle_command("go infinite", &mut engine, &mut out);
        assert!(engine.search_thread.is_some());
        handle_command("stop", &mut engine, &mut out);
        handle_command("quit", &mut engine, &mut out);
        assert!(engine.search_thread.is_none());
    }

    #[test]
    fn ponder_search_withholds_bestmove_until_ponderhit_arrives() {
        // A depth-2 search finishes almost instantly, but while `pondering`
        // stays true `bestmove` must be withheld regardless — that's the
        // whole point of pondering: the search may well finish long before
        // the opponent actually moves.
        let board = Board::start_pos();
        let buf = SharedBuf::default();
        let pondering = Arc::new(AtomicBool::new(true));
        let handle = spawn_search(
            board,
            search::SearchLimits { max_depth: Some(2), ..Default::default() },
            SearchSignals { stop: Arc::new(AtomicBool::new(false)), pondering: Arc::clone(&pondering) },
            Arc::new(search::Tt::new(1)),
            Vec::new(),
            1,
            buf.clone(),
        );
        thread::sleep(Duration::from_millis(150));
        assert!(!String::from_utf8(buf.0.lock().unwrap().clone()).unwrap().contains("bestmove"));

        pondering.store(false, Ordering::Relaxed);
        handle.join().unwrap();
        let out = String::from_utf8(buf.0.lock().unwrap().clone()).unwrap();
        assert!(out.contains("bestmove"));
    }

    #[test]
    fn stop_also_releases_a_withheld_ponder_bestmove() {
        // The opponent played something other than the pondered move: the
        // GUI sends `stop`, not `ponderhit`, and the withheld bestmove
        // (now irrelevant, but still owed to the protocol) must still come
        // through rather than hanging forever waiting for a `ponderhit`
        // that will never arrive.
        let board = Board::start_pos();
        let buf = SharedBuf::default();
        let stop_flag = Arc::new(AtomicBool::new(false));
        let handle = spawn_search(
            board,
            search::SearchLimits { max_depth: Some(2), ..Default::default() },
            SearchSignals { stop: Arc::clone(&stop_flag), pondering: Arc::new(AtomicBool::new(true)) },
            Arc::new(search::Tt::new(1)),
            Vec::new(),
            1,
            buf.clone(),
        );
        thread::sleep(Duration::from_millis(150));
        assert!(!String::from_utf8(buf.0.lock().unwrap().clone()).unwrap().contains("bestmove"));

        stop_flag.store(true, Ordering::Relaxed);
        handle.join().unwrap();
        let out = String::from_utf8(buf.0.lock().unwrap().clone()).unwrap();
        assert!(out.contains("bestmove"));
    }

    #[test]
    fn cmd_ponderhit_clears_the_pondering_flag() {
        let mut engine = Engine::new();
        let mut out = Vec::new();
        handle_command("go ponder wtime 60000 btime 60000", &mut engine, &mut out);
        assert!(engine.pondering.load(Ordering::Relaxed));
        handle_command("ponderhit", &mut engine, &mut out);
        assert!(!engine.pondering.load(Ordering::Relaxed));
        handle_command("quit", &mut engine, &mut out);
    }

    #[test]
    fn a_plain_go_after_a_ponder_search_does_not_leave_pondering_set() {
        let mut engine = Engine::new();
        let mut out = Vec::new();
        handle_command("go ponder wtime 60000 btime 60000", &mut engine, &mut out);
        assert!(engine.pondering.load(Ordering::Relaxed));
        handle_command("stop", &mut engine, &mut out);
        handle_command("go depth 1", &mut engine, &mut out);
        assert!(!engine.pondering.load(Ordering::Relaxed));
        handle_command("quit", &mut engine, &mut out);
    }
}
