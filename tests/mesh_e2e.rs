//! 多电脑的本机端到端：一个真的中转、两个互不相干的守护进程，走完
//! A login → A invite（出 6 位邀请码）→ B join <码>（自动登录）→ peers → send。
//!
//! 默认不跑（`#[ignore]`），手动跑：
//!
//! ```text
//! cargo test --test mesh_e2e -- --ignored --nocapture
//! ```
//!
//! # 用到的真东西和替身
//!
//! - **中转是真的**：`dct-srv <127.0.0.1:0> --with-link --relay-keys <tmp>/issuer.pub`，
//!   子进程，跑的是工作区里此刻这份源码（先 `cargo build -p dct-srv`）。同
//!   `live_end_to_end.rs`：不把 `dct_srv` 链进这个测试二进制，免得 `dct` 这边
//!   的依赖树里长出 tokio。
//! - **守护进程是真的**：跟生产同一份 `dct::daemon::run`，各自一个临时 home
//!   （socket、secrets、profiles、`mesh/`、`config.toml` 全都跟着 socket 走，
//!   两台「电脑」之间什么都不共享）。
//! - **网关是替身**：一个手写在 `TcpListener` 上的 `/admin/api/relay/token`，
//!   收到请求就调 `dct-srv token mint` 用测试自己的 issuer 钥匙签一张令牌。
//!   dct 这边走的是真的 `dct login` 那条路（`http_transport`），只是网关换了人。
//! - **智能体是替身**：一个 `is_agent = true` 的 profile，命令是
//!   `sh -c 'echo E2E-READY; exec cat'`——打一行就绪标记，然后把敲进来的字
//!   原样吐回去。Windows 上的 `sh`/`cat` 借 Git for Windows 自带的那一套
//!   （`common::posix_tool`），跟别的集成测试一样。
//!
//! # 断言
//!
//! 1. B 用 A 的邀请码进了 A 的组，A 那边记下「电脑B 已加入」、码作废；
//! 2. B 的智能体会话里出现 `[来自 电脑A/终端 的留言 #xxxx]` 和正文；
//! 3. 中转进程的全部输出里找不到留言正文。

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::thread::sleep;
use std::time::{Duration, Instant};

use dct::client::Client;
use dct::proto::{MeshView, Request, Response, SendOutcome};

mod common;

/// 两台电脑拿到的令牌都属于这个账号——中转只让同一账号的端点互相看见。
const ACCOUNT: &str = "acct-e2e";

struct ChildGuard(Child);

impl Drop for ChildGuard {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// `target/<profile>/`：这个测试二进制在 `target/<profile>/deps/` 里。
fn target_profile_dir() -> PathBuf {
    let exe = std::env::current_exe().unwrap();
    exe.parent()
        .and_then(Path::parent)
        .expect("测试二进制不在 target/<profile>/deps 下面")
        .to_path_buf()
}

/// 先编一遍 `dct-srv`（跑的必须是此刻的源码），返回二进制的路径。
fn build_srv() -> PathBuf {
    let status = Command::new(env!("CARGO"))
        .args(["build", "--quiet", "-p", "dct-srv"])
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .status()
        .expect("跑不了 cargo build -p dct-srv");
    assert!(status.success(), "cargo build -p dct-srv 失败");
    let bin = target_profile_dir().join(format!("dct-srv{}", std::env::consts::EXE_SUFFIX));
    assert!(bin.is_file(), "编完了却找不到 {}", bin.display());
    bin
}

fn srv_ok(srv: &Path, args: &[&str]) -> String {
    let out = Command::new(srv).args(args).output().unwrap();
    assert!(
        out.status.success(),
        "dct-srv {args:?} 失败：{}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout).unwrap()
}

/// 起中转，返回（守卫，`http://127.0.0.1:PORT`，这个进程的全部输出）。
fn start_relay(srv: &Path, relay_keys: &Path) -> (ChildGuard, String, Arc<Mutex<String>>) {
    let mut child = Command::new(srv)
        .arg("127.0.0.1:0")
        .arg("--with-link")
        .arg("--relay-keys")
        .arg(relay_keys)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("起不来 dct-srv");
    let log = Arc::new(Mutex::new(String::new()));

    let stderr = child.stderr.take().unwrap();
    {
        let log = log.clone();
        std::thread::spawn(move || {
            let mut r = BufReader::new(stderr);
            let mut line = String::new();
            while r.read_line(&mut line).unwrap_or(0) > 0 {
                log.lock().unwrap().push_str(&line);
                line.clear();
            }
        });
    }

    let mut out = BufReader::new(child.stdout.take().unwrap());
    let mut first = String::new();
    out.read_line(&mut first).unwrap();
    log.lock().unwrap().push_str(&first);
    let addr = first
        .split_whitespace()
        .find(|t| t.starts_with("http://"))
        .unwrap_or_else(|| panic!("中转没报出地址：{first:?}；stderr：{}", log.lock().unwrap()))
        // 打印的是 `http://127.0.0.1:PORT` 后面紧跟中文逗号
        .split('，')
        .next()
        .unwrap()
        .to_string();
    {
        let log = log.clone();
        std::thread::spawn(move || {
            let mut line = String::new();
            while out.read_line(&mut line).unwrap_or(0) > 0 {
                log.lock().unwrap().push_str(&line);
                line.clear();
            }
        });
    }
    (ChildGuard(child), addr, log)
}

/// 网关替身收到过的请求：(Bearer, endpoint)。
type Seen = Arc<Mutex<Vec<(String, String)>>>;

/// 读一条 HTTP 请求：请求行、`Authorization`、body（按 `Content-Length` 读干净，
/// 理由见 `common::read_request_path`）。
fn read_request(stream: &mut TcpStream) -> Option<(String, String, String)> {
    let mut buf = Vec::new();
    let mut chunk = [0u8; 1024];
    let header_end = loop {
        let n = stream.read(&mut chunk).ok()?;
        if n == 0 {
            return None;
        }
        buf.extend_from_slice(&chunk[..n]);
        if let Some(p) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            break p + 4;
        }
    };
    let head = String::from_utf8_lossy(&buf[..header_end]).to_string();
    let header = |name: &str| {
        head.lines().find_map(|l| {
            let (k, v) = l.split_once(':')?;
            k.trim()
                .eq_ignore_ascii_case(name)
                .then(|| v.trim().to_string())
        })
    };
    let len: usize = header("content-length")
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);
    let mut body = buf[header_end..].to_vec();
    while body.len() < len {
        let n = stream.read(&mut chunk).ok()?;
        if n == 0 {
            break;
        }
        body.extend_from_slice(&chunk[..n]);
    }
    let path = head
        .lines()
        .next()
        .and_then(|l| l.split_whitespace().nth(1))
        .unwrap_or("")
        .to_string();
    Some((
        path,
        header("authorization").unwrap_or_default(),
        String::from_utf8_lossy(&body).to_string(),
    ))
}

/// 网关替身：只认 `POST /admin/api/relay/token`，拿 `dct-srv token mint` 签。
fn start_gateway(srv: PathBuf, issuer_key: PathBuf) -> (String, Seen) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let origin = format!("http://127.0.0.1:{}", listener.local_addr().unwrap().port());
    let seen: Seen = Arc::default();
    let s = seen.clone();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { continue };
            let Some((path, auth, body)) = read_request(&mut stream) else {
                continue;
            };
            let (status, resp) = if path != "/admin/api/relay/token" {
                ("404 Not Found", "{}".to_string())
            } else {
                let endpoint = serde_json::from_str::<serde_json::Value>(&body)
                    .ok()
                    .and_then(|v| v.get("endpoint")?.as_str().map(str::to_string))
                    .unwrap_or_default();
                s.lock().unwrap().push((auth, endpoint.clone()));
                // 跟网关一样签 7 天（dc_llm `routes_relay.TOKEN_TTL_SECONDS`）。
                // 签得比 1 天短的话，dct 每次轮询前都会认为「该续期了」
                // （`login::needs_renewal`），一登录就连着敲网关。
                let ttl = 7 * 24 * 3600u64;
                let token = srv_ok(
                    &srv,
                    &[
                        "token",
                        "mint",
                        "--key",
                        &issuer_key.display().to_string(),
                        "--account",
                        ACCOUNT,
                        "--endpoint",
                        &endpoint,
                        "--ttl",
                        &ttl.to_string(),
                    ],
                );
                let exp = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_secs()
                    + ttl;
                (
                    "200 OK",
                    serde_json::json!({ "token": token.trim(), "exp": exp }).to_string(),
                )
            };
            let _ = write!(
                stream,
                "HTTP/1.1 {status}\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{resp}",
                resp.len()
            );
            let _ = stream.flush();
            let _ = stream.shutdown(std::net::Shutdown::Write);
        }
    });
    (origin, seen)
}

/// TOML 基本字符串。Windows 路径里的反斜杠要转义。
fn toml_str(s: &str) -> String {
    format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""))
}

/// 一台「电脑」：一个守护进程，加上它 home 里预先写好的东西。
struct Machine {
    h: common::DaemonHandle,
    c: Client,
}

impl Machine {
    fn new(name: &str, gateway: &str, relay: &str, api_key: &str) -> Machine {
        let h = common::start_daemon();
        let home = h.sock.parent().unwrap().to_path_buf();
        let profiles = home.join("profiles");
        std::fs::create_dir_all(&profiles).unwrap();
        // 换令牌用的 DC 账号：`[api].base_url` 的 origin 就是网关。
        std::fs::write(
            profiles.join("dc.toml"),
            format!(
                "name = \"dc\"\ncommand = [\"echo\"]\npairable = true\n\n[api]\nbase_url = {}\nwire = \"anthropic\"\n",
                toml_str(&format!("{gateway}/v1"))
            ),
        )
        .unwrap();
        // 智能体替身。
        let sh = common::posix_tool("sh");
        std::fs::write(
            profiles.join("e2e-agent.toml"),
            format!(
                "name = \"e2e-agent\"\ncommand = [{}, \"-c\", \"echo E2E-READY; exec cat\"]\nis_agent = true\nidle_pattern = \"E2E-READY\"\n",
                toml_str(&sh)
            ),
        )
        .unwrap();
        std::fs::write(
            home.join("config.toml"),
            format!("[mesh]\nrelay = {}\n", toml_str(relay)),
        )
        .unwrap();
        std::fs::create_dir_all(home.join("mesh")).unwrap();
        std::fs::write(home.join("mesh").join("name"), name).unwrap();

        let mut c = h.client();
        let r = c
            .call(Request::SetSecret {
                profile: "dc".into(),
                value: api_key.into(),
            })
            .unwrap();
        assert!(matches!(r, Response::Ok), "SetSecret：{r:?}");
        Machine { h, c }
    }

    fn call(&mut self, req: Request) -> Response {
        self.c.call(req).unwrap()
    }

    fn view(&mut self, req: Request) -> MeshView {
        match self.call(req) {
            Response::Mesh(v) => v,
            other => panic!("预期 Mesh，实际 {other:?}"),
        }
    }

    fn screen(&mut self, id: u32) -> String {
        match self.call(Request::Screen { id }) {
            Response::Screen { lines, .. } => lines
                .iter()
                .map(|l| l.iter().map(|s| s.text.as_str()).collect::<String>())
                .map(|l| l.trim_end().to_string())
                .collect::<Vec<_>>()
                .join("\n"),
            other => panic!("预期 Screen，实际 {other:?}"),
        }
    }
}

/// 一直问，直到 `f` 给出 `Some`。
fn wait_until<T>(what: &str, timeout: Duration, mut f: impl FnMut() -> Option<T>) -> T {
    let deadline = Instant::now() + timeout;
    loop {
        if let Some(v) = f() {
            return v;
        }
        assert!(Instant::now() < deadline, "{timeout:?} 内没等到：{what}");
        sleep(Duration::from_millis(200));
    }
}

#[test]
#[ignore]
fn two_computers_join_and_leave_a_message_over_a_real_relay() {
    let srv = build_srv();
    let keys = tempfile::tempdir().unwrap();
    srv_ok(
        &srv,
        &[
            "token",
            "keygen",
            "--out",
            &keys.path().display().to_string(),
        ],
    );
    let (_relay_guard, relay, relay_log) = start_relay(&srv, &keys.path().join("issuer.pub"));
    let (gateway, gateway_seen) = start_gateway(srv.clone(), keys.path().join("issuer.key"));

    let mut a = Machine::new("电脑A", &gateway, &relay, "sk-e2e-a");
    let mut b = Machine::new("电脑B", &gateway, &relay, "sk-e2e-b");

    // ---- A 登录、出邀请码 ----
    let va = a.view(Request::MeshLogin);
    assert!(
        va.logged_in && va.in_group,
        "A 登录后该有一个只有自己的组：{va:?}"
    );
    assert_eq!(va.name, "电脑A");
    let invite = match a.call(Request::MeshInvite) {
        Response::MeshInvite(v) => v,
        other => panic!("预期 MeshInvite，实际 {other:?}"),
    };
    assert_eq!(invite.code.len(), 6);

    // ---- B：dct join <码>，没登录过，自动登录 ----
    // 两条连接刚起，中转那边未必已经把 A 登记上：B 问不到发邀请的电脑就再问
    // （只探问不消耗码）；别的失败都是真失败。码里故意带个空格，同人敲的。
    let typed = format!("{} {}", &invite.code[..3], &invite.code[3..]);
    let vb = wait_until("B 用邀请码进了组", Duration::from_secs(30), || {
        match b.call(Request::MeshJoin {
            code: typed.clone(),
            name: String::new(),
        }) {
            Response::Mesh(v) => Some(v),
            Response::Error(dct::proto::ErrorCode::Mesh(
                dct::proto::MeshProblem::NoInvite { .. },
            )) => None,
            other => panic!("B 加入失败：{other:?}"),
        }
    });
    assert!(vb.logged_in, "{vb:?}");
    assert!(
        vb.members.iter().any(|m| m.endpoint == va.endpoint),
        "B 的名单上有 A：{vb:?}"
    );
    {
        let seen = gateway_seen.lock().unwrap();
        assert_eq!(
            *seen,
            vec![
                ("Bearer sk-e2e-a".to_string(), va.endpoint.clone()),
                ("Bearer sk-e2e-b".to_string(), vb.endpoint.clone()),
            ],
            "api_key 只走 Bearer，端点是各自的；B 的登录是 join 顺手做的"
        );
    }
    let after = a.view(Request::MeshStatus);
    assert!(after.members.iter().any(|m| m.endpoint == vb.endpoint));
    assert!(after.invite.is_none(), "码用过就作废");
    assert_eq!(
        after.invite_note.map(|n| n.outcome),
        Some(dct::proto::InviteOutcome::Joined {
            name: "电脑B".into()
        })
    );

    // ---- B 开一个智能体会话 ----
    let repo = b.h.git_repo("proj");
    let sid = match b.call(Request::Create {
        dir: repo.display().to_string(),
        profile: "e2e-agent".into(),
        remember: false,
    }) {
        Response::Created { id } => id,
        other => panic!("预期 Created，实际 {other:?}"),
    };
    // 宽一点，标记行不折行。
    let _ = b.call(Request::Resize {
        id: sid,
        rows: 30,
        cols: 160,
    });
    wait_until("B 的智能体就绪", Duration::from_secs(15), || {
        b.screen(sid).contains("E2E-READY").then_some(())
    });

    // ---- peers：A 看得到 B 在线、开着这个会话 ----
    let list = match a.call(Request::MeshPeers) {
        Response::MeshPeers(l) => l,
        other => panic!("预期 MeshPeers，实际 {other:?}"),
    };
    let pb = list
        .iter()
        .find(|p| p.name == "电脑B")
        .unwrap_or_else(|| panic!("peers 里没有电脑B：{list:?}"));
    assert!(pb.online, "{pb:?}");
    assert_eq!(pb.sessions.len(), 1, "{pb:?}");

    // ---- send ----
    let body = format!(
        "e2e-canary-{:x} 跑一下测试",
        std::process::id() as u64 * 7919 + 17
    );
    let out = match a.call(Request::MeshSend {
        to: format!("电脑B/#{sid}"),
        text: body.clone(),
        from_session: None,
    }) {
        Response::MeshSent(o) => o,
        other => panic!("预期 MeshSent，实际 {other:?}"),
    };
    assert!(
        matches!(out, SendOutcome::Delivered | SendOutcome::Queued),
        "{out:?}"
    );

    let screen = wait_until(
        "留言出现在 B 的会话里",
        Duration::from_secs(15),
        || {
            let s = b.screen(sid);
            (s.contains("[来自 电脑A/终端 的留言 #") && s.contains(&body)).then_some(s)
        },
    );
    // 标记行本身：`[来自 电脑A/终端 的留言 #xxxx]`，# 后面是 4 位短编号。
    let line = screen
        .lines()
        .find(|l| l.contains("[来自 电脑A/终端 的留言 #"))
        .unwrap();
    let tail = line.split("的留言 #").nth(1).unwrap();
    let (short, _) = tail.split_once(']').expect("标记没有收口");
    assert_eq!(short.chars().count(), 4, "短编号：{line:?}");

    println!("B 的会话屏幕：\n{screen}");

    // 计数在整段敲完（连同回车）之后才记，比屏幕上出字晚一点。
    wait_until(
        "B 的看板记上一条留言",
        Duration::from_secs(10),
        || {
            let v = b.view(Request::MeshStatus);
            (v.messages.get(&sid) == Some(&1)).then_some(())
        },
    );

    // ---- 中转看不到原文 ----
    sleep(Duration::from_millis(300));
    let log = relay_log.lock().unwrap().clone();
    assert!(!log.contains(&body), "中转的输出里出现了留言原文：\n{log}");
    assert!(
        !log.contains("e2e-canary"),
        "中转的输出里出现了留言原文：\n{log}"
    );
    println!("中转输出（{} 字节）：\n{log}", log.len());

    let _ = b.call(Request::Kill { id: sid });
}
