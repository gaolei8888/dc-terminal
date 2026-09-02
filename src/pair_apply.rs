//! 配对成功之后的落盘步骤：把 `pair::Approved` 写进 `secrets`/`profiles`，
//! 并按学生在配对屏上的勾选决定要不要顺手把 `[llm]` 也接上。
//!
//! **本任务（Task 3）只建这个桩。** 真正的落盘逻辑、它自己的单测，都是
//! Task 4 的范围——daemon 这边的轮询线程已经按这个签名接好了调用点
//! （见 `daemon.rs::spawn_pair_poller`），Task 4 只需要换掉函数体。

/// 配对落盘之后，两个方言口各自有没有可用的模型。
pub struct Ready {
    pub anthropic: bool,
    pub openai: bool,
}

/// `home` 是 dct 的家目录（`socket.parent()`），不是 profiles 目录——
/// 这一步既要往 `home/secrets.toml` 写钥匙，也可能要往 `home/profiles/`
/// 写一份新 profile，两者是同一个锚下的两个子路径。
pub fn apply(
    a: &crate::pair::Approved,
    home: &std::path::Path,
    secrets: &std::sync::Mutex<crate::secrets::SecretStore>,
    opt_in_llm: bool,
) -> Result<Ready, String> {
    let _ = (a, home, secrets, opt_in_llm);
    Err("not_implemented".into())
}
