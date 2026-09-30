### Task 8: 删掉旧流程（dct-sas-v2 核对 + 批准）

**Files:**
- Delete: `crates/dct-mesh/src/sas.rs`（`git rm`）
- Modify: `crates/dct-mesh/src/lib.rs`（去掉 `pub mod sas;`、改模块文档）、`wire.rs`（删 `Join(JoinRequest)`/`JoinPending`/`JoinReveal`、`JoinRequest`、`encode32`/`decode32` 和它们的测试；模块文档改成邀请码三步）、`roster.rs`/`keys.rs`（文档里的旧名字）。**`rec` 已经在 `invite.rs`，`sign_member`/`verify_member`（`dct-join-v1`）留在 `wire.rs`，`accept_invite` 留在 `roster.rs`——这三样新流程还在用。**
- Modify: `src/mesh/mod.rs`（删 `JOIN_TTL`、`MAX_PENDING`、`MAX_CODES_PER_TTL`、`PendingReq`、旧的 B 端 `Invite`、字段 `pending_joins`/`codes_shown`/`invites`/`confirmed`/`held`、`join_request`/`prune_pending`/`take_join`/`take_reveal`/`set_invites`/`prune_invites`/`confirm_inviter`/`take_invite`；`take_roster` 只剩「已有名单 + `accept`」一条路；`fresh_nonce` 返回 `[u8; 32]`；`worst_case` 去掉两条旧请求）
- Replace: `src/mesh/group.rs`（整份换成下面的；去掉 `JOIN_ASK_TIMEOUT`、`join`、`ask_all`、`confirm`、`approve`、`joining`；测试模块重写，`ab()` 改成用邀请码拉人）
- Modify: `src/proto.rs`（删 `MeshApprove`、`MeshConfirmInviter`、`PendingJoin`、`MeshView.pending`/`joining`，`MeshProblem` 删 `NoOneAnswered`/`NoSuchRequest`/`Ambiguous`/`NameTaken`/`NoSuchInviter`/`CodeMismatch`；两条形状守卫改成新形状，**协议号仍是 24**，版本注释写明）
- Modify: `src/daemon.rs`（删两条请求的处理和分派）
- Modify: `src/mesh/cli.rs`（删 `peers approve`、`pick`、`Ask` 和整条 `ask` 参数链、`dct peers` 里的待批准列表；测试相应改）
- Modify: `src/main.rs`（帮助文字删 `dct peers approve` 那行和「谁在等批准」）
- Replace: `src/ui/computers.rs`（整份换成下面的；删 `SETTLE`、确认行、y/n、`grid_notice_lines`、`prompt_lines`；`set_view` 只剩一个参数；测试里 `mesh_view` 帮手去掉 `pending` 参数）
- Modify: `src/ui/board.rs`（不再先交给 `computers::handle_key`；只画 `invite_lines`；三条确认行的画面测试换成两条码的画面测试）、`src/ui/grid.rs`（删顶上那条「有电脑想加入」的提醒）
- Modify: `src/i18n.rs`（删 17 个再没人用的 `msg::` 函数和 6 个 `MeshProblem` 分支；`TooManyAnswered`、`mesh_peers_usage` 改写）

**Interfaces:**
- Consumes: Task 3–7 的全部。
- Produces（删掉的，后面谁都不许再用）：`dct_mesh::sas`、`wire::{JoinRequest, encode32, decode32}`、`Payload::{Join, JoinPending, JoinReveal}`、`Request::{MeshApprove, MeshConfirmInviter}`、`proto::PendingJoin`、`MeshView.{pending, joining}`、`group::{join, confirm, approve, JOIN_ASK_TIMEOUT}`、`mesh::{JOIN_TTL, PendingReq, Invite}`、`computers::{SETTLE, handle_key, prompt_lines, grid_notice_lines}`。
- 保留并改签名：`Mesh::fresh_nonce(&self) -> [u8; 32]`、`MeshPanel::set_view(&mut self, v: MeshView)`、测试帮手 `computers::tests::mesh_view(logged_in: bool, members: &[(&str, bool, bool)]) -> MeshView`。

- [ ] **Step 1: 写失败的测试**

`src/mesh/cli.rs` 测试模块里，把 `peers_lists_members_and_asks_about_pending_joins` 换成下面两条：

```rust
    /// 详情问不成（旧守护进程、出错）：退回名单那几行；不再有「谁在等批准」。
    #[test]
    fn peers_falls_back_to_the_member_lines() {
        let v = view(true, &[("B", true), ("A", false)]);
        let sc = Script::new(vec![
            Response::Mesh(v),
            Response::Error(ErrorCode::DaemonNotResponding),
        ]);
        let (mut out, mut err) = (vec![], vec![]);
        assert_eq!(peers(&mut sc.call(), &mut out, &mut err, Lang::Zh, &[]), 0);
        let o = s(&out);
        assert!(o.contains("  B  (这台)"), "{o}");
        assert!(o.contains("  A  (在线)"), "{o}");
        assert!(!o.contains("approve"), "{o}");
    }

    /// `dct peers approve` 没有了：当成用法不对。
    #[test]
    fn peers_approve_is_gone() {
        let sc = Script::new(vec![]);
        let (mut out, mut err) = (vec![], vec![]);
        let code = peers(&mut sc.call(), &mut out, &mut err, Lang::Zh, &args(&["approve", "B"]));
        assert_eq!(code, 2);
        assert_eq!(s(&err).trim(), "用法：dct peers | dct peers remove <电脑名>");
    }
```

`src/ui/board.rs` 测试模块里，把 `a_pending_join_shows_the_confirm_line_under_the_title`、`several_pending_joins_are_asked_one_at_a_time`、`narrow_and_tiny_terminals_keep_the_code_and_never_panic` 三条换成（这两条里的 `mesh_view` 用的是 Step 3 之后的两参数形式）：

```rust
    /// 发着邀请码：标题下面一行黄字（码、有效期、倒计时），再一行灰字说
    /// 新电脑上敲什么；会话行被挤到下面。
    fn with_code() -> crate::proto::MeshView {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();
        let mut v = mesh_view(
            true,
            &[
                ("家里Mac", true, true),
                ("一个特别特别长的电脑名字啊啊啊", false, false),
            ],
        );
        v.invite = Some(crate::proto::InviteView {
            id: 1,
            code: "482913".into(),
            expires_at: now + 300,
        });
        v
    }

    #[test]
    fn a_live_code_shows_under_the_title_with_the_hint() {
        let term = draw_mesh(Some(with_code()), 80, 12);
        let r = rows(&term);
        assert!(r[1].starts_with("邀请码482913·10分钟内有效·还剩"), "{r:?}");
        assert_eq!(r[2], "在新电脑上运行：dctjoin482913（码只能用一次）");
        let buf = term.backend().buffer();
        assert_eq!(
            buf.cell((0, 1)).unwrap().fg,
            crate::ui::theme_now().asking().fg.unwrap(),
            "码那一行是「等你拍板」那一档暖色"
        );
        assert!(r.iter().any(|l| l.contains("proj")));
    }

    /// 窄终端上折行，码一位都不能丢；矮到放不下时先保码；再窄再矮也不 panic。
    #[test]
    fn narrow_and_tiny_terminals_keep_the_code_and_never_panic() {
        let v = with_code();
        let c: String = rows(&draw_mesh(Some(v.clone()), 30, 20)).concat();
        assert!(c.contains("482913"), "{c}");
        let r = rows(&draw_mesh(Some(v.clone()), 80, 6));
        assert!(r[1].contains("482913"), "{r:?}");
        assert!(r[3].contains("proj"), "列表还剩一行：{r:?}");
        for (w, h) in [(80, 24), (30, 10), (30, 3), (10, 5), (1, 1), (2, 2), (0, 0)] {
            draw_mesh(Some(v.clone()), w, h);
            draw_mesh(Some(mesh_view(false, &[])), w, h);
        }
    }
```

- [ ] **Step 2: 跑测试确认失败**

Run: `~/.cargo/bin/cargo test --lib -- mesh::cli ui::board`
Expected: `peers_approve_is_gone` 失败（`approve` 还被当成子命令，退出码不是 2）；`ui::board` 的新测试编译失败（`mesh_view` 还要三个参数）。

- [ ] **Step 3: 实现——补丁部分**

先把 Step 1 里手改过的文件还原（补丁里已经包含同样的测试）：`git checkout -- src/mesh/cli.rs src/ui/board.rs`。

把下面整个补丁存成 `/tmp/dct-invite-t8.patch`（**只存 ```diff 块里的内容**），在 worktree 根目录应用：

```bash
git apply --check /tmp/dct-invite-t8.patch && git apply /tmp/dct-invite-t8.patch
```

`--check` 报错说明前面的任务跟本计划的代码有出入：改用 `git apply --3way`，还不行就照补丁逐块手改（上下文行用来定位），**不要**跳过任何一块。

```diff
diff --git a/crates/dct-mesh/src/keys.rs b/crates/dct-mesh/src/keys.rs
index 668a7cc..9022fcb 100644
--- a/crates/dct-mesh/src/keys.rs
+++ b/crates/dct-mesh/src/keys.rs
@@ -60,7 +60,7 @@ impl MachineKeys {
     /// （"low-S" 和 "high-S"）对同一条消息、同一把私钥都是合法签名，这个
     /// 函数不保证只产出其中一种。谁都能拿一个合法签名算出另一个同样合法、
     /// 但字节不同的签名，不需要私钥。所以这里返回的字节，以及所有拿它当
-    /// 字段用的地方（`SignedRoster::sig`、`wire::JoinRequest::sig`、
+    /// 字段用的地方（`SignedRoster::sig`、`wire::sign_member` 的结果、
     /// `relay_token` 令牌里的 `sig`……）**都不能被当成 id 或去重键**——同一
     /// 条逻辑上的消息可能对应两种不同的签名字节。真要去重/防重放，认
     /// 消息自己的字段（比如 `seal::Message::id`），不要认签名或整个信封。
diff --git a/crates/dct-mesh/src/lib.rs b/crates/dct-mesh/src/lib.rs
index f0fc119..589e590 100644
--- a/crates/dct-mesh/src/lib.rs
+++ b/crates/dct-mesh/src/lib.rs
@@ -1,6 +1,6 @@
-//! 多机之间互相认识需要的纯逻辑：机器 id 怎么从公钥算出来、配对时两台电脑
-//! 各自读出的 6 位核对码、一台电脑的签名/加密钥匙对、以及组名单（谁在组
-//! 里、各自的公钥）怎么签名和怎么校验一份新名单能不能取代旧的。
+//! 多机之间互相认识需要的纯逻辑：机器 id 怎么从公钥算出来、加电脑时用的
+//! 6 位邀请码（SPAKE2，`invite`）、一台电脑的签名/加密钥匙对、以及组名单
+//! （谁在组里、各自的公钥）怎么签名和怎么校验一份新名单能不能取代旧的。
 //!
 //! **这个 crate 不碰网络、磁盘、环境变量，也不自己生成随机数**——种子、要签的
 //! 消息、收到的名单，全部由调用方（`dct` 里的 `mesh` 模块）传进来。这样它才能
@@ -11,7 +11,6 @@ pub mod invite;
 pub mod keys;
 pub mod relay_token;
 pub mod roster;
-pub mod sas;
 pub mod seal;
 pub mod wire;
 
diff --git a/crates/dct-mesh/src/roster.rs b/crates/dct-mesh/src/roster.rs
index 5081760..647a4ff 100644
--- a/crates/dct-mesh/src/roster.rs
+++ b/crates/dct-mesh/src/roster.rs
@@ -352,9 +352,9 @@ pub fn accept(current: Option<&SignedRoster>, incoming: &SignedRoster) -> Result
 /// 一台**加入**别人组的电脑，接受它的第一份名单。
 ///
 /// 这台电脑此刻没有（别人的）旧名单可以对照，`accept` 那条「签名者在上一版
-/// 里」用不上。取而代之的是：签名者必须就是 `inviter`——那台回过我
-/// `JoinPending`、我给它算过 6 位数、用户两边核对过的电脑，验签也只用
-/// `inviter` 自己那把 `sign_pub`（中转伪造不了：换一把钥匙，数字就对不上）。
+/// 里」用不上。取而代之的是：签名者必须就是 `inviter`——跟我用邀请码走完
+/// SPAKE2、身份（`invite::rec`）绑进了确认值的那台电脑，验签也只用 `inviter`
+/// 自己那把 `sign_pub`（中转伪造不了：换一把钥匙，确认值就对不上）。
 ///
 /// 规则：
 /// - 名单里每个成员的结构性校验同 `accept`（名字、公钥、`endpoint` 绑钥匙），
@@ -362,7 +362,7 @@ pub fn accept(current: Option<&SignedRoster>, incoming: &SignedRoster) -> Result
 /// - `inviter.endpoint` 真的是从 `inviter.sign_pub` 算出来的；
 /// - `signer == inviter.endpoint`，签名用 `inviter.sign_pub` 验得过；
 /// - 名单里有 `inviter`，而且那一条的两把公钥跟 `inviter` 的一模一样——
-///   6 位数核对的是这两把，名单里换成别的就等于没核对过；
+///   SPAKE2 绑定的是这两把，名单里换成别的就等于没核对过；
 /// - 名单里有我（`me_endpoint`）。
 ///
 /// 版本号和组名不看：这是我的第一份，没有东西可比。我自己那一条的加密公钥
@@ -406,8 +406,8 @@ mod tests {
 
     // -- accept_invite: a joining machine's first roster -----------------------
 
-    /// A 的组 v2 = {A, X}，A 签了一份 v3 把 B 加进来。B 手上只有 A 的
-    /// `JoinPending` 里那条成员记录（B 为它算过 6 位数）。
+    /// A 的组 v2 = {A, X}，A 签了一份 v3 把 B 加进来。B 手上只有 A 在
+    /// `InviteOpen` 里给的那条成员记录（SPAKE2 绑定的就是它）。
     fn invite_fixture() -> (MachineKeys, Member, Member, SignedRoster) {
         let ka = keys_for(1);
         let kb = keys_for(2);
@@ -495,8 +495,8 @@ mod tests {
         );
     }
 
-    /// 名单里 A 那一条的加密公钥被换了：B 核对的 6 位数是按 A 在
-    /// `JoinPending` 里给的那把算的，名单里的这把没人核对过。
+    /// 名单里 A 那一条的加密公钥被换了：SPAKE2 绑定的是 A 在 `InviteOpen`
+    /// 里给的那把，名单里的这把没人核对过。
     #[test]
     fn an_invite_that_lists_the_inviter_with_different_keys_is_refused() {
         let (ka, a, b, _) = invite_fixture();
diff --git a/crates/dct-mesh/src/wire.rs b/crates/dct-mesh/src/wire.rs
index aa728d0..4d3f52d 100644
--- a/crates/dct-mesh/src/wire.rs
+++ b/crates/dct-mesh/src/wire.rs
@@ -1,14 +1,13 @@
 //! 中转（relay）上跑的消息线上形状：一台电脑发给中转的每一条 JSON 都是一个
-//! `Payload`，`t` 字段说明它是密封留言、加入请求、整份名单、「你的加入申请
-//! 收到了，这是我的随机数」，还是加入方揭晓自己的随机数。
+//! `Payload`，`t` 字段说明它是密封留言、整份名单，还是用邀请码加电脑的那几步。
 //!
-//! 加入的三步（先承诺、再揭晓，见 `sas` 模块头）：
+//! 用邀请码加电脑（dct-invite-v1，见 `invite` 模块头），全是新电脑 B 问、
+//! 老电脑 A 当场答的 `ask`：
 //!
-//! 1. 加入方 → 邀请方：`Join`（自签的成员记录 + 承诺，`ask`）；
-//! 2. 邀请方 → 加入方：`JoinPending`（自签的成员记录 + 新鲜随机数，`ask` 的答复）；
-//! 3. 加入方 → 邀请方：`JoinReveal`（自己的随机数，`send`）。
-//!
-//! 两边都要到第 3 步之后才有数字可亮。
+//! 1. `InviteProbe` → `InviteOpen`（A 自签的记录 + 组 id）或 `NoInvite`；
+//! 2. `InviteJoin`（B 自签的记录 + SPAKE2 消息）→ `InviteKey`（A 的 SPAKE2 消息 + `cA`）
+//!    或 `InviteFailed`；
+//! 3. `InviteFinish`（`cB`）→ `InviteDone`（新名单）或 `InviteFailed`。
 use crate::canon::field;
 use crate::id;
 use crate::keys::{self, MachineKeys};
@@ -23,20 +22,7 @@ pub const JOIN_VERSION_TAG: &str = "dct-join-v1";
 #[serde(tag = "t", rename_all = "snake_case")]
 pub enum Payload {
     Sealed(Sealed),
-    Join(JoinRequest),
     Roster(SignedRoster),
-    /// 邀请方的答复：自签的成员记录，和它为这一次加入新出的随机数
-    /// （32 字节，标准 base64）。每收到一次 `Join` 就换一个。
-    JoinPending {
-        member: Member,
-        sig: String,
-        nonce: String,
-    },
-    /// 加入方揭晓 `Join` 里承诺过的随机数（32 字节，标准 base64）。发件
-    /// 端点由中转认证，邀请方按它找到那一条请求。
-    JoinReveal {
-        nonce: String,
-    },
 
     // —— 邀请码（dct-invite-v1，见 `invite` 模块头）。全走 `ask`：B 问，A 当场答。
 
@@ -76,30 +62,6 @@ pub enum Payload {
     InviteFailed,
 }
 
-/// 新电脑请求加入组：`member` 是它自己的名单条目（还没被任何人签认），
-/// `sig` 是它用自己的钥匙对 `member` 的 `dct-join-v1` 规范字节签的名——证明
-/// 它真的掌握 `member.sign_pub` 对应的私钥。ECDSA 签名可延展（见
-/// `keys::MachineKeys::sign` 的文档），`sig` 不能当 id 或去重键用。
-#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
-pub struct JoinRequest {
-    pub member: Member,
-    pub sig: String,
-    /// `sas::commit(member, 我的随机数)`，32 字节，标准 base64。随机数本身
-    /// 等拿到邀请方的随机数之后才在 `JoinReveal` 里给。`sig` 不覆盖它：换掉
-    /// 承诺的人拿不出能打开它的随机数，换了也只是让这一次加入对不上。
-    pub commit: String,
-}
-
-/// 32 字节（随机数、承诺）写成标准 base64。
-pub fn encode32(b: &[u8; 32]) -> String {
-    STANDARD.encode(b)
-}
-
-/// `encode32` 的反面。不是恰好 32 字节的标准 base64 就是 `None`。
-pub fn decode32(s: &str) -> Option<[u8; 32]> {
-    STANDARD.decode(s).ok()?.try_into().ok()
-}
-
 /// 任意字节（SPAKE2 消息、确认值）写成标准 base64。
 pub fn encode_bytes(b: &[u8]) -> String {
     STANDARD.encode(b)
@@ -119,9 +81,10 @@ pub fn decode(b: &[u8]) -> Result<Payload, serde_json::Error> {
 }
 
 /// `dct-join-v1` 规范字节：依次写 `field("dct-join-v1")`、`name`、
-/// `endpoint`、`sign_pub`、`kx_pub`、`added_at`。`JoinRequest` 和
-/// `JoinPending` 都是「一台电脑对自己的 `Member` 记录自签名」，共用这一份
-/// 编码和下面这两个函数。
+/// `endpoint`、`sign_pub`、`kx_pub`、`added_at`。`InviteOpen` 和
+/// `InviteJoin` 都是「一台电脑对自己的 `Member` 记录自签名」，共用这一份
+/// 编码和下面这两个函数。ECDSA 签名可延展（见 `keys::MachineKeys::sign`），
+/// `sig` 不能当 id 或去重键用。
 fn member_bytes(m: &Member) -> Vec<u8> {
     let mut out = Vec::new();
     field(&mut out, JOIN_VERSION_TAG);
@@ -195,15 +158,6 @@ mod tests {
             .unwrap()
             .contains("\"t\":\"sealed\""));
 
-        let join = Payload::Join(JoinRequest {
-            member: m.clone(),
-            sig: "sig".into(),
-            commit: "c".into(),
-        });
-        assert!(serde_json::to_string(&join)
-            .unwrap()
-            .contains("\"t\":\"join\""));
-
         let roster = Payload::Roster(SignedRoster {
             roster: Roster {
                 group: "g".into(),
@@ -216,21 +170,6 @@ mod tests {
         assert!(serde_json::to_string(&roster)
             .unwrap()
             .contains("\"t\":\"roster\""));
-
-        let pending = Payload::JoinPending {
-            member: m,
-            sig: "sig".into(),
-            nonce: "n".into(),
-        };
-        assert!(serde_json::to_string(&pending)
-            .unwrap()
-            .contains("\"t\":\"join_pending\""));
-
-        let reveal = Payload::JoinReveal { nonce: "n".into() };
-        assert_eq!(
-            serde_json::to_string(&reveal).unwrap(),
-            r#"{"t":"join_reveal","nonce":"n"}"#
-        );
     }
 
     /// 邀请码那 8 种的线上形状。改了就是跟别的版本的 dct 说不上话了。
@@ -305,24 +244,15 @@ mod tests {
     fn payloads_round_trip_through_encode_and_decode() {
         let k = keys_for(1);
         let m = member_from(&k, "laptop");
-        let p = Payload::Join(JoinRequest {
+        let p = Payload::InviteJoin {
             member: m,
             sig: "sig".into(),
-            commit: "c".into(),
-        });
+            spake: "x".into(),
+        };
         let encoded = encode(&p);
         assert_eq!(decode(&encoded).unwrap(), p);
     }
 
-    #[test]
-    fn thirty_two_byte_values_round_trip_and_anything_else_is_refused() {
-        let b = [7u8; 32];
-        assert_eq!(decode32(&encode32(&b)), Some(b));
-        assert_eq!(decode32(&STANDARD.encode([7u8; 31])), None);
-        assert_eq!(decode32(&STANDARD.encode([7u8; 33])), None);
-        assert_eq!(decode32("not base64!!"), None);
-    }
-
     #[test]
     fn decode_rejects_garbage_without_panicking() {
         assert!(decode(b"not json").is_err());
diff --git a/src/daemon.rs b/src/daemon.rs
index fe11386..fb7fa10 100644
--- a/src/daemon.rs
+++ b/src/daemon.rs
@@ -352,20 +352,6 @@ fn handle_mesh(
             .ok_or(MeshProblem::NotLoggedIn)
             .map(|(m, _)| recover(m.lock()).cancel_invite())
             .map(|_| view(ctl)),
-        Request::MeshApprove {
-            endpoint,
-            code,
-            yes,
-        } => ctl
-            .running()
-            .ok_or(MeshProblem::NotLoggedIn)
-            .and_then(|(m, n)| group::approve(&m, n.as_ref(), &endpoint, &code, yes))
-            .map(|_| view(ctl)),
-        Request::MeshConfirmInviter { endpoint } => ctl
-            .running()
-            .ok_or(MeshProblem::NotLoggedIn)
-            .and_then(|(m, _)| group::confirm(&m, &endpoint))
-            .map(|_| view(ctl)),
         Request::MeshRemove { name } => ctl
             .running()
             .ok_or(MeshProblem::NotLoggedIn)
@@ -404,8 +390,6 @@ fn mesh_view(ctl: &MeshCtl) -> crate::proto::MeshView {
         endpoint: String::new(),
         in_group: matches!(store.roster(), Ok(Some(_))),
         members: Vec::new(),
-        pending: Vec::new(),
-        joining: Vec::new(),
         messages: Default::default(),
         invite: None,
         invite_note: None,
@@ -883,15 +867,13 @@ fn serve(
         }
         let resp = match serde_json::from_str::<Request>(&line) {
             // 多电脑这几条只在这里答：从 HTTP 上来的请求走 `handle`，那边
-            // 一律拒绝（批准一台电脑进组，不能从局域网手机页上点）。
+            // 一律拒绝（出邀请码、加电脑，不能从局域网手机页上点）。
             Ok(
                 req @ (Request::MeshStatus
                 | Request::MeshLogin
                 | Request::MeshJoin { .. }
                 | Request::MeshInvite
                 | Request::MeshInviteCancel
-                | Request::MeshApprove { .. }
-                | Request::MeshConfirmInviter { .. }
                 | Request::MeshRemove { .. }
                 | Request::MeshPeers
                 | Request::MeshSend { .. }),
@@ -1261,8 +1243,6 @@ fn handle(
         | Request::MeshJoin { .. }
         | Request::MeshInvite
         | Request::MeshInviteCancel
-        | Request::MeshApprove { .. }
-        | Request::MeshConfirmInviter { .. }
         | Request::MeshRemove { .. }
         | Request::MeshPeers
         | Request::MeshSend { .. } => Ok(Response::Error(ErrorCode::BadRequest(
@@ -3625,14 +3605,6 @@ mod tests {
             },
             Request::MeshInvite,
             Request::MeshInviteCancel,
-            Request::MeshApprove {
-                endpoint: "c-x".into(),
-                code: "123456".into(),
-                yes: true,
-            },
-            Request::MeshConfirmInviter {
-                endpoint: "c-x".into(),
-            },
             Request::MeshRemove { name: "x".into() },
             Request::MeshPeers,
             Request::MeshSend {
@@ -4107,14 +4079,6 @@ mod mesh_tests {
         for req in [
             Request::MeshInvite,
             Request::MeshInviteCancel,
-            Request::MeshApprove {
-                endpoint: "x".into(),
-                code: "1".into(),
-                yes: true,
-            },
-            Request::MeshConfirmInviter {
-                endpoint: "x".into(),
-            },
             Request::MeshRemove { name: "x".into() },
             Request::MeshPeers,
             Request::MeshSend {
diff --git a/src/i18n.rs b/src/i18n.rs
index 9139571..da5e77c 100644
--- a/src/i18n.rs
+++ b/src/i18n.rs
@@ -1984,15 +1984,10 @@ pub mod msg {
                 en: "This computer is already in a group with your other computers".to_string(),
                 zh: "这台电脑已经跟你的其它电脑在一个组里了".to_string(),
             ),
-            NoOneAnswered => t!(
-                lang,
-                en: "None of your other computers answered. Make sure dct is running and signed in on one of them, then try again".to_string(),
-                zh: "你的其它电脑一台都没回应。确认有一台已有的电脑开着 dct、登录过多电脑，再试一次".to_string(),
-            ),
             TooManyAnswered => t!(
                 lang,
-                en: "Too many computers answered at once, which should not happen. Nothing was shown to compare. Try again later".to_string(),
-                zh: "一下子回应的电脑太多了，这不正常，这次不给你核对数字。过一会儿再试一次".to_string(),
+                en: "Far too many computers are online under your account (more than 16), which should not happen. Nothing was asked. Try again later".to_string(),
+                zh: "同一个账号在线的电脑太多了（超过 16 台），这不正常，这次一台都没问。过一会儿再试一次".to_string(),
             ),
             BadName => t!(
                 lang,
@@ -2004,36 +1999,11 @@ pub mod msg {
                 en: format!("There is no computer called {name} in the group"),
                 zh: format!("组里没有叫 {name} 的电脑"),
             ),
-            NoSuchRequest(name) => t!(
-                lang,
-                en: format!("No computer called {name} is waiting to join (requests expire after 10 minutes)"),
-                zh: format!("没有叫 {name} 的电脑在等加入（请求 10 分钟后作废）"),
-            ),
-            Ambiguous(name) => t!(
-                lang,
-                en: format!("More than one computer called {name} is waiting. Use the c-… number shown by dct peers instead"),
-                zh: format!("不止一台叫 {name} 的电脑在等。改用 dct peers 里列出的 c-… 编号"),
-            ),
-            NameTaken(name) => t!(
-                lang,
-                en: format!("The group already has a computer called {name}. On the new computer run: dct join --name <another name>"),
-                zh: format!("组里已经有一台叫 {name} 的电脑了。在新电脑上换个名字重来：dct join --name 新名字"),
-            ),
             CannotRemoveSelf => t!(
                 lang,
                 en: "A computer cannot remove itself. Remove it from one of your other computers".to_string(),
                 zh: "不能移除这台电脑自己。到你的另一台电脑上移除它".to_string(),
             ),
-            NoSuchInviter(who) => t!(
-                lang,
-                en: format!("{who} did not answer this join request, or it is more than 10 minutes old. Run dct join again"),
-                zh: format!("{who} 没回应过这次加入，或者已经过了 10 分钟。重新运行 dct join"),
-            ),
-            CodeMismatch => t!(
-                lang,
-                en: "The number does not match: the request waiting now is not the one you looked at. Run dct peers and check again".to_string(),
-                zh: "数字对不上：现在等着的已经不是你看过的那一条请求了。重新运行 dct peers 再看一眼".to_string(),
-            ),
             NotSaved => t!(
                 lang,
                 en: "Could not save the group list on this computer. Nothing was changed".to_string(),
@@ -2083,7 +2053,7 @@ pub mod msg {
         }
     }
 
-    // —— 多电脑（`dct login` / `dct join` / `dct peers`）——
+    // —— 多电脑（`dct login` / `dct invite` / `dct join` / `dct peers`）——
 
     /// 登录之后、这台电脑自己建了组。
     pub fn mesh_first_computer(lang: Lang) -> String {
@@ -2113,139 +2083,6 @@ pub mod msg {
         )
     }
 
-    /// 新电脑上：去已有的电脑上核对数字。只问到一台就把数字填进句子里；
-    /// 好几台就逐台列在下面（`mesh_code_line`）。
-    pub fn mesh_compare_codes(lang: Lang, single: Option<&str>) -> String {
-        match single {
-            Some(code) => t!(
-                lang,
-                en: format!("Look at any computer you already have: it will show a 6-digit number. If it is the same as {code} here, approve it there"),
-                zh: format!("请在你已有的任意一台电脑上看一眼：那边会显示一个 6 位数，跟这里的 {code} 一样就点同意"),
-            ),
-            None => t!(
-                lang,
-                en: "Look at any computer you already have: it will show a 6-digit number. If it is the same as the one listed here for that computer, approve it there".to_string(),
-                zh: "请在你已有的任意一台电脑上看一眼：那边会显示一个 6 位数，跟这里那台电脑后面的数字一样就点同意".to_string(),
-            ),
-        }
-    }
-
-    /// 「电脑名：数字」。
-    pub fn mesh_code_line(lang: Lang, name: &str, code: &str) -> String {
-        t!(
-            lang,
-            en: format!("  {name}: {code}"),
-            zh: format!("  {name}：{code}"),
-        )
-    }
-
-    /// 同名的不止一台时那一行：名字后面带端点，用户照着敲 `c-…`。
-    pub fn mesh_code_line_with_endpoint(lang: Lang, name: &str, endpoint: &str, code: &str) -> String {
-        t!(
-            lang,
-            en: format!("  {name} ({endpoint}): {code}"),
-            zh: format!("  {name} ({endpoint})：{code}"),
-        )
-    }
-
-    /// 新电脑上：用户说的名字对得上不止一台回过话的电脑。
-    pub fn mesh_ambiguous_responder(lang: Lang, name: &str) -> String {
-        t!(
-            lang,
-            en: format!("More than one computer called {name} answered. Run dct join again and type the c-… number shown next to the one whose number matches"),
-            zh: format!("不止一台叫 {name} 的电脑回应了。重新运行 dct join，输入数字对得上的那台后面括号里的 c-… 编号"),
-        )
-    }
-
-    /// 新电脑上：列完数字之后，问是哪一台。
-    pub fn mesh_which_computer(lang: Lang) -> String {
-        t!(
-            lang,
-            en: "Which computer shows the same number on its screen? Type its name (press Enter if none match — do not join): ".to_string(),
-            zh: "哪一台电脑屏幕上显示的是同一个数字？输入它的名字（都对不上就直接回车，不要加入）：".to_string(),
-        )
-    }
-
-    pub fn mesh_not_a_responder(lang: Lang, input: &str) -> String {
-        t!(
-            lang,
-            en: format!("No computer called {input} answered this join request"),
-            zh: format!("没有叫 {input} 的电脑回应过这次加入"),
-        )
-    }
-
-    /// `dct peers approve` 问 y/n 时没答 y。
-    pub fn mesh_not_approved(lang: Lang) -> String {
-        t!(
-            lang,
-            en: "Not approved. The request is still waiting".to_string(),
-            zh: "没有批准，请求还挂着".to_string(),
-        )
-    }
-
-    pub fn mesh_join_cancelled(lang: Lang) -> String {
-        t!(
-            lang,
-            en: "Not joined. If the numbers do not match, do not approve on the other computer".to_string(),
-            zh: "没有加入。数字对不上的话，别在那边点同意".to_string(),
-        )
-    }
-
-    pub fn mesh_waiting_for_approval(lang: Lang) -> String {
-        t!(
-            lang,
-            en: "Waiting for approval (up to 10 minutes)…".to_string(),
-            zh: "等那边点同意（最多等 10 分钟）……".to_string(),
-        )
-    }
-
-    pub fn mesh_join_timed_out(lang: Lang) -> String {
-        t!(
-            lang,
-            en: "No approval within 10 minutes. The invitation has expired, run dct join again".to_string(),
-            zh: "10 分钟里没等到同意，邀请已过期，请重新运行 dct join".to_string(),
-        )
-    }
-
-    /// 新电脑上：进组了。
-    pub fn mesh_joined_group(lang: Lang) -> String {
-        t!(lang, en: "Joined".to_string(), zh: "已加入".to_string())
-    }
-
-    /// 已有的电脑上：有一台想加入。
-    pub fn mesh_join_prompt(lang: Lang, name: &str, code: &str) -> String {
-        t!(
-            lang,
-            en: format!("A computer called {name} wants to join \"My computers\". Is the number on its screen {code}? (y/n)"),
-            zh: format!("一台叫 {name} 的电脑想加入「我的电脑」。它屏幕上的数字是 {code} 吗？(y/n)"),
-        )
-    }
-
-    /// 看板上那一行加入确认之后，还有几条排着（一次只问一条，先来的先问）。
-    pub fn mesh_more_requests(lang: Lang, n: usize) -> String {
-        t!(lang, en: format!("{n} more waiting"), zh: format!("还有 {n} 条"))
-    }
-
-    /// 看板上加入确认下面那行灰字：两台**新**电脑同时加入，会互相看到对方
-    /// 的请求、互相批准，进错组。
-    pub fn mesh_cross_join_hint(lang: Lang) -> String {
-        t!(
-            lang,
-            en: "Add one new computer at a time: two joining at once can approve each other".to_string(),
-            zh: "一次只加一台新电脑：两台同时加入，可能互相批准进错组".to_string(),
-        )
-    }
-
-    /// 九宫格顶上：有电脑在等批准。y/n 只在看板上接，这里只说怎么过去。
-    /// `g` 切的是**保存下来的**模式（九宫格 ↔ 看板），照实说。
-    pub fn mesh_grid_notice(lang: Lang) -> String {
-        t!(
-            lang,
-            en: "A computer wants to join \"My computers\". Press g to switch to the board and confirm (g also makes the board your default view; press g again to go back to the grid)".to_string(),
-            zh: "有电脑想加入「我的电脑」。按 g 切到看板确认（g 会把默认视图也换成看板，确认完再按 g 换回九宫格）".to_string(),
-        )
-    }
-
     /// 看板底部「我的电脑」那一段：没登录多电脑时唯一的一行。
     pub fn mesh_board_off(lang: Lang) -> String {
         t!(
@@ -2280,23 +2117,10 @@ pub mod msg {
         format!("✉ {n}")
     }
 
-    /// `dct peers` 里，一条等批准的请求下面那行：怎么批、怎么拒。
-    pub fn mesh_approve_hint(lang: Lang, target: &str) -> String {
-        t!(
-            lang,
-            en: format!("  approve: dct peers approve {target}    refuse: dct peers approve {target} --no"),
-            zh: format!("  同意：dct peers approve {target}    拒绝：dct peers approve {target} --no"),
-        )
-    }
-
     pub fn mesh_member_joined(lang: Lang, name: &str) -> String {
         t!(lang, en: format!("{name} has joined"), zh: format!("{name} 已加入"))
     }
 
-    pub fn mesh_member_refused(lang: Lang, name: &str) -> String {
-        t!(lang, en: format!("Refused {name}"), zh: format!("已拒绝 {name}"))
-    }
-
     pub fn mesh_member_removed(lang: Lang, name: &str) -> String {
         t!(lang, en: format!("{name} has been removed"), zh: format!("{name} 已移出"))
     }
@@ -2318,8 +2142,8 @@ pub mod msg {
     pub fn mesh_peers_usage(lang: Lang) -> String {
         t!(
             lang,
-            en: "Usage: dct peers | dct peers approve <name or c-…> [--no] | dct peers remove <name>".to_string(),
-            zh: "用法：dct peers | dct peers approve <电脑名或 c-…> [--no] | dct peers remove <电脑名>".to_string(),
+            en: "Usage: dct peers | dct peers remove <name>".to_string(),
+            zh: "用法：dct peers | dct peers remove <电脑名>".to_string(),
         )
     }
 
@@ -3394,17 +3218,11 @@ mod tests {
             Mesh(crate::proto::MeshProblem::NotLoggedIn),
             Mesh(crate::proto::MeshProblem::NoDcAccount),
             Mesh(crate::proto::MeshProblem::AlreadyInGroup),
-            Mesh(crate::proto::MeshProblem::NoOneAnswered),
             Mesh(crate::proto::MeshProblem::TooManyAnswered),
             Mesh(crate::proto::MeshProblem::BadName),
             Mesh(crate::proto::MeshProblem::NoSuchMachine("pc".into())),
-            Mesh(crate::proto::MeshProblem::NoSuchRequest("pc".into())),
-            Mesh(crate::proto::MeshProblem::Ambiguous("pc".into())),
-            Mesh(crate::proto::MeshProblem::NameTaken("pc".into())),
             Mesh(crate::proto::MeshProblem::CannotRemoveSelf),
             Mesh(crate::proto::MeshProblem::NotSaved),
-            Mesh(crate::proto::MeshProblem::NoSuchInviter("pc".into())),
-            Mesh(crate::proto::MeshProblem::CodeMismatch),
             Mesh(crate::proto::MeshProblem::TooLong),
             Mesh(crate::proto::MeshProblem::BadAddress("pc".into())),
             Mesh(crate::proto::MeshProblem::BadInviteCode),
@@ -3474,10 +3292,6 @@ mod tests {
     #[test]
     fn mesh_strings_say_exactly_what_the_brief_says() {
         use crate::proto::{ErrorCode, MeshProblem};
-        assert_eq!(
-            msg::mesh_join_prompt(Lang::Zh, "公司Windows", "123456"),
-            "一台叫 公司Windows 的电脑想加入「我的电脑」。它屏幕上的数字是 123456 吗？(y/n)"
-        );
         assert_eq!(msg::mesh_member_joined(Lang::Zh, "B"), "B 已加入");
         assert_eq!(msg::mesh_member_removed(Lang::Zh, "B"), "B 已移出");
         assert_eq!(
@@ -3489,12 +3303,6 @@ mod tests {
             "先在 dct 里用 DC 配对账号（进 dct 按 c 选 DC）"
         );
         assert_eq!(msg::mesh_first_computer(Lang::Zh), "这台电脑成了「我的电脑」组的第一台");
-        assert_eq!(
-            msg::mesh_compare_codes(Lang::Zh, Some("654321")),
-            "请在你已有的任意一台电脑上看一眼：那边会显示一个 6 位数，跟这里的 654321 一样就点同意"
-        );
-        assert_eq!(msg::mesh_code_line(Lang::Zh, "家里Mac", "000123"), "  家里Mac：000123");
-        assert_eq!(msg::mesh_joined_group(Lang::Zh), "已加入");
         // 看板（Task 8）
         assert_eq!(msg::mesh_board_off(Lang::Zh), "多电脑未开启 · 运行 dct login");
         assert_eq!(msg::mesh_board_more(Lang::Zh, 3), "还有 3 台");
@@ -3509,9 +3317,6 @@ mod tests {
             msg::mesh_board_state(Lang::En, true, true),
             msg::mesh_board_state(Lang::En, true, false),
             msg::mesh_board_state(Lang::En, false, false),
-            msg::mesh_more_requests(Lang::En, 2),
-            msg::mesh_cross_join_hint(Lang::En),
-            msg::mesh_grid_notice(Lang::En),
         ] {
             assert!(!has_han(&s), "{s}");
         }
@@ -3521,36 +3326,22 @@ mod tests {
         );
         assert!(login.starts_with("Could not sign in"), "{login}");
         for s in [
-            msg::mesh_join_prompt(Lang::En, "pc", "1"),
             msg::mesh_member_joined(Lang::En, "pc"),
             msg::mesh_member_removed(Lang::En, "pc"),
-            msg::mesh_member_refused(Lang::En, "pc"),
             msg::mesh_first_computer(Lang::En),
             msg::mesh_logged_in(Lang::En, "pc"),
-            msg::mesh_compare_codes(Lang::En, Some("1")),
-            msg::mesh_compare_codes(Lang::En, None),
-            msg::mesh_waiting_for_approval(Lang::En),
-            msg::mesh_join_timed_out(Lang::En),
-            msg::mesh_joined_group(Lang::En),
-            msg::mesh_approve_hint(Lang::En, "pc"),
             msg::mesh_members_header(Lang::En),
             msg::mesh_member_line(Lang::En, "pc", true, false),
             msg::mesh_member_line(Lang::En, "pc", false, false),
             msg::mesh_member_line(Lang::En, "pc", true, true),
             msg::mesh_peers_usage(Lang::En),
             msg::mesh_join_usage(Lang::En),
-            msg::mesh_which_computer(Lang::En),
-            msg::mesh_not_a_responder(Lang::En, "pc"),
-            msg::mesh_join_cancelled(Lang::En),
-            msg::mesh_not_approved(Lang::En),
-            msg::mesh_code_line_with_endpoint(Lang::En, "pc", "c-x", "1"),
             msg::mesh_peer_line(Lang::En, "pc", true, false, "macos"),
             msg::mesh_no_sessions(Lang::En),
             msg::mesh_tentacles_line(Lang::En, &["cam".into()]),
             msg::mesh_sent(Lang::En, "pc/s"),
             msg::mesh_queued(Lang::En),
             msg::mesh_send_usage(Lang::En),
-            msg::mesh_ambiguous_responder(Lang::En, "pc"),
         ] {
             assert!(!has_han(&s), "英文里有汉字：{s}");
         }
diff --git a/src/main.rs b/src/main.rs
index 8281599..18dc0d4 100644
--- a/src/main.rs
+++ b/src/main.rs
@@ -29,11 +29,10 @@ dct —— vibe coding 终端
   dct invite       老电脑上：出一个 6 位邀请码（10 分钟内有效，只能用一次）
   dct join <邀请码> [--name 电脑名]
                    新电脑上：用老电脑给的码加入（没登录会先自动登录）
-  dct peers        看组里有哪些电脑、开着哪些会话、谁在等批准
+  dct peers        看组里有哪些电脑、开着哪些会话
   dct send <电脑名>/<会话名> \"<内容>\"
                    给另一台电脑上的会话留一句话；会话名也可以写 #编号。
                    那边空着就马上敲进去，忙就排队
-  dct peers approve <电脑名> [--no]   同意（或拒绝）一台电脑加入
   dct peers remove <电脑名>           把一台电脑移出组
   dct daemon       只跑守护进程，不开界面
   dct gate         远程版那道门：守着一个端口，只放带对钥匙的人进去。
diff --git a/src/mesh/cli.rs b/src/mesh/cli.rs
index 34c105f..24ef289 100644
--- a/src/mesh/cli.rs
+++ b/src/mesh/cli.rs
@@ -3,9 +3,9 @@
 //! 这里只是把话说给人听：真正的事（换令牌、问别的电脑、签名单）都在守护
 //! 进程里做——它握着中转连接、钥匙和密钥仓，命令行这边一样也不碰。
 //!
-//! 跟守护进程说话的那一下（`call`）和问用户的那一下（`ask`）都是参数，
-//! 于是整套对话不起守护进程、不碰终端也能测。
-use std::io::{BufRead, IsTerminal, Write};
+//! 跟守护进程说话的那一下（`call`）是参数，于是整套对话不起守护进程、
+//! 不碰终端也能测。
+use std::io::{IsTerminal, Write};
 use std::path::Path;
 use std::time::Duration;
 
@@ -14,7 +14,7 @@ use anyhow::Result;
 use crate::client::Client;
 use crate::i18n::{msg, text, Key, Lang};
 use crate::proto::{
-    ErrorCode, MeshProblem, MeshView, PeerView, PendingJoin, Request, Response, SendOutcome,
+    ErrorCode, MeshProblem, MeshView, PeerView, Request, Response, SendOutcome,
 };
 
 /// 守护进程替我们打网络的那几条请求（换令牌、问别的电脑、广播名单）等多久。
@@ -27,8 +27,8 @@ const INVITE_POLL: Duration = Duration::from_secs(1);
 /// 这一条请求命令行等多久：多电脑的请求等 `MESH_CALL_WAIT`，别的（握手）
 /// 按本机答一句的 `READ_TIMEOUT`。
 ///
-/// 等不够的代价不是「慢一点」：守护进程可能在命令行放弃之后才办成——批准
-/// 已经签进名单、发给了每一台，屏幕上却说失败，用户会再批一次。
+/// 等不够的代价不是「慢一点」：守护进程可能在命令行放弃之后才办成——新
+/// 电脑已经进了组，屏幕上却说失败，用户会拿一个已经作废的码再试一次。
 fn wait_for(req: &Request) -> Duration {
     match crate::mesh::worst_case(req) {
         Some(_) => MESH_CALL_WAIT,
@@ -37,8 +37,6 @@ fn wait_for(req: &Request) -> Duration {
 }
 
 type Call<'a> = &'a mut dyn FnMut(Request) -> Result<Response>;
-/// 把一句提示给用户看、读回他敲的一行。读不到（没有终端、EOF）是 `None`。
-type Ask<'a> = &'a mut dyn FnMut(&str) -> Option<String>;
 
 /// `dct join` 的参数。
 #[derive(Debug, Default, PartialEq, Eq)]
@@ -89,16 +87,7 @@ pub fn run(args: &[String]) -> i32 {
         let wait = wait_for(&req);
         client.call_within(req, wait)
     };
-    let mut ask = |prompt: &str| {
-        print!("{prompt}");
-        let _ = std::io::stdout().flush();
-        let mut line = String::new();
-        match std::io::stdin().lock().read_line(&mut line) {
-            Ok(0) | Err(_) => None,
-            Ok(_) => Some(line.trim().to_string()),
-        }
-    };
-    dispatch(cmd, rest, &mut call, &mut ask, &mut out, &mut err, lang)
+    dispatch(cmd, rest, &mut call, &mut out, &mut err, lang)
 }
 
 /// 连上守护进程之后的全部：先握手，再按子命令办。
@@ -106,7 +95,6 @@ fn dispatch(
     cmd: &str,
     rest: &[String],
     call: Call,
-    ask: Ask,
     out: &mut dyn Write,
     err: &mut dyn Write,
     lang: Lang,
@@ -136,7 +124,7 @@ fn dispatch(
                 2
             }
         },
-        "peers" => peers(call, ask, out, err, lang, rest),
+        "peers" => peers(call, out, err, lang, rest),
         "send" => {
             // 在 dct 的会话里跑的，守护进程给子进程设过这个变量（`session.rs`）。
             let from = std::env::var(crate::session::SESSION_ID_ENV)
@@ -208,18 +196,6 @@ fn parse_join(args: &[String]) -> Option<JoinOpts> {
     (!opts.code.is_empty()).then_some(opts)
 }
 
-/// 按名字或端点在一张列表里找一条。找不到、不止一条都是 `Err`。
-fn pick<'a>(list: &'a [PendingJoin], who: &str) -> Result<&'a PendingJoin, usize> {
-    let hits: Vec<&PendingJoin> = list
-        .iter()
-        .filter(|p| p.endpoint == who || p.name == who)
-        .collect();
-    match hits.as_slice() {
-        [p] => Ok(p),
-        _ => Err(hits.len()),
-    }
-}
-
 /// 别的电脑报来的东西（名字、系统、会话……）印到终端之前都过一遍：
 /// 一个 `\x1b[8m` 就能把后面那行核对数字藏起来。守护进程给的现状已经洗过
 /// （`group::view`），这里是第二道，跟界面用的是同一份洗法。
@@ -396,7 +372,6 @@ pub(crate) fn join(
 
 pub(crate) fn peers(
     call: Call,
-    ask: Ask,
     out: &mut dyn Write,
     err: &mut dyn Write,
     lang: Lang,
@@ -426,66 +401,6 @@ pub(crate) fn peers(
                     }
                 }
             }
-            for p in &v.pending {
-                // 两台同名就只能按端点批，提示里直接给端点。
-                let dup = v.pending.iter().filter(|q| q.name == p.name).count() > 1;
-                let target = if dup { &p.endpoint } else { &p.name };
-                let _ = writeln!(
-                    out,
-                    "{}",
-                    msg::mesh_join_prompt(lang, &c(&p.name), &c(&p.code))
-                );
-                let _ = writeln!(out, "{}", msg::mesh_approve_hint(lang, &c(target)));
-            }
-            0
-        }
-        ["approve", who] | ["approve", who, "--no"] => {
-            let yes = args.len() == 2;
-            // 先看一眼挂着的是哪一条，把名字和数字再给用户看一遍；送去批的
-            // 是**这次给他看的**端点和数字，守护进程两样都对上才批。
-            let Some(v) = view_of(call, Request::MeshStatus, err, lang) else {
-                return 1;
-            };
-            let p = match pick(&v.pending, who) {
-                Ok(p) => p.clone(),
-                Err(0) => {
-                    say_error(
-                        err,
-                        lang,
-                        &ErrorCode::Mesh(MeshProblem::NoSuchRequest(who.to_string())),
-                    );
-                    return 1;
-                }
-                Err(_) => {
-                    say_error(
-                        err,
-                        lang,
-                        &ErrorCode::Mesh(MeshProblem::Ambiguous(who.to_string())),
-                    );
-                    return 1;
-                }
-            };
-            if yes {
-                let q = format!("{} ", msg::mesh_join_prompt(lang, &c(&p.name), &c(&p.code)));
-                let said_yes = ask(&q).is_some_and(|a| a.trim().eq_ignore_ascii_case("y"));
-                if !said_yes {
-                    let _ = writeln!(out, "{}", msg::mesh_not_approved(lang));
-                    return 1;
-                }
-            }
-            let req = Request::MeshApprove {
-                endpoint: p.endpoint.clone(),
-                code: p.code.clone(),
-                yes,
-            };
-            if view_of(call, req, err, lang).is_none() {
-                return 1;
-            }
-            if yes {
-                let _ = writeln!(out, "{}", msg::mesh_member_joined(lang, &c(&p.name)));
-            } else {
-                let _ = writeln!(out, "{}", msg::mesh_member_refused(lang, &c(&p.name)));
-            }
             0
         }
         ["remove", who] => {
@@ -629,8 +544,6 @@ mod tests {
                     is_me: *me,
                 })
                 .collect(),
-            pending: vec![],
-            joining: vec![],
             messages: Default::default(),
             invite: None,
             invite_note: None,
@@ -700,7 +613,6 @@ mod tests {
             &["invite"],
             &["join"],
             &["peers"],
-            &["peers", "approve", "B"],
             &["peers", "remove", "B"],
             &["send", "B/x", "hi"],
         ];
@@ -715,7 +627,6 @@ mod tests {
                 cmd,
                 &rest,
                 &mut sc.call(),
-                &mut no_ask,
                 &mut out,
                 &mut err,
                 Lang::Zh,
@@ -743,14 +654,6 @@ mod tests {
             },
             Request::MeshInvite,
             Request::MeshInviteCancel,
-            Request::MeshApprove {
-                endpoint: "c-x".into(),
-                code: "123456".into(),
-                yes: true,
-            },
-            Request::MeshConfirmInviter {
-                endpoint: "c-x".into(),
-            },
             Request::MeshRemove { name: "n".into() },
             Request::MeshPeers,
             Request::MeshSend {
@@ -783,7 +686,6 @@ mod tests {
             online: false,
             is_me: false,
         });
-        v.pending = vec![pj(evil, "c-w", "123456")];
         let sc = Script::new(vec![
             Response::Mesh(v),
             Response::Error(ErrorCode::DaemonNotResponding),
@@ -792,7 +694,6 @@ mod tests {
         assert_eq!(
             peers(
                 &mut sc.call(),
-                &mut no_ask,
                 &mut out,
                 &mut err,
                 Lang::Zh,
@@ -822,7 +723,6 @@ mod tests {
         let (mut out, mut err) = (vec![], vec![]);
         peers(
             &mut sc.call(),
-            &mut no_ask,
             &mut out,
             &mut err,
             Lang::Zh,
@@ -861,7 +761,6 @@ mod tests {
             "login",
             &[],
             &mut call,
-            &mut no_ask,
             &mut out,
             &mut err,
             Lang::Zh,
@@ -884,7 +783,6 @@ mod tests {
             "login",
             &[],
             &mut sc.call(),
-            &mut no_ask,
             &mut out,
             &mut err,
             Lang::Zh,
@@ -910,140 +808,34 @@ mod tests {
         );
     }
 
-    fn pj(name: &str, ep: &str, code: &str) -> PendingJoin {
-        PendingJoin {
-            name: name.into(),
-            endpoint: ep.into(),
-            code: code.into(),
-        }
-    }
-
-    fn no_ask(_: &str) -> Option<String> {
-        panic!("不该问用户")
-    }
-
+    /// 详情问不成（旧守护进程、出错）：退回名单那几行；不再有「谁在等批准」。
     #[test]
-    fn peers_lists_members_and_asks_about_pending_joins() {
-        let mut v = view(true, &[("B", true), ("A", false)]);
-        v.pending = vec![pj("公司Windows", "c-w", "123456")];
-        // 详情问不成（旧守护进程、出错）：退回名单那几行。
+    fn peers_falls_back_to_the_member_lines() {
+        let v = view(true, &[("B", true), ("A", false)]);
         let sc = Script::new(vec![
             Response::Mesh(v),
             Response::Error(ErrorCode::DaemonNotResponding),
         ]);
         let (mut out, mut err) = (vec![], vec![]);
-        assert_eq!(
-            peers(
-                &mut sc.call(),
-                &mut no_ask,
-                &mut out,
-                &mut err,
-                Lang::Zh,
-                &[]
-            ),
-            0
-        );
+        assert_eq!(peers(&mut sc.call(), &mut out, &mut err, Lang::Zh, &[]), 0);
         let o = s(&out);
         assert!(o.contains("  B  (这台)"), "{o}");
-        assert!(o.contains(
-            "一台叫 公司Windows 的电脑想加入「我的电脑」。它屏幕上的数字是 123456 吗？(y/n)"
-        ));
-        assert!(o.contains("dct peers approve 公司Windows"), "{o}");
-    }
-
-    fn args(v: &[&str]) -> Vec<String> {
-        v.iter().map(|s| s.to_string()).collect()
+        assert!(o.contains("  A  (在线)"), "{o}");
+        assert!(!o.contains("approve"), "{o}");
     }
 
-    /// `approve <名字>`：再给用户看一遍名字和数字，答 y 才送；送的是这次
-    /// 给他看的那条的端点和数字。
+    /// `dct peers approve` 没有了：当成用法不对。
     #[test]
-    fn peers_approve_reshows_the_code_and_sends_endpoint_and_code() {
-        let mut v = view(true, &[("B", true)]);
-        v.pending = vec![pj("C", "c-c", "123456")];
-        let sc = Script::new(vec![
-            Response::Mesh(v.clone()),
-            Response::Mesh(view(true, &[("B", true), ("C", false)])),
-        ]);
-        let shown = RefCell::new(String::new());
-        let mut ask = |p: &str| {
-            *shown.borrow_mut() = p.to_string();
-            Some("y".to_string())
-        };
+    fn peers_approve_is_gone() {
+        let sc = Script::new(vec![]);
         let (mut out, mut err) = (vec![], vec![]);
-        assert_eq!(
-            peers(
-                &mut sc.call(),
-                &mut ask,
-                &mut out,
-                &mut err,
-                Lang::Zh,
-                &args(&["approve", "C"])
-            ),
-            0
-        );
-        assert!(shown.borrow().contains("一台叫 C 的电脑") && shown.borrow().contains("123456"));
-        assert_eq!(
-            sc.seen.borrow()[1],
-            r#"MeshApprove { endpoint: "c-c", code: "123456", yes: true }"#
-        );
-        assert!(s(&out).contains("C 已加入"));
-
-        // 答 n：什么都不送。
-        let sc = Script::new(vec![Response::Mesh(v.clone())]);
-        let mut ask = |_: &str| Some("n".to_string());
-        out.clear();
-        assert_eq!(
-            peers(
-                &mut sc.call(),
-                &mut ask,
-                &mut out,
-                &mut err,
-                Lang::Zh,
-                &args(&["approve", "C"])
-            ),
-            1
-        );
-        assert_eq!(sc.seen.borrow().len(), 1);
-        assert!(s(&out).contains("没有批准"));
-
-        // --no：不问，直接拒。
-        let sc = Script::new(vec![
-            Response::Mesh(v.clone()),
-            Response::Mesh(view(true, &[("B", true)])),
-        ]);
-        out.clear();
-        peers(
-            &mut sc.call(),
-            &mut no_ask,
-            &mut out,
-            &mut err,
-            Lang::Zh,
-            &args(&["approve", "c-c", "--no"]),
-        );
-        assert!(sc.seen.borrow()[1].contains("yes: false"));
-        assert!(s(&out).contains("已拒绝 C"));
+        let code = peers(&mut sc.call(), &mut out, &mut err, Lang::Zh, &args(&["approve", "B"]));
+        assert_eq!(code, 2);
+        assert_eq!(s(&err).trim(), "用法：dct peers | dct peers remove <电脑名>");
     }
 
-    #[test]
-    fn peers_approve_with_two_of_the_same_name_asks_for_the_endpoint() {
-        let mut v = view(true, &[("B", true)]);
-        v.pending = vec![pj("C", "c-c", "1"), pj("C", "c-x", "2")];
-        let sc = Script::new(vec![Response::Mesh(v)]);
-        let (mut out, mut err) = (vec![], vec![]);
-        assert_eq!(
-            peers(
-                &mut sc.call(),
-                &mut no_ask,
-                &mut out,
-                &mut err,
-                Lang::Zh,
-                &args(&["approve", "C"])
-            ),
-            1
-        );
-        assert!(s(&err).contains("不止一台叫 C"));
-        assert_eq!(sc.seen.borrow().len(), 1);
+    fn args(v: &[&str]) -> Vec<String> {
+        v.iter().map(|s| s.to_string()).collect()
     }
 
     /// `dct peers` 的详情：每台一行带系统，在线的下面列会话和触手，不在线
@@ -1086,7 +878,6 @@ mod tests {
         let (mut out, mut err) = (vec![], vec![]);
         let code = peers(
             &mut sc.call(),
-            &mut no_ask,
             &mut out,
             &mut err,
             Lang::Zh,
@@ -1211,7 +1002,6 @@ mod tests {
         let sc = Script::new(vec![Response::Mesh(view(true, &[("B", true)]))]);
         peers(
             &mut sc.call(),
-            &mut no_ask,
             &mut out,
             &mut err,
             Lang::Zh,
@@ -1226,7 +1016,6 @@ mod tests {
         assert_eq!(
             peers(
                 &mut sc.call(),
-                &mut no_ask,
                 &mut out,
                 &mut err,
                 Lang::Zh,
@@ -1240,7 +1029,6 @@ mod tests {
         assert_eq!(
             peers(
                 &mut sc.call(),
-                &mut no_ask,
                 &mut out,
                 &mut err,
                 Lang::Zh,
@@ -1329,7 +1117,7 @@ mod tests {
             protocol: crate::proto::PROTOCOL_VERSION,
         }]);
         let (mut out, mut err) = (vec![], vec![]);
-        let code = dispatch("join", &[], &mut sc.call(), &mut no_ask, &mut out, &mut err, Lang::Zh);
+        let code = dispatch("join", &[], &mut sc.call(), &mut out, &mut err, Lang::Zh);
         assert_eq!(code, 2);
         assert!(s(&err).contains("dct join <邀请码>"), "{}", s(&err));
     }
@@ -1461,7 +1249,7 @@ mod tests {
         }]);
         let (mut out, mut err) = (vec![], vec![]);
         let rest = vec!["482913".to_string()];
-        let code = dispatch("invite", &rest, &mut sc.call(), &mut no_ask, &mut out, &mut err, Lang::Zh);
+        let code = dispatch("invite", &rest, &mut sc.call(), &mut out, &mut err, Lang::Zh);
         assert_eq!(code, 2);
         assert!(s(&err).contains("用法：dct invite"), "{}", s(&err));
     }
diff --git a/src/mesh/mod.rs b/src/mesh/mod.rs
index c47db9a..22c2d23 100644
--- a/src/mesh/mod.rs
+++ b/src/mesh/mod.rs
@@ -4,9 +4,10 @@
 //! - `login`：跟网关换中转令牌、到期前续期；
 //! - `store`：钥匙、电脑名、组名单落盘；
 //! - `net`：往外发（`Net` trait，真的 `LinkNet` 和测试用的 `FakeHub`）；
-//! - `group`：登录建组、加入、批准、移除这几个要跟别的电脑说话的流程；
+//! - `group`：登录建组、看现状、移除这几个要跟别的电脑说话的流程；
+//! - `invite`：用 6 位邀请码加电脑（老电脑出码、新电脑 `dct join <码>`）；
 //! - `deliver`：留言——`dct peers` 的详情、`dct send`、送进会话、忙时排队；
-//! - `cli`：`dct login` / `dct join` / `dct peers` / `dct send`；
+//! - `cli`：`dct login` / `dct invite` / `dct join` / `dct peers` / `dct send`；
 //! - 这里：`Mesh`，守护进程里这台电脑在组里的全部状态，以及电脑信封唯一的
 //!   入口 `Mesh::on_envelope`。
 use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
@@ -17,8 +18,8 @@ use base64::{engine::general_purpose::STANDARD, Engine as _};
 use dct_link::Envelope;
 use dct_mesh::roster::{self, Member, SignedRoster};
 use dct_mesh::seal::{self, Kind, Message, Sealed};
-use dct_mesh::wire::{self, JoinRequest, Payload};
-use dct_mesh::{id, sas, MachineKeys};
+use dct_mesh::wire::{self, Payload};
+use dct_mesh::{id, MachineKeys};
 
 use crate::journal::Journal;
 use crate::link::Handler;
@@ -51,61 +52,11 @@ pub const SENT_AT_WINDOW_SECS: u64 = 10 * 60;
 /// 去重表记多少条。
 const SEEN_CAP: usize = 1024;
 
-/// 一条加入请求等多久没人批就作废；新电脑那边也最多等这么久。
-pub const JOIN_TTL: std::time::Duration = std::time::Duration::from_secs(10 * 60);
-
-/// 最多同时挂几条等批准的加入请求。同账号里的一台电脑能一直发 `Join`，
-/// 不设上限的话这张表就是一个谁都能灌的口子。
-///
-/// **满了就拒新的，不挤掉旧的。** 挤旧的话，攻击者灌一轮就能把用户正在
-/// 核对的那条请求挤出去，再换上一条同名的冒牌货。
-const MAX_PENDING: usize = 16;
-
-/// 一段 `JOIN_TTL` 里最多亮出几个核对数字（每一次加入方揭晓随机数、承诺
-/// 验过，就亮一个）。
-///
-/// 数字是 6 位的：中转冒充一台电脑，每试一次有一百万分之一的机会让这边亮出
-/// 跟新电脑屏幕上一样的数字。不设上限，它可以一秒钟试几百次，在用户盯着
-/// 屏幕的那一两分钟里把一百万次试完。设了上限，一个加入窗口里它最多猜这么
-/// 几次；正常用一次 `dct join` 只亮一个。
-pub const MAX_CODES_PER_TTL: usize = 20;
-
-/// 新电脑加入时最多问几台已有的电脑（中转报的在线列表去重之后）。
-///
-/// 邀请方那边一个窗口最多亮 `MAX_CODES_PER_TTL` 个数字，这边也得有个数：
-/// 中转能在「谁在线」里列出任意多台假电脑，每台都回一句签得对的
-/// `JoinPending`、名字照抄用户的真电脑。每多一台，新电脑屏幕上就多一个
-/// 数字，就多一次百万分之一的「碰巧对上」。超过这个数就一台都不问
+/// 新电脑 `dct join` 时最多探问几台同账号在线的电脑（中转报的在线列表去重
+/// 之后）。中转能在「谁在线」里列出任意多台假电脑；超过这个数就一台都不问
 /// （`MeshProblem::TooManyAnswered`）——自己的电脑同时在线不会有这么多。
 pub const MAX_JOIN_ASK: usize = 16;
 
-/// 这台电脑是邀请方时，一条等人批准的加入请求。
-#[derive(Debug, Clone)]
-pub struct PendingReq {
-    pub req: JoinRequest,
-    /// 加入方在 `Join` 里交的承诺（`sas::commit`），已经解出来的 32 字节。
-    commit: [u8; 32],
-    /// 我为这一次出的随机数（在 `JoinPending` 里回给了对方）。
-    nonce: sas::Nonce,
-    /// 回 `JoinPending` 时我自己的成员记录。数字按这一份算，免得中间改了
-    /// 名、换了名单之后两边算的不是同一份。
-    mine: Member,
-    /// 加入方揭晓了随机数、承诺对得上之后才有：屏幕上的 6 位数。没有之前
-    /// 这条请求不给用户看，也批不了。
-    pub code: Option<String>,
-    pub at: Instant,
-}
-
-/// 新电脑这边：一台回过我 `JoinPending` 的已有电脑。
-#[derive(Debug, Clone)]
-pub struct Invite {
-    pub member: Member,
-    /// 我给它算的 6 位数。
-    pub code: String,
-    /// 什么时候回的（`clock` 的 unix 秒）。过了 `JOIN_TTL` 作废。
-    pub at: u64,
-}
-
 /// 一条等着投进会话的留言。
 #[derive(Debug, Clone)]
 pub struct QueuedMsg {
@@ -168,21 +119,6 @@ pub struct Mesh {
     pub keys: MachineKeys,
     pub me: Member,
     pub roster: Option<SignedRoster>,
-    /// 等人批准的加入请求。
-    pub pending_joins: Vec<PendingReq>,
-    /// 最近亮出核对数字的时刻（`MAX_CODES_PER_TTL`）。
-    codes_shown: VecDeque<Instant>,
-    /// 这台电脑自己在请求加入：回过我 `JoinPending` 的已有电脑，和我给它
-    /// 算的 6 位数。
-    pub invites: Vec<Invite>,
-    /// 用户核对过数字、认定的那一台（端点）。**只有它签的名单能成为我进组
-    /// 的第一份**（`roster::accept_invite`）。回过话的电脑不止它一台时，其余
-    /// 的谁都没被人核对过——中转可以塞进来一台自己的电脑，让它也回一句。
-    pub confirmed: Option<String>,
-    /// 用户还没认定之前，回过话的电脑先送来的名单。那边的人可能先点了同意，
-    /// 这边的人还没敲名字；先存着，认定的那一刻再验。别的电脑送来的永远
-    /// 用不上。每台最多一份。
-    held: Vec<SignedRoster>,
     /// 按会话 id 排队的留言（`deliver`）。
     pub queues: HashMap<u32, VecDeque<QueuedMsg>>,
     /// 会话清单和敲字的那一头。没有（测试、还没接上）就不收留言。
@@ -236,11 +172,6 @@ impl Mesh {
             keys,
             me,
             roster,
-            pending_joins: Vec::new(),
-            codes_shown: VecDeque::new(),
-            invites: Vec::new(),
-            confirmed: None,
-            held: Vec::new(),
             queues: HashMap::new(),
             inbox: None,
             in_flight: HashSet::new(),
@@ -334,11 +265,6 @@ impl Mesh {
         match payload {
             Payload::Roster(r) => Step::Done(self.take_roster(env, r)),
             Payload::Sealed(s) => self.take_sealed(env, &s),
-            Payload::Join(req) => Step::Done(self.take_join(env, req)),
-            // `JoinPending` 只该作为 `ask` 的答复回来（`group::join` 在那里
-            // 读它），不该从轮询里进来。
-            Payload::JoinPending { .. } => Step::Done(self.drop(env, "unasked_join_pending")),
-            Payload::JoinReveal { nonce } => Step::Done(self.take_reveal(env, &nonce)),
             Payload::InviteProbe => Step::Done(self.take_probe(env)),
             Payload::InviteJoin { member, sig, spake } => {
                 Step::Done(self.take_invite_join(env, member, &sig, &spake))
@@ -371,7 +297,7 @@ impl Mesh {
     }
 
     /// 这台电脑还不跟任何别的电脑同组：没有名单，或者名单上只有自己
-    /// （`dct login` 建的那一份）。只有这时候才能去加入别人的组。
+    /// （`dct login` 建的那一份）。只有这时候才能 `dct join` 别人的组。
     pub fn is_alone(&self) -> bool {
         match &self.roster {
             None => true,
@@ -419,24 +345,13 @@ impl Mesh {
         Ok(())
     }
 
-    /// 这台电脑请求加入时发出去的那一份：自己的成员记录，自己签名，外加对
-    /// `nonce` 的承诺（`sas::commit`）。`nonce` 每问一台电脑就另出一个，等
-    /// 那台回了它的随机数再揭晓。
-    pub fn join_request(&self, nonce: &sas::Nonce) -> JoinRequest {
-        JoinRequest {
-            member: self.me.clone(),
-            sig: wire::sign_member(&self.me, &self.keys),
-            commit: wire::encode32(&sas::commit(&self.me, nonce)),
-        }
-    }
-
-    /// 一个新的一次性随机数。
-    pub(crate) fn fresh_nonce(&self) -> sas::Nonce {
+    /// 32 字节新鲜随机数（SPAKE2 的种子）。
+    pub(crate) fn fresh_nonce(&self) -> [u8; 32] {
         (self.rand)()
     }
 
     /// 换上一份已经验过的名单：先落盘，再换内存。存不下就不换（理由见
-    /// `take_roster`）。已经在名单上的电脑，它的加入请求就不用再等了。
+    /// `take_roster`）。
     fn commit(&mut self, r: SignedRoster) -> anyhow::Result<()> {
         if let Some(s) = &self.store {
             s.save_roster(&r)?;
@@ -444,187 +359,18 @@ impl Mesh {
         if let Some(mine) = r.roster.member(&self.me.endpoint) {
             self.me = mine.clone();
         }
-        self.pending_joins
-            .retain(|p| r.roster.member(&p.req.member.endpoint).is_none());
         self.roster = Some(r);
         // 被移出组的电脑，它排着队的留言也一起作废。
         self.purge_non_members();
         Ok(())
     }
 
-    /// 扔掉过期的加入请求。
-    pub fn prune_pending(&mut self) {
-        self.pending_joins.retain(|p| p.at.elapsed() < JOIN_TTL);
-    }
-
-    /// 一台电脑想加入：验它的自签名、确认它说的端点就是中转认证过的发件
-    /// 端点，记下它的承诺，出一个新鲜的随机数，回一份我自己的自签成员记录
-    /// 加这个随机数。**这时还没有数字**：要等它揭晓自己的随机数
-    /// （`take_reveal`）。
-    ///
-    /// **这里不改名单。** 进名单只有一条路：用户看过两边的数字之后批准
-    /// （`group::approve`）。
-    fn take_join(&mut self, env: &Envelope, req: JoinRequest) -> Option<Vec<u8>> {
-        let Some(current) = self.roster.as_ref() else {
-            return self.drop(env, "join_without_group");
-        };
-        if req.member.endpoint != env.from.as_str() || !wire::verify_member(&req.member, &req.sig)
-        {
-            return self.drop(env, "join_bad_sig");
-        }
-        // 签得进名单的才挂：名字不合规、加密公钥解不出来的，批了也是白批。
-        if !valid_name(&req.member.name) || !valid_kx_pub(&req.member.kx_pub) {
-            return self.drop(env, "join_bad_member");
-        }
-        if current.roster.member(&req.member.endpoint).is_some() {
-            return self.drop(env, "join_already_member");
-        }
-        let Some(commit) = wire::decode32(&req.commit) else {
-            return self.drop(env, "join_bad_commit");
-        };
-        self.prune_pending();
-        let again = self
-            .pending_joins
-            .iter()
-            .any(|p| p.req.member.endpoint == req.member.endpoint);
-        if !again && self.pending_joins.len() >= MAX_PENDING {
-            return self.drop(env, "join_pending_full");
-        }
-        // 同一台再问一次（重跑了 `dct join`）：旧的那条连同它的数字一起作废，
-        // 这一次换新的随机数。
-        self.pending_joins
-            .retain(|p| p.req.member.endpoint != req.member.endpoint);
-        let nonce = self.fresh_nonce();
-        let mine = self.me.clone();
-        self.journal.mesh(&format!("join_asked from={}", env.from));
-        self.pending_joins.push(PendingReq {
-            req,
-            commit,
-            nonce,
-            mine: mine.clone(),
-            code: None,
-            at: Instant::now(),
-        });
-        Some(wire::encode(&Payload::JoinPending {
-            sig: wire::sign_member(&mine, &self.keys),
-            member: mine,
-            nonce: wire::encode32(&nonce),
-        }))
-    }
-
-    /// 加入方揭晓随机数：跟它在 `Join` 里的承诺对得上，才算出 6 位数、让
-    /// 这条请求出现在用户面前。对不上的整条扔掉——那是有人在中间换东西。
-    fn take_reveal(&mut self, env: &Envelope, nonce: &str) -> Option<Vec<u8>> {
-        self.prune_pending();
-        let Some(i) = self
-            .pending_joins
-            .iter()
-            .position(|p| p.req.member.endpoint == env.from.as_str())
-        else {
-            return self.drop(env, "reveal_without_join");
-        };
-        if self.pending_joins[i].code.is_some() {
-            return self.drop(env, "reveal_again");
-        }
-        let p = &self.pending_joins[i];
-        let Some(n) = wire::decode32(nonce).filter(|n| sas::opens(&p.commit, &p.req.member, n))
-        else {
-            self.pending_joins.remove(i);
-            return self.drop(env, "reveal_does_not_open");
-        };
-        while self
-            .codes_shown
-            .front()
-            .is_some_and(|t| t.elapsed() >= JOIN_TTL)
-        {
-            self.codes_shown.pop_front();
-        }
-        if self.codes_shown.len() >= MAX_CODES_PER_TTL {
-            self.pending_joins.remove(i);
-            return self.drop(env, "reveal_rate_limited");
-        }
-        self.codes_shown.push_back(Instant::now());
-        let code = sas::code(&p.mine, &p.req.member, &p.nonce, &n);
-        self.pending_joins[i].code = Some(code);
-        self.journal
-            .mesh(&format!("join_pending from={}", env.from));
-        None
-    }
-
-    /// 换一批回过话的电脑（新的一次 `dct join`）。之前认定的、存着的都作废。
-    pub fn set_invites(&mut self, found: Vec<(Member, String)>) {
-        let at = (self.clock)();
-        self.invites = found
-            .into_iter()
-            .map(|(member, code)| Invite { member, code, at })
-            .collect();
-        self.confirmed = None;
-        self.held.clear();
-    }
-
-    /// 扔掉过期的邀请，以及跟着它们的认定。（存着的名单不用跟着清：它只在
-    /// `confirm_inviter` 里、那台的邀请还在时才会被拿出来验。）
-    pub fn prune_invites(&mut self) {
-        let now = (self.clock)();
-        self.invites
-            .retain(|i| now.saturating_sub(i.at) < JOIN_TTL.as_secs());
-        let live: Vec<String> = self.invites.iter().map(|i| i.member.endpoint.clone()).collect();
-        if self.confirmed.as_ref().is_some_and(|c| !live.contains(c)) {
-            self.confirmed = None;
-        }
-    }
-
-    /// 用户说「是这一台，数字一样」。它先前送来过名单的话，现在验。
-    ///
-    /// 认定不了时，错误里带的是给人看的电脑名（过期之前记得住的话），不是
-    /// 端点——用户敲的就是名字。
-    pub fn confirm_inviter(&mut self, endpoint: &str) -> Result<(), crate::proto::MeshProblem> {
-        let name = self
-            .invites
-            .iter()
-            .find(|i| i.member.endpoint == endpoint)
-            .map(|i| i.member.name.clone())
-            .unwrap_or_else(|| endpoint.to_string());
-        self.prune_invites();
-        if !self.invites.iter().any(|i| i.member.endpoint == endpoint) {
-            return Err(crate::proto::MeshProblem::NoSuchInviter(name));
-        }
-        self.confirmed = Some(endpoint.to_string());
-        if let Some(i) = self.held.iter().position(|r| r.signer == endpoint) {
-            let r = self.held.remove(i);
-            self.take_invite(endpoint, r);
-        }
-        Ok(())
-    }
-
     fn take_roster(&mut self, env: &Envelope, incoming: SignedRoster) -> Option<Vec<u8>> {
-        // 我正在请求加入：只有用户认定的那台签的名单走 `accept_invite`；
-        // 别的回过话的电脑送来的，先存着（用户可能还没认定），永远不直接收。
-        if self.is_alone() {
-            self.prune_invites();
-            if self.confirmed.as_deref() == Some(incoming.signer.as_str()) {
-                self.take_invite(env.from.as_str(), incoming);
-                return None;
-            }
-            if self.invites.iter().any(|i| i.member.endpoint == incoming.signer) {
-                // `signer` 只是名单里的一个字段，还没验过。按它存的话，同账号
-                // 里随便一台电脑送一份写着「signer = A」的垃圾，就能把 A 真正
-                // 送来的那份顶掉，用户认定 A 的那一刻什么也验不出来。所以
-                // 只存**签名者亲自送来的**：中转认证过发件端点，冒不了。
-                if env.from.as_str() != incoming.signer {
-                    return self.drop(env, "invite_not_from_signer");
-                }
-                self.journal
-                    .mesh(&format!("invite_held from={}", env.from));
-                self.held.retain(|r| r.signer != incoming.signer);
-                self.held.push(incoming);
-                return None;
-            }
-        }
-        // 还没在任何组里的电脑，**不从网上接一份名单当自己的第一份**：
-        // `accept(None, ..)` 只查「是一份自签的创世名单」，同账号里谁都造得
-        // 出一份。加入别人的组走上面那条 `accept_invite`，它还要核对签名者
-        // 就是给我算过 6 位数的那台电脑。
+        // **不从网上接一份名单当自己的第一份**：`accept(None, ..)` 只查「是一份
+        // 自签的创世名单」，同账号里谁都造得出一份；只有自己一台的电脑手上那
+        // 份创世名单，组 id 跟别人的也对不上（`accept` 拒掉）。加入别人的组只
+        // 有一条路：`invite::join`，名单跟着 `InviteDone` 回来，签名者就是
+        // SPAKE2 里绑定的那台。
         let Some(current) = self.roster.as_ref() else {
             return self.drop(env, "roster_without_group");
         };
@@ -640,40 +386,6 @@ impl Mesh {
         None
     }
 
-    /// 验、收一份邀请名单。`incoming.signer` 必须是用户认定的那台（调用方
-    /// 已经对过）。
-    fn take_invite(&mut self, from: &str, incoming: SignedRoster) {
-        let Some(inviter) = self
-            .invites
-            .iter()
-            .find(|i| Some(&i.member.endpoint) == self.confirmed.as_ref())
-            .map(|i| i.member.clone())
-        else {
-            return self.drop_from(from, "invite_not_confirmed");
-        };
-        if let Err(e) = roster::accept_invite(&incoming, &self.me.endpoint, &inviter) {
-            return self.drop_from(from, &format!("invite_{e:?}"));
-        }
-        // `accept_invite` 只拿得到我的端点（它绑着签名公钥）；我那一条的
-        // 加密公钥对不对，只有我自己知道。换成别的，发给我的东西我就解不开，
-        // 而能解开的是别人。
-        let mine = incoming.roster.member(&self.me.endpoint);
-        if mine.map(|m| (&m.sign_pub, &m.kx_pub)) != Some((&self.me.sign_pub, &self.me.kx_pub)) {
-            return self.drop_from(from, "invite_not_my_keys");
-        }
-        match self.commit(incoming) {
-            Ok(()) => {
-                self.invites.clear();
-                self.confirmed = None;
-                self.held.clear();
-                self.journal.mesh(&format!("joined via={from}"));
-            }
-            Err(e) => self
-                .journal
-                .mesh(&format!("roster_not_saved from={from} err={e}")),
-        }
-    }
-
     fn take_sealed(&mut self, env: &Envelope, s: &Sealed) -> Step {
         let Some(current) = self.roster.as_ref() else {
             return Step::Done(self.drop(env, "sealed_without_group"));
@@ -745,7 +457,7 @@ impl Mesh {
 ///
 /// 每一次打中转都有上限：`peers`/`send` 是 `net::CALL_TIMEOUT`，`ask` 是各自
 /// 给的超时，换令牌是 `login::GATEWAY_TIMEOUT`；一次发给好几台（广播名单、
-/// 加入时揭晓随机数）是并排发的，只算一次。命令行等的比这个长（`cli::wait_for`）。
+/// 探问）是并排发的，只算一次。命令行等的比这个长（`cli::wait_for`）。
 ///
 /// 发给**这台电脑自己**的会话（`dct send 本机/…`）不经过中转，但要在本机
 /// 敲字，敲字前的 git 快照没有上限；那一条不算在这里。
@@ -753,7 +465,7 @@ pub fn worst_case(req: &crate::proto::Request) -> Option<std::time::Duration> {
     use crate::proto::Request as R;
     let call = net::CALL_TIMEOUT;
     Some(match req {
-        R::MeshStatus | R::MeshConfirmInviter { .. } => call,
+        R::MeshStatus => call,
         R::MeshLogin => login::GATEWAY_TIMEOUT + call,
         R::MeshInvite | R::MeshInviteCancel => call,
         // 可能先登录（换令牌）；问谁在线、并排探问；最多试
@@ -767,7 +479,7 @@ pub fn worst_case(req: &crate::proto::Request) -> Option<std::time::Duration> {
                 + call
         }
         // 并排广播新名单，再看一眼现状。
-        R::MeshApprove { .. } | R::MeshRemove { .. } => call + call,
+        R::MeshRemove { .. } => call + call,
         R::MeshPeers => call + deliver::STATUS_ASK_TIMEOUT,
         // 中转回 `Busy` 时换个 id 再问一次。
         R::MeshSend { .. } => deliver::SEND_ASK_TIMEOUT * 2,
diff --git a/src/proto.rs b/src/proto.rs
index b60035a..537668a 100644
--- a/src/proto.rs
+++ b/src/proto.rs
@@ -134,10 +134,14 @@ use crate::session::{ScrollBy, ScrollState, SessionInfo, SessionState};
 /// 期间，别的电脑送进每个会话的留言条数）。**响应**的形状变了，照 13 那次
 /// 的规矩加一。
 ///
-/// 24 = 用 6 位邀请码加电脑（dct-invite-v1）：多了 `Request::MeshInvite` /
-/// `MeshInviteCancel`、`Response::MeshInvite(InviteView)`，`MeshJoin` 多了
-/// `code`，`MeshView` 多了 `invite` / `invite_note`，`MeshProblem` 多了
-/// `BadInviteCode` / `WrongInviteCode` / `NoInvite` / `InviteRosterRefused`。
+/// 24 = 用 6 位邀请码加电脑（dct-invite-v1），**旧的核对 + 批准整个删掉**：
+/// 多了 `Request::MeshInvite` / `MeshInviteCancel`、`Response::MeshInvite(InviteView)`，
+/// `MeshJoin` 多了 `code`，`MeshView` 多了 `invite` / `invite_note`、少了
+/// `pending` / `joining`，删掉 `MeshApprove` / `MeshConfirmInviter`；
+/// `MeshProblem` 多了 `BadInviteCode` / `WrongInviteCode` / `NoInvite` /
+/// `InviteRosterRefused`，删掉 `NoOneAnswered` / `NoSuchRequest` / `Ambiguous` /
+/// `NameTaken` / `NoSuchInviter` / `CodeMismatch`。24 在一个分支里定了两回
+/// （先加后删），中间没有发过版，所以只加一次号。
 pub const PROTOCOL_VERSION: u32 = 24;
 
 /// 对面那个守护进程能不能用。
@@ -516,10 +520,10 @@ pub enum Request {
     LiveUnpublish,
     /// 给管理台一张只能切换公开状态的凭证。没在播回 `LiveStagingRejected(NotLive)`。
     LivePublishGrant,
-    /// 多电脑：这台电脑登录了没有、叫什么、组里有谁、谁在等批准。
+    /// 多电脑：这台电脑登录了没有、叫什么、组里有谁、发着的邀请码。
     ///
-    /// **`Mesh*` 这几条只从本机 socket 上答**（同 `Web*`）：批准一台电脑进组
-    /// 是这台机器主人的决定，不能从局域网手机页上点。
+    /// **`Mesh*` 这几条只从本机 socket 上答**（同 `Web*`）：出邀请码、加一台
+    /// 电脑进组是这台机器主人的决定，不能从局域网手机页上点；现状里还带着码。
     MeshStatus,
     /// 拿 DC 账号的 `api_key` 跟网关换中转令牌、连上中转；还没有组就自己
     /// 建一个。会打网络（网关），界面要放后台线程。
@@ -537,21 +541,6 @@ pub enum Request {
     MeshInvite,
     /// 收回手上的邀请码（`dct invite` 被 Ctrl-C）。
     MeshInviteCancel,
-    /// 批准（`yes`）或拒绝一台等着加入的电脑。
-    ///
-    /// `endpoint` 和 `code` 都是**界面刚给用户看过的那一条**：守护进程要两样
-    /// 都跟挂着的请求对上才批。只按名字批的话，攻击者可以换上一条同名的
-    /// 请求，用户点的「同意」就落到了冒牌货身上。
-    MeshApprove {
-        endpoint: String,
-        code: String,
-        yes: bool,
-    },
-    /// 新电脑上：用户核对过数字，认定是 `endpoint` 这一台。只有它签的名单
-    /// 能让这台电脑进组。
-    MeshConfirmInviter {
-        endpoint: String,
-    },
     /// 把一台电脑移出组。不能是自己。
     MeshRemove {
         name: String,
@@ -705,20 +694,6 @@ impl std::fmt::Debug for Request {
                 .finish(),
             Request::MeshInvite => write!(f, "MeshInvite"),
             Request::MeshInviteCancel => write!(f, "MeshInviteCancel"),
-            Request::MeshApprove {
-                endpoint,
-                code,
-                yes,
-            } => f
-                .debug_struct("MeshApprove")
-                .field("endpoint", endpoint)
-                .field("code", code)
-                .field("yes", yes)
-                .finish(),
-            Request::MeshConfirmInviter { endpoint } => f
-                .debug_struct("MeshConfirmInviter")
-                .field("endpoint", endpoint)
-                .finish(),
             Request::MeshRemove { name } => {
                 f.debug_struct("MeshRemove").field("name", name).finish()
             }
@@ -893,10 +868,6 @@ pub struct MeshView {
     /// 有没有一份名单（登录之后至少是只有自己的那一份）。
     pub in_group: bool,
     pub members: Vec<MemberView>,
-    /// 别的电脑想加入、等这台批准的。
-    pub pending: Vec<PendingJoin>,
-    /// 这台电脑自己在请求加入：问到的每台已有电脑，和给它算的 6 位数。
-    pub joining: Vec<PendingJoin>,
     /// 会话 id → 这次守护进程运行期间送进这个会话的留言条数（看板上的
     /// 「✉ N」）。只记真的敲进去了的，排着队的不算。
     pub messages: std::collections::BTreeMap<u32, u32>,
@@ -914,13 +885,6 @@ pub struct MemberView {
     pub is_me: bool,
 }
 
-/// 一次加入请求，以及两边屏幕上都该出现的那个 6 位数。
-#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
-pub struct PendingJoin {
-    pub name: String,
-    pub endpoint: String,
-    pub code: String,
-}
 
 /// 这台电脑此刻发着的邀请码（`dct invite` / 看板上按 `a`）。
 ///
@@ -1224,29 +1188,15 @@ pub enum MeshProblem {
     LoginFailed(String),
     /// 已经跟别的电脑在一个组里了，不用再加入。
     AlreadyInGroup,
-    /// 一台在线的、能回答的已有电脑都没有。
-    NoOneAnswered,
-    /// 中转报上来的在线电脑多得不像话（超过 `mesh::MAX_JOIN_ASK` 台）：一台
-    /// 都不问、一个数字都不给看。每多一个数字，就多一次让冒充的电脑碰巧
-    /// 对上老电脑屏幕的机会。
+    /// 中转报上来的同账号在线电脑多得不像话（超过 `mesh::MAX_JOIN_ASK` 台）：
+    /// 一台都不问。自己的电脑同时在线不会有这么多。
     TooManyAnswered,
     /// 电脑名不合规（空、带 `/`、太长）。
     BadName,
     /// 组里没有这台电脑。
     NoSuchMachine(String),
-    /// 没有这台电脑的加入请求（或者已经过期）。
-    NoSuchRequest(String),
-    /// 不止一台叫这个名字的电脑在等，得用端点指明。
-    Ambiguous(String),
-    /// 组里已经有一台叫这个名字的了。
-    NameTaken(String),
     /// 不能移除这台电脑自己。
     CannotRemoveSelf,
-    /// 新电脑上认定的那台没回应过这次加入（或者已经过了 10 分钟）。
-    NoSuchInviter(String),
-    /// 要批准的那条请求的数字跟屏幕上显示的对不上——挂着的已经不是用户
-    /// 看到的那一条了。
-    CodeMismatch,
     /// 名单存不下。
     NotSaved,
     /// 留言洗过之后还是超过 `mesh::deliver::MAX_BODY_CHARS`。
@@ -1596,15 +1546,7 @@ mod tests {
             },
             Request::MeshInvite,
             Request::MeshInviteCancel,
-            Request::MeshApprove {
-                endpoint: "c-x".into(),
-                code: "123456".into(),
-                yes: true,
-            },
             Request::MeshRemove { name: "n".into() },
-            Request::MeshConfirmInviter {
-                endpoint: "c-x".into(),
-            },
             Request::MeshPeers,
             Request::MeshSend {
                 to: "pc/s".into(),
@@ -1618,7 +1560,7 @@ mod tests {
             (PROTOCOL_VERSION, shape.as_str()),
             (
                 24,
-                r#"["Hello","List",{"Create":{"dir":"d","profile":"p","remember":true}},{"Input":{"id":1,"text":"t"}},{"Screen":{"id":1}},{"Screens":{"ids":[1]}},{"Resize":{"id":1,"rows":2,"cols":3}},{"Stop":{"id":1}},{"Kill":{"id":1}},"Prune",{"Undo":{"id":1}},{"Diff":{"id":1}},{"Profiles":{"lang":"Zh"}},"Projects",{"SetSecret":{"profile":"p","value":"v"}},{"DeleteSecret":{"profile":"p"}},{"LastProfile":{"dir":"d"}},{"PinProject":{"dir":"d"}},{"UnpinProject":{"dir":"d"}},{"VerifySecret":{"profile":"p","value":"v"}},{"PairStart":{"profile":"p","opt_in_llm":true}},{"PairPoll":{"profile":"p","opt_in_llm":true}},{"PairCancel":{"profile":"p"}},{"Explanation":{"id":1}},{"Scroll":{"id":1,"by":{"Rows":3}}},{"Mouse":{"id":1,"event":{"col":10,"row":20,"kind":{"Press":0},"shift":false,"alt":false,"ctrl":false}}},"PhoneStatus",{"PhoneSetToken":{"token":"t"}},"PhoneUnpair","PhoneDisable",{"Key":{"id":1,"name":"Up"}},{"WebStrings":{"lang":"zh-CN"}},"WebStatus","WebEnable","WebDisable",{"LiveStart":{"ids":[1],"names":["n"]}},{"LiveRestage":{"ids":[1],"names":["n"]}},"LiveStop","LiveStatus",{"LivePublish":{"title":"t"}},"LiveUnpublish","LivePublishGrant","MeshStatus","MeshLogin",{"MeshJoin":{"code":"482913","name":"n"}},"MeshInvite","MeshInviteCancel",{"MeshApprove":{"endpoint":"c-x","code":"123456","yes":true}},{"MeshRemove":{"name":"n"}},{"MeshConfirmInviter":{"endpoint":"c-x"}},"MeshPeers",{"MeshSend":{"to":"pc/s","text":"t","from_session":1}}]"#
+                r#"["Hello","List",{"Create":{"dir":"d","profile":"p","remember":true}},{"Input":{"id":1,"text":"t"}},{"Screen":{"id":1}},{"Screens":{"ids":[1]}},{"Resize":{"id":1,"rows":2,"cols":3}},{"Stop":{"id":1}},{"Kill":{"id":1}},"Prune",{"Undo":{"id":1}},{"Diff":{"id":1}},{"Profiles":{"lang":"Zh"}},"Projects",{"SetSecret":{"profile":"p","value":"v"}},{"DeleteSecret":{"profile":"p"}},{"LastProfile":{"dir":"d"}},{"PinProject":{"dir":"d"}},{"UnpinProject":{"dir":"d"}},{"VerifySecret":{"profile":"p","value":"v"}},{"PairStart":{"profile":"p","opt_in_llm":true}},{"PairPoll":{"profile":"p","opt_in_llm":true}},{"PairCancel":{"profile":"p"}},{"Explanation":{"id":1}},{"Scroll":{"id":1,"by":{"Rows":3}}},{"Mouse":{"id":1,"event":{"col":10,"row":20,"kind":{"Press":0},"shift":false,"alt":false,"ctrl":false}}},"PhoneStatus",{"PhoneSetToken":{"token":"t"}},"PhoneUnpair","PhoneDisable",{"Key":{"id":1,"name":"Up"}},{"WebStrings":{"lang":"zh-CN"}},"WebStatus","WebEnable","WebDisable",{"LiveStart":{"ids":[1],"names":["n"]}},{"LiveRestage":{"ids":[1],"names":["n"]}},"LiveStop","LiveStatus",{"LivePublish":{"title":"t"}},"LiveUnpublish","LivePublishGrant","MeshStatus","MeshLogin",{"MeshJoin":{"code":"482913","name":"n"}},"MeshInvite","MeshInviteCancel",{"MeshRemove":{"name":"n"}},"MeshPeers",{"MeshSend":{"to":"pc/s","text":"t","from_session":1}}]"#
             ),
             "协议的线上形状变了。把 PROTOCOL_VERSION 加一，再把这里的期望值更新成新的形状。"
         );
@@ -2009,12 +1951,6 @@ mod tests {
                 online: true,
                 is_me: true,
             }],
-            pending: vec![PendingJoin {
-                name: "B".into(),
-                endpoint: "c-b".into(),
-                code: "123456".into(),
-            }],
-            joining: Vec::new(),
             messages: [(7, 2)].into_iter().collect(),
             invite: Some(InviteView {
                 id: 3,
@@ -2032,7 +1968,7 @@ mod tests {
             (PROTOCOL_VERSION, serde_json::to_string(&v).unwrap().as_str()),
             (
                 24,
-                r#"{"Mesh":{"logged_in":true,"name":"A","endpoint":"c-a","in_group":true,"members":[{"name":"A","endpoint":"c-a","online":true,"is_me":true}],"pending":[{"name":"B","endpoint":"c-b","code":"123456"}],"joining":[],"messages":{"7":2},"invite":{"id":3,"code":"012345","expires_at":1800000600},"invite_note":{"id":2,"outcome":{"Joined":{"name":"公司电脑"}}}}}"#
+                r#"{"Mesh":{"logged_in":true,"name":"A","endpoint":"c-a","in_group":true,"members":[{"name":"A","endpoint":"c-a","online":true,"is_me":true}],"messages":{"7":2},"invite":{"id":3,"code":"012345","expires_at":1800000600},"invite_note":{"id":2,"outcome":{"Joined":{"name":"公司电脑"}}}}}"#
             ),
             "协议的线上形状变了。把 PROTOCOL_VERSION 加一，再把这里的期望值更新成新的形状。"
         );
diff --git a/src/ui/board.rs b/src/ui/board.rs
index 0e0ec34..a5f1981 100644
--- a/src/ui/board.rs
+++ b/src/ui/board.rs
@@ -30,10 +30,6 @@ const SESSION_PREFIX_COLS: usize = 2 + 1 + 7 + 8 + 16;
 /// 用户怎么退出的行（`e0ba1ec`）。现在它是函数，`return` 是安全的，
 /// 但如果哪天又被内联回循环里，这条约束就会重新生效。
 pub(crate) fn handle_key(app: &mut App, key: KeyEvent) -> Result<()> {
-    // 顶部挂着一条加入确认时，y / n 先归它（`n` 平时是新建会话）。
-    if super::computers::handle_key(app, &key) {
-        return Ok(());
-    }
     match key.code {
         KeyCode::Char('q') if is_plain_key(&key) => app.quit = true,
         KeyCode::Down => super::move_row(app, 1),
@@ -369,16 +365,15 @@ pub(crate) fn draw(f: &mut Frame, area: Rect, app: &mut App) {
     if show_version {
         block = block.title_top(Line::from(Span::styled(version, dim())).right_aligned());
     }
-    // 多电脑那两块：标题下面一行加入确认，底部一段「我的电脑」。都是现成
-    // 的数据（`App::mesh`），这里不发请求。
+    // 多电脑那两块：标题下面的邀请码，底部一段「我的电脑」。都是现成的
+    // 数据（`App::mesh`），这里不发请求。
     let inner = block.inner(area);
     let width = inner.width as usize;
     let now = std::time::SystemTime::now()
         .duration_since(std::time::UNIX_EPOCH)
         .map(|d| d.as_secs())
         .unwrap_or(0);
-    let mut prompt = super::computers::invite_lines(&app.mesh, app.lang, width, now);
-    prompt.extend(super::computers::prompt_lines(&app.mesh, app.lang, width));
+    let prompt = super::computers::invite_lines(&app.mesh, app.lang, width, now);
     let section = super::computers::section_lines(&app.mesh, app.lang, width);
     let (prompt_area, list_area, section_area) = split(inner, prompt.len(), section.len());
     f.render_widget(block, area);
@@ -391,8 +386,8 @@ pub(crate) fn draw(f: &mut Frame, area: Rect, app: &mut App) {
     f.render_widget(Paragraph::new(section), section_area);
 }
 
-/// 把标题下面那块分成三截：确认行、列表、「我的电脑」。高度不够时先保
-/// 确认行（要人拍板的事），再给列表留至少一行，剩下的才给底部那一段。
+/// 把标题下面那块分成三截：邀请码、列表、「我的电脑」。高度不够时先保
+/// 邀请码（码一位都不能丢），再给列表留至少一行，剩下的才给底部那一段。
 fn split(inner: Rect, prompt: usize, section: usize) -> (Rect, Rect, Rect) {
     let h = inner.height;
     let ph = (prompt.min(u16::MAX as usize) as u16).min(h);
@@ -1327,7 +1322,7 @@ mod tests {
     /// 没登录多电脑：底部只有一行灰字，不列名单、不画标题。
     #[test]
     fn not_logged_in_shows_a_single_gray_line_at_the_bottom() {
-        let term = draw_mesh(Some(mesh_view(false, &[], &[])), 80, 12);
+        let term = draw_mesh(Some(mesh_view(false, &[])), 80, 12);
         let r = rows(&term);
         assert_eq!(r[11], "多电脑未开启·运行dctlogin");
         assert!(!r[10].contains("我的电脑"), "{r:?}");
@@ -1365,7 +1360,6 @@ mod tests {
                 ("公司Windows", true, false),
                 ("家里Mac", true, true),
             ],
-            &[],
         );
         let r = rows(&draw_mesh(Some(v), 80, 12));
         assert_eq!(
@@ -1389,7 +1383,7 @@ mod tests {
             .enumerate()
             .map(|(i, n)| (*n, true, i == 0))
             .collect();
-        let r = rows(&draw_mesh(Some(mesh_view(true, &members, &[])), 80, 14));
+        let r = rows(&draw_mesh(Some(mesh_view(true, &members)), 80, 14));
         assert_eq!(r[13], "还有2台", "{r:?}");
         assert_eq!(r[7], "我的电脑");
         assert_eq!(
@@ -1399,72 +1393,55 @@ mod tests {
         assert!(!r.iter().any(|l| l.contains("A6")));
     }
 
-    /// 有电脑在等：标题下面一行黄字确认，带名字和数字；下面一行灰字提醒
-    /// 一次只加一台。80 列放得下整句。
-    #[test]
-    fn a_pending_join_shows_the_confirm_line_under_the_title() {
-        let v = mesh_view(
+    /// 发着邀请码：标题下面一行黄字（码、有效期、倒计时），再一行灰字说
+    /// 新电脑上敲什么；会话行被挤到下面。
+    fn with_code() -> crate::proto::MeshView {
+        let now = std::time::SystemTime::now()
+            .duration_since(std::time::UNIX_EPOCH)
+            .unwrap()
+            .as_secs();
+        let mut v = mesh_view(
             true,
-            &[("家里Mac", true, true)],
-            &[("公司Windows", "c-w", "123456")],
+            &[
+                ("家里Mac", true, true),
+                ("一个特别特别长的电脑名字啊啊啊", false, false),
+            ],
         );
-        let term = draw_mesh(Some(v), 80, 12);
+        v.invite = Some(crate::proto::InviteView {
+            id: 1,
+            code: "482913".into(),
+            expires_at: now + 300,
+        });
+        v
+    }
+
+    #[test]
+    fn a_live_code_shows_under_the_title_with_the_hint() {
+        let term = draw_mesh(Some(with_code()), 80, 12);
         let r = rows(&term);
-        assert_eq!(
-            r[1],
-            "一台叫公司Windows的电脑想加入「我的电脑」。它屏幕上的数字是123456吗？(y/n)"
-        );
-        assert_eq!(r[2], "一次只加一台新电脑：两台同时加入，可能互相批准进错组");
-        assert!(!r.iter().any(|l| l.contains("还有")));
+        assert!(r[1].starts_with("邀请码482913·10分钟内有效·还剩"), "{r:?}");
+        assert_eq!(r[2], "在新电脑上运行：dctjoin482913（码只能用一次）");
         let buf = term.backend().buffer();
         assert_eq!(
             buf.cell((0, 1)).unwrap().fg,
             crate::ui::theme_now().asking().fg.unwrap(),
-            "确认行是「等你拍板」那一档暖色"
+            "码那一行是「等你拍板」那一档暖色"
         );
-        // 会话行还在，被挤到下面
         assert!(r.iter().any(|l| l.contains("proj")));
     }
 
-    /// 等着的不止一台：只问最早的那台，后面说「还有 N 条」。
-    #[test]
-    fn several_pending_joins_are_asked_one_at_a_time() {
-        let v = mesh_view(
-            true,
-            &[("家里Mac", true, true)],
-            &[
-                ("甲", "c-a", "111111"),
-                ("乙", "c-b", "222222"),
-                ("丙", "c-c", "333333"),
-            ],
-        );
-        let c: String = rows(&draw_mesh(Some(v), 80, 12)).concat();
-        assert!(c.contains("一台叫甲的电脑") && c.contains("111111"), "{c}");
-        assert!(c.contains("还有2条"), "{c}");
-        assert!(!c.contains("222222") && !c.contains("333333"), "{c}");
-    }
-
-    /// 窄终端上折行，数字和 (y/n) 一个都不能丢；再窄再矮也不 panic。
+    /// 窄终端上折行，码一位都不能丢；矮到放不下时先保码；再窄再矮也不 panic。
     #[test]
     fn narrow_and_tiny_terminals_keep_the_code_and_never_panic() {
-        let v = mesh_view(
-            true,
-            &[
-                ("家里Mac", true, true),
-                ("一个特别特别长的电脑名字啊啊啊", false, false),
-            ],
-            &[("公司Windows", "c-w", "123456")],
-        );
+        let v = with_code();
         let c: String = rows(&draw_mesh(Some(v.clone()), 30, 20)).concat();
-        assert!(c.contains("123456") && c.contains("(y/n)"), "{c}");
-        // 矮到放不下：确认行先保，列表至少留一行，剩下的才给「我的电脑」。
-        let r = rows(&draw_mesh(Some(v.clone()), 80, 5));
-        assert!(r[1].contains("123456"), "{r:?}");
+        assert!(c.contains("482913"), "{c}");
+        let r = rows(&draw_mesh(Some(v.clone()), 80, 6));
+        assert!(r[1].contains("482913"), "{r:?}");
         assert!(r[3].contains("proj"), "列表还剩一行：{r:?}");
-        assert_eq!(r[4], "我的电脑");
         for (w, h) in [(80, 24), (30, 10), (30, 3), (10, 5), (1, 1), (2, 2), (0, 0)] {
             draw_mesh(Some(v.clone()), w, h);
-            draw_mesh(Some(mesh_view(false, &[], &[])), w, h);
+            draw_mesh(Some(mesh_view(false, &[])), w, h);
         }
     }
 
@@ -1476,7 +1453,7 @@ mod tests {
         let mut a = sess(1, &proj);
         a.activity = "x".repeat(200);
         app.set_sessions(vec![a, sess(2, &proj)]);
-        let mut v = mesh_view(true, &[("家里Mac", true, true)], &[]);
+        let mut v = mesh_view(true, &[("家里Mac", true, true)]);
         v.messages = [(1, 3)].into_iter().collect();
         app.mesh.view = Some(v);
         let mut term = Terminal::new(TestBackend::new(80, 12)).unwrap();
diff --git a/src/ui/grid.rs b/src/ui/grid.rs
index 301e7b0..84cdf29 100644
--- a/src/ui/grid.rs
+++ b/src/ui/grid.rs
@@ -388,31 +388,6 @@ pub(crate) fn draw(f: &mut Frame, area: Rect, app: &mut App) {
         View::Grid { focus, reply } => (*focus, reply.clone()),
         _ => return,
     };
-    // 有电脑在等批准：顶上暖色提醒回看板（y/n 只在看板上接）。整句折行、
-    // 不截。
-    //
-    // 格子让出这几行**只在让得起的时候**：80×24 下内容区正好是 `MIN_ROWS`，
-    // 切掉两行整个九宫格就换成「窗口太小」（同 `draw_reply` 为什么是盖上去
-    // 的）。让不起就先画格子，再把提醒**盖**在最上面几行——压掉的是第一排
-    // 格子的上边框和开头几行，格子本身还在。
-    //
-    // 回复框开着的时候不提醒：那时按 `g` 是往框里打字，这句话就说错了。
-    let notice = if draft.is_none() {
-        super::computers::grid_notice_lines(&app.mesh, app.lang, area.width as usize)
-    } else {
-        Vec::new()
-    };
-    let nh = (notice.len().min(u16::MAX as usize) as u16).min(area.height);
-    let notice_area = Rect { height: nh, ..area };
-    let area = if area.height.saturating_sub(nh) >= MIN_ROWS {
-        Rect {
-            y: area.y + nh,
-            height: area.height - nh,
-            ..area
-        }
-    } else {
-        area
-    };
     let visible = app.grid_sessions();
     draw_grid(
         f,
@@ -426,10 +401,6 @@ pub(crate) fn draw(f: &mut Frame, area: Rect, app: &mut App) {
         },
         !app.sessions.is_empty(),
     );
-    if nh > 0 {
-        f.render_widget(ratatui::widgets::Clear, notice_area);
-        f.render_widget(Paragraph::new(notice), notice_area);
-    }
     if let Some(draft) = draft {
         let who = visible
             .iter()
```

- [ ] **Step 4: 实现——删 `sas.rs`、整份替换两个文件**

```bash
git rm crates/dct-mesh/src/sas.rs
```

`src/mesh/group.rs` 整份换成：

```rust
//! 要跟别的电脑说话的那几件事：看一眼现状、移除，以及加电脑（`invite`）
//! 用到的签名单、广播。
//!
//! 全都是「先在锁里算好、放锁、再发」：**调用 `Net` 的时候绝不攥着 `Mesh`
//! 那把锁**（见 `net` 模块头）。`FakeHub` 上发出去是同步调到对方的
//! `on_envelope`，对方要是回头调到我，攥着锁就是死锁；真网络上一次 `ask`
//! 能挂好几秒，那段时间里收件的轮询线程也要这把锁。
use std::collections::HashSet;
use std::sync::Mutex;

use dct_mesh::roster::{self, Roster};
use dct_mesh::wire::{self, Payload};

use super::deliver::clean_name;
use super::net::Net;
use super::Mesh;
use crate::proto::{MemberView, MeshProblem, MeshView};

fn lock(m: &Mutex<Mesh>) -> std::sync::MutexGuard<'_, Mesh> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// 现状。`online` 来自 `net.peers()`，问不到就当谁都不在线。
pub fn view(mesh: &Mutex<Mesh>, net: &dyn Net, logged_in: bool) -> MeshView {
    let online: HashSet<String> = net.peers().unwrap_or_default().into_iter().collect();
    let mut m = lock(mesh);
    let me = m.me.endpoint.clone();
    let members = m
        .roster
        .as_ref()
        .map(|r| {
            r.roster
                .members
                .iter()
                .map(|x| MemberView {
                    name: clean_name(&x.name),
                    endpoint: x.endpoint.clone(),
                    online: x.endpoint == me || online.contains(&x.endpoint),
                    is_me: x.endpoint == me,
                })
                .collect()
        })
        .unwrap_or_default();
    MeshView {
        logged_in,
        name: clean_name(&m.me.name),
        endpoint: me,
        in_group: m.roster.is_some(),
        members,
        messages: m.delivered_counts(),
        // 先 `invite_view`：它顺手把到点的码清掉、记下结果，下面读到的
        // `invite_note` 才是新的。
        invite: m.invite_view(),
        invite_note: m.invite_note.clone(),
    }
}

/// `Mac` 撞了名就是 `Mac 2`，再撞 `Mac 3`……名字本来就接近上限
/// （`roster::MAX_NAME_LEN` 个字）的，先截短再编号，编出来的还是合法名字。
pub(super) fn free_name(base: &str, taken: &[String]) -> String {
    (2..)
        .map(|n| {
            let suffix = format!(" {n}");
            let keep = dct_mesh::roster::MAX_NAME_LEN - suffix.chars().count();
            let head: String = base.chars().take(keep).collect();
            format!("{}{suffix}", head.trim_end())
        })
        .find(|c| !taken.contains(c))
        .unwrap_or_default()
}

/// 把一台电脑移出组。`who` 是电脑名或端点。不能是自己：名单规定签名者不能
/// 在自己签的那一版里把自己删掉，要走也得由别的电脑来移。
///
/// 新名单只发给留下来的；被移出的那台发来的东西，从这一版起在每台留下的
/// 电脑上都验不过（`seal::open` 按名单找发件人）。
pub fn remove(mesh: &Mutex<Mesh>, net: &dyn Net, who: &str) -> Result<String, MeshProblem> {
    let (name, payload, to) = {
        let mut m = lock(mesh);
        let current = m.roster.clone().ok_or(MeshProblem::NotLoggedIn)?;
        let target = current
            .roster
            .by_name(who)
            .or_else(|| current.roster.member(who))
            .cloned()
            .ok_or_else(|| MeshProblem::NoSuchMachine(who.to_string()))?;
        if target.endpoint == m.me.endpoint {
            return Err(MeshProblem::CannotRemoveSelf);
        }
        let members = current
            .roster
            .members
            .iter()
            .filter(|x| x.endpoint != target.endpoint)
            .cloned()
            .collect();
        let next = sign_next(&m, &current.roster, members);
        let to = recipients(&m, &next);
        m.commit(next.clone()).map_err(|_| MeshProblem::NotSaved)?;
        m.journal
            .mesh(&format!("removed endpoint={}", target.endpoint));
        (target.name, wire::encode(&Payload::Roster(next)), to)
    };
    broadcast(mesh, net, &to, &payload);
    Ok(name)
}

pub(super) fn sign_next(m: &Mesh, current: &Roster, members: Vec<dct_mesh::Member>) -> dct_mesh::SignedRoster {
    let next = Roster {
        group: current.group.clone(),
        version: current.version + 1,
        members,
    };
    roster::sign(next, &m.me, &m.keys)
}

pub(super) fn recipients(m: &Mesh, r: &dct_mesh::SignedRoster) -> Vec<String> {
    r.roster
        .members
        .iter()
        .filter(|x| x.endpoint != m.me.endpoint)
        .map(|x| x.endpoint.clone())
        .collect()
}

/// 发给每一台，不等答复。不在线的这一次收不到（第一步没有离线留言），
/// 记一行 journal。**调用时没攥着锁**，记 journal 时才短暂拿一下。
///
/// 并排发：一台卡到超时不拖着别的，整次广播只花一次 `send` 的时间
/// （`mesh::worst_case` 按这个算命令行该等多久）。
pub(super) fn broadcast(mesh: &Mutex<Mesh>, net: &dyn Net, to: &[String], payload: &[u8]) {
    let failed: Vec<(String, crate::link::LinkError)> = std::thread::scope(|s| {
        let hs: Vec<_> = to
            .iter()
            .map(|ep| s.spawn(move || (ep.clone(), net.send(ep, payload.to_vec()))))
            .collect();
        hs.into_iter()
            .filter_map(|h| h.join().ok())
            .filter_map(|(ep, r)| r.err().map(|e| (ep, e)))
            .collect()
    });
    for (ep, e) in failed {
        lock(mesh)
            .journal
            .mesh(&format!("roster_not_delivered to={ep} err={e:?}"));
    }
}

#[cfg(test)]
mod tests {
    use super::super::net::testing::{FakeHub, FakeNet};
    use super::*;
    use base64::{engine::general_purpose::STANDARD, Engine as _};
    use dct_link::{EndpointId, Envelope};
    use dct_mesh::seal::{self, Kind, Message};
    use dct_mesh::{MachineKeys, Member};
    use std::sync::Arc;
    use std::time::Duration;

    fn keys(b: u8) -> MachineKeys {
        MachineKeys::from_seeds([b; 32], [b.wrapping_add(100); 32]).unwrap()
    }

    struct Node {
        mesh: Arc<Mutex<Mesh>>,
        net: FakeNet,
        ep: String,
    }

    impl Node {
        fn new(hub: &Arc<FakeHub>, seed: u8, name: &str) -> Node {
            let mesh = Arc::new(Mutex::new(Mesh::new(keys(seed), name.into(), None)));
            hub.register(mesh.clone());
            let ep = mesh.lock().unwrap().endpoint().to_string();
            Node {
                net: hub.net_for(&ep),
                mesh,
                ep,
            }
        }

        /// `dct login` 在守护进程里做的那一半（令牌之外）：没有组就建一个。
        fn login(&self) -> bool {
            self.mesh.lock().unwrap().ensure_group().unwrap()
        }

        /// 我出一个码，`other` 拿它 `dct join`；新名单再发给组里其余的电脑
        /// （守护进程的投递线程做的那一下）。
        fn invite_in(&self, other: &Node) {
            let v = self.mesh.lock().unwrap().start_invite().unwrap();
            crate::mesh::invite::join(&other.mesh, &other.net, &v.code, None).unwrap();
            crate::mesh::invite::flush_outbox(&self.mesh, &self.net);
        }

        fn view(&self) -> MeshView {
            view(&self.mesh, &self.net, true)
        }

        fn roster(&self) -> dct_mesh::SignedRoster {
            self.mesh.lock().unwrap().roster.clone().unwrap()
        }

        fn names(&self) -> Vec<String> {
            let mut v: Vec<String> = self
                .roster()
                .roster
                .members
                .iter()
                .map(|m| m.name.clone())
                .collect();
            v.sort();
            v
        }

        fn me(&self) -> Member {
            self.mesh.lock().unwrap().me.clone()
        }
    }

    /// A 建组、用邀请码把 B 拉进来。
    fn ab(hub: &Arc<FakeHub>) -> (Node, Node) {
        let a = Node::new(hub, 1, "A");
        let b = Node::new(hub, 2, "B");
        assert!(a.login());
        assert!(b.login());
        a.invite_in(&b);
        (a, b)
    }

    #[test]
    fn login_makes_a_group_of_one_only_the_first_time() {
        let hub = FakeHub::new();
        let a = Node::new(&hub, 1, "A");
        assert!(a.login(), "第一次登录建组");
        assert!(!a.login(), "再登录不重建");
        let v = a.view();
        assert!(v.in_group);
        assert_eq!(v.members.len(), 1);
        assert!(v.members[0].is_me);
        assert_eq!(a.roster().roster.version, 1);
        assert_eq!(a.roster().roster.group, format!("mine-{}", a.ep));
    }

    /// 整条路：A 登录 → A 出码、B 加入 → B 出码、C 加入 → A 也收到 B 签的
    /// 新名单。组里任何一台都能发邀请。
    #[test]
    fn three_computers_join_one_after_another() {
        let hub = FakeHub::new();
        let (a, b) = ab(&hub);
        assert_eq!(a.names(), ["A", "B"]);
        assert_eq!(b.roster(), a.roster(), "B 从 A 那里拿到了同一份名单");
        assert_eq!(b.roster().roster.version, 2);
        let bv = b.view();
        assert_eq!(bv.members.len(), 2);
        assert!(bv.members.iter().all(|m| m.online));

        let c = Node::new(&hub, 3, "C");
        assert!(c.login());
        b.invite_in(&c);
        assert_eq!(b.names(), ["A", "B", "C"]);
        assert_eq!(a.roster(), b.roster(), "A 也收到了新名单");
        assert_eq!(c.roster(), b.roster());
        assert_eq!(a.roster().roster.version, 3);
        assert_eq!(a.roster().signer, b.ep);
    }

    /// 只有自己一台的电脑，不从网上接别人签的名单——哪怕名单里有它、签得
    /// 完全正确。进别人的组只有 `invite::join` 一条路。
    #[test]
    fn a_lone_machine_does_not_take_a_roster_off_the_wire() {
        let hub = FakeHub::new();
        let b = Node::new(&hub, 2, "B");
        let m = Node::new(&hub, 9, "M");
        b.login();
        m.login();
        let r = Roster {
            group: m.roster().roster.group.clone(),
            version: 2,
            members: vec![m.me(), b.me()],
        };
        let signed = roster::sign(r, &m.me(), &keys(9));
        m.net
            .send(&b.ep, wire::encode(&Payload::Roster(signed)))
            .unwrap();
        assert_eq!(b.names(), ["B"]);
    }

    /// 移出之后：名单上没有它；它再发来密封留言，一律丢掉。不能移除自己。
    #[test]
    fn a_removed_machine_is_dropped_and_self_removal_is_refused() {
        let hub = FakeHub::new();
        let (a, b) = ab(&hub);
        let c = Node::new(&hub, 3, "C");
        c.login();
        a.invite_in(&c);
        assert_eq!(b.names(), ["A", "B", "C"]);
        assert_eq!(c.names(), ["A", "B", "C"]);

        assert_eq!(
            remove(&a.mesh, &a.net, "A"),
            Err(MeshProblem::CannotRemoveSelf)
        );
        assert_eq!(
            remove(&a.mesh, &a.net, "Z"),
            Err(MeshProblem::NoSuchMachine("Z".into()))
        );

        assert_eq!(remove(&a.mesh, &a.net, "C").unwrap(), "C");
        assert_eq!(a.names(), ["A", "B"]);
        assert_eq!(b.roster(), a.roster(), "B 也收到了");
        assert_eq!(a.roster().roster.version, 4);

        // C 手上还是旧名单，照旧给 A 发一条密封留言：A 不认它了。
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();
        let msg = Message {
            id: "m1".into(),
            kind: Kind::Msg,
            from: c.ep.clone(),
            from_session: "#1".into(),
            to: a.ep.clone(),
            to_session: "#2".into(),
            body: "hi".into(),
            sent_at: now + 5,
        };
        let a_kx: [u8; 32] = STANDARD.decode(&a.me().kx_pub).unwrap().try_into().unwrap();
        let sealed = seal::seal(&msg, &keys(3), &a_kx, [7; 32]);
        let env = Envelope {
            from: EndpointId::new(c.ep.clone()).unwrap(),
            to: EndpointId::new(a.ep.clone()).unwrap(),
            seq: 1,
            payload: wire::encode(&Payload::Sealed(sealed)),
            recipients: vec![],
        };
        assert_eq!(a.mesh.lock().unwrap().on_envelope(&env), None);
        assert!(a.mesh.lock().unwrap().queues.is_empty());

        // 对照：拿「移出之前」的名单副本验同一条——证明丢掉是因为移出，
        // 不是因为这条留言本身造得不对。
        let mut before = Mesh::new(keys(1), "A".into(), Some(c.roster()));
        assert!(before.on_envelope(&env).is_some(), "移出之前这条是会收的");
    }

    /// 一台卡住不拖着别的：广播 6 台只花一次 `send` 的时间——
    /// `mesh::worst_case` 是这么算的。
    #[test]
    fn a_broadcast_takes_one_send_not_one_per_member() {
        struct Slow;
        impl Net for Slow {
            fn peers(&self) -> Result<Vec<String>, crate::link::LinkError> {
                Ok(vec![])
            }
            fn send(&self, _: &str, _: Vec<u8>) -> Result<(), crate::link::LinkError> {
                std::thread::sleep(Duration::from_millis(300));
                Ok(())
            }
            fn ask(
                &self,
                _: &str,
                _: Vec<u8>,
                _: Duration,
            ) -> Result<Vec<u8>, crate::link::LinkError> {
                Err(crate::link::LinkError::Unreachable)
            }
        }
        let mesh = Mutex::new(Mesh::new(keys(1), "A".into(), None));
        let to: Vec<String> = (0..6).map(|i| format!("c-{i:020x}")).collect();
        let t = std::time::Instant::now();
        broadcast(&mesh, &Slow, &to, b"x");
        assert!(
            t.elapsed() < Duration::from_millis(1200),
            "6 台花了 {:?}",
            t.elapsed()
        );
    }

    /// 名单里别的电脑的名字，交出去之前洗干净。
    #[test]
    fn the_view_hands_out_cleaned_names() {
        let ka = keys(1);
        let me = Mesh::new(keys(1), "A".into(), None).me.clone();
        let mut other = Mesh::new(keys(2), "B".into(), None).me.clone();
        other.name = "B\x1b[8m\u{202e}x".into();
        let r = Roster {
            group: "g".into(),
            version: 2,
            members: vec![me.clone(), other],
        };
        let signed = roster::sign(r, &me, &ka);
        let mesh = Mutex::new(Mesh::new(ka, "A".into(), Some(signed)));
        let hub = FakeHub::new();
        let v = view(&mesh, &hub.net_for(&me.endpoint), true);
        let names: Vec<&str> = v.members.iter().map(|m| m.name.as_str()).collect();
        assert!(names.contains(&"Bx"), "{names:?}");
    }

    /// 现状里带着发着的码；码用掉之后码没了、结果在。
    #[test]
    fn the_view_carries_the_live_code_and_then_its_outcome() {
        let hub = FakeHub::new();
        let a = Node::new(&hub, 1, "A");
        let b = Node::new(&hub, 2, "B");
        a.login();
        b.login();
        let v = a.mesh.lock().unwrap().start_invite().unwrap();
        assert_eq!(a.view().invite, Some(v.clone()));
        assert_eq!(a.view().invite_note, None);
        crate::mesh::invite::join(&b.mesh, &b.net, &v.code, None).unwrap();
        let after = a.view();
        assert_eq!(after.invite, None);
        assert_eq!(
            after.invite_note,
            Some(crate::proto::InviteNote {
                id: v.id,
                outcome: crate::proto::InviteOutcome::Joined { name: "B".into() }
            })
        );
        assert_eq!(b.view().invite, None, "B 那边没有码");
    }
}
```

`src/ui/computers.rs` 整份换成：

```rust
//! 看板上的「我的电脑」：顶上的邀请码（按 `a` 要来的）、底部那一段电脑
//! 名单、会话行末尾的「✉ N」。
//!
//! **绘制路径上不发任何请求。** `MeshStatus` 在守护进程那边要问一趟中转
//! （谁在线），可能卡好几秒；`MeshInvite` 虽然只在本机，也照样丢给后台
//! 线程，主循环每轮 `poll` 一次收结果（同 `pair_start_rx` 的做法）。画的
//! 时候只读 `MeshPanel` 里现成的那一份。
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver};
use std::time::{Duration, Instant};

use ratatui::prelude::*;

use super::app::App;
use super::view::View;
use super::widgets::{display_width, truncate, Msg};
use super::{dim, theme_now};
use crate::i18n::{msg, Lang};
use crate::proto::{InviteOutcome, MemberView, MeshView, Request, Response};

/// 多久问一次 `MeshStatus`。
pub(crate) const POLL_EVERY: Duration = Duration::from_secs(5);
/// 「我的电脑」最多列几台，多的只报个数。
pub(crate) const MAX_SHOWN: usize = 5;

type Reply = Result<Response, String>;

/// 看板上多电脑那一块的全部状态。
#[derive(Default)]
pub(crate) struct MeshPanel {
    /// 最近一次拿到的现状。`None` = 还没问到过（守护进程太老、刚启动），
    /// 这时候那一段整个不画，不猜。
    pub view: Option<MeshView>,
    status_rx: Option<Receiver<Reply>>,
    last_fetch: Option<Instant>,
    /// 按了 `a`、`MeshInvite` 还在飞。飞着的时候再按不再发第二条。
    invite_rx: Option<Receiver<Reply>>,
    /// 这个界面自己要来的最近一个码（`InviteView::id`）。只有它的结果才在
    /// 底栏说一句——别处（`dct invite`）要的码，那边自己会说。
    created: Option<u64>,
    /// 已经说过的那个结果（`InviteNote::id`），同一句不说两遍。
    announced: Option<u64>,
}

impl MeshPanel {
    /// 换上一份新的现状。
    pub(crate) fn set_view(&mut self, v: MeshView) {
        self.view = Some(v);
    }

    /// 这个会话这次运行期间收到过几条留言。
    pub(crate) fn messages_for(&self, session: u32) -> u32 {
        self.view
            .as_ref()
            .and_then(|v| v.messages.get(&session).copied())
            .unwrap_or(0)
    }
}

/// 看板上按 `a`：要一个新码（旧的由守护进程作废）。上一条还在飞就不再发。
pub(crate) fn start_invite(app: &mut App) {
    if app.mesh.invite_rx.is_some() {
        return;
    }
    app.mesh.invite_rx = Some(spawn_call(
        app.socket.clone(),
        Request::MeshInvite,
        crate::client::READ_TIMEOUT * 2,
    ));
}

/// 新拿到的现状里有这个界面要来的那个码的结果，还没说过：说一句。
fn announce(app: &mut App) {
    let Some(n) = app.mesh.view.as_ref().and_then(|v| v.invite_note.clone()) else {
        return;
    };
    if app.mesh.created != Some(n.id) || app.mesh.announced == Some(n.id) {
        return;
    }
    app.mesh.announced = Some(n.id);
    let m = match &n.outcome {
        InviteOutcome::Joined { name } => msg::mesh_member_joined(app.lang, &clean(name)).into(),
        InviteOutcome::Burned => Msg::err(msg::mesh_invite_burned_board(app.lang)),
        InviteOutcome::Expired => Msg::err(msg::mesh_invite_expired_board(app.lang)),
    };
    say(app, m);
}

/// 在后台线程里发一条请求，结果从返回的通道里取。
fn spawn_call(socket: PathBuf, req: Request, timeout: Duration) -> Receiver<Reply> {
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let r = crate::client::Client::connect(&socket)
            .and_then(|mut c| c.call_within(req, timeout))
            .map_err(|e| e.to_string());
        let _ = tx.send(r);
    });
    rx
}

/// 主循环每轮调一次：收后台的结果；到点了、又没有一条在飞，就再问一次。
/// 只在看板和九宫格上问——别的视图不画这一块（九宫格上也要问：码的结果
/// 在底栏说，人可能正停在九宫格上）。
pub(crate) fn poll(app: &mut App, now: Instant) {
    if let Some(rx) = &app.mesh.invite_rx {
        match rx.try_recv() {
            Ok(r) => {
                app.mesh.invite_rx = None;
                match r {
                    Ok(Response::MeshInvite(v)) => {
                        app.mesh.created = Some(v.id);
                        if let Some(view) = app.mesh.view.as_mut() {
                            view.invite = Some(v);
                        }
                        // 要码之前发出去的那次状态查询，回来的是没有这个码的
                        // 样子，扔掉；下一轮就重新问，别等 5 秒。
                        app.mesh.status_rx = None;
                        app.mesh.last_fetch = None;
                    }
                    Ok(Response::Error(e)) => say(app, Msg::err(msg::error(app.lang, &e))),
                    _ => say(
                        app,
                        Msg::err(msg::error(
                            app.lang,
                            &crate::proto::ErrorCode::DaemonNotResponding,
                        )),
                    ),
                }
            }
            Err(mpsc::TryRecvError::Empty) => {}
            Err(mpsc::TryRecvError::Disconnected) => app.mesh.invite_rx = None,
        }
    }
    if let Some(rx) = &app.mesh.status_rx {
        match rx.try_recv() {
            Ok(r) => {
                app.mesh.status_rx = None;
                // 问不到（断线、守护进程太老不认识这条）就留着手里那份。
                if let Ok(Response::Mesh(v)) = r {
                    app.mesh.set_view(v);
                    announce(app);
                }
            }
            Err(mpsc::TryRecvError::Empty) => {}
            Err(mpsc::TryRecvError::Disconnected) => app.mesh.status_rx = None,
        }
    }
    let due = app
        .mesh
        .last_fetch
        .is_none_or(|t| now.saturating_duration_since(t) >= POLL_EVERY);
    if due
        && app.connected
        && matches!(app.view, View::Board | View::Grid { .. })
        && app.mesh.status_rx.is_none()
    {
        app.mesh.last_fetch = Some(now);
        app.mesh.status_rx = Some(spawn_call(
            app.socket.clone(),
            Request::MeshStatus,
            crate::client::READ_TIMEOUT * 2,
        ));
    }
}

/// 晚到的那句话：人已经在会话里了就不说——会话视图的底栏是给 agent
/// 那一屏用的，一句「已加入」盖上去没人需要。
fn say(app: &mut App, m: Msg) {
    if !matches!(app.view, View::Attached(_)) {
        app.message = m;
    }
}

/// 别的电脑给的名字：跟命令行、标记行同一份洗法（`deliver::clean_name`）。
fn clean(s: &str) -> String {
    crate::mesh::deliver::clean_name(s)
}

/// 按显示宽度折行（中文占两列）。`width` 为 0 时什么都不出。
fn wrap(s: &str, width: usize) -> Vec<String> {
    if width == 0 {
        return Vec::new();
    }
    let mut out = Vec::new();
    let mut line = String::new();
    let mut w = 0;
    for ch in s.chars() {
        let cw = super::widgets::char_width(ch);
        if w + cw > width && !line.is_empty() {
            out.push(std::mem::take(&mut line));
            w = 0;
        }
        line.push(ch);
        w += cw;
    }
    if !line.is_empty() {
        out.push(line);
    }
    out
}

/// 顶部的邀请码：`邀请码 482 913 · 10 分钟内有效 · 还剩 9:41`（黄字，整句
/// 折行——码一位都不能被切掉），下面一行灰字说新电脑上敲什么。`now` 是
/// unix 秒；到点了就不画（现状最多晚 5 秒才知道它过期）。
pub(crate) fn invite_lines(panel: &MeshPanel, lang: Lang, width: usize, now: u64) -> Vec<Line<'static>> {
    let Some(v) = panel.view.as_ref().and_then(|v| v.invite.as_ref()) else {
        return Vec::new();
    };
    if now >= v.expires_at || v.code.len() != 6 {
        return Vec::new();
    }
    let spaced = format!("{} {}", &v.code[..3], &v.code[3..]);
    let line = format!(
        "{} · {}",
        msg::mesh_invite_code(lang, &spaced),
        msg::mesh_invite_countdown(lang, v.expires_at - now)
    );
    let style = theme_now().asking().add_modifier(Modifier::BOLD);
    let mut lines: Vec<Line> = wrap(&line, width)
        .into_iter()
        .map(|l| Line::from(Span::styled(l, style)))
        .collect();
    if width > 1 {
        lines.push(Line::from(Span::styled(
            truncate(&msg::mesh_invite_hint(lang, &v.code), width.saturating_sub(1)),
            dim(),
        )));
    }
    lines
}

/// 名单怎么排：本机最前，然后在线的，然后离线的；同一档里照名单原来的顺序。
fn ordered(members: &[MemberView]) -> Vec<&MemberView> {
    let mut v: Vec<&MemberView> = members.iter().collect();
    v.sort_by_key(|m| (!m.is_me, !m.online));
    v
}

/// 底部「我的电脑」那一段。还没问到过就是空的。
pub(crate) fn section_lines(panel: &MeshPanel, lang: Lang, width: usize) -> Vec<Line<'static>> {
    let Some(v) = &panel.view else {
        return Vec::new();
    };
    if width == 0 {
        return Vec::new();
    }
    if !v.logged_in {
        return vec![Line::from(Span::styled(
            truncate(&msg::mesh_board_off(lang), width.saturating_sub(1)),
            dim(),
        ))];
    }
    let mut lines = vec![Line::from(Span::styled(
        truncate(&msg::mesh_board_header(lang), width.saturating_sub(1)),
        dim(),
    ))];
    let members = ordered(&v.members);
    for m in members.iter().take(MAX_SHOWN) {
        let state = msg::mesh_board_state(lang, m.online, m.is_me);
        let (dot, dot_style) = if m.online {
            ("●", theme_now().idle())
        } else {
            ("○", dim())
        };
        // 名字能用多宽：整行减去圆点、两处空白和后面那两个字。
        let room = width.saturating_sub(2 + 2 + display_width(&state) + 1);
        lines.push(Line::from(vec![
            Span::styled(format!("{dot} "), dot_style),
            Span::raw(truncate(&clean(&m.name), room.saturating_sub(1))),
            Span::raw("  "),
            Span::styled(state, dim()),
        ]));
    }
    if members.len() > MAX_SHOWN {
        lines.push(Line::from(Span::styled(
            truncate(
                &msg::mesh_board_more(lang, members.len() - MAX_SHOWN),
                width.saturating_sub(1),
            ),
            dim(),
        )));
    }
    lines
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::proto::{InviteNote, InviteView, MeshView};
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use std::sync::{Arc, Mutex};

    /// 一份现状：`members` 是（名字, 在线, 本机）。
    pub(crate) fn mesh_view(logged_in: bool, members: &[(&str, bool, bool)]) -> MeshView {
        MeshView {
            logged_in,
            name: "家里Mac".into(),
            endpoint: "c-home".into(),
            in_group: logged_in,
            members: members
                .iter()
                .enumerate()
                .map(|(i, (n, online, me))| MemberView {
                    name: n.to_string(),
                    endpoint: format!("c-{i}"),
                    online: *online,
                    is_me: *me,
                })
                .collect(),
            messages: Default::default(),
            invite: None,
            invite_note: None,
        }
    }

    /// 假守护进程发的码在这一刻（unix 秒）过期。
    const INVITE_EXPIRES: u64 = 1_800_000_600;

    /// 假守护进程：每条连接一个线程（会话视图那条长连接不能把别的挡住），
    /// 记下收到的每一条请求。`MeshStatus` 回 `status`；`MeshInvite` 回一个
    /// 固定的码，`id` 是第几次要。
    fn fake_daemon(sock: &std::path::Path, status: MeshView) -> Arc<Mutex<Vec<Request>>> {
        use std::io::{BufRead, BufReader, Write};
        let listener = crate::sys::ipc::bind_private(sock).unwrap();
        let got: Arc<Mutex<Vec<Request>>> = Default::default();
        let got2 = got.clone();
        std::thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                let (got, status) = (got2.clone(), status.clone());
                std::thread::spawn(move || {
                    let mut reader = BufReader::new(stream.try_clone().unwrap());
                    let mut writer = stream;
                    loop {
                        let mut line = String::new();
                        if reader.read_line(&mut line).unwrap_or(0) == 0 {
                            break;
                        }
                        let Ok(req) = serde_json::from_str::<Request>(&line) else {
                            break;
                        };
                        let answer = match &req {
                            Request::MeshStatus => Response::Mesh(status.clone()),
                            Request::MeshInvite => {
                                let n = got
                                    .lock()
                                    .unwrap()
                                    .iter()
                                    .filter(|r| matches!(r, Request::MeshInvite))
                                    .count() as u64;
                                Response::MeshInvite(InviteView {
                                    id: n + 1,
                                    code: "482913".into(),
                                    expires_at: INVITE_EXPIRES,
                                })
                            }
                            _ => Response::Ok,
                        };
                        got.lock().unwrap().push(req);
                        let s = serde_json::to_string(&answer).unwrap();
                        if writeln!(writer, "{s}").is_err() {
                            break;
                        }
                    }
                });
            }
        });
        got
    }

    fn count(got: &Arc<Mutex<Vec<Request>>>, f: fn(&Request) -> bool) -> usize {
        got.lock().unwrap().iter().filter(|r| f(r)).count()
    }

    fn invites(got: &Arc<Mutex<Vec<Request>>>) -> usize {
        count(got, |r| matches!(r, Request::MeshInvite))
    }

    fn statuses(got: &Arc<Mutex<Vec<Request>>>) -> usize {
        count(got, |r| matches!(r, Request::MeshStatus))
    }

    /// 等到 `cond` 成立，最多 5 秒；每轮跑一次 `poll`（主循环就是这么收的）。
    fn wait_until(app: &mut App, mut cond: impl FnMut(&App) -> bool) {
        let deadline = Instant::now() + Duration::from_secs(5);
        while !cond(app) {
            assert!(Instant::now() < deadline, "等了 5 秒没等到");
            poll(app, Instant::now());
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    fn key(c: char) -> KeyEvent {
        KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE)
    }

    fn board_app(status: MeshView) -> (App, tempfile::TempDir, Arc<Mutex<Vec<Request>>>) {
        let (mut app, dir) = App::test_app();
        let got = fake_daemon(&app.socket, status.clone());
        app.connected = true;
        app.view = View::Board;
        app.mesh.set_view(status);
        (app, dir, got)
    }

    fn text_of(lines: &[Line]) -> String {
        lines
            .iter()
            .map(|l| l.spans.iter().map(|s| s.content.as_ref()).collect::<String>())
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn one_computer() -> MeshView {
        mesh_view(true, &[("家里Mac", true, true)])
    }

    /// 每 5 秒问一次，而且只在看板上问；一次在飞的时候不再发第二次。
    #[test]
    fn status_is_asked_every_five_seconds_and_only_on_the_board() {
        let status = one_computer();
        let (mut app, _d, got) = board_app(status.clone());
        app.mesh.view = None;
        let t0 = Instant::now();
        poll(&mut app, t0);
        poll(&mut app, t0);
        let deadline = Instant::now() + Duration::from_secs(5);
        while app.mesh.view.is_none() {
            assert!(Instant::now() < deadline);
            poll(&mut app, t0 + Duration::from_secs(1));
            std::thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(app.mesh.view, Some(status));
        assert_eq!(statuses(&got), 1);
        poll(&mut app, t0 + POLL_EVERY - Duration::from_millis(1));
        std::thread::sleep(Duration::from_millis(100));
        assert_eq!(statuses(&got), 1, "不到 5 秒不问");

        app.view = View::Attached(1);
        poll(&mut app, t0 + POLL_EVERY * 3);
        std::thread::sleep(Duration::from_millis(100));
        assert_eq!(statuses(&got), 1, "会话视图里不问");

        app.view = View::Board;
        poll(&mut app, t0 + POLL_EVERY * 3);
        let deadline = Instant::now() + Duration::from_secs(5);
        while statuses(&got) < 2 {
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    /// 上一次还没回来（中转慢）：到点了也不再发第二次，不越堆越多。
    #[test]
    fn no_second_status_request_while_one_is_in_flight() {
        let (mut app, _d, got) = board_app(mesh_view(true, &[]));
        let (_tx, rx) = mpsc::channel::<Reply>();
        app.mesh.status_rx = Some(rx);
        poll(&mut app, Instant::now() + POLL_EVERY * 2);
        std::thread::sleep(Duration::from_millis(100));
        assert_eq!(statuses(&got), 0);
    }

    /// 九宫格里也问：码的结果要在底栏说，人可能停在九宫格上。
    #[test]
    fn status_is_asked_in_the_grid_too() {
        let (mut app, _d, got) = board_app(mesh_view(true, &[]));
        app.view = View::grid(0);
        app.mesh.view = None;
        poll(&mut app, Instant::now());
        let deadline = Instant::now() + Duration::from_secs(5);
        while statuses(&got) < 1 {
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    /// 别的电脑给的名字里有双向控制符、零宽字符：画之前去掉。
    #[test]
    fn format_characters_in_a_remote_name_never_reach_the_screen() {
        let name = "\u{202E}公司\u{200B}Win\u{2066}";
        let mut panel = MeshPanel::default();
        panel.set_view(mesh_view(true, &[(name, true, false)]));
        let text: String = section_lines(&panel, Lang::Zh, 80)
            .into_iter()
            .flat_map(|l| l.spans.into_iter().map(|s| s.content.into_owned()))
            .collect();
        assert!(text.contains("公司Win"), "{text:?}");
        assert!(
            !text.chars().any(crate::mesh::deliver::is_format_char),
            "{text:?}"
        );
    }

    #[test]
    fn wrapping_counts_wide_characters_as_two_columns() {
        assert_eq!(wrap("一二三abc", 4), ["一二", "三ab", "c"]);
        assert_eq!(wrap("abc", 0), Vec::<String>::new());
        assert_eq!(wrap("一", 1), ["一"], "一个字都放不下也不能死循环");
    }

    /// 看板上按 `a`：要一个码，拿到之后顶上画出来，带倒计时和新电脑上敲什么。
    #[test]
    fn a_on_the_board_gets_a_code_and_shows_it_with_a_countdown() {
        let (mut app, _d, got) = board_app(one_computer());
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

    /// 窄屏：码那一行整句折行，一位数字都不切掉。
    #[test]
    fn a_narrow_board_wraps_the_code_line_instead_of_cutting_it() {
        let mut v = one_computer();
        v.invite = Some(InviteView {
            id: 1,
            code: "482913".into(),
            expires_at: 1_600,
        });
        let mut panel = MeshPanel::default();
        panel.set_view(v);
        let lines = invite_lines(&panel, Lang::Zh, 20, 1_000);
        let all: String = lines[..lines.len() - 1]
            .iter()
            .flat_map(|l| l.spans.iter().map(|s| s.content.to_string()))
            .collect();
        assert!(all.contains("482 913"), "{all:?}");
        assert!(lines.len() > 2, "20 列放不下一行");
    }

    /// 连按两下 `a`：第一条还在飞，第二下不发；回来之后再按才换新码。
    #[test]
    fn pressing_a_twice_quickly_asks_once() {
        let (mut app, _d, got) = board_app(one_computer());
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
        let mut v = one_computer();
        v.invite = Some(InviteView {
            id: 1,
            code: "012345".into(),
            expires_at: 1_000,
        });
        let mut panel = MeshPanel::default();
        panel.set_view(v);
        assert!(text_of(&invite_lines(&panel, Lang::Zh, 80, 999)).contains("012 345"));
        assert!(invite_lines(&panel, Lang::Zh, 80, 1_000).is_empty());
    }

    /// 这个界面要来的码有了结果：底栏说一句，只说一次；别处要来的码的结果不说。
    #[test]
    fn the_outcome_of_our_own_code_is_said_once() {
        use crate::proto::InviteOutcome::*;
        let cases = [
            (Joined { name: "公司电脑".into() }, "公司电脑 已加入"),
            (Burned, "有人用错码试过一次，码已作废，按 a 重新生成"),
            (Expired, "邀请码过期了，按 a 重新生成"),
        ];
        for (outcome, want) in cases {
            let mut v = one_computer();
            v.invite_note = Some(InviteNote { id: 7, outcome });
            let (mut app, _d, _got) = board_app(one_computer());
            app.mesh.created = Some(7);
            app.mesh.set_view(v.clone());
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

    /// 结果晚到、人已经进了会话：不往会话的底栏上说话。
    #[test]
    fn a_late_outcome_does_not_talk_over_the_session_view() {
        let mut v = one_computer();
        v.invite_note = Some(InviteNote {
            id: 1,
            outcome: crate::proto::InviteOutcome::Joined { name: "B".into() },
        });
        let (mut app, _d, _got) = board_app(v);
        app.mesh.created = Some(1);
        app.view = View::Attached(1);
        app.message = "会话里的话".into();
        announce(&mut app);
        assert_eq!(app.message.text, "会话里的话");
    }

    /// 会话视图里 `a` 归 agent：不要码。
    #[test]
    fn a_in_the_session_view_asks_for_no_code() {
        let (mut app, _d, got) = board_app(one_computer());
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
        assert!(
            got.lock()
                .unwrap()
                .iter()
                .any(|r| matches!(r, Request::Input { .. })),
            "a 该送进会话里"
        );
    }

    /// 没登录多电脑时按 `a`：守护进程说没登录，底栏照说。
    #[test]
    fn a_before_login_says_to_log_in() {
        let (mut app, _d, _got) = board_app(mesh_view(false, &[]));
        app.mesh.invite_rx = Some({
            let (tx, rx) = mpsc::channel();
            tx.send(Ok(Response::Error(crate::proto::ErrorCode::Mesh(
                crate::proto::MeshProblem::NotLoggedIn,
            ))))
            .unwrap();
            rx
        });
        poll(&mut app, Instant::now());
        assert_eq!(app.message.text, "还没登录多电脑，先运行 dct login");
        assert!(app.mesh.created.is_none());
    }
}
```

- [ ] **Step 5: 跑测试确认通过，并确认旧名字一个不剩**

```bash
~/.cargo/bin/cargo test --workspace -- --test-threads=4
grep -rn -E "sas::|JoinRequest|JoinPending|JoinReveal|MeshApprove|MeshConfirmInviter|PendingJoin|pending_joins|take_reveal|confirm_inviter|SETTLE|grid_notice|peers approve|dct-sas" src crates tests | grep -v "^src/proto.rs:1[2-4][0-9]:"
```

Expected: 测试全绿；`grep` 只剩两行注释，都允许留着：`src/mesh/net.rs` 里讲超时的那段历史注释（「一次 `dct peers approve` 要是挂 40 秒」），和 `src/mesh/cli.rs` 里 `peers_approve_is_gone` 的文档注释。（`src/proto.rs` 第 120–149 行是协议版本的历史注释，已经排除。）

- [ ] **Step 6: 端到端再跑一遍**

Run: `~/.cargo/bin/cargo test --test mesh_e2e -- --ignored --nocapture`
Expected: `... ok`。

- [ ] **Step 7: 全量检查**

```bash
~/.cargo/bin/cargo test --workspace -- --test-threads=4
~/.cargo/bin/cargo clippy --workspace --all-targets -- -D warnings
~/.cargo/bin/cargo check --workspace --all-targets --target x86_64-pc-windows-msvc
```
Expected：`test result: ok.` 全绿（`pty::tests::*` / `session::tests::*` 偶发失败见 Global Constraints 的「已知 flaky」，单独重跑）；clippy `Finished`、无 warning；Windows check `Finished`（`src/student_projects.rs:669` 那条 `unused variable: path` 是本来就有的）。

- [ ] **Step 8: Commit**

```bash
git add crates/dct-mesh/src/sas.rs crates/dct-mesh/src/lib.rs crates/dct-mesh/src/wire.rs crates/dct-mesh/src/roster.rs crates/dct-mesh/src/keys.rs src/mesh/mod.rs src/mesh/group.rs src/mesh/cli.rs src/proto.rs src/daemon.rs src/main.rs src/ui/computers.rs src/ui/board.rs src/ui/grid.rs src/i18n.rs
git commit -m "refactor(mesh): drop the dct-sas-v2 compare-and-approve join; the invite code is the only way in" -m "Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---
