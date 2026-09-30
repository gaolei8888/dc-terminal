# dct 多电脑：用 6 位邀请码加电脑 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 老电脑按 `a`（或 `dct invite`）出一个一次性的 6 位邀请码，新电脑 `dct join 482913` 就进组；用 SPAKE2（dct-invite-v1）把码、两台电脑的身份绑进确认值，删掉旧的「两边核对 6 位数 + `dct peers approve`」流程（dct-sas-v2）。

**Architecture:**
- `crates/dct-mesh/src/invite.rs`（新）：纯计算——码的解析和拒绝采样、SPAKE2 包装（`Handshake`）、HKDF 出 kA/kB、HMAC 确认值 `cA`/`cB`（常数时间验）、`rec`/`T` 的拼法。不碰网络、时间、随机数。
- `crates/dct-mesh/src/wire.rs`：8 种新 `Payload`；最后一个任务删掉 `Join`/`JoinPending`/`JoinReveal` 和 `sas.rs`。
- `src/mesh/invite.rs`（新）：A 端码状态机 `Live → InFlight → 作废`（`impl Mesh` 里的 `start_invite`/`take_probe`/`take_invite_join`/`take_invite_finish`），B 端 `join()`，名单广播排进 `Mesh::outbox`、由守护进程投递线程 `flush_outbox`。
- 守护进程：`Request::MeshInvite`/`MeshInviteCancel`、`MeshJoin{code,name}`（没登录先自动登录），协议号 23 → 24；命令行 `dct invite` / `dct join <码>`；看板 `a` 出码、倒计时、结果提示。

**Tech Stack:** Rust 2021，全部纯 Rust：`spake2 =0.4.0`（RustCrypto，`default-features = false`，用 `Ed25519Group`，依赖的 `curve25519-dalek 4.1.3`、`hkdf 0.12.4`、`sha2 0.10.9`、`rand_core 0.6.4` 都已在 `Cargo.lock` 里）、`hmac 0.12.1`（已在 `Cargo.lock`，`verify_slice` 走 `subtle 2.6.1` 常数时间）、`rand_core 0.6`（给 `SeedRng` 实现 `RngCore + CryptoRng`）。`Cargo.lock` 只新增 `spake2 0.4.0` 一个包。

**Spec:** `docs/superpowers/specs/2026-09-30-dct-invite-code-design.md`（全文；第 2 段是字节级协议）。它替换 `docs/superpowers/specs/2026-09-28-dct-multi-machine-design.md` 里「加一台新电脑」「6 位数怎么算（dct-sas-v2）」两节。

## Global Constraints

- **在 git worktree 里做，别碰主工作区。** 主工作区（`/Users/lei/Documents/work/dc/dc-terminal`）有 7 个跟本计划无关的未提交改动：`src/daemon.rs`、`src/pty.rs`、`src/session.rs`、`src/ui/attach.rs`、`src/ui/mod.rs`、`src/ui/pick.rs`、`tests/concurrency.rs`。**绝不 stash、revert、add、checkout 它们。** 开 worktree：`git worktree add ../dc-terminal-invite -b feat/dct-invite-code docs/dct-invite-code`，之后所有命令都在 worktree 根目录跑。本计划会改到其中的 `src/daemon.rs`（只改多电脑那几段，跟主工作区的未提交改动不重叠），合并回去时由用户处理。
- **提交只 `git add` 列出的路径**，不许 `git add -A` / `git add .`。提交信息用英文，结尾带 `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`（每个任务的 commit 步骤已写好）。
- 所有依赖纯 Rust，不许有编 C 的 crate。每个任务结束时 `~/.cargo/bin/cargo check --workspace --all-targets --target x86_64-pc-windows-msvc` 必须过。这台机器上 Windows 标准库**目前没装**（2026-09-30 查过：`rustup target list --installed` 只有 `aarch64-apple-darwin`），第一次先跑 `~/.cargo/bin/rustup target add x86_64-pc-windows-msvc`（要联网）；装不上就在任务报告里写明「Windows check 没跑成」，不许谎报。
- `crates/dct-mesh` 不碰网络、磁盘、环境变量、时钟，**不自己拿随机数**：`spake2` 必须 `default-features = false`（默认的 `getrandom` 特性会让它自己去拿），随机数一律经 `Handshake::*` 的 `seed` / `Code::random` 的 `rand` 从调用方来（`dct` 里是 `mesh::os_rand`）。
- `spake2` 钉死 `=0.4.0`（没有审计报告，换版本要有意为之）。它跟 RFC 9382 不互通，固定向量只防我们自己的回归。
- 码（6 位数字）**不进任何日志、`Debug`、journal**：`Code`、`InviteView`、`Request::MeshJoin` 的 `Debug` 都打星号；journal 只记邀请的 `id`。确认值比较只走 `hmac` 的 `verify_slice`（常数时间），不许 `==`。
- 码的线上字节（spec 第 2 段）逐字节不许改：口令 `field("dct-invite-v1") ‖ field(6 个 ASCII 数字)`；`rec(m)` = `field(name) ‖ field(endpoint) ‖ field(sign_pub) ‖ field(kx_pub)`；info `"dct-invite-v1 confirm A"` / `"dct-invite-v1 confirm B"`；`T = field("dct-invite-v1") ‖ field(group) ‖ field_bytes(rec(A)) ‖ field_bytes(rec(B)) ‖ field_bytes(msgA) ‖ field_bytes(msgB)`。
- 常数（spec 原值）：码 10 分钟有效（`INVITE_TTL_SECS = 600`，`now >= expires` 即过期）；`InFlight` 后 30 秒内要收到 `InviteFinish`（`FINISH_WITHIN_SECS = 30`，`now > deadline` 才算晚）；探问上限沿用 `MAX_JOIN_ASK = 16`（超过 16 才拒）。
- 给人看的话中文（每句都有英文），不出现栈追踪；命令行出错退出码非 0。设计文档里给定的原句一字不差：`邀请码 482 913 · 10 分钟内有效`、`已加入「我的电脑」，组里有：Mac、公司电脑`、`邀请码不对或已作废，请在老电脑上按 a 重新生成`、`{名字} 已加入`。
- `cargo` 可能不在 PATH 上，一律写 `~/.cargo/bin/cargo`。macOS 上没有 `timeout(1)`。
- `cargo fmt --check` 在 main 上本来就不过；**不要**对整个 workspace 跑 `cargo fmt`。新建的文件可以 `~/.cargo/bin/rustfmt --edition 2021 <文件>`，改动的文件跟周围风格一致即可。
- **已知 flaky**：`pty::tests::*`、`session::tests::*` 里有几条在满负载下偶发失败（openpty 并发竞争，修复在主工作区未提交的 `src/pty.rs` 里，不在这个分支上）。全量跑用 `-- --test-threads=4`；还红就单独重跑那几条（`~/.cargo/bin/cargo test --lib -- pty::tests::<名字>`），过了就在报告里写一句「flaky，单独重跑通过」，别去改它们。
- 每个任务结束时 `cargo test --workspace`、`cargo clippy --workspace --all-targets -- -D warnings` 必须全过。

## 相对于 spec 的裁决（写计划时定的，执行时照此办）

1. **`group` 放进 `InviteOpen`。** spec 的 `T` 里有 `field(group_id)`，但 B 在收到 `InviteDone` 之前拿不到 A 的名单、也就不知道组 id。`InviteOpen{member, sig, group}` 把它带过来；它不在 A 的自签名里，但被 `cA`/`cB` 绑住：中转改了它，确认值就对不上。B 收名单时再核一次 `roster.group == group`。
2. **`field(msgA)` 这类字节字段用新加的 `canon::field_bytes`**（同一种「长度:内容」编码，对字符串跟 `field` 一模一样）。`msgA`/`msgB` 是库给的完整 33 字节（1 字节角色 + 32 字节点）。
3. **撞名一律由 A 编号**（`Mac` → `Mac 2`，沿用 `fd7d901` 的 `free_name`，并修成编号后仍不超过 32 个字）。`fd7d901` 里「`--name` 起的名字从不改」这条作废：A 不知道 B 的名字是不是用户起的，而 spec 要求撞名编号、不能失败。B 的命令行在名字被改时多印一行「这台电脑现在叫 Mac 2」。
4. **码作废 / 过期之后，探问答的是 `NoInvite`**，所以 B 此时看到的是「没有在线的电脑发出邀请（邀请码可能已过期或已作废）…」而不是「邀请码不对或已作废」。后者只在 B 真的跟某台走了 SPAKE2、没对上时出现。两句都提示去老电脑上按 `a`。
5. **「中转替换 B 的公钥」分两种情况测。** 就地改 B 的记录 → B 的自签名不成立，A 在进 `InFlight` 前就拒，**不作废**（符合 spec 的作废规则，改签名换不来码的信息）；中转换上自己完整的一条记录、从自己的端点发（= 抢先）→ 进 `InFlight`，作废。spec 测试清单那条「码作废」只对后一种成立。
6. **`InviteFinish` 不是从 `InFlight.peer` 来的：不回任何东西（`None`）**，不是 `InviteFailed`；状态不变。
7. **广播走 `Mesh::outbox`。** `on_envelope` 在锁里跑、拿不到 `Net`，所以 A 在 `InviteFinish` 里只把「发给其余成员的新名单」排进 `outbox`，守护进程的投递线程每秒调一次 `invite::flush_outbox` 放锁后发。其余成员最多晚 1 秒拿到新名单。
8. **协议号只加一次（24）。** Task 5 加新请求时加到 24；Task 8 删旧请求时改的是同一个 24 的形状（分支内部，中间没发版），文档注释里写明。
9. **B 逐台试最多 `MAX_INVITERS_TRIED = 3` 台，每次 ask 等 `INVITE_ASK_TIMEOUT = 5s`**（A 都是当场答）。`MeshJoin` 最坏要 15+8+8+5+30+8 = 74 秒，命令行 `MESH_CALL_WAIT` 从 60 秒放宽到 90 秒（守卫测试要求多等 10 秒以上）。
10. **SPAKE2 的随机数**：`SeedRng` = SHA-512(`field("dct-invite-v1 rng")` ‖ seed ‖ 计数器) 的流，种子是调用方的 32 字节系统随机数。
11. **A 已签进名单、B 却验不过 `InviteDone`**（只可能是中间人改包）：B 报 `MeshProblem::InviteRosterRefused`，提示在老电脑上 `dct peers remove` 再重来。A 那边此时名单里有 B——已知、写进文案。
12. **`dct invite` 的 Ctrl-C** 复用 `sys::signal::restore_terminal_when_killed`，钩子里另开一条连接发 `MeshInviteCancel`。必须在连上守护进程**之后**装：Unix 上它会屏蔽 SIGINT/SIGTERM，掩码会被子进程继承。`MeshInviteCancel` 不带 id，收回的是此刻的码。
13. **`a` 只绑看板**；九宫格不画码，但码的结果（已加入/作废/过期）在两个视图的底栏都会说。九宫格顶上那条「有电脑想加入」的提醒随旧流程一起删。
14. **`MeshView` 带着码**（`invite: Option<InviteView>`，只在本机 socket 上走）；另有 `invite_note: Option<InviteNote>` 记最近一个码怎么结束的。看板/`dct invite` 只报**自己要来的那个 id** 的结果。
15. **自动登录**：`MeshJoin` 在槽是空的（没登录）时先走 `mesh_login`，没配对 DC 账号就停在 `NoDcAccount`，不生成钥匙。

## Review Focus

spec 没明说、但真人最容易撞上的五件事（每条的测试已经加进对应任务）：

1. **码怎么敲进来**：`dct join 482 913`（shell 把空格拆成两个参数）、`482-913`、中文输入法的全角数字 `４８２９１３`、前导 0 的 `012345`——都该认，前导 0 不能丢。测试：Task 1 `typed_codes_ignore_spaces_dashes_and_full_width_forms`、Task 4 `a_code_typed_with_spaces_dashes_or_full_width_digits_still_works`、Task 5 `join_arguments`、Task 5 的 e2e 故意带空格敲码。
2. **码有效期间老电脑的守护进程重启了**（`dct restart`、升级）：码在内存里，没了；新电脑要得到「没有在线的电脑发出邀请…按 a 重新生成」，不能挂住、也不能进组。测试：Task 3 `a_restart_forgets_the_code`、Task 4 `an_expired_code_finds_no_invite`（同一条错误路径）。
3. **到点的边界**：第 599 秒还能用、第 600 秒作废；`InFlight` 第 30 秒送到还算、第 31 秒作废；看板上到点就不画码（现状最多晚 5 秒）。测试：Task 3 `a_code_expires_at_exactly_ten_minutes`、`a_finish_at_the_deadline_counts_and_one_second_later_burns`，Task 7 `an_expired_code_is_not_drawn`。
4. **两台 dct 版本不一样**：老电脑是旧版（不认识 `InviteProbe`，解不出来就丢、不回话）→ 新电脑要提示「可能太旧，先升级」；新命令碰上旧守护进程 → 协议号拦下、提示 `dct restart`。测试：Task 4 `an_old_dct_that_does_not_know_invite_probe_counts_as_unanswered`、Task 6 `every_mesh_command_stops_at_a_stale_daemon_with_the_restart_hint`（加了 `invite`）。
5. **连按两下 `a`、或者一边 `dct invite` 一边在看板上按 `a`**：请求在飞时第二下不发；码被换掉时 `dct invite` 要说「已经换成新的了」而不是一直等；看板只报自己要来的码的结果。测试：Task 7 `pressing_a_twice_quickly_asks_once`、`the_outcome_of_our_own_code_is_said_once`，Task 6 `invite_says_when_the_code_was_burned_expired_replaced_or_withdrawn`。

---

## 文件结构

| 文件 | 职责 | 任务 |
|---|---|---|
| `crates/dct-mesh/Cargo.toml` | 加 `spake2 =0.4.0`（无默认特性）、`hmac 0.12`、`rand_core 0.6` | 1 |
| `crates/dct-mesh/src/canon.rs` | 加 `field_bytes` | 1 |
| `crates/dct-mesh/src/invite.rs`（新） | 码、SPAKE2、确认值（纯计算） | 1 |
| `crates/dct-mesh/src/wire.rs` | 8 种新 `Payload`、`encode_bytes`/`decode_len`；删旧的 3 种和 `JoinRequest`/`encode32`/`decode32` | 2、8 |
| `crates/dct-mesh/src/sas.rs` | 删 | 8 |
| `crates/dct-mesh/src/{lib,roster,keys}.rs` | 模块出口、文档里的旧名字 | 1、8 |
| `src/mesh/invite.rs`（新） | A 端状态机、B 端 `join`、`flush_outbox` | 3、4 |
| `src/mesh/mod.rs` | `Mesh` 新字段、`step` 分派、`worst_case`；删旧的加入状态 | 2、3、5、8 |
| `src/mesh/group.rs` | `free_name`/`sign_next`/`recipients`/`broadcast` 给 `invite` 用；`view` 带码；删 `join`/`approve`/`confirm` | 3、5、8 |
| `src/mesh/net.rs` | 测试用的恶意中转 `EvilNet` | 3 |
| `src/proto.rs` | `InviteView`/`InviteNote`/`InviteOutcome`、新 `MeshProblem`、新请求、协议号 24；删旧请求 | 3、4、5、8 |
| `src/daemon.rs` | 接新请求、自动登录、投递线程里 `flush_outbox`；删旧请求 | 5、8 |
| `src/mesh/cli.rs` | `dct join <码>`、`dct invite`；删 `peers approve`、`--confirm`、问 y/n | 5、6、8 |
| `src/main.rs` | 路由 `invite`、帮助文字 | 6、8 |
| `src/ui/computers.rs`、`src/ui/board.rs`、`src/ui/keys.rs`、`src/ui/grid.rs` | 看板 `a`、码和倒计时、结果提示；删 y/n 确认行、`SETTLE`、九宫格提醒 | 7、8 |
| `src/i18n.rs` | 文案 | 4–8 |
| `tests/mesh_e2e.rs` | invite → join → peers → send | 5 |
| `README.md`、`README.zh-CN.md`、2026-09-28 spec | 文档 | 9 |

补丁（```diff 块）都是相对**上一个任务做完之后**的树生成的，并且整条链已经在一份 HEAD（`70546bb`）的干净副本上从头重放、编译、全量测试、clippy 过（2026-09-30）。照顺序做、别自己改格式，`git apply` 就能直接打上。

---
### Task 1: `dct-mesh` 邀请码的纯计算（码、SPAKE2、确认值）

**Files:**
- Modify: `crates/dct-mesh/Cargo.toml`（加依赖）
- Modify: `crates/dct-mesh/src/canon.rs`（`field_bytes`）
- Modify: `crates/dct-mesh/src/lib.rs`（`pub mod invite;`）
- Create: `crates/dct-mesh/src/invite.rs`
- `Cargo.lock` 会被 cargo 自动加上 `spake2 0.4.0`（跟着提交）

**Interfaces:**
- Consumes: `dct_mesh::canon::field(out: &mut Vec<u8>, s: &str)`、`dct_mesh::roster::Member { name, endpoint, sign_pub, kx_pub, added_at }`。
- Produces（`dct_mesh::...`）：
  - `canon::field_bytes(out: &mut Vec<u8>, b: &[u8])`
  - `invite::INVITE_VERSION_TAG: &str = "dct-invite-v1"`、`invite::CODE_DIGITS: usize = 6`、`invite::MSG_LEN: usize = 33`、`invite::TAG_LEN: usize = 32`
  - `invite::Code`（`Clone, PartialEq, Eq`，`Debug` 打 `Code(******)`）：`Code::parse(input: &str) -> Option<Code>`、`Code::random(rand: &mut dyn FnMut() -> [u8; 32]) -> Code`、`Code::as_str(&self) -> &str`、`Code::spaced(&self) -> String`
  - `invite::rec(m: &Member) -> Vec<u8>`、`invite::password(code: &Code) -> Vec<u8>`、`invite::transcript(group: &str, a: &Member, b: &Member, msg_a: &[u8], msg_b: &[u8]) -> Vec<u8>`
  - `invite::InviteError::BadMessage`
  - `invite::Handshake`：`Handshake::inviter(code: &Code, a: &Member, b: &Member, seed: [u8; 32]) -> Handshake`、`Handshake::joiner(...)` 同签名、`fn message(&self) -> &[u8]`、`fn finish(self, group: &str, theirs: &[u8]) -> Result<Confirmed, InviteError>`
  - `invite::Confirmed`（无 `Debug`）：`inviter_tag(&self) -> [u8; 32]`（cA）、`joiner_tag(&self) -> [u8; 32]`（cB）、`inviter_tag_ok(&self, tag: &[u8]) -> bool`、`joiner_tag_ok(&self, tag: &[u8]) -> bool`
- `spake2 0.4.0` 真实 API（读过 `~/.cargo/registry/src/*/spake2-0.4.0/src/lib.rs`）：`Spake2::<Ed25519Group>::start_a_with_rng(&Password::new(pw), &Identity::new(&id_a), &Identity::new(&id_b), rng) -> (Spake2<_>, Vec<u8>)`，`start_b_with_rng` 同签名，`Spake2::finish(self, msg2: &[u8]) -> spake2::Result<Vec<u8>>`（32 字节 K；身份 `idA`/`idB` 被哈希进 K；角色字节 `'A'`/`'B'` 不对 → `BadSide`，长度不对 → `WrongLength`，不是点 → `CorruptMessage`）。

- [ ] **Step 1: 依赖和 `field_bytes`、`lib.rs` 出口**

把下面整个补丁存成 `/tmp/dct-invite-t1.patch`（**只存 ```diff 块里的内容**），在 worktree 根目录应用：

```bash
git apply --check /tmp/dct-invite-t1.patch && git apply /tmp/dct-invite-t1.patch
```

`--check` 报错说明前面的任务跟本计划的代码有出入：改用 `git apply --3way`，还不行就照补丁逐块手改（上下文行用来定位），**不要**跳过任何一块。

```diff
diff --git a/crates/dct-mesh/Cargo.toml b/crates/dct-mesh/Cargo.toml
index 900e492..c05b836 100644
--- a/crates/dct-mesh/Cargo.toml
+++ b/crates/dct-mesh/Cargo.toml
@@ -14,3 +14,9 @@ p256 = { version = "0.13", default-features = false, features = ["ecdsa", "std"]
 x25519-dalek = { version = "2", features = ["static_secrets"] }
 chacha20poly1305 = { version = "0.10", default-features = false, features = ["alloc"] }
 hkdf = "0.12"
+# 邀请码的 PAKE（dct-invite-v1）。钉死版本：0.4.0 没有审计报告，换版本要有意为之。
+# 关掉默认特性：默认的 getrandom 会让它自己去拿系统随机数，而这个 crate 的
+# 随机数一律由调用方给（种子经 `invite::SeedRng` 喂进去）。
+spake2 = { version = "=0.4.0", default-features = false }
+hmac = "0.12"
+rand_core = { version = "0.6", default-features = false }
diff --git a/crates/dct-mesh/src/canon.rs b/crates/dct-mesh/src/canon.rs
index 22b3c93..40d6194 100644
--- a/crates/dct-mesh/src/canon.rs
+++ b/crates/dct-mesh/src/canon.rs
@@ -4,9 +4,15 @@
 //! `dct-common`——为几行代码去拉一个新的共享 crate 不值得。带长度前缀，所以
 //! 字段里有什么字符都不会跟相邻字段粘在一起被误读。
 pub fn field(out: &mut Vec<u8>, s: &str) {
-    out.extend_from_slice(s.len().to_string().as_bytes());
+    field_bytes(out, s.as_bytes());
+}
+
+/// 同 `field`，但字段是任意字节（SPAKE2 的消息、拼好的成员记录）。对一个
+/// 字符串，`field(s)` 和 `field_bytes(s.as_bytes())` 写出来的一模一样。
+pub fn field_bytes(out: &mut Vec<u8>, b: &[u8]) {
+    out.extend_from_slice(b.len().to_string().as_bytes());
     out.push(b':');
-    out.extend_from_slice(s.as_bytes());
+    out.extend_from_slice(b);
 }
 
 #[cfg(test)]
@@ -25,4 +31,17 @@ mod tests {
         assert_eq!(a, b"2:ab1:c");
         assert_eq!(b, b"1:a2:bc");
     }
+
+    #[test]
+    fn field_bytes_is_field_for_strings_and_takes_any_bytes() {
+        let mut a = Vec::new();
+        field(&mut a, "公司");
+        let mut b = Vec::new();
+        field_bytes(&mut b, "公司".as_bytes());
+        assert_eq!(a, b);
+        assert_eq!(a, "6:公司".as_bytes());
+        let mut c = Vec::new();
+        field_bytes(&mut c, &[0xff, 0x00, 0x3a]);
+        assert_eq!(c, [b'3', b':', 0xff, 0x00, 0x3a]);
+    }
 }
diff --git a/crates/dct-mesh/src/lib.rs b/crates/dct-mesh/src/lib.rs
index 2f79e66..f0fc119 100644
--- a/crates/dct-mesh/src/lib.rs
+++ b/crates/dct-mesh/src/lib.rs
@@ -7,6 +7,7 @@
 //! 被单元测试跑穷尽，不用起进程、不用碰真文件系统。
 pub mod canon;
 pub mod id;
+pub mod invite;
 pub mod keys;
 pub mod relay_token;
 pub mod roster;
```

- [ ] **Step 2: 写失败的测试**

建 `crates/dct-mesh/src/invite.rs`，先只放测试（整个 `#[cfg(test)]` 模块，原样）：

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use base64::{engine::general_purpose::STANDARD, Engine as _};

    fn member_a() -> Member {
        Member {
            name: "A".into(),
            endpoint: "c-aaaa".into(),
            sign_pub: STANDARD.encode([&[4u8][..], &[1u8; 64][..]].concat()),
            kx_pub: STANDARD.encode([2u8; 32]),
            added_at: 0,
        }
    }

    fn member_b() -> Member {
        Member {
            name: "B".into(),
            endpoint: "c-bbbb".into(),
            sign_pub: STANDARD.encode([&[4u8][..], &[3u8; 64][..]].concat()),
            kx_pub: STANDARD.encode([5u8; 32]),
            added_at: 0,
        }
    }

    fn code(s: &str) -> Code {
        Code::parse(s).unwrap()
    }

    const SA: [u8; 32] = [0x11; 32];
    const SB: [u8; 32] = [0x22; 32];

    /// 两边各走一遍，返回（msgA, msgB, A 那边的结果, B 那边的结果）。
    fn run(
        code_a: &str,
        code_b: &str,
        a_sees_b: &Member,
        b_sees_a: &Member,
        group_a: &str,
        group_b: &str,
    ) -> (Vec<u8>, Vec<u8>, Confirmed, Confirmed) {
        let (a, b) = (member_a(), member_b());
        let ha = Handshake::inviter(&code(code_a), &a, a_sees_b, SA);
        let hb = Handshake::joiner(&code(code_b), b_sees_a, &b, SB);
        let (ma, mb) = (ha.message().to_vec(), hb.message().to_vec());
        let ca = ha.finish(group_a, &mb).unwrap();
        let cb = hb.finish(group_b, &ma).unwrap();
        (ma, mb, ca, cb)
    }

    fn hex(b: &[u8]) -> String {
        b.iter().map(|x| format!("{x:02x}")).collect()
    }

    /// 回归向量：这份实现自己算出来的（`spake2` 0.4 跟 RFC 9382 不互通，没有
    /// 外部向量可对）。规范、依赖版本、种子伸长的方式任何一样变了，这条就红
    /// ——改的人得有意地一起改这四个值。
    #[test]
    fn messages_and_tags_match_the_fixed_vectors() {
        let (ma, mb, ca, cb) = run(
            "482913",
            "482913",
            &member_b(),
            &member_a(),
            "mine-c-aaaa",
            "mine-c-aaaa",
        );
        assert_eq!(
            hex(&ma),
            "41b9ae5bb06aebcb471712289ea0b04434180af656316b01e0d288ee17d06a27e5"
        );
        assert_eq!(
            hex(&mb),
            "4295d789c2cda9b9d0c2375835455fe7b86018bc2425cc116f974293e13196045e"
        );
        assert_eq!(
            hex(&ca.inviter_tag()),
            "550ad8912b2c50f067a69352d17d96f13ae511ea21c6bf67fb82dcb17d9fbb75"
        );
        assert_eq!(
            hex(&cb.joiner_tag()),
            "183cd869eb83b867bff70c3df3fce018c2cd7434f1641b4004eabc98c4b1368f"
        );
        assert_eq!(ma.len(), MSG_LEN);
        assert_eq!(ma[0], b'A');
        assert_eq!(mb[0], b'B');
    }

    #[test]
    fn the_password_and_record_bytes_are_pinned() {
        assert_eq!(password(&code("482913")), b"13:dct-invite-v16:482913");
        let m = Member {
            name: "A".into(),
            endpoint: "c-aaaa".into(),
            sign_pub: "sp".into(),
            kx_pub: "kx".into(),
            added_at: 99,
        };
        assert_eq!(rec(&m), b"1:A6:c-aaaa2:sp2:kx", "added_at 不算");
    }

    #[test]
    fn the_same_code_confirms_both_ways() {
        let (_, _, ca, cb) = run("482913", "482913", &member_b(), &member_a(), "g", "g");
        assert!(cb.inviter_tag_ok(&ca.inviter_tag()), "B 验得过 A 的 cA");
        assert!(ca.joiner_tag_ok(&cb.joiner_tag()), "A 验得过 B 的 cB");
        assert_eq!(ca.inviter_tag(), cb.inviter_tag(), "两边算出同一个 cA");
        assert_ne!(ca.inviter_tag(), ca.joiner_tag(), "cA 和 cB 用的是两把钥匙");
    }

    #[test]
    fn a_code_one_digit_off_fails_both_confirmations() {
        let (_, _, ca, cb) = run("482913", "482914", &member_b(), &member_a(), "g", "g");
        assert!(!cb.inviter_tag_ok(&ca.inviter_tag()));
        assert!(!ca.joiner_tag_ok(&cb.joiner_tag()));
        let (_, _, ca, cb) = run("012345", "112345", &member_b(), &member_a(), "g", "g");
        assert!(!cb.inviter_tag_ok(&ca.inviter_tag()));
        assert!(!ca.joiner_tag_ok(&cb.joiner_tag()));
    }

    /// 中转在中间换掉任何一台记录里的任何一项（名字、端点、任一把公钥），
    /// 两边绑定的身份就不一样，确认值对不上。
    #[test]
    fn swapping_any_field_of_either_record_fails_the_confirmation() {
        type Mutate = fn(&mut Member);
        let mutants: Vec<(&str, Mutate)> = vec![
            ("name", |m| m.name = "公司电脑".into()),
            ("endpoint", |m| m.endpoint = "c-xxxx".into()),
            ("sign_pub", |m| {
                m.sign_pub = STANDARD.encode([&[4u8][..], &[9u8; 64][..]].concat())
            }),
            ("kx_pub", |m| m.kx_pub = STANDARD.encode([9u8; 32])),
        ];
        for (what, f) in &mutants {
            // B 看到的 A 被换了。
            let mut fake_a = member_a();
            f(&mut fake_a);
            let (_, _, ca, cb) = run("482913", "482913", &member_b(), &fake_a, "g", "g");
            assert!(!cb.inviter_tag_ok(&ca.inviter_tag()), "A 的 {what}");
            assert!(!ca.joiner_tag_ok(&cb.joiner_tag()), "A 的 {what}");
            // A 看到的 B 被换了。
            let mut fake_b = member_b();
            f(&mut fake_b);
            let (_, _, ca, cb) = run("482913", "482913", &fake_b, &member_a(), "g", "g");
            assert!(!cb.inviter_tag_ok(&ca.inviter_tag()), "B 的 {what}");
            assert!(!ca.joiner_tag_ok(&cb.joiner_tag()), "B 的 {what}");
        }
    }

    #[test]
    fn a_different_group_fails_the_confirmation() {
        let (_, _, ca, cb) = run(
            "482913",
            "482913",
            &member_b(),
            &member_a(),
            "mine-a",
            "mine-x",
        );
        assert!(!cb.inviter_tag_ok(&ca.inviter_tag()));
        assert!(!ca.joiner_tag_ok(&cb.joiner_tag()));
    }

    /// 反射：把 A 自己的消息原样送回给 A，或者把一个加入方的消息送给另一个
    /// 加入方——角色字节不对，直接拒。长度不对、不是曲线上的点，也拒。
    #[test]
    fn reflected_wrong_length_and_garbage_messages_are_refused() {
        let (a, b) = (member_a(), member_b());
        let ha = Handshake::inviter(&code("482913"), &a, &b, SA);
        let own = ha.message().to_vec();
        assert!(matches!(ha.finish("g", &own), Err(InviteError::BadMessage)));

        let hb = Handshake::joiner(&code("482913"), &a, &b, SB);
        let other_b = Handshake::joiner(&code("482913"), &a, &b, [0x33; 32]);
        assert!(matches!(
            hb.finish("g", other_b.message()),
            Err(InviteError::BadMessage)
        ));

        for bad in [vec![], vec![b'B'; 32], vec![b'B'; 34]] {
            let ha = Handshake::inviter(&code("482913"), &a, &b, SA);
            assert!(matches!(ha.finish("g", &bad), Err(InviteError::BadMessage)));
        }
        // 角色字节对、长度对，但这 32 字节解压不出曲线上的点（y = 0x0202…02
        // 没有对应的 x）。
        let mut not_a_point = vec![b'B'];
        not_a_point.extend_from_slice(&[0x02; 32]);
        let ha = Handshake::inviter(&code("482913"), &a, &b, SA);
        assert!(matches!(
            ha.finish("g", &not_a_point),
            Err(InviteError::BadMessage)
        ));
    }

    /// 验确认值只走 `hmac` 的 `verify_slice`（常数时间），不拿 `==` 比。
    #[test]
    fn tags_are_verified_in_constant_time() {
        let src = include_str!("invite.rs");
        let body = &src[..src.find("#[cfg(test)]").unwrap()];
        assert_eq!(body.matches(".verify_slice(tag)").count(), 2);
        assert!(!body.contains("== tag") && !body.contains("tag =="));
    }

    #[test]
    fn a_truncated_or_empty_tag_is_refused() {
        let (_, _, ca, cb) = run("482913", "482913", &member_b(), &member_a(), "g", "g");
        let tag = ca.inviter_tag();
        assert!(!cb.inviter_tag_ok(&tag[..31]));
        assert!(!cb.inviter_tag_ok(&[]));
        let mut flipped = tag;
        flipped[31] ^= 1;
        assert!(!cb.inviter_tag_ok(&flipped));
    }

    #[test]
    fn different_seeds_give_different_messages() {
        let (a, b) = (member_a(), member_b());
        let x = Handshake::inviter(&code("482913"), &a, &b, SA);
        let y = Handshake::inviter(&code("482913"), &a, &b, [0x12; 32]);
        assert_ne!(x.message(), y.message());
        let again = Handshake::inviter(&code("482913"), &a, &b, SA);
        assert_eq!(
            x.message(),
            again.message(),
            "同一个种子同一条消息（向量靠这个）"
        );
    }

    #[test]
    fn typed_codes_ignore_spaces_dashes_and_full_width_forms() {
        for ok in [
            "482913",
            "482 913",
            " 482913 ",
            "482-913",
            "48 29 13",
            "４８２９１３",
            "４８２　９１３",
            "482－913",
            "\t482913",
        ] {
            assert_eq!(
                Code::parse(ok).map(|c| c.as_str().to_string()),
                Some("482913".into()),
                "{ok:?}"
            );
        }
        assert_eq!(code("012345").as_str(), "012345", "前导 0 照留");
        assert_eq!(code("000000").as_str(), "000000");
        for bad in [
            "",
            "48291",
            "4829131",
            "48291a",
            "482_913",
            "482.913",
            "４８２９１",
            "①②③④⑤⑥",
        ] {
            assert_eq!(Code::parse(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn a_code_prints_spaced_for_people_and_never_in_debug() {
        assert_eq!(code("482913").spaced(), "482 913");
        assert_eq!(code("012345").spaced(), "012 345");
        let d = format!("{:?}", code("482913"));
        assert!(!d.contains("482") && !d.contains("913"), "{d}");
    }

    /// 拒绝采样：落在 `SAMPLE_LIMIT` 及以上的 u32 必须扔掉（直接取模的话，
    /// 0–967295 会比别的多出现一点点——分布检查查不出这么小的偏差，这条查）。
    #[test]
    fn random_codes_skip_values_at_or_above_the_limit() {
        let mut block = [0u8; 32];
        block[..4].copy_from_slice(&u32::MAX.to_be_bytes());
        block[4..8].copy_from_slice(&SAMPLE_LIMIT.to_be_bytes());
        block[8..12].copy_from_slice(&482_913u32.to_be_bytes());
        assert_eq!(Code::random(&mut || block).as_str(), "482913");

        let mut edge = [0u8; 32];
        edge[..4].copy_from_slice(&(SAMPLE_LIMIT - 1).to_be_bytes());
        assert_eq!(Code::random(&mut || edge).as_str(), "999999");

        // 一整块都不能用：再要一块。
        let mut calls = 0;
        let mut next = || {
            calls += 1;
            if calls == 1 {
                [0xff; 32]
            } else {
                let mut b = [0u8; 32];
                b[..4].copy_from_slice(&7u32.to_be_bytes());
                b
            }
        };
        assert_eq!(Code::random(&mut next).as_str(), "000007");
    }

    /// 10⁶ 个码：只有数字、正好 6 位；首位和末位各自的分布粗略均匀（卡方，
    /// 9 个自由度，阈值 40 ≈ p 1e-5）。随机数用确定的 splitmix64，结果不会飘。
    #[test]
    fn a_million_random_codes_are_six_digits_and_roughly_uniform() {
        let mut state: u64 = 0x9e37_79b9_7f4a_7c15;
        let mut next = move || {
            let mut out = [0u8; 32];
            for chunk in out.chunks_exact_mut(8) {
                state = state.wrapping_add(0x9e37_79b9_7f4a_7c15);
                let mut z = state;
                z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
                z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
                chunk.copy_from_slice(&(z ^ (z >> 31)).to_be_bytes());
            }
            out
        };
        const N: usize = 1_000_000;
        let (mut first, mut last) = ([0usize; 10], [0usize; 10]);
        for _ in 0..N {
            let c = Code::random(&mut next);
            let b = c.as_str().as_bytes();
            assert_eq!(b.len(), 6);
            assert!(b.iter().all(u8::is_ascii_digit));
            first[(b[0] - b'0') as usize] += 1;
            last[(b[5] - b'0') as usize] += 1;
        }
        let chi = |h: &[usize; 10]| {
            let e = N as f64 / 10.0;
            h.iter().map(|&o| (o as f64 - e).powi(2) / e).sum::<f64>()
        };
        assert!(chi(&first) < 40.0, "首位 {first:?}");
        assert!(chi(&last) < 40.0, "末位 {last:?}");
    }
}
```

- [ ] **Step 3: 跑测试确认失败**

Run: `~/.cargo/bin/cargo test -p dct-mesh invite`
Expected: 编译失败，`cannot find type `Code` in this scope`、`cannot find function `password`` 之类（`canon::tests::field_bytes_is_field_for_strings_and_takes_any_bytes` 这时已经能过）。

- [ ] **Step 4: 实现**

把下面这段放在 `crates/dct-mesh/src/invite.rs` 的**测试模块之上**（文件开头）：

```rust
//! 邀请码（dct-invite-v1）的纯计算部分：生成 6 位码、把码和两台电脑的记录
//! 喂给 SPAKE2、从共享密钥派生两把确认钥匙、算和验两边的确认值。
//!
//! **不碰网络、不碰时间、不自己拿随机数**：码和 SPAKE2 用的随机数都由调用方
//! 给（`Code::random` 的 `rand`、`Handshake::*` 的 `seed`）。状态机（码什么时候
//! 作废、谁能发 `InviteFinish`）在 `dct` 的 `mesh::invite` 里。
//!
//! 规范（字节精确，跟 `docs/superpowers/specs/2026-09-30-dct-invite-code-design.md`
//! 第 2 段一致）：
//!
//! - 口令 = `field("dct-invite-v1")` ‖ `field(码的 6 个 ASCII 数字)`；
//! - 身份 `idA = rec(A)`、`idB = rec(B)`，`rec(m)` = `field(name)` ‖
//!   `field(endpoint)` ‖ `field(sign_pub)` ‖ `field(kx_pub)`（`added_at` 不算）；
//! - SPAKE2：`spake2` 0.4.0，`Ed25519Group`，A（邀请方）`start_a`，B（加入方）
//!   `start_b`；线上的消息是库给的 33 字节（1 字节角色 + 32 字节点）；
//! - `kA`/`kB` = HKDF-SHA256(ikm = K, salt = 空, info = "dct-invite-v1 confirm A"/"… B")；
//! - `T` = `field("dct-invite-v1")` ‖ `field(group)` ‖ `field_bytes(rec(A))` ‖
//!   `field_bytes(rec(B))` ‖ `field_bytes(msgA)` ‖ `field_bytes(msgB)`；
//! - `cA = HMAC-SHA256(kA, T)`，`cB = HMAC-SHA256(kB, T)`；验的时候常数时间比较
//!   （`hmac` 的 `verify_slice`）。
//!
//! `spake2` 0.4 用自己的 M/N 常数，**跟 RFC 9382 不互通**；这里的固定向量只防
//! 我们自己的回归。
use std::fmt;

use hkdf::Hkdf;
use hmac::{Hmac, Mac};
use rand_core::{CryptoRng, RngCore};
use sha2::{Digest, Sha256, Sha512};
use spake2::{Ed25519Group, Identity, Password, Spake2};

use crate::canon::{field, field_bytes};
use crate::roster::Member;

pub const INVITE_VERSION_TAG: &str = "dct-invite-v1";
const CONFIRM_INFO_A: &[u8] = b"dct-invite-v1 confirm A";
const CONFIRM_INFO_B: &[u8] = b"dct-invite-v1 confirm B";
const RNG_TAG: &str = "dct-invite-v1 rng";

/// 码有几位。
pub const CODE_DIGITS: usize = 6;
/// SPAKE2 一条消息的字节数（角色 1 + Ed25519 点 32）。
pub const MSG_LEN: usize = 33;
/// 确认值的字节数（HMAC-SHA256）。
pub const TAG_LEN: usize = 32;

/// 一百万的最大整数倍、且不超过 2^32：落在它下面的 u32 取模才均匀。
const SAMPLE_LIMIT: u32 = 4_294_000_000;

/// 6 位十进制数字，前导 0 照留（`012345` 就是 `012345`）。
///
/// `Debug` 不打出数字：码是一次性的口令，不该进任何日志。
#[derive(Clone, PartialEq, Eq)]
pub struct Code(String);

impl fmt::Debug for Code {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Code(******)")
    }
}

impl Code {
    /// 人敲进来的码。空格、`-`（含全角的）、制表符不算；全角数字当半角。
    /// 去掉这些之后必须正好是 6 位数字，否则 `None`。
    pub fn parse(input: &str) -> Option<Code> {
        let mut digits = String::with_capacity(CODE_DIGITS);
        for c in input.chars() {
            match c {
                ' ' | '-' | '\t' | '\u{3000}' | '\u{FF0D}' => {}
                '0'..='9' => digits.push(c),
                '\u{FF10}'..='\u{FF19}' => {
                    digits.push(char::from(b'0' + (c as u32 - 0xFF10) as u8))
                }
                _ => return None,
            }
            if digits.len() > CODE_DIGITS {
                return None;
            }
        }
        (digits.len() == CODE_DIGITS).then_some(Code(digits))
    }

    /// 从随机字节里拒绝采样出一个均匀的 000000–999999。`rand` 每次给 32
    /// 字节，按 8 个大端 u32 依次试；不小于 `SAMPLE_LIMIT` 的扔掉。8 个全扔
    /// 掉（概率约 10⁻²⁴）就再要一次。
    pub fn random(rand: &mut dyn FnMut() -> [u8; 32]) -> Code {
        loop {
            let b = rand();
            for chunk in b.chunks_exact(4) {
                let x = u32::from_be_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]);
                if x < SAMPLE_LIMIT {
                    return Code(format!("{:06}", x % 1_000_000));
                }
            }
        }
    }

    /// 6 个数字，没有空格。`dct join` 后面敲的就是这个。
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// 给人看的：`482 913`。
    pub fn spaced(&self) -> String {
        format!("{} {}", &self.0[..3], &self.0[3..])
    }
}

/// 一台电脑在邀请里的身份：名字、端点、两把公钥，逐项 `field`。
pub fn rec(m: &Member) -> Vec<u8> {
    let mut out = Vec::new();
    field(&mut out, &m.name);
    field(&mut out, &m.endpoint);
    field(&mut out, &m.sign_pub);
    field(&mut out, &m.kx_pub);
    out
}

/// SPAKE2 的口令字节。
pub fn password(code: &Code) -> Vec<u8> {
    let mut out = Vec::new();
    field(&mut out, INVITE_VERSION_TAG);
    field(&mut out, code.as_str());
    out
}

/// 两边确认值共同覆盖的那段字节 `T`。
pub fn transcript(group: &str, a: &Member, b: &Member, msg_a: &[u8], msg_b: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    field(&mut out, INVITE_VERSION_TAG);
    field(&mut out, group);
    field_bytes(&mut out, &rec(a));
    field_bytes(&mut out, &rec(b));
    field_bytes(&mut out, msg_a);
    field_bytes(&mut out, msg_b);
    out
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InviteError {
    /// 对方的 SPAKE2 消息长度不对、角色不对，或者不是曲线上的点。
    BadMessage,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Role {
    Inviter,
    Joiner,
}

/// 一边的 SPAKE2 进行到一半：自己的消息已经出了，等对方的。
pub struct Handshake {
    role: Role,
    spake: Spake2<Ed25519Group>,
    mine: Vec<u8>,
    a: Member,
    b: Member,
}

impl Handshake {
    /// 邀请方（A）。`a` 是自己，`b` 是来加入的那台自报的记录。
    pub fn inviter(code: &Code, a: &Member, b: &Member, seed: [u8; 32]) -> Handshake {
        let (spake, mine) = Spake2::<Ed25519Group>::start_a_with_rng(
            &Password::new(password(code)),
            &Identity::new(&rec(a)),
            &Identity::new(&rec(b)),
            SeedRng::new(seed),
        );
        Handshake {
            role: Role::Inviter,
            spake,
            mine,
            a: a.clone(),
            b: b.clone(),
        }
    }

    /// 加入方（B）。`a` 是 `InviteOpen` 里那台邀请方的记录，`b` 是自己。
    pub fn joiner(code: &Code, a: &Member, b: &Member, seed: [u8; 32]) -> Handshake {
        let (spake, mine) = Spake2::<Ed25519Group>::start_b_with_rng(
            &Password::new(password(code)),
            &Identity::new(&rec(a)),
            &Identity::new(&rec(b)),
            SeedRng::new(seed),
        );
        Handshake {
            role: Role::Joiner,
            spake,
            mine,
            a: a.clone(),
            b: b.clone(),
        }
    }

    /// 发给对方的那 33 字节。
    pub fn message(&self) -> &[u8] {
        &self.mine
    }

    /// 收到对方的消息：算出共享密钥、两把确认钥匙和 `T`。**这一步不说明码
    /// 对不对**——码不对照样算得出一个 K，只是两边的不一样；对不对要看确认值。
    pub fn finish(self, group: &str, theirs: &[u8]) -> Result<Confirmed, InviteError> {
        let k = self
            .spake
            .finish(theirs)
            .map_err(|_| InviteError::BadMessage)?;
        let (msg_a, msg_b) = match self.role {
            Role::Inviter => (self.mine.as_slice(), theirs),
            Role::Joiner => (theirs, self.mine.as_slice()),
        };
        let hk = Hkdf::<Sha256>::new(None, &k);
        let mut ka = [0u8; 32];
        let mut kb = [0u8; 32];
        hk.expand(CONFIRM_INFO_A, &mut ka)
            .expect("32 bytes is a valid HKDF-SHA256 length");
        hk.expand(CONFIRM_INFO_B, &mut kb)
            .expect("32 bytes is a valid HKDF-SHA256 length");
        Ok(Confirmed {
            ka,
            kb,
            transcript: transcript(group, &self.a, &self.b, msg_a, msg_b),
        })
    }
}

/// SPAKE2 做完之后两边手里的东西。不实现 `Debug`：里面是钥匙。
pub struct Confirmed {
    ka: [u8; 32],
    kb: [u8; 32],
    transcript: Vec<u8>,
}

impl Confirmed {
    fn mac(key: &[u8; 32]) -> Hmac<Sha256> {
        <Hmac<Sha256> as Mac>::new_from_slice(key).expect("HMAC takes any key length")
    }

    /// `cA`：邀请方在 `InviteKey` 里给的。
    pub fn inviter_tag(&self) -> [u8; TAG_LEN] {
        let mut m = Self::mac(&self.ka);
        m.update(&self.transcript);
        m.finalize().into_bytes().into()
    }

    /// `cB`：加入方在 `InviteFinish` 里给的。
    pub fn joiner_tag(&self) -> [u8; TAG_LEN] {
        let mut m = Self::mac(&self.kb);
        m.update(&self.transcript);
        m.finalize().into_bytes().into()
    }

    /// 加入方验 `cA`。常数时间。
    pub fn inviter_tag_ok(&self, tag: &[u8]) -> bool {
        let mut m = Self::mac(&self.ka);
        m.update(&self.transcript);
        m.verify_slice(tag).is_ok()
    }

    /// 邀请方验 `cB`。常数时间。
    pub fn joiner_tag_ok(&self, tag: &[u8]) -> bool {
        let mut m = Self::mac(&self.kb);
        m.update(&self.transcript);
        m.verify_slice(tag).is_ok()
    }
}

/// 把调用方给的 32 字节种子伸长成 SPAKE2 要的随机数流：
/// SHA-512(`field("dct-invite-v1 rng")` ‖ seed ‖ 计数器大端 8 字节)，一块 64 字节。
/// 种子来自系统随机数（`dct` 的 `os_rand`），所以这条流也是。
struct SeedRng {
    seed: [u8; 32],
    counter: u64,
    buf: [u8; 64],
    used: usize,
}

impl SeedRng {
    fn new(seed: [u8; 32]) -> SeedRng {
        SeedRng {
            seed,
            counter: 0,
            buf: [0; 64],
            used: 64,
        }
    }

    fn refill(&mut self) {
        let mut pre = Vec::new();
        field(&mut pre, RNG_TAG);
        let mut h = Sha512::new();
        h.update(&pre);
        h.update(self.seed);
        h.update(self.counter.to_be_bytes());
        self.buf.copy_from_slice(&h.finalize());
        self.counter += 1;
        self.used = 0;
    }
}

impl RngCore for SeedRng {
    fn next_u32(&mut self) -> u32 {
        rand_core::impls::next_u32_via_fill(self)
    }

    fn next_u64(&mut self) -> u64 {
        rand_core::impls::next_u64_via_fill(self)
    }

    fn fill_bytes(&mut self, dest: &mut [u8]) {
        for d in dest.iter_mut() {
            if self.used == self.buf.len() {
                self.refill();
            }
            *d = self.buf[self.used];
            self.used += 1;
        }
    }

    fn try_fill_bytes(&mut self, dest: &mut [u8]) -> Result<(), rand_core::Error> {
        self.fill_bytes(dest);
        Ok(())
    }
}

impl CryptoRng for SeedRng {}
```

- [ ] **Step 5: 跑测试确认通过**

Run: `~/.cargo/bin/cargo test -p dct-mesh`
Expected: 全过，其中 `invite::tests::` 14 条（含 `messages_and_tags_match_the_fixed_vectors` 四个固定值、`a_million_random_codes_are_six_digits_and_roughly_uniform`、`tags_are_verified_in_constant_time`）。固定向量是这份实现在写计划时算出来的回归值；**第一次跑就对不上，说明实现跟本计划的字节规范有出入——去对规范，不许改向量。**

- [ ] **Step 6: 全量检查**

```bash
~/.cargo/bin/cargo test --workspace -- --test-threads=4
~/.cargo/bin/cargo clippy --workspace --all-targets -- -D warnings
~/.cargo/bin/cargo check --workspace --all-targets --target x86_64-pc-windows-msvc
~/.cargo/bin/cargo tree -p dct-mesh -e normal | grep -E "spake2|getrandom"
```

Expected：`test result: ok.` 全绿（`pty::tests::*` / `session::tests::*` 偶发失败见 Global Constraints 的「已知 flaky」，单独重跑）；clippy `Finished`、无 warning；Windows check `Finished`（`src/student_projects.rs:669` 那条 `unused variable: path` 是本来就有的）。 最后一行只列出 `spake2 v0.4.0`，**不许**出现 `getrandom`（出现就是 `default-features = false` 没写对）。

- [ ] **Step 7: Commit**

```bash
git add Cargo.lock crates/dct-mesh/Cargo.toml crates/dct-mesh/src/canon.rs crates/dct-mesh/src/lib.rs crates/dct-mesh/src/invite.rs
git commit -m "feat(mesh): dct-invite-v1 crypto: one-time 6-digit code over SPAKE2 with key confirmation" -m "Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---
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
### Task 3: 老电脑那边——码的状态机 `Live → InFlight → 作废`

**Files:**
- Create: `src/mesh/invite.rs`（A 端：`impl Mesh` 的出码/收回/三种 ask、`flush_outbox`）
- Modify: `src/mesh/mod.rs`（`pub mod invite;`；`Mesh` 加 `invite`/`invite_note`/`invite_seq`/`outbox` 四个字段；`step` 把三种请求分给 `take_probe`/`take_invite_join`/`take_invite_finish`，答复类的丢掉）
- Modify: `src/mesh/group.rs`（`free_name`/`sign_next`/`recipients`/`broadcast` 改成 `pub(super)`；`free_name` 编号后不超过 `MAX_NAME_LEN`）
- Modify: `src/mesh/net.rs`（`testing::EvilNet`：恶意中转，Task 4 的对抗测试用）
- Modify: `src/proto.rs`（`InviteView`、`InviteOutcome`、`InviteNote` 三个数据类型；**这一步不进任何 `Request`/`Response`，协议形状不变，不加协议号**）

**Interfaces:**
- Consumes: Task 1 的 `Code::random`/`Code::as_str`、`Handshake::inviter`、`Handshake::finish`、`Confirmed::inviter_tag`/`joiner_tag_ok`、`MSG_LEN`；Task 2 的 8 种 `Payload`、`wire::encode_bytes`/`decode_len`；已有的 `Mesh::commit`（私有，子模块可用）、`Mesh::drop_from`、`mesh::valid_name`、`mesh::valid_kx_pub`、`Mesh::clock`/`rand`/`journal`、`roster::MAX_NAME_LEN`。
- Produces：
  - `proto::InviteView { id: u64, code: String, expires_at: u64 }`（`Debug` 打星号）、`proto::InviteOutcome::{Joined { name: String }, Burned, Expired}`、`proto::InviteNote { id: u64, outcome: InviteOutcome }`
  - `mesh::invite::INVITE_TTL_SECS: u64 = 600`、`FINISH_WITHIN_SECS: u64 = 30`、`INVITE_ASK_TIMEOUT: Duration = 5s`、`MAX_INVITERS_TRIED: usize = 3`
  - `Mesh::start_invite(&mut self) -> Result<InviteView, MeshProblem>`（没名单 → `NotLoggedIn`；旧码作废）
  - `Mesh::cancel_invite(&mut self)`
  - `Mesh::invite_view(&mut self) -> Option<InviteView>`（顺手清掉到点的，记下结果）
  - `pub(crate) Mesh::expire_invite(&mut self)`；字段 `pub(crate) invite_note: Option<InviteNote>`、`pub(crate) outbox: Vec<(Vec<String>, Vec<u8>)>`
  - `mesh::invite::flush_outbox(mesh: &Mutex<Mesh>, net: &dyn Net)`
  - `pub(super) group::free_name(base: &str, taken: &[String]) -> String`、`sign_next`、`recipients`、`broadcast`
  - 测试用：`mesh::net::testing::EvilNet { inner: FakeNet, outgoing, incoming, sent }`，`EvilNet::honest(inner)`、`.outgoing(f)`、`.incoming(f)`、`.count(tag) -> usize`；`mesh::invite::tests::{Node, raw_join, T0, keys}`（`pub(super)`，Task 4 在同一个测试模块里接着用）

**行为要点**（都有测试钉着）：
- `InviteProbe`：`Live` 且没过期 → `InviteOpen{我自签的记录, group}`，否则 `NoInvite`；不消耗码。
- `InviteJoin`：先查（码是 `Live`、`member.endpoint == env.from`、自签名对、名字合法、`kx_pub` 是 32 字节、不在组里、`spake` 正好 33 字节），任何一项不过回 `InviteFailed` **不作废**；全过**先**把状态置 `InFlight{peer, joiner, deadline = now+30}`，再算 SPAKE2——算不出来（不是曲线上的点、角色字节不对）也作废。
- `InviteFinish`：不是 `InFlight` 或不是那台 → 不回、不改；过了 `deadline` → 作废、`InviteFailed`；`cB` 验不过 → 作废、`InviteFailed`；对 → 撞名编号、签 v+1、落盘（存不下 → 作废、`InviteFailed`）、记 `Joined{name}`、给其余成员的广播排进 `outbox`、回 `InviteDone{roster}`。
- journal 只记 `id=`，从不记码。

- [ ] **Step 1: 写失败的测试**

建 `src/mesh/invite.rs`，先只放测试模块：

```rust
#[cfg(test)]
mod tests {
    use super::super::net::testing::{FakeHub, FakeNet};
    use super::*;
    use dct_link::EndpointId;
    use dct_mesh::MachineKeys;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::Arc;

    pub(super) const T0: u64 = 1_800_000_000;

    pub(super) fn keys(b: u8) -> MachineKeys {
        MachineKeys::from_seeds([b; 32], [b.wrapping_add(100); 32]).unwrap()
    }

    /// 一台内存里的电脑：自己的 `Mesh`、挂在 `hub` 上的 `FakeNet`、一只拨得动的钟。
    pub(super) struct Node {
        pub mesh: Arc<Mutex<Mesh>>,
        pub net: FakeNet,
        pub ep: String,
        pub clock: Arc<AtomicU64>,
    }

    impl Node {
        pub fn new(hub: &Arc<FakeHub>, seed: u8, name: &str) -> Node {
            let clock = Arc::new(AtomicU64::new(T0));
            let c = clock.clone();
            let mesh = Mesh::new(keys(seed), name.into(), None)
                .with_clock(move || c.load(Ordering::SeqCst));
            let mesh = Arc::new(Mutex::new(mesh));
            hub.register(mesh.clone());
            let ep = mesh.lock().unwrap().endpoint().to_string();
            Node {
                net: hub.net_for(&ep),
                mesh,
                ep,
                clock,
            }
        }

        /// 登录过：有一份只有自己的名单。
        pub fn grouped(hub: &Arc<FakeHub>, seed: u8, name: &str) -> Node {
            let n = Node::new(hub, seed, name);
            n.mesh.lock().unwrap().ensure_group().unwrap();
            n
        }

        pub fn at(&self, t: u64) {
            self.clock.store(t, Ordering::SeqCst);
        }

        pub fn invite(&self) -> InviteView {
            self.mesh.lock().unwrap().start_invite().unwrap()
        }

        /// 最近一个码的结果。先按钟把到点的清掉（守护进程里 `view` 也是这么做的）。
        pub fn note(&self) -> Option<InviteNote> {
            let mut m = self.mesh.lock().unwrap();
            m.expire_invite();
            m.invite_note.clone()
        }

        pub fn live(&self) -> bool {
            self.mesh.lock().unwrap().invite_view().is_some()
        }

        pub fn me(&self) -> Member {
            self.mesh.lock().unwrap().me.clone()
        }

        pub fn roster(&self) -> dct_mesh::SignedRoster {
            self.mesh.lock().unwrap().roster.clone().unwrap()
        }

        pub fn names(&self) -> Vec<String> {
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

        /// 发一条 ask 给 `to`，把答复解出来。没答就是 `None`。
        pub fn ask(&self, to: &Node, p: &Payload) -> Option<Payload> {
            self.net
                .ask(&to.ep, wire::encode(p), INVITE_ASK_TIMEOUT)
                .ok()
                .map(|b| wire::decode(&b).unwrap())
        }
    }

    /// 不走 `join()`，手工当一回加入方 B：用 `code` 跟 A 走 SPAKE2。返回 A 对
    /// `InviteJoin` 的答复，和（答复是 `InviteKey`、`cA` 验得过时）B 手里的
    /// 确认结果。
    pub(super) fn raw_join(b: &Node, a: &Node, code: &str) -> (Option<Payload>, Option<Confirmed>) {
        let Some(Payload::InviteOpen { member, group, .. }) = b.ask(a, &Payload::InviteProbe)
        else {
            panic!("A 该答 InviteOpen");
        };
        let me = b.me();
        let hs = Handshake::joiner(&Code::parse(code).unwrap(), &member, &me, [0x5b; 32]);
        let join = Payload::InviteJoin {
            sig: wire::sign_member(&me, &b.mesh.lock().unwrap().keys),
            member: me,
            spake: wire::encode_bytes(hs.message()),
        };
        let reply = b.ask(a, &join);
        let confirmed = match &reply {
            Some(Payload::InviteKey { spake, confirm }) => {
                let c = hs
                    .finish(&group, &wire::decode_len(spake, MSG_LEN).unwrap())
                    .unwrap();
                let tag = STANDARD.decode(confirm).unwrap();
                c.inviter_tag_ok(&tag).then_some(c)
            }
            _ => None,
        };
        (reply, confirmed)
    }

    fn finish(b: &Node, a: &Node, c: &Confirmed) -> Option<Payload> {
        b.ask(
            a,
            &Payload::InviteFinish {
                confirm: wire::encode_bytes(&c.joiner_tag()),
            },
        )
    }

    /// 一个信封，`from` 随便写——模拟中转冒充任何同账号的端点。
    fn env_from(from: &str, to: &Node, p: &Payload) -> Envelope {
        Envelope {
            from: EndpointId::new(from).unwrap(),
            to: EndpointId::new(to.ep.clone()).unwrap(),
            seq: 1,
            payload: wire::encode(p),
            recipients: vec![],
        }
    }

    fn deliver(to: &Node, env: &Envelope) -> Option<Payload> {
        to.mesh
            .lock()
            .unwrap()
            .on_envelope(env)
            .map(|b| wire::decode(&b).unwrap())
    }

    #[test]
    fn without_a_live_code_a_probe_gets_no_invite_and_with_one_it_gets_a_signed_open() {
        let hub = FakeHub::new();
        let a = Node::grouped(&hub, 1, "A");
        let b = Node::grouped(&hub, 2, "B");
        assert_eq!(b.ask(&a, &Payload::InviteProbe), Some(Payload::NoInvite));
        a.invite();
        for _ in 0..2 {
            let Some(Payload::InviteOpen { member, sig, group }) = b.ask(&a, &Payload::InviteProbe)
            else {
                panic!("该答 InviteOpen");
            };
            assert_eq!(member, a.me());
            assert!(wire::verify_member(&member, &sig));
            assert_eq!(group, a.roster().roster.group);
        }
        assert!(a.live(), "探问不消耗码");
    }

    #[test]
    fn a_machine_without_a_group_cannot_invite() {
        let hub = FakeHub::new();
        let a = Node::new(&hub, 1, "A");
        assert_eq!(
            a.mesh.lock().unwrap().start_invite(),
            Err(MeshProblem::NotLoggedIn)
        );
    }

    #[test]
    fn a_code_is_six_digits_valid_for_ten_minutes_and_ids_count_up() {
        let hub = FakeHub::new();
        let a = Node::grouped(&hub, 1, "A");
        let v1 = a.invite();
        assert_eq!(v1.code.len(), 6);
        assert!(v1.code.bytes().all(|c| c.is_ascii_digit()));
        assert_eq!(v1.expires_at, T0 + INVITE_TTL_SECS);
        assert_eq!(v1.id, 1);
        assert_eq!(a.invite().id, 2);
    }

    /// 整条路，手工走：InviteJoin → InviteKey（cA 验得过）→ InviteFinish →
    /// InviteDone。名单 v2 = {A, B}，A 签；码用掉了；结果记下来。
    #[test]
    fn the_right_code_brings_the_joiner_in_and_uses_the_code_up() {
        let hub = FakeHub::new();
        let a = Node::grouped(&hub, 1, "A");
        let b = Node::grouped(&hub, 2, "B");
        let v = a.invite();
        let (reply, confirmed) = raw_join(&b, &a, &v.code);
        assert!(matches!(reply, Some(Payload::InviteKey { .. })));
        let c = confirmed.expect("码对，cA 该验得过");
        let Some(Payload::InviteDone { roster }) = finish(&b, &a, &c) else {
            panic!("该答 InviteDone");
        };
        assert_eq!(roster, a.roster());
        assert_eq!(roster.roster.version, 2);
        assert_eq!(roster.signer, a.ep);
        assert_eq!(a.names(), ["A", "B"]);
        assert!(!a.live(), "用过一次就作废");
        assert_eq!(
            a.note(),
            Some(InviteNote {
                id: v.id,
                outcome: InviteOutcome::Joined { name: "B".into() }
            })
        );
        assert!(
            a.mesh.lock().unwrap().outbox.is_empty(),
            "组里没有别人，不用广播"
        );
    }

    /// 码一进 `InFlight`：探问答 `NoInvite`，别人再来加入一律失败，而且不打扰
    /// 在飞的那一个。
    #[test]
    fn once_in_flight_nobody_else_can_use_the_code() {
        let hub = FakeHub::new();
        let a = Node::grouped(&hub, 1, "A");
        let b = Node::grouped(&hub, 2, "B");
        let c = Node::grouped(&hub, 3, "C");
        let v = a.invite();
        let (_, confirmed) = raw_join(&b, &a, &v.code);
        let conf = confirmed.unwrap();
        assert_eq!(c.ask(&a, &Payload::InviteProbe), Some(Payload::NoInvite));
        let me = c.me();
        let hs = Handshake::joiner(&Code::parse(&v.code).unwrap(), &a.me(), &me, [9; 32]);
        let join = Payload::InviteJoin {
            sig: wire::sign_member(&me, &c.mesh.lock().unwrap().keys),
            member: me,
            spake: wire::encode_bytes(hs.message()),
        };
        assert_eq!(c.ask(&a, &join), Some(Payload::InviteFailed));
        assert!(matches!(
            finish(&b, &a, &conf),
            Some(Payload::InviteDone { .. })
        ));
        assert_eq!(a.names(), ["A", "B"]);
    }

    #[test]
    fn a_wrong_code_burns_it_and_a_retry_with_the_right_one_fails() {
        let hub = FakeHub::new();
        let a = Node::grouped(&hub, 1, "A");
        let b = Node::grouped(&hub, 2, "B");
        let v = a.invite();
        let wrong = if v.code == "000000" { "000001" } else { "000000" };
        let (reply, confirmed) = raw_join(&b, &a, wrong);
        assert!(matches!(reply, Some(Payload::InviteKey { .. })), "A 还不知道码错了");
        assert!(confirmed.is_none(), "B 验 cA 验不过");
        // B 不发 InviteFinish：到点之后 A 把码作废。
        a.at(T0 + FINISH_WITHIN_SECS + 1);
        assert!(!a.live());
        assert_eq!(
            a.note(),
            Some(InviteNote {
                id: v.id,
                outcome: InviteOutcome::Burned
            })
        );
        assert_eq!(b.ask(&a, &Payload::InviteProbe), Some(Payload::NoInvite));
        assert_eq!(a.names(), ["A"]);
    }

    /// B 那边码错了还硬发一个 cB（中转也能这么干）：当场作废。
    #[test]
    fn a_wrong_joiner_tag_burns_the_code_at_once() {
        let hub = FakeHub::new();
        let a = Node::grouped(&hub, 1, "A");
        let b = Node::grouped(&hub, 2, "B");
        let v = a.invite();
        raw_join(&b, &a, &v.code);
        let bad = Payload::InviteFinish {
            confirm: wire::encode_bytes(&[0u8; 32]),
        };
        assert_eq!(b.ask(&a, &bad), Some(Payload::InviteFailed));
        assert!(!a.live());
        assert_eq!(a.note().unwrap().outcome, InviteOutcome::Burned);
        assert_eq!(a.names(), ["A"]);
    }

    /// `InviteFinish` 不是从 `InFlight` 的那一台来的：不理、不改状态；真的 B
    /// 随后照样完成。
    #[test]
    fn a_finish_from_another_endpoint_is_ignored() {
        let hub = FakeHub::new();
        let a = Node::grouped(&hub, 1, "A");
        let b = Node::grouped(&hub, 2, "B");
        let m = Node::grouped(&hub, 9, "M");
        let v = a.invite();
        let (_, confirmed) = raw_join(&b, &a, &v.code);
        let conf = confirmed.unwrap();
        // 连 B 的 cB 都偷到了，从 M 的端点送：照样不理。
        let stolen = Payload::InviteFinish {
            confirm: wire::encode_bytes(&conf.joiner_tag()),
        };
        assert_eq!(deliver(&a, &env_from(&m.ep, &a, &stolen)), None);
        assert!(a.live(), "还在飞，没作废");
        assert!(matches!(
            finish(&b, &a, &conf),
            Some(Payload::InviteDone { .. })
        ));
        assert_eq!(a.names(), ["A", "B"]);
    }

    #[test]
    fn a_finish_at_the_deadline_counts_and_one_second_later_burns() {
        let hub = FakeHub::new();
        let a = Node::grouped(&hub, 1, "A");
        let b = Node::grouped(&hub, 2, "B");
        let v = a.invite();
        let (_, c) = raw_join(&b, &a, &v.code);
        a.at(T0 + FINISH_WITHIN_SECS);
        assert!(matches!(
            finish(&b, &a, &c.unwrap()),
            Some(Payload::InviteDone { .. })
        ));

        let c2 = Node::grouped(&hub, 3, "C");
        a.at(T0 + 100);
        let v = a.invite();
        let (_, c) = raw_join(&c2, &a, &v.code);
        a.at(T0 + 100 + FINISH_WITHIN_SECS + 1);
        assert_eq!(finish(&c2, &a, &c.unwrap()), Some(Payload::InviteFailed));
        assert_eq!(a.note().unwrap().outcome, InviteOutcome::Burned);
        assert_eq!(a.names(), ["A", "B"]);
    }

    /// 10 分钟整：第 599 秒还能用，第 600 秒作废。
    #[test]
    fn a_code_expires_at_exactly_ten_minutes() {
        let hub = FakeHub::new();
        let a = Node::grouped(&hub, 1, "A");
        let b = Node::grouped(&hub, 2, "B");
        let v = a.invite();
        a.at(T0 + INVITE_TTL_SECS - 1);
        assert!(matches!(
            b.ask(&a, &Payload::InviteProbe),
            Some(Payload::InviteOpen { .. })
        ));
        a.at(T0 + INVITE_TTL_SECS);
        assert_eq!(b.ask(&a, &Payload::InviteProbe), Some(Payload::NoInvite));
        assert_eq!(
            a.note(),
            Some(InviteNote {
                id: v.id,
                outcome: InviteOutcome::Expired
            })
        );
    }

    /// 进 `InFlight` 之前的失败都不作废：格式错、签名错、端点不是自己的、
    /// 名字不合规、已经在组里。之后真的 B 照样进得来。
    #[test]
    fn failures_before_in_flight_do_not_burn_the_code() {
        let hub = FakeHub::new();
        let a = Node::grouped(&hub, 1, "A");
        let b = Node::grouped(&hub, 2, "B");
        let m = Node::grouped(&hub, 9, "M");
        let v = a.invite();
        let code = Code::parse(&v.code).unwrap();
        let bm = b.me();
        let b_sig = wire::sign_member(&bm, &b.mesh.lock().unwrap().keys);
        let good_spake =
            wire::encode_bytes(Handshake::joiner(&code, &a.me(), &bm, [1; 32]).message());

        let mut renamed = bm.clone();
        renamed.name = "B2".into();
        let mut bad_name = bm.clone();
        bad_name.name = "a/b".into();
        let bad_name_sig = wire::sign_member(&bad_name, &b.mesh.lock().unwrap().keys);
        let cases: Vec<(&str, Envelope)> = vec![
            (
                "签名跟记录对不上",
                env_from(
                    &b.ep,
                    &a,
                    &Payload::InviteJoin {
                        member: renamed,
                        sig: b_sig.clone(),
                        spake: good_spake.clone(),
                    },
                ),
            ),
            (
                "从别的端点发来",
                env_from(
                    &m.ep,
                    &a,
                    &Payload::InviteJoin {
                        member: bm.clone(),
                        sig: b_sig.clone(),
                        spake: good_spake.clone(),
                    },
                ),
            ),
            (
                "名字不合规",
                env_from(
                    &b.ep,
                    &a,
                    &Payload::InviteJoin {
                        member: bad_name,
                        sig: bad_name_sig,
                        spake: good_spake.clone(),
                    },
                ),
            ),
            (
                "SPAKE2 消息不是 33 字节",
                env_from(
                    &b.ep,
                    &a,
                    &Payload::InviteJoin {
                        member: bm.clone(),
                        sig: b_sig.clone(),
                        spake: wire::encode_bytes(&[b'B'; 32]),
                    },
                ),
            ),
            (
                "SPAKE2 消息不是 base64",
                env_from(
                    &b.ep,
                    &a,
                    &Payload::InviteJoin {
                        member: bm.clone(),
                        sig: b_sig.clone(),
                        spake: "!!".into(),
                    },
                ),
            ),
        ];
        for (what, e) in &cases {
            assert_eq!(deliver(&a, e), Some(Payload::InviteFailed), "{what}");
            assert!(a.live(), "{what}：不该作废");
            assert_eq!(a.note(), None, "{what}");
        }
        // 已经在组里的那一台再来：也不作废。
        {
            let mut r = a.roster().roster.clone();
            r.version = 2;
            r.members.push(m.me());
            let v2 = dct_mesh::roster::sign(r, &a.me(), &keys(1));
            a.mesh.lock().unwrap().commit(v2).unwrap();
        }
        let mm = m.me();
        let already = Payload::InviteJoin {
            sig: wire::sign_member(&mm, &m.mesh.lock().unwrap().keys),
            member: mm.clone(),
            spake: wire::encode_bytes(Handshake::joiner(&code, &a.me(), &mm, [2; 32]).message()),
        };
        assert_eq!(m.ask(&a, &already), Some(Payload::InviteFailed));
        assert!(a.live(), "已经在组里：不该作废");

        let (_, c) = raw_join(&b, &a, &v.code);
        assert!(matches!(
            finish(&b, &a, &c.unwrap()),
            Some(Payload::InviteDone { .. })
        ));
        assert_eq!(a.names(), ["A", "B", "M"]);
    }

    /// 长度、角色字节都对，但不是曲线上的点：这时码已经进了 `InFlight`，作废。
    #[test]
    fn a_spake_message_that_is_not_a_point_burns_the_code() {
        let hub = FakeHub::new();
        let a = Node::grouped(&hub, 1, "A");
        let b = Node::grouped(&hub, 2, "B");
        a.invite();
        let bm = b.me();
        let mut not_a_point = vec![b'B'];
        not_a_point.extend_from_slice(&[0x02; 32]);
        let join = Payload::InviteJoin {
            sig: wire::sign_member(&bm, &b.mesh.lock().unwrap().keys),
            member: bm,
            spake: wire::encode_bytes(&not_a_point),
        };
        assert_eq!(b.ask(&a, &join), Some(Payload::InviteFailed));
        assert!(!a.live());
        assert_eq!(a.note().unwrap().outcome, InviteOutcome::Burned);
    }

    /// 再按一次 `a`：旧码作废，只有新码能用。
    #[test]
    fn a_second_invite_voids_the_first_code() {
        let hub = FakeHub::new();
        let a = Node::grouped(&hub, 1, "A");
        let b = Node::grouped(&hub, 2, "B");
        let first = a.invite();
        let second = a.invite();
        assert_ne!(first.id, second.id);
        if first.code != second.code {
            let (_, c) = raw_join(&b, &a, &first.code);
            assert!(c.is_none(), "旧码对不上");
            assert_eq!(a.names(), ["A"]);
        }
        let third = a.invite();
        let (_, c) = raw_join(&b, &a, &third.code);
        assert!(matches!(
            finish(&b, &a, &c.unwrap()),
            Some(Payload::InviteDone { .. })
        ));
    }

    #[test]
    fn cancelling_takes_the_code_away_without_a_note() {
        let hub = FakeHub::new();
        let a = Node::grouped(&hub, 1, "A");
        let b = Node::grouped(&hub, 2, "B");
        a.invite();
        a.mesh.lock().unwrap().cancel_invite();
        assert!(!a.live());
        assert_eq!(a.note(), None);
        assert_eq!(b.ask(&a, &Payload::InviteProbe), Some(Payload::NoInvite));
    }

    /// 撞名就编号（`Mac` → `Mac 2`），结果里报的是编过号的名字。
    #[test]
    fn a_joiner_whose_name_is_taken_is_numbered() {
        let hub = FakeHub::new();
        let a = Node::grouped(&hub, 1, "Mac");
        let b = Node::grouped(&hub, 2, "Mac");
        let v = a.invite();
        let (_, c) = raw_join(&b, &a, &v.code);
        let Some(Payload::InviteDone { roster }) = finish(&b, &a, &c.unwrap()) else {
            panic!()
        };
        assert_eq!(a.names(), ["Mac", "Mac 2"]);
        assert_eq!(roster.roster.member(&b.ep).unwrap().name, "Mac 2");
        assert_eq!(
            a.note().unwrap().outcome,
            InviteOutcome::Joined {
                name: "Mac 2".into()
            }
        );
    }

    /// 码对了，但 A 的名单存不下（目录是个文件）：不换名单，码作废，答
    /// `InviteFailed`——B 那边也就不会以为自己进来了。
    #[test]
    fn an_invite_that_cannot_be_saved_changes_nothing() {
        let hub = FakeHub::new();
        let t = tempfile::tempdir().unwrap();
        let blocker = t.path().join("mesh");
        std::fs::write(&blocker, b"not a dir").unwrap();
        let a = Node::grouped(&hub, 1, "A");
        {
            let mut m = a.mesh.lock().unwrap();
            let taken = std::mem::replace(&mut *m, Mesh::new(keys(1), "A".into(), None));
            *m = taken.with_store(super::super::store::Store::at(blocker));
        }
        let b = Node::grouped(&hub, 2, "B");
        let v = a.invite();
        let (_, c) = raw_join(&b, &a, &v.code);
        assert_eq!(finish(&b, &a, &c.unwrap()), Some(Payload::InviteFailed));
        assert_eq!(a.names(), ["A"]);
        assert_eq!(a.note().unwrap().outcome, InviteOutcome::Burned);
        assert!(a.mesh.lock().unwrap().outbox.is_empty());
    }

    #[test]
    fn numbering_a_long_name_stays_a_valid_name() {
        let long: String = "长".repeat(dct_mesh::roster::MAX_NAME_LEN);
        let n = group::free_name(&long, std::slice::from_ref(&long));
        assert!(super::super::valid_name(&n), "{n}");
        assert!(n.ends_with(" 2"));
        assert_eq!(group::free_name("Mac", &["Mac".into(), "Mac 2".into()]), "Mac 3");
    }

    /// 组里原来还有 C：B 进来之后，新名单排进 `outbox`，发出去 C 也收到。
    #[test]
    fn the_new_roster_is_broadcast_to_the_other_members() {
        let hub = FakeHub::new();
        let a = Node::grouped(&hub, 1, "A");
        let c = Node::grouped(&hub, 3, "C");
        let v = a.invite();
        let (_, conf) = raw_join(&c, &a, &v.code);
        let Some(Payload::InviteDone { roster }) = finish(&c, &a, &conf.unwrap()) else {
            panic!()
        };
        c.mesh.lock().unwrap().commit(roster).unwrap();

        let b = Node::grouped(&hub, 2, "B");
        let v = a.invite();
        let (_, conf) = raw_join(&b, &a, &v.code);
        finish(&b, &a, &conf.unwrap());
        assert_eq!(c.names(), ["A", "C"], "还没发");
        let queued = a.mesh.lock().unwrap().outbox.clone();
        assert_eq!(queued.len(), 1);
        assert_eq!(queued[0].0, vec![c.ep.clone()], "只发给其余成员，不发给 B");
        flush_outbox(&a.mesh, &a.net);
        assert_eq!(c.names(), ["A", "B", "C"]);
        assert!(a.mesh.lock().unwrap().outbox.is_empty());
    }

    /// 守护进程重启：码只在内存里。同一把钥匙、同一份名单起一个新的 `Mesh`
    /// （换掉 hub 上那一台），它不认得重启前的码。
    #[test]
    fn a_restart_forgets_the_code() {
        let hub = FakeHub::new();
        let a = Node::grouped(&hub, 1, "A");
        let b = Node::grouped(&hub, 2, "B");
        a.invite();
        let restarted = Arc::new(Mutex::new(Mesh::new(keys(1), "A".into(), Some(a.roster()))));
        assert!(restarted.lock().unwrap().invite.is_none());
        hub.register(restarted);
        assert_eq!(b.ask(&a, &Payload::InviteProbe), Some(Payload::NoInvite));
    }

    /// 重放上一轮的 `InviteJoin` / `InviteFinish`：码已经用掉了，都不管用。
    #[test]
    fn replaying_a_finished_round_gets_nothing() {
        let hub = FakeHub::new();
        let a = Node::grouped(&hub, 1, "A");
        let b = Node::grouped(&hub, 2, "B");
        let v = a.invite();
        let bm = b.me();
        let hs = Handshake::joiner(&Code::parse(&v.code).unwrap(), &a.me(), &bm, [3; 32]);
        let join = Payload::InviteJoin {
            sig: wire::sign_member(&bm, &b.mesh.lock().unwrap().keys),
            member: bm,
            spake: wire::encode_bytes(hs.message()),
        };
        let Some(Payload::InviteKey { spake, .. }) = b.ask(&a, &join) else {
            panic!()
        };
        let c = hs
            .finish(
                &a.roster().roster.group,
                &wire::decode_len(&spake, MSG_LEN).unwrap(),
            )
            .unwrap();
        let fin = Payload::InviteFinish {
            confirm: wire::encode_bytes(&c.joiner_tag()),
        };
        assert!(matches!(b.ask(&a, &fin), Some(Payload::InviteDone { .. })));
        let before = a.roster();
        assert_eq!(b.ask(&a, &join), Some(Payload::InviteFailed));
        assert_eq!(b.ask(&a, &fin), None);
        // A 又出了一个新码：重放旧的 InviteJoin，B 已经在组里，被拒，也不连累新码。
        a.invite();
        assert_eq!(b.ask(&a, &join), Some(Payload::InviteFailed));
        assert!(a.live());
        assert_eq!(a.roster(), before);
    }

    /// 答复类的消息不该从轮询里进来。
    #[test]
    fn unasked_invite_replies_are_dropped() {
        let hub = FakeHub::new();
        let a = Node::grouped(&hub, 1, "A");
        let b = Node::grouped(&hub, 2, "B");
        for p in [
            Payload::NoInvite,
            Payload::InviteFailed,
            Payload::InviteKey {
                spake: "x".into(),
                confirm: "y".into(),
            },
            Payload::InviteOpen {
                member: b.me(),
                sig: "s".into(),
                group: "g".into(),
            },
            Payload::InviteDone {
                roster: b.roster(),
            },
        ] {
            assert_eq!(deliver(&a, &env_from(&b.ep, &a, &p)), None, "{p:?}");
        }
        assert_eq!(a.names(), ["A"]);
    }

    /// 码不进日志：走完一整轮，journal 里找不到它。
    #[test]
    fn the_code_never_reaches_the_journal() {
        let hub = FakeHub::new();
        let t = tempfile::tempdir().unwrap();
        let path = t.path().join("sessions.log");
        let j = crate::journal::Journal::new();
        j.set_path(path.clone());
        let a = Node::grouped(&hub, 1, "A");
        {
            let mut m = a.mesh.lock().unwrap();
            let taken = std::mem::replace(&mut *m, Mesh::new(keys(1), "A".into(), None));
            *m = taken.with_journal(Arc::new(j));
        }
        let b = Node::grouped(&hub, 2, "B");
        let v = a.invite();
        let (_, c) = raw_join(&b, &a, &v.code);
        finish(&b, &a, &c.unwrap());
        let log = std::fs::read_to_string(&path).unwrap();
        assert!(log.contains("invite_created"), "{log}");
        assert!(!log.contains(&v.code), "{log}");
    }
}
```

- [ ] **Step 2: 跑测试确认失败**

Run: `~/.cargo/bin/cargo test --lib mesh::invite`
Expected: 编译失败（`src/mesh/invite.rs` 还没挂进 `mod.rs`，或者挂上之后 `start_invite`、`EvilNet`、`InviteView` 找不到）。

- [ ] **Step 3: 实现——其它文件的改动**

把下面整个补丁存成 `/tmp/dct-invite-t3.patch`（**只存 ```diff 块里的内容**），在 worktree 根目录应用：

```bash
git apply --check /tmp/dct-invite-t3.patch && git apply /tmp/dct-invite-t3.patch
```

`--check` 报错说明前面的任务跟本计划的代码有出入：改用 `git apply --3way`，还不行就照补丁逐块手改（上下文行用来定位），**不要**跳过任何一块。

```diff
diff --git a/src/mesh/group.rs b/src/mesh/group.rs
index a9a31eb..55cfe7b 100644
--- a/src/mesh/group.rs
+++ b/src/mesh/group.rs
@@ -233,10 +233,16 @@ fn ask_all(mesh: &Mutex<Mesh>, net: &dyn Net, peers: &[String]) -> Vec<Reply> {
     })
 }
 
-/// `Mac` 撞了名就是 `Mac 2`，再撞 `Mac 3`……
-fn free_name(base: &str, taken: &[String]) -> String {
+/// `Mac` 撞了名就是 `Mac 2`，再撞 `Mac 3`……名字本来就接近上限
+/// （`roster::MAX_NAME_LEN` 个字）的，先截短再编号，编出来的还是合法名字。
+pub(super) fn free_name(base: &str, taken: &[String]) -> String {
     (2..)
-        .map(|n| format!("{base} {n}"))
+        .map(|n| {
+            let suffix = format!(" {n}");
+            let keep = dct_mesh::roster::MAX_NAME_LEN - suffix.chars().count();
+            let head: String = base.chars().take(keep).collect();
+            format!("{}{suffix}", head.trim_end())
+        })
         .find(|c| !taken.contains(c))
         .unwrap_or_default()
 }
@@ -335,7 +341,7 @@ pub fn remove(mesh: &Mutex<Mesh>, net: &dyn Net, who: &str) -> Result<String, Me
     Ok(name)
 }
 
-fn sign_next(m: &Mesh, current: &Roster, members: Vec<dct_mesh::Member>) -> dct_mesh::SignedRoster {
+pub(super) fn sign_next(m: &Mesh, current: &Roster, members: Vec<dct_mesh::Member>) -> dct_mesh::SignedRoster {
     let next = Roster {
         group: current.group.clone(),
         version: current.version + 1,
@@ -344,7 +350,7 @@ fn sign_next(m: &Mesh, current: &Roster, members: Vec<dct_mesh::Member>) -> dct_
     roster::sign(next, &m.me, &m.keys)
 }
 
-fn recipients(m: &Mesh, r: &dct_mesh::SignedRoster) -> Vec<String> {
+pub(super) fn recipients(m: &Mesh, r: &dct_mesh::SignedRoster) -> Vec<String> {
     r.roster
         .members
         .iter()
@@ -358,7 +364,7 @@ fn recipients(m: &Mesh, r: &dct_mesh::SignedRoster) -> Vec<String> {
 ///
 /// 并排发：一台卡到超时不拖着别的，整次广播只花一次 `send` 的时间
 /// （`mesh::worst_case` 按这个算命令行该等多久）。
-fn broadcast(mesh: &Mutex<Mesh>, net: &dyn Net, to: &[String], payload: &[u8]) {
+pub(super) fn broadcast(mesh: &Mutex<Mesh>, net: &dyn Net, to: &[String], payload: &[u8]) {
     let failed: Vec<(String, crate::link::LinkError)> = std::thread::scope(|s| {
         let hs: Vec<_> = to
             .iter()
diff --git a/src/mesh/mod.rs b/src/mesh/mod.rs
index a32bc9c..e3f4fa0 100644
--- a/src/mesh/mod.rs
+++ b/src/mesh/mod.rs
@@ -26,6 +26,7 @@ use crate::link::Handler;
 pub mod cli;
 pub mod deliver;
 pub mod group;
+pub mod invite;
 pub mod login;
 pub mod net;
 pub mod store;
@@ -204,6 +205,16 @@ pub struct Mesh {
     started_at: u64,
     clock: Clock,
     rand: Rand,
+    /// 这台电脑此刻发着的邀请码（`invite` 模块）。只在内存里：守护进程一重启
+    /// 就没了，要重新按 `a`。
+    pub(crate) invite: Option<invite::Invite>,
+    /// 最近一个结束了的邀请码怎么结束的（看板、`dct invite` 拿它说话）。
+    pub(crate) invite_note: Option<crate::proto::InviteNote>,
+    /// 发过几个邀请码（`InviteView::id`）。
+    invite_seq: u64,
+    /// 攥着锁的时候定下来、放了锁再发的名单广播（接收方, payload）。
+    /// `invite::flush_outbox` 发；守护进程的投递线程每拍调一次。
+    pub(crate) outbox: Vec<(Vec<String>, Vec<u8>)>,
 }
 
 impl Mesh {
@@ -241,6 +252,10 @@ impl Mesh {
             started_at: unix_now(),
             clock: Box::new(unix_now),
             rand: Box::new(os_rand),
+            invite: None,
+            invite_note: None,
+            invite_seq: 0,
+            outbox: Vec::new(),
         }
     }
 
@@ -324,15 +339,18 @@ impl Mesh {
             // 读它），不该从轮询里进来。
             Payload::JoinPending { .. } => Step::Done(self.drop(env, "unasked_join_pending")),
             Payload::JoinReveal { nonce } => Step::Done(self.take_reveal(env, &nonce)),
-            // 邀请码那一路下一步才接上；在那之前一律当没听见。
-            Payload::InviteProbe
-            | Payload::InviteOpen { .. }
+            Payload::InviteProbe => Step::Done(self.take_probe(env)),
+            Payload::InviteJoin { member, sig, spake } => {
+                Step::Done(self.take_invite_join(env, member, &sig, &spake))
+            }
+            Payload::InviteFinish { confirm } => Step::Done(self.take_invite_finish(env, &confirm)),
+            // 这几种只该作为 `ask` 的答复回来（`invite::join` 在那里读），不该
+            // 从轮询里进来。
+            Payload::InviteOpen { .. }
             | Payload::NoInvite
-            | Payload::InviteJoin { .. }
             | Payload::InviteKey { .. }
-            | Payload::InviteFinish { .. }
             | Payload::InviteDone { .. }
-            | Payload::InviteFailed => Step::Done(self.drop(env, "invite_not_ready")),
+            | Payload::InviteFailed => Step::Done(self.drop(env, "unasked_invite_reply")),
         }
     }
 
diff --git a/src/mesh/net.rs b/src/mesh/net.rs
index 20450fa..5a3f878 100644
--- a/src/mesh/net.rs
+++ b/src/mesh/net.rs
@@ -208,6 +208,74 @@ pub mod testing {
                 .ok_or(LinkError::Relay(dct_link::LinkError::NoAnswer))
         }
     }
+
+    /// 中转可以对经过它的东西做什么：发出去之前改（或者换成别的），答复回来
+    /// 之后改（或者吞掉）。包在一台电脑自己的 `FakeNet` 外面，替它往外发。
+    type Outgoing = Box<dyn Fn(&str, Vec<u8>) -> Vec<u8> + Send + Sync>;
+    type Incoming = Box<dyn Fn(&str, Vec<u8>) -> Result<Vec<u8>, LinkError> + Send + Sync>;
+
+    pub struct EvilNet {
+        pub inner: FakeNet,
+        pub outgoing: Outgoing,
+        pub incoming: Incoming,
+        /// 真的发出去的每一条（改过之后的）：`(发给谁, payload)`。
+        pub sent: Mutex<Vec<(String, Vec<u8>)>>,
+    }
+
+    impl EvilNet {
+        /// 什么都不改，只记账。
+        pub fn honest(inner: FakeNet) -> EvilNet {
+            EvilNet {
+                inner,
+                outgoing: Box::new(|_, p| p),
+                incoming: Box::new(|_, r| Ok(r)),
+                sent: Mutex::new(Vec::new()),
+            }
+        }
+
+        pub fn outgoing(mut self, f: impl Fn(&str, Vec<u8>) -> Vec<u8> + Send + Sync + 'static) -> EvilNet {
+            self.outgoing = Box::new(f);
+            self
+        }
+
+        pub fn incoming(
+            mut self,
+            f: impl Fn(&str, Vec<u8>) -> Result<Vec<u8>, LinkError> + Send + Sync + 'static,
+        ) -> EvilNet {
+            self.incoming = Box::new(f);
+            self
+        }
+
+        /// 发出去的 payload 里 `"t"` 是 `tag` 的有几条。
+        pub fn count(&self, tag: &str) -> usize {
+            let want = format!("\"t\":\"{tag}\"");
+            self.sent
+                .lock()
+                .unwrap()
+                .iter()
+                .filter(|(_, p)| String::from_utf8_lossy(p).contains(&want))
+                .count()
+        }
+    }
+
+    impl Net for EvilNet {
+        fn peers(&self) -> Result<Vec<String>, LinkError> {
+            self.inner.peers()
+        }
+
+        fn send(&self, to: &str, payload: Vec<u8>) -> Result<(), LinkError> {
+            let p = (self.outgoing)(to, payload);
+            self.sent.lock().unwrap().push((to.to_string(), p.clone()));
+            self.inner.send(to, p)
+        }
+
+        fn ask(&self, to: &str, payload: Vec<u8>, timeout: Duration) -> Result<Vec<u8>, LinkError> {
+            let p = (self.outgoing)(to, payload);
+            self.sent.lock().unwrap().push((to.to_string(), p.clone()));
+            let r = self.inner.ask(to, p, timeout)?;
+            (self.incoming)(to, r)
+        }
+    }
 }
 
 #[cfg(test)]
diff --git a/src/proto.rs b/src/proto.rs
index f5a3c52..3983cfb 100644
--- a/src/proto.rs
+++ b/src/proto.rs
@@ -896,6 +896,49 @@ pub struct PendingJoin {
     pub code: String,
 }
 
+/// 这台电脑此刻发着的邀请码（`dct invite` / 看板上按 `a`）。
+///
+/// **`Debug` 不打出码**：码在 10 分钟里就是进组的钥匙，不该进任何日志。
+/// 线上照常带着（只在本机 socket 上走，见 `Request::MeshStatus`）。
+#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
+pub struct InviteView {
+    /// 这台电脑上第几个邀请（守护进程这次运行期间从 1 数起）。结果（`InviteNote`）
+    /// 按它对上是哪一个。
+    pub id: u64,
+    /// 6 位数字，不带空格。
+    pub code: String,
+    /// 到这一刻（unix 秒）就作废。
+    pub expires_at: u64,
+}
+
+impl std::fmt::Debug for InviteView {
+    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
+        f.debug_struct("InviteView")
+            .field("id", &self.id)
+            .field("code", &"******")
+            .field("expires_at", &self.expires_at)
+            .finish()
+    }
+}
+
+/// 一个邀请码怎么结束的。
+#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
+pub enum InviteOutcome {
+    /// 有一台用它进了组，名单上叫 `name`（撞名时已经编过号）。
+    Joined { name: String },
+    /// 有人拿它试过一次、没对上（或者试到一半不见了）：作废了。
+    Burned,
+    /// 10 分钟到了，没人用。
+    Expired,
+}
+
+/// 最近一个结束了的邀请码。`id` 同 `InviteView::id`。
+#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
+pub struct InviteNote {
+    pub id: u64,
+    pub outcome: InviteOutcome,
+}
+
 /// 一场直播眼下的样子。
 ///
 /// **老师那把推帧/停播用的钥匙（`push_secret`）不在这个类型里。**
```

- [ ] **Step 4: 实现——`src/mesh/invite.rs` 的正文**

把下面这段放在 `src/mesh/invite.rs` 的测试模块**之上**（文件开头）：

```rust
//! 用 6 位邀请码加电脑（dct-invite-v1）。设计见
//! `docs/superpowers/specs/2026-09-30-dct-invite-code-design.md`。
//!
//! - **老电脑（A，发邀请）**：`Mesh::start_invite` 出一个码，状态
//!   `Live → InFlight → 作废`。收到的三种 ask 在这里答：`InviteProbe`、
//!   `InviteJoin`、`InviteFinish`。
//! - **新电脑（B，加入）**：`join`——问一遍同账号在线的电脑、跟有码的那台走
//!   SPAKE2、验过确认值和名单才进组。
//!
//! 密码学（码、SPAKE2、确认值）在 `dct_mesh::invite`，这里只管状态和收发。
//!
//! **作废规则：** 码一进 `InFlight`，不管后面成不成都作废。进 `InFlight`
//! 之前就失败的（格式不对、签名不对、已经在组里）不作废——那几样跟码无关，
//! 中转伪造它们换不来任何关于码的信息。
use std::sync::Mutex;
use std::time::Duration;

use base64::{engine::general_purpose::STANDARD, Engine as _};
use dct_link::Envelope;
use dct_mesh::invite::{Code, Confirmed, Handshake, MSG_LEN};
use dct_mesh::wire::{self, Payload};
use dct_mesh::Member;

use super::group;
use super::net::Net;
use super::Mesh;
use crate::proto::{InviteNote, InviteOutcome, InviteView, MeshProblem};

/// 一个码多久没人用就作废（秒）。
pub const INVITE_TTL_SECS: u64 = 10 * 60;
/// 码进了 `InFlight` 之后，B 必须在这么久里把 `InviteFinish` 送到（秒）。
pub const FINISH_WITHIN_SECS: u64 = 30;
/// B 问每台电脑（探问、加入、确认）等多久。A 都是当场答，这只是一次往返。
pub const INVITE_ASK_TIMEOUT: Duration = Duration::from_secs(5);
/// 同时有好几台发着码时，B 最多试几台（逐台试，对不上的那台码也作废）。
pub const MAX_INVITERS_TRIED: usize = 3;

/// A 这边发着的一个码。
pub(crate) struct Invite {
    id: u64,
    code: Code,
    expires: u64,
    state: State,
}

// 一台电脑同时只有一个码，两个变体差多大无所谓。
#[allow(clippy::large_enum_variant)]
enum State {
    Live,
    /// 有一台（`peer`）拿它来加入了。此后别人再来都失败；它必须在 `deadline`
    /// 之前送来对的 `cB`。
    InFlight {
        peer: String,
        joiner: Member,
        deadline: u64,
        /// SPAKE2 做完之后才有；算不出来的话码已经作废了，走不到要用它的地方。
        confirmed: Option<Confirmed>,
    },
}

impl Invite {
    fn view(&self) -> InviteView {
        InviteView {
            id: self.id,
            code: self.code.as_str().to_string(),
            expires_at: self.expires,
        }
    }
}

impl Mesh {
    /// 出一个新码（看板上按 `a`、`dct invite`）。已经有一个的话，旧的作废。
    /// 组里只有自己一台也行：第一台电脑就是这样拉第二台的。
    pub fn start_invite(&mut self) -> Result<InviteView, MeshProblem> {
        if self.roster.is_none() {
            return Err(MeshProblem::NotLoggedIn);
        }
        let now = (self.clock)();
        let rand = &self.rand;
        let code = Code::random(&mut || rand());
        self.invite_seq += 1;
        let inv = Invite {
            id: self.invite_seq,
            code,
            expires: now + INVITE_TTL_SECS,
            state: State::Live,
        };
        let view = inv.view();
        if let Some(old) = self.invite.replace(inv) {
            self.journal.mesh(&format!("invite_replaced id={}", old.id));
        }
        // 只记编号，**绝不记码**。
        self.journal.mesh(&format!("invite_created id={}", view.id));
        Ok(view)
    }

    /// 收回手上的码（`dct invite` 被 Ctrl-C）。没有就什么都不做。
    pub fn cancel_invite(&mut self) {
        if let Some(old) = self.invite.take() {
            self.journal.mesh(&format!("invite_cancelled id={}", old.id));
        }
    }

    /// 此刻发着的码（过期的先清掉）。`InFlight` 的也算：屏幕上的码还没结果。
    pub fn invite_view(&mut self) -> Option<InviteView> {
        self.expire_invite();
        self.invite.as_ref().map(Invite::view)
    }

    /// 懒着查的两种到点：`Live` 过了 10 分钟 → 过期；`InFlight` 过了 30 秒
    /// 还没等到 `InviteFinish` → 作废。
    ///
    /// 边界：`now == expires` 就算过期；`now == deadline` 还算来得及。
    pub(crate) fn expire_invite(&mut self) {
        let now = (self.clock)();
        let outcome = match &self.invite {
            Some(Invite {
                state: State::Live,
                expires,
                ..
            }) if now >= *expires => InviteOutcome::Expired,
            Some(Invite {
                state: State::InFlight { deadline, .. },
                ..
            }) if now > *deadline => InviteOutcome::Burned,
            _ => return,
        };
        self.end_invite(outcome, "invite_timed_out");
    }

    /// 码结束了：清掉、记下结果。
    fn end_invite(&mut self, outcome: InviteOutcome, why: &str) {
        if let Some(inv) = self.invite.take() {
            self.journal.mesh(&format!("{why} id={}", inv.id));
            self.invite_note = Some(InviteNote {
                id: inv.id,
                outcome,
            });
        }
    }

    /// `InviteProbe`：有 `Live` 的码就报上自己（自签的成员记录和组 id），
    /// 否则 `NoInvite`。不消耗码。
    pub(super) fn take_probe(&mut self, env: &Envelope) -> Option<Vec<u8>> {
        self.expire_invite();
        let live = matches!(
            self.invite,
            Some(Invite {
                state: State::Live,
                ..
            })
        );
        let group = self.roster.as_ref().map(|r| r.roster.group.clone());
        match (live, group) {
            (true, Some(group)) => {
                self.journal
                    .mesh(&format!("invite_probed from={}", env.from));
                Some(wire::encode(&Payload::InviteOpen {
                    sig: wire::sign_member(&self.me, &self.keys),
                    member: self.me.clone(),
                    group,
                }))
            }
            _ => Some(wire::encode(&Payload::NoInvite)),
        }
    }

    /// `InviteJoin`：检查都过了，**先**把码置成 `InFlight`，再算 SPAKE2、回
    /// `InviteKey`。
    pub(super) fn take_invite_join(
        &mut self,
        env: &Envelope,
        member: Member,
        sig: &str,
        spake: &str,
    ) -> Option<Vec<u8>> {
        self.expire_invite();
        let failed = Some(wire::encode(&Payload::InviteFailed));
        let from = env.from.as_str();
        // —— 进 `InFlight` 之前：失败不作废。
        let Some(inv) = self.invite.as_ref() else {
            self.drop_from(from, "invite_join_without_invite");
            return failed;
        };
        if !matches!(inv.state, State::Live) {
            self.drop_from(from, "invite_join_in_flight");
            return failed;
        }
        if member.endpoint != from || !wire::verify_member(&member, sig) {
            self.drop_from(from, "invite_join_bad_sig");
            return failed;
        }
        if !super::valid_name(&member.name) || !super::valid_kx_pub(&member.kx_pub) {
            self.drop_from(from, "invite_join_bad_member");
            return failed;
        }
        let Some(group_id) = self.roster.as_ref().map(|r| r.roster.group.clone()) else {
            self.drop_from(from, "invite_join_without_group");
            return failed;
        };
        if self
            .roster
            .as_ref()
            .is_some_and(|r| r.roster.member(from).is_some())
        {
            self.drop_from(from, "invite_join_already_member");
            return failed;
        }
        let Some(theirs) = wire::decode_len(spake, MSG_LEN) else {
            self.drop_from(from, "invite_join_bad_spake");
            return failed;
        };

        // —— 从这里起码已经用掉了：成不成都作废。
        let now = (self.clock)();
        let seed = (self.rand)();
        let me = self.me.clone();
        let inv = self.invite.as_mut().expect("checked above");
        inv.state = State::InFlight {
            peer: from.to_string(),
            joiner: member.clone(),
            deadline: now + FINISH_WITHIN_SECS,
            confirmed: None,
        };
        let hs = Handshake::inviter(&inv.code, &me, &member, seed);
        let msg_a = hs.message().to_vec();
        match hs.finish(&group_id, &theirs) {
            Ok(c) => {
                let tag = c.inviter_tag();
                if let State::InFlight { confirmed, .. } = &mut inv.state {
                    *confirmed = Some(c);
                }
                self.journal
                    .mesh(&format!("invite_in_flight id={} from={from}", inv.id));
                Some(wire::encode(&Payload::InviteKey {
                    spake: wire::encode_bytes(&msg_a),
                    confirm: wire::encode_bytes(&tag),
                }))
            }
            Err(_) => {
                self.end_invite(InviteOutcome::Burned, "invite_bad_spake_point");
                failed
            }
        }
    }

    /// `InviteFinish`：只认 `InFlight` 的那一台、只在 `deadline` 之前。`cB`
    /// 对了就编号、签名单 v+1、回 `InviteDone`，广播排进 `outbox`；不对就作废。
    pub(super) fn take_invite_finish(&mut self, env: &Envelope, confirm: &str) -> Option<Vec<u8>> {
        let from = env.from.as_str();
        let failed = Some(wire::encode(&Payload::InviteFailed));
        let Some(Invite {
            state: State::InFlight { peer, deadline, .. },
            ..
        }) = &self.invite
        else {
            self.drop_from(from, "invite_finish_not_in_flight");
            return None;
        };
        // 不是那一台：不理，也不改状态——不然谁都能替 B 把码烧掉之外，还能
        // 抢在 B 前面把它「完成」掉。
        if peer != from {
            self.drop_from(from, "invite_finish_from_stranger");
            return None;
        }
        if (self.clock)() > *deadline {
            self.end_invite(InviteOutcome::Burned, "invite_finish_too_late");
            return failed;
        }
        let Some(Invite {
            id,
            state: State::InFlight {
                joiner, confirmed, ..
            },
            ..
        }) = self.invite.take()
        else {
            unreachable!("matched above");
        };
        let ok = confirmed.as_ref().is_some_and(|c| {
            STANDARD
                .decode(confirm)
                .is_ok_and(|tag| c.joiner_tag_ok(&tag))
        });
        if !ok {
            self.journal.mesh(&format!("invite_wrong_code id={id} from={from}"));
            self.invite_note = Some(InviteNote {
                id,
                outcome: InviteOutcome::Burned,
            });
            return failed;
        }
        let Some(current) = self.roster.clone() else {
            return failed;
        };
        let taken: Vec<String> = current
            .roster
            .members
            .iter()
            .map(|m| m.name.clone())
            .collect();
        let mut joiner = joiner;
        if taken.contains(&joiner.name) {
            joiner.name = group::free_name(&joiner.name, &taken);
        }
        let mut members = current.roster.members.clone();
        members.push(joiner.clone());
        let next = group::sign_next(self, &current.roster, members);
        let to: Vec<String> = group::recipients(self, &next)
            .into_iter()
            .filter(|ep| *ep != joiner.endpoint)
            .collect();
        if let Err(e) = self.commit(next.clone()) {
            self.journal
                .mesh(&format!("roster_not_saved id={id} err={e}"));
            self.invite_note = Some(InviteNote {
                id,
                outcome: InviteOutcome::Burned,
            });
            return failed;
        }
        self.journal
            .mesh(&format!("invite_joined id={id} endpoint={}", joiner.endpoint));
        self.invite_note = Some(InviteNote {
            id,
            outcome: InviteOutcome::Joined { name: joiner.name },
        });
        if !to.is_empty() {
            self.outbox
                .push((to, wire::encode(&Payload::Roster(next.clone()))));
        }
        Some(wire::encode(&Payload::InviteDone { roster: next }))
    }
}

fn lock(m: &Mutex<Mesh>) -> std::sync::MutexGuard<'_, Mesh> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// 把 `outbox` 里排着的广播发出去。**不攥着锁发**（`net` 模块头）。守护进程的
/// 投递线程每拍调一次；测试里手动调。
pub fn flush_outbox(mesh: &Mutex<Mesh>, net: &dyn Net) {
    let jobs = std::mem::take(&mut lock(mesh).outbox);
    for (to, payload) in jobs {
        group::broadcast(mesh, net, &to, &payload);
    }
}
```

- [ ] **Step 5: 跑测试确认通过**

Run: `~/.cargo/bin/cargo test --lib mesh::invite`
Expected: `mesh::invite::tests::` 22 条全过。

- [ ] **Step 6: 全量检查**

```bash
~/.cargo/bin/cargo test --workspace -- --test-threads=4
~/.cargo/bin/cargo clippy --workspace --all-targets -- -D warnings
~/.cargo/bin/cargo check --workspace --all-targets --target x86_64-pc-windows-msvc
```
Expected：`test result: ok.` 全绿（`pty::tests::*` / `session::tests::*` 偶发失败见 Global Constraints 的「已知 flaky」，单独重跑）；clippy `Finished`、无 warning；Windows check `Finished`（`src/student_projects.rs:669` 那条 `unused variable: path` 是本来就有的）。

- [ ] **Step 7: Commit**

```bash
git add src/mesh/invite.rs src/mesh/mod.rs src/mesh/group.rs src/mesh/net.rs src/proto.rs
git commit -m "feat(mesh): inviter side of the invite code: Live, InFlight, then burned" -m "Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---
### Task 4: 新电脑那边——`join`、名单验收、对抗测试

**Files:**
- Modify: `src/mesh/invite.rs`（加 `join`、`attempt`、`settle`；测试模块尾部接着加 B 端的测试）
- Modify: `src/proto.rs`（`MeshProblem` 加 `BadInviteCode`、`WrongInviteCode`、`NoInvite { unanswered: u32 }`、`InviteRosterRefused`）
- Modify: `src/i18n.rs`（这四个的文案，和「每个错误码都组得出话、英文没有汉字」那张表）

**Interfaces:**
- Consumes: Task 1 的 `Code::parse`、`Handshake::joiner`、`Confirmed::inviter_tag_ok`/`joiner_tag`；Task 2 的 `Payload`；Task 3 的 `INVITE_ASK_TIMEOUT`、`MAX_INVITERS_TRIED`、`tests::{Node, raw_join, T0, keys}`、`EvilNet`；已有的 `mesh::MAX_JOIN_ASK`（16）、`Mesh::is_alone`、`Mesh::fresh_nonce`（还叫这个名字，返回 32 字节，Task 8 改签名）、`roster::accept_invite(incoming: &SignedRoster, me_endpoint: &str, inviter: &Member) -> Result<(), RosterError>`、`Store::set_name`。
- Produces：
  - `mesh::invite::join(mesh: &Mutex<Mesh>, net: &dyn Net, code: &str, name: Option<&str>) -> Result<(), MeshProblem>`
  - `MeshProblem::{BadInviteCode, WrongInviteCode, NoInvite { unanswered: u32 }, InviteRosterRefused}`

**`join` 的规矩：**
1. 码先 `Code::parse`（去空格、`-`、全角），不对 → `BadInviteCode`，一个字都不发；名字不合规 → `BadName`；已经跟别人同组 → `AlreadyInGroup`。
2. 在线列表排序去重，超过 16 台 → `TooManyAnswered`，一台都不问。
3. 并排 `InviteProbe`；`InviteOpen` 要 `member.endpoint == 问的那台`、自签名对、名字合法、`kx_pub` 合法、`group` 非空才算发邀请的电脑；`NoInvite` 不计；没回话/回了解不出来的/`InviteOpen` 验不过的算 `unanswered`。一台都没有 → `NoInvite { unanswered }`。
4. 按端点排、最多试 3 台：`InviteJoin` → `InviteKey` → 先验 `cA`，**不对就停，不发 `InviteFinish`** → `InviteFinish` → `InviteDone`。名单要过 `accept_invite(roster, 我, 那台A)`、`roster.group == InviteOpen 里的 group`、我那一条的两把公钥是我的。
5. 都没成：有一台走到最后名单被拒 → `InviteRosterRefused`，否则 `WrongInviteCode`。
6. **收到 `InviteDone` 之前本地一样都不改**；进组之后才落盘，`--name` 给过就 `set_name` 并把 `auto_name` 置 false。

- [ ] **Step 1: 写失败的测试**

在 `src/mesh/invite.rs` 测试模块的**最后一个 `}` 之前**接上：

```rust
    impl Node {
        /// 自己造好的 `Mesh`（带 store、带 journal 的）挂上来。钟换成可拨的。
        pub fn from_mesh(hub: &Arc<FakeHub>, mesh: Mesh) -> Node {
            let clock = Arc::new(AtomicU64::new(T0));
            let c = clock.clone();
            let mesh = Arc::new(Mutex::new(
                mesh.with_clock(move || c.load(Ordering::SeqCst)),
            ));
            hub.register(mesh.clone());
            let ep = mesh.lock().unwrap().endpoint().to_string();
            Node {
                net: hub.net_for(&ep),
                mesh,
                ep,
                clock,
            }
        }
    }

    // —— 新电脑这边：`join` ——

    use super::super::net::testing::EvilNet;
    use super::super::store::Store;

    fn join_via(b: &Node, net: &dyn Net, code: &str) -> Result<(), MeshProblem> {
        join(&b.mesh, net, code, None)
    }

    /// 一个跟 `code` 不同的 6 位码。
    fn other_than(code: &str) -> String {
        if code == "000000" { "000001".into() } else { "000000".into() }
    }

    /// B 的整份本地状态：名单、自己那条记录、名字是不是 dct 起的。
    fn state_of(n: &Node) -> (Option<dct_mesh::SignedRoster>, Member, bool) {
        let m = n.mesh.lock().unwrap();
        (m.roster.clone(), m.me.clone(), m.auto_name)
    }

    #[test]
    fn joining_with_the_right_code_takes_the_inviters_roster() {
        let hub = FakeHub::new();
        let a = Node::grouped(&hub, 1, "A");
        let b = Node::grouped(&hub, 2, "B");
        let v = a.invite();
        assert_eq!(join_via(&b, &b.net, &v.code), Ok(()));
        assert_eq!(b.roster(), a.roster());
        assert_eq!(b.names(), ["A", "B"]);
        assert_eq!(
            a.note().unwrap().outcome,
            InviteOutcome::Joined { name: "B".into() }
        );
        assert!(!b.mesh.lock().unwrap().is_alone());
    }

    /// 人敲的码：中间带空格、带 `-`、全角数字、前导 0，都认。
    #[test]
    fn a_code_typed_with_spaces_dashes_or_full_width_digits_still_works() {
        for style in 0..3 {
            let hub = FakeHub::new();
            let a = Node::grouped(&hub, 1, "A");
            let b = Node::grouped(&hub, 2, "B");
            let v = a.invite();
            let typed = match style {
                0 => format!("{} {}", &v.code[..3], &v.code[3..]),
                1 => format!("{}-{}", &v.code[..3], &v.code[3..]),
                _ => v
                    .code
                    .chars()
                    .map(|c| char::from_u32(c as u32 - '0' as u32 + 0xFF10).unwrap())
                    .collect(),
            };
            assert_eq!(join_via(&b, &b.net, &typed), Ok(()), "{typed:?}");
        }
    }

    #[test]
    fn a_malformed_code_or_name_is_refused_before_anything_is_sent() {
        let hub = FakeHub::new();
        let _a = Node::grouped(&hub, 1, "A");
        let b = Node::grouped(&hub, 2, "B");
        let net = EvilNet::honest(hub.net_for(&b.ep));
        for bad in ["48291", "4829134", "48291x", ""] {
            assert_eq!(join_via(&b, &net, bad), Err(MeshProblem::BadInviteCode), "{bad:?}");
        }
        assert_eq!(
            join(&b.mesh, &net, "482913", Some("a/b")),
            Err(MeshProblem::BadName)
        );
        assert!(net.sent.lock().unwrap().is_empty());
    }

    #[test]
    fn a_wrong_code_fails_burns_the_inviters_code_and_changes_nothing_here() {
        let hub = FakeHub::new();
        let a = Node::grouped(&hub, 1, "A");
        let b = Node::grouped(&hub, 2, "B");
        let v = a.invite();
        let before = state_of(&b);
        let net = EvilNet::honest(hub.net_for(&b.ep));
        assert_eq!(
            join_via(&b, &net, &other_than(&v.code)),
            Err(MeshProblem::WrongInviteCode)
        );
        assert_eq!(net.count("invite_finish"), 0, "cA 验不过就不再往下走");
        assert_eq!(state_of(&b), before);
        a.at(T0 + FINISH_WITHIN_SECS + 1);
        assert!(!a.live());
        assert_eq!(a.note().unwrap().outcome, InviteOutcome::Burned);
        // 作废之后，对的码也进不来。
        assert_eq!(
            join_via(&b, &b.net, &v.code),
            Err(MeshProblem::NoInvite { unanswered: 0 })
        );
        assert_eq!(a.names(), ["A"]);
    }

    /// 同账号的另一台（或中转冒充它）抢先拿错码试一次：码作废，真的 B
    /// 随后进不来，提示重新生成。
    #[test]
    fn a_front_running_wrong_guess_burns_the_code_for_the_real_joiner() {
        let hub = FakeHub::new();
        let a = Node::grouped(&hub, 1, "A");
        let b = Node::grouped(&hub, 2, "B");
        let m = Node::grouped(&hub, 9, "M");
        let v = a.invite();
        let (_, c) = raw_join(&m, &a, &other_than(&v.code));
        assert!(c.is_none());
        assert_eq!(
            join_via(&b, &b.net, &v.code),
            Err(MeshProblem::NoInvite { unanswered: 0 })
        );
        a.at(T0 + FINISH_WITHIN_SECS + 1);
        assert_eq!(a.note().unwrap().outcome, InviteOutcome::Burned);
        assert_eq!(a.names(), ["A"]);
        assert_eq!(b.mesh.lock().unwrap().roster.as_ref().unwrap().roster.version, 1);
    }

    /// 中转把 B 的 SPAKE2 消息换成自己的（拿一个猜的码算的），B 的记录原样
    /// 转：A 进 `InFlight`，但 B 验 `cA` 验不过，不发 `InviteFinish`；A 的码
    /// 到点作废。
    #[test]
    fn a_relay_that_swaps_the_joiners_spake_message_gets_nowhere() {
        let hub = FakeHub::new();
        let a = Node::grouped(&hub, 1, "A");
        let b = Node::grouped(&hub, 2, "B");
        let v = a.invite();
        let (am, bm) = (a.me(), b.me());
        let guess = Code::parse(&other_than(&v.code)).unwrap();
        let net = EvilNet::honest(hub.net_for(&b.ep)).outgoing(move |_, p| match wire::decode(&p) {
            Ok(Payload::InviteJoin { member, sig, .. }) => {
                let evil = Handshake::joiner(&guess, &am, &bm, [0xee; 32]);
                wire::encode(&Payload::InviteJoin {
                    member,
                    sig,
                    spake: wire::encode_bytes(evil.message()),
                })
            }
            _ => p,
        });
        assert_eq!(join_via(&b, &net, &v.code), Err(MeshProblem::WrongInviteCode));
        assert_eq!(net.count("invite_finish"), 0);
        a.at(T0 + FINISH_WITHIN_SECS + 1);
        assert_eq!(a.note().unwrap().outcome, InviteOutcome::Burned);
        assert_eq!(a.names(), ["A"]);
        assert_eq!(b.names(), ["B"]);
    }

    /// 中转冒充 A 回 `InviteKey`（自己拿一个猜的码跟 B 的消息走完 SPAKE2）：
    /// B 的 `cA` 验不过，停下，不发 `InviteFinish`。
    #[test]
    fn a_relay_that_answers_for_the_inviter_fails_the_inviter_tag() {
        let hub = FakeHub::new();
        let a = Node::grouped(&hub, 1, "A");
        let b = Node::grouped(&hub, 2, "B");
        let v = a.invite();
        let (am, bm) = (a.me(), b.me());
        let group = a.roster().roster.group.clone();
        let guess = Code::parse(&other_than(&v.code)).unwrap();
        let joined = Arc::new(Mutex::new(None::<Vec<u8>>));
        let seen = joined.clone();
        let net = EvilNet::honest(hub.net_for(&b.ep))
            .outgoing(move |_, p| {
                if let Ok(Payload::InviteJoin { spake, .. }) = wire::decode(&p) {
                    *seen.lock().unwrap() = wire::decode_len(&spake, MSG_LEN);
                }
                p
            })
            .incoming(move |_, r| match wire::decode(&r) {
                Ok(Payload::InviteKey { .. }) => {
                    let msg_b = joined.lock().unwrap().clone().unwrap();
                    let evil = Handshake::inviter(&guess, &am, &bm, [0xaa; 32]);
                    let msg_a = evil.message().to_vec();
                    let c = evil.finish(&group, &msg_b).unwrap();
                    Ok(wire::encode(&Payload::InviteKey {
                        spake: wire::encode_bytes(&msg_a),
                        confirm: wire::encode_bytes(&c.inviter_tag()),
                    }))
                }
                _ => Ok(r),
            });
        assert_eq!(join_via(&b, &net, &v.code), Err(MeshProblem::WrongInviteCode));
        assert_eq!(net.count("invite_finish"), 0);
        assert_eq!(b.names(), ["B"]);
    }

    /// 中转在 `InviteJoin` 里把 B 的一把公钥换成自己的：B 的自签名就不成立，
    /// A 在进 `InFlight` 之前就拒——**码不作废**（这一下换不来任何关于码的
    /// 信息）。B 重来一次，中转不捣乱，就进得来。
    #[test]
    fn a_relay_that_edits_the_joiners_keys_is_refused_without_burning() {
        let hub = FakeHub::new();
        let a = Node::grouped(&hub, 1, "A");
        let b = Node::grouped(&hub, 2, "B");
        let v = a.invite();
        let evil_kx = STANDARD.encode(keys(9).kx_pub());
        let net = EvilNet::honest(hub.net_for(&b.ep)).outgoing(move |_, p| match wire::decode(&p) {
            Ok(Payload::InviteJoin { mut member, sig, spake }) => {
                member.kx_pub = evil_kx.clone();
                wire::encode(&Payload::InviteJoin { member, sig, spake })
            }
            _ => p,
        });
        assert_eq!(join_via(&b, &net, &v.code), Err(MeshProblem::WrongInviteCode));
        assert!(a.live(), "签名不对不作废");
        assert_eq!(join_via(&b, &b.net, &v.code), Ok(()));
        let listed = a.roster().roster.member(&b.ep).cloned().unwrap();
        assert_eq!(listed.kx_pub, b.me().kx_pub);
    }

    /// 中转伪造 `InviteOpen`：换成另一台电脑的自签记录（端点不是问的那台）、
    /// 或者改了记录里的字段（自签不成立）——都不算发着码的电脑。
    #[test]
    fn a_forged_invite_open_is_not_an_inviter() {
        let hub = FakeHub::new();
        let a = Node::grouped(&hub, 1, "A");
        let b = Node::grouped(&hub, 2, "B");
        let m = Node::grouped(&hub, 9, "M");
        hub.set_online(&m.ep, false);
        let v = a.invite();
        let mm = m.me();
        let m_sig = wire::sign_member(&mm, &m.mesh.lock().unwrap().keys);
        for forge in 0..2 {
            let (mm, m_sig) = (mm.clone(), m_sig.clone());
            let net = EvilNet::honest(hub.net_for(&b.ep)).incoming(move |_, r| match wire::decode(&r) {
                Ok(Payload::InviteOpen { mut member, sig, group }) => Ok(wire::encode(&if forge == 0 {
                    Payload::InviteOpen { member: mm.clone(), sig: m_sig.clone(), group }
                } else {
                    member.name = "改过的名字".into();
                    Payload::InviteOpen { member, sig, group }
                })),
                _ => Ok(r),
            });
            assert_eq!(
                join_via(&b, &net, &v.code),
                Err(MeshProblem::NoInvite { unanswered: 1 }),
                "forge {forge}"
            );
            assert_eq!(net.count("invite_join"), 0, "forge {forge}");
        }
        assert!(a.live());
    }

    /// `InviteDone` 里的名单被动过：组不对、签名不对、我的加密公钥被换——都
    /// 不收，本地什么都不变，告诉用户去老电脑上把这台移掉。后两种是 A 自己
    /// 签的（签名验得过），只有「组对不上 `T`」「我那一条不是我的钥匙」拦得住。
    #[test]
    fn a_tampered_invite_done_roster_is_refused() {
        for tamper in 0..5 {
            let hub = FakeHub::new();
            let a = Node::grouped(&hub, 1, "A");
            let b = Node::grouped(&hub, 2, "B");
            let v = a.invite();
            let before = state_of(&b);
            let (b_ep, evil_kx) = (b.ep.clone(), STANDARD.encode(keys(9).kx_pub()));
            let net = EvilNet::honest(hub.net_for(&b.ep)).incoming(move |_, r| match wire::decode(&r) {
                Ok(Payload::InviteDone { mut roster }) => {
                    match tamper {
                        0 => roster.roster.group = "evil".into(),
                        1 => roster.sig = STANDARD.encode([0u8; 64]),
                        2 => {
                            for m in &mut roster.roster.members {
                                if m.endpoint == b_ep {
                                    m.kx_pub = evil_kx.clone();
                                }
                            }
                        }
                        3 => {
                            let signer = roster.roster.member(&roster.signer).cloned().unwrap();
                            let mut r = roster.roster.clone();
                            r.group = "mine-somewhere-else".into();
                            roster = dct_mesh::roster::sign(r, &signer, &keys(1));
                        }
                        _ => {
                            let signer = roster.roster.member(&roster.signer).cloned().unwrap();
                            let mut r = roster.roster.clone();
                            for m in &mut r.members {
                                if m.endpoint == b_ep {
                                    m.kx_pub = evil_kx.clone();
                                }
                            }
                            roster = dct_mesh::roster::sign(r, &signer, &keys(1));
                        }
                    }
                    Ok(wire::encode(&Payload::InviteDone { roster }))
                }
                _ => Ok(r),
            });
            assert_eq!(
                join_via(&b, &net, &v.code),
                Err(MeshProblem::InviteRosterRefused),
                "tamper {tamper}"
            );
            assert_eq!(state_of(&b), before, "tamper {tamper}");
        }
    }

    /// 走到最后一步、`InviteDone` 没回来：本地一样也没改（名单、名字、
    /// `--name` 起的名字都没落盘）。
    #[test]
    fn nothing_changes_locally_until_invite_done_arrives() {
        let hub = FakeHub::new();
        let t = tempfile::tempdir().unwrap();
        let a = Node::grouped(&hub, 1, "A");
        let mesh = Mesh::new(keys(2), "Mac".into(), None).with_store(Store::at(t.path().join("mesh")));
        let b = Node::from_mesh(&hub, mesh);
        b.mesh.lock().unwrap().auto_name = true;
        b.mesh.lock().unwrap().ensure_group().unwrap();
        let before = state_of(&b);
        let st = Store::at(t.path().join("mesh"));
        let roster_before = st.roster().unwrap();
        let v = a.invite();
        let net = EvilNet::honest(hub.net_for(&b.ep)).incoming(|_, r| match wire::decode(&r) {
            Ok(Payload::InviteDone { .. }) => Err(crate::link::LinkError::Unreachable),
            _ => Ok(r),
        });
        assert_eq!(
            join(&b.mesh, &net, &v.code, Some("公司电脑")),
            Err(MeshProblem::WrongInviteCode)
        );
        assert_eq!(state_of(&b), before);
        assert_eq!(st.roster().unwrap(), roster_before);
        assert!(!st.has_chosen_name(), "--name 没落盘");
        assert_eq!(a.names(), ["A", "公司电脑"], "A 那边已经签进去了（已知：要 dct peers remove）");
    }

    #[test]
    fn a_chosen_name_is_used_and_saved_once_joined() {
        let hub = FakeHub::new();
        let t = tempfile::tempdir().unwrap();
        let a = Node::grouped(&hub, 1, "A");
        let mesh = Mesh::new(keys(2), "Mac".into(), None).with_store(Store::at(t.path().join("mesh")));
        let b = Node::from_mesh(&hub, mesh);
        b.mesh.lock().unwrap().ensure_group().unwrap();
        let v = a.invite();
        assert_eq!(join(&b.mesh, &b.net, &v.code, Some("  公司电脑 ")), Ok(()));
        assert_eq!(a.names(), ["A", "公司电脑"]);
        let st = Store::at(t.path().join("mesh"));
        assert!(st.has_chosen_name());
        assert_eq!(st.name().unwrap(), "公司电脑");
        assert_eq!(st.roster().unwrap(), Some(a.roster()));
    }

    /// B 的名字跟组里的撞了：A 编号，B 进组之后自己也叫编过号的名字。
    #[test]
    fn a_clashing_name_is_numbered_by_the_inviter() {
        let hub = FakeHub::new();
        let a = Node::grouped(&hub, 1, "Mac");
        let b = Node::grouped(&hub, 2, "Mac");
        let v = a.invite();
        assert_eq!(join_via(&b, &b.net, &v.code), Ok(()));
        assert_eq!(b.me().name, "Mac 2");
        assert_eq!(b.names(), ["Mac", "Mac 2"]);
    }

    #[test]
    fn an_expired_code_finds_no_invite() {
        let hub = FakeHub::new();
        let a = Node::grouped(&hub, 1, "A");
        let b = Node::grouped(&hub, 2, "B");
        let v = a.invite();
        a.at(T0 + INVITE_TTL_SECS);
        assert_eq!(
            join_via(&b, &b.net, &v.code),
            Err(MeshProblem::NoInvite { unanswered: 0 })
        );
        assert_eq!(a.note().unwrap().outcome, InviteOutcome::Expired);
    }

    /// 两台同时发着码：B 逐台试（按端点排），对不上的那台码也作废；用的是
    /// 哪台的码就进哪台的组。
    #[test]
    fn with_two_inviters_the_wrong_one_is_burned_and_the_right_one_takes_b_in() {
        let hub = FakeHub::new();
        let x = Node::grouped(&hub, 1, "X");
        let y = Node::grouped(&hub, 3, "Y");
        let b = Node::grouped(&hub, 2, "B");
        let (first, second) = if x.ep < y.ep { (&x, &y) } else { (&y, &x) };
        first.invite();
        let v = second.invite();
        let v_first = first.mesh.lock().unwrap().invite_view().unwrap();
        if v_first.code == v.code {
            return; // 百万分之一：两个码一样，这一轮测不出。
        }
        assert_eq!(join_via(&b, &b.net, &v.code), Ok(()));
        assert_eq!(b.roster().signer, second.ep);
        first.at(T0 + FINISH_WITHIN_SECS + 1);
        assert_eq!(first.note().unwrap().outcome, InviteOutcome::Burned);
        assert_eq!(first.names(), [first.me().name]);
    }

    #[test]
    fn at_most_three_inviters_are_tried() {
        let hub = FakeHub::new();
        let b = Node::grouped(&hub, 2, "B");
        let inviters: Vec<Node> = (10..14).map(|s| Node::grouped(&hub, s, &format!("I{s}"))).collect();
        for n in &inviters {
            n.invite();
        }
        let codes: Vec<String> = inviters
            .iter()
            .map(|n| n.mesh.lock().unwrap().invite_view().unwrap().code)
            .collect();
        let unused = (0..1_000_000)
            .map(|i| format!("{i:06}"))
            .find(|c| !codes.contains(c))
            .unwrap();
        let net = EvilNet::honest(hub.net_for(&b.ep));
        assert_eq!(join_via(&b, &net, &unused), Err(MeshProblem::WrongInviteCode));
        assert_eq!(net.count("invite_probe"), 4);
        assert_eq!(net.count("invite_join"), MAX_INVITERS_TRIED);
    }

    /// 中转说同账号在线的有 17 台：一台都不问。
    #[test]
    fn more_than_sixteen_computers_online_asks_nobody() {
        struct Crowd;
        impl Net for Crowd {
            fn peers(&self) -> Result<Vec<String>, crate::link::LinkError> {
                Ok((0..=super::super::MAX_JOIN_ASK).map(|i| format!("c-{i:020}")).collect())
            }
            fn send(&self, _: &str, _: Vec<u8>) -> Result<(), crate::link::LinkError> {
                panic!("不该发任何东西")
            }
            fn ask(&self, _: &str, _: Vec<u8>, _: Duration) -> Result<Vec<u8>, crate::link::LinkError> {
                panic!("不该问任何一台")
            }
        }
        let hub = FakeHub::new();
        let b = Node::grouped(&hub, 2, "B");
        assert_eq!(join_via(&b, &Crowd, "482913"), Err(MeshProblem::TooManyAnswered));
    }

    /// 正好 16 台：每台都问（上限是「超过 16」）。
    #[test]
    fn exactly_sixteen_computers_online_are_all_asked() {
        struct Sixteen(Mutex<usize>);
        impl Net for Sixteen {
            fn peers(&self) -> Result<Vec<String>, crate::link::LinkError> {
                Ok((0..super::super::MAX_JOIN_ASK).map(|i| format!("c-{i:020}")).collect())
            }
            fn send(&self, _: &str, _: Vec<u8>) -> Result<(), crate::link::LinkError> {
                panic!("不该 send")
            }
            fn ask(&self, _: &str, _: Vec<u8>, _: Duration) -> Result<Vec<u8>, crate::link::LinkError> {
                *self.0.lock().unwrap() += 1;
                Ok(wire::encode(&Payload::NoInvite))
            }
        }
        let hub = FakeHub::new();
        let b = Node::grouped(&hub, 2, "B");
        let net = Sixteen(Mutex::new(0));
        assert_eq!(
            join_via(&b, &net, "482913"),
            Err(MeshProblem::NoInvite { unanswered: 0 })
        );
        assert_eq!(*net.0.lock().unwrap(), super::super::MAX_JOIN_ASK);
    }

    /// 发邀请那台的名字不合规（控制字符、双向控制符）：它的 `InviteOpen` 不算。
    #[test]
    fn an_inviter_with_a_bad_name_is_not_an_inviter() {
        let hub = FakeHub::new();
        let a = Node::from_mesh(&hub, Mesh::new(keys(1), "A\u{202e}x".into(), None));
        a.mesh.lock().unwrap().ensure_group().unwrap();
        let b = Node::grouped(&hub, 2, "B");
        let v = a.invite();
        assert_eq!(
            join_via(&b, &b.net, &v.code),
            Err(MeshProblem::NoInvite { unanswered: 1 })
        );
        assert!(a.live(), "B 一句都没跟它多说");
    }

    /// 两边比确认值只走 `verify_slice`（常数时间），这一层不自己拿 `==` 比。
    #[test]
    fn tags_are_only_checked_through_the_constant_time_helpers() {
        let src = include_str!("invite.rs");
        let body = &src[..src.find("#[cfg(test)]").unwrap()];
        assert!(!body.contains("_tag() =="), "别拿 == 比确认值");
        assert!(!body.contains("== tag"), "别拿 == 比确认值");
        assert!(body.contains("inviter_tag_ok(") && body.contains("joiner_tag_ok("));
    }

    #[test]
    fn a_machine_already_grouped_with_others_is_refused_without_asking() {
        let hub = FakeHub::new();
        let a = Node::grouped(&hub, 1, "A");
        let b = Node::grouped(&hub, 2, "B");
        let v = a.invite();
        join_via(&b, &b.net, &v.code).unwrap();
        a.invite();
        let net = EvilNet::honest(hub.net_for(&b.ep));
        assert_eq!(join_via(&b, &net, "482913"), Err(MeshProblem::AlreadyInGroup));
        assert!(net.sent.lock().unwrap().is_empty());
    }

    /// 老电脑的 dct 太旧，不认识 `InviteProbe`（解不出来就丢，不回）：
    /// 算一台没回话的，提示可能太旧。
    #[test]
    fn an_old_dct_that_does_not_know_invite_probe_counts_as_unanswered() {
        let hub = FakeHub::new();
        let a = Node::grouped(&hub, 1, "A");
        let b = Node::grouped(&hub, 2, "B");
        a.invite();
        let net = EvilNet::honest(hub.net_for(&b.ep)).outgoing(|_, p| {
            if matches!(wire::decode(&p), Ok(Payload::InviteProbe)) {
                br#"{"t":"something_this_version_does_not_know"}"#.to_vec()
            } else {
                p
            }
        });
        assert_eq!(
            join_via(&b, &net, "482913"),
            Err(MeshProblem::NoInvite { unanswered: 1 })
        );
    }
```

在 `src/i18n.rs` 的 `every_error_code_has_words_in_both_languages`（那张 `codes` 表，`Mesh(crate::proto::MeshProblem::BadAddress("pc".into())),` 那一行后面）加：

```rust
            Mesh(crate::proto::MeshProblem::BadInviteCode),
            Mesh(crate::proto::MeshProblem::WrongInviteCode),
            Mesh(crate::proto::MeshProblem::NoInvite { unanswered: 0 }),
            Mesh(crate::proto::MeshProblem::NoInvite { unanswered: 2 }),
            Mesh(crate::proto::MeshProblem::InviteRosterRefused),
```

- [ ] **Step 2: 跑测试确认失败**

Run: `~/.cargo/bin/cargo test --lib mesh::invite`
Expected: 编译失败：`cannot find function `join``、`no variant `BadInviteCode``。

- [ ] **Step 3: 实现——`proto.rs`、`i18n.rs`**

先把 Step 1 里手改过的文件还原（补丁里已经包含同样的测试）：`git checkout -- src/i18n.rs`。

把下面整个补丁存成 `/tmp/dct-invite-t4.patch`（**只存 ```diff 块里的内容**），在 worktree 根目录应用：

```bash
git apply --check /tmp/dct-invite-t4.patch && git apply /tmp/dct-invite-t4.patch
```

`--check` 报错说明前面的任务跟本计划的代码有出入：改用 `git apply --3way`，还不行就照补丁逐块手改（上下文行用来定位），**不要**跳过任何一块。

```diff
diff --git a/src/i18n.rs b/src/i18n.rs
index 92f2894..9940c5d 100644
--- a/src/i18n.rs
+++ b/src/i18n.rs
@@ -2046,6 +2046,37 @@ pub mod msg {
                 en: format!("{to} is not an address. Write it as computer/session, for example: office-pc/dc-terminal"),
                 zh: format!("{to} 不是一个地址。写成 电脑名/会话名，比如：公司Windows/dc-terminal"),
             ),
+            BadInviteCode => t!(
+                lang,
+                en: "The invite code is 6 digits, for example: dct join 482913".to_string(),
+                zh: "邀请码是 6 位数字，比如：dct join 482913".to_string(),
+            ),
+            WrongInviteCode => t!(
+                lang,
+                en: "The invite code is wrong or no longer valid. On your other computer press a to get a new one".to_string(),
+                zh: "邀请码不对或已作废，请在老电脑上按 a 重新生成".to_string(),
+            ),
+            NoInvite { unanswered } => {
+                let base = t!(
+                    lang,
+                    en: "None of your computers online is offering an invite code (it may have expired or been used). On your other computer press a (or run dct invite) to get one. If that does not help, check that both computers use the same DC account and that dct is running on the other one".to_string(),
+                    zh: "没有在线的电脑发出邀请（邀请码可能已过期或已作废）。在老电脑上按 a（或运行 dct invite）出一个码；还不行就检查两台是不是同一个 DC 账号、老电脑的 dct 开着没有".to_string(),
+                );
+                if *unanswered == 0 {
+                    base
+                } else {
+                    t!(
+                        lang,
+                        en: format!("{base}. {unanswered} computer(s) did not answer at all: their dct may be too old, update it first"),
+                        zh: format!("{base}。有 {unanswered} 台电脑没回话：它们的 dct 可能太旧，先升级"),
+                    )
+                }
+            }
+            InviteRosterRefused => t!(
+                lang,
+                en: "Your other computer has already put this one in the group, but the group list it sent does not check out here, so this computer did not join. On the other computer run dct peers remove <this computer's name>, then press a for a new code".to_string(),
+                zh: "老电脑那边已经把这台电脑记进组里了，但它发来的名单在这边验不过，没有加入。在老电脑上运行 dct peers remove <这台电脑的名字>，再按 a 重新生成".to_string(),
+            ),
         }
     }
 
@@ -3268,6 +3299,11 @@ mod tests {
             Mesh(crate::proto::MeshProblem::CodeMismatch),
             Mesh(crate::proto::MeshProblem::TooLong),
             Mesh(crate::proto::MeshProblem::BadAddress("pc".into())),
+            Mesh(crate::proto::MeshProblem::BadInviteCode),
+            Mesh(crate::proto::MeshProblem::WrongInviteCode),
+            Mesh(crate::proto::MeshProblem::NoInvite { unanswered: 0 }),
+            Mesh(crate::proto::MeshProblem::NoInvite { unanswered: 2 }),
+            Mesh(crate::proto::MeshProblem::InviteRosterRefused),
             // `LoginFailed` 不在这里：它带的原因是网关那层给的中文，同 `Git`
             // 照抄原文，英文里会有汉字，见下面那条单独的测试。
         ];
diff --git a/src/proto.rs b/src/proto.rs
index 3983cfb..5b474ff 100644
--- a/src/proto.rs
+++ b/src/proto.rs
@@ -1227,6 +1227,16 @@ pub enum MeshProblem {
     TooLong,
     /// 地址不是 `电脑名/会话名`。
     BadAddress(String),
+    /// `dct join` 后面敲的不是 6 位数字（空格、`-` 已经去掉了）。
+    BadInviteCode,
+    /// 码不对、已过期或已被用掉：发邀请的那台已经把它作废了。
+    WrongInviteCode,
+    /// 同账号在线的电脑里没有一台发着邀请码。其中 `unanswered` 台连话都没回
+    /// ——多半是 dct 太旧，不认识 `InviteProbe`。
+    NoInvite { unanswered: u32 },
+    /// 老电脑已经把这台签进了名单，但发来的那份名单这边验不过（签名、组、
+    /// 我的钥匙对不上），没进组。
+    InviteRosterRefused,
 }
 
 /// 把一个 `ErrorCode` 塞进 `anyhow::Error` 里带出去。
```

- [ ] **Step 4: 实现——`join`**

放在 `src/mesh/invite.rs` 里 `flush_outbox` 之后、测试模块之前：

```rust
/// 新电脑上 `dct join <码> [--name 名字]`。
///
/// 1. 问一遍同账号在线的电脑（最多 `MAX_JOIN_ASK` 台，排序去重）：`InviteProbe`；
/// 2. 答 `InviteOpen` 的（自签对、端点就是问的那台）按端点排好，最多试
///    `MAX_INVITERS_TRIED` 台，逐台走 SPAKE2：`InviteJoin` → 验 `cA` →
///    `InviteFinish` → 收 `InviteDone` 里的名单；
/// 3. 名单验过（签名者就是 PAKE 里绑定的那台 A、组对得上、我那一条是我的
///    钥匙）才进组。
///
/// **收到 `InviteDone` 之前不改任何本地状态**：`name` 只用在发出去的那份
/// 记录里，进组之后才存下来。
pub fn join(
    mesh: &Mutex<Mesh>,
    net: &dyn Net,
    code: &str,
    name: Option<&str>,
) -> Result<(), MeshProblem> {
    let code = Code::parse(code).ok_or(MeshProblem::BadInviteCode)?;
    let chosen = name.map(str::trim).filter(|n| !n.is_empty());
    let (me, sig) = {
        let m = lock(mesh);
        if !m.is_alone() {
            return Err(MeshProblem::AlreadyInGroup);
        }
        let mut me = m.me.clone();
        if let Some(n) = chosen {
            if !super::valid_name(n) {
                return Err(MeshProblem::BadName);
            }
            me.name = n.to_string();
        }
        let sig = wire::sign_member(&me, &m.keys);
        (me, sig)
    };

    let mut peers = net.peers().unwrap_or_default();
    peers.sort();
    peers.dedup();
    if peers.len() > super::MAX_JOIN_ASK {
        lock(mesh)
            .journal
            .mesh(&format!("join_too_many_peers n={}", peers.len()));
        return Err(MeshProblem::TooManyAnswered);
    }

    // 并排探问：有一台卡住不该拖着别的。
    let answers: Vec<(String, Option<Payload>)> = std::thread::scope(|s| {
        let hs: Vec<_> = peers
            .iter()
            .map(|p| {
                s.spawn(move || {
                    let r = net
                        .ask(p, wire::encode(&Payload::InviteProbe), INVITE_ASK_TIMEOUT)
                        .ok()
                        .and_then(|b| wire::decode(&b).ok());
                    (p.clone(), r)
                })
            })
            .collect();
        hs.into_iter().filter_map(|h| h.join().ok()).collect()
    });
    let mut unanswered = 0u32;
    let mut inviters: Vec<(Member, String)> = Vec::new();
    for (peer, answer) in answers {
        match answer {
            Some(Payload::InviteOpen { member, sig, group })
                if member.endpoint == peer
                    && wire::verify_member(&member, &sig)
                    && super::valid_name(&member.name)
                    && super::valid_kx_pub(&member.kx_pub)
                    && !group.is_empty() =>
            {
                inviters.push((member, group))
            }
            Some(Payload::NoInvite) => {}
            // 没回话、回了认不出的东西（旧版 dct 不认识 `InviteProbe`，
            // 直接丢掉不回），或者回的 `InviteOpen` 验不过。
            _ => unanswered += 1,
        }
    }
    if inviters.is_empty() {
        return Err(MeshProblem::NoInvite { unanswered });
    }
    inviters.sort_by(|x, y| x.0.endpoint.cmp(&y.0.endpoint));
    inviters.truncate(MAX_INVITERS_TRIED);

    let mut refused = false;
    for (inviter, group) in &inviters {
        match attempt(mesh, net, &code, inviter, group, &me, &sig) {
            Ok(roster) => return settle(mesh, roster, inviter, chosen.is_some()),
            Err(Attempt::Mismatch) => {}
            Err(Attempt::Unreachable) => {}
            Err(Attempt::BadRoster) => refused = true,
        }
    }
    Err(if refused {
        MeshProblem::InviteRosterRefused
    } else {
        MeshProblem::WrongInviteCode
    })
}

enum Attempt {
    /// 码对不上，或者那台说不行（码已作废、正有别人在用）。
    Mismatch,
    /// 没回话、回了认不出的东西。
    Unreachable,
    /// 走完了，A 也签了名单，但名单这边验不过。
    BadRoster,
}

/// 跟一台发着码的电脑走完 SPAKE2，拿回它签的名单（已经验过）。
fn attempt(
    mesh: &Mutex<Mesh>,
    net: &dyn Net,
    code: &Code,
    inviter: &Member,
    group: &str,
    me: &Member,
    sig: &str,
) -> Result<dct_mesh::SignedRoster, Attempt> {
    let seed = lock(mesh).fresh_nonce();
    let hs = Handshake::joiner(code, inviter, me, seed);
    let join = Payload::InviteJoin {
        member: me.clone(),
        sig: sig.to_string(),
        spake: wire::encode_bytes(hs.message()),
    };
    let to = inviter.endpoint.as_str();
    let reply = net
        .ask(to, wire::encode(&join), INVITE_ASK_TIMEOUT)
        .map_err(|_| Attempt::Unreachable)?;
    let (msg_a, tag_a) = match wire::decode(&reply) {
        Ok(Payload::InviteKey { spake, confirm }) => (
            wire::decode_len(&spake, MSG_LEN).ok_or(Attempt::Mismatch)?,
            STANDARD.decode(confirm).map_err(|_| Attempt::Mismatch)?,
        ),
        Ok(Payload::InviteFailed) => return Err(Attempt::Mismatch),
        _ => return Err(Attempt::Unreachable),
    };
    let confirmed = hs.finish(group, &msg_a).map_err(|_| Attempt::Mismatch)?;
    // 先验 A：对不上就停，不发 `InviteFinish`（A 那边到点自己作废）。
    if !confirmed.inviter_tag_ok(&tag_a) {
        lock(mesh)
            .journal
            .mesh(&format!("join_wrong_code via={to}"));
        return Err(Attempt::Mismatch);
    }
    let finish = Payload::InviteFinish {
        confirm: wire::encode_bytes(&confirmed.joiner_tag()),
    };
    let reply = net
        .ask(to, wire::encode(&finish), INVITE_ASK_TIMEOUT)
        .map_err(|_| Attempt::Unreachable)?;
    let roster = match wire::decode(&reply) {
        Ok(Payload::InviteDone { roster }) => roster,
        Ok(Payload::InviteFailed) => return Err(Attempt::Mismatch),
        _ => return Err(Attempt::Unreachable),
    };
    // 名单：签名者 == PAKE 里绑定的 A、签名在 A 的钥匙下成立、A 和我都在上面
    // 且钥匙没被换（`accept_invite`）；组就是 `T` 里的那个；我那一条的加密
    // 公钥也是我的（`accept_invite` 只看得到我的端点）。
    let mine_ok = roster
        .roster
        .member(&me.endpoint)
        .is_some_and(|m| m.sign_pub == me.sign_pub && m.kx_pub == me.kx_pub);
    if dct_mesh::roster::accept_invite(&roster, &me.endpoint, inviter).is_err()
        || roster.roster.group != group
        || !mine_ok
    {
        lock(mesh)
            .journal
            .mesh(&format!("join_bad_roster via={to}"));
        return Err(Attempt::BadRoster);
    }
    Ok(roster)
}

/// 验过的名单落盘、换上。`chose_name`：用户用 `--name` 起过名字，从此不再算
/// dct 起的。
fn settle(
    mesh: &Mutex<Mesh>,
    roster: dct_mesh::SignedRoster,
    inviter: &Member,
    chose_name: bool,
) -> Result<(), MeshProblem> {
    let mut m = lock(mesh);
    if !m.is_alone() {
        return Err(MeshProblem::AlreadyInGroup);
    }
    m.commit(roster).map_err(|_| MeshProblem::NotSaved)?;
    if chose_name {
        let name = m.me.name.clone();
        if let Some(s) = &m.store {
            let _ = s.set_name(&name);
        }
        m.auto_name = false;
    }
    m.journal
        .mesh(&format!("joined via={}", inviter.endpoint));
    Ok(())
}
```

- [ ] **Step 5: 跑测试确认通过**

Run: `~/.cargo/bin/cargo test --lib mesh::invite && ~/.cargo/bin/cargo test --lib i18n`
Expected: `mesh::invite::tests::` 44 条全过，i18n 全过。

- [ ] **Step 6: 全量检查**

```bash
~/.cargo/bin/cargo test --workspace -- --test-threads=4
~/.cargo/bin/cargo clippy --workspace --all-targets -- -D warnings
~/.cargo/bin/cargo check --workspace --all-targets --target x86_64-pc-windows-msvc
```
Expected：`test result: ok.` 全绿（`pty::tests::*` / `session::tests::*` 偶发失败见 Global Constraints 的「已知 flaky」，单独重跑）；clippy `Finished`、无 warning；Windows check `Finished`（`src/student_projects.rs:669` 那条 `unused variable: path` 是本来就有的）。

- [ ] **Step 7: Commit**

```bash
git add src/mesh/invite.rs src/proto.rs src/i18n.rs
git commit -m "feat(mesh): joiner side of the invite code, with relay-tampering tests" -m "Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---
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
### Task 9: 文档——两份 README 的「多电脑」一节、旧设计文档上的指路

**Files:**
- Modify: `README.zh-CN.md`（`## 多电脑` 一节）
- Modify: `README.md`（`## Several computers` 一节）
- Modify: `docs/superpowers/specs/2026-09-28-dct-multi-machine-design.md`（「### 加一台新电脑」「#### 6 位数怎么算（dct-sas-v2）」两个标题下各加一行，**不删原文**）

**Interfaces:** 无代码。文档里的命令、文案必须跟 Task 5–7 实现的一字不差（`邀请码 482 913 · 10 分钟内有效`、`dct join 482913`、`dct invite`、看板 `a`）。

- [ ] **Step 1: 先确认文档里还写着旧流程（这一步的「失败的测试」）**

```bash
grep -n -E "peers approve|compare the 6 digits|核对 6 位数|两边的人都看一眼" README.md README.zh-CN.md
```

Expected: 两份 README 都有命中（旧的 `dct peers approve`、「两边的人都看一眼」那一段）。

- [ ] **Step 2: 改 `README.zh-CN.md`**

把 `## 多电脑` 下面第一个代码块到「**现在还做不到的：**」列表的最后一条（`- 还不能「进入」别的电脑上的会话看屏幕、打字，这是下一步。`）整段换成：

````markdown
```
dct login                       # 老电脑上：用 DC 账号登录；第一台顺手建「我的电脑」组
dct invite                      # 老电脑上：出一个 6 位邀请码（看板上按 a 也一样）
dct join 482913                 # 新电脑上：敲那个码（没登录会先自动登录）
dct peers                       # 组里有哪几台、各开着哪些会话
dct send Mac/#3 "跑一下 Windows 测试"
```

- **登录**：`dct login` 用配对过的 DC 账号换一张中转令牌，只在这台电脑上存着。
  第一台登录的电脑顺手建一个「我的电脑」组。
- **加电脑只要一个码**：老电脑的看板上按 `a`（或者运行 `dct invite`），屏幕上出现
  `邀请码 482 913 · 10 分钟内有效`；新电脑上 `dct join 482913`，就进组了——
  `已加入「我的电脑」，组里有：Mac、公司电脑`，老电脑的看板上提示「公司电脑 已加入」。
  码里的空格、`-` 随便敲，全角数字也认。新电脑必须已经配对了**同一个** DC 账号：别的
  账号的电脑连问都问不到你的电脑。
- **码只能用一次**：用过一次（成不成都算）、过了 10 分钟、或者再按一次 `a` 换了新码，
  旧码就作废。有人拿错码试过一次，老电脑上会提示「有人用错码试过一次，码已作废」，
  重新按 `a` 就行。守护进程重启之后码也没了，重新按 `a`。
- **中转猜不出码**：码是一次 PAKE（SPAKE2）的口令，中转看得见来往的每一条消息也算不出
  它；冒充一台电脑，每个码只有一次百万分之一的机会。
- **看到码的人就能进来**（只要他也有你这个 DC 账号）：投屏、截图的时候留意。码就是
  同意，老电脑上不会再问一遍。
- **名字**：默认叫 Mac / Windows / Linux；组里撞名了，老电脑自动编成「Mac 2」（用
  `--name` 起的名字也一样，新电脑会告诉你最后叫什么）。想自己起名：
  `dct join 482913 --name 公司电脑`。
- **留言**：`dct send <电脑名>/<会话名> "<内容>"`，会话名也可以写 `#编号`。对方
  看到的是这样一段：

  ```
  [来自 家里Mac/写文档 的留言 #a1b2]
  跑一下 Windows 测试
  ```

  那个会话空着就马上敲进去；正在干活就排队，空下来再送，一次一条。
- **中转看不到内容**：留言在你的电脑上就加密好，只有组里那台收件的电脑解得开；
  中转只管转发，不落盘，也读不出一个字。
- **中转地址**默认是 `https://dataclue.cn/dct-relay`，要换在 `~/.dct/config.toml`
  里写 `[mesh] relay = "…"`。

**现在还做不到的：**

- 对方电脑不在线，留言就没送出去（会告诉你），不会替你存着等它上线；
- 只能送进 agent 会话，送不进普通的命令行会话；
- 排着队的留言、发着的邀请码，守护进程一重启就没了；
- 两台老电脑同时各拉一台新电脑进组，名单可能分叉；不在线的电脑会错过名单变化；
- 同账号里谁都能不停拿错码去试、把你的码一个个烧掉（进不来，但烦人），这一版不防；
- 还不能「进入」别的电脑上的会话看屏幕、打字，这是下一步。
````

再把「### 部署」第 4 条的 `dct login` → `dct join` → `dct peers approve` → `dct send` 换成 `dct login` → `dct invite` → `dct join <码>` → `dct send`。

- [ ] **Step 3: 改 `README.md`**

把 `## Several computers` 下面第一个代码块到「**Not yet:**」列表的最后一条（`- you can't open another computer's session yet — that's the next step.`）整段换成：

````markdown
```
dct login                       # on an existing computer: sign in with the DC account
dct invite                      # on an existing computer: show a 6-digit invite code (or press a on the board)
dct join 482913                 # on the new one: type that code (signs in first if needed)
dct peers                       # who is in the group, what each has open
dct send Mac/#3 "run the Windows tests"
```

- **Sign in**: `dct login` trades the paired DC account for a relay token, kept on
  that computer only. The first computer to sign in creates the group.
- **Adding a computer takes one code**: press `a` on the board of a computer that is
  already in (or run `dct invite`); it shows `Invite code 482 913 · valid for 10 minutes`.
  On the new computer run `dct join 482913` and it is in — the old one's board says
  "<name> has joined". Spaces, dashes and full-width digits in the code are fine. The
  new computer must be paired with the **same** DC account: computers on other accounts
  cannot even reach yours.
- **A code works once**: used once (right or wrong), 10 minutes old, or replaced by
  pressing `a` again — then it is gone. If someone tried a wrong code, the old
  computer says so; press `a` for a new one. Restarting the daemon also drops the code.
- **The relay can't guess it**: the code is the password of a PAKE (SPAKE2), so the
  relay sees every message and still learns nothing; impersonating a computer is a
  one-in-a-million shot per code.
- **Whoever sees the code can join** (if they also hold your DC account): mind screen
  sharing and screenshots. The code is the approval; the old computer does not ask again.
- **Names**: a computer is called Mac / Windows / Linux by default; a clash in the
  group is numbered by the old computer ("Mac 2"), even for a name set with `--name` —
  the new computer tells you what it ended up as. To pick your own:
  `dct join 482913 --name work-pc`.
- **Messages**: `dct send <computer>/<session> "<text>"`, the session can be `#id`.
  The other side sees a marker line (`[来自 home-mac/docs 的留言 #a1b2]`) and then
  the text. Idle session: typed now. Busy: queued, one at a time.
- **The relay can't read them**: sealed on your computer for the one recipient;
  the relay forwards, never writes to disk, and sees ciphertext only.
- The relay defaults to `https://dataclue.cn/dct-relay`; override with
  `[mesh] relay = "…"` in `~/.dct/config.toml`.

**Not yet:**

- the other computer offline → not sent (you are told); nothing is held for later;
- only agent sessions receive, not plain terminal sessions;
- queued messages and a live invite code are lost when the daemon restarts;
- two old computers each adding a newcomer at once can fork the group list, and an
  offline computer misses list changes;
- any computer on your account can keep burning your codes with wrong guesses
  (it cannot get in, but it is annoying); this version does not stop that;
- you can't open another computer's session yet — that's the next step.
````

再把「### Deploying」第 4 条的 `dct login` → `dct join` → `dct peers approve` → `dct send` 换成 `dct login` → `dct invite` → `dct join <code>` → `dct send`；最后一段里的 `walks login → join → approve → peers → send` 换成 `walks login → invite → join → peers → send`。

- [ ] **Step 4: 旧设计文档上指路**

`docs/superpowers/specs/2026-09-28-dct-multi-machine-design.md` 里，`### 加一台新电脑` 和 `#### 6 位数怎么算（dct-sas-v2）` 两个标题的**下一行**各插入（后面空一行，原文一字不动）：

```markdown
> **已被替换（2026-09-30）：** 这一节已经被 `2026-09-30-dct-invite-code-design.md`（6 位邀请码 + SPAKE2，dct-invite-v1）替换，代码里已经没有这套流程；下面的原文只留作记录。
```

- [ ] **Step 5: 确认旧说法不剩**

```bash
grep -n -E "peers approve|compare the 6 digits|核对 6 位数" README.md README.zh-CN.md
grep -c "已被替换（2026-09-30）" docs/superpowers/specs/2026-09-28-dct-multi-machine-design.md
```

Expected: 第一条没有输出；第二条输出 `2`。

- [ ] **Step 6: Commit**

```bash
git add README.md README.zh-CN.md docs/superpowers/specs/2026-09-28-dct-multi-machine-design.md
git commit -m "docs(mesh): add a computer with a one-time invite code" -m "Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 10: 手工变异测试、最终验收

照第一步（`.superpowers/sdd/2026-09-28-dct-multi-machine-step1/task-6-report.md` 的 “Hand mutation pass”）的做法：每个变异打上、跑相关测试、还原、用 `cmp` 确认还原干净。重点就是 spec 点名的五样：「进入 `InFlight` 即作废」、身份绑定、`env.from == InFlight.peer`、常数时间比较、B 提前改状态。

**Files:**
- Create: `.superpowers/sdd/2026-09-30-dct-invite-code/mutation-report.md`（结果记录；这是 SDD 台账，跟 step 1 的报告放一起）
- 不改任何代码（有变异活下来就回到对应任务补测试，补的测试单独提交）

**Interfaces:** 无。

- [ ] **Step 1: 变异脚本**

存成 `/tmp/dct-invite-mutants.py`（在 worktree 根目录跑；每个变异的「原文」都必须在文件里**恰好出现一次**，否则脚本报 `SKIP`——那说明代码跟本计划不一致，先查清楚）：

```python
import subprocess, sys, os, filecmp, shutil, tempfile
# 在 worktree 根目录跑：python3 /tmp/dct-invite-mutants.py [名字前缀...]
CARGO = os.path.expanduser('~/.cargo/bin/cargo')
BAK = os.path.join(tempfile.gettempdir(), 'dct-invite-mutant.bak')
M=[
 ("M1 T 不含 group", "crates/dct-mesh/src/invite.rs", "    field(&mut out, group);\n", "", ["-p","dct-mesh","--","invite"]),
 ("M2 rec 不含 kx_pub", "crates/dct-mesh/src/invite.rs", "    field(&mut out, &m.kx_pub);\n    out\n}\n\n/// SPAKE2 的口令字节。", "    out\n}\n\n/// SPAKE2 的口令字节。", ["-p","dct-mesh","--","invite"]),
 ("M6 拒绝采样 < 改 <=", "crates/dct-mesh/src/invite.rs", "if x < SAMPLE_LIMIT {", "if x <= SAMPLE_LIMIT {", ["-p","dct-mesh","--","invite"]),
 ("M8 常数时间改 ==", "crates/dct-mesh/src/invite.rs", "        m.verify_slice(tag).is_ok()\n    }\n\n    /// 邀请方验 `cB`。", "        m.finalize().into_bytes().as_slice() == tag\n    }\n\n    /// 邀请方验 `cB`。", ["-p","dct-mesh","--","invite"]),
 ("M9 算不出 SPAKE2 不作废", "src/mesh/invite.rs", "                self.end_invite(InviteOutcome::Burned, \"invite_bad_spake_point\");\n", "", ["--lib","--","mesh::invite"]),
 ("M10 InviteFinish 不查 peer", "src/mesh/invite.rs", "        if peer != from {", "        if false && peer != from {", ["--lib","--","mesh::invite"]),
 ("M11 deadline > 改 >=", "src/mesh/invite.rs", "        if (self.clock)() > *deadline {", "        if (self.clock)() >= *deadline {", ["--lib","--","mesh::invite"]),
 ("M12 expires >= 改 >", "src/mesh/invite.rs", "            }) if now >= *expires => InviteOutcome::Expired,", "            }) if now > *expires => InviteOutcome::Expired,", ["--lib","--","mesh::invite"]),
 ("M13 不查 endpoint==from", "src/mesh/invite.rs", "        if member.endpoint != from || !wire::verify_member(&member, sig) {", "        if !wire::verify_member(&member, sig) {", ["--lib","--","mesh::invite"]),
 ("M15 不查已在组里", "src/mesh/invite.rs", "            .is_some_and(|r| r.roster.member(from).is_some())", "            .is_some_and(|r| r.roster.member(from).is_some() && false)", ["--lib","--","mesh::invite"]),
 ("M16 不编号", "src/mesh/invite.rs", "        if taken.contains(&joiner.name) {", "        if false {", ["--lib","--","mesh::invite"]),
 ("M17 B 不查 Open 的端点", "src/mesh/invite.rs", "                if member.endpoint == peer\n", "                if true\n", ["--lib","--","mesh::invite"]),
 ("M18 B 不验 cA", "src/mesh/invite.rs", "    if !confirmed.inviter_tag_ok(&tag_a) {", "    if false {", ["--lib","--","mesh::invite"]),
 ("M19 B 不查组", "src/mesh/invite.rs", "        || roster.roster.group != group\n", "", ["--lib","--","mesh::invite"]),
 ("M20 B 不查我的钥匙", "src/mesh/invite.rs", "        || !mine_ok\n", "", ["--lib","--","mesh::invite"]),
 ("M21 --name 先落盘", "src/mesh/invite.rs", "            me.name = n.to_string();\n", "            me.name = n.to_string();\n            if let Some(s) = &m.store { let _ = s.set_name(n); }\n", ["--lib","--","mesh::invite"]),
 ("M22 广播也发给 B", "src/mesh/invite.rs", "            .filter(|ep| *ep != joiner.endpoint)\n", "", ["--lib","--","mesh::invite"]),
 ("M23 不限 3 台", "src/mesh/invite.rs", "    inviters.truncate(MAX_INVITERS_TRIED);\n", "", ["--lib","--","mesh::invite"]),
 ("M24 >16 改 >=16", "src/mesh/invite.rs", "    if peers.len() > super::MAX_JOIN_ASK {", "    if peers.len() >= super::MAX_JOIN_ASK {", ["--lib","--","mesh::invite"]),
 ("M25 连按 a 不拦", "src/ui/computers.rs", "    if app.mesh.invite_rx.is_some() {\n        return;\n    }\n", "", ["--lib","--","ui::computers"]),
 ("M26 别处的码也报", "src/ui/computers.rs", "    if app.mesh.created != Some(n.id) || app.mesh.announced == Some(n.id) {", "    if app.mesh.announced == Some(n.id) {", ["--lib","--","ui::computers"]),
 ("M27 CLI 不按 id 认结果", "src/mesh/cli.rs", ".filter(|n| n.id == v.id)", "", ["--lib","--","mesh::cli"]),
 ("M28 Debug 露码", "src/proto.rs", '.field("code", &if code.is_empty() { "" } else { "******" })', '.field("code", code)', ["--lib","--","proto::"]),
 ("M29 journal 记码", "src/mesh/invite.rs", '        self.journal.mesh(&format!("invite_created id={}", view.id));', '        self.journal.mesh(&format!("invite_created id={} code={}", view.id, view.code));', ["--lib","--","mesh::invite"]),
 ("M30 InFlight 期间探问仍答 Open", "src/mesh/invite.rs", "        let live = matches!(\n            self.invite,\n            Some(Invite {\n                state: State::Live,\n                ..\n            })\n        );", "        let live = self.invite.is_some();", ["--lib","--","mesh::invite"]),
]

only = sys.argv[1:]
for name, path, old, new, args in M:
    if only and not any(name.startswith(o) for o in only):
        continue
    src = open(path).read()
    if src.count(old) != 1:
        print("SKIP(no unique match)", name, src.count(old)); continue
    shutil.copy(path, BAK)
    open(path, 'w').write(src.replace(old, new))
    try:
        r = subprocess.run([CARGO, 'test', '-q'] + args, capture_output=True, text=True)
    finally:
        shutil.copy(BAK, path)
    assert filecmp.cmp(path, BAK, shallow=False), "没还原干净：" + path
    out = r.stdout + r.stderr
    status = "KILLED" if r.returncode != 0 else "SURVIVED"
    if "could not compile" in out:
        status = "COMPILE-ERROR"
    print(status, name, flush=True)
```

- [ ] **Step 2: 跑，期望全部 KILLED**

Run: `python3 /tmp/dct-invite-mutants.py 2>&1 | tee /tmp/dct-invite-mutants.out`
Expected（写计划时在同一份代码上实测，25 个全部 `KILLED`，约 10 分钟）：

```
KILLED M1 T 不含 group
KILLED M2 rec 不含 kx_pub
KILLED M6 拒绝采样 < 改 <=
KILLED M8 常数时间改 ==
KILLED M9 算不出 SPAKE2 不作废
KILLED M10 InviteFinish 不查 peer
KILLED M11 deadline > 改 >=
KILLED M12 expires >= 改 >
KILLED M13 不查 endpoint==from
KILLED M15 不查已在组里
KILLED M16 不编号
KILLED M17 B 不查 Open 的端点
KILLED M18 B 不验 cA
KILLED M19 B 不查组
KILLED M20 B 不查我的钥匙
KILLED M21 --name 先落盘
KILLED M22 广播也发给 B
KILLED M23 不限 3 台
KILLED M24 >16 改 >=16
KILLED M25 连按 a 不拦
KILLED M26 别处的码也报
KILLED M27 CLI 不按 id 认结果
KILLED M28 Debug 露码
KILLED M29 journal 记码
KILLED M30 InFlight 期间探问仍答 Open
```

任何一条 `SURVIVED`：找出是哪条测试该拦没拦住，回到拥有那段代码的任务补一条测试（先看它在未变异的代码上是绿的、在变异上是红的），单独提交 `test(mesh): kill mutant <名字>`，再重跑这一条（`python3 /tmp/dct-invite-mutants.py M18`）。`COMPILE-ERROR` 说明变异写错了，不算数，改脚本重跑。跑完 `git status --short` 必须是干净的（脚本会还原）。

- [ ] **Step 3: 最终验收**

```bash
~/.cargo/bin/cargo test --workspace -- --test-threads=4
~/.cargo/bin/cargo clippy --workspace --all-targets -- -D warnings
~/.cargo/bin/cargo check --workspace --all-targets --target x86_64-pc-windows-msvc
~/.cargo/bin/cargo test --test mesh_e2e -- --ignored --nocapture
~/.cargo/bin/cargo tree -p dct-mesh -e normal | grep -E "spake2|getrandom"
git log --oneline docs/dct-invite-code..HEAD
```

Expected: 全绿；clippy 无 warning；Windows check `Finished`（没装 target 就如实写）；e2e `ok`；`cargo tree` 只列出 `spake2 v0.4.0`；`git log` 列出 Task 1–9 的 9 个提交（加上补测试的，如果有）。

- [ ] **Step 4: 写台账并提交**

`.superpowers/sdd/2026-09-30-dct-invite-code/mutation-report.md` 写：跑的命令、25 个变异各自的结果（照抄 `/tmp/dct-invite-mutants.out`）、补过的测试（没有就写「无」）、Step 3 每条命令的结果摘要（数字照抄，比如 `test result: ok. 1620 passed`）、flaky 重跑过哪几条、Windows check 跑没跑成。

```bash
git add .superpowers/sdd/2026-09-30-dct-invite-code/mutation-report.md
git commit -m "docs(mesh): mutation pass and acceptance record for the invite code" -m "Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

- [ ] **Step 5: 真电脑验收（上线动作，由控制端问过用户之后才做，子 agent 不做）**

两台真电脑（一台 Mac、一台 Windows，同一个 DC 账号，都装这个分支编出来的 dct）：老电脑看板按 `a` → 新电脑 `dct join <码>`（故意带空格敲）→ 两边 `dct peers` 都看到对方 → `dct send` 一条两行的留言。再验一次错码：新电脑敲错一位 → 看到「邀请码不对或已作废…」，老电脑看板提示「有人用错码试过一次，码已作废，按 a 重新生成」。再验 `dct invite` 按 Ctrl-C 之后码收回。结果记进台账。

---

## 自检记录

**1. spec 覆盖**

| spec | 任务 |
|---|---|
| 第 1 段 老电脑：看板 `a`、`dct invite`、码 + 有效期 + 倒计时 | 6、7 |
| 码一次性、再按一次换新码、组里只有自己也能邀请 | 3（`a_second_invite_voids_the_first_code`、`the_right_code_brings_the_joiner_in…` 用的就是只有一台的组） |
| 新电脑 `dct join 482913 [--name]`、忽略空格和 `-`、自动登录、已在别的组就拒 | 1、4、5 |
| 成功/出错的文案（码不对、找不到发邀请的电脑、两台同时发、守护进程重启） | 4、5、6、7（裁决 4） |
| 删掉 `peers approve`、`--confirm`、y/n 行、待批准列表 | 8 |
| 第 2 段 A 的状态、`getrandom` 拒绝采样、只在内存 | 1、3 |
| 三次 ask、`MAX_JOIN_ASK`、排序去重、自签名 + `endpoint == from` | 2、3、4 |
| 作废规则（进 `InFlight` 之前不作废、之后必作废） | 3（`failures_before_in_flight_do_not_burn_the_code`、`a_spake_message_that_is_not_a_point_burns_the_code`）、10（M9） |
| SPAKE2 输入（口令、身份 = `rec`）、确认值、常数时间 | 1（固定向量、篡改、源码守卫）、10（M1、M2、M8） |
| B 先验 `cA`、不对不发 `InviteFinish` | 4（`count("invite_finish") == 0`）、10（M18） |
| B 收名单的四条 + B 在 `InviteDone` 前不改本地状态 | 4（`a_tampered_invite_done_roster_is_refused` 五种、`nothing_changes_locally_until_invite_done_arrives`）、10（M19–M21） |
| 版本：8 种新消息、删 3 种、`sas.rs`、协议号 +1、旧 A 当 `NoInvite` 提示升级 | 2、5、8、4（`an_old_dct…`） |
| 第 3 段模块表（dct-mesh/invite.rs、wire、src/mesh/invite.rs、mod/group、proto、cli、computers、i18n、README、旧 spec 指路） | 1–9 |
| 测试清单：固定向量、码对/差一位、换任一项、换 group、10⁶ 分布 | 1 |
| 对抗：换 B 的钥匙（两种，见裁决 5）/ 换 A 的钥匙、抢先错码、冒充 `InviteOpen`/`InviteKey`、`InviteFinish` 从别处来、重放、过期、作废后再试、两台同时邀请、>16 台、B 已在组里、B 状态不变 | 3、4 |
| 端到端 invite → join → peers → send | 5、8 |
| 变异测试 | 10 |

「安全隐患」一节的第 1、2、5 条写进了 README（Task 9）；第 3、4、6、7 条本设计不改。

**2. 占位扫描**：没有 TBD/TODO/「类似 Task N」；每个改代码的步骤都有完整代码或完整补丁；所有补丁在写计划时从 `70546bb` 的干净副本上按顺序重放、编译、全量测试、clippy 过。

**3. 类型一致**：`InviteView { id, code, expires_at }`、`InviteNote { id, outcome }`、`InviteOutcome::{Joined { name }, Burned, Expired}` 在 Task 3 定义，Task 5–7 用；`Mesh::start_invite`/`cancel_invite`/`invite_view`/`expire_invite` 在 Task 3 定义，Task 5 的守护进程和 `group::view` 用；`invite::join(mesh, net, code, name: Option<&str>)` 在 Task 4 定义，Task 5 的 `handle_mesh` 用；`MeshProblem::{BadInviteCode, WrongInviteCode, NoInvite { unanswered }, InviteRosterRefused}` 在 Task 4 定义，Task 5–6 的命令行测试用；`computers::mesh_view` 在 Task 8 从三参数改成两参数，Task 8 同时改了所有调用处（`board.rs` 测试）。

**4. Review Focus**：五条各自的测试都已写进拥有那段代码的任务（见 Review Focus 一节每条后面的测试名）。
