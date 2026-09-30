### Task 2: 线上形状——8 种邀请码消息

**Files:**
- Modify: `crates/dct-mesh/src/wire.rs`（`Payload` 加 8 个变体，加 `encode_bytes`/`decode_len`；旧的 `Join`/`JoinPending`/`JoinReveal` 这一步**不删**，Task 8 删）
- Modify: `src/mesh/mod.rs`（`Mesh::step` 的 `match` 要穷尽：新变体先一律丢掉，Task 3 接上）

**Interfaces:**
- Consumes: Task 1 无（只用已有的 `Member`、`SignedRoster`）。
- Produces（`dct_mesh::wire::`）：
  - `Payload::InviteProbe`（`{"t":"invite_probe"}`）
  - `Payload::InviteOpen { member: Member, sig: String, group: String }`
  - `Payload::NoInvite`
  - `Payload::InviteJoin { member: Member, sig: String, spake: String /*33 字节 b64*/ }`
  - `Payload::InviteKey { spake: String /*33 字节*/, confirm: String /*32 字节*/ }`
  - `Payload::InviteFinish { confirm: String /*32 字节*/ }`
  - `Payload::InviteDone { roster: SignedRoster }`
  - `Payload::InviteFailed`
  - `wire::encode_bytes(b: &[u8]) -> String`、`wire::decode_len(s: &str, len: usize) -> Option<Vec<u8>>`
  - 沿用：`wire::sign_member(m: &Member, keys: &MachineKeys) -> String`、`wire::verify_member(m: &Member, sig: &str) -> bool`（`dct-join-v1` 自签名，`InviteOpen`/`InviteJoin` 都用它）。

- [ ] **Step 1: 写失败的测试**

在 `crates/dct-mesh/src/wire.rs` 的测试模块里、`payloads_round_trip_through_encode_and_decode` 之前加：

```rust
    /// 邀请码那 8 种的线上形状。改了就是跟别的版本的 dct 说不上话了。
    #[test]
    fn invite_payload_shapes_are_pinned() {
        let k = keys_for(1);
        let m = member_from(&k, "A");
        let r = SignedRoster {
            roster: Roster {
                group: "g".into(),
                version: 2,
                members: vec![m.clone()],
            },
            signer: m.endpoint.clone(),
            sig: "s".into(),
        };
        let cases = [
            (Payload::InviteProbe, r#"{"t":"invite_probe"}"#.to_string()),
            (Payload::NoInvite, r#"{"t":"no_invite"}"#.to_string()),
            (Payload::InviteFailed, r#"{"t":"invite_failed"}"#.to_string()),
            (
                Payload::InviteKey {
                    spake: "m".into(),
                    confirm: "c".into(),
                },
                r#"{"t":"invite_key","spake":"m","confirm":"c"}"#.to_string(),
            ),
            (
                Payload::InviteFinish { confirm: "c".into() },
                r#"{"t":"invite_finish","confirm":"c"}"#.to_string(),
            ),
        ];
        for (p, want) in &cases {
            assert_eq!(&serde_json::to_string(p).unwrap(), want);
            assert_eq!(&decode(want.as_bytes()).unwrap(), p);
        }
        for (p, tag) in [
            (
                Payload::InviteOpen {
                    member: m.clone(),
                    sig: "s".into(),
                    group: "g".into(),
                },
                r#""t":"invite_open""#,
            ),
            (
                Payload::InviteJoin {
                    member: m.clone(),
                    sig: "s".into(),
                    spake: "x".into(),
                },
                r#""t":"invite_join""#,
            ),
            (Payload::InviteDone { roster: r }, r#""t":"invite_done""#),
        ] {
            let json = serde_json::to_string(&p).unwrap();
            assert!(json.contains(tag), "{json}");
            assert_eq!(decode(json.as_bytes()).unwrap(), p);
        }
    }

    #[test]
    fn byte_fields_round_trip_only_at_their_exact_length() {
        let b = [7u8; 33];
        assert_eq!(decode_len(&encode_bytes(&b), 33), Some(b.to_vec()));
        assert_eq!(decode_len(&encode_bytes(&b), 32), None);
        assert_eq!(decode_len("not base64!!", 33), None);
        assert_eq!(decode_len("", 0), Some(vec![]));
    }
```

- [ ] **Step 2: 跑测试确认失败**

Run: `~/.cargo/bin/cargo test -p dct-mesh wire`
Expected: 编译失败：`no variant or associated item named `InviteProbe``、`cannot find function `decode_len``。

- [ ] **Step 3: 实现**

先把 Step 1 里手改过的文件还原（补丁里已经包含同样的测试）：`git checkout -- crates/dct-mesh/src/wire.rs`。

把下面整个补丁存成 `/tmp/dct-invite-t2.patch`（**只存 ```diff 块里的内容**），在 worktree 根目录应用：

```bash
git apply --check /tmp/dct-invite-t2.patch && git apply /tmp/dct-invite-t2.patch
```

`--check` 报错说明前面的任务跟本计划的代码有出入：改用 `git apply --3way`，还不行就照补丁逐块手改（上下文行用来定位），**不要**跳过任何一块。

```diff
diff --git a/crates/dct-mesh/src/wire.rs b/crates/dct-mesh/src/wire.rs
index 27ce0f9..aa728d0 100644
--- a/crates/dct-mesh/src/wire.rs
+++ b/crates/dct-mesh/src/wire.rs
@@ -37,6 +37,43 @@ pub enum Payload {
     JoinReveal {
         nonce: String,
     },
+
+    // —— 邀请码（dct-invite-v1，见 `invite` 模块头）。全走 `ask`：B 问，A 当场答。
+
+    /// B → 每台同账号在线电脑：你手上有没有一个还有效的邀请码？不带任何东西，
+    /// 也不消耗码。
+    InviteProbe,
+    /// A 的答复：有。`member`/`sig` 是 A 自签的成员记录（`sign_member`），
+    /// `group` 是 A 的组 id——确认值 `T` 里要用，B 这时还没有 A 的名单。
+    InviteOpen {
+        member: Member,
+        sig: String,
+        group: String,
+    },
+    /// A 的答复：没有（没发过、过期了、用掉了、正有别人在用）。
+    NoInvite,
+    /// B → A：B 自签的成员记录，和 B 的 SPAKE2 消息（33 字节，标准 base64）。
+    InviteJoin {
+        member: Member,
+        sig: String,
+        spake: String,
+    },
+    /// A 的答复：A 的 SPAKE2 消息（33 字节）和确认值 `cA`（32 字节），都是标准 base64。
+    InviteKey {
+        spake: String,
+        confirm: String,
+    },
+    /// B → A：确认值 `cB`（32 字节，标准 base64）。
+    InviteFinish {
+        confirm: String,
+    },
+    /// A 的答复：把 B 签进去的新名单。
+    InviteDone {
+        roster: SignedRoster,
+    },
+    /// A 的答复：不行。**不说为什么**——码错、过期、已被用掉、签名不对，
+    /// 一律这一句（同 `Mesh::on_envelope` 的「验证失败都是沉默」）。
+    InviteFailed,
 }
 
 /// 新电脑请求加入组：`member` 是它自己的名单条目（还没被任何人签认），
@@ -63,6 +100,16 @@ pub fn decode32(s: &str) -> Option<[u8; 32]> {
     STANDARD.decode(s).ok()?.try_into().ok()
 }
 
+/// 任意字节（SPAKE2 消息、确认值）写成标准 base64。
+pub fn encode_bytes(b: &[u8]) -> String {
+    STANDARD.encode(b)
+}
+
+/// `encode_bytes` 的反面，而且必须正好 `len` 字节，否则 `None`。
+pub fn decode_len(s: &str, len: usize) -> Option<Vec<u8>> {
+    STANDARD.decode(s).ok().filter(|b| b.len() == len)
+}
+
 pub fn encode(p: &Payload) -> Vec<u8> {
     serde_json::to_vec(p).expect("Payload always serializes")
 }
@@ -186,6 +233,74 @@ mod tests {
         );
     }
 
+    /// 邀请码那 8 种的线上形状。改了就是跟别的版本的 dct 说不上话了。
+    #[test]
+    fn invite_payload_shapes_are_pinned() {
+        let k = keys_for(1);
+        let m = member_from(&k, "A");
+        let r = SignedRoster {
+            roster: Roster {
+                group: "g".into(),
+                version: 2,
+                members: vec![m.clone()],
+            },
+            signer: m.endpoint.clone(),
+            sig: "s".into(),
+        };
+        let cases = [
+            (Payload::InviteProbe, r#"{"t":"invite_probe"}"#.to_string()),
+            (Payload::NoInvite, r#"{"t":"no_invite"}"#.to_string()),
+            (Payload::InviteFailed, r#"{"t":"invite_failed"}"#.to_string()),
+            (
+                Payload::InviteKey {
+                    spake: "m".into(),
+                    confirm: "c".into(),
+                },
+                r#"{"t":"invite_key","spake":"m","confirm":"c"}"#.to_string(),
+            ),
+            (
+                Payload::InviteFinish { confirm: "c".into() },
+                r#"{"t":"invite_finish","confirm":"c"}"#.to_string(),
+            ),
+        ];
+        for (p, want) in &cases {
+            assert_eq!(&serde_json::to_string(p).unwrap(), want);
+            assert_eq!(&decode(want.as_bytes()).unwrap(), p);
+        }
+        for (p, tag) in [
+            (
+                Payload::InviteOpen {
+                    member: m.clone(),
+                    sig: "s".into(),
+                    group: "g".into(),
+                },
+                r#""t":"invite_open""#,
+            ),
+            (
+                Payload::InviteJoin {
+                    member: m.clone(),
+                    sig: "s".into(),
+                    spake: "x".into(),
+                },
+                r#""t":"invite_join""#,
+            ),
+            (Payload::InviteDone { roster: r }, r#""t":"invite_done""#),
+        ] {
+            let json = serde_json::to_string(&p).unwrap();
+            assert!(json.contains(tag), "{json}");
+            assert_eq!(decode(json.as_bytes()).unwrap(), p);
+        }
+    }
+
+    #[test]
+    fn byte_fields_round_trip_only_at_their_exact_length() {
+        let b = [7u8; 33];
+        assert_eq!(decode_len(&encode_bytes(&b), 33), Some(b.to_vec()));
+        assert_eq!(decode_len(&encode_bytes(&b), 32), None);
+        assert_eq!(decode_len("not base64!!", 33), None);
+        assert_eq!(decode_len("", 0), Some(vec![]));
+    }
+
     #[test]
     fn payloads_round_trip_through_encode_and_decode() {
         let k = keys_for(1);
diff --git a/src/mesh/mod.rs b/src/mesh/mod.rs
index 31f1502..a32bc9c 100644
--- a/src/mesh/mod.rs
+++ b/src/mesh/mod.rs
@@ -324,6 +324,15 @@ impl Mesh {
             // 读它），不该从轮询里进来。
             Payload::JoinPending { .. } => Step::Done(self.drop(env, "unasked_join_pending")),
             Payload::JoinReveal { nonce } => Step::Done(self.take_reveal(env, &nonce)),
+            // 邀请码那一路下一步才接上；在那之前一律当没听见。
+            Payload::InviteProbe
+            | Payload::InviteOpen { .. }
+            | Payload::NoInvite
+            | Payload::InviteJoin { .. }
+            | Payload::InviteKey { .. }
+            | Payload::InviteFinish { .. }
+            | Payload::InviteDone { .. }
+            | Payload::InviteFailed => Step::Done(self.drop(env, "invite_not_ready")),
         }
     }
```

- [ ] **Step 4: 跑测试确认通过**

Run: `~/.cargo/bin/cargo test -p dct-mesh wire`
Expected: `wire::tests::` 全过（新的两条在内）。

- [ ] **Step 5: 全量检查**

```bash
~/.cargo/bin/cargo test --workspace -- --test-threads=4
~/.cargo/bin/cargo clippy --workspace --all-targets -- -D warnings
~/.cargo/bin/cargo check --workspace --all-targets --target x86_64-pc-windows-msvc
```
Expected：`test result: ok.` 全绿（`pty::tests::*` / `session::tests::*` 偶发失败见 Global Constraints 的「已知 flaky」，单独重跑）；clippy `Finished`、无 warning；Windows check `Finished`（`src/student_projects.rs:669` 那条 `unused variable: path` 是本来就有的）。

- [ ] **Step 6: Commit**

```bash
git add crates/dct-mesh/src/wire.rs src/mesh/mod.rs
git commit -m "feat(mesh): wire shapes for the invite-code join (dct-invite-v1)" -m "Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---
