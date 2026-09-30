### Task 5: 守护进程和命令行接上——`MeshInvite`、`MeshJoin{code,name}`、协议 24、端到端

**Files:**
- Modify: `src/proto.rs`（`Request::MeshInvite`、`MeshInviteCancel`；`MeshJoin { code, name }`，`Debug` 打星号；`Response::MeshInvite(InviteView)`；`MeshView` 加 `invite`/`invite_note`；`PROTOCOL_VERSION` 23 → 24；所有 `(PROTOCOL_VERSION, …)` 守卫里的 23 改 24，共 8 处，含两条单行的）
- Modify: `src/daemon.rs`（`handle_mesh` 接三条；`MeshJoin` 没登录先 `mesh_login`；本机 socket 分派表和 HTTP 拒绝表各加两条；投递线程里 `invite::flush_outbox`；`mesh_view` 空槽时两个新字段为 `None`）
- Modify: `src/mesh/group.rs`（`view` 先 `invite_view()` 再读 `invite_note`）
- Modify: `src/mesh/mod.rs`（`worst_case`：`MeshInvite`/`MeshInviteCancel` 是一次 `call`，`MeshJoin` 按裁决 9 算）
- Modify: `src/mesh/cli.rs`（`dct join <码> [--name]` 整个换掉：不再列数字、不再问是哪台、不再等同意；`MESH_CALL_WAIT` 60 → 90 秒；删掉旧 join 的 10 条测试和 `opts` 帮手）
- Modify: `src/i18n.rs`（`mesh_join_usage` 改写，加 `mesh_joined_with`、`mesh_renamed_on_join`）
- Modify: `src/ui/computers.rs`（测试里的 `mesh_view` 帮手补两个新字段）
- Modify: `tests/mesh_e2e.rs`（A login → A `MeshInvite` → B `MeshJoin{码带空格}`（自动登录）→ peers → send）

**Interfaces:**
- Consumes: Task 3 的 `Mesh::start_invite`/`cancel_invite`/`invite_view`、`invite_note`、`flush_outbox`、`INVITE_ASK_TIMEOUT`、`MAX_INVITERS_TRIED`；Task 4 的 `invite::join`、新 `MeshProblem`。
- Produces：
  - `Request::MeshInvite` → `Response::MeshInvite(InviteView)`；`Request::MeshInviteCancel` → `Response::Mesh(MeshView)`；`Request::MeshJoin { code: String, name: String }` → `Response::Mesh(MeshView)`
  - `MeshView { …, invite: Option<InviteView>, invite_note: Option<InviteNote> }`（`pending`/`joining` 这一步还在，Task 8 删）
  - `proto::PROTOCOL_VERSION = 24`
  - `mesh::cli::JoinOpts { code: String, name: String }`、`cli::join(call, out, err, lang, opts: &JoinOpts) -> i32`、`msg::mesh_joined_with(lang, names: &[String]) -> String`、`msg::mesh_renamed_on_join(lang, name: &str) -> String`
- `dct peers approve`、`MeshApprove`、`MeshConfirmInviter` **这一步还留着**（Task 8 删）；旧的 `group::join` 已经没人调，是 `pub fn`，不报 dead code。

- [ ] **Step 1: 写失败的测试**

`src/proto.rs` 测试模块里（`a_mesh_send_request_does_not_print_its_text` 之前）：

```rust
    /// 邀请码不进 `Debug`：`MeshJoin` 只报有没有码，`InviteView` 打星号。
    #[test]
    fn invite_codes_never_show_up_in_debug() {
        let r = Request::MeshJoin {
            code: "482913".into(),
            name: "公司电脑".into(),
        };
        let d = format!("{r:?}");
        assert!(!d.contains("482913") && d.contains("公司电脑"), "{d}");
        let v = Response::MeshInvite(InviteView {
            id: 1,
            code: "482913".into(),
            expires_at: 9,
        });
        let d = format!("{v:?}");
        assert!(!d.contains("482913") && d.contains("expires_at: 9"), "{d}");
        assert_eq!(
            serde_json::to_string(&v).unwrap(),
            r#"{"MeshInvite":{"id":1,"code":"482913","expires_at":9}}"#,
            "线上照常带着码"
        );
    }
```

`src/daemon.rs` 测试模块里（`a_restarted_daemon_keeps_its_endpoint` 之前）：

```rust
    /// `dct join <码>` 在还没登录的电脑上：先登录（要 DC 账号），再找发邀请
    /// 的电脑。没配对 DC 就停在登录那一步，什么都不生成。
    #[test]
    fn mesh_join_logs_in_first_and_needs_a_dc_account() {
        let (t, socket, secrets) = home();
        std::fs::write(
            crate::config::config_path_for_socket(&socket),
            "[mesh]\nrelay = \"http://127.0.0.1:9\"\n",
        )
        .unwrap();
        let ctl = ctl_for(&socket);
        let join = || Request::MeshJoin {
            code: "482913".into(),
            name: String::new(),
        };
        let r = mesh_call(join(), &ctl, &secrets, &t.path().join("profiles"), &no_network);
        assert!(
            matches!(
                r,
                Response::Error(ErrorCode::Mesh(crate::proto::MeshProblem::NoDcAccount))
            ),
            "{r:?}"
        );
        assert!(recover(ctl.slot.lock()).is_none());
        assert!(!crate::mesh::store::dir_for_socket(&socket)
            .join("sign.key")
            .exists());

        secrets.lock().unwrap().set("dc", "sk-dc").unwrap();
        let fake = |_: &str, _: &str, _: &str| {
            Ok((200, format!(r#"{{"token":"tok","exp":{}}}"#, u64::MAX / 2)))
        };
        let r = mesh_call(join(), &ctl, &secrets, &t.path().join("profiles"), &fake);
        // 登录成了；中转连不上（127.0.0.1:9），问不到任何电脑。
        assert!(
            matches!(
                r,
                Response::Error(ErrorCode::Mesh(crate::proto::MeshProblem::NoInvite {
                    unanswered: 0
                }))
            ),
            "{r:?}"
        );
        assert_eq!(secrets.lock().unwrap().get(RELAY_TOKEN_KEY), Some("tok"));
        let slot = recover(ctl.slot.lock());
        slot.as_ref().expect("join 顺手登录了").link.stop();
    }

    /// 登录之后才能出码；出了码 `MeshStatus` 里看得到，收回之后就没了。
    #[test]
    fn mesh_invite_hands_out_a_code_that_status_shows_until_cancelled() {
        let (t, socket, secrets) = home();
        std::fs::write(
            crate::config::config_path_for_socket(&socket),
            "[mesh]\nrelay = \"http://127.0.0.1:9\"\n",
        )
        .unwrap();
        let ctl = ctl_for(&socket);
        let profiles = t.path().join("profiles");
        secrets.lock().unwrap().set("dc", "sk-dc").unwrap();
        let fake = |_: &str, _: &str, _: &str| {
            Ok((200, format!(r#"{{"token":"tok","exp":{}}}"#, u64::MAX / 2)))
        };
        assert!(matches!(
            mesh_call(Request::MeshLogin, &ctl, &secrets, &profiles, &fake),
            Response::Mesh(_)
        ));
        let Response::MeshInvite(v) =
            mesh_call(Request::MeshInvite, &ctl, &secrets, &profiles, &no_network)
        else {
            panic!("该回 MeshInvite")
        };
        assert_eq!(v.code.len(), 6);
        let Response::Mesh(status) =
            mesh_call(Request::MeshStatus, &ctl, &secrets, &profiles, &no_network)
        else {
            panic!()
        };
        assert_eq!(status.invite, Some(v));
        let Response::Mesh(after) =
            mesh_call(Request::MeshInviteCancel, &ctl, &secrets, &profiles, &no_network)
        else {
            panic!()
        };
        assert_eq!(after.invite, None);
        recover(ctl.slot.lock()).as_ref().unwrap().link.stop();
    }
```

`src/mesh/cli.rs` 测试模块末尾（同时删掉旧的 `join_arguments`）：

```rust
    #[test]
    fn join_arguments() {
        let a = |v: &[&str]| parse_join(&v.iter().map(|s| s.to_string()).collect::<Vec<_>>());
        let j = |code: &str, name: &str| {
            Some(JoinOpts {
                code: code.into(),
                name: name.into(),
            })
        };
        assert_eq!(a(&["482913"]), j("482913", ""));
        assert_eq!(a(&["482", "913"]), j("482913", ""), "shell 把空格拆开了");
        assert_eq!(a(&["482-913"]), j("482-913", ""));
        assert_eq!(a(&["012345"]), j("012345", ""), "前导 0 原样");
        assert_eq!(a(&["482913", "--name", "公司电脑"]), j("482913", "公司电脑"));
        assert_eq!(a(&["--name=家里", "482913"]), j("482913", "家里"));
        assert_eq!(a(&[]), None, "没有码");
        assert_eq!(a(&["--name", "N"]), None, "只有名字没有码");
        assert_eq!(a(&["482913", "--confirm", "A"]), None, "--confirm 没有了");
        assert_eq!(a(&["482913", "--name"]), None);
    }

    /// 送给守护进程的就是用户敲的码和名字；进组之后报组里有谁。
    #[test]
    fn join_sends_the_code_and_says_who_is_in_the_group() {
        let mut v = view(true, &[("Mac", false), ("公司电脑", true)]);
        v.name = "公司电脑".into();
        let sc = Script::new(vec![Response::Mesh(v)]);
        let (mut out, mut err) = (vec![], vec![]);
        let o = JoinOpts {
            code: "482 913".into(),
            name: "公司电脑".into(),
        };
        assert_eq!(join(&mut sc.call(), &mut out, &mut err, Lang::Zh, &o), 0);
        assert_eq!(
            sc.seen.borrow().as_slice(),
            [r#"MeshJoin { code: "******", name: "公司电脑" }"#]
        );
        assert_eq!(s(&out).trim(), "已加入「我的电脑」，组里有：Mac、公司电脑");
    }

    #[test]
    fn join_tells_the_new_name_when_the_inviter_numbered_it() {
        let mut v = view(true, &[("Mac", false), ("Mac 2", true)]);
        v.name = "Mac 2".into();
        let sc = Script::new(vec![Response::Mesh(v)]);
        let (mut out, mut err) = (vec![], vec![]);
        let o = JoinOpts {
            code: "482913".into(),
            name: "Mac".into(),
        };
        assert_eq!(join(&mut sc.call(), &mut out, &mut err, Lang::Zh, &o), 0);
        assert!(s(&out).contains("组里已经有一台叫这个名字的了，这台电脑现在叫 Mac 2"), "{}", s(&out));
    }

    #[test]
    fn join_errors_are_said_in_words_and_exit_1() {
        for (p, want) in [
            (MeshProblem::WrongInviteCode, "邀请码不对或已作废，请在老电脑上按 a 重新生成"),
            (MeshProblem::BadInviteCode, "邀请码是 6 位数字，比如：dct join 482913"),
            (MeshProblem::NoDcAccount, "先在 dct 里用 DC 配对账号（进 dct 按 c 选 DC）"),
        ] {
            let sc = Script::new(vec![Response::Error(ErrorCode::Mesh(p))]);
            let (mut out, mut err) = (vec![], vec![]);
            let o = JoinOpts {
                code: "1".into(),
                name: String::new(),
            };
            assert_eq!(join(&mut sc.call(), &mut out, &mut err, Lang::Zh, &o), 1);
            assert_eq!(s(&err).trim(), want);
            assert!(out.is_empty());
        }
    }

    #[test]
    fn join_without_a_code_prints_the_usage() {
        let sc = Script::new(vec![Response::Hello {
            protocol: crate::proto::PROTOCOL_VERSION,
        }]);
        let (mut out, mut err) = (vec![], vec![]);
        let code = dispatch("join", &[], &mut sc.call(), &mut no_ask, &mut out, &mut err, Lang::Zh);
        assert_eq!(code, 2);
        assert!(s(&err).contains("dct join <邀请码>"), "{}", s(&err));
    }
```

- [ ] **Step 2: 跑测试确认失败**

Run: `~/.cargo/bin/cargo test --lib -- proto:: daemon::tests::mesh_ mesh::cli`
Expected: 编译失败：`no variant named `MeshInvite``、`struct `JoinOpts` has no field named `code``。

- [ ] **Step 3: 实现**

先把 Step 1 里手改过的文件还原（补丁里已经包含同样的测试）：`git checkout -- src/proto.rs src/daemon.rs src/mesh/cli.rs`。

把下面整个补丁存成 `/tmp/dct-invite-t5.patch`（**只存 ```diff 块里的内容**），在 worktree 根目录应用：

```bash
git apply --check /tmp/dct-invite-t5.patch && git apply /tmp/dct-invite-t5.patch
```

`--check` 报错说明前面的任务跟本计划的代码有出入：改用 `git apply --3way`，还不行就照补丁逐块手改（上下文行用来定位），**不要**跳过任何一块。

```diff
diff --git a/src/daemon.rs b/src/daemon.rs
index 3623f79..fe11386 100644
--- a/src/daemon.rs
+++ b/src/daemon.rs
@@ -225,9 +225,11 @@ pub fn run_with_manager(socket: &Path, mgr: Arc<SessionManager>) -> Result<()> {
         let mc = mesh_ctl.clone();
         std::thread::spawn(move || loop {
             std::thread::sleep(crate::mesh::deliver::TICK);
-            if let Some((m, _)) = mc.running() {
+            if let Some((m, n)) = mc.running() {
                 // 一拍里敲字 panic 了，这条线不能跟着死（`tick_catching`）。
                 crate::mesh::deliver::tick_catching(&m);
+                // 有人用邀请码进了组：新名单发给组里其余的电脑（`invite::flush_outbox`）。
+                crate::mesh::invite::flush_outbox(&m, n.as_ref());
             }
         });
     }
@@ -332,10 +334,23 @@ fn handle_mesh(
     let r: Result<Response, MeshProblem> = match req {
         Request::MeshStatus => Ok(view(ctl)),
         Request::MeshLogin => mesh_login(ctl, secrets, profiles_dir, transport).map(|_| view(ctl)),
-        Request::MeshJoin { name } => ctl
+        // 还没登录就先登录（要已配对 DC 账号，同 `dct login`），再用码加入。
+        Request::MeshJoin { code, name } => (match ctl.running() {
+            Some(parts) => Ok(parts),
+            None => mesh_login(ctl, secrets, profiles_dir, transport)
+                .and_then(|_| ctl.running().ok_or(MeshProblem::NotLoggedIn)),
+        })
+        .and_then(|(m, n)| crate::mesh::invite::join(&m, n.as_ref(), &code, Some(&name)))
+        .map(|_| view(ctl)),
+        Request::MeshInvite => ctl
             .running()
             .ok_or(MeshProblem::NotLoggedIn)
-            .and_then(|(m, n)| group::join(&m, n.as_ref(), Some(&name)))
+            .and_then(|(m, _)| recover(m.lock()).start_invite())
+            .map(Response::MeshInvite),
+        Request::MeshInviteCancel => ctl
+            .running()
+            .ok_or(MeshProblem::NotLoggedIn)
+            .map(|(m, _)| recover(m.lock()).cancel_invite())
             .map(|_| view(ctl)),
         Request::MeshApprove {
             endpoint,
@@ -392,6 +407,8 @@ fn mesh_view(ctl: &MeshCtl) -> crate::proto::MeshView {
         pending: Vec::new(),
         joining: Vec::new(),
         messages: Default::default(),
+        invite: None,
+        invite_note: None,
     }
 }
 
@@ -871,6 +888,8 @@ fn serve(
                 req @ (Request::MeshStatus
                 | Request::MeshLogin
                 | Request::MeshJoin { .. }
+                | Request::MeshInvite
+                | Request::MeshInviteCancel
                 | Request::MeshApprove { .. }
                 | Request::MeshConfirmInviter { .. }
                 | Request::MeshRemove { .. }
@@ -1240,6 +1259,8 @@ fn handle(
         Request::MeshStatus
         | Request::MeshLogin
         | Request::MeshJoin { .. }
+        | Request::MeshInvite
+        | Request::MeshInviteCancel
         | Request::MeshApprove { .. }
         | Request::MeshConfirmInviter { .. }
         | Request::MeshRemove { .. }
@@ -3598,7 +3619,12 @@ mod tests {
         for req in [
             Request::MeshStatus,
             Request::MeshLogin,
-            Request::MeshJoin { name: String::new() },
+            Request::MeshJoin {
+                code: "482913".into(),
+                name: String::new(),
+            },
+            Request::MeshInvite,
+            Request::MeshInviteCancel,
             Request::MeshApprove {
                 endpoint: "c-x".into(),
                 code: "123456".into(),
@@ -4079,7 +4105,8 @@ mod mesh_tests {
             .join("sign.key")
             .exists());
         for req in [
-            Request::MeshJoin { name: String::new() },
+            Request::MeshInvite,
+            Request::MeshInviteCancel,
             Request::MeshApprove {
                 endpoint: "x".into(),
                 code: "1".into(),
@@ -4213,6 +4240,94 @@ mod mesh_tests {
         assert_eq!(secrets.lock().unwrap().get(RELAY_TOKEN_KEY), None);
     }
 
+    /// `dct join <码>` 在还没登录的电脑上：先登录（要 DC 账号），再找发邀请
+    /// 的电脑。没配对 DC 就停在登录那一步，什么都不生成。
+    #[test]
+    fn mesh_join_logs_in_first_and_needs_a_dc_account() {
+        let (t, socket, secrets) = home();
+        std::fs::write(
+            crate::config::config_path_for_socket(&socket),
+            "[mesh]\nrelay = \"http://127.0.0.1:9\"\n",
+        )
+        .unwrap();
+        let ctl = ctl_for(&socket);
+        let join = || Request::MeshJoin {
+            code: "482913".into(),
+            name: String::new(),
+        };
+        let r = mesh_call(join(), &ctl, &secrets, &t.path().join("profiles"), &no_network);
+        assert!(
+            matches!(
+                r,
+                Response::Error(ErrorCode::Mesh(crate::proto::MeshProblem::NoDcAccount))
+            ),
+            "{r:?}"
+        );
+        assert!(recover(ctl.slot.lock()).is_none());
+        assert!(!crate::mesh::store::dir_for_socket(&socket)
+            .join("sign.key")
+            .exists());
+
+        secrets.lock().unwrap().set("dc", "sk-dc").unwrap();
+        let fake = |_: &str, _: &str, _: &str| {
+            Ok((200, format!(r#"{{"token":"tok","exp":{}}}"#, u64::MAX / 2)))
+        };
+        let r = mesh_call(join(), &ctl, &secrets, &t.path().join("profiles"), &fake);
+        // 登录成了；中转连不上（127.0.0.1:9），问不到任何电脑。
+        assert!(
+            matches!(
+                r,
+                Response::Error(ErrorCode::Mesh(crate::proto::MeshProblem::NoInvite {
+                    unanswered: 0
+                }))
+            ),
+            "{r:?}"
+        );
+        assert_eq!(secrets.lock().unwrap().get(RELAY_TOKEN_KEY), Some("tok"));
+        let slot = recover(ctl.slot.lock());
+        slot.as_ref().expect("join 顺手登录了").link.stop();
+    }
+
+    /// 登录之后才能出码；出了码 `MeshStatus` 里看得到，收回之后就没了。
+    #[test]
+    fn mesh_invite_hands_out_a_code_that_status_shows_until_cancelled() {
+        let (t, socket, secrets) = home();
+        std::fs::write(
+            crate::config::config_path_for_socket(&socket),
+            "[mesh]\nrelay = \"http://127.0.0.1:9\"\n",
+        )
+        .unwrap();
+        let ctl = ctl_for(&socket);
+        let profiles = t.path().join("profiles");
+        secrets.lock().unwrap().set("dc", "sk-dc").unwrap();
+        let fake = |_: &str, _: &str, _: &str| {
+            Ok((200, format!(r#"{{"token":"tok","exp":{}}}"#, u64::MAX / 2)))
+        };
+        assert!(matches!(
+            mesh_call(Request::MeshLogin, &ctl, &secrets, &profiles, &fake),
+            Response::Mesh(_)
+        ));
+        let Response::MeshInvite(v) =
+            mesh_call(Request::MeshInvite, &ctl, &secrets, &profiles, &no_network)
+        else {
+            panic!("该回 MeshInvite")
+        };
+        assert_eq!(v.code.len(), 6);
+        let Response::Mesh(status) =
+            mesh_call(Request::MeshStatus, &ctl, &secrets, &profiles, &no_network)
+        else {
+            panic!()
+        };
+        assert_eq!(status.invite, Some(v));
+        let Response::Mesh(after) =
+            mesh_call(Request::MeshInviteCancel, &ctl, &secrets, &profiles, &no_network)
+        else {
+            panic!()
+        };
+        assert_eq!(after.invite, None);
+        recover(ctl.slot.lock()).as_ref().unwrap().link.stop();
+    }
+
     /// 重启之后还是同一台电脑：端点由落盘的钥匙决定。
     #[test]
     fn a_restarted_daemon_keeps_its_endpoint() {
diff --git a/src/i18n.rs b/src/i18n.rs
index 9940c5d..d1bbe2a 100644
--- a/src/i18n.rs
+++ b/src/i18n.rs
@@ -2412,8 +2412,26 @@ pub mod msg {
     pub fn mesh_join_usage(lang: Lang) -> String {
         t!(
             lang,
-            en: "Usage: dct join [--name <computer name>] [--confirm <name of the computer whose number matches>]".to_string(),
-            zh: "用法：dct join [--name 电脑名] [--confirm 数字对得上的那台电脑的名字]".to_string(),
+            en: "Usage: dct join <invite code> [--name <computer name>]. Get the code on your other computer: press a on the board, or run dct invite".to_string(),
+            zh: "用法：dct join <邀请码> [--name 电脑名]。邀请码在老电脑上拿：看板上按 a，或者运行 dct invite".to_string(),
+        )
+    }
+
+    /// 新电脑上用邀请码进组了：组里有谁。
+    pub fn mesh_joined_with(lang: Lang, names: &[String]) -> String {
+        t!(
+            lang,
+            en: format!("Joined \"My computers\". In the group: {}", names.join(", ")),
+            zh: format!("已加入「我的电脑」，组里有：{}", names.join("、")),
+        )
+    }
+
+    /// `--name` 起的名字跟组里的撞了，老电脑编了号。
+    pub fn mesh_renamed_on_join(lang: Lang, name: &str) -> String {
+        t!(
+            lang,
+            en: format!("The group already had a computer with that name, so this one is now called {name}"),
+            zh: format!("组里已经有一台叫这个名字的了，这台电脑现在叫 {name}"),
         )
     }
 
diff --git a/src/mesh/cli.rs b/src/mesh/cli.rs
index c2531a8..244f7d8 100644
--- a/src/mesh/cli.rs
+++ b/src/mesh/cli.rs
@@ -7,7 +7,7 @@
 //! 于是整套对话不起守护进程、不碰终端也能测。
 use std::io::{BufRead, Write};
 use std::path::Path;
-use std::time::{Duration, Instant};
+use std::time::Duration;
 
 use anyhow::Result;
 
@@ -17,13 +17,9 @@ use crate::proto::{
     ErrorCode, MeshProblem, MeshView, PeerView, PendingJoin, Request, Response, SendOutcome,
 };
 
-/// `dct join` 最多等多久有人点同意。跟对面那条请求的有效期一样长。
-const JOIN_WAIT: Duration = crate::mesh::JOIN_TTL;
-/// 等同意时多久问一次守护进程。
-const JOIN_POLL: Duration = Duration::from_secs(1);
 /// 守护进程替我们打网络的那几条请求（换令牌、问别的电脑、广播名单）等多久。
 /// 必须比守护进程那边最坏的情况（`mesh::worst_case`）长出一截，见 `wait_for`。
-const MESH_CALL_WAIT: Duration = Duration::from_secs(60);
+const MESH_CALL_WAIT: Duration = Duration::from_secs(90);
 
 /// 这一条请求命令行等多久：多电脑的请求等 `MESH_CALL_WAIT`，别的（握手）
 /// 按本机答一句的 `READ_TIMEOUT`。
@@ -44,10 +40,10 @@ type Ask<'a> = &'a mut dyn FnMut(&str) -> Option<String>;
 /// `dct join` 的参数。
 #[derive(Debug, Default, PartialEq, Eq)]
 pub(crate) struct JoinOpts {
-    /// 顺手改名；空 = 不改。
+    /// 老电脑上显示的邀请码，照用户敲的原样（空格、`-` 由守护进程那边去掉）。
+    pub code: String,
+    /// 用这个名字进组；空 = 用现在的名字。
     pub name: String,
-    /// 不问了，直接认定这一台（脚本用）。
-    pub confirm: Option<String>,
 }
 
 pub fn run(args: &[String]) -> i32 {
@@ -111,9 +107,7 @@ fn dispatch(
     match cmd {
         "login" => login(call, out, err, lang),
         "join" => match parse_join(rest) {
-            Some(opts) => join(call, ask, out, err, lang, &opts, JOIN_WAIT, &|| {
-                std::thread::sleep(JOIN_POLL)
-            }),
+            Some(opts) => join(call, out, err, lang, &opts),
             None => {
                 let _ = writeln!(err, "{}", msg::mesh_join_usage(lang));
                 2
@@ -171,25 +165,24 @@ fn connect_or_start(sock: &Path) -> Option<Client> {
     None
 }
 
-/// `--name X` / `--name=X`、`--confirm X` / `--confirm=X`，都可以不给。
-/// `None` = 用法不对。
+/// `dct join <码> [--name X]`。码可以被空格拆成几段（`dct join 482 913`
+/// 到这里是两个参数），拼回去；`--name X` / `--name=X` 可以放在哪儿都行。
+/// `None` = 用法不对（没有码、不认识的 `--` 参数）。
 fn parse_join(args: &[String]) -> Option<JoinOpts> {
     let mut opts = JoinOpts::default();
     let mut it = args.iter();
     while let Some(a) = it.next() {
         if let Some(v) = a.strip_prefix("--name=") {
             opts.name = v.to_string();
-        } else if let Some(v) = a.strip_prefix("--confirm=") {
-            opts.confirm = Some(v.to_string());
         } else if a == "--name" {
             opts.name = it.next()?.clone();
-        } else if a == "--confirm" {
-            opts.confirm = Some(it.next()?.clone());
-        } else {
+        } else if a.starts_with("--") {
             return None;
+        } else {
+            opts.code.push_str(a);
         }
     }
-    Some(opts)
+    (!opts.code.is_empty()).then_some(opts)
 }
 
 /// 按名字或端点在一张列表里找一条。找不到、不止一条都是 `Err`。
@@ -252,125 +245,29 @@ pub(crate) fn login(call: Call, out: &mut dyn Write, err: &mut dyn Write, lang:
     0
 }
 
-/// 已经跟别的电脑同组了吗（名单上不止我自己）。
-fn grouped(v: &MeshView) -> bool {
-    v.members.iter().any(|m| !m.is_me)
-}
-
-/// 新电脑上：问一遍已有的电脑，列出每台的数字，**让用户说出是哪一台**
-/// （或者 `--confirm`），告诉守护进程只认那一台，然后等它的名单。
-///
-/// 不替用户挑：回话的电脑可能不止一台，其中哪台是中转塞进来的，只有看过
-/// 两块屏幕的人知道。只有一台回话也一样要他说一声——那一台同样可能是塞
-/// 进来的。
-#[allow(clippy::too_many_arguments)]
+/// 新电脑上：拿老电脑给的码加入。登录（没登录的话）、找发邀请的那台、
+/// 核对，全在守护进程里一次办完；这里只把结果说给人听。
 pub(crate) fn join(
     call: Call,
-    ask: Ask,
     out: &mut dyn Write,
     err: &mut dyn Write,
     lang: Lang,
     opts: &JoinOpts,
-    wait: Duration,
-    pause: &dyn Fn(),
 ) -> i32 {
-    let Some(now) = view_of(call, Request::MeshStatus, err, lang) else {
-        return 1;
-    };
-    if !now.logged_in {
-        say_error(err, lang, &ErrorCode::Mesh(MeshProblem::NotLoggedIn));
-        return 1;
-    }
-    // 邀请的有效期从守护进程问到那几台电脑的那一刻算起（`Mesh::set_invites`），
-    // 等同意的期限也从这里算——不从认定之后才算，不然用户在提示那儿想一会儿，
-    // 这边就会接着等一段那边早已不收的时间，最后只报一句笼统的超时。
-    let deadline = Instant::now() + wait;
     let req = Request::MeshJoin {
+        code: opts.code.clone(),
         name: opts.name.clone(),
     };
-    let Some(asked) = view_of(call, req, err, lang) else {
-        return 1;
-    };
-    // 同名的不止一台时，名字认不出是哪台：把端点印在旁边，用户照着敲。
-    let line = |j: &PendingJoin| {
-        let dup = asked.joining.iter().filter(|x| x.name == j.name).count() > 1;
-        if dup {
-            msg::mesh_code_line_with_endpoint(lang, &c(&j.name), &c(&j.endpoint), &c(&j.code))
-        } else {
-            msg::mesh_code_line(lang, &c(&j.name), &c(&j.code))
-        }
-    };
-    match asked.joining.as_slice() {
-        [one] => {
-            let _ = writeln!(
-                out,
-                "{}",
-                msg::mesh_compare_codes(lang, Some(&c(&one.code)))
-            );
-            let _ = writeln!(out, "{}", line(one));
-        }
-        many => {
-            let _ = writeln!(out, "{}", msg::mesh_compare_codes(lang, None));
-            for j in many {
-                let _ = writeln!(out, "{}", line(j));
-            }
-        }
-    }
-    let _ = out.flush();
-
-    let choice = match &opts.confirm {
-        Some(c) => Some(c.clone()),
-        None => ask(&msg::mesh_which_computer(lang)),
-    };
-    let choice = choice.map(|c| c.trim().to_string()).unwrap_or_default();
-    if choice.is_empty() {
-        let _ = writeln!(err, "{}", msg::mesh_join_cancelled(lang));
-        return 1;
-    }
-    let inviter = match pick(&asked.joining, &choice) {
-        Ok(p) => p.clone(),
-        Err(0) => {
-            let _ = writeln!(err, "{}", msg::mesh_not_a_responder(lang, &choice));
-            return 1;
-        }
-        Err(_) => {
-            let _ = writeln!(err, "{}", msg::mesh_ambiguous_responder(lang, &choice));
-            return 1;
-        }
-    };
-    let req = Request::MeshConfirmInviter {
-        endpoint: inviter.endpoint.clone(),
-    };
     let Some(v) = view_of(call, req, err, lang) else {
         return 1;
     };
-    // 那边可能已经点过同意：认定的那一刻就进组了。
-    if grouped(&v) {
-        let _ = writeln!(out, "{}", msg::mesh_joined_group(lang));
-        return 0;
-    }
-    let _ = writeln!(out, "{}", msg::mesh_waiting_for_approval(lang));
-    let _ = out.flush();
-
-    loop {
-        pause();
-        if let Some(v) = view_of(call, Request::MeshStatus, err, lang) {
-            if grouped(&v) {
-                let _ = writeln!(out, "{}", msg::mesh_joined_group(lang));
-                return 0;
-            }
-            // 守护进程已经把这次邀请作废了（过了期限，或者别处又跑了一次
-            // `dct join`）：再等也收不到，现在就说。
-            if v.joining.is_empty() {
-                let _ = writeln!(err, "{}", msg::mesh_join_timed_out(lang));
-                return 1;
-            }
-        }
-        if Instant::now() >= deadline {
-            let _ = writeln!(err, "{}", msg::mesh_join_timed_out(lang));
-            return 1;
-        }
+    let names: Vec<String> = v.members.iter().map(|m| c(&m.name)).collect();
+    let _ = writeln!(out, "{}", msg::mesh_joined_with(lang, &names));
+    // 起的名字跟组里的撞了，老电脑给编了号：告诉他现在叫什么。
+    if !opts.name.trim().is_empty() && v.name != opts.name.trim() {
+        let _ = writeln!(out, "{}", msg::mesh_renamed_on_join(lang, &c(&v.name)));
     }
+    0
 }
 
 pub(crate) fn peers(
@@ -611,6 +508,8 @@ mod tests {
             pending: vec![],
             joining: vec![],
             messages: Default::default(),
+            invite: None,
+            invite_note: None,
         }
     }
 
@@ -713,7 +612,12 @@ mod tests {
         let all = [
             Request::MeshStatus,
             Request::MeshLogin,
-            Request::MeshJoin { name: "n".into() },
+            Request::MeshJoin {
+                code: "482913".into(),
+                name: "n".into(),
+            },
+            Request::MeshInvite,
+            Request::MeshInviteCancel,
             Request::MeshApprove {
                 endpoint: "c-x".into(),
                 code: "123456".into(),
@@ -802,25 +706,21 @@ mod tests {
         let o = s(&out);
         assert!(!o.contains('\x1b') && !o.contains('\u{202e}'), "{o:?}");
 
-        // `dct join` 列数字的那几行。
-        let mut asked = view(true, &[("B", true)]);
-        asked.joining = vec![pj(evil, "c-a1", "111111")];
-        let sc = Script::new(vec![
-            Response::Mesh(view(true, &[("B", true)])),
-            Response::Mesh(asked),
-        ]);
-        let mut ask = |_: &str| None;
+        // `dct join` 列组里有谁的那一行。
+        let mut joined = view(true, &[("B", true)]);
+        joined.members.push(crate::proto::MemberView {
+            name: evil.into(),
+            endpoint: "c-e".into(),
+            online: true,
+            is_me: false,
+        });
+        let sc = Script::new(vec![Response::Mesh(joined)]);
         let (mut out, mut err) = (vec![], vec![]);
-        join(
-            &mut sc.call(),
-            &mut ask,
-            &mut out,
-            &mut err,
-            Lang::Zh,
-            &opts(None),
-            Duration::from_secs(60),
-            &|| {},
-        );
+        let o = JoinOpts {
+            code: "482913".into(),
+            name: String::new(),
+        };
+        assert_eq!(join(&mut sc.call(), &mut out, &mut err, Lang::Zh, &o), 0);
         let o = s(&out);
         assert!(!o.contains('\x1b') && !o.contains('\u{202e}'), "{o:?}");
     }
@@ -897,342 +797,6 @@ mod tests {
         panic!("不该问用户")
     }
 
-    fn opts(confirm: Option<&str>) -> JoinOpts {
-        JoinOpts {
-            name: String::new(),
-            confirm: confirm.map(str::to_string),
-        }
-    }
-
-    /// 两台回了话：逐台列数字，问是哪一台，用户说「家里Mac」，送去认定的
-    /// 就是它的端点；然后等到名单。
-    #[test]
-    fn join_lists_codes_asks_which_computer_and_confirms_that_one() {
-        let mut asked = view(true, &[("B", true)]);
-        asked.joining = vec![
-            pj("家里Mac", "c-a", "111111"),
-            pj("办公室", "c-c", "222222"),
-        ];
-        let sc = Script::new(vec![
-            Response::Mesh(view(true, &[("B", true)])),
-            Response::Mesh(asked.clone()),
-            Response::Mesh(asked.clone()),
-            Response::Mesh(asked),
-            Response::Mesh(view(true, &[("B", true), ("家里Mac", false)])),
-        ]);
-        let prompts = RefCell::new(Vec::<String>::new());
-        let mut ask = |p: &str| {
-            prompts.borrow_mut().push(p.to_string());
-            Some("办公室".to_string())
-        };
-        let (mut out, mut err) = (vec![], vec![]);
-        let o = JoinOpts {
-            name: "公司Windows".into(),
-            confirm: None,
-        };
-        let code = join(
-            &mut sc.call(),
-            &mut ask,
-            &mut out,
-            &mut err,
-            Lang::Zh,
-            &o,
-            Duration::from_secs(60),
-            &|| {},
-        );
-        assert_eq!(code, 0, "{}", s(&err));
-        let o = s(&out);
-        assert!(o.contains("  家里Mac：111111"), "{o}");
-        assert!(o.contains("  办公室：222222"), "{o}");
-        assert!(o.trim_end().ends_with("已加入"), "{o}");
-        assert_eq!(prompts.borrow().len(), 1);
-        assert!(prompts.borrow()[0].contains("哪一台电脑"));
-        let seen = sc.seen.borrow();
-        assert!(seen[1].contains("公司Windows"));
-        assert_eq!(seen[2], r#"MeshConfirmInviter { endpoint: "c-c" }"#);
-    }
-
-    #[test]
-    fn join_with_one_computer_puts_the_code_in_the_sentence_and_still_asks() {
-        let mut asked = view(true, &[("B", true)]);
-        asked.joining = vec![pj("A", "c-a", "654321")];
-        let sc = Script::new(vec![
-            Response::Mesh(view(true, &[("B", true)])),
-            Response::Mesh(asked),
-            // 那边已经点过同意：认定那一刻就进组了，不再等。
-            Response::Mesh(view(true, &[("B", true), ("A", false)])),
-        ]);
-        let asked_user = RefCell::new(false);
-        let mut ask = |_: &str| {
-            *asked_user.borrow_mut() = true;
-            Some("A".to_string())
-        };
-        let (mut out, mut err) = (vec![], vec![]);
-        let code = join(
-            &mut sc.call(),
-            &mut ask,
-            &mut out,
-            &mut err,
-            Lang::Zh,
-            &opts(None),
-            Duration::from_secs(60),
-            &|| panic!("不该再等"),
-        );
-        assert_eq!(code, 0);
-        assert!(*asked_user.borrow(), "只有一台也要用户说一声");
-        assert!(s(&out).contains(
-            "请在你已有的任意一台电脑上看一眼：那边会显示一个 6 位数，跟这里的 654321 一样就点同意"
-        ));
-    }
-
-    /// `--confirm` 不问用户，直接认定。
-    #[test]
-    fn join_confirm_flag_skips_the_question() {
-        let mut asked = view(true, &[("B", true)]);
-        asked.joining = vec![pj("A", "c-a", "1"), pj("M", "c-m", "2")];
-        let sc = Script::new(vec![
-            Response::Mesh(view(true, &[("B", true)])),
-            Response::Mesh(asked),
-            Response::Mesh(view(true, &[("B", true), ("A", false)])),
-        ]);
-        let (mut out, mut err) = (vec![], vec![]);
-        let code = join(
-            &mut sc.call(),
-            &mut no_ask,
-            &mut out,
-            &mut err,
-            Lang::Zh,
-            &opts(Some("A")),
-            Duration::from_secs(60),
-            &|| {},
-        );
-        assert_eq!(code, 0);
-        assert_eq!(
-            sc.seen.borrow()[2],
-            r#"MeshConfirmInviter { endpoint: "c-a" }"#
-        );
-    }
-
-    /// 数字都对不上（直接回车）、说了一台没回过话的：不认定任何一台，退 1。
-    #[test]
-    fn join_without_a_matching_computer_confirms_nothing() {
-        for (answer, want) in [
-            (Some(""), "没有加入"),
-            (None, "没有加入"),
-            (Some("Z"), "没有叫 Z 的电脑回应过这次加入"),
-        ] {
-            let mut asked = view(true, &[("B", true)]);
-            asked.joining = vec![pj("A", "c-a", "1")];
-            let sc = Script::new(vec![
-                Response::Mesh(view(true, &[("B", true)])),
-                Response::Mesh(asked),
-            ]);
-            let mut ask = |_: &str| answer.map(str::to_string);
-            let (mut out, mut err) = (vec![], vec![]);
-            let code = join(
-                &mut sc.call(),
-                &mut ask,
-                &mut out,
-                &mut err,
-                Lang::Zh,
-                &opts(None),
-                Duration::from_secs(60),
-                &|| {},
-            );
-            assert_eq!(code, 1);
-            assert!(s(&err).contains(want), "{answer:?}: {}", s(&err));
-            assert_eq!(sc.seen.borrow().len(), 2, "没送认定");
-        }
-    }
-
-    #[test]
-    fn join_gives_up_after_the_wait() {
-        let mut asked = view(true, &[("B", true)]);
-        asked.joining = vec![pj("A", "c-a", "1")];
-        let sc = Script::new(vec![
-            Response::Mesh(view(true, &[("B", true)])),
-            Response::Mesh(asked.clone()),
-            Response::Mesh(asked.clone()),
-            Response::Mesh(asked),
-        ]);
-        let (mut out, mut err) = (vec![], vec![]);
-        let code = join(
-            &mut sc.call(),
-            &mut no_ask,
-            &mut out,
-            &mut err,
-            Lang::Zh,
-            &opts(Some("A")),
-            Duration::ZERO,
-            &|| {},
-        );
-        assert_eq!(code, 1);
-        assert_eq!(
-            s(&err).trim(),
-            "10 分钟里没等到同意，邀请已过期，请重新运行 dct join"
-        );
-    }
-
-    /// 审查给的 m3：等同意的期限从问到那几台电脑时算起，跟邀请的有效期
-    /// 同一个起点。用户在「哪一台」那儿想了比期限还久，认定之后只再看一眼
-    /// 就该说过期，不该再接着等一整段。
-    #[test]
-    fn the_join_wait_starts_when_the_invites_were_made_not_after_confirming() {
-        let mut asked = view(true, &[("B", true)]);
-        asked.joining = vec![pj("A", "c-a", "1")];
-        let sc = Script::new(vec![
-            Response::Mesh(view(true, &[("B", true)])),
-            Response::Mesh(asked.clone()),
-            Response::Mesh(asked.clone()),
-            // 只准再问这一次：从认定那一刻才起算的话，这里会接着问下去。
-            Response::Mesh(asked),
-        ]);
-        let mut slow_user = |_: &str| {
-            std::thread::sleep(Duration::from_millis(60));
-            Some("A".to_string())
-        };
-        let (mut out, mut err) = (vec![], vec![]);
-        let code = join(
-            &mut sc.call(),
-            &mut slow_user,
-            &mut out,
-            &mut err,
-            Lang::Zh,
-            &opts(None),
-            Duration::from_millis(30),
-            &|| {},
-        );
-        assert_eq!(code, 1);
-        assert!(s(&err).contains("邀请已过期"), "{}", s(&err));
-        assert_eq!(sc.seen.borrow().len(), 4);
-    }
-
-    /// 守护进程那边邀请已经作废了（`joining` 空了、也没进组）：马上说过期，
-    /// 不等到期限。
-    #[test]
-    fn join_says_expired_as_soon_as_the_daemon_drops_the_invite() {
-        let mut asked = view(true, &[("B", true)]);
-        asked.joining = vec![pj("A", "c-a", "1")];
-        let sc = Script::new(vec![
-            Response::Mesh(view(true, &[("B", true)])),
-            Response::Mesh(asked.clone()),
-            Response::Mesh(asked),
-            Response::Mesh(view(true, &[("B", true)])),
-        ]);
-        let (mut out, mut err) = (vec![], vec![]);
-        let code = join(
-            &mut sc.call(),
-            &mut no_ask,
-            &mut out,
-            &mut err,
-            Lang::Zh,
-            &opts(Some("A")),
-            Duration::from_secs(600),
-            &|| {},
-        );
-        assert_eq!(code, 1);
-        assert!(
-            s(&err).contains("邀请已过期，请重新运行 dct join"),
-            "{}",
-            s(&err)
-        );
-    }
-
-    /// 审查给的 m2：两台同名都回了话。每行名字旁边印出端点，用户照着敲
-    /// 端点就能认定；只敲名字的话，说清楚是回话的同名、该敲什么。
-    #[test]
-    fn join_with_two_responders_of_the_same_name_shows_their_endpoints() {
-        let mut asked = view(true, &[("B", true)]);
-        asked.joining = vec![
-            pj("A", "c-a1", "111111"),
-            pj("A", "c-a2", "222222"),
-            pj("C", "c-c", "333333"),
-        ];
-        let sc = Script::new(vec![
-            Response::Mesh(view(true, &[("B", true)])),
-            Response::Mesh(asked.clone()),
-            Response::Mesh(view(true, &[("B", true), ("A", false)])),
-        ]);
-        let mut ask = |_: &str| Some("c-a2".to_string());
-        let (mut out, mut err) = (vec![], vec![]);
-        let code = join(
-            &mut sc.call(),
-            &mut ask,
-            &mut out,
-            &mut err,
-            Lang::Zh,
-            &opts(None),
-            Duration::from_secs(60),
-            &|| {},
-        );
-        assert_eq!(code, 0, "{}", s(&err));
-        let o = s(&out);
-        assert!(o.contains("  A (c-a1)：111111"), "{o}");
-        assert!(o.contains("  A (c-a2)：222222"), "{o}");
-        assert!(o.contains("  C：333333"), "不同名的照旧只印名字：{o}");
-        assert_eq!(
-            sc.seen.borrow()[2],
-            r#"MeshConfirmInviter { endpoint: "c-a2" }"#
-        );
-
-        // 只敲名字：不送认定，告诉他敲括号里的编号。
-        let sc = Script::new(vec![
-            Response::Mesh(view(true, &[("B", true)])),
-            Response::Mesh(asked),
-        ]);
-        let mut ask = |_: &str| Some("A".to_string());
-        let (mut out, mut err) = (vec![], vec![]);
-        let code = join(
-            &mut sc.call(),
-            &mut ask,
-            &mut out,
-            &mut err,
-            Lang::Zh,
-            &opts(None),
-            Duration::from_secs(60),
-            &|| {},
-        );
-        assert_eq!(code, 1);
-        assert_eq!(
-            s(&err).trim(),
-            "不止一台叫 A 的电脑回应了。重新运行 dct join，输入数字对得上的那台后面括号里的 c-… 编号"
-        );
-        assert_eq!(sc.seen.borrow().len(), 2);
-    }
-
-    #[test]
-    fn join_before_login_says_to_log_in() {
-        let sc = Script::new(vec![Response::Mesh(view(false, &[]))]);
-        let (mut out, mut err) = (vec![], vec![]);
-        let code = join(
-            &mut sc.call(),
-            &mut no_ask,
-            &mut out,
-            &mut err,
-            Lang::Zh,
-            &opts(None),
-            Duration::ZERO,
-            &|| {},
-        );
-        assert_eq!(code, 1);
-        assert_eq!(s(&err).trim(), "还没登录多电脑，先运行 dct login");
-    }
-
-    #[test]
-    fn join_arguments() {
-        let a = |v: &[&str]| parse_join(&v.iter().map(|s| s.to_string()).collect::<Vec<_>>());
-        assert_eq!(a(&[]), Some(JoinOpts::default()));
-        assert_eq!(a(&["--name", "公司Windows"]).unwrap().name, "公司Windows");
-        assert_eq!(a(&["--name=家里"]).unwrap().name, "家里");
-        let both = a(&["--name", "N", "--confirm", "家里Mac"]).unwrap();
-        assert_eq!(both.name, "N");
-        assert_eq!(both.confirm.as_deref(), Some("家里Mac"));
-        assert_eq!(a(&["--confirm=A"]).unwrap().confirm.as_deref(), Some("A"));
-        assert_eq!(a(&["--nope"]), None);
-        assert_eq!(a(&["--name"]), None);
-        assert_eq!(a(&["--confirm"]), None);
-    }
-
     #[test]
     fn peers_lists_members_and_asks_about_pending_joins() {
         let mut v = view(true, &[("B", true), ("A", false)]);
@@ -1560,4 +1124,88 @@ mod tests {
             2
         );
     }
+
+    #[test]
+    fn join_arguments() {
+        let a = |v: &[&str]| parse_join(&v.iter().map(|s| s.to_string()).collect::<Vec<_>>());
+        let j = |code: &str, name: &str| {
+            Some(JoinOpts {
+                code: code.into(),
+                name: name.into(),
+            })
+        };
+        assert_eq!(a(&["482913"]), j("482913", ""));
+        assert_eq!(a(&["482", "913"]), j("482913", ""), "shell 把空格拆开了");
+        assert_eq!(a(&["482-913"]), j("482-913", ""));
+        assert_eq!(a(&["012345"]), j("012345", ""), "前导 0 原样");
+        assert_eq!(a(&["482913", "--name", "公司电脑"]), j("482913", "公司电脑"));
+        assert_eq!(a(&["--name=家里", "482913"]), j("482913", "家里"));
+        assert_eq!(a(&[]), None, "没有码");
+        assert_eq!(a(&["--name", "N"]), None, "只有名字没有码");
+        assert_eq!(a(&["482913", "--confirm", "A"]), None, "--confirm 没有了");
+        assert_eq!(a(&["482913", "--name"]), None);
+    }
+
+    /// 送给守护进程的就是用户敲的码和名字；进组之后报组里有谁。
+    #[test]
+    fn join_sends_the_code_and_says_who_is_in_the_group() {
+        let mut v = view(true, &[("Mac", false), ("公司电脑", true)]);
+        v.name = "公司电脑".into();
+        let sc = Script::new(vec![Response::Mesh(v)]);
+        let (mut out, mut err) = (vec![], vec![]);
+        let o = JoinOpts {
+            code: "482 913".into(),
+            name: "公司电脑".into(),
+        };
+        assert_eq!(join(&mut sc.call(), &mut out, &mut err, Lang::Zh, &o), 0);
+        assert_eq!(
+            sc.seen.borrow().as_slice(),
+            [r#"MeshJoin { code: "******", name: "公司电脑" }"#]
+        );
+        assert_eq!(s(&out).trim(), "已加入「我的电脑」，组里有：Mac、公司电脑");
+    }
+
+    #[test]
+    fn join_tells_the_new_name_when_the_inviter_numbered_it() {
+        let mut v = view(true, &[("Mac", false), ("Mac 2", true)]);
+        v.name = "Mac 2".into();
+        let sc = Script::new(vec![Response::Mesh(v)]);
+        let (mut out, mut err) = (vec![], vec![]);
+        let o = JoinOpts {
+            code: "482913".into(),
+            name: "Mac".into(),
+        };
+        assert_eq!(join(&mut sc.call(), &mut out, &mut err, Lang::Zh, &o), 0);
+        assert!(s(&out).contains("组里已经有一台叫这个名字的了，这台电脑现在叫 Mac 2"), "{}", s(&out));
+    }
+
+    #[test]
+    fn join_errors_are_said_in_words_and_exit_1() {
+        for (p, want) in [
+            (MeshProblem::WrongInviteCode, "邀请码不对或已作废，请在老电脑上按 a 重新生成"),
+            (MeshProblem::BadInviteCode, "邀请码是 6 位数字，比如：dct join 482913"),
+            (MeshProblem::NoDcAccount, "先在 dct 里用 DC 配对账号（进 dct 按 c 选 DC）"),
+        ] {
+            let sc = Script::new(vec![Response::Error(ErrorCode::Mesh(p))]);
+            let (mut out, mut err) = (vec![], vec![]);
+            let o = JoinOpts {
+                code: "1".into(),
+                name: String::new(),
+            };
+            assert_eq!(join(&mut sc.call(), &mut out, &mut err, Lang::Zh, &o), 1);
+            assert_eq!(s(&err).trim(), want);
+            assert!(out.is_empty());
+        }
+    }
+
+    #[test]
+    fn join_without_a_code_prints_the_usage() {
+        let sc = Script::new(vec![Response::Hello {
+            protocol: crate::proto::PROTOCOL_VERSION,
+        }]);
+        let (mut out, mut err) = (vec![], vec![]);
+        let code = dispatch("join", &[], &mut sc.call(), &mut no_ask, &mut out, &mut err, Lang::Zh);
+        assert_eq!(code, 2);
+        assert!(s(&err).contains("dct join <邀请码>"), "{}", s(&err));
+    }
 }
diff --git a/src/mesh/group.rs b/src/mesh/group.rs
index 55cfe7b..58db384 100644
--- a/src/mesh/group.rs
+++ b/src/mesh/group.rs
@@ -68,6 +68,10 @@ pub fn view(mesh: &Mutex<Mesh>, net: &dyn Net, logged_in: bool) -> MeshView {
             .collect(),
         joining: joining(&m),
         messages: m.delivered_counts(),
+        // 先 `invite_view`：它顺手把到点的码清掉、记下结果，下面读到的
+        // `invite_note` 才是新的。
+        invite: m.invite_view(),
+        invite_note: m.invite_note.clone(),
     }
 }
 
diff --git a/src/mesh/mod.rs b/src/mesh/mod.rs
index e3f4fa0..c47db9a 100644
--- a/src/mesh/mod.rs
+++ b/src/mesh/mod.rs
@@ -755,8 +755,17 @@ pub fn worst_case(req: &crate::proto::Request) -> Option<std::time::Duration> {
     Some(match req {
         R::MeshStatus | R::MeshConfirmInviter { .. } => call,
         R::MeshLogin => login::GATEWAY_TIMEOUT + call,
-        // 问谁在线、并排问每台、并排揭晓、再看一眼现状。
-        R::MeshJoin { .. } => call + group::JOIN_ASK_TIMEOUT + call + call,
+        R::MeshInvite | R::MeshInviteCancel => call,
+        // 可能先登录（换令牌）；问谁在线、并排探问；最多试
+        // `MAX_INVITERS_TRIED` 台、每台两次 ask；再看一眼现状。
+        R::MeshJoin { .. } => {
+            login::GATEWAY_TIMEOUT
+                + call
+                + call
+                + invite::INVITE_ASK_TIMEOUT
+                + invite::INVITE_ASK_TIMEOUT * 2 * invite::MAX_INVITERS_TRIED as u32
+                + call
+        }
         // 并排广播新名单，再看一眼现状。
         R::MeshApprove { .. } | R::MeshRemove { .. } => call + call,
         R::MeshPeers => call + deliver::STATUS_ASK_TIMEOUT,
diff --git a/src/proto.rs b/src/proto.rs
index 5b474ff..b60035a 100644
--- a/src/proto.rs
+++ b/src/proto.rs
@@ -133,7 +133,12 @@ use crate::session::{ScrollBy, ScrollState, SessionInfo, SessionState};
 /// 23 = 看板上的「我的电脑」：`MeshView` 多了 `messages`（这次守护进程运行
 /// 期间，别的电脑送进每个会话的留言条数）。**响应**的形状变了，照 13 那次
 /// 的规矩加一。
-pub const PROTOCOL_VERSION: u32 = 23;
+///
+/// 24 = 用 6 位邀请码加电脑（dct-invite-v1）：多了 `Request::MeshInvite` /
+/// `MeshInviteCancel`、`Response::MeshInvite(InviteView)`，`MeshJoin` 多了
+/// `code`，`MeshView` 多了 `invite` / `invite_note`，`MeshProblem` 多了
+/// `BadInviteCode` / `WrongInviteCode` / `NoInvite` / `InviteRosterRefused`。
+pub const PROTOCOL_VERSION: u32 = 24;
 
 /// 对面那个守护进程能不能用。
 #[derive(Debug, Clone, Copy, PartialEq, Eq)]
@@ -519,11 +524,19 @@ pub enum Request {
     /// 拿 DC 账号的 `api_key` 跟网关换中转令牌、连上中转；还没有组就自己
     /// 建一个。会打网络（网关），界面要放后台线程。
     MeshLogin,
-    /// 在新电脑上请求加入：问一遍在线的其它电脑，回答里带着每台的 6 位数。
-    /// `name` 非空就顺手改名。
+    /// 在新电脑上用老电脑给的邀请码加入（`dct join <码>`）。还没登录就先
+    /// 登录（要已配对同一个 DC 账号）。`name` 非空就用这个名字进组。
+    ///
+    /// **`code` 不进 `Debug`**：10 分钟里它就是进组的钥匙。
     MeshJoin {
+        code: String,
         name: String,
     },
+    /// 出一个新的邀请码（`dct invite`、看板上按 `a`）。旧的作废。回
+    /// `Response::MeshInvite`。
+    MeshInvite,
+    /// 收回手上的邀请码（`dct invite` 被 Ctrl-C）。
+    MeshInviteCancel,
     /// 批准（`yes`）或拒绝一台等着加入的电脑。
     ///
     /// `endpoint` 和 `code` 都是**界面刚给用户看过的那一条**：守护进程要两样
@@ -684,7 +697,14 @@ impl std::fmt::Debug for Request {
             // 电脑名、端点都不是密钥。
             Request::MeshStatus => write!(f, "MeshStatus"),
             Request::MeshLogin => write!(f, "MeshLogin"),
-            Request::MeshJoin { name } => f.debug_struct("MeshJoin").field("name", name).finish(),
+            // 邀请码是 10 分钟有效的口令，只报有没有。
+            Request::MeshJoin { code, name } => f
+                .debug_struct("MeshJoin")
+                .field("code", &if code.is_empty() { "" } else { "******" })
+                .field("name", name)
+                .finish(),
+            Request::MeshInvite => write!(f, "MeshInvite"),
+            Request::MeshInviteCancel => write!(f, "MeshInviteCancel"),
             Request::MeshApprove {
                 endpoint,
                 code,
@@ -811,6 +831,8 @@ pub enum Response {
     LiveGrant(LiveGrantToken),
     /// `Mesh*` 几条请求的共同回答：做完之后的样子。
     Mesh(MeshView),
+    /// 对 [`Request::MeshInvite`] 的回答：新出的码。
+    MeshInvite(InviteView),
     /// 对 [`Request::MeshPeers`] 的回答，按名单顺序，这台电脑自己也在里面。
     MeshPeers(Vec<PeerView>),
     /// 对 [`Request::MeshSend`] 的回答。
@@ -878,6 +900,10 @@ pub struct MeshView {
     /// 会话 id → 这次守护进程运行期间送进这个会话的留言条数（看板上的
     /// 「✉ N」）。只记真的敲进去了的，排着队的不算。
     pub messages: std::collections::BTreeMap<u32, u32>,
+    /// 这台电脑此刻发着的邀请码。没有就是 `None`。
+    pub invite: Option<InviteView>,
+    /// 最近一个结束了的邀请码怎么结束的。
+    pub invite_note: Option<InviteNote>,
 }
 
 #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
@@ -1564,7 +1590,12 @@ mod tests {
             Request::LivePublishGrant,
             Request::MeshStatus,
             Request::MeshLogin,
-            Request::MeshJoin { name: "n".into() },
+            Request::MeshJoin {
+                code: "482913".into(),
+                name: "n".into(),
+            },
+            Request::MeshInvite,
+            Request::MeshInviteCancel,
             Request::MeshApprove {
                 endpoint: "c-x".into(),
                 code: "123456".into(),
@@ -1586,8 +1617,8 @@ mod tests {
         assert_eq!(
             (PROTOCOL_VERSION, shape.as_str()),
             (
-                23,
-                r#"["Hello","List",{"Create":{"dir":"d","profile":"p","remember":true}},{"Input":{"id":1,"text":"t"}},{"Screen":{"id":1}},{"Screens":{"ids":[1]}},{"Resize":{"id":1,"rows":2,"cols":3}},{"Stop":{"id":1}},{"Kill":{"id":1}},"Prune",{"Undo":{"id":1}},{"Diff":{"id":1}},{"Profiles":{"lang":"Zh"}},"Projects",{"SetSecret":{"profile":"p","value":"v"}},{"DeleteSecret":{"profile":"p"}},{"LastProfile":{"dir":"d"}},{"PinProject":{"dir":"d"}},{"UnpinProject":{"dir":"d"}},{"VerifySecret":{"profile":"p","value":"v"}},{"PairStart":{"profile":"p","opt_in_llm":true}},{"PairPoll":{"profile":"p","opt_in_llm":true}},{"PairCancel":{"profile":"p"}},{"Explanation":{"id":1}},{"Scroll":{"id":1,"by":{"Rows":3}}},{"Mouse":{"id":1,"event":{"col":10,"row":20,"kind":{"Press":0},"shift":false,"alt":false,"ctrl":false}}},"PhoneStatus",{"PhoneSetToken":{"token":"t"}},"PhoneUnpair","PhoneDisable",{"Key":{"id":1,"name":"Up"}},{"WebStrings":{"lang":"zh-CN"}},"WebStatus","WebEnable","WebDisable",{"LiveStart":{"ids":[1],"names":["n"]}},{"LiveRestage":{"ids":[1],"names":["n"]}},"LiveStop","LiveStatus",{"LivePublish":{"title":"t"}},"LiveUnpublish","LivePublishGrant","MeshStatus","MeshLogin",{"MeshJoin":{"name":"n"}},{"MeshApprove":{"endpoint":"c-x","code":"123456","yes":true}},{"MeshRemove":{"name":"n"}},{"MeshConfirmInviter":{"endpoint":"c-x"}},"MeshPeers",{"MeshSend":{"to":"pc/s","text":"t","from_session":1}}]"#
+                24,
+                r#"["Hello","List",{"Create":{"dir":"d","profile":"p","remember":true}},{"Input":{"id":1,"text":"t"}},{"Screen":{"id":1}},{"Screens":{"ids":[1]}},{"Resize":{"id":1,"rows":2,"cols":3}},{"Stop":{"id":1}},{"Kill":{"id":1}},"Prune",{"Undo":{"id":1}},{"Diff":{"id":1}},{"Profiles":{"lang":"Zh"}},"Projects",{"SetSecret":{"profile":"p","value":"v"}},{"DeleteSecret":{"profile":"p"}},{"LastProfile":{"dir":"d"}},{"PinProject":{"dir":"d"}},{"UnpinProject":{"dir":"d"}},{"VerifySecret":{"profile":"p","value":"v"}},{"PairStart":{"profile":"p","opt_in_llm":true}},{"PairPoll":{"profile":"p","opt_in_llm":true}},{"PairCancel":{"profile":"p"}},{"Explanation":{"id":1}},{"Scroll":{"id":1,"by":{"Rows":3}}},{"Mouse":{"id":1,"event":{"col":10,"row":20,"kind":{"Press":0},"shift":false,"alt":false,"ctrl":false}}},"PhoneStatus",{"PhoneSetToken":{"token":"t"}},"PhoneUnpair","PhoneDisable",{"Key":{"id":1,"name":"Up"}},{"WebStrings":{"lang":"zh-CN"}},"WebStatus","WebEnable","WebDisable",{"LiveStart":{"ids":[1],"names":["n"]}},{"LiveRestage":{"ids":[1],"names":["n"]}},"LiveStop","LiveStatus",{"LivePublish":{"title":"t"}},"LiveUnpublish","LivePublishGrant","MeshStatus","MeshLogin",{"MeshJoin":{"code":"482913","name":"n"}},"MeshInvite","MeshInviteCancel",{"MeshApprove":{"endpoint":"c-x","code":"123456","yes":true}},{"MeshRemove":{"name":"n"}},{"MeshConfirmInviter":{"endpoint":"c-x"}},"MeshPeers",{"MeshSend":{"to":"pc/s","text":"t","from_session":1}}]"#
             ),
             "协议的线上形状变了。把 PROTOCOL_VERSION 加一，再把这里的期望值更新成新的形状。"
         );
@@ -1611,7 +1642,7 @@ mod tests {
         assert_eq!(
             (PROTOCOL_VERSION, json.as_str()),
             (
-                23,
+                24,
                 r#"{"Done":{"anthropic_ready":true,"openai_ready":true,"llm_written":true}}"#
             ),
             "PairTick 的线上形状变了。把 PROTOCOL_VERSION 加一，再把这里的期望值更新成新的形状。"
@@ -1720,7 +1751,7 @@ mod tests {
         assert_eq!(
             (PROTOCOL_VERSION, shape.as_str()),
             (
-                23,
+                24,
                 r#"{"id":1,"profile":"claude","dir":"/d","state":"Idle","activity":"a","is_agent":true,"tag":""}"#
             ),
             "会话信息的线上形状变了。把 PROTOCOL_VERSION 加一，再把这里的期望值更新成新的形状。"
@@ -1823,7 +1854,7 @@ mod tests {
         let r = Response::Error(ErrorCode::LiveRelayNotConfigured);
         assert_eq!(
             (PROTOCOL_VERSION, serde_json::to_string(&r).unwrap().as_str()),
-            (23, r#"{"Error":"LiveRelayNotConfigured"}"#),
+            (24, r#"{"Error":"LiveRelayNotConfigured"}"#),
             "协议的线上形状变了。把 PROTOCOL_VERSION 加一，再把这里和 server.mjs 一起更新。"
         );
     }
@@ -1841,7 +1872,7 @@ mod tests {
         let s = serde_json::to_string(&r).unwrap();
         assert_eq!(
             (PROTOCOL_VERSION, s.as_str()),
-            (23, r#"{"Projects":{"recent":["/a"],"pinned":["/b"]}}"#),
+            (24, r#"{"Projects":{"recent":["/a"],"pinned":["/b"]}}"#),
             "协议的线上形状变了。把 PROTOCOL_VERSION 加一，再把这里的期望值更新成新的形状。"
         );
     }
@@ -1918,7 +1949,7 @@ mod tests {
                 shape(&LivePublic::Listed { title: "课".into() })
             ),
             (
-                23,
+                24,
                 r#""Private""#.to_string(),
                 r#"{"Listed":{"title":"课"}}"#.to_string()
             )
@@ -1955,7 +1986,7 @@ mod tests {
         assert_eq!(
             (PROTOCOL_VERSION, sent[0].as_str(), sent[1].as_str(), peers.as_str()),
             (
-                23,
+                24,
                 r#"{"MeshSent":"Delivered"}"#,
                 r##"{"MeshSent":{"NoSuchSession":["#3 a（b）"]}}"##,
                 r#"{"MeshPeers":[{"name":"A","online":true,"os":"macos","sessions":[{"name":"s","state":"闲","dir":"/d"}],"tentacles":["cam"]}]}"#
@@ -1985,12 +2016,23 @@ mod tests {
             }],
             joining: Vec::new(),
             messages: [(7, 2)].into_iter().collect(),
+            invite: Some(InviteView {
+                id: 3,
+                code: "012345".into(),
+                expires_at: 1_800_000_600,
+            }),
+            invite_note: Some(InviteNote {
+                id: 2,
+                outcome: InviteOutcome::Joined {
+                    name: "公司电脑".into(),
+                },
+            }),
         });
         assert_eq!(
             (PROTOCOL_VERSION, serde_json::to_string(&v).unwrap().as_str()),
             (
-                23,
-                r#"{"Mesh":{"logged_in":true,"name":"A","endpoint":"c-a","in_group":true,"members":[{"name":"A","endpoint":"c-a","online":true,"is_me":true}],"pending":[{"name":"B","endpoint":"c-b","code":"123456"}],"joining":[],"messages":{"7":2}}}"#
+                24,
+                r#"{"Mesh":{"logged_in":true,"name":"A","endpoint":"c-a","in_group":true,"members":[{"name":"A","endpoint":"c-a","online":true,"is_me":true}],"pending":[{"name":"B","endpoint":"c-b","code":"123456"}],"joining":[],"messages":{"7":2},"invite":{"id":3,"code":"012345","expires_at":1800000600},"invite_note":{"id":2,"outcome":{"Joined":{"name":"公司电脑"}}}}}"#
             ),
             "协议的线上形状变了。把 PROTOCOL_VERSION 加一，再把这里的期望值更新成新的形状。"
         );
@@ -2003,6 +2045,29 @@ mod tests {
         assert_eq!(back, sent);
     }
 
+    /// 邀请码不进 `Debug`：`MeshJoin` 只报有没有码，`InviteView` 打星号。
+    #[test]
+    fn invite_codes_never_show_up_in_debug() {
+        let r = Request::MeshJoin {
+            code: "482913".into(),
+            name: "公司电脑".into(),
+        };
+        let d = format!("{r:?}");
+        assert!(!d.contains("482913") && d.contains("公司电脑"), "{d}");
+        let v = Response::MeshInvite(InviteView {
+            id: 1,
+            code: "482913".into(),
+            expires_at: 9,
+        });
+        let d = format!("{v:?}");
+        assert!(!d.contains("482913") && d.contains("expires_at: 9"), "{d}");
+        assert_eq!(
+            serde_json::to_string(&v).unwrap(),
+            r#"{"MeshInvite":{"id":1,"code":"482913","expires_at":9}}"#,
+            "线上照常带着码"
+        );
+    }
+
     /// 留言正文不进 `Debug`：只报长度。
     #[test]
     fn a_mesh_send_request_does_not_print_its_text() {
diff --git a/src/ui/computers.rs b/src/ui/computers.rs
index 612c99c..c80d261 100644
--- a/src/ui/computers.rs
+++ b/src/ui/computers.rs
@@ -390,6 +390,8 @@ pub(crate) mod tests {
                 .collect(),
             joining: Vec::new(),
             messages: Default::default(),
+            invite: None,
+            invite_note: None,
         }
     }
 
diff --git a/tests/mesh_e2e.rs b/tests/mesh_e2e.rs
index 82aa284..0cf8bd5 100644
--- a/tests/mesh_e2e.rs
+++ b/tests/mesh_e2e.rs
@@ -1,5 +1,5 @@
-//! 多电脑第一步的本机端到端：一个真的中转、两个互不相干的守护进程，走完
-//! login → join（两边核对 6 位数）→ approve → peers → send。
+//! 多电脑的本机端到端：一个真的中转、两个互不相干的守护进程，走完
+//! A login → A invite（出 6 位邀请码）→ B join <码>（自动登录）→ peers → send。
 //!
 //! 默认不跑（`#[ignore]`），手动跑：
 //!
@@ -26,7 +26,7 @@
 //!
 //! # 断言
 //!
-//! 1. 两边屏幕上的 6 位数一致（新电脑看到的、已有电脑看到的是同一个）；
+//! 1. B 用 A 的邀请码进了 A 的组，A 那边记下「电脑B 已加入」、码作废；
 //! 2. B 的智能体会话里出现 `[来自 电脑A/终端 的留言 #xxxx]` 和正文；
 //! 3. 中转进程的全部输出里找不到留言正文。
 
@@ -362,15 +362,40 @@ fn two_computers_join_and_leave_a_message_over_a_real_relay() {
     let mut a = Machine::new("电脑A", &gateway, &relay, "sk-e2e-a");
     let mut b = Machine::new("电脑B", &gateway, &relay, "sk-e2e-b");
 
-    // ---- login ----
+    // ---- A 登录、出邀请码 ----
     let va = a.view(Request::MeshLogin);
     assert!(
         va.logged_in && va.in_group,
         "A 登录后该有一个只有自己的组：{va:?}"
     );
     assert_eq!(va.name, "电脑A");
-    let vb = b.view(Request::MeshLogin);
+    let invite = match a.call(Request::MeshInvite) {
+        Response::MeshInvite(v) => v,
+        other => panic!("预期 MeshInvite，实际 {other:?}"),
+    };
+    assert_eq!(invite.code.len(), 6);
+
+    // ---- B：dct join <码>，没登录过，自动登录 ----
+    // 两条连接刚起，中转那边未必已经把 A 登记上：B 问不到发邀请的电脑就再问
+    // （只探问不消耗码）；别的失败都是真失败。码里故意带个空格，同人敲的。
+    let typed = format!("{} {}", &invite.code[..3], &invite.code[3..]);
+    let vb = wait_until("B 用邀请码进了组", Duration::from_secs(30), || {
+        match b.call(Request::MeshJoin {
+            code: typed.clone(),
+            name: String::new(),
+        }) {
+            Response::Mesh(v) => Some(v),
+            Response::Error(dct::proto::ErrorCode::Mesh(
+                dct::proto::MeshProblem::NoInvite { .. },
+            )) => None,
+            other => panic!("B 加入失败：{other:?}"),
+        }
+    });
     assert!(vb.logged_in, "{vb:?}");
+    assert!(
+        vb.members.iter().any(|m| m.endpoint == va.endpoint),
+        "B 的名单上有 A：{vb:?}"
+    );
     {
         let seen = gateway_seen.lock().unwrap();
         assert_eq!(
@@ -379,58 +404,18 @@ fn two_computers_join_and_leave_a_message_over_a_real_relay() {
                 ("Bearer sk-e2e-a".to_string(), va.endpoint.clone()),
                 ("Bearer sk-e2e-b".to_string(), vb.endpoint.clone()),
             ],
-            "api_key 只走 Bearer，端点是各自的"
+            "api_key 只走 Bearer，端点是各自的；B 的登录是 join 顺手做的"
         );
     }
-
-    // ---- join：B 问在线的电脑，拿到 A 的 6 位数 ----
-    // 两条连接刚起，中转那边未必已经把两个端点都登记上：问到有人回话为止。
-    let joining = wait_until(
-        "B 的加入请求有人回话",
-        Duration::from_secs(30),
-        || match b.call(Request::MeshJoin {
-            name: String::new(),
-        }) {
-            Response::Mesh(v) if !v.joining.is_empty() => Some(v.joining),
-            _ => None,
-        },
-    );
-    assert_eq!(joining.len(), 1, "只有 A 一台在线：{joining:?}");
-    let inviter = &joining[0];
-    assert_eq!(inviter.name, "电脑A");
-    assert_eq!(inviter.endpoint, va.endpoint);
-    assert_eq!(inviter.code.len(), 6);
-
-    // A 这边挂着 B 的请求，屏幕上的数字跟 B 看到的一样——人就是比这个。
-    let pending = wait_until(
-        "A 看到 B 的加入请求",
-        Duration::from_secs(15),
-        || {
-            let v = a.view(Request::MeshStatus);
-            v.pending.into_iter().find(|p| p.endpoint == vb.endpoint)
-        },
-    );
-    assert_eq!(pending.name, "电脑B");
-    assert_eq!(pending.code, inviter.code, "两块屏幕上的 6 位数必须一样");
-
-    // B 上的人认定是 A 这一台；A 上的人核对过数字，点同意。
-    b.view(Request::MeshConfirmInviter {
-        endpoint: inviter.endpoint.clone(),
-    });
-    let after = a.view(Request::MeshApprove {
-        endpoint: pending.endpoint.clone(),
-        code: pending.code.clone(),
-        yes: true,
-    });
+    let after = a.view(Request::MeshStatus);
     assert!(after.members.iter().any(|m| m.endpoint == vb.endpoint));
-
-    wait_until("B 收到有 A 的名单", Duration::from_secs(15), || {
-        let v = b.view(Request::MeshStatus);
-        v.members
-            .iter()
-            .any(|m| m.endpoint == va.endpoint)
-            .then_some(())
-    });
+    assert!(after.invite.is_none(), "码用过就作废");
+    assert_eq!(
+        after.invite_note.map(|n| n.outcome),
+        Some(dct::proto::InviteOutcome::Joined {
+            name: "电脑B".into()
+        })
+    );
 
     // ---- B 开一个智能体会话 ----
     let repo = b.h.git_repo("proj");
```

- [ ] **Step 4: 跑测试确认通过**

Run: `~/.cargo/bin/cargo test --lib -- proto:: daemon::tests::mesh_ daemon::tests::a_restarted mesh::cli`
Expected: 全过（`the_request_shape_is_pinned_to_the_protocol_version`、`the_mesh_view_shape_is_pinned`、`the_cli_outwaits_the_daemon_on_every_mesh_request` 在内）。

- [ ] **Step 5: 端到端（真中转 + 两个真守护进程）**

Run: `~/.cargo/bin/cargo test --test mesh_e2e -- --ignored --nocapture`
Expected: `test two_computers_join_and_leave_a_message_over_a_real_relay ... ok`，输出里有 `B 的会话屏幕：` 和 `[来自 电脑A/终端 的留言 #`，末尾 `中转输出（… 字节）` 里没有留言原文。（写计划时实测 6.8 秒跑完。）

- [ ] **Step 6: 全量检查**

```bash
~/.cargo/bin/cargo test --workspace -- --test-threads=4
~/.cargo/bin/cargo clippy --workspace --all-targets -- -D warnings
~/.cargo/bin/cargo check --workspace --all-targets --target x86_64-pc-windows-msvc
```
Expected：`test result: ok.` 全绿（`pty::tests::*` / `session::tests::*` 偶发失败见 Global Constraints 的「已知 flaky」，单独重跑）；clippy `Finished`、无 warning；Windows check `Finished`（`src/student_projects.rs:669` 那条 `unused variable: path` 是本来就有的）。

- [ ] **Step 7: Commit**

```bash
git add src/proto.rs src/daemon.rs src/mesh/group.rs src/mesh/mod.rs src/mesh/cli.rs src/i18n.rs src/ui/computers.rs tests/mesh_e2e.rs
git commit -m "feat(mesh): dct join <code> through the daemon, protocol 24, e2e over a real relay" -m "Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---
