### Task 4: 接进 `ui.rs`

把 `DIM` 常量换成运行时探测的结果。这一步之后功能才真的生效。

**Files:**
- Modify: `src/ui.rs`（删 `DIM` 常量；加 `OnceLock` 与 `dim()`；`run()` 里初始化；10 处引用改掉；`status_color` → `status_style`）

**Interfaces:**
- Consumes: Task 1 的 `Theme::dim()`；Task 3 的 `theme::detect()`
- Produces: `ui::status_style(s: SessionState) -> Style`（取代 `ui::status_color(s) -> Color`）

- [ ] **Step 1: 写失败的测试**

在 `src/ui.rs` 的 `mod tests`（约 2327 行起）里，把现有的 `asking_and_working_use_different_colors` 测试**整个替换**成下面三个。原测试断言的是 `status_color` 的返回值，那个函数这一步会消失。

找到：

```rust
    #[test]
    fn asking_and_working_use_different_colors() {
        assert_ne!(
            status_color(SessionState::Asking),
            status_color(SessionState::Working)
        );
    }
```

替换成：

```rust
    #[test]
    fn asking_and_working_use_different_colors() {
        assert_ne!(
            status_style(SessionState::Asking),
            status_style(SessionState::Working)
        );
    }

    /// Stopped/Unknown 这两个「没在干活」的状态要走弱化样式，跟说明栏、
    /// 不可用项用的是同一套自适应灰，不能再自己钉一个写死的颜色。
    #[test]
    fn inactive_states_use_the_adaptive_dim_style() {
        let dim = dim();
        assert_eq!(status_style(SessionState::Stopped), dim);
        assert_eq!(status_style(SessionState::Unknown), dim);
    }

    /// 测试进程里没人调过 `init_theme`，`dim()` 必须给出 `Unknown` 的样式，
    /// 而不是 panic 或者某个写死的灰。这条同时守着「探测没跑过也能正常渲染」
    /// 这个前提——`draw()` 的那批渲染测试全靠它。
    #[test]
    fn dim_falls_back_to_unknown_before_detection() {
        assert_eq!(dim(), crate::theme::Theme::Unknown.dim());
    }
```

- [ ] **Step 2: 跑测试确认失败**

Run: `export PATH="$HOME/.cargo/bin:$PATH" && cargo test --lib ui 2>&1 | head -20`
Expected: 编译错误，`cannot find function status_style` / `cannot find function dim`。

- [ ] **Step 3: 写实现**

**3a.** 在 `src/ui.rs` 的 `use` 块里补一行（跟其它 `use crate::` 放一起）：

```rust
use crate::theme::Theme;
```

并在 `use std::path::{Path, PathBuf};` 附近补：

```rust
use std::sync::OnceLock;
```

**3b.** 把现有的 `DIM` 常量（约 32–38 行，从 `/// 弱化文字…` 那段注释到 `const DIM: Color = Color::Indexed(245);`）**整段删掉**，换成：

```rust
/// 启动时探测出来的终端背景。`ui::run()` 设一次，之后只读。
///
/// 用全局而不是给 `DrawInput` 加字段：主题是进程级配置，启动后不变，
/// 塞进 `DrawInput` 是把一个常量伪装成每帧的状态。而且 `DrawInput` 有
/// 26 个构造点（25 个在测试里），加一个必填字段就是 26 处纯噪音的改动。
static THEME: OnceLock<Theme> = OnceLock::new();

/// 探测终端背景并记下来。`run()` 在 `enable_raw_mode()` 之后、
/// `EnterAlternateScreen` 之前调，只调一次。
pub fn init_theme() {
    let _ = THEME.set(crate::theme::detect());
}

/// 弱化文字（说明栏、不可用项、操作提示、没在干活的状态）的样式。
///
/// 没探测过就按 `Unknown` 算——那是三种取值里最保守的一个（只用 DIM
/// 修饰符，不钉任何颜色），所以测试和任何绕过 `run()` 的路径都能正常渲染。
pub fn dim() -> Style {
    THEME.get().copied().unwrap_or(Theme::Unknown).dim()
}
```

**3c.** 把 `status_color` 改成 `status_style`（约 40–47 行）：

```rust
/// 状态在界面上的样式。返回 `Style` 而不是 `Color`：Stopped/Unknown 要用
/// `dim()`，而 `dim()` 在 `Theme::Unknown` 下表达的是 DIM 修饰符、不是某个
/// 颜色，`Color` 装不下。
///
/// 干活中/等你回答/空闲仍用具名 ANSI 色：终端主题本来就保证这几个色在自己
/// 背景上可读，我们再去重映射等于跟用户自己的配色打架。
pub fn status_style(s: SessionState) -> Style {
    match s {
        SessionState::Working => Style::default().fg(Color::Cyan),
        SessionState::Asking => Style::default().fg(Color::Yellow),
        SessionState::Idle => Style::default().fg(Color::Green),
        SessionState::Stopped => dim(),
        SessionState::Unknown => dim(),
    }
}
```

**3d.** 改 `status_color` 的生产调用点（原约 2197 行，现在行号会偏移）。找到：

```rust
                            Style::default().fg(status_color(s.state)),
```

改成：

```rust
                            status_style(s.state),
```

**3e.** 把剩下 8 处 `DIM` 引用改成 `dim()`。用这条命令定位：

```bash
export PATH="$HOME/.cargo/bin:$PATH"
grep -n "DIM" src/ui.rs
```

两种改法，按上下文选：
- `Style::default().fg(DIM)` → `dim()`
- `base.fg(DIM)` → `base.patch(dim())`

第二种为什么用 `patch` 而不是 `.fg(...)`：`base` 那一层带着「整行是否压暗」的信息，而 `dim()` 在 `Unknown` 下给的是修饰符不是颜色，`.fg()` 收不下一个 `Style`。`patch` 把两个 `Style` 叠起来，两种取值都对。

**3f.** 在 `run()` 里初始化。找到（约 229–232 行）：

```rust
    enable_raw_mode()?;
    // 必须在 EnterAlternateScreen / Terminal::new 之前构造：这样即便它们俩失败，
    // raw mode 也还是能被 Drop 恢复。
    let _guard = TerminalGuard;
```

在 `let _guard = TerminalGuard;` 之后插入：

```rust
    // 探测终端背景，位置被两头夹死：
    // - 必须在 enable_raw_mode() 之后：OSC 11 的回复是终端塞进 stdin 的
    //   一串字节，非 raw 模式下会被行缓冲（它不带换行，读不出来）并且被
    //   回显到屏幕上（用户会看见乱码）。
    // - 必须在 EnterAlternateScreen 之前：万一有字节漏到屏幕上，此刻还在
    //   主屏、还没开始画界面，脏字符会被随后的 alternate screen 切换盖掉；
    //   反过来就是把乱码糊在已经画好的界面上。
    // 在 TerminalGuard 之后是为了万一探测里有什么 panic，raw mode 仍能恢复。
    init_theme();
```

- [ ] **Step 4: 跑全量测试确认通过**

Run: `export PATH="$HOME/.cargo/bin:$PATH" && cargo build 2>&1 | tail -5`
Expected: `Finished`，没有 warning。特别确认没有残留的 `DIM` 或 `status_color`：

Run: `grep -n "DIM\|status_color" src/ui.rs; echo "--- 上面应该只剩注释里提到 DIM 修饰符的地方，没有 status_color"`
Expected: 没有 `status_color`；`DIM` 只可能出现在注释里。

Run: `export PATH="$HOME/.cargo/bin:$PATH" && cargo test 2>&1 | grep -E "^test result|error"`
Expected: 所有测试套件 `ok`，0 failed。基线是改动前的 172 个单元测试 + 本计划新增的 23 个，即 195 个左右，加上各集成测试套件原有的数目。

- [ ] **Step 5: 提交**

```bash
git add src/ui.rs
git commit -m "feat: adapt dim text color to the terminal background

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>"
```

---

