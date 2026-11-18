# 平稳重启：等会话停下、记住状态、重启后接着干

日期：2026-10-14
状态：设计，用户 2026-10-14 拍板；没有代码。
背景：后台重启会杀掉所有会话（pty 在后台进程里）。用户有 18 个会话，因此一直不敢重启——10 月 3 号以来约 90 处后台改动卡着没生效。用户要：重启前通知各会话、记住每个会话的状态、重启后还在干活的会话自动接着干。

## 用户定下的几条

1. 在忙的会话**先等它干完这一段**，最长 15 分钟（A）。
2. 15 分钟还在忙的，**再发一句让它在安全处停下、写一句做到哪了**，最多再等 3 分钟，然后照样重启（C 兜底）。
3. **能准确接回对话的会话不写记忆、不写交接**——对话本身就是交接；接不回的，重启前自动让它写一份交接。
4. 「准备重启」期间用户自己在界面里打字不拦；等待时间可调，默认 15 + 3 分钟。

## 现状（核对过代码）

- `src/last_sessions.rs`：`~/.dct/last-sessions.toml` 记 `RecordedSession { dir, profile, tag, last_active }`，**不记对话 id**——当初设计时有意划出范围（文件头注释）。本设计推翻这一条，理由见第 1 节。
- `src/daemon.rs:128-158` + `restore_last_sessions`（`:735-805`）：新后台启动时按清单把会话全部重开。
- `group_for_resume`（`last_sessions.rs:145`）：同一 (目录, profile) 只有最近用的那个拿 `resume_args`（claude 系是 `--continue`），**其余从空白开始**。codex / opencode / qwen 没有 `resume_args`，一律从空白开始。
- `SessionState`（`session.rs:56`）：`Working` / `Idle` / `Stopped` / … 能区分「在忙」和「空闲等人」；每 200ms `tick()` 判一次。
- 往会话里打字：`bridge::submit`（文字 + 空串=回车）是唯一的按键路径；mesh 留言只在 `Idle` 时投递、否则排队（`mesh/deliver.rs`）。
- `dct restart [-y]`（`cli.rs:386-450`）→ `client::restart_daemon`（`client.rs:22-60`）：SIGTERM、等 2 秒、硬杀、`spawn_daemon`。**后台不处理 SIGTERM**，子进程靠 pty 关闭时的 SIGHUP 死掉。没有任何通知。
- **坑**：在某个 dct 会话里执行 `dct restart`（比如 agent 自己去敲），旧后台一死，这个命令所在的 pty 关闭，命令自己也被 SIGHUP 杀掉，新后台起不来。

## 设计

### 1. 记住每个会话的对话 id（profile 声明，dct 不写死工具名）

当初不记对话 id，是因为「接回同一个对话」交给 `--continue` 按目录去猜。这在同目录多会话时只接回一个，而平稳重启的前提就是**每个会话都接回自己的对话**，所以改为记 id。

profile 新增两个可选字段：

```toml
# 开会话时由 dct 生成一个 UUID，用这组参数交给 agent
session_id_args = ["--session-id", "{id}"]
# 重启后按 id 接回
resume_id_args = ["--resume", "{id}"]
```

| profile | 工具 | 声明 | 说明 |
|---|---|---|---|
| claude、deepseek、glm、kimi、qwen-api | claude | 两项都声明 | `claude --help` 有 `--session-id <uuid>`、`--resume <id>` |
| qwen、dc | qwen | 两项都声明，**但须先实测** | `qwen --help` 有 `--session-id`、`--resume`；按 `unverified_profiles_declare_no_resume_args` 的规矩，没实测过不许声明 |
| codex、opencode | — | 不声明 | 开会话时没法指定 id → 视为「接不回」 |
| shell | — | 不声明 | 不是 agent，不需要 |

- `RecordedSession` 加 `conversation_id: Option<String>`（serde default，旧文件照读）。
- 新建会话：profile 有 `session_id_args` 就生成 UUID、拼进命令、记下来。
- 恢复：有 `conversation_id` 且 profile 有 `resume_id_args` → 用它；否则走原来的 `group_for_resume` / `--continue` 逻辑（旧清单、旧会话不受影响）。
- 接回时 id 不变（claude `--resume` 不加 `--fork-session` 时续写同一个对话），所以下次重启还能用。
- 接回失败（对话文件没了、agent 报错退出）：退回原逻辑重开，并在 `set_resume_skips` 那条路上告诉用户「这个会话没能接回原来的对话」。

### 2. 记住谁「被打断了」

`RecordedSession` 加 `interrupted: bool`（serde default false）。只在平稳重启的最后一步写：重启那一刻**仍在 `Working`**、或在 C 步被叫停过的会话，记 `true`。自己干完停下的会话不记——它本来就在等用户。

### 3. `dct restart`：排空 → 等 → 兜底 → 交接 → 重启

流程放在**后台里**跑（新请求 `Request::Drain { wait_ms, nudge_wait_ms }`），CLI 只负责发起、显示进度、Ctrl+C 取消（`Request::DrainCancel`）。新增请求要按协议规矩升 `PROTOCOL_VERSION`。

1. **进入「准备重启」**：后台广播一个状态，界面顶上显示一行「dct 准备重启」。自动来源不往会话里打：mesh 留言**先存着**（本来就有排队，复用），取消或重启后照常投递；手机和语音**当场回一句「dct 正在准备重启，稍后再发」**（它们没有队列，硬存会让用户以为发出去了）。用户自己在界面里打字不拦。
2. **接不回的空闲会话**（profile 没有 `resume_id_args` 的 agent 会话）：立刻打一句交接请求（见第 5 节）。在忙的接不回会话，等它停下再打。
3. **等**：每个会话停到 `Idle`（或 `Stopped` / `Failed`）就算好了。最多 `wait_ms`（默认 15 分钟）。
4. **兜底（C）**：到点还在 `Working` 的，打一句：
   > dct 马上要重启。请在安全的地方停下手上的事，然后用一句话写下你做到哪了、下一步是什么。

   再等最多 `nudge_wait_ms`（默认 3 分钟）。
5. **记状态**：写 `last-sessions.toml`，带上 `conversation_id` 和 `interrupted`。
6. **交给独立的小进程重启**（见第 4 节）。

**发起者自己的会话不等**：如果 `dct restart` 是在某个 dct 会话里敲的（环境里有 `DCT_SESSION_ID`，见 `session.rs:434`），CLI 把这个 id 带进 `Drain`。那个会话正在等命令跑完，永远不会变 `Idle`，所以不等它、不对它发 C 那句，直接记 `interrupted = true`，重启后它会被叫「继续」。

**取消**：第 5 步之前，CLI 断开连接或按 Ctrl+C 都算取消，后台回到正常、放出存着的投递；第 5 步开始后不能取消。

CLI 一直打印进度，例如「还在等 2 个会话（dc-octo、dc_llm），剩 9 分钟」。
`dct restart --now`：跳过 1–4，直接记状态并重启（等于现在的行为，但会带上 id）。
`--wait <分钟>` / `--nudge-wait <分钟>` 调等待时间。

### 4. 重启本身交给脱离会话的小进程

后台在第 5 步之后起一个用 `setsid` 脱离的 `dct __swap <旧后台 pid>`（Windows 走 `DETACHED_PROCESS`，与 `spawn_daemon` 同法），然后自己退出。小进程：等旧后台退出（最多 10 秒，超时硬杀）→ 用磁盘上**当前的** dct 二进制起新后台 → 等 `Hello` 版本对上 → 退出。新后台照常 `restore_last_sessions`，不问用户（headless 路径本来就不问）。

这样无论 `dct restart` 是在普通终端里敲的，还是某个会话里的 agent 敲的，重启都能完成。界面客户端掉线后按现有的重连逻辑重连（实施时核对现有重连行为）。

### 5. 接不回的会话：交接

- 交接文件：`~/.dct/handoffs/<会话 tag 或 id>-<日期时间>.md`，不往用户项目里丢文件。
- 重启前打的话：
  > dct 马上要重启，这个会话的对话接不回来。请把要接着做的事写进 `<路径>`：在做什么、做到哪了、下一步、要注意的坑。写完就停下。
- 重启后这个会话从空白开始，dct 等它 `Idle` 后打：
  > dct 刚重启。你上次的交接在 `<路径>`，先读它，再接着做。

  如果它重启前是空闲且没写出文件（文件不存在），不打这句。

### 6. 重启之后：只叫被打断的会话继续

`restore_last_sessions` 重开会话后，对 `interrupted = true` 且已按 id 接回的会话，等它第一次 `Idle`（界面准备好）再打：

> dct 重启完了。请先检查一下你做到哪了（看看文件和改动的状态），再接着做重启前没做完的事。

投递复用 mesh 那套「等 Idle 再打、一次一条」的逻辑。等 `Idle` 最多 2 分钟，超时就不打，并在界面上提示用户手动处理。打完把 `interrupted` 清掉。

## 不做的

- 不让每个会话在重启前写记忆——记忆应该在学到的当下写，重启那一刻没有新东西。
- 不做「保持 pty 跨后台重启存活」（把 fd 传给新后台）：彻底，但复杂，且 Windows 不同。以后需要再说。
- codex 不去 `~/.codex/sessions` 里按目录和时间猜对话 id：猜错比不接更糟。以后若 codex 能指定 id 再加。

## 出错处理

- 排空期间后台被别的原因杀掉：没写新状态，下次启动按旧清单恢复（今天的行为）。
- 小进程起新后台失败：打印一句人话到 `~/.dct/daemon.log`；下次用户敲 `dct` 会照常起后台、照常恢复。
- 某个会话在等待期间自己退出了：算「好了」，`Stopped` 的会话本来就不进清单。
- agent 无视 C 步那句话：到点照样重启，`interrupted = true`，重启后会被叫继续。

## 测试

- 自动：
  - `session_id_args` / `resume_id_args` 拼参数；旧清单（没有新字段）照读；同目录两个会话各自接回自己的 id。
  - 没有 `resume_id_args` 的 profile 退回 `--continue` 逻辑。
  - 排空状态机：全空闲立即通过；在忙→空闲通过；到点发 C 那句；C 后到点照样走；取消回到正常；自动来源的投递在排空期间被存住、取消后放出。
  - 只有 `interrupted` 的会话收到「继续」；接不回的会话收到交接请求和「先读交接」。
  - 用一个假的 agent（shell 脚本模拟忙/闲提示）做端到端：restart → 恢复 → 收到正确的话。
- 真机：
  - 先实测 qwen 的 `--session-id` / `--resume`，过了才给 qwen、dc 声明。
  - 用 3–4 个测试会话（同目录两个 claude、一个在忙、一个 codex）完整跑一遍 `dct restart`，核对对话确实接回、被打断的那个收到「继续」、codex 写了交接并在重启后读到。
  - 在会话里让 agent 自己敲 `dct restart`，核对没有卡在等它自己、新后台起来了、它被叫「继续」。
  - 都过了，再给用户的 18 个会话用。
