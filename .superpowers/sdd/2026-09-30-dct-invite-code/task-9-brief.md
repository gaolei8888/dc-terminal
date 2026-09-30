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

