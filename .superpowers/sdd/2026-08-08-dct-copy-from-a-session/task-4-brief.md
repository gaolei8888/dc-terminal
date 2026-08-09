### Task 4: 两份 README 说实话

**Files:**
- Modify: `README.zh-CN.md`
- Modify: `README.md`

**Interfaces:**
- Consumes: 前三个 Task 的行为
- Produces: 无

⚠️ **工作树里有用户自己未提交的 README 改动**（`scripts/install.sh` 那一段）。**不要**把它们卷进你的提交——必要时 `git stash push -- README.md README.zh-CN.md`，改完再 pop，或者只 `git add -p` 你自己那几段。

- [ ] **Step 1: 重写中文那段**

`README.zh-CN.md` 现在有这么一段（在滚屏说明附近）：

> 往回滚屏能用了，但它是有代价的：进了会话之后 `dct` 会接管鼠标，终端自己那套拖动选中文字就在会话里失灵了。iTerm2 里按住 Option 能拿回来，别的终端一般也有对应的修饰键。`dct` 自己还没有复制功能。退回看板，鼠标就还给你了。

换成：

```markdown
`dct` 只在 **agent 自己要鼠标的时候**才接管它。Claude Code 会要（它自己用鼠标滚
它那一屏），codex 和普通命令行不要——那些会话里鼠标一直归终端，拖动选中文字、
复制，跟平时完全一样。代价是那些会话里滚轮不再翻 `dct` 的历史，用
`PageUp`/`PageDown`/`End`。

在 agent 要鼠标的会话里想复制，按 `F4` 进复制模式：鼠标临时还给终端，底栏会写着
现在是这个状态，复制完再按一次 `F4` 回去。也可以用终端自己的修饰键（iTerm2 是
按住 Option），不用退出会话。

`dct` 自己没有复制功能——复制用的是你终端本来那一套。
```

- [ ] **Step 2: 同步英文**

`README.md` 对应段落做等价改动。**两份是同一个文档的两种语言，不能漂移**——逐条对照，claim 对 claim。

- [ ] **Step 3: 核对文档里的每个键**

`F4` 对着 `src/ui/attach.rs::handle_key` 核一遍，`PageUp`/`PageDown`/`End` 对着 `key_scroll` 核一遍。**文档里写一个不存在的键，比漏写一个更糟。**

- [ ] **Step 4: 提交**

```bash
git add README.md README.zh-CN.md   # 只 add 你自己改的那几段，见上面的警告
git commit -m "docs: the mouse stays yours unless the agent asked for it"
```

---

## 自查

**Spec 覆盖：**

| Spec 小节 | Task |
|---|---|
| 一、一条规则（三条件相与） | 1 |
| 二、agent 订没订阅（复用 `agent_owns`，不改协议） | 1 |
| 三、`F4` 复制模式 | 2（状态）、3（底栏） |
| 四、要动的文件 | 1–4，且守护进程侧零改动 |
| 错误处理：`Screen` 拉不到不翻转 | 1 |
| 错误处理：复制模式下会话结束要复位 | 2 |
| 测试清单 | 1（真值表、断连）、2（`F4`、复位三路）、3（底栏优先级） |
| 破坏性变更：无 | —— |

**排期：** Task 1 → 2 → 3 是一条依赖链（字段 → 键 → 文案），Task 4 依赖前三个的最终行为。不能并行。

**留给执行者的两个坑：**

1. **`copy_mode` 在「进入」这一侧复位，不在「离开」那一侧。** 初稿写的是把三处「回看板」收成一个 `leave_session`，核代码之后否掉了：Ctrl+Q 那条走 `back_one_level`，是所有视图共用的纯函数，为一个 `bool` 改它的签名不值；另外两处各自还做着别的事。`enter_session` 是所有进会话路径的唯一漏斗，在那儿写一行就够，而且结构上漏不掉。
2. **别碰守护进程。** 如果发现自己想改 `proto.rs` / `pty.rs` / `session.rs` / `daemon.rs`，停下来——`agent_owns` 已经在传了，需要的事实全都在 `App.scroll` 里。
