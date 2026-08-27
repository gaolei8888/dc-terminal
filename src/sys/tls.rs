//! 一个**真的能发 HTTPS 的** `ureq::Agent`。
//!
//! ## 为什么需要这个模块
//!
//! `ureq::AgentBuilder::new()` 在 Windows 上建出来的 agent 发不了 HTTPS。
//! 一个字节都发不出去，报的是 `Unknown Scheme: cannot make HTTPS request
//! because no TLS backend is configured`。
//!
//! 原因在 ureq 自己的源码里，注释就写在那段代码上面（`ureq-2.12.1`
//! `src/lib.rs:401`）：
//!
//! ```text
//! // native-tls is a feature that must be configured via the AgentBuilder.
//! // it is never picked up as a default (and never used by `ureq::get` etc).
//! ```
//!
//! 也就是说 `native-tls` **永远不会**被当成默认后端，必须显式
//! `.tls_connector(...)` 装上去。而 `default_tls_config()` 在没有 `tls`
//! （rustls）特性时返回的是一个专门用来报上面那句错的空实现
//! （`lib.rs:414`）。
//!
//! dct 在 Windows 上恰恰只开 native-tls——那是为了甩掉 `ring`，因为 `ring`
//! 要 `lib.exe`，而那东西只在几个 GB 的 Visual Studio Build Tools 里
//! （见 `Cargo.toml`）。所以「Windows 不用装 Build Tools」这个好处，
//! 一直是拿「Windows 上 HTTPS 全废」换来的，而且没人发现。
//!
//! 没人发现是因为这三条路都**没有也不可能有单测**（测试一律不碰网络，
//! 这是仓库的规矩），于是它们各自的注释都写着「在实测那一步验」，
//! 而实测从来没在 Windows 上做过：
//!
//! - `verify.rs` —— 存密钥之前拿真端点探一下
//! - `llm/http.rs` —— 给会话起名字
//! - `channel/telegram.rs` —— 手机通知
//! - `runtime.rs` —— 下自带的那份 Node（就是它把这件事撞出来的）
//!
//! ## 所以这里只做一件事
//!
//! 四处共用同一个入口。**不要再在别处写 `ureq::AgentBuilder::new()`**——
//! 那正是这个 bug 能同时存在于三个文件里的原因：每处都自己建，每处都
//! 漏掉同一样东西，而漏掉的后果只在另一个操作系统上、只在真打网络时出现。

/// 建一个已经配好 TLS 后端的 `AgentBuilder`。超时由调用方接着往上加——
/// 那件事每处的答案不一样（探测要短，下载要长）。
pub fn agent_builder() -> ureq::AgentBuilder {
    imp::configure(ureq::AgentBuilder::new())
}

#[cfg(not(windows))]
mod imp {
    /// Unix 上开的是 rustls（`tls` 特性），ureq 会把它当默认后端，
    /// 这里没有事情要做。
    pub fn configure(b: ureq::AgentBuilder) -> ureq::AgentBuilder {
        b
    }
}

#[cfg(windows)]
mod imp {
    use std::sync::Arc;

    /// 把系统自带的 schannel 装上去。
    ///
    /// `TlsConnector::new()` 失败时**不 panic，也不报错**，原样返回没配
    /// 后端的那个 builder：这个函数在守护进程启动路径上被调用，为了
    /// 「手机通知发不出去」把整个 dct 拽下水是不成比例的。真发请求时
    /// 会得到一句「连不上」，那正是调用方本来就要处理的情况。
    ///
    /// （ureq 自己那份 `ntls::default_tls_config` 在这里直接 `.unwrap()`，
    /// 我们不学它。）
    pub fn configure(b: ureq::AgentBuilder) -> ureq::AgentBuilder {
        match ureq::native_tls::TlsConnector::new() {
            Ok(c) => b.tls_connector(Arc::new(c)),
            Err(_) => b,
        }
    }
}

#[cfg(test)]
mod tests {
    /// 这条测试**不碰网络**，它盯的是别的东西：`ureq::Agent` 派生了
    /// `Debug`，而没配后端时那份 Debug 里印的是 ureq 自己那个空实现的
    /// 名字。真配上 schannel 之后名字会变。
    ///
    /// 为什么值得钉：这个 bug 的全部特征就是「编得过、跑得动、只在真发
    /// 请求时才炸，而且只在一个操作系统上」。没有一条不碰网络的测试拦得住
    /// 它——除了这一条。
    #[test]
    fn the_shared_agent_has_a_real_tls_backend_on_every_platform() {
        let agent = super::agent_builder().build();
        let debug = format!("{agent:?}");
        assert!(
            !debug.contains("NoTlsConfig"),
            "没有 TLS 后端的话，这台机器上所有 HTTPS 请求都会失败：{debug}"
        );
    }
}
