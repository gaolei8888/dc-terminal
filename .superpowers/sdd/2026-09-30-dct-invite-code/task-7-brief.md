### Task 7: 看板——按 `a` 出码、倒计时、结果提示

**Files:**
- Modify: `src/ui/computers.rs`（`MeshPanel` 加 `invite_rx`/`created`/`announced`；`start_invite`、`announce`、`invite_lines`；`poll` 收 `MeshInvite` 的答复——**顺手扔掉在飞的那次状态查询**，不然它会把刚画上的码盖掉；测试里的假守护进程会答 `MeshInvite`）
- Modify: `src/ui/board.rs`（`KeyCode::Char('a')` → `computers::start_invite`；`draw` 顶上先画 `invite_lines`，再画旧的确认行）
- Modify: `src/ui/keys.rs`（`?` 浮层配置组里，看板上列 `a 加电脑`）
- Modify: `src/i18n.rs`（`Key::AddComputer`（进 `ALL_KEYS`，`every_key_is_listed_for_the_guards` 里的 211 改 212）、`mesh_invite_burned_board`、`mesh_invite_expired_board`）

**Interfaces:**
- Consumes: Task 5 的 `Request::MeshInvite`、`Response::MeshInvite(InviteView)`、`MeshView.invite`/`invite_note`；Task 6 的 `msg::mesh_invite_code`/`mesh_invite_hint`/`mesh_invite_countdown`；已有的 `computers::spawn_call`、`say`、`wrap`、`theme_now().asking()`、`dim()`、`truncate`。
- Produces: `pub(crate) computers::start_invite(app: &mut App)`、`pub(crate) computers::invite_lines(panel: &MeshPanel, lang: Lang, width: usize, now: u64) -> Vec<Line<'static>>`、`Key::AddComputer`、`msg::mesh_invite_burned_board(lang)`、`msg::mesh_invite_expired_board(lang)`。

**行为**：`a` 只在看板视图接（会话视图里 `a` 归 agent，九宫格不接）；请求在飞时再按不发第二条；拿到码之后顶上一行黄字 `邀请码 482 913 · 10 分钟内有效 · 还剩 9:59`（窄屏整句折行），下一行灰字 `在新电脑上运行：dct join 482913（码只能用一次）`；到点不画；只对**这个界面要来的 id** 的结果说一句（已加入 / 「有人用错码试过一次，码已作废，按 a 重新生成」/「邀请码过期了，按 a 重新生成」），同一句只说一次；人在会话视图里不说。

- [ ] **Step 1: 写失败的测试**

`src/ui/computers.rs` 测试模块末尾：

```rust
    fn text_of(lines: &[Line]) -> String {
        lines
            .iter()
            .map(|l| l.spans.iter().map(|s| s.content.as_ref()).collect::<String>())
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// 看板上按 `a`：要一个码，拿到之后顶上画出来，带倒计时和新电脑上敲什么。
    #[test]
    fn a_on_the_board_gets_a_code_and_shows_it_with_a_countdown() {
        let (mut app, _d, got) = board_app(mesh_view(true, &[("家里Mac", true, true)], &[]));
        crate::ui::dispatch_key(&mut app, key('a')).unwrap();
        wait_until(&mut app, |a| {
            a.mesh.view.as_ref().is_some_and(|v| v.invite.is_some())
        });
        assert_eq!(invites(&got), 1);
        assert_eq!(app.mesh.created, Some(1));
        let shown = text_of(&invite_lines(&app.mesh, Lang::Zh, 80, INVITE_EXPIRES - 599));
        assert_eq!(
            shown,
            "邀请码 482 913 · 10 分钟内有效 · 还剩 9:59\n在新电脑上运行：dct join 482913（码只能用一次）"
        );
    }

    /// 连按两下 `a`：第一条还在飞，第二下不发；回来之后再按才换新码。
    #[test]
    fn pressing_a_twice_quickly_asks_once() {
        let (mut app, _d, got) = board_app(mesh_view(true, &[("家里Mac", true, true)], &[]));
        start_invite(&mut app);
        start_invite(&mut app);
        wait_until(&mut app, |a| a.mesh.invite_rx.is_none());
        std::thread::sleep(Duration::from_millis(100));
        assert_eq!(invites(&got), 1);
        start_invite(&mut app);
        wait_until(&mut app, |a| a.mesh.invite_rx.is_none());
        assert_eq!(invites(&got), 2);
        assert_eq!(app.mesh.created, Some(2), "以新码为准");
    }

    #[test]
    fn an_expired_code_is_not_drawn() {
        let mut v = mesh_view(true, &[("家里Mac", true, true)], &[]);
        v.invite = Some(crate::proto::InviteView {
            id: 1,
            code: "012345".into(),
            expires_at: 1_000,
        });
        let mut panel = MeshPanel::default();
        panel.set_view(v, Instant::now());
        assert!(!invite_lines(&panel, Lang::Zh, 80, 999).is_empty());
        assert!(text_of(&invite_lines(&panel, Lang::Zh, 80, 999)).contains("012 345"));
        assert!(invite_lines(&panel, Lang::Zh, 80, 1_000).is_empty());
    }

    /// 这个界面要来的码有了结果：底栏说一句，只说一次；别处要来的码的结果不说。
    #[test]
    fn the_outcome_of_our_own_code_is_said_once() {
        use crate::proto::{InviteNote, InviteOutcome::*};
        let cases = [
            (Joined { name: "公司电脑".into() }, "公司电脑 已加入"),
            (Burned, "有人用错码试过一次，码已作废，按 a 重新生成"),
            (Expired, "邀请码过期了，按 a 重新生成"),
        ];
        for (outcome, want) in cases {
            let mut v = mesh_view(true, &[("家里Mac", true, true)], &[]);
            v.invite_note = Some(InviteNote { id: 7, outcome });
            let (mut app, _d, _got) = board_app(mesh_view(true, &[("家里Mac", true, true)], &[]));
            app.mesh.created = Some(7);
            app.mesh.set_view(v.clone(), Instant::now());
            announce(&mut app);
            assert_eq!(app.message.text, want);
            app.message = "别的".into();
            announce(&mut app);
            assert_eq!(app.message.text, "别的", "同一句不说两遍");

            app.mesh.created = Some(8);
            app.mesh.announced = None;
            announce(&mut app);
            assert_eq!(app.message.text, "别的", "不是这个界面要的码");
        }
    }

    /// 会话视图里 `a` 归 agent：不要码。
    #[test]
    fn a_in_the_session_view_asks_for_no_code() {
        let (mut app, _d, got) = board_app(mesh_view(true, &[("家里Mac", true, true)], &[]));
        app.set_sessions(vec![crate::session::SessionInfo {
            id: 1,
            profile: "claude".into(),
            dir: "/tmp/a".into(),
            state: crate::session::SessionState::Idle,
            activity: String::new(),
            is_agent: true,
            tag: String::new(),
        }]);
        app.client = Some(crate::client::Client::connect(&app.socket).unwrap());
        app.view = View::Attached(1);
        crate::ui::dispatch_key(&mut app, key('a')).unwrap();
        std::thread::sleep(Duration::from_millis(200));
        poll(&mut app, Instant::now());
        assert_eq!(invites(&got), 0);
        assert!(app.mesh.invite_rx.is_none());
    }
```

- [ ] **Step 2: 跑测试确认失败**

Run: `~/.cargo/bin/cargo test --lib ui::computers`
Expected: 编译失败：`cannot find function `invite_lines``、`no field `created``。

- [ ] **Step 3: 实现**

先把 Step 1 里手改过的文件还原（补丁里已经包含同样的测试）：`git checkout -- src/ui/computers.rs`。

把下面整个补丁存成 `/tmp/dct-invite-t7.patch`（**只存 ```diff 块里的内容**），在 worktree 根目录应用：

```bash
git apply --check /tmp/dct-invite-t7.patch && git apply /tmp/dct-invite-t7.patch
```

`--check` 报错说明前面的任务跟本计划的代码有出入：改用 `git apply --3way`，还不行就照补丁逐块手改（上下文行用来定位），**不要**跳过任何一块。

```diff
diff --git a/src/i18n.rs b/src/i18n.rs
index b2aa579..9139571 100644
--- a/src/i18n.rs
+++ b/src/i18n.rs
@@ -106,6 +106,8 @@ pub enum Key {
     /// 的换项目手段；`Tab` 出现之后再写「换项目」，用户会以为按 `p` 能一步
     /// 换过去，而实际弹出来的是一个选择器。
     AddProject,
+    /// `a` 的说明：看板上要一个 6 位邀请码，给新电脑 `dct join` 用。
+    AddComputer,
     /// `1`…`9` 的说明：组头前面印着号码，按下去一步落到那个项目上。
     /// 跟 `Tab` 是同一件事的两种走法，所以两条都得写：`Tab` 是挨个翻，
     /// 数字是看见号码直接跳。
@@ -620,6 +622,7 @@ pub fn text(k: Key, lang: Lang) -> &'static str {
         // 「换项目」放得下，不必跟着缩。
         SwitchProject => t!(lang, en: "project", zh: "换项目"),
         AddProject => t!(lang, en: "add project", zh: "加项目"),
+        AddComputer => t!(lang, en: "add computer", zh: "加电脑"),
         GotoProject => t!(lang, en: "go to project", zh: "直达项目"),
         RemoveProject => t!(lang, en: "remove", zh: "移除"),
         ToggleCollapse => t!(lang, en: "fold", zh: "折叠"),
@@ -2453,6 +2456,24 @@ pub mod msg {
         )
     }
 
+    /// 看板底栏：这个界面要来的码被试错、作废了。
+    pub fn mesh_invite_burned_board(lang: Lang) -> String {
+        t!(
+            lang,
+            en: "Someone tried a wrong code once, so it is no longer valid. Press a for a new one".to_string(),
+            zh: "有人用错码试过一次，码已作废，按 a 重新生成".to_string(),
+        )
+    }
+
+    /// 看板底栏：这个界面要来的码过期了。
+    pub fn mesh_invite_expired_board(lang: Lang) -> String {
+        t!(
+            lang,
+            en: "The invite code expired. Press a for a new one".to_string(),
+            zh: "邀请码过期了，按 a 重新生成".to_string(),
+        )
+    }
+
     pub fn mesh_invite_expired(lang: Lang) -> String {
         t!(
             lang,
@@ -3066,6 +3087,7 @@ mod tests {
             SwitchAgent,
             SwitchProject,
             AddProject,
+            AddComputer,
             GotoProject,
             RemoveProject,
             ToggleCollapse,
@@ -3305,7 +3327,7 @@ mod tests {
     fn every_key_is_listed_for_the_guards() {
         // 这个数字改动时，请确认 ALL_KEYS 也补上了新变体——它不是凑出来的，
         // 而是「词条表里到底有多少条」这个事实。
-        assert_eq!(ALL_KEYS.len(), 211, "加了 Key 变体就要同步进 ALL_KEYS");
+        assert_eq!(ALL_KEYS.len(), 212, "加了 Key 变体就要同步进 ALL_KEYS");
         let mut seen: Vec<String> = ALL_KEYS.iter().map(|k| format!("{k:?}")).collect();
         seen.sort();
         let before = seen.len();
@@ -3435,6 +3457,8 @@ mod tests {
             msg::mesh_invite_countdown(Lang::En, 61),
             msg::mesh_invite_burned(Lang::En),
             msg::mesh_invite_expired(Lang::En),
+            msg::mesh_invite_burned_board(Lang::En),
+            msg::mesh_invite_expired_board(Lang::En),
             msg::mesh_invite_replaced(Lang::En),
             msg::mesh_invite_gone(Lang::En),
             msg::mesh_invite_usage(Lang::En),
diff --git a/src/ui/board.rs b/src/ui/board.rs
index 78840ec..0e0ec34 100644
--- a/src/ui/board.rs
+++ b/src/ui/board.rs
@@ -60,6 +60,8 @@ pub(crate) fn handle_key(app: &mut App, key: KeyEvent) -> Result<()> {
             let _ = super::unpin_current(app);
         }
         KeyCode::Char('c') if is_plain_key(&key) => open_secrets(app),
+        // 加电脑：要一个 6 位邀请码（不用 `i`：九宫格里 `i` 是开回复框）。
+        KeyCode::Char('a') if is_plain_key(&key) => super::computers::start_invite(app),
         // `l` = language。设置页跟 `c 密钥` 挨着：两个都是「配置」类入口，
         // 而且跟 g 一样，两个视图共用同一个键。
         KeyCode::Char('l') if is_plain_key(&key) => super::open_settings(app),
@@ -371,7 +373,12 @@ pub(crate) fn draw(f: &mut Frame, area: Rect, app: &mut App) {
     // 的数据（`App::mesh`），这里不发请求。
     let inner = block.inner(area);
     let width = inner.width as usize;
-    let prompt = super::computers::prompt_lines(&app.mesh, app.lang, width);
+    let now = std::time::SystemTime::now()
+        .duration_since(std::time::UNIX_EPOCH)
+        .map(|d| d.as_secs())
+        .unwrap_or(0);
+    let mut prompt = super::computers::invite_lines(&app.mesh, app.lang, width, now);
+    prompt.extend(super::computers::prompt_lines(&app.mesh, app.lang, width));
     let section = super::computers::section_lines(&app.mesh, app.lang, width);
     let (prompt_area, list_area, section_area) = split(inner, prompt.len(), section.len());
     f.render_widget(block, area);
diff --git a/src/ui/computers.rs b/src/ui/computers.rs
index c80d261..b0f7c22 100644
--- a/src/ui/computers.rs
+++ b/src/ui/computers.rs
@@ -17,7 +17,7 @@ use super::view::View;
 use super::widgets::{display_width, truncate, Msg};
 use super::{dim, theme_now};
 use crate::i18n::{msg, Lang};
-use crate::proto::{MemberView, MeshView, PendingJoin, Request, Response};
+use crate::proto::{InviteOutcome, MemberView, MeshView, PendingJoin, Request, Response};
 
 /// 多久问一次 `MeshStatus`。
 pub(crate) const POLL_EVERY: Duration = Duration::from_secs(5);
@@ -49,6 +49,13 @@ pub(crate) struct MeshPanel {
     /// 正在回答的那一条（端点、数字、同意与否）。回答飞着的时候确认行不画、
     /// y/n 不再接——免得连按两下，第二下落到下一条请求上。
     answering: Option<(PendingJoin, bool)>,
+    /// 按了 `a`、`MeshInvite` 还在飞。飞着的时候再按不再发第二条。
+    invite_rx: Option<Receiver<Reply>>,
+    /// 这个界面自己要来的最近一个码（`InviteView::id`）。只有它的结果才在
+    /// 底栏说一句——别处（`dct invite`）要的码，那边自己会说。
+    created: Option<u64>,
+    /// 已经说过的那个结果（`InviteNote::id`），同一句不说两遍。
+    announced: Option<u64>,
 }
 
 impl MeshPanel {
@@ -92,6 +99,35 @@ impl MeshPanel {
     }
 }
 
+/// 看板上按 `a`：要一个新码（旧的由守护进程作废）。上一条还在飞就不再发。
+pub(crate) fn start_invite(app: &mut App) {
+    if app.mesh.invite_rx.is_some() {
+        return;
+    }
+    app.mesh.invite_rx = Some(spawn_call(
+        app.socket.clone(),
+        Request::MeshInvite,
+        crate::client::READ_TIMEOUT * 2,
+    ));
+}
+
+/// 新拿到的现状里有这个界面要来的那个码的结果，还没说过：说一句。
+fn announce(app: &mut App) {
+    let Some(n) = app.mesh.view.as_ref().and_then(|v| v.invite_note.clone()) else {
+        return;
+    };
+    if app.mesh.created != Some(n.id) || app.mesh.announced == Some(n.id) {
+        return;
+    }
+    app.mesh.announced = Some(n.id);
+    let m = match &n.outcome {
+        InviteOutcome::Joined { name } => msg::mesh_member_joined(app.lang, &clean(name)).into(),
+        InviteOutcome::Burned => Msg::err(msg::mesh_invite_burned_board(app.lang)),
+        InviteOutcome::Expired => Msg::err(msg::mesh_invite_expired_board(app.lang)),
+    };
+    say(app, m);
+}
+
 /// 在后台线程里发一条请求，结果从返回的通道里取。
 fn spawn_call(socket: PathBuf, req: Request, timeout: Duration) -> Receiver<Reply> {
     let (tx, rx) = mpsc::channel();
@@ -108,6 +144,35 @@ fn spawn_call(socket: PathBuf, req: Request, timeout: Duration) -> Receiver<Repl
 /// 只在看板和九宫格上问（九宫格上有人想加入时要提醒一句）——别的视图不画
 /// 这一块。
 pub(crate) fn poll(app: &mut App, now: Instant) {
+    if let Some(rx) = &app.mesh.invite_rx {
+        match rx.try_recv() {
+            Ok(r) => {
+                app.mesh.invite_rx = None;
+                match r {
+                    Ok(Response::MeshInvite(v)) => {
+                        app.mesh.created = Some(v.id);
+                        if let Some(view) = app.mesh.view.as_mut() {
+                            view.invite = Some(v);
+                        }
+                        // 要码之前发出去的那次状态查询，回来的是没有这个码的
+                        // 样子，扔掉；下一轮就重新问，别等 5 秒。
+                        app.mesh.status_rx = None;
+                        app.mesh.last_fetch = None;
+                    }
+                    Ok(Response::Error(e)) => say(app, Msg::err(msg::error(app.lang, &e))),
+                    _ => say(
+                        app,
+                        Msg::err(msg::error(
+                            app.lang,
+                            &crate::proto::ErrorCode::DaemonNotResponding,
+                        )),
+                    ),
+                }
+            }
+            Err(mpsc::TryRecvError::Empty) => {}
+            Err(mpsc::TryRecvError::Disconnected) => app.mesh.invite_rx = None,
+        }
+    }
     if let Some(rx) = &app.mesh.answer_rx {
         if let Ok(r) = rx.try_recv() {
             app.mesh.answer_rx = None;
@@ -152,6 +217,7 @@ pub(crate) fn poll(app: &mut App, now: Instant) {
                 // 问不到（断线、守护进程太老不认识这条）就留着手里那份。
                 if let Ok(Response::Mesh(v)) = r {
                     app.mesh.set_view(v, now);
+                    announce(app);
                 }
             }
             Err(mpsc::TryRecvError::Empty) => {}
@@ -271,6 +337,36 @@ fn wrap(s: &str, width: usize) -> Vec<String> {
     out
 }
 
+/// 顶部的邀请码：`邀请码 482 913 · 10 分钟内有效 · 还剩 9:41`（黄字，整句
+/// 折行），下面一行灰字说新电脑上敲什么。`now` 是 unix 秒；到点了就不画
+/// （现状最多晚 5 秒才知道它过期）。
+pub(crate) fn invite_lines(panel: &MeshPanel, lang: Lang, width: usize, now: u64) -> Vec<Line<'static>> {
+    let Some(v) = panel.view.as_ref().and_then(|v| v.invite.as_ref()) else {
+        return Vec::new();
+    };
+    if now >= v.expires_at || v.code.len() != 6 {
+        return Vec::new();
+    }
+    let spaced = format!("{} {}", &v.code[..3], &v.code[3..]);
+    let line = format!(
+        "{} · {}",
+        msg::mesh_invite_code(lang, &spaced),
+        msg::mesh_invite_countdown(lang, v.expires_at - now)
+    );
+    let style = theme_now().asking().add_modifier(Modifier::BOLD);
+    let mut lines: Vec<Line> = wrap(&line, width)
+        .into_iter()
+        .map(|l| Line::from(Span::styled(l, style)))
+        .collect();
+    if width > 1 {
+        lines.push(Line::from(Span::styled(
+            truncate(&msg::mesh_invite_hint(lang, &v.code), width.saturating_sub(1)),
+            dim(),
+        )));
+    }
+    lines
+}
+
 /// 顶部那几行：黄字的加入确认（整句折行，不截——数字和 (y/n) 一个都不能
 /// 被切掉），后面跟「还有 N 条」；下面一行灰字提醒一次只加一台（放不下
 /// 就截短，它只是提醒）。
@@ -424,6 +520,20 @@ pub(crate) mod tests {
                                 v.pending.retain(|p| &p.endpoint != endpoint);
                                 Response::Mesh(v)
                             }
+                            // 第几次要码，`id` 就是几；码固定。
+                            Request::MeshInvite => {
+                                let n = got
+                                    .lock()
+                                    .unwrap()
+                                    .iter()
+                                    .filter(|r| matches!(r, Request::MeshInvite))
+                                    .count() as u64;
+                                Response::MeshInvite(crate::proto::InviteView {
+                                    id: n + 1,
+                                    code: "482913".into(),
+                                    expires_at: INVITE_EXPIRES,
+                                })
+                            }
                             _ => Response::Ok,
                         };
                         got.lock().unwrap().push(req);
@@ -438,6 +548,17 @@ pub(crate) mod tests {
         got
     }
 
+    /// 假守护进程发的码在这一刻（unix 秒）过期。
+    const INVITE_EXPIRES: u64 = 1_800_000_600;
+
+    fn invites(got: &Arc<Mutex<Vec<Request>>>) -> usize {
+        got.lock()
+            .unwrap()
+            .iter()
+            .filter(|r| matches!(r, Request::MeshInvite))
+            .count()
+    }
+
     fn approvals(got: &Arc<Mutex<Vec<Request>>>) -> Vec<(String, String, bool)> {
         got.lock()
             .unwrap()
@@ -896,4 +1017,109 @@ pub(crate) mod tests {
         assert_eq!(wrap("abc", 0), Vec::<String>::new());
         assert_eq!(wrap("一", 1), ["一"], "一个字都放不下也不能死循环");
     }
+
+    fn text_of(lines: &[Line]) -> String {
+        lines
+            .iter()
+            .map(|l| l.spans.iter().map(|s| s.content.as_ref()).collect::<String>())
+            .collect::<Vec<_>>()
+            .join("\n")
+    }
+
+    /// 看板上按 `a`：要一个码，拿到之后顶上画出来，带倒计时和新电脑上敲什么。
+    #[test]
+    fn a_on_the_board_gets_a_code_and_shows_it_with_a_countdown() {
+        let (mut app, _d, got) = board_app(mesh_view(true, &[("家里Mac", true, true)], &[]));
+        crate::ui::dispatch_key(&mut app, key('a')).unwrap();
+        wait_until(&mut app, |a| {
+            a.mesh.view.as_ref().is_some_and(|v| v.invite.is_some())
+        });
+        assert_eq!(invites(&got), 1);
+        assert_eq!(app.mesh.created, Some(1));
+        let shown = text_of(&invite_lines(&app.mesh, Lang::Zh, 80, INVITE_EXPIRES - 599));
+        assert_eq!(
+            shown,
+            "邀请码 482 913 · 10 分钟内有效 · 还剩 9:59\n在新电脑上运行：dct join 482913（码只能用一次）"
+        );
+    }
+
+    /// 连按两下 `a`：第一条还在飞，第二下不发；回来之后再按才换新码。
+    #[test]
+    fn pressing_a_twice_quickly_asks_once() {
+        let (mut app, _d, got) = board_app(mesh_view(true, &[("家里Mac", true, true)], &[]));
+        start_invite(&mut app);
+        start_invite(&mut app);
+        wait_until(&mut app, |a| a.mesh.invite_rx.is_none());
+        std::thread::sleep(Duration::from_millis(100));
+        assert_eq!(invites(&got), 1);
+        start_invite(&mut app);
+        wait_until(&mut app, |a| a.mesh.invite_rx.is_none());
+        assert_eq!(invites(&got), 2);
+        assert_eq!(app.mesh.created, Some(2), "以新码为准");
+    }
+
+    #[test]
+    fn an_expired_code_is_not_drawn() {
+        let mut v = mesh_view(true, &[("家里Mac", true, true)], &[]);
+        v.invite = Some(crate::proto::InviteView {
+            id: 1,
+            code: "012345".into(),
+            expires_at: 1_000,
+        });
+        let mut panel = MeshPanel::default();
+        panel.set_view(v, Instant::now());
+        assert!(!invite_lines(&panel, Lang::Zh, 80, 999).is_empty());
+        assert!(text_of(&invite_lines(&panel, Lang::Zh, 80, 999)).contains("012 345"));
+        assert!(invite_lines(&panel, Lang::Zh, 80, 1_000).is_empty());
+    }
+
+    /// 这个界面要来的码有了结果：底栏说一句，只说一次；别处要来的码的结果不说。
+    #[test]
+    fn the_outcome_of_our_own_code_is_said_once() {
+        use crate::proto::{InviteNote, InviteOutcome::*};
+        let cases = [
+            (Joined { name: "公司电脑".into() }, "公司电脑 已加入"),
+            (Burned, "有人用错码试过一次，码已作废，按 a 重新生成"),
+            (Expired, "邀请码过期了，按 a 重新生成"),
+        ];
+        for (outcome, want) in cases {
+            let mut v = mesh_view(true, &[("家里Mac", true, true)], &[]);
+            v.invite_note = Some(InviteNote { id: 7, outcome });
+            let (mut app, _d, _got) = board_app(mesh_view(true, &[("家里Mac", true, true)], &[]));
+            app.mesh.created = Some(7);
+            app.mesh.set_view(v.clone(), Instant::now());
+            announce(&mut app);
+            assert_eq!(app.message.text, want);
+            app.message = "别的".into();
+            announce(&mut app);
+            assert_eq!(app.message.text, "别的", "同一句不说两遍");
+
+            app.mesh.created = Some(8);
+            app.mesh.announced = None;
+            announce(&mut app);
+            assert_eq!(app.message.text, "别的", "不是这个界面要的码");
+        }
+    }
+
+    /// 会话视图里 `a` 归 agent：不要码。
+    #[test]
+    fn a_in_the_session_view_asks_for_no_code() {
+        let (mut app, _d, got) = board_app(mesh_view(true, &[("家里Mac", true, true)], &[]));
+        app.set_sessions(vec![crate::session::SessionInfo {
+            id: 1,
+            profile: "claude".into(),
+            dir: "/tmp/a".into(),
+            state: crate::session::SessionState::Idle,
+            activity: String::new(),
+            is_agent: true,
+            tag: String::new(),
+        }]);
+        app.client = Some(crate::client::Client::connect(&app.socket).unwrap());
+        app.view = View::Attached(1);
+        crate::ui::dispatch_key(&mut app, key('a')).unwrap();
+        std::thread::sleep(Duration::from_millis(200));
+        poll(&mut app, Instant::now());
+        assert_eq!(invites(&got), 0);
+        assert!(app.mesh.invite_rx.is_none());
+    }
 }
diff --git a/src/ui/keys.rs b/src/ui/keys.rs
index b603601..1d023d7 100644
--- a/src/ui/keys.rs
+++ b/src/ui/keys.rs
@@ -135,6 +135,10 @@ fn groups(from: &View, ctx: HelpCtx, lang: Lang) -> Vec<Group> {
                 if ctx.can_remove {
                     v.extend(help_items(&[("x", Key::RemoveProject)], lang));
                 }
+                // `a` 只绑在看板上（九宫格里不接，那边不画邀请码）。
+                if !in_grid {
+                    v.extend(help_items(&[("a", Key::AddComputer)], lang));
+                }
                 v.extend(help_items(
                     &[
                         ("c", Key::Secrets),
```

- [ ] **Step 4: 跑测试确认通过**

Run: `~/.cargo/bin/cargo test --lib -- ui:: i18n`
Expected: 全过（`every_key_is_listed_for_the_guards` 在内）。再连跑 5 遍 `~/.cargo/bin/cargo test --lib -- ui::computers mesh::invite`，5 遍都绿（新测试有后台线程，确认不 flaky）。

- [ ] **Step 5: 全量检查**

```bash
~/.cargo/bin/cargo test --workspace -- --test-threads=4
~/.cargo/bin/cargo clippy --workspace --all-targets -- -D warnings
~/.cargo/bin/cargo check --workspace --all-targets --target x86_64-pc-windows-msvc
```
Expected：`test result: ok.` 全绿（`pty::tests::*` / `session::tests::*` 偶发失败见 Global Constraints 的「已知 flaky」，单独重跑）；clippy `Finished`、无 warning；Windows check `Finished`（`src/student_projects.rs:669` 那条 `unused variable: path` 是本来就有的）。

- [ ] **Step 6: Commit**

```bash
git add src/ui/computers.rs src/ui/board.rs src/ui/keys.rs src/i18n.rs
git commit -m "feat(ui): press a on the board for an invite code, with a countdown and the outcome" -m "Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---
