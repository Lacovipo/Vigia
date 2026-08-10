//! Self-play harness: plays a fixed opening book between two Vigia (or any
//! UCI-speaking) binaries and reports a win/draw/loss tally and a rough Elo
//! estimate, so a change to search/eval/performance can be judged by actual
//! playing strength instead of "looks right".
//!
//! This is deliberately not a general tournament manager (no PGN output, no
//! adjudication tuning, no concurrency across games): it exists to answer
//! one question — "is candidate stronger than baseline?" — for two
//! Vigia builds on this machine, without depending on an external tool like
//! cutechess-cli that isn't installed here. It links against the `vigia`
//! library crate to referee games with the engine's own board/movegen
//! rules (legality, checkmate/stalemate, repetition, fifty-move,
//! insufficient material) rather than trusting either engine process to
//! report the game as over.
//!
//! Usage: `selfplay <engine_a> <engine_b> [movetime_ms] [max_plies]`
//! Exit code convention: 0 always (this is a report tool, not a gate);
//! failures to spawn/handshake with an engine abort with a message on
//! stderr and exit code 1.

use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::thread;
use std::time::{Duration, Instant};

use vigia::board::Board;
use vigia::movegen;
use vigia::types::Color;

const DEFAULT_MOVETIME_MS: u64 = 100;
/// Plies, not moves; 300 plies is generous enough that a real decisive game
/// is essentially never adjudicated away, while a genuine fortress/shuffle
/// still terminates instead of hanging the harness forever.
const DEFAULT_MAX_PLIES: u32 = 300;
/// On top of `movetime`, how long we wait for `bestmove` before declaring
/// the engine hung and forfeiting the game to its opponent. Generous
/// because a slow machine or a cold TT allocation shouldn't be mistaken for
/// a hang.
const BESTMOVE_GRACE: Duration = Duration::from_secs(10);
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(5);

/// A handful of well-known, short opening lines (in UCI long-algebraic
/// notation, applied from `startpos`) used to seed games. Two games are
/// played per line with colors swapped, so opening imbalance cancels out
/// instead of favoring whichever engine happens to play White. Kept as
/// move sequences rather than hand-typed FENs: a typo here is caught
/// immediately as an "illegal book move" error instead of silently
/// producing a corrupted position.
const OPENING_BOOK: &[&[&str]] = &[
    &[],
    &["e2e4", "e7e5", "g1f3", "b8c6", "f1b5"],
    &["e2e4", "c7c5"],
    &["e2e4", "e7e6"],
    &["d2d4", "d7d5", "c2c4"],
    &["d2d4", "g8f6", "c2c4", "g7g6"],
    &["c2c4"],
    &["g1f3", "d7d5", "c2c4"],
];

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Outcome {
    WhiteWins,
    BlackWins,
    Draw,
}

struct EngineProcess {
    label: String,
    child: Child,
    stdin: ChildStdin,
    rx: Receiver<String>,
}

impl EngineProcess {
    fn spawn(path: &str, label: &str) -> Result<Self, String> {
        let mut child = Command::new(path)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| format!("no se pudo lanzar '{path}': {e}"))?;
        let stdin = child.stdin.take().expect("stdin piped");
        let stdout = child.stdout.take().expect("stdout piped");

        // A dedicated reader thread turns the blocking `read_line` protocol
        // into a channel, so the caller can wait for a specific line with a
        // timeout instead of risking an indefinite block on a hung engine.
        let (tx, rx) = mpsc::channel();
        thread::spawn(move || {
            let mut reader = BufReader::new(stdout);
            let mut line = String::new();
            loop {
                line.clear();
                match reader.read_line(&mut line) {
                    Ok(0) | Err(_) => break,
                    Ok(_) => {
                        if tx.send(line.trim_end().to_string()).is_err() {
                            break;
                        }
                    }
                }
            }
        });

        let mut engine = EngineProcess { label: label.to_string(), child, stdin, rx };
        engine.send("uci")?;
        if !engine.wait_for_line("uciok", HANDSHAKE_TIMEOUT) {
            return Err(format!("{label}: no respondió 'uciok'"));
        }
        engine.new_game()?;
        Ok(engine)
    }

    fn send(&mut self, cmd: &str) -> Result<(), String> {
        writeln!(self.stdin, "{cmd}").map_err(|e| format!("{}: fallo al escribir: {e}", self.label))?;
        self.stdin.flush().map_err(|e| format!("{}: fallo al vaciar el buffer: {e}", self.label))
    }

    fn wait_for_line(&mut self, target: &str, timeout: Duration) -> bool {
        let deadline = Instant::now() + timeout;
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return false;
            }
            match self.rx.recv_timeout(remaining) {
                Ok(line) if line == target => return true,
                Ok(_) => continue,
                Err(_) => return false,
            }
        }
    }

    fn new_game(&mut self) -> Result<(), String> {
        self.send("ucinewgame")?;
        self.send("isready")?;
        if !self.wait_for_line("readyok", HANDSHAKE_TIMEOUT) {
            return Err(format!("{}: no respondió 'readyok'", self.label));
        }
        Ok(())
    }

    /// Sends `position ...` + `go movetime ...` and waits for `bestmove`.
    /// Returns `None` on timeout (treated by the caller as a forfeit, not a
    /// harness crash) rather than blocking forever on a hung process.
    fn best_move(&mut self, moves: &[String], movetime_ms: u64) -> Result<Option<String>, String> {
        let position_cmd = if moves.is_empty() {
            "position startpos".to_string()
        } else {
            format!("position startpos moves {}", moves.join(" "))
        };
        self.send(&position_cmd)?;
        self.send(&format!("go movetime {movetime_ms}"))?;

        let deadline = Instant::now() + Duration::from_millis(movetime_ms) + BESTMOVE_GRACE;
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Ok(None);
            }
            match self.rx.recv_timeout(remaining) {
                Ok(line) => {
                    if let Some(rest) = line.strip_prefix("bestmove ") {
                        return Ok(rest.split_whitespace().next().map(|s| s.to_string()));
                    }
                }
                Err(_) => return Ok(None),
            }
        }
    }
}

impl Drop for EngineProcess {
    fn drop(&mut self) {
        let _ = self.send("quit");
        for _ in 0..50 {
            if matches!(self.child.try_wait(), Ok(Some(_))) {
                return;
            }
            thread::sleep(Duration::from_millis(10));
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// The one dead-material rule in the project, now shared with the engine's
/// own evaluation and search instead of being a third near-identical copy
/// (which is how the three of them came to disagree about K+B vs K+B on
/// same-colored squares).
fn is_insufficient_material(board: &Board) -> bool {
    vigia::eval::is_insufficient_material(board)
}

/// Plays one game to completion. `white`/`black` are already-spawned engine
/// processes (freshly `new_game`d by the caller); `book` is a sequence of
/// legal opening moves applied before either engine is asked to move.
/// The arbiter (this harness, via `vigia::movegen`) decides when the game
/// is over — checkmate, stalemate, fifty-move rule, threefold repetition,
/// or insufficient material — independently of what either engine process
/// claims, so a buggy or foreign engine can't corrupt the result.
fn play_game(
    white: &mut EngineProcess,
    black: &mut EngineProcess,
    book: &[&str],
    movetime_ms: u64,
    max_plies: u32,
) -> Result<Outcome, String> {
    let mut board = Board::start_pos();
    let mut moves: Vec<String> = Vec::new();
    let mut hash_history: Vec<u64> = vec![board.hash];

    for &mv_str in book {
        let legal = movegen::generate_legal_moves(&board);
        let mv = legal
            .into_iter()
            .find(|m| m.to_string() == mv_str)
            .ok_or_else(|| format!("jugada de apertura ilegal en el libro: {mv_str}"))?;
        board.make_move(mv);
        moves.push(mv_str.to_string());
        hash_history.push(board.hash);
    }

    for _ply in 0..max_plies {
        let legal = movegen::generate_legal_moves(&board);
        if legal.is_empty() {
            return Ok(if movegen::is_in_check(&board, board.side_to_move) {
                if board.side_to_move == Color::White { Outcome::BlackWins } else { Outcome::WhiteWins }
            } else {
                Outcome::Draw
            });
        }
        if board.halfmove_clock >= 100 || is_insufficient_material(&board) {
            return Ok(Outcome::Draw);
        }
        if hash_history.iter().filter(|&&h| h == board.hash).count() >= 3 {
            return Ok(Outcome::Draw);
        }

        let mover = if board.side_to_move == Color::White { &mut *white } else { &mut *black };
        let forfeit_to = if board.side_to_move == Color::White { Outcome::BlackWins } else { Outcome::WhiteWins };

        let bm = match mover.best_move(&moves, movetime_ms)? {
            Some(bm) => bm,
            None => return Ok(forfeit_to), // hung / no reply in time
        };
        let mv = match legal.into_iter().find(|m| m.to_string() == bm) {
            Some(mv) => mv,
            None => return Ok(forfeit_to), // illegal or malformed move
        };
        board.make_move(mv);
        moves.push(bm);
        hash_history.push(board.hash);
    }
    Ok(Outcome::Draw) // ply cap: adjudicated draw, most likely a fortress/shuffle
}

/// Standard Elo-from-score-fraction estimate. `None` at the 0%/100%
/// extremes, where the formula blows up and the sample is too skewed to
/// mean anything anyway.
fn elo_diff(score_fraction: f64) -> Option<f64> {
    if score_fraction <= 0.0 || score_fraction >= 1.0 {
        return None;
    }
    let diff = -400.0 * (1.0 / score_fraction - 1.0).log10();
    // Avoids printing a cosmetic "-0" at an exactly even score, where the
    // formula's sign is meaningless floating-point noise.
    Some(if diff == 0.0 { 0.0 } else { diff })
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 3 {
        eprintln!("Uso: selfplay <engine_a> <engine_b> [movetime_ms] [max_plies]");
        eprintln!("Ejemplo: selfplay \"target/release/vigia.exe\" \"./Vigia 0.19.0.exe\" 100");
        eprintln!(
            "Nota: en Windows hay que anteponer \"./\" (o dar una ruta completa) a un \
             ejecutable del directorio actual; Command::new no busca ahí por su cuenta."
        );
        std::process::exit(1);
    }
    let path_a = &args[1];
    let path_b = &args[2];
    let movetime_ms: u64 = args.get(3).and_then(|s| s.parse().ok()).unwrap_or(DEFAULT_MOVETIME_MS);
    let max_plies: u32 = args.get(4).and_then(|s| s.parse().ok()).unwrap_or(DEFAULT_MAX_PLIES);

    let mut engine_a = match EngineProcess::spawn(path_a, "A") {
        Ok(e) => e,
        Err(err) => {
            eprintln!("Error: {err}");
            std::process::exit(1);
        }
    };
    let mut engine_b = match EngineProcess::spawn(path_b, "B") {
        Ok(e) => e,
        Err(err) => {
            eprintln!("Error: {err}");
            std::process::exit(1);
        }
    };

    println!("A = {path_a}");
    println!("B = {path_b}");
    println!("movetime = {movetime_ms} ms, max_plies = {max_plies}, aperturas = {}", OPENING_BOOK.len());
    println!();

    let (mut a_wins, mut b_wins, mut draws) = (0u32, 0u32, 0u32);
    let mut game_no = 0u32;

    for book in OPENING_BOOK {
        for &a_plays_white in &[true, false] {
            game_no += 1;
            if let Err(e) = engine_a.new_game() {
                eprintln!("Error preparando la partida {game_no}: {e}");
                continue;
            }
            if let Err(e) = engine_b.new_game() {
                eprintln!("Error preparando la partida {game_no}: {e}");
                continue;
            }

            let result = if a_plays_white {
                play_game(&mut engine_a, &mut engine_b, book, movetime_ms, max_plies)
            } else {
                play_game(&mut engine_b, &mut engine_a, book, movetime_ms, max_plies)
            };

            let outcome = match result {
                Ok(o) => o,
                Err(e) => {
                    eprintln!("Partida {game_no} abortada: {e}");
                    continue;
                }
            };

            let a_result = match (outcome, a_plays_white) {
                (Outcome::WhiteWins, true) | (Outcome::BlackWins, false) => "A gana",
                (Outcome::BlackWins, true) | (Outcome::WhiteWins, false) => "B gana",
                (Outcome::Draw, _) => "tablas",
            };
            match a_result {
                "A gana" => a_wins += 1,
                "B gana" => b_wins += 1,
                _ => draws += 1,
            }
            let color = if a_plays_white { "A=blancas" } else { "A=negras" };
            println!("Partida {game_no:>2} ({color}, apertura {} jugadas): {a_result}", book.len());
        }
    }

    let games = a_wins + b_wins + draws;
    let score_a = (a_wins as f64 + 0.5 * draws as f64) / games.max(1) as f64;
    println!();
    println!("--- Resultado ---");
    println!("Partidas: {games}  A: {a_wins}  B: {b_wins}  Tablas: {draws}");
    println!("Puntuación de A: {:.1}%", score_a * 100.0);
    match elo_diff(score_a) {
        Some(diff) => println!("Diferencia de Elo estimada (A - B): {diff:+.0}"),
        None => println!("Diferencia de Elo: no estimable (resultado en un extremo, muestra demasiado pequeña o unilateral)"),
    }
    println!(
        "Nota: {games} partidas es una muestra pequeña; esta cifra de Elo es orientativa, \
         no una medición estadísticamente sólida. Para más confianza, aumentar el número \
         de aperturas o repetir con distintos `movetime`."
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn elo_diff_is_zero_at_an_even_score() {
        assert!(elo_diff(0.5).unwrap().abs() < 1e-9);
    }

    #[test]
    fn elo_diff_is_positive_when_a_scores_above_half() {
        assert!(elo_diff(0.75).unwrap() > 0.0);
    }

    #[test]
    fn elo_diff_is_unestimable_at_the_extremes() {
        assert_eq!(elo_diff(0.0), None);
        assert_eq!(elo_diff(1.0), None);
    }

    #[test]
    fn bare_kings_is_insufficient_material() {
        let board = Board::from_fen("4k3/8/8/8/8/8/8/4K3 w - - 0 1").unwrap();
        assert!(is_insufficient_material(&board));
    }

    #[test]
    fn a_lone_extra_knight_is_still_insufficient_material() {
        let board = Board::from_fen("4k3/8/8/8/8/8/8/3NK3 w - - 0 1").unwrap();
        assert!(is_insufficient_material(&board));
    }

    #[test]
    fn two_extra_knights_are_enough_material_to_keep_playing_on() {
        let board = Board::from_fen("4k3/8/8/8/8/8/8/2NNK3 w - - 0 1").unwrap();
        assert!(!is_insufficient_material(&board));
    }

    #[test]
    fn a_lone_pawn_is_enough_material_to_keep_playing_on() {
        let board = Board::from_fen("4k3/8/8/8/8/8/4P3/4K3 w - - 0 1").unwrap();
        assert!(!is_insufficient_material(&board));
    }

    #[test]
    fn scholars_mate_book_line_leaves_black_with_no_legal_moves_while_in_check() {
        // 1.e4 e5 2.Bc4 Nc6 3.Qh5 Nf6?? 4.Qxf7#: exercises the same
        // `generate_legal_moves`/`is_in_check` combination `play_game` uses
        // to detect checkmate, without needing to spawn a real engine
        // process (which a unit test shouldn't depend on).
        let book: &[&str] =
            &["e2e4", "e7e5", "f1c4", "b8c6", "d1h5", "g8f6", "h5f7"];
        let mut board = Board::start_pos();
        for &mv_str in book {
            let legal = movegen::generate_legal_moves(&board);
            let mv = legal.into_iter().find(|m| m.to_string() == mv_str).unwrap();
            board.make_move(mv);
        }
        let legal = movegen::generate_legal_moves(&board);
        assert!(legal.is_empty());
        assert!(movegen::is_in_check(&board, board.side_to_move));
        assert_eq!(board.side_to_move, Color::Black);
    }
}
