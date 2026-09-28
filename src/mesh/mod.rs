//! 多电脑：几台电脑上的智能体互相留言、派活。设计见
//! `docs/superpowers/specs/2026-09-28-dct-multi-machine-design.md`。
//!
//! 这一步只放 `login`（跟网关换中转令牌）。名单、钥匙落盘、收发信封等
//! 后续任务会陆续往这个模块里加。

pub mod login;
