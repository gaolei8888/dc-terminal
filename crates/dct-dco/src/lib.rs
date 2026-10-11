//! 连本机 dco：unix socket 上的 MCP（一行一条 JSON-RPC）。握手跟 dco 自己的 `dco call` 一样：
//! 先写 `{"dco_token": "<~/.dco/token>"}`，读到 `{"ok":true}`，再 initialize → notifications/initialized → tools/call。
//! 三消（`dct-game`）和摸索共用这一份。
//!
//! 数据类型（`DcoError`、`Reply`、超时常量）所有平台都有；走 unix socket 的 `DcoClient` 和测试用的
//! `fake` 只在 unix 上编——dct 也发 Windows 版，那边不能碰 `std::os::unix`。
use serde_json::Value;
use std::time::Duration;

#[cfg(unix)]
mod client;
#[cfg(unix)]
pub use client::DcoClient;

#[cfg(all(unix, any(test, feature = "fake")))]
pub mod fake;

/// dco 一次回话最多等多久。
pub const IO_TIMEOUT: Duration = Duration::from_secs(10);

/// 跑一整条流程（`run_procedure` 默认最多 600 秒）要等的时间：600 秒加余量。
pub const RUN_TIMEOUT: Duration = Duration::from_secs(660);

#[derive(Debug, Clone, PartialEq)]
pub struct DcoError {
    pub code: String,
    pub message: String,
    /// 工具报错时 dco 还带着的运行号：私人画面（`private_screen`）时运行停在那里等答复，
    /// 调用方要拿它去 `cancel_run`（dco tools/runs.rs `with_help_image`）。别的错一般是 None。
    pub run_id: Option<String>,
}

/// 一次工具回复：文字部分的 JSON，加上回复里带的图片（编号图、截图）。
pub struct Reply {
    pub json: Value,
    pub image_png_base64: Option<String>,
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use serde_json::json;

    const TOKEN: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

    #[test]
    fn call_full_returns_the_picture_next_to_the_json() {
        let dir = tempfile::tempdir().unwrap();
        let h = fake::serve(dir.path(), TOKEN, |tool, args| {
            assert_eq!(tool, "see");
            assert_eq!(args["marks"], true);
            (
                json!({"snapshot_id": "s1", "elements": []}),
                false,
                Some("iVBORw0KGgo=".into()),
            )
        });
        let mut c = DcoClient::connect(dir.path()).unwrap();
        let r = c
            .call_full("see", json!({"window": {"window_id": 1}, "marks": true}))
            .unwrap();
        assert_eq!(r.json["snapshot_id"], "s1");
        assert_eq!(r.image_png_base64.as_deref(), Some("iVBORw0KGgo="));
        drop(c);
        h.join().unwrap();
    }

    #[test]
    fn a_tool_error_keeps_dcos_code() {
        let dir = tempfile::tempdir().unwrap();
        let h = fake::serve(dir.path(), TOKEN, |_, _| {
            (
                json!({"error": {"code": "private_screen", "message": "私人画面"}}),
                true,
                None,
            )
        });
        let mut c = DcoClient::connect(dir.path()).unwrap();
        let e = c.call("see", json!({})).unwrap_err();
        assert_eq!(e.code, "private_screen");
        drop(c);
        h.join().unwrap();
    }

    #[test]
    fn a_private_screen_error_keeps_the_parked_run_id() {
        let dir = tempfile::tempdir().unwrap();
        let h = fake::serve(dir.path(), TOKEN, |_, _| {
            // dco tools/runs.rs with_help_image：私人画面只给错误、run_id 和 status。
            (
                json!({"error": {"code": "private_screen", "message": "私人画面"},
                       "run_id": "run-7", "status": "needs_help"}),
                true,
                None,
            )
        });
        let mut c = DcoClient::connect(dir.path()).unwrap();
        let e = c.call("answer_run", json!({})).unwrap_err();
        assert_eq!(e.code, "private_screen");
        assert_eq!(e.run_id.as_deref(), Some("run-7"));
        let e2 = c.call("see", json!({})).unwrap_err();
        assert_eq!(e2.run_id.as_deref(), Some("run-7"));
        drop(c);
        h.join().unwrap();
    }

    #[test]
    fn the_timeout_message_names_the_real_wait() {
        let dir = tempfile::tempdir().unwrap();
        let sock = dir.path().join("dco.sock");
        std::fs::write(
            dir.path().join("endpoint.json"),
            json!({"socket": sock}).to_string(),
        )
        .unwrap();
        std::fs::write(dir.path().join("token"), TOKEN).unwrap();
        let _l = std::os::unix::net::UnixListener::bind(&sock).unwrap();
        let e = DcoClient::connect_with_timeout(dir.path(), Duration::from_millis(200))
            .err()
            .unwrap();
        assert_eq!(e.code, "dco_timeout");
        assert!(e.message.contains("0.2 秒"), "{}", e.message);
        assert!(!e.message.contains("10 秒"));
    }
}
