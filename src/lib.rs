//! Library crate exposing Vigia's internals so more than one binary can
//! reuse them: the engine itself (`src/main.rs`) and the self-play testing
//! harness (`src/bin/selfplay.rs`), which links against `board`/`movegen`
//! to referee games between two engine binaries instead of re-implementing
//! chess rules from scratch.

pub mod bitboard;
pub mod board;
pub mod eval;
pub mod kpk;
pub mod movegen;
pub mod search;
pub mod types;
pub mod uci;
pub mod zobrist;
