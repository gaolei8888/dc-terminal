### Task 6: `dct invite`——印码、等结果、Ctrl-C 收回

**Files:**
- Modify: `src/mesh/cli.rs`（`dispatch` 加 `"invite"`；`invite()`；`cancel_invite_on_exit`；`run()` 里对 `invite` 装 Ctrl-C 钩子；旧守护进程那条测试加 `&["invite"]`）
- Modify: `src/main.rs`（`Some("invite")` 路由到 `mesh::cli::run`；帮助文字加 `dct invite`、改 `dct join <邀请码>`）
- Modify: `src/i18n.rs`（`mesh_invite_code`、`mesh_invite_hint`、`mesh_invite_countdown`、`mesh_invite_burned`、`mesh_invite_expired`、`mesh_invite_replaced`、`mesh_invite_gone`、`mesh_invite_usage`；一条文案测试）

**Interfaces:**
- Consumes: Task 5 的 `Request::MeshInvite`/`MeshInviteCancel`/`MeshStatus`、`Response::MeshInvite(InviteView)`、`MeshView.invite`/`invite_note`；已有的 `crate::sys::signal::restore_terminal_when_killed(restore: fn())`、`crate::client::Client::connect`/`call_within`、`crate::client::READ_TIMEOUT`、`msg::mesh_member_joined`。
- Produces: `mesh::cli::invite(call, out, err, lang, tty: bool, now: &dyn Fn() -> u64, pause: &dyn Fn()) -> i32`；上面八个 `msg::` 函数（签名见补丁）。

**行为**：印 `邀请码 482 913 · 10 分钟内有效` 和 `在新电脑上运行：dct join 482913（码只能用一次）`；每秒问一次 `MeshStatus`：`invite_note.id == 我的 id` → 按结果说话（进组退 0，作废/过期退 1）；`invite.id` 变了 → 「已经换成新的了」退 1；没有码也没有我的结果 → 「已经收回了」退 1；终端里每秒 `\r还剩 9:41` 覆盖一行，管道里不刷。

- [ ] **Step 1: 写失败的测试**

`src/mesh/cli.rs` 测试模块末尾：

```rust
    fn invite_view(id: u64, code: &str) -> crate::proto::InviteView {
        crate::proto::InviteView {
            id,
            code: code.into(),
            expires_at: 1_000 + 600,
        }
    }

    /// 一份带着邀请码（或者结果）的现状。
    fn with_invite(
        invite: Option<crate::proto::InviteView>,
        note: Option<(u64, crate::proto::InviteOutcome)>,
    ) -> MeshView {
        let mut v = view(true, &[("Mac", true)]);
        v.invite = invite;
        v.invite_note = note.map(|(id, outcome)| crate::proto::InviteNote { id, outcome });
        v
    }

    fn run_invite(answers: Vec<Response>, tty: bool) -> (i32, String, String, Vec<String>) {
        let sc = Script::new(answers);
        let (mut out, mut err) = (vec![], vec![]);
        let code = invite(&mut sc.call(), &mut out, &mut err, Lang::Zh, tty, &|| 1_001, &|| {});
        let seen = sc.seen.borrow().clone();
        (code, s(&out), s(&err), seen)
    }

    /// 印出码（带空格）和怎么用，等到有人用它进组：报名字、退 0。
    #[test]
    fn invite_prints_the_code_and_waits_until_someone_joins() {
        use crate::proto::InviteOutcome::Joined;
        let (code, out, err, seen) = run_invite(
            vec![
                Response::MeshInvite(invite_view(3, "012345")),
                Response::Mesh(with_invite(Some(invite_view(3, "012345")), None)),
                Response::Mesh(with_invite(
                    None,
                    Some((
                        3,
                        Joined {
                            name: "公司电脑".into(),
                        },
                    )),
                )),
            ],
            false,
        );
        assert_eq!(code, 0, "{err}");
        assert_eq!(
            out,
            "邀请码 012 345 · 10 分钟内有效\n在新电脑上运行：dct join 012345（码只能用一次）\n公司电脑 已加入\n"
        );
        assert_eq!(seen, ["MeshInvite", "MeshStatus", "MeshStatus"]);
    }

    #[test]
    fn invite_says_when_the_code_was_burned_expired_replaced_or_withdrawn() {
        use crate::proto::InviteOutcome::{Burned, Expired};
        let cases: Vec<(MeshView, &str)> = vec![
            (
                with_invite(None, Some((3, Burned))),
                "有人用错码试过一次，码已作废。要加电脑就重新生成一个（看板上按 a，或运行 dct invite）",
            ),
            (
                with_invite(None, Some((3, Expired))),
                "邀请码过期了，没人用。要加电脑就重新生成一个（看板上按 a，或运行 dct invite）",
            ),
            (
                with_invite(Some(invite_view(4, "999999")), None),
                "这个邀请码已经换成新的了（看板上又按了 a？），以新的为准",
            ),
            (with_invite(None, None), "邀请码已经收回了"),
            // 上一个码的结果不算这一个的。
            (
                with_invite(None, Some((2, Burned))),
                "邀请码已经收回了",
            ),
        ];
        for (status, want) in cases {
            let (code, _, err, _) = run_invite(
                vec![
                    Response::MeshInvite(invite_view(3, "482913")),
                    Response::Mesh(status),
                ],
                false,
            );
            assert_eq!(code, 1);
            assert_eq!(err.trim(), want);
        }
    }

    /// 终端里每秒刷一行倒计时（`\r` 覆盖）；结果另起一行。管道里不刷。
    #[test]
    fn invite_counts_down_only_on_a_terminal() {
        use crate::proto::InviteOutcome::Joined;
        let answers = || {
            vec![
                Response::MeshInvite(invite_view(3, "482913")),
                Response::Mesh(with_invite(Some(invite_view(3, "482913")), None)),
                Response::Mesh(with_invite(None, Some((3, Joined { name: "B".into() })))),
            ]
        };
        let (_, out, _, _) = run_invite(answers(), true);
        assert!(out.contains("\r还剩 9:59"), "{out:?}");
        assert!(out.ends_with("\nB 已加入\n"), "{out:?}");
        let (_, out, _, _) = run_invite(answers(), false);
        assert!(!out.contains('\r'), "{out:?}");
    }

    #[test]
    fn invite_before_login_says_to_log_in() {
        let (code, out, err, _) = run_invite(
            vec![Response::Error(ErrorCode::Mesh(MeshProblem::NotLoggedIn))],
            false,
        );
        assert_eq!(code, 1);
        assert!(out.is_empty());
        assert_eq!(err.trim(), "还没登录多电脑，先运行 dct login");
    }

    #[test]
    fn invite_takes_no_arguments() {
        let sc = Script::new(vec![Response::Hello {
            protocol: crate::proto::PROTOCOL_VERSION,
        }]);
        let (mut out, mut err) = (vec![], vec![]);
        let rest = vec!["482913".to_string()];
        let code = dispatch("invite", &rest, &mut sc.call(), &mut no_ask, &mut out, &mut err, Lang::Zh);
        assert_eq!(code, 2);
        assert!(s(&err).contains("用法：dct invite"), "{}", s(&err));
    }
```

`src/i18n.rs` 测试模块里（`mesh_strings_say_exactly_what_the_brief_says` 之前）：

```rust
    /// 邀请码那几句：中文照设计文档一字不差，英文里没有汉字。
    #[test]
    fn invite_strings_say_what_the_design_says() {
        assert_eq!(
            msg::mesh_invite_code(Lang::Zh, "482 913"),
            "邀请码 482 913 · 10 分钟内有效"
        );
        assert_eq!(
            msg::mesh_joined_with(Lang::Zh, &["Mac".into(), "公司电脑".into()]),
            "已加入「我的电脑」，组里有：Mac、公司电脑"
        );
        assert_eq!(msg::mesh_invite_countdown(Lang::Zh, 601), "还剩 10:01");
        assert_eq!(msg::mesh_invite_countdown(Lang::Zh, 9), "还剩 0:09");
        assert_eq!(
            msg::error(
                Lang::Zh,
                &crate::proto::ErrorCode::Mesh(crate::proto::MeshProblem::WrongInviteCode)
            ),
            "邀请码不对或已作废，请在老电脑上按 a 重新生成"
        );
        for s in [
            msg::mesh_invite_code(Lang::En, "482 913"),
            msg::mesh_invite_hint(Lang::En, "482913"),
            msg::mesh_invite_countdown(Lang::En, 61),
            msg::mesh_invite_burned(Lang::En),
            msg::mesh_invite_expired(Lang::En),
            msg::mesh_invite_replaced(Lang::En),
            msg::mesh_invite_gone(Lang::En),
            msg::mesh_invite_usage(Lang::En),
            msg::mesh_joined_with(Lang::En, &["A".into()]),
            msg::mesh_renamed_on_join(Lang::En, "A 2"),
            msg::mesh_join_usage(Lang::En),
        ] {
            assert!(!has_han(&s), "{s}");
        }
    }
```

- [ ] **Step 2: 跑测试确认失败**

Run: `~/.cargo/bin/cargo test --lib -- mesh::cli i18n`
Expected: 编译失败：`cannot find function `invite``、`no function `mesh_invite_code` in module `msg``。

- [ ] **Step 3: 实现**

先把 Step 1 里手改过的文件还原（补丁里已经包含同样的测试）：`git checkout -- src/mesh/cli.rs src/i18n.rs`。

把下面整个补丁存成 `/tmp/dct-invite-t6.patch`（**只存 ```diff 块里的内容**），在 worktree 根目录应用：

```bash
git apply --check /tmp/dct-invite-t6.patch && git apply /tmp/dct-invite-t6.patch
```

`--check` 报错说明前面的任务跟本计划的代码有出入：改用 `git apply --3way`，还不行就照补丁逐块手改（上下文行用来定位），**不要**跳过任何一块。

```diff
diff --git a/src/i18n.rs b/src/i18n.rs
index d1bbe2a..b2aa579 100644
--- a/src/i18n.rs
+++ b/src/i18n.rs
@@ -2417,6 +2417,74 @@ pub mod msg {
         )
     }
 
+    /// 老电脑上 `dct invite` / 看板：码本身。`spaced` 是 `482 913` 这样。
+    pub fn mesh_invite_code(lang: Lang, spaced: &str) -> String {
+        t!(
+            lang,
+            en: format!("Invite code {spaced} · valid for 10 minutes"),
+            zh: format!("邀请码 {spaced} · 10 分钟内有效"),
+        )
+    }
+
+    /// 码下面那一行：新电脑上敲什么。
+    pub fn mesh_invite_hint(lang: Lang, code: &str) -> String {
+        t!(
+            lang,
+            en: format!("On the new computer run: dct join {code} (the code works once)"),
+            zh: format!("在新电脑上运行：dct join {code}（码只能用一次）"),
+        )
+    }
+
+    /// 倒计时：还剩几分几秒。
+    pub fn mesh_invite_countdown(lang: Lang, secs_left: u64) -> String {
+        let (m, s) = (secs_left / 60, secs_left % 60);
+        t!(
+            lang,
+            en: format!("{m}:{s:02} left"),
+            zh: format!("还剩 {m}:{s:02}"),
+        )
+    }
+
+    pub fn mesh_invite_burned(lang: Lang) -> String {
+        t!(
+            lang,
+            en: "Someone tried a wrong code once, so this code is no longer valid. To add a computer, make a new one (press a on the board, or run dct invite)".to_string(),
+            zh: "有人用错码试过一次，码已作废。要加电脑就重新生成一个（看板上按 a，或运行 dct invite）".to_string(),
+        )
+    }
+
+    pub fn mesh_invite_expired(lang: Lang) -> String {
+        t!(
+            lang,
+            en: "The invite code expired without being used. To add a computer, make a new one (press a on the board, or run dct invite)".to_string(),
+            zh: "邀请码过期了，没人用。要加电脑就重新生成一个（看板上按 a，或运行 dct invite）".to_string(),
+        )
+    }
+
+    pub fn mesh_invite_replaced(lang: Lang) -> String {
+        t!(
+            lang,
+            en: "This invite code has been replaced by a newer one (was a pressed on the board?). Use the new one".to_string(),
+            zh: "这个邀请码已经换成新的了（看板上又按了 a？），以新的为准".to_string(),
+        )
+    }
+
+    pub fn mesh_invite_gone(lang: Lang) -> String {
+        t!(
+            lang,
+            en: "The invite code has been withdrawn".to_string(),
+            zh: "邀请码已经收回了".to_string(),
+        )
+    }
+
+    pub fn mesh_invite_usage(lang: Lang) -> String {
+        t!(
+            lang,
+            en: "Usage: dct invite (shows a 6-digit code; on the new computer run dct join <code>)".to_string(),
+            zh: "用法：dct invite（显示一个 6 位邀请码；在新电脑上运行 dct join <码>）".to_string(),
+        )
+    }
+
     /// 新电脑上用邀请码进组了：组里有谁。
     pub fn mesh_joined_with(lang: Lang, names: &[String]) -> String {
         t!(
@@ -3341,6 +3409,43 @@ mod tests {
         assert_eq!(msg::error(Lang::En, &Internal("原文".into())), "原文");
     }
 
+    /// 邀请码那几句：中文照设计文档一字不差，英文里没有汉字。
+    #[test]
+    fn invite_strings_say_what_the_design_says() {
+        assert_eq!(
+            msg::mesh_invite_code(Lang::Zh, "482 913"),
+            "邀请码 482 913 · 10 分钟内有效"
+        );
+        assert_eq!(
+            msg::mesh_joined_with(Lang::Zh, &["Mac".into(), "公司电脑".into()]),
+            "已加入「我的电脑」，组里有：Mac、公司电脑"
+        );
+        assert_eq!(msg::mesh_invite_countdown(Lang::Zh, 601), "还剩 10:01");
+        assert_eq!(msg::mesh_invite_countdown(Lang::Zh, 9), "还剩 0:09");
+        assert_eq!(
+            msg::error(
+                Lang::Zh,
+                &crate::proto::ErrorCode::Mesh(crate::proto::MeshProblem::WrongInviteCode)
+            ),
+            "邀请码不对或已作废，请在老电脑上按 a 重新生成"
+        );
+        for s in [
+            msg::mesh_invite_code(Lang::En, "482 913"),
+            msg::mesh_invite_hint(Lang::En, "482913"),
+            msg::mesh_invite_countdown(Lang::En, 61),
+            msg::mesh_invite_burned(Lang::En),
+            msg::mesh_invite_expired(Lang::En),
+            msg::mesh_invite_replaced(Lang::En),
+            msg::mesh_invite_gone(Lang::En),
+            msg::mesh_invite_usage(Lang::En),
+            msg::mesh_joined_with(Lang::En, &["A".into()]),
+            msg::mesh_renamed_on_join(Lang::En, "A 2"),
+            msg::mesh_join_usage(Lang::En),
+        ] {
+            assert!(!has_han(&s), "{s}");
+        }
+    }
+
     /// 多电脑那几句给人看的话：原文照 brief 一字不差，英文里没有汉字。
     #[test]
     fn mesh_strings_say_exactly_what_the_brief_says() {
diff --git a/src/main.rs b/src/main.rs
index 21b3f68..8281599 100644
--- a/src/main.rs
+++ b/src/main.rs
@@ -26,8 +26,9 @@ dct —— vibe coding 终端
                    后台空着就直接起一个新的
   dct llm check    把配置里那条 LLM 连接真的跑一次，看通不通
   dct login        多电脑：用 DC 账号登录；第一台电脑顺手建「我的电脑」组
-  dct join [--name 电脑名]
-                   在新电脑上请求加入，去已有的电脑上核对 6 位数、点同意
+  dct invite       老电脑上：出一个 6 位邀请码（10 分钟内有效，只能用一次）
+  dct join <邀请码> [--name 电脑名]
+                   新电脑上：用老电脑给的码加入（没登录会先自动登录）
   dct peers        看组里有哪些电脑、开着哪些会话、谁在等批准
   dct send <电脑名>/<会话名> \"<内容>\"
                    给另一台电脑上的会话留一句话；会话名也可以写 #编号。
@@ -119,7 +120,7 @@ fn main() -> Result<()> {
             std::process::exit(dct::cli::llm_check(cli_lang()))
         }
         // 多电脑。真正的事都在守护进程里做，这里只把话说给人听。
-        Some("login") | Some("join") | Some("peers") | Some("send") => {
+        Some("login") | Some("invite") | Some("join") | Some("peers") | Some("send") => {
             std::process::exit(dct::mesh::cli::run(&args))
         }
         Some("--help") | Some("-h") => {
diff --git a/src/mesh/cli.rs b/src/mesh/cli.rs
index 244f7d8..34c105f 100644
--- a/src/mesh/cli.rs
+++ b/src/mesh/cli.rs
@@ -1,11 +1,11 @@
-//! `dct login` / `dct join` / `dct peers` / `dct send`。
+//! `dct login` / `dct invite` / `dct join` / `dct peers` / `dct send`。
 //!
 //! 这里只是把话说给人听：真正的事（换令牌、问别的电脑、签名单）都在守护
 //! 进程里做——它握着中转连接、钥匙和密钥仓，命令行这边一样也不碰。
 //!
 //! 跟守护进程说话的那一下（`call`）和问用户的那一下（`ask`）都是参数，
 //! 于是整套对话不起守护进程、不碰终端也能测。
-use std::io::{BufRead, Write};
+use std::io::{BufRead, IsTerminal, Write};
 use std::path::Path;
 use std::time::Duration;
 
@@ -21,6 +21,9 @@ use crate::proto::{
 /// 必须比守护进程那边最坏的情况（`mesh::worst_case`）长出一截，见 `wait_for`。
 const MESH_CALL_WAIT: Duration = Duration::from_secs(90);
 
+/// `dct invite` 等结果时多久问一次守护进程（倒计时也按这个跳）。
+const INVITE_POLL: Duration = Duration::from_secs(1);
+
 /// 这一条请求命令行等多久：多电脑的请求等 `MESH_CALL_WAIT`，别的（握手）
 /// 按本机答一句的 `READ_TIMEOUT`。
 ///
@@ -74,6 +77,13 @@ pub fn run(args: &[String]) -> i32 {
             }
         }
     };
+    // `dct invite` 被 Ctrl-C：把码收回，不留一个没人看着的码挂 10 分钟。
+    // **必须装在连上守护进程之后**：Unix 上这一下会屏蔽 SIGINT/SIGTERM，
+    // 屏蔽掩码会被子进程继承——要是在 `connect_or_start` 拉起守护进程之前
+    // 装，那个守护进程就再也收不到 `dct stop` 的 SIGTERM。
+    if cmd == "invite" {
+        crate::sys::signal::restore_terminal_when_killed(cancel_invite_on_exit);
+    }
     let mut client = client;
     let mut call = |req: Request| {
         let wait = wait_for(&req);
@@ -106,6 +116,19 @@ fn dispatch(
     }
     match cmd {
         "login" => login(call, out, err, lang),
+        "invite" if rest.is_empty() => invite(
+            call,
+            out,
+            err,
+            lang,
+            std::io::stdout().is_terminal(),
+            &unix_now,
+            &|| std::thread::sleep(INVITE_POLL),
+        ),
+        "invite" => {
+            let _ = writeln!(err, "{}", msg::mesh_invite_usage(lang));
+            2
+        }
         "join" => match parse_join(rest) {
             Some(opts) => join(call, out, err, lang, &opts),
             None => {
@@ -245,6 +268,107 @@ pub(crate) fn login(call: Call, out: &mut dyn Write, err: &mut dyn Write, lang:
     0
 }
 
+fn unix_now() -> u64 {
+    std::time::SystemTime::now()
+        .duration_since(std::time::UNIX_EPOCH)
+        .map(|d| d.as_secs())
+        .unwrap_or(0)
+}
+
+/// Ctrl-C 打断 `dct invite` 时跑（Unix 上在 `sigwait` 那条普通线程里，
+/// Windows 上在控制台处理线程里——都能放心开 socket）。另开一条连接，
+/// 跑完进程就退出。
+fn cancel_invite_on_exit() {
+    let sock = crate::proto::socket_path();
+    if let Ok(mut c) = Client::connect(&sock) {
+        let _ = c.call_within(Request::MeshInviteCancel, crate::client::READ_TIMEOUT);
+    }
+}
+
+/// 老电脑上 `dct invite`：出一个码，印出来，一直等到它有结果（有人用它进了
+/// 组、被试错作废、过期、被看板上新按的 `a` 换掉）。`tty` 为真时每秒刷一行
+/// 倒计时。进组退 0，别的退 1。
+#[allow(clippy::too_many_arguments)]
+pub(crate) fn invite(
+    call: Call,
+    out: &mut dyn Write,
+    err: &mut dyn Write,
+    lang: Lang,
+    tty: bool,
+    now: &dyn Fn() -> u64,
+    pause: &dyn Fn(),
+) -> i32 {
+    let v = match call(Request::MeshInvite) {
+        Ok(Response::MeshInvite(v)) => v,
+        Ok(Response::Error(e)) => {
+            say_error(err, lang, &e);
+            return 1;
+        }
+        Ok(other) => {
+            say_error(err, lang, &ErrorCode::Internal(format!("{other:?}")));
+            return 1;
+        }
+        Err(e) => {
+            say_error(err, lang, &ErrorCode::Internal(e.to_string()));
+            return 1;
+        }
+    };
+    let spaced = format!("{} {}", &v.code[..3], &v.code[3..]);
+    let _ = writeln!(out, "{}", msg::mesh_invite_code(lang, &spaced));
+    let _ = writeln!(out, "{}", msg::mesh_invite_hint(lang, &v.code));
+    let _ = out.flush();
+    let mut ticked = false;
+    // 倒计时那一行后面接的话要另起一行。
+    let end = |out: &mut dyn Write, ticked: bool| {
+        if ticked {
+            let _ = writeln!(out);
+        }
+    };
+    loop {
+        pause();
+        let Some(s) = view_of(call, Request::MeshStatus, err, lang) else {
+            end(out, ticked);
+            return 1;
+        };
+        if let Some(n) = s.invite_note.as_ref().filter(|n| n.id == v.id) {
+            end(out, ticked);
+            return match &n.outcome {
+                crate::proto::InviteOutcome::Joined { name } => {
+                    let _ = writeln!(out, "{}", msg::mesh_member_joined(lang, &c(name)));
+                    0
+                }
+                crate::proto::InviteOutcome::Burned => {
+                    let _ = writeln!(err, "{}", msg::mesh_invite_burned(lang));
+                    1
+                }
+                crate::proto::InviteOutcome::Expired => {
+                    let _ = writeln!(err, "{}", msg::mesh_invite_expired(lang));
+                    1
+                }
+            };
+        }
+        match &s.invite {
+            Some(cur) if cur.id == v.id => {}
+            Some(_) => {
+                end(out, ticked);
+                let _ = writeln!(err, "{}", msg::mesh_invite_replaced(lang));
+                return 1;
+            }
+            None => {
+                end(out, ticked);
+                let _ = writeln!(err, "{}", msg::mesh_invite_gone(lang));
+                return 1;
+            }
+        }
+        if tty {
+            let left = v.expires_at.saturating_sub(now());
+            let _ = write!(out, "\r{}", msg::mesh_invite_countdown(lang, left));
+            let _ = out.flush();
+            ticked = true;
+        }
+    }
+}
+
 /// 新电脑上：拿老电脑给的码加入。登录（没登录的话）、找发邀请的那台、
 /// 核对，全在守护进程里一次办完；这里只把结果说给人听。
 pub(crate) fn join(
@@ -573,6 +697,7 @@ mod tests {
     fn every_mesh_command_stops_at_a_stale_daemon_with_the_restart_hint() {
         let cases: &[&[&str]] = &[
             &["login"],
+            &["invite"],
             &["join"],
             &["peers"],
             &["peers", "approve", "B"],
@@ -1208,4 +1333,136 @@ mod tests {
         assert_eq!(code, 2);
         assert!(s(&err).contains("dct join <邀请码>"), "{}", s(&err));
     }
+
+    fn invite_view(id: u64, code: &str) -> crate::proto::InviteView {
+        crate::proto::InviteView {
+            id,
+            code: code.into(),
+            expires_at: 1_000 + 600,
+        }
+    }
+
+    /// 一份带着邀请码（或者结果）的现状。
+    fn with_invite(
+        invite: Option<crate::proto::InviteView>,
+        note: Option<(u64, crate::proto::InviteOutcome)>,
+    ) -> MeshView {
+        let mut v = view(true, &[("Mac", true)]);
+        v.invite = invite;
+        v.invite_note = note.map(|(id, outcome)| crate::proto::InviteNote { id, outcome });
+        v
+    }
+
+    fn run_invite(answers: Vec<Response>, tty: bool) -> (i32, String, String, Vec<String>) {
+        let sc = Script::new(answers);
+        let (mut out, mut err) = (vec![], vec![]);
+        let code = invite(&mut sc.call(), &mut out, &mut err, Lang::Zh, tty, &|| 1_001, &|| {});
+        let seen = sc.seen.borrow().clone();
+        (code, s(&out), s(&err), seen)
+    }
+
+    /// 印出码（带空格）和怎么用，等到有人用它进组：报名字、退 0。
+    #[test]
+    fn invite_prints_the_code_and_waits_until_someone_joins() {
+        use crate::proto::InviteOutcome::Joined;
+        let (code, out, err, seen) = run_invite(
+            vec![
+                Response::MeshInvite(invite_view(3, "012345")),
+                Response::Mesh(with_invite(Some(invite_view(3, "012345")), None)),
+                Response::Mesh(with_invite(
+                    None,
+                    Some((
+                        3,
+                        Joined {
+                            name: "公司电脑".into(),
+                        },
+                    )),
+                )),
+            ],
+            false,
+        );
+        assert_eq!(code, 0, "{err}");
+        assert_eq!(
+            out,
+            "邀请码 012 345 · 10 分钟内有效\n在新电脑上运行：dct join 012345（码只能用一次）\n公司电脑 已加入\n"
+        );
+        assert_eq!(seen, ["MeshInvite", "MeshStatus", "MeshStatus"]);
+    }
+
+    #[test]
+    fn invite_says_when_the_code_was_burned_expired_replaced_or_withdrawn() {
+        use crate::proto::InviteOutcome::{Burned, Expired};
+        let cases: Vec<(MeshView, &str)> = vec![
+            (
+                with_invite(None, Some((3, Burned))),
+                "有人用错码试过一次，码已作废。要加电脑就重新生成一个（看板上按 a，或运行 dct invite）",
+            ),
+            (
+                with_invite(None, Some((3, Expired))),
+                "邀请码过期了，没人用。要加电脑就重新生成一个（看板上按 a，或运行 dct invite）",
+            ),
+            (
+                with_invite(Some(invite_view(4, "999999")), None),
+                "这个邀请码已经换成新的了（看板上又按了 a？），以新的为准",
+            ),
+            (with_invite(None, None), "邀请码已经收回了"),
+            // 上一个码的结果不算这一个的。
+            (
+                with_invite(None, Some((2, Burned))),
+                "邀请码已经收回了",
+            ),
+        ];
+        for (status, want) in cases {
+            let (code, _, err, _) = run_invite(
+                vec![
+                    Response::MeshInvite(invite_view(3, "482913")),
+                    Response::Mesh(status),
+                ],
+                false,
+            );
+            assert_eq!(code, 1);
+            assert_eq!(err.trim(), want);
+        }
+    }
+
+    /// 终端里每秒刷一行倒计时（`\r` 覆盖）；结果另起一行。管道里不刷。
+    #[test]
+    fn invite_counts_down_only_on_a_terminal() {
+        use crate::proto::InviteOutcome::Joined;
+        let answers = || {
+            vec![
+                Response::MeshInvite(invite_view(3, "482913")),
+                Response::Mesh(with_invite(Some(invite_view(3, "482913")), None)),
+                Response::Mesh(with_invite(None, Some((3, Joined { name: "B".into() })))),
+            ]
+        };
+        let (_, out, _, _) = run_invite(answers(), true);
+        assert!(out.contains("\r还剩 9:59"), "{out:?}");
+        assert!(out.ends_with("\nB 已加入\n"), "{out:?}");
+        let (_, out, _, _) = run_invite(answers(), false);
+        assert!(!out.contains('\r'), "{out:?}");
+    }
+
+    #[test]
+    fn invite_before_login_says_to_log_in() {
+        let (code, out, err, _) = run_invite(
+            vec![Response::Error(ErrorCode::Mesh(MeshProblem::NotLoggedIn))],
+            false,
+        );
+        assert_eq!(code, 1);
+        assert!(out.is_empty());
+        assert_eq!(err.trim(), "还没登录多电脑，先运行 dct login");
+    }
+
+    #[test]
+    fn invite_takes_no_arguments() {
+        let sc = Script::new(vec![Response::Hello {
+            protocol: crate::proto::PROTOCOL_VERSION,
+        }]);
+        let (mut out, mut err) = (vec![], vec![]);
+        let rest = vec!["482913".to_string()];
+        let code = dispatch("invite", &rest, &mut sc.call(), &mut no_ask, &mut out, &mut err, Lang::Zh);
+        assert_eq!(code, 2);
+        assert!(s(&err).contains("用法：dct invite"), "{}", s(&err));
+    }
 }
```

- [ ] **Step 4: 跑测试确认通过**

Run: `~/.cargo/bin/cargo test --lib -- mesh::cli i18n`
Expected: 全过。

- [ ] **Step 5: 手工看一眼**（单元测试碰不到信号和真终端。用 worktree 里 `~/.cargo/bin/cargo build` 出的 `target/debug/dct`，**换一个临时 `HOME`**——socket、密钥、名单全跟着 `HOME` 走，碰不到用户正在用的守护进程）

```bash
~/.cargo/bin/cargo build
export HOME=$(mktemp -d)
./target/debug/dct invite; echo "exit=$?"
./target/debug/dct join 482 913; echo "exit=$?"
pkill -f "$(pwd)/target/debug/dct"   # 只杀这个 worktree 编出来的 dct，用户装的那份不动
```

Expected（文案跟 `LANG`/设置走；写计划时在英文环境实测）：第一条印「还没登录多电脑，先运行 dct login」（英文 `Multi-computer is not signed in yet. Run dct login first`）、`exit=1`；第二条印「先在 dct 里用 DC 配对账号（进 dct 按 c 选 DC）」（英文 `First pair your DC account in dct (open dct, press c, choose DC)`）、`exit=1`（`join` 先自动登录，没配对 DC 就停在这）；都没有栈追踪。有真账号的话再验 Ctrl-C：`dct invite` 按 Ctrl-C 以 130 退出，随后新电脑上 `dct join <那个码>` 得到「没有在线的电脑发出邀请…」；没有真账号就在报告里写「Ctrl-C 收回只验了代码路径，没实机验」。

- [ ] **Step 6: 全量检查**

```bash
~/.cargo/bin/cargo test --workspace -- --test-threads=4
~/.cargo/bin/cargo clippy --workspace --all-targets -- -D warnings
~/.cargo/bin/cargo check --workspace --all-targets --target x86_64-pc-windows-msvc
```
Expected：`test result: ok.` 全绿（`pty::tests::*` / `session::tests::*` 偶发失败见 Global Constraints 的「已知 flaky」，单独重跑）；clippy `Finished`、无 warning；Windows check `Finished`（`src/student_projects.rs:669` 那条 `unused variable: path` 是本来就有的）。

- [ ] **Step 7: Commit**

```bash
git add src/mesh/cli.rs src/main.rs src/i18n.rs
git commit -m "feat(mesh): dct invite prints a one-time code, waits for the outcome, withdraws it on Ctrl-C" -m "Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---
