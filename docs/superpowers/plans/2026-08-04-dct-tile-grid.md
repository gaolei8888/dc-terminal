# 看板九宫格（tile grid）实施计划

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 看板加一个九宫格视图：平铺显示所有会话的实时画面（只读），`Enter` 放大交互，`F3` 在会话间轮转。

**Architecture:** 协议加一对批量消息 `Screens`（一次取多个会话的样式化屏幕），daemon 侧 `SessionManager` 加批量读取，UI 侧新增 `View::Grid` 视图（纯布局数学独立成模块）。格子只读、永不 Resize；交互靠放大到现有附加视图。

**Tech Stack:** Rust、ratatui、crossterm、vt100、serde（newline-delimited JSON over unix socket）。

**Spec:** `docs/superpowers/specs/2026-08-04-dct-tile-grid-design.md` —— 动手前通读一遍。

## Global Constraints

- 先决条件：滚屏设计 0.2 节的 `ui.rs` 拆分**必须已完成**（`src/ui/board.rs` 存在）。没完成就停下，先去执行滚屏计划的拆分任务。
- 测试串行跑：`cargo test -- --test-threads=1`（测试开真进程、绑真 socket）。
- 每次提交前：`cargo fmt --check`、`cargo clippy --all-targets` 干净。
- 房规：按键分支里永远不要 `continue`（循环末尾清理陈旧 message，`e0ba1ec` 翻过车）。
- 房规：用户看得到的每一句话写给没编程过的人，错误提示必须说清下一步干什么。
- 房规：注释解释为什么，不解释是什么；密度对齐现有代码。
- 协议改动是纯增量（新增变体 + 新增 struct），按滚屏设计 0.1 节规则**不加协议版本号**。
- 九宫格**永远不发 `Request::Resize`**。
- 不用 emoji 当图标。

---

### Task 0: 先决条件闸门

**Files:** 只读检查，不改代码。

- [ ] **Step 1: 确认 `ui.rs` 已拆分**

```bash
ls src/ui/board.rs src/ui/attach.rs src/ui/widgets.rs src/ui/mod.rs
```

四个文件都在 → 继续。任何一个不在 → **停止本计划**，报告：「先决条件未满足：`ui.rs` 拆分（滚屏计划 0.2 节）还没做，先执行那个计划再回来。」

- [ ] **Step 2: 基线绿**

```bash
export PATH="$HOME/.cargo/bin:$PATH"
cargo test -- --test-threads=1
```

全部通过才继续。记下测试总数（后面任务只增不减）。

---

### Task 1: 协议加 `Screens` 批量消息

**Files:**
- Modify: `src/proto.rs`（`Request`、`Response`、手写 `Debug`、新 struct）

**Interfaces:**
- Produces: `Request::Screens { ids: Vec<u32> }`、`Response::Screens { screens: Vec<ScreenEntry> }`、`pub struct ScreenEntry { pub id: u32, pub lines: Vec<Vec<ScreenSpan>> }`。Task 2/3/5 都按这三个名字用。
- 注意：不带光标字段。只读格子不画光标，画了只会误导人去打字（见 spec）。

- [ ] **Step 1: 写失败的测试**

`src/proto.rs` 的 `mod tests` 里加：

```rust
#[test]
fn screens_request_round_trips() {
    let req = Request::Screens { ids: vec![1, 3, 7] };
    let s = serde_json::to_string(&req).unwrap();
    let back: Request = serde_json::from_str(&s).unwrap();
    match back {
        Request::Screens { ids } => assert_eq!(ids, vec![1, 3, 7]),
        other => panic!("解回来不是 Screens：{other:?}"),
    }
}

#[test]
fn screens_response_round_trips() {
    use crate::pty::{ScreenSpan, ScreenStyle};
    let resp = Response::Screens {
        screens: vec![ScreenEntry {
            id: 4,
            lines: vec![vec![ScreenSpan {
                text: "干活中".into(),
                style: ScreenStyle::default(),
            }]],
        }],
    };
    let s = serde_json::to_string(&resp).unwrap();
    let back: Response = serde_json::from_str(&s).unwrap();
    match back {
        Response::Screens { screens } => {
            assert_eq!(screens.len(), 1);
            assert_eq!(screens[0].id, 4);
            assert_eq!(screens[0].lines[0][0].text, "干活中");
        }
        other => panic!("解回来不是 Screens：{other:?}"),
    }
}
```

- [ ] **Step 2: 跑一遍确认编译失败**

```bash
cargo test -p dct proto -- --test-threads=1
```

预期：编译错误，`Screens`/`ScreenEntry` 不存在。（binary crate 名字如果不是 `dct`，去掉 `-p dct` 直接 `cargo test proto`。）

- [ ] **Step 3: 最小实现**

`src/proto.rs`：

```rust
/// 九宫格一格的内容。跟 `Response::Screen` 不同，不带光标——
/// 只读的格子画光标只会误导人去打字。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScreenEntry {
    pub id: u32,
    pub lines: Vec<Vec<ScreenSpan>>,
}
```

`Request` enum 里、`Screen { id }` 变体后面加：

```rust
/// 一次取多个会话的屏幕。九宫格九个格子要是一个个问，
/// 一问一答的串行连接上就是九个来回。
Screens {
    ids: Vec<u32>,
},
```

手写 `Debug` 的 `match` 里加一臂（不加编译不过，这是手写 Debug 的用意之一）：

```rust
Request::Screens { ids } => f.debug_struct("Screens").field("ids", ids).finish(),
```

`Response` enum 里、`Screen` 变体后面加：

```rust
Screens {
    screens: Vec<ScreenEntry>,
},
```

- [ ] **Step 4: 测试变绿**

```bash
cargo test -- --test-threads=1
```

预期：新增 2 个测试通过，总数 = 基线 + 2。

- [ ] **Step 5: 提交**

```bash
cargo fmt && cargo clippy --all-targets
git add src/proto.rs
git commit -m "feat: 协议加 Screens 批量取屏消息"
```

---

### Task 2: `SessionManager::screens()`

**Files:**
- Modify: `src/session.rs`

**Interfaces:**
- Consumes: Task 1 的 `ScreenEntry`；现有 `PtySession::screen_spans()`（`src/pty.rs:161`）。
- Produces: `pub fn screens(&self, ids: &[u32]) -> Vec<ScreenEntry>` —— 不存在的 id 静默跳过（会话可能刚被停掉，九宫格下一轮自然就不问它了，报错没有意义）。

- [ ] **Step 1: 写失败的测试**

`src/session.rs` 测试模块里。仿照同文件里现有的 manager 测试怎么建 `SessionManager` 和会话（用真进程；看现有测试用什么命令，通常是 `sh`/`sleep` 一类，照抄它的做法）：

```rust
#[test]
fn screens_returns_entries_for_known_ids_and_skips_unknown() {
    // 按本文件现有测试的方式构造 manager 和两个会话（真进程）。
    // 下面的 new_test_manager()/spawn_test_session() 指代现有测试
    // 已经在用的那套辅助——名字以文件里实际存在的为准，照着用。
    let mgr = new_test_manager();
    let id1 = spawn_test_session(&mgr);
    let id2 = spawn_test_session(&mgr);

    let entries = mgr.screens(&[id1, id2, 9999]);

    assert_eq!(entries.len(), 2, "9999 不存在，应该被跳过而不是报错");
    assert_eq!(entries[0].id, id1);
    assert_eq!(entries[1].id, id2);
    // 屏幕是 40 行的 vt100 缓冲，行数应该等于会话的行数
    assert_eq!(entries[0].lines.len(), 40);
}
```

如果现有测试没有可复用的辅助函数，就照最近一个「建 manager + 开会话」的测试内联写，别发明新的抽象。

- [ ] **Step 2: 确认失败**

```bash
cargo test screens_returns -- --test-threads=1
```

预期：编译错误，`screens` 方法不存在。

- [ ] **Step 3: 实现**

`src/session.rs`，放在 `screen()`（约 286 行）旁边：

```rust
/// 一次取多个会话的屏幕，九宫格用。锁的纪律跟 `list()` 一致：
/// 逐个短暂拿锁，不跨会话持有任何东西。不存在的 id 跳过——
/// 会话可能在两次轮询之间被停掉，这不是错误。
pub fn screens(&self, ids: &[u32]) -> Vec<crate::proto::ScreenEntry> {
    ids.iter()
        .filter_map(|id| {
            let session = self.get(*id)?;
            Some(crate::proto::ScreenEntry {
                id: *id,
                lines: session.pty.screen_spans(),
            })
        })
        .collect()
}
```

`self.get(*id)`、`session.pty` 这两处按 `screen()` 现有的写法对齐——它怎么从 id 拿到 `PtySession`，这里就怎么拿（可能是 map 查找加锁，照抄它的结构，包括 `unwrap_or_else(|e| e.into_inner())` 的毒锁处理）。

- [ ] **Step 4: 变绿**

```bash
cargo test -- --test-threads=1
```

- [ ] **Step 5: 提交**

```bash
cargo fmt && cargo clippy --all-targets
git add src/session.rs
git commit -m "feat: SessionManager 批量取屏 screens()"
```

---

### Task 3: daemon 分发 `Screens`

**Files:**
- Modify: `src/daemon.rs`（`Request::Screen` 分发臂旁边，约 187 行）

**Interfaces:**
- Consumes: Task 1 的消息、Task 2 的 `SessionManager::screens()`。
- Produces: socket 上 `Request::Screens` → `Response::Screens`。

- [ ] **Step 1: 写失败的集成测试**

daemon 的集成测试在哪、怎么起测试 daemon（临时目录的 socket 路径），看 `src/daemon.rs` 测试模块或 `tests/` 里现有的「起 daemon → 连 socket → 发请求」测试，照那个模式写：

```rust
#[test]
fn screens_request_returns_batch_over_socket() {
    // 照现有 daemon 集成测试的方式：临时目录 socket、起 daemon、
    // 建两个会话，然后：
    let resp = client.call(Request::Screens { ids: vec![id1, id2] }).unwrap();
    match resp {
        Response::Screens { screens } => {
            assert_eq!(screens.len(), 2);
            assert_eq!(screens[0].id, id1);
        }
        other => panic!("回的不是 Screens：{other:?}"),
    }
}
```

- [ ] **Step 2: 确认失败**

预期：daemon 对不认识的… 不对——Request 解析这一侧是新代码，daemon 编译后能解析但 `match` 不穷尽 → 编译错误。这正是要的失败。

- [ ] **Step 3: 实现**

`src/daemon.rs`，`Request::Screen { id }` 臂后面：

```rust
Request::Screens { ids } => Response::Screens {
    screens: manager.screens(&ids),
},
```

（`manager` 的实际变量名按 `Screen` 臂里的写法对齐。）

- [ ] **Step 4: 变绿**

```bash
cargo test -- --test-threads=1
```

- [ ] **Step 5: 提交**

```bash
cargo fmt && cargo clippy --all-targets
git add src/daemon.rs
git commit -m "feat: daemon 分发 Screens 批量取屏"
```

---

### Task 4: 九宫格纯数学（布局、翻页、裁剪）

**Files:**
- Create: `src/ui/grid.rs`（本任务只放纯函数和测试，视图接线在 Task 5）
- Modify: `src/ui/mod.rs`（加 `mod grid;`）

**Interfaces:**
- Consumes: `ScreenSpan`/`ScreenStyle`（`crate::pty`）；`display_width`/`char_width`（`src/ui/widgets.rs`，拆分后在那里；如果还是私有的，改成 `pub(crate)`）。
- Produces（Task 5 按这些签名调）:
  - `pub const TILES_PER_PAGE: usize = 9;`
  - `pub fn grid_shape(count: usize) -> (u16, u16)` —— 返回（行数，列数）
  - `pub fn page_of(focus: usize) -> usize`、`pub fn page_count(total: usize) -> usize`
  - `pub fn move_focus(focus: usize, total: usize, dir: Dir) -> usize`（`pub enum Dir { Up, Down, Left, Right }`）
  - `pub fn crop_line(spans: &[ScreenSpan], max_cols: usize) -> Vec<ScreenSpan>`

- [ ] **Step 1: 写失败的测试**

`src/ui/grid.rs`：

```rust
//! 九宫格的布局数学。全是纯函数，跟终端、协议、会话都没关系——
//! 能独立测，也只在这里测。

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pty::{ScreenSpan, ScreenStyle};

    #[test]
    fn shape_scales_with_session_count() {
        assert_eq!(grid_shape(1), (1, 1));
        assert_eq!(grid_shape(2), (1, 2));
        assert_eq!(grid_shape(3), (2, 2));
        assert_eq!(grid_shape(4), (2, 2));
        assert_eq!(grid_shape(5), (2, 3));
        assert_eq!(grid_shape(6), (2, 3));
        assert_eq!(grid_shape(7), (3, 3));
        assert_eq!(grid_shape(9), (3, 3));
        // 超过 9 的调用方先按页切好再问形状，这里按满页算
        assert_eq!(grid_shape(0), (1, 1), "空看板画一个空格子占位");
    }

    #[test]
    fn paging_math() {
        assert_eq!(page_of(0), 0);
        assert_eq!(page_of(8), 0);
        assert_eq!(page_of(9), 1);
        assert_eq!(page_count(0), 1);
        assert_eq!(page_count(9), 1);
        assert_eq!(page_count(10), 2);
    }

    #[test]
    fn focus_moves_in_two_dimensions_and_wraps_pages() {
        // 5 个会话 → 2×3 布局，index 0..=4
        assert_eq!(move_focus(0, 5, Dir::Right), 1);
        assert_eq!(move_focus(2, 5, Dir::Down), 4, "2 的正下方越出最后一行，收到最后一格");
        assert_eq!(move_focus(0, 5, Dir::Down), 3);
        assert_eq!(move_focus(4, 5, Dir::Right), 0, "尾格右移回绕到头");
        assert_eq!(move_focus(0, 5, Dir::Left), 4, "头格左移回绕到尾");
        // 10 个会话：第 8 格（第一页尾）右移进第二页
        assert_eq!(move_focus(8, 10, Dir::Right), 9);
        assert_eq!(move_focus(9, 10, Dir::Right), 0);
    }

    fn sp(text: &str) -> ScreenSpan {
        ScreenSpan { text: text.into(), style: ScreenStyle::default() }
    }

    #[test]
    fn crop_cuts_at_display_width_without_splitting_wide_chars() {
        // "干活中" 每个字占 2 列。上限 5 列 → 只装得下 2 个字（4 列），
        // 第 3 个字会跨过边界，整个丢掉。
        let out = crop_line(&[sp("干活中")], 5);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].text, "干活");

        // 跨 span 累计：第一个 span 占 3 列，剩 2 列只够 "b" 一个
        let out = crop_line(&[sp("abc"), sp("bcd")], 5);
        assert_eq!(out[1].text, "bc");

        // 不超限的原样保留
        let out = crop_line(&[sp("ok")], 80);
        assert_eq!(out[0].text, "ok");
    }
}
```

- [ ] **Step 2: 确认失败**

`src/ui/mod.rs` 里加 `mod grid;` 后：

```bash
cargo test grid -- --test-threads=1
```

预期：编译错误，函数们不存在。

- [ ] **Step 3: 实现**

```rust
use crate::pty::ScreenSpan;
use super::widgets::char_width;

pub const TILES_PER_PAGE: usize = 9;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Dir {
    Up,
    Down,
    Left,
    Right,
}

/// 当页格子数 → （行数，列数）。上限九格；空看板画一个空格子占位，
/// 免得渲染分支到处判零。
pub fn grid_shape(count: usize) -> (u16, u16) {
    match count {
        0 | 1 => (1, 1),
        2 => (1, 2),
        3 | 4 => (2, 2),
        5 | 6 => (2, 3),
        _ => (3, 3),
    }
}

pub fn page_of(focus: usize) -> usize {
    focus / TILES_PER_PAGE
}

pub fn page_count(total: usize) -> usize {
    if total == 0 {
        1
    } else {
        total.div_ceil(TILES_PER_PAGE)
    }
}

/// 焦点在格子间移动。左右在全体会话上一维回绕（越过页边自然翻页）；
/// 上下在当页的二维布局里走，向下越出最后一行收到最后一格。
pub fn move_focus(focus: usize, total: usize, dir: Dir) -> usize {
    if total == 0 {
        return 0;
    }
    let page_start = page_of(focus) * TILES_PER_PAGE;
    let in_page = focus - page_start;
    let page_len = (total - page_start).min(TILES_PER_PAGE);
    let (_, cols) = grid_shape(page_len);
    let cols = cols as usize;
    match dir {
        Dir::Right => (focus + 1) % total,
        Dir::Left => (focus + total - 1) % total,
        Dir::Down => {
            let down = in_page + cols;
            page_start + down.min(page_len - 1)
        }
        Dir::Up => page_start + in_page.saturating_sub(cols),
    }
}

/// 按显示宽度裁一行。宽字符（CJK 占两列）跨过边界就整个丢掉——
/// 裁一半会把后面所有列推歪。宽度的定义必须跟 widgets 里的
/// `char_width` 是同一份，两边悄悄分叉的话裁的位置就对不上。
pub fn crop_line(spans: &[ScreenSpan], max_cols: usize) -> Vec<ScreenSpan> {
    let mut out: Vec<ScreenSpan> = Vec::new();
    let mut used = 0usize;
    for sp in spans {
        if used >= max_cols {
            break;
        }
        let mut text = String::new();
        for ch in sp.text.chars() {
            let w = char_width(ch);
            if used + w > max_cols {
                break;
            }
            used += w;
            text.push(ch);
        }
        if !text.is_empty() {
            out.push(ScreenSpan {
                text,
                style: sp.style,
            });
        }
    }
    out
}
```

`char_width` 拆分后如果在 `widgets.rs` 里是私有的，改 `pub(crate)`；如果它还叫别的名字或还留在别处，`grep -rn "fn char_width" src/` 找到后对齐路径。

- [ ] **Step 4: 变绿**

```bash
cargo test -- --test-threads=1
```

- [ ] **Step 5: 提交**

```bash
cargo fmt && cargo clippy --all-targets
git add src/ui/grid.rs src/ui/mod.rs src/ui/widgets.rs
git commit -m "feat: 九宫格布局与裁剪的纯函数"
```

---

### Task 5: `View::Grid` 视图接线

**Files:**
- Modify: `src/ui/view.rs`（`View` enum 加变体、`back_one_level`、`escape_hint`）
- Modify: `src/ui/mod.rs`（run 循环：轮询 + 按键分发）
- Modify: `src/ui/grid.rs`（加渲染函数）
- Modify: `src/ui/board.rs`（`g` 键入口）

**Interfaces:**
- Consumes: Task 1 消息、Task 4 全部函数、`screen_to_lines`/`to_style`（现在多半在 `attach.rs` 或 `widgets.rs`，`grep -rn "fn screen_to_lines" src/ui/` 找到后设为 `pub(crate)`）、`status_label`/`status_color`（`widgets.rs`）。
- Produces: `View::Grid { focus: usize }` 变体；`grid::draw_grid(...)` 渲染入口。

- [ ] **Step 1: View 变体与返回路径的测试**

`src/ui/view.rs` 测试里（仿照现有 `back_one_level` 的测试，约老 ui.rs 2398 行那种）：

```rust
#[test]
fn grid_backs_out_to_board() {
    assert!(matches!(back_one_level(View::Grid { focus: 3 }), Some(View::Board)));
}
```

- [ ] **Step 2: 确认失败 → 加变体**

`View` enum 加：

```rust
/// 九宫格：平铺所有会话的实时画面，只读。focus 是全体会话里的
/// 下标（不是当页内的），翻页从它推导，见 grid::page_of。
Grid {
    focus: usize,
},
```

`back_one_level` 里 `Grid` → `Some(View::Board)`。`escape_hint`（或对应状态栏提示函数）加一臂：

```rust
View::Grid { .. } => "方向键移动　Enter 放大　F3 下一格　g 回列表　s 停止　u 撤销　d 看改动",
```

跑测试变绿。

- [ ] **Step 3: 轮询接线**

`src/ui/mod.rs` run 循环里，仿照 `View::Attached` 的取屏块（老 ui.rs 412–434 的位置），加 Grid 的分支。循环外补两个状态：

```rust
let mut grid_screens: Vec<crate::proto::ScreenEntry> = Vec::new();
let mut grid_last_fetch: Option<std::time::Instant> = None;
```

分支：

```rust
if let View::Grid { focus } = &view {
    // 300ms 一轮就够：格子是扫一眼的东西，不是打字的地方。
    // 附加视图的 16ms 是为了跟手，这里没有手要跟。
    let due = grid_last_fetch.map_or(true, |t| t.elapsed().as_millis() >= 300);
    if due {
        let start = grid::page_of(*focus) * grid::TILES_PER_PAGE;
        let ids: Vec<u32> = sessions
            .iter()
            .skip(start)
            .take(grid::TILES_PER_PAGE)
            .map(|s| s.id)
            .collect();
        match client.call(Request::Screens { ids }) {
            Ok(Response::Screens { screens }) => {
                grid_screens = screens;
                connected = true;
            }
            Ok(Response::Error(_)) => {
                // 老守护进程不认识 Screens。列表视图还能用，
                // 提示怎么修，别让用户对着空格子猜。
                message = Msg::err("后台服务版本太老，画面拿不到。重启它：dct restart".into());
                view = View::Board;
            }
            _ => connected = false,
        }
        grid_last_fetch = Some(std::time::Instant::now());
    }
}
```

注意：Grid 视图**不属于** `attached`，所以 398 行那个 `if need_sessions || !attached` 的会话列表轮询自然覆盖它（150ms 拿一次 `List`，格子标题的状态就是新鲜的）——确认这一点，别再加一路 List 轮询。`Msg::err` 与 `message` 的实际写法对齐现有代码。如果 `dct restart` 子命令还不存在（它属于滚屏计划 0.1），提示改成「后台服务版本太老，画面拿不到。退出后重新启动它再试」。

- [ ] **Step 4: 渲染**

`src/ui/grid.rs` 加（`use` 按需补，ratatui 类型对齐 `board.rs` 现有 import 风格）：

```rust
/// 画九宫格。tiles 的顺序 = sessions 当页的顺序；screens 里按 id 配对，
/// 一时没取到画面的格子只画标题和空白——下一轮 300ms 就有了，
/// 比画错内容强。
pub fn draw_grid(
    f: &mut Frame,
    area: Rect,
    sessions: &[SessionInfo],
    screens: &[crate::proto::ScreenEntry],
    focus: usize,
) {
    // 终端太小画不下格子，说人话让用户放大窗口
    if area.width < 60 || area.height < 20 {
        let msg = Paragraph::new("窗口太小，放大终端窗口后再看九宫格");
        f.render_widget(msg, area);
        return;
    }

    let total = sessions.len();
    let page = page_of(focus);
    let start = page * TILES_PER_PAGE;
    let page_sessions = &sessions[start.min(total)..(start + TILES_PER_PAGE).min(total)];
    let (rows, cols) = grid_shape(page_sessions.len());

    let row_areas = Layout::vertical(vec![Constraint::Ratio(1, rows as u32); rows as usize]).split(area);
    let mut tile_areas: Vec<Rect> = Vec::new();
    for r in row_areas.iter() {
        tile_areas.extend(
            Layout::horizontal(vec![Constraint::Ratio(1, cols as u32); cols as usize])
                .split(*r)
                .iter()
                .copied(),
        );
    }

    for (i, info) in page_sessions.iter().enumerate() {
        let tile = tile_areas[i];
        let focused = start + i == focus;
        // 标题就是状态指示器：状态词用 status_color 上色（干活中/空闲/
        // 已停止各自的颜色跟列表一致），扫一眼九个格子就知道谁在干活。
        let title = Line::from(vec![
            Span::raw(format!(" #{} {} ", info.id, info.profile)),
            Span::styled(
                format!("{} ", status_label(&info.state)),
                Style::default().fg(status_color(&info.state)),
            ),
        ]);
        let border = if focused {
            Style::default().fg(Color::Cyan)
        } else {
            Style::default().fg(Color::DarkGray)
        };
        let block = Block::bordered().title(title).border_style(border);
        let inner = block.inner(tile);
        f.render_widget(block, tile);

        if let Some(entry) = screens.iter().find(|e| e.id == info.id) {
            // 底部 N 行：agent 的输入框和最新输出都在屏幕底部
            let h = inner.height as usize;
            let skip = entry.lines.len().saturating_sub(h);
            let cropped: Vec<Vec<ScreenSpan>> = entry.lines[skip..]
                .iter()
                .map(|l| crop_line(l, inner.width as usize))
                .collect();
            f.render_widget(Paragraph::new(screen_to_lines(&cropped)), inner);
        }
    }

    // 页码画在右下角，只有多页才画——单页画 1/1 是噪音
    let pages = page_count(total);
    if pages > 1 {
        let label = format!("{}/{}", page + 1, pages);
        let w = label.len() as u16;
        let corner = Rect {
            x: area.x + area.width.saturating_sub(w + 1),
            y: area.y + area.height.saturating_sub(1),
            width: w,
            height: 1,
        };
        f.render_widget(Paragraph::new(label), corner);
    }
}
```

`status_label` 的签名（拿 `&SessionState` 还是别的）、`SessionInfo` 的 import 路径、`Frame`/`Rect` 的泛型写法，全部对齐 `board.rs` 现状。已停止的会话 `status_label` 自然给出停止字样，标题跟列表一致地变灰（`status_color` 有现成的映射就用它给标题上色）。

主 `draw` 函数（拆分后在 `mod.rs` 或 `view.rs`）的视图分发里加 `View::Grid` 臂调 `draw_grid`。会话列表为空时 `page_sessions` 为空、循环不进、只画一个空区域——加一句居中提示「没有会话，按 n 开一个」（对齐看板空态的现有文案，有现成的就复用）。

- [ ] **Step 5: 按键接线**

`board.rs` 的看板按键 `match` 里加：

```rust
KeyCode::Char('g') => {
    // 进九宫格时焦点落在列表当前选中的那一行，两个视图对同一个
    // 会话的"当前"认知要一致，不然按完 g 焦点跳到别处会迷路
    let focus = list_state.selected().unwrap_or(0);
    view = View::Grid { focus };
}
```

`mod.rs`（或按键分发所在处）加 `View::Grid` 臂：

```rust
View::Grid { focus } => match key.code {
    KeyCode::Up => view = View::Grid { focus: grid::move_focus(focus, sessions.len(), grid::Dir::Up) },
    KeyCode::Down => view = View::Grid { focus: grid::move_focus(focus, sessions.len(), grid::Dir::Down) },
    KeyCode::Left => view = View::Grid { focus: grid::move_focus(focus, sessions.len(), grid::Dir::Left) },
    KeyCode::Right | KeyCode::F(3) => view = View::Grid { focus: grid::move_focus(focus, sessions.len(), grid::Dir::Right) },
    KeyCode::Char('g') => view = View::Board,
    KeyCode::Enter => {
        if let Some(s) = sessions.get(focus) {
            need_sessions = true; // 会话标题要显示项目名
            view = View::Attached(s.id);
        }
    }
    KeyCode::Char('s') | KeyCode::Char('u') | KeyCode::Char('d') => {
        // 跟看板同一套动作，作用在焦点格上。看板的 s/u/d 分支怎么发
        // Stop/Undo/Diff、怎么设置 message，这里原样复用——最好把
        // 看板那三段提炼成一个接 id 的辅助函数，两个视图共用，
        // 而不是复制一份将来改一半漏一半。
        if let Some(s) = sessions.get(focus) {
            session_action(&mut client, key.code, s.id, &mut message, &mut need_sessions);
        }
    }
    _ => {}
},
```

`s`/`u`/`d` 那段是真提炼不是注释了事：把 `board.rs` 里对应三个分支的请求发送 + message 设置抽成 `fn session_action(client: &mut Client, key: KeyCode, id: u32, message: &mut Msg, need_sessions: &mut bool)`（签名按实际依赖调整），看板和九宫格都调它。房规重申：这些新分支里**不要 `continue`**。

`Ctrl+Q` 的全局拦截走 `back_one_level`，Task 5 Step 2 已经让 Grid 回 Board，不用再写。

- [ ] **Step 6: 全量验证**

```bash
cargo test -- --test-threads=1
cargo fmt --check
cargo clippy --all-targets
```

手动烟测（真终端）：`cargo run --release`，开两三个 `命令行` 会话 → `g` 进九宫格 → 格子里能看到 shell 画面在动 → 方向键移焦点 → `Enter` 放大 → `Ctrl+Q` 回九宫格…… 不对，`back_one_level(Attached)` 回的是 Board——确认这符合 spec（spec 说 `Ctrl+Q` 从九宫格回列表；从放大回哪里 spec 没说，回 Board 是现状，保持现状）。缩小终端窗口到很小，确认显示「窗口太小」而不是画残。

- [ ] **Step 7: 提交**

```bash
git add src/ui/
git commit -m "feat: 看板九宫格视图——平铺实时画面，Enter 放大"
```

---

### Task 6: `F3` 会话轮转（附加视图）

**Files:**
- Modify: `src/ui/attach.rs`（附加视图按键，老 ui.rs 950–959 对应处）
- Modify: `src/ui/grid.rs` 或按键分发处（九宫格的 F3 已在 Task 5 接好，这里只管附加视图）
- Modify: 状态栏提示文案（老 ui.rs 1892 对应处）

**Interfaces:**
- Consumes: `sessions: &[SessionInfo]`（run 循环已有）、`SessionState`。
- Produces: `pub fn next_running(sessions: &[SessionInfo], current: u32) -> Option<u32>`（放 `grid.rs`，九宫格与附加视图共用的轮转逻辑，虽然九宫格今天用的是 move_focus——放一起是因为它们是同一个「下一个」概念）。

- [ ] **Step 1: 写失败的测试**

`src/ui/grid.rs` 测试里（`SessionInfo`/`SessionState` 的构造按 `src/session.rs` 里现有测试的写法，字段全给上）：

```rust
#[test]
fn next_running_wraps_and_skips_stopped() {
    let sessions = vec![
        info(1, SessionState::Working),
        info(2, SessionState::Stopped),
        info(3, SessionState::Idle),
    ];
    assert_eq!(next_running(&sessions, 1), Some(3), "2 停了要跳过");
    assert_eq!(next_running(&sessions, 3), Some(1), "到尾回绕");
    let only = vec![info(1, SessionState::Working)];
    assert_eq!(next_running(&only, 1), None, "没有别的会话就别跳，跳回自己是噪音");
}
```

`fn info(id, state) -> SessionInfo` 是测试内的小构造函数，其余字段填默认值（`profile: "claude".into()` 等）。`SessionState` 的变体名以 `src/session.rs` 实际定义为准（`grep -n "enum SessionState" src/session.rs`），对不上就改测试里的名字，不改语义。

- [ ] **Step 2: 确认失败 → 实现**

```rust
/// 下一个还在跑的会话，按 id 在 sessions 里的顺序，到尾回绕，
/// 跳过已停止的（停了的没画面可看，列表里处理它）。当前会话是
/// 唯一在跑的 → None，调用方原地不动。
pub fn next_running(sessions: &[SessionInfo], current: u32) -> Option<u32> {
    let cur = sessions.iter().position(|s| s.id == current)?;
    let n = sessions.len();
    (1..n)
        .map(|off| &sessions[(cur + off) % n])
        .find(|s| !matches!(s.state, SessionState::Stopped))
        .map(|s| s.id)
}
```

- [ ] **Step 3: 附加视图接键**

`attach.rs` 按键处理里，`F2` 的分支（老 ui.rs 957）旁边加：

```rust
if key.code == KeyCode::F(3) {
    // F3 = 直接切到下一个在跑的会话，不用先退回看板。选 F3 沿用
    // F2 的理由：没有 CLI agent 用 F 功能键，偷它不踩任何人。
    match crate::ui::grid::next_running(&sessions, id) {
        Some(next) => {
            need_sessions = true; // 会话标题要显示新会话的项目名
            view = View::Attached(next);
        }
        None => message = Msg::info("没有其他正在跑的会话".into()),
    }
    // 这里跟着现有 F2 分支的控制流写法走（它怎么结束这次按键处理，
    // 这里就怎么结束）——但记住房规：不要 continue。
}
```

`Msg::info` 与实际的 message API 对齐；这个分支必须放在「其余按键 `key_to_input` 转发给 agent」**之前**，位置对齐 F2 的拦截点。`key_to_input` 不用改——`F(3)` 落在 `_ => return None`，本来就不转发（老 ui.rs 1630 一带的通配臂），**加一个测试钉住这件事**：

```rust
#[test]
fn f3_is_never_forwarded_to_the_agent() {
    assert_eq!(key_to_input(&key(KeyCode::F(3))), None);
}
```

（`key(...)` 辅助函数在 key_to_input 现有测试里已经有，照用。）

- [ ] **Step 4: 提示文案**

附加视图状态栏（老 ui.rs 1892）改成：

```rust
View::Attached(_) => "F2 同效　F3 下一个会话　回看板后按 n 新建会话　其余按键都发给 agent",
```

有断言提示文案的测试（老 ui.rs 3422 那种）会碎，按新文案更新断言——断言的意图（「F2 肌肉记忆要留在提示里」）不能丢。

- [ ] **Step 5: 全量验证 + 提交**

```bash
cargo test -- --test-threads=1
cargo fmt --check && cargo clippy --all-targets
git add src/ui/
git commit -m "feat: F3 在会话之间轮转"
```

---

### Task 7: 文档与收尾

**Files:**
- Modify: `README.md`（看板按键表、会话内按键说明）
- Modify: `README.zh-CN.md`（同步）

- [ ] **Step 1: README 按键表加两行**

看板按键表加：

```markdown
| `g` | tile grid: every session's live screen at once; `Enter` zooms in |
| `F3` | jump to the next running session (works inside a session too) |
```

「Inside a session」那段的两个保留键说明改为三个：`F2`、`F3`、`Ctrl+Q`（措辞对齐现有句式：agents keep every other key）。`README.zh-CN.md` 做同样的两处改动，中文措辞对齐该文件现有风格。

- [ ] **Step 2: 全量最终验证**

```bash
cargo test -- --test-threads=1
cargo fmt --check
cargo clippy --all-targets
```

测试总数 ≥ 基线 + 本计划新增的每一个（Task 1 两个、Task 2 一个、Task 3 一个、Task 4 四个、Task 5 一个、Task 6 两个 ≈ 基线 + 11，以实际为准，只增不减）。

- [ ] **Step 3: 提交**

```bash
git add README.md README.zh-CN.md
git commit -m "docs: README 加九宫格与 F3 轮转"
```
