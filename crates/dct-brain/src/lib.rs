//! dct 的「大脑」：定档、执行票、签名核对。dct 和 dco 的安卓 App 共用，只写一份，
//! 手机才能在电脑关着时自己判断、自己把关（设计：docs/superpowers/specs/2026-09-27-dct-brain-design.md）。
//!
//! 两条约束：
//! - 没有 C 依赖（dct 的老规矩，也方便安卓用 NDK 编）。
//! - 不直接碰磁盘和网络：钥匙怎么存、流程从哪来，都由调用方传进来。测试不打网络，
//!   同一份代码在手机上也能跑。
pub mod canon;
pub mod steps;
pub mod tier;
pub mod ticket;
