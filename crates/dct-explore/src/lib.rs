//! 摸索新平台、重放流程要用到的东西。所有对 dco 的调用都收在 `hands::Hands` 里，
//! 上层逻辑对着 `testing::FakeHands` 测，不碰真屏幕。
pub mod draft;
pub mod explore;
pub mod gate;
pub mod hands;
pub mod ladder;
pub mod options;
pub mod platform;
pub mod trace;
#[cfg(any(test, feature = "fake"))]
pub mod testing;

/// 给 Jeff 的一道选择题。和根包 `judge::ChoiceQuestion` 同形（顺序要保留：Jeff 按位置读选项），
/// 这里单独定义是为了不让本 crate 依赖根包；转换在根包里写。
#[derive(Debug, Clone, PartialEq)]
pub struct JevChoice {
    pub instructions: String,
    /// 按顺序：key -> 可选说明。
    pub criteria: Vec<(String, Option<String>)>,
}
