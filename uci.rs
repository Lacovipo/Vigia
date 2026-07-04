use std::io::{self, BufRead, Write};
use std::str::SplitWhitespace;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use crate::board::Board;
use crate::movegen;
use crate::search;

const ENGINE_NAME: &str = "Vigia 0.18.0";
const ENGINE_AUTHOR: &str = "Vigia Team";

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
    let _ = writeln!(out, "uciok");
    let _ = out.flush();
}

fn cmd_isready(out: &mut impl Write) {
    let _ = writeln!(out, "readyok");
    let _ = out.flush();
}

fn cmd_debug(engine: &mut Engine, mut tokens: SplitWhitespace) {
    match tokens.next() {
        Some("on") => engine.debug = true,
        Some("off") => engine.debug = false,
        _ => {}
    }
}

fn cmd_setoption(_engine: &mut Engine, _tokens: SplitWhitespace) {
    // No options exposed yet.
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
            "nodes" => {
                limits.max_nodes = tokens.get(i + 1).and_then(|s| s.parse().ok());
                i += 2;
            }
            "infinite" => {
                limits.infinite = true;
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
            // movestogo, mate, ponder: not supported yet, skip safely.
            _ => i += 1,
        }
    }
    limits
}

fn cmd_go(engine: &mut Engine, tokens: SplitWhitespace) {
    join_search_thread(engine);

    let limits = parse_go_limits(tokens, &engine.board);
    engine.stop_flag.store(false, Ordering::Relaxed);
    let stop_flag = Arc::clone(&engine.stop_flag);
    let tt = Arc::clone(&engine.tt);
    let board = engine.board.clone();
    // Everything before the current position; `search` appends the
    // current position's own hash to this as the start of its path.
    let game_history = engine.history[..engine.history.len().saturating_sub(1)].to_vec();

    engine.search_thread = Some(spawn_search(board, limits, stop_flag, tt, game_history, io::stdout()));
}

/// Runs the search on a background thread so the main UCI loop stays free
/// to read `stop`/`isready` while the engine "thinks", and writes `info`
/// lines plus the final `bestmove` to `output` as they become available.
fn spawn_search(
    board: Board,
    limits: search::SearchLimits,
    stop_flag: Arc<AtomicBool>,
    tt: Arc<search::Tt>,
    game_history: Vec<u64>,
    mut output: impl Write + Send + 'static,
) -> thread::JoinHandle<()> {
    thread::spawn(move || {
        let result = search::search(&board, limits, &stop_flag, &tt, &game_history, |res, elapsed| {
            print_info(&mut output, res, elapsed);
        });
        let mv = result
            .best_move
            .map(|m| m.to_string())
            .unwrap_or_else(|| "0000".to_string());
        let _ = writeln!(output, "bestmove {mv}");
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

fn cmd_ponderhit(_engine: &mut Engine) {}

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
            Arc::new(AtomicBool::new(false)),
            Arc::new(search::Tt::new(1)),
            Vec::new(),
            buf.clone(),
        );
        handle.join().unwrap();
        let out = String::from_utf8(buf.0.lock().unwrap().clone()).unwrap();
        let played = out
            .lines()
            .find_map(|l| l.strip_prefix("bestmove "))
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
            Arc::new(AtomicBool::new(false)),
            Arc::new(search::Tt::new(1)),
            Vec::new(),
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
}
