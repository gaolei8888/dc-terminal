//! 三消游戏的选步：读到的棋盘 → 合法交换 → 模拟消除 → 打分。纯算法，不碰屏幕、磁盘和网络，
//! 所以能直接拿存下来的真实棋盘测（设计：docs/superpowers/specs/2026-10-02-dct-match3-play-design.md）。
pub mod ask;
pub mod board;
pub mod choose;
#[cfg(unix)]
pub mod dco;
pub mod genre;
pub mod goal;
pub mod lab;
pub mod navigate;
pub mod play;
pub mod screen;
pub mod sim;

pub use board::{fixed_ids, looks_like_board, Board, BoardError, Cell, GridRead};
pub use choose::{choose, Candidate, Features, Weights};
pub use sim::{Move, Pos};

#[cfg(test)]
mod tests;
#[cfg(test)]
mod play_tests;
#[cfg(all(test, unix))]
mod dco_tests;
