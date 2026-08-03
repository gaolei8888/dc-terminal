# dct 看板内切换项目 —— 设计

**状态：** 已确认，待出实施计划
**前置：** `docs/superpowers/plans/2026-08-01-dct-core.md` 已完成（守护进程、会话、检查点、TUI 看板）

## 问题

dct 启动时把当前工作目录记为 `default_dir`（`src/main.rs:72`），此后新建会话永远落在这个目录里
（`src/ui.rs:216`）。运行期间无法改变。换一个项目只能退出 dct、`cd`、重开。

看板本身已经是跨项目的——每个会话记着自己的目录，看板有一列显示它（`src/ui.rs:475`），
所以多个项目的会话可以同时挂在一块看板上。缺的只是**新建**会话时选目录的能力。

## 范围

「切换项目」只改变**下一个新会话开在哪**。看板照旧列出全部会话，不过滤、不分组。

明确不做：目录浏览器、模糊匹配打分、项目置顶、项目改名、扫描工作区根目录自动发现。
理由分别记在末尾的「被否掉的方案」。

## 架构

三层，各自独立可测：

```
projects.rs   最近项目的持久化           纯数据 + 文件 IO，不认识会话也不认识界面
   ↑
daemon.rs     Create 成功后 touch 一笔    通过 Request::Projects 把列表交给界面
   ↑
ui.rs         p 键弹出选择器             只管交互，选中结果存成界面自己的 current_dir
```

### `src/projects.rs`（新建）

```rust
pub struct Store { /* 私有 */ }

impl Store {
    pub fn load(path: &Path) -> Store;          // 缺失或损坏一律返回空 Store，不报错
    pub fn touch(&mut self, dir: &Path);        // 去重 + 提到最前 + 截断 + 落盘
    pub fn list(&self) -> Vec<String>;          // 最近使用的在最前
}

pub fn store_path() -> PathBuf;                 // $HOME/.dct/projects.json
```

磁盘格式：

```json
{"recent": ["/Users/lei/work/dc/dc-terminal", "/Users/lei/work/dc/dc_workbench"]}
```

- 上限 **20** 条，超出丢弃末尾
- 只存**绝对路径**，`touch` 时用 `canonicalize` 归一，归一失败就存原样（目录可能刚被删）
- 落盘是**原子的**：写同目录下的临时文件再 `rename`。半截 JSON 会让下次启动丢掉整个列表
- `load` 遇到文件不存在、JSON 语法错、字段类型不对，**一律当空列表**。
  这是一份便利性缓存，不值得为它让守护进程起不来

### `src/daemon.rs`（改动）

`run_with_manager` 里构造 `Arc<Mutex<projects::Store>>`，与 `mgr` 一同传给 `serve` / `handle`。
`handle` 签名变成 `handle(req, &Arc<SessionManager>, &Arc<Mutex<Store>>)`。

- `Request::Create` 成功后 `store.touch(&dir)`。**失败不记**——目录不存在或不是 git
  仓库的路径进了「最近项目」，下次还会被选中，还会失败
- `Request::Projects` → `Response::Projects(store.list())`

锁用 `session.rs` 里已有的 `recover()` 处理 poison，跟仓库现有做法一致。持锁期间只做内存
操作和一次小文件写，不牵扯 git 子进程，不存在 Task 5 处理过的那种持锁跑慢操作的问题。

**为什么不放进 `SessionManager`：** 「最近项目」是界面关切，不属于会话生命周期。
`session.rs` 已经 473 行，塞进去只会让它继续膨胀，而且 `SessionManager` 的测试将被迫处理文件 IO。

### `src/proto.rs`（改动）

```rust
Request::Projects                       // 新增
Response::Projects(Vec<String>)         // 新增
```

### `src/ui.rs`（改动）

`View` 新增一个变体：

```rust
PickProject {
    all: Vec<String>,       // 守护进程返回的完整列表
    filter: String,         // 用户打的字
    state: ListState,
    typing_path: Option<String>,   // Some 表示正处在「手输路径」的输入态
}
```

`run()` 里 `default_dir: PathBuf` 改成可变的 `current_dir`，`Request::Create` 用它。

**交互**

- 看板底部多一行：`当前项目：~/work/dc/dc-terminal`（家目录缩写成 `~`，复用已有的 `short_path`）
- 底部提示加 `p 换项目`
- `p` **只在看板视图生效**。会话视图（`View::Attached`）里所有按键都转发给 agent，不能被截走
- `p` → 发 `Request::Projects` → 进入 `PickProject`。请求失败时不进选择器，红字提示，与现有 `n` 键的处理一致
- 列表 = 过滤后的项目 + 末行「手输路径…」。**末行不参与过滤，永远在**
- 每行渲染成 `<目录名>  <缩写路径>`，例如 `dc-terminal   ~/work/dc/dc-terminal`
- `↑` `↓` 选择，`Enter` 确认，`Esc` 取消。**不用数字键**——列表最多 20 条，数字键不够
- 直接打可见字符即过滤：**不区分大小写的子串匹配**，匹配的是完整路径而不只是目录名
  （这样 `work` 和 `dc-term` 都管用）。`Backspace` 删一个字。过滤后光标回到第一项
- 选中「手输路径…」→ 进入 `typing_path`，原地变成一行输入框。**这个状态下可见字符全部进输入框，
  不再当过滤用**。`Enter` 确认，`Esc` 退回列表（过滤词保留）。
  粘贴（已有的加括号粘贴）在这里直接可用，这也是不做目录浏览器的底气
- 确认时：`~` 展开为 `$HOME`，相对路径按 dct 启动目录解析

**校验** 切换时只查一次 `is_dir()`，不是目录就红字提示且**不切换**。
是不是 git 仓库**不在这里判**——那条规则留在 `SessionManager::create()`（`src/session.rs:105`），
两处各判一次迟早漂移。所以切到一个非 git 目录是允许的，直到你按 `n` 建 agent 会话才会被拒，
错误话术沿用现有的「不是 git 仓库，无法开 agent 会话」。

**冷启动** `projects.json` 不存在时列表为空，界面补上启动目录一条，`current_dir` 就是它。
第一次用不会看到空列表。

**`current_dir` 不持久化。** 每次启动 dct，当前项目一律回到启动目录，哪怕 `projects.json`
里另有更近的一条。持久化的是**列表**，不是**选择**——`cd` 到某个项目再开 dct 是最直白的
表达意图的方式，不该被上次的选择推翻。

## 数据流

```
用户按 n
  └─ Request::Create { dir: current_dir, profile }
       └─ SessionManager::create()  成功
            └─ store.touch(dir) → 写 ~/.dct/projects.json

用户按 p
  └─ Request::Projects
       └─ store.list() → Response::Projects
            └─ View::PickProject
                 └─ Enter → current_dir = 选中项
```

注意箭头方向：项目列表是**建会话的副产物**，不需要单独的「添加项目」动作。手输一次路径、
建一次会话，这个项目就永久进列表了。

## 错误处理

| 情形 | 行为 |
|---|---|
| `projects.json` 不存在 / 损坏 | 当空列表，界面补启动目录一条 |
| 落盘失败（磁盘满、权限） | 忽略，内存列表照常用。丢的是便利性，不是数据 |
| `Request::Projects` 请求失败 | 不进选择器，红字提示 |
| 选中的目录已不存在 | `is_dir()` 拦下，红字提示，不切换。列表里那条**不删**——可能只是外置盘没挂 |
| 切到非 git 目录后建 agent 会话 | 由 `create()` 拒绝，沿用现有中文错误 |
| 过滤后无匹配 | 列表只剩「手输路径…」一行 |

## 测试

**`src/projects.rs` 单测**
- `touch` 去重并把已有项提到最前
- 超过 20 条时截断，最旧的被丢
- 损坏 JSON（`{`）退化成空列表且不 panic
- `touch` 后重新 `load` 能读回（覆盖原子写这条路径）

**`src/ui.rs` 单测**
- `expand_path`：`~/x` 展开、相对路径按基准目录解析、绝对路径原样
- `filter_projects`：不区分大小写、子串命中路径中段、无匹配返回空

**`tests/projects_flow.rs` 集成测试**
- 起守护进程 → 在两个不同的临时 git 仓库各 `Create` 一次 → `Request::Projects`
  返回顺序必须是「后建的在前」
- `Create` 失败（目录不存在）后 `Request::Projects` **不含**那个路径

**看板渲染 smoke test** 补一条：底部那行显示当前项目。

## 被否掉的方案

**目录浏览器** —— 与加括号粘贴重复。`ff1e37d` 已支持粘贴，在「手输路径…」里粘一次路径就到位。
一套目录导航要处理上级目录、隐藏目录、权限错误、长列表滚动，换来的只是「不用粘贴」。

**模糊匹配打分（fzf 式）** —— 那是给几千条候选准备的。这里上限 20 条，子串过滤足够。

**项目置顶** —— 置顶解决「常用的沉下去了」，而按最近使用排序不会让常用的沉下去，问题不存在。

**项目改名** —— `2026-08-01-dc-terminal-design.md` 第 95 行写了手机端 `/new` 也要选目录，
将复用同一份列表。允许改名等于造一个只在一台机器的 TUI 里存在的别名，手机上对不上号。

**扫描工作区根目录** —— 得先配「根目录是哪个」才有用，而配根目录比手输一次项目路径更麻烦。

## 下一步

做完转去 `docs/superpowers/plans/2026-08-03-dct-phone-relay.md`。dct 存在的理由是
「开了任务之后人可以离开电脑」（`2026-08-01-dc-terminal-design.md` 第 8 行），
那条链路目前一行代码都没有。项目选择器再顺手也不推进它，所以做完就停，不再往上加东西。
