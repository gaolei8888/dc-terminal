//! `dct game`：让 dct 自己玩三消游戏。选步是纯算法（`crates/dct-game`），这里只管棋盘配置、
//! 记录、给用户看的话、装给 agent 的说明卡，和命令本身（设计：
//! docs/superpowers/specs/2026-10-02-dct-match3-play-design.md）。
pub mod log;
pub mod profile;
