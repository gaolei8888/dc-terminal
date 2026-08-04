# dct 多 agent —— 设计

**状态：** 已确认，待出实施计划
**前置：** `docs/superpowers/plans/2026-08-03-dct-escape-hatch.md` 已完成（Ctrl+Q 全局退一层、逃生提示不被顶掉）
**相关：** `docs/superpowers/specs/2026-08-03-dct-i18n-design.md`（已确认未实施，本设计与它的接口见「与 i18n 的关系」）

## 问题

dct 现在只认两个 profile：`claude` 和 `shell`，都写死在 `src/profile.rs` 的
`builtin()` 里。用户要用 codex、opencode、Kimi、GLM、DeepSeek、Qwen，没有任何入口。

而且这批 agent 不是同一类东西：

- **codex / opencode / qwen（Qwen Code）** 是独立 CLI，各有自己的二进制和 TUI
- **Kimi / GLM / DeepSeek / Qwen（API 形态）** 没有自己的 CLI。通行做法是跑 `claude`，
  把 `ANTHROPIC_BASE_URL` 和 `ANTHROPIC_AUTH_TOKEN` 换成它们的 Anthropic 兼容端点

第二类现在**根本无法表达**——`Profile` 只有 `command`，没有环境变量。

顺带暴露的两个现存缺陷：

1. **没有 `idle_pattern` 的会话在看板上永远显示「干活中」。** `shell` 就是这样：
   `src/session.rs:135` 初始化成 `Working`，`tick()` 里 `if let Some(re) = &s.idle_re`
   拿不到正则就整个跳过，状态再也不变。这是明确的假信息。
2. **选完 agent 要再选一次。** `src/ui.rs` 的 `PickProfile` 分支建完会话弹回看板，
   用户得在看板上找到自己刚建的那个再按 Enter。（已在本轮修掉，见「已完成」。）

## 目标用户

非程序员（见 `dc_classroom/CLAUDE.md` 的 Target Users）。这决定了几件事：

- 九个 agent 名字摆在他面前，**没有说明等于没得选**
- 「未安装」「密钥无效」这类话必须给得出下一步，不能只报状态
- 让他每次开会话都在九个里挑一个，是设计失败

## 范围

**要做：** profile 数据结构扩展、磁盘自定义 profile、密钥仓与密钥界面、可用性判定、
`busy_pattern` 状态判定、选择器改造、`n`/`N` 分工。

**不做：** 每个 agent 的模型选择（`/model` 是 agent 自己的事）、多 agent 并排对比、
agent 自己的登录流程（codex / qwen 有各自的 `login`，dct 不插手）。

## 已完成

「选中 agent 后直接进会话」已经改掉（`src/ui.rs` 的 `PickProfile` 数字键分支）：
`Create` 成功直接 `view = View::Attached(id)` 并置 `need_sessions = true`（会话标题要
显示项目名），失败才回看板——那儿才有报错要看。

## 架构

选定的方案是**「Profile 自描述 + 密钥仓单独存」**：

```
profiles/*.toml    「怎么起这个 agent」   命令、静态环境变量、声明需要哪种密钥
   +
~/.dct/secrets.toml 「用户的私货是什么」   0600，按 profile 名索引
   ↓
daemon 组装         command + env（静态 ∪ 密钥）→ PtySession::spawn
```

两件事正交，各自能单独测。profile 文件里**一个字节的密钥都没有**，可以随便拷贝分享。

考虑过但否掉的两个替代：

- **给 Profile 加 `kind = "cli" | "anthropic_compatible"`**：后者只写 base_url，
  command 固定成 claude。省下四条重复，但这个抽象只对 Anthropic 兼容端点成立，
  接 OpenAI 兼容的就得再加一种 kind。过早抽象。
- **profile 里用 `{{secret}}` 模板变量**：通用，但模板语法对目标用户是纯噪音，
  一个 `secret` 字段就够了。

方案的代价说清楚：kimi / glm / deepseek / qwen-api 四个文件的 `command` 字面重复四遍。
这是**可读的重复**——每个文件独立可懂，用户照着改一份就能加自己的第五个。

## 数据结构

### `Profile`（`src/profile.rs`，扩展）

```rust
pub struct Profile {
    pub name: String,
    pub command: Vec<String>,
    pub is_agent: bool,
    pub idle_pattern: Option<String>,
    pub busy_pattern: Option<String>,      // 新
    pub env: BTreeMap<String, String>,     // 新，静态非机密
    pub secret: Option<SecretSpec>,        // 新
    pub install: Option<InstallSpec>,      // 新
    pub label: LocalizedText,              // 新，给用户看的名字
    pub note: LocalizedText,               // 新，一行说明
}

pub struct SecretSpec {
    pub env: String,           // 密钥注到哪个环境变量
    pub hint: LocalizedText,   // 密钥界面上的一句人话
    pub url: Option<String>,   // 申领页面，Ctrl+O 打开
    pub verify: Option<VerifySpec>,
}

pub struct VerifySpec {
    pub url: String,           // 探测端点，通常是 base_url + /v1/messages
}

pub struct InstallSpec {
    pub command: Vec<String>,        // 例：["npm", "i", "-g", "@openai/codex"]
    pub note: LocalizedText,         // 例：「需要先装 Node.js」
}
```

`command[0]` 是可用性判定的依据，也是「依赖谁」的依据——见「可用性判定」。

### `LocalizedText`

```rust
pub struct LocalizedText {
    pub zh: Option<String>,
    pub en: Option<String>,
}
```

TOML 里写成子表。取不到当前语言时的回落：`label` 回落到 profile 的 `name`，
`note` 与 `hint` 回落到空串（宁可不显示，不要显示一个命令名当说明）。

```toml
[label]
zh = "Kimi"
en = "Kimi"
```

**为什么现在就做成多语言结构**：i18n 设计（已确认待实施）把界面文案收进词条表，但
profile 是**用户可编辑的数据文件**，进不了那张表——用户自己加的 profile 也得能有说明。
现在写成平字符串，i18n 落地时就是一次会打破用户文件的改动。结构一次到位，代价只是
多两层嵌套。当前实现只读 `zh`，`Lang` 存在之后按语言取。

### 完整的 profile 示例

```toml
# profiles/kimi.toml
name = "kimi"
command = ["claude", "--dangerously-skip-permissions"]
is_agent = true
idle_pattern = "\\? for shortcuts"

[label]
zh = "Kimi"

[note]
zh = "月之暗面，套用 Claude Code 界面"

[env]
ANTHROPIC_BASE_URL = "https://api.moonshot.cn/anthropic"

[secret]
env = "ANTHROPIC_AUTH_TOKEN"
url = "https://platform.moonshot.cn/console/api-keys"

[secret.hint]
zh = "在 platform.moonshot.cn 开通后复制 API Key"

[secret.verify]
url = "https://api.moonshot.cn/anthropic/v1/messages"
```

```toml
# profiles/codex.toml
name = "codex"
command = ["codex", "--dangerously-bypass-approvals-and-sandbox"]
is_agent = true
busy_pattern = "esc to interrupt"

[label]
zh = "Codex"

[note]
zh = "OpenAI 官方"

[install]
command = ["npm", "i", "-g", "@openai/codex"]

[install.note]
zh = "需要先装 Node.js"
```

### 内置清单

顺序即菜单顺序。

| name | 形态 | command | 密钥 |
|---|---|---|---|
| `claude` | 独立 CLI | `claude --dangerously-skip-permissions` | 自己登录 |
| `codex` | 独立 CLI | `codex --dangerously-bypass-approvals-and-sandbox` | 自己登录 |
| `opencode` | 独立 CLI | `opencode` | 自己登录 |
| `qwen` | 独立 CLI | `qwen` | 自己登录 |
| `kimi` | claude + base_url | `claude --dangerously-skip-permissions` | 需要 |
| `glm` | claude + base_url | 同上 | 需要 |
| `deepseek` | claude + base_url | 同上 | 需要 |
| `qwen-api` | claude + base_url | 同上 | 需要 |
| `shell` | 非 agent | `/bin/zsh` | — |

### ⚠️ 未实测项

实施时必须逐条实跑验证，不能照抄本文档：

| 项 | 状态 |
|---|---|
| `codex` 的命令与 `busy_pattern` | **已实测**（v0.146.0，PTY 抓屏确认 `esc to interrupt`） |
| `claude` 的 `idle_pattern` | **已实测**——`claude` 本身已安装，在开发机上日常使用中 |
| `opencode` 的命令、安装包名、pattern | **未实测**，本机没装，也没找到能装的机器；pattern 依旧刻意留空，跟文档最初的决定一致 |
| `qwen` 的命令、安装包名、pattern | **未实测**，本机没装，也没找到能装的机器；pattern 依旧刻意留空，跟文档最初的决定一致 |
| kimi / glm / deepseek / qwen-api 的 base_url 与 verify url | **仍未实测**——照公开文档写，没拿真实密钥探测过；这四项是发布前最后一个阻塞项，需要各家的真实 API key 才能验 |

未实测的 pattern 一律留空，状态显示 `—`（见「状态判定」），不瞎猜。

**codex 的首次目录信任提示**：codex 头一次在某个目录跑会问「Do you trust the contents of
this directory?」，`--dangerously-bypass-approvals-and-sandbox` 不跳过它。这是 codex 自己的
行为，dct 不绕过——用户按一次 `1` 即可，codex 会记住。

### 密钥仓（`~/.dct/secrets.toml`）

```toml
[secrets]
kimi = "sk-..."
glm = "..."
```

- 权限 **0600，在创建时就带上**（`OpenOptions::mode(0o600)`），不是先建再 chmod——
  那中间有一个可读窗口
- 写盘走「临时文件 → `rename`」，中途断电不会留半个文件
- daemon 里一个 `SecretStore`，和现有的 projects `Store` 并列

## 可用性判定

在 **daemon 侧**做，不在 UI 侧。daemon 查 PATH 和它 `spawn` 进程用的是同一个环境，
「显示可用」和「真能起来」才不会打架。

```rust
pub enum ProfileStatus {
    Ready,
    NeedsSecret,                       // 声明了 secret 但仓里没有
    NeedsDependency { name: String },  // command[0] 是另一个 profile 的命令，而它没装
    NotInstalled,                      // command[0] 在 PATH 上找不到
}
```

判定顺序（**顺序很重要**）：

1. `command[0]` 在 PATH 上找不到 → 如果这个命令正好是另一个内置 profile 的
   `command[0]`（kimi 系全都是 `claude`），报 `NeedsDependency { "claude" }`，
   否则报 `NotInstalled`
2. 声明了 `secret` 但密钥仓里没有 → `NeedsSecret`
3. 否则 `Ready`

**为什么依赖检查必须排在密钥检查前面**：kimi/glm/deepseek/qwen-api 跑的都是 `claude`。
claude 没装时如果先报「未填密钥」，用户会去填 key，填完还是起不来，然后以为是 key 的
问题——把人送进死胡同。

## 状态判定

`src/session.rs` 的 `tick()`：

```
有 busy_pattern → 匹配上 = Working，没匹配上 = Idle
否则有 idle_pattern → 匹配上 = Idle，没匹配上 = Working
两者皆无 → Unknown，状态不再变
```

`busy_pattern` 优先，因为它更可靠：agent 干活时的「按 esc 中断」提示是稳定的，而
空闲时的输入框占位符会在用户一打字就消失。

新增 `SessionState::Unknown`，`status_label` 给 `—`，颜色 `DarkGray`。会话创建时的初始
状态改成「有任一 pattern 就 `Working`，都没有就 `Unknown`」。

这同时修掉 `shell` 永远显示「干活中」的现存缺陷，也让还没实测出 pattern 的 agent
诚实地显示「不知道」，而不是编一个状态。

## 协议（`src/proto.rs`）

```rust
// 改
Response::Profiles {                    // 原来是 Response::Profiles(Vec<String>)
    entries: Vec<ProfileEntry>,
    warning: Option<String>,            // 密钥文件读不了之类，UI 顶部红字
}

pub struct ProfileEntry {
    pub name: String,
    pub label: String,                  // 已按当前语言取好
    pub note: String,
    pub status: ProfileStatus,
    pub secret: Option<SecretPrompt>,   // NeedsSecret 时 UI 要用
    pub install: Option<InstallPrompt>, // NotInstalled 时 UI 要用
}

// 协议层只带**已经取好语言**的字符串，不把 LocalizedText 送过线——
// 组句发生在哪一侧要一致（见「与 i18n 的关系」）。
pub struct SecretPrompt {
    pub hint: String,
    pub url: Option<String>,
}

pub struct InstallPrompt {
    pub command: Vec<String>,
    pub note: String,
}

// 新
Request::SetSecret { profile: String, value: String }
Request::DeleteSecret { profile: String }
Request::VerifySecret { profile: String, value: String }
Request::LastProfile                    // n 键要用
Response::SecretOk
Response::LastProfile(Option<String>)
```

上次用过的 agent 记在 daemon 侧（`Create` 成功时写），和 projects `Store` 同一个位置。

一个例外：**「装 CLI」开的那个 shell 会话不记账**。它是为了装东西，不是用户选的 agent；
记了的话下次按 `n` 会直接掉进一个命令行。`Request::Create` 加一个 `remember: bool`，
安装路径传 `false`。

## 数据流

**开会话（日常路径）**

```
看板按 n → Request::LastProfile → 拿到 "kimi"
        → Request::Create { dir, profile: "kimi" }
        → daemon: resolve_profile → 查密钥仓 → env = 静态 ∪ {ANTHROPIC_AUTH_TOKEN: key}
        → PtySession::spawn(command, env, dir)
        → View::Attached(id)
```

一个按键到底。没有记录、或那个 profile 现在不可用，退回选择器。

**首次配密钥**

```
选择器选中「Kimi（未填密钥）」
  → View::EnterSecret
  → 用户粘贴 → Enter
  → 后台线程：Request::VerifySecret → 界面显示「正在验证…」
  → 通过：Request::SetSecret → Request::Create → View::Attached
  → 不通过：留在 EnterSecret，红字说原因
```

**装没装的 CLI**

```
选择器选中「OpenCode（未安装）」→ Enter
  → 用 shell profile 建一个会话
  → 把 install.command 拼成一行、带换行发进去，自动开跑
  → View::Attached，用户全程看得见安装过程
```

dct 本来就是跑终端会话的工具，装 CLI 复用这套机制，不需要新的执行路径。

装完之后 dct **不自动重试**：安装可能失败、可能要 sudo、可能要先装 Node。用户看完
输出自己 Ctrl+Q 回看板、按 `N` 重选即可——那时可用性会重新算，装成功的就不灰了。
自动重试会在装失败时又开一个起不来的会话，比让用户自己看一眼更糟。

## UI

### 一、agent 选择器（`View::PickProfile` 改造）

```rust
View::PickProfile { entries: Vec<ProfileEntry>, state: ListState }
```

```
1. Claude            Anthropic 官方
2. Codex             OpenAI 官方
3. OpenCode          开源，多模型            (未安装)
4. Qwen              阿里通义，独立 CLI      (未安装)
5. Kimi              月之暗面，Claude 界面   (未填密钥)
6. GLM               智谱，Claude 界面       (未填密钥)
7. DeepSeek          深度求索，Claude 界面   (未填密钥)
8. Qwen API          阿里通义，Claude 界面   (未填密钥)
9. 命令行            普通终端，不带 AI
```

（这一屏是「claude 已装、其余四个 API 形态都还没填密钥」的样子。claude 没装时
5-8 会统一变成 `(需要先装 Claude)`——它们跑的都是 claude。）

按键：↑↓ 移动，Enter 确认，`1`-`9` 秒选，Esc 取消。数字保留是因为快，↑↓ 是因为
自定义 profile 会让条目超过 9 个。**置灰项也占编号**——编号跳号比编号漂移更难受。

Enter / 数字落在哪一类，走哪条分支：

| 状态 | 行为 |
|---|---|
| `Ready` | 建会话 + 直接进 |
| `NeedsSecret` | 切到 `View::EnterSecret` |
| `NotInstalled` 且有 `install` | 开 shell 会话跑安装命令 + 直接进 |
| `NotInstalled` 无 `install` | 底栏说「本机没有找到 <命令>」，不切视图 |
| `NeedsDependency` | 底栏说「要先装 Claude 才能用 <label>」，不切视图 |

置灰项渲染成 `DarkGray`，原因写在行尾括号里。

### 二、填密钥（`View::EnterSecret`，新）

```rust
View::EnterSecret {
    profile: String,
    label: String,
    prompt: SecretPrompt,      // hint + url，协议里带过来的
    buf: String,
    phase: SecretPhase,        // Typing | Verifying | Failed(String)
}
```

- 输入显示成圆点，不显示明文
- 粘贴走已有的 `Event::Paste` 分支，加一个 arm。**自动清洗**：`trim`、去掉两边的
  单双引号、去掉 `Bearer ` 前缀——用户从网页复制经常带这些
- Backspace 删一个，Ctrl+U 清空
- **`Ctrl+O` 打开申领页面**（`open <url>`）。用 `Ctrl+O` 不用 `o`，因为 `o` 得留给输入
- Enter → 验证 → 存盘 → 建会话 → 进去
- Esc / Ctrl+Q 回选择器

⚠️ **验证必须在后台线程做。** 会话视图是 16ms 一刷，在按键循环里直接发网络请求会把
整个界面冻住。做法：起一个线程 + 另开一条 daemon 连接，`mpsc` 回传结果，主循环每轮
`try_recv`。`phase` 从 `Typing` 变 `Verifying` 时界面显示「正在验证…」。

### 三、密钥设置页（`View::Secrets`，新，二期）

看板按 `c` 进。列出所有声明了 `secret` 的 profile 和「已配 / 未配」，Enter 改（进
`EnterSecret`），`d` 删。这一页解决的是「换 key / 删 key」；首次配置走选择器那条路，
不需要用户先知道有这一页。

### 四、`n` / `N`

- **`n`**：用上次那个 agent 直接开会话并进去。没有记录、或那个 profile 现在不是
  `Ready`，退回选择器
- **`N`**：进选择器

理由：目标用户是非程序员，让他每次在九个 agent 里挑一个是设计失败——他不知道区别。
日常路径从「n → 看菜单 → 认字 → 按数字 → 在看板找会话 → Enter」压成一个 `n`。

底栏提示相应改成 `n 新建  N 换 agent  p 换项目  ...`。

### `back_one_level`

`EnterSecret` → `PickProfile`（不是直接回看板，用户可能只是选错了 agent）。
`Secrets` → `Board`。

## 错误处理

**`secrets.toml` 读不出来**（手改坏了、权限不对）：**不当成空**。当成空的话用户会以为
密钥丢了，接着一次写入就把本来还能手工救回的文件彻底覆盖。做法是记住这个错误，
选择器顶部红字「密钥文件读不了：<原因>」，并且**拒绝任何写入**，直到文件恢复。

**密钥验证的三种结果**分开说：

| 结果 | 文案 |
|---|---|
| 401 / 403 | 「这个密钥用不了，可能是复制的时候少了一段」 |
| 连不上 | 「连不上服务器，检查一下网络」 |
| 其他任何响应 | **放行** |

最后一条是刻意的：各家 Anthropic 兼容端点行为不一，不能因为返回码奇怪就把用户拦在
门外。验证的职责是抓住「key 明显是错的」，不是当网关。

**没有 `verify` 的 profile** 直接存盘，不验证。

**`spawn` 失败**（命令在 PATH 上但起不来）：把系统错误转成人话，别把 `ENOENT` 之类
甩给用户。

## 测试

纯函数部分好测，重点放在这些：

- **profile 解析**：`env` / `secret` / `secret.verify` / `install` / `busy_pattern` /
  `label` / `note` 各有用例；缺省字段能回落
- **内置清单**：每一个都能解析、`command` 非空、`name` 与文件名一致
- **可用性判定**：注入一个假的「命令是否存在」查询，覆盖 `Ready` / `NeedsSecret` /
  `NotInstalled` / `NeedsDependency` 四种，以及「claude 缺失时 kimi 报依赖不报密钥」
  这条顺序断言
- **密钥仓**：落盘权限是 0600、原子替换、写完能读回、读坏文件时拒绝写入
- **状态判定**：busy 优先于 idle；只有 idle 时行为不变（回归）；两者皆无得到
  `Unknown`；`shell` 不再是 `Working`
- **粘贴清洗**：带引号、带 `Bearer `、带尾随换行的三种输入
- **选择器按键**：数字、↑↓、Enter，以及落在四种状态上分别走哪条分支
- **`back_one_level`** 覆盖两个新视图

**密钥验证抽成接受「发请求」闭包的函数**，测试注入假响应（401 / 网络错 / 200 /
奇怪返回码），不打真网络。

## 与 i18n 的关系

i18n 设计（`2026-08-03-dct-i18n-design.md`）已确认未实施。本设计与它的接口：

- `LocalizedText` 现在就按多语言结构落地，当前只读 `zh`。i18n 落地后按 `Lang` 取，
  用户的 profile 文件不用改
- 本设计新增的**界面文案**（「未安装」「正在验证…」等）先按现状硬编码中文，
  跟着 i18n 那一期一起收进词条表
- 本设计新增的 **daemon 侧错误**同理，i18n 那期会把它们转成 `ErrorCode`

## 分期

**一期**：profile 数据结构 + 磁盘自定义 profile + 内置九条 + 密钥仓 + 可用性判定 +
状态判定 + 选择器改造 + 密钥输入与验证 + `n`/`N`。

**二期**：密钥设置页（`c` 键）。

一期自足可用：首次配密钥有路径，换 key 可以先删 `~/.dct/secrets.toml` 里那一行。
二期把这条路径补成界面操作。

## 新增依赖

`ureq`（阻塞式 HTTP，rustls）——只用于密钥验证。

考虑过调系统 `curl` 换零依赖，否掉了：验证逻辑要能单测，shell 出去的东西注入假响应
很别扭，而且 `curl` 的存在性又是一个新的可用性判定。
