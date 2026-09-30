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
