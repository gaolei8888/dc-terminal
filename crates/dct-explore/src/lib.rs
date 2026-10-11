//! 摸索新平台、重放流程要用到的东西。所有对 dco 的调用都收在 `hands::Hands` 里，
//! 上层逻辑对着 `testing::FakeHands` 测，不碰真屏幕。
pub mod hands;
pub mod platform;
#[cfg(any(test, feature = "fake"))]
pub mod testing;
