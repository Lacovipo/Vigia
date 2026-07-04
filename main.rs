mod bitboard;
mod board;
mod eval;
mod movegen;
mod search;
mod types;
mod uci;
mod zobrist;

fn main() {
    uci::run();
}
