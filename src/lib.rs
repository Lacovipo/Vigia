//! Library crate exposing Vigia's internals so more than one binary can
//! reuse them: the engine itself (`src/main.rs`), the test bench
//! (`src/bin/banco/`, see `docs/BancoPruebas.md`) and the older self-play
//! harness (`src/bin/selfplay.rs`). Both testing binaries link against
//! `board`/`movegen`/`eval` to referee games with the engine's own rules
//! instead of re-implementing chess from scratch — which is also how the
//! three divergent copies of the insufficient-material rule got collapsed
//! into one.

pub mod bitboard;
pub mod board;
pub mod eval;
pub mod kpk;
mod magic;
pub mod movegen;
pub mod search;
pub mod types;
pub mod uci;
pub mod zobrist;
