# dct 多语言 —— 设计

**状态：** 已确认，待出实施计划
**前置：** `docs/superpowers/plans/2026-08-03-dct-project-switch.md` 已完成（`p` 键换项目、底部栏一行化、F2 逆转键）

## 问题

dct 的界面文案全是硬编码的中文——全仓 153 条中文字面量，84 条在 `src/ui.rs`。
非中文用户开起来是一片看不懂的方块。

麻烦的是文案不止在界面进程里。用户按 `n` 建会话失败时，底部那行红字来自**守护进程**：

| 位置 | 文案 |
|---|---|
| `src/session.rs:97` | `没有这个 profile: {name}` |
| `src/session.rs:104` | `目录不存在: {dir}` |
| `src/session.rs:117` | `{dir} 不是 git 仓库，无法开 agent 会话` |
| `src/session.rs:149` | `没有这个会话: {id}` |
| `src/session.rs:257` | `还没有检查点` |
| `src/session.rs:251` | `这个会话没有检查点` / `这个会话没有改动记录` |
| `src/daemon.rs:69` | `请求解析失败: {e}` |
| `src/git.rs:22,37` | git 自己的 stderr，原样冒泡 |

守护进程是常驻的独立进程，界面切了语言它并不知道。所以「翻译界面」不是一个文件的事。

## 范围

**要做：** dct 自己的界面文案、守护进程返回给用户的错误、README、语言选择器里的语言名。

**不做：** agent 自己的输出（Claude Code 在 PTY 里画什么是它的事）、`profiles/*.toml` 里的
profile 名（`claude` / `shell` 是命令名，不是文案）、日志与 `anyhow` 内部上下文（进 stderr，
不上界面）、git 的 stderr（英文原文照抄，见「错误码」一节）。

**语言：** 英、中、西、日、韩、法、德，共七种。**第一阶段只实现英、中**，机制按七种设计。

**繁体中文不在这七种里。** `zh_TW` 会落到简体。假装支持比不支持更糟。

## 架构

```
i18n.rs      Lang 解析 + 词条 + 组句        纯函数，不认识界面也不认识守护进程
   ↑
settings.rs  语言设置的持久化               只有界面进程读写，守护进程不碰
   ↑
proto.rs     Response::Error(ErrorCode)     守护进程只报「哪类错 + 参数」，不组句
   ↑
ui.rs        l 键进设置视图；所有文案取自 i18n
```

关键是**组句只发生在界面进程**。守护进程不知道语言，也不需要知道——它报码，界面组句。
切语言立刻生效，不用重启守护进程。

### `src/i18n.rs`（新建）

```rust
#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Lang { En, Zh }        // 第一阶段只有两个变体

impl Lang {
    /// 语言自己的名字。误切到看不懂的语言时，用户还能认出自己那一行切回来。
    pub fn native_name(self) -> &'static str;   // "English" / "中文"
    /// 存进 settings.json 的稳定短码，不随枚举顺序变
    pub fn code(self) -> &'static str;          // "en" / "zh"
    pub fn from_code(s: &str) -> Option<Lang>;
    pub fn all() -> &'static [Lang];
}

/// 无参文案
pub enum Key { New, SwitchProject, Enter, Undo, Stop, Diff, Quit, /* … */ }
pub fn text(k: Key, lang: Lang) -> &'static str;

/// 带参文案，每条一个函数
pub mod msg {
    pub fn no_such_session(lang: Lang, id: u32) -> String;
    pub fn not_a_git_repo(lang: Lang, dir: &str) -> String;
    // …
}

/// 解析优先级：DCT_LANG > 传入的已存设置 > 系统 locale > En
pub fn resolve(saved: Option<Lang>, env: &dyn Fn(&str) -> Option<String>) -> Lang;
```

**为什么带参文案是函数而不是带 `{}` 的模板：** 模板要靠调用方按顺序填参，漏填、错序、
参数类型不对，编译器一概不管，而且各语言的语序本来就不同。写成函数，这些全归签名管。

**词条排布：** 同一条文案的各语言写在一起，不是每种语言各写一个大 `match`：

```rust
Key::New => t!(lang, en: "New", zh: "新建"),
```

改中文时英文就在眼前，不会出现「改了一半」。

**为什么是枚举而不是配置文件：** 第一阶段只做中英，另外五种以后补。加 `Lang::Ja` 那天，
编译器会把每一条没翻的都点名——这是编译期穷尽唯一值钱的地方，也正是本项目的路线。
代价是加语言要重编、不能让非程序员直接提译文，这两条都不是本项目的需求。

**`resolve` 的 `env` 参数是个闭包**，不是直接读 `std::env`。环境变量是进程全局状态，
测试里改它会互相打架（本仓测试统一 `--test-threads=1` 也不该依赖这个）。生产代码传
`|k| std::env::var(k).ok()`，测试传一张假表。

系统 locale 按 `LC_ALL` → `LC_MESSAGES` → `LANG` 的顺序取第一个非空值，只认主码：
`ja_JP.UTF-8` → `ja`，`zh_CN.UTF-8` → `zh`。认不出就是 `En`。

### `src/settings.rs`（新建）

```rust
/// 盘上存了什么。缺失、损坏、语言码不认识，一律 None——「盘上没有可用的选择」
/// 是一种情况，不是三种。决定最终用哪种语言的是 i18n::resolve，不是这里。
pub fn load_lang(path: &Path) -> Option<Lang>;
pub fn save_lang(path: &Path, lang: Lang) -> Result<()>;     // 原子写；失败要让调用方知道

pub fn settings_path_for_socket(socket: &Path) -> PathBuf;   // socket 同目录下的 settings.json
```

这个模块只管读写，不做判断。判断集中在 `i18n::resolve` 一处，免得「优先级」这条规则
散在两个文件里各写一半。

盘上格式 `{"lang":"zh"}`。位置跟着 socket 走，与 `projects.rs::store_path_for_socket`
同一套推导：生产是 `~/.dct/settings.json`，集成测试把 socket 放临时目录就自动隔离。

**和 `projects.rs` 的一处刻意不同：`save` 返回 `Result`。** 「最近项目」是缓存，丢了无所谓，
所以它的 `save()` 吞掉一切错误；语言是用户明确做出的选择，写不进去必须说一声，否则用户
下次开 dct 发现语言变回去了，不知道该怪谁。

### `proto.rs` 的错误码

```rust
pub enum ErrorCode {
    NoSuchProfile(String),
    DirNotFound(String),
    NotAGitRepo(String),
    NoSuchSession(u32),
    NoCheckpoint,
    NotAnAgentSession,      // 现在的「这个会话没有检查点 / 没有改动记录」是同一个成因
    BadRequest(String),     // 请求解析失败，带原始错误供排查
    Git(String),            // git 的 stderr 原文
}

pub enum Response { /* … */ Error(ErrorCode) }
```

`Git(String)` 是刻意留的兜底：git 的 stderr 是 git 自己按 `LANG` 输出的，dct 翻不动也
不该翻。界面显示成「操作失败：<原文>」——外面那半句是翻译过的，里面照抄。

`SessionManager` 的错误类型随之从 `anyhow::Error` 变成 `ErrorCode`（或一个能转成它的
错误枚举）。

`checkpoint_base(id, not_agent: &str)` 现在靠调用方传一句中文来区分场景
（`src/session.rs:247`）：回滚传「这个会话没有检查点」，看改动传「这个会话没有改动记录」。
成因其实是同一个——这不是 agent 会话。改成统一返回 `NotAnAgentSession`，措辞由界面定：
界面知道用户刚按的是 `u` 还是 `d`，比守护进程更有资格决定这句话怎么说。

### `ui.rs` 的改动

1. 所有中文字面量换成 `i18n::text(...)` / `i18n::msg::...`
2. 底部提示改成按宽度自适应（下一节）
3. 新增 `View::Settings`

## 排版

**七种语言的长度差得很远。** 底部那行现在是

```
n 新建  p 换项目  ↑↓ 选择  Enter 进入  u 回滚  s 停止  d 改动  q 退出
```

德语大致会是

```
n Neu  p Projekt wechseln  ↑↓ Auswählen  Enter Öffnen  u Zurücksetzen  s Stoppen  d Änderungen  q Beenden
```

80 列放不下。现在的代码从右边直接截，越靠后的键越先消失——`q 退出` 首当其冲，
而「怎么退出」恰恰是最不该丢的一条。

```rust
/// 量实际显示宽度，放不下就从优先级最低的项开始丢，丢过就在末尾放 …
fn fit_help(items: &[HelpItem], width: usize) -> String;
```

优先级写死在代码里，高到低：`n 新建`、`Enter 进入`、`p 换项目`、`q 退出`、`↑↓ 选择`、
`s 停止`、`d 改动`、`u 回滚`。**翻译的人不必为长度操心**——这是选自适应而不是「强制译文
写短」的理由：缩写（`Zurücks.` / `Änd.`）对零编程经验的用户不友好。

顺带修掉一个既有毛病：看板第一列用 `{:<20}` 按**字符数**补齐，而 `truncate` 按**显示宽度**
截断（`src/ui.rs:585`）。中文目录名 4 个字符、显示宽 8，右边那列就被推歪。补齐改成按显示
宽度。宽度规则沿用现有的手写近似（`> U+1100` 算双宽），不引入新依赖。

## 设置视图

看板按 `l` 进入 `View::Settings`。第一阶段里面就是语言列表：`↑↓` 选，`Enter` 生效并写盘，
`Esc` 取消。语言用它自己的语言写。

**不为一个设置项造两层导航。** 以后真加第二项设置时，它已经是个能长大的容器。

`Enter` 之后立刻重绘，整个界面换语言。写盘失败给一句红字（「设置没能保存」），但语言照样
当场生效——用户这次的操作不该因为磁盘问题白做。

`l` 只在看板视图生效。会话视图里所有按键都转发给 agent，抢走 `l` 会让用户在 agent 里
打不出这个字母——和 `p` 键同一条规矩。

## 数据流

```
启动    main.rs: settings::load_lang() → Option<Lang>
          → i18n::resolve(saved, env) 定最终语言 → 传进 ui::run

界面    每次 draw 用当前 Lang 组句；守护进程返回 ErrorCode 时也在这里组句

切换    l → View::Settings → Enter
          → 内存里的 Lang 换掉（立即生效）
          → settings.save()，失败给红字
```

守护进程完全不参与。它连 `Lang` 这个类型都不引用。

## 错误处理

| 情形 | 行为 |
|---|---|
| `settings.json` 不存在 | 按系统 locale 定，不报错。第一次用不该看见任何提示 |
| `settings.json` 损坏 | 同上。**不删文件**——用户可能手工编辑写错了，删掉他就没法看出错在哪 |
| `settings.json` 里是没见过的语言码（比如以后降级） | 同上 |
| 写盘失败 | 红字「设置没能保存」，语言仍当场生效 |
| `DCT_LANG` 值认不出 | 忽略，继续按后面的优先级走。不报错——它是调试开关 |
| git 的 stderr | 包在 `ErrorCode::Git` 里原样显示，外层那句话翻译 |

## 测试

| 测什么 | 怎么测 |
|---|---|
| 词条完整性 | 编译期保证，不写运行时测试 |
| `resolve` 优先级 | 假 env 表：`DCT_LANG` 压过已存设置；已存设置压过 `LANG`；`LC_ALL` 压过 `LANG`；认不出的值回退 `En`；全空回退 `En` |
| locale 主码解析 | `zh_CN.UTF-8` → Zh，`en_US` → En，`ja_JP.UTF-8` → En（第一阶段没有 Ja），空串 → En |
| 设置往返 | `save_lang(Zh)` → `load_lang` 拿回 `Some(Zh)`；文件缺失 / 内容损坏 / 语言码不认识 → 三种都是 `None`；写不进去（父目录不可写）要返回 `Err` |
| `fit_help` | 宽度足够时全显示；不够时按优先级丢且末尾有 `…`；极窄时只剩最高优先级；宽度为 0 不 panic |
| 组句 | 每个 `ErrorCode` 在 En / Zh 下都能组出非空、且不含 `{}` 残留的句子 |
| 渲染 | `View::Settings` 的 draw 不 panic；中英各画一次底部栏，断言各自的关键词出现 |

渲染测试每次新建 `TestBackend`——ratatui 画宽字符时只写首格、第二格留旧值，复用同一个
backend 会把上一帧的残字拼进断言（`src/ui.rs` 既有测试已记录这个坑）。

## README

`README.md` 保持中文，新增 `README.en.md`，两边开头互相链接。以后加语言再加文件。
不做单文件双语并排——中英交替的 README 两种读者都难读。

## 分两期

**第一期（本次计划）：** `i18n.rs` + `settings.rs` + `ErrorCode` + `View::Settings` +
`fit_help` + 列宽修复 + 中英全量词条 + `README.en.md`。

**第二期（另一份计划）：** 加 `Es / Ja / Ko / Fr / De` 五个 `Lang` 变体。编译器会逐条点名
要补的译文，那份计划的主体就是译文本身。

拆开的理由：第一期是机制，第二期是内容。机制没跑通之前翻七种语言，返工的是七倍的量。

## 被否掉的方案

**守护进程也读语言设置，自己组句。** 改动更小，但守护进程先起后不重读——界面切了语言它
还用旧的，除非再加一条通知机制。为省一次协议改动引入一个长期存在的不一致，不划算。

**只翻界面、守护进程的错误留中文。** 工作量最小。但选了日语的用户，在建会话失败时看到的
是一句中文——而那正是最需要看懂的时刻。

**用 fluent / gettext 这类成熟 i18n crate。** 复数与语序规则完备，但本仓至今一个非必要
依赖都没加，而 dct 的文案里几乎没有复数变位需求（「已开会话 3」这类是编号不是计数）。

**译文强制写短，一行固定。** 代码最简单，但 `Zurücks.` / `Änd.` 这种缩写对零编程经验的
用户不友好，而这正是目标用户。

**在界面里做繁体/简体转换。** 转换质量取决于词表，做不好比不做更冒犯。繁体要做就当成
第八种语言，独立翻译。
