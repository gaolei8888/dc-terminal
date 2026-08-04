# dct 自适应配色 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 让界面上的弱化文字（说明栏、不可用项、操作提示、Stopped/Unknown 状态）跟着终端背景的深浅自动选灰度，探测不出来时退回终端自己的 DIM 属性。

**Architecture:** 新模块 `src/theme.rs` 拿一个 `Theme { Dark, Light, Unknown }` 表示背景，`Theme::dim()` 给出对应的 `Style`。探测按 `DCT_THEME` 环境变量 → OSC 11 查询 → `COLORFGBG` 环境变量 → `Unknown` 四级降级。`ui.rs` 用一个 `OnceLock<Theme>` 存启动时探测的结果，渲染处调 `theme::dim()`。

**Tech Stack:** Rust 2021，ratatui 0.28（`Style` / `Color::Indexed` / `Modifier::DIM`），libc 0.2（`poll(2)` 做读超时）。测试是模块内联 `#[cfg(test)] mod tests`，跑 `cargo test`。

**Spec:** `docs/superpowers/specs/2026-08-04-dct-adaptive-color-design.md`

## Global Constraints

- **不加新依赖。** `libc` 和 `ratatui` 已经在 `Cargo.toml` 里，够用。不要引入 `termbg` / `terminal-light`。
- **探测不可能让界面启动失败。** 每一种失败（无回复、超时、格式错、stdin 不是 tty、十六进制解析不了）都只降到链条下一步，最终落到 `Unknown`。`detect()` 不返回 `Result`，没有 `?`，没有 `unwrap`/`expect`/`panic!`。
- **只有灰适配。** 具名 ANSI 色（`Color::Red` / `Cyan` / `Yellow` / `Green`）和会话画面里 agent 输出的颜色（`ScreenColor::Idx` / `Rgb`）一律不动。
- **OSC 11 超时上限 150ms**，硬性。
- **注释写「为什么」，不写「做什么」，用中文**，跟本仓库现有风格一致（见 `src/clipboard.rs`、`src/profile.rs`）。
- **`cargo build` 不能有 warning**，`cargo test` 全绿才算一个 task 完成。
- 这个仓库 `cargo` 不在默认 PATH 上。每个 shell 步骤先 `export PATH="$HOME/.cargo/bin:$PATH"`，或者用 `~/.cargo/bin/cargo`。
- 工作目录一律是 `/Users/lei/Documents/work/dc/dc-terminal`。

## File Structure

| 文件 | 职责 |
|---|---|
| `src/theme.rs`（新建） | `Theme` 枚举、`dim()` 样式映射、四级探测链、三个纯解析函数、`ReplyReader` 抽象 |
| `src/lib.rs`（改） | 加一行 `pub mod theme;` |
| `src/ui.rs`（改） | 删掉 `DIM` 常量；加 `OnceLock<Theme>` 和 `run()` 里的初始化；10 处引用改调 `theme::dim()`；`status_color` 改名 `status_style` 并改返回类型 |

为什么探测和样式映射放同一个文件：它们改的时候一起改（加一级探测源、调一档灰度，都是「这个终端底色该配什么灰」这一件事），拆两个文件只会让人两头翻。整个模块预计 250 行上下，含测试。

---

### Task 1: `Theme` 枚举与 `dim()` 样式映射

先把「三种背景各配什么样式」这个决定单独落地并锁在测试里。探测逻辑一行都还不写——它是在回答一个不同的问题（现在是什么背景），而这个 task 回答的是「知道背景之后用什么样式」。

**Files:**
- Create: `src/theme.rs`
- Modify: `src/lib.rs`（加 `pub mod theme;`）

**Interfaces:**
- Consumes: 无（第一个 task）
- Produces:
  - `pub enum Theme { Dark, Light, Unknown }`，derive `Debug, Clone, Copy, PartialEq, Eq`
  - `pub fn Theme::dim(self) -> ratatui::style::Style`

- [ ] **Step 1: 建 `src/theme.rs`，只写测试**

创建 `src/theme.rs`，内容就是下面这些（`Theme` 和 `dim` 还不存在，所以编译会失败，这是预期的）：

```rust
//! 终端背景是深是浅，以及据此选出的弱化文字样式。
//!
//! 存在的理由是一个真实事故：界面上所有弱化文字原本用 `Color::DarkGray`
//! （ANSI 亮黑，8 号色），而 Solarized 一类主题把 8 号色定义成和背景同色，
//! 于是选 agent 菜单在这些主题下渲染成一片空白——六个不可用的 agent、
//! 每行的说明栏、底部提示全部隐形，只剩一个悬空的 `▶`。
//!
//! 换成写死的 256 色灰能治好深色背景，但那个灰在浅色背景上同样接近隐形。
//! 一个写死的灰不可能同时适配深浅两种底色，所以这里让它跟着背景走。

use ratatui::style::{Color, Modifier, Style};

#[cfg(test)]
mod tests {
    use super::*;

    /// 三种背景必须给出三种不同的样式，否则「自适应」就是假的。
    #[test]
    fn each_theme_has_a_distinct_dim_style() {
        assert_ne!(Theme::Dark.dim(), Theme::Light.dim());
        assert_ne!(Theme::Dark.dim(), Theme::Unknown.dim());
        assert_ne!(Theme::Light.dim(), Theme::Unknown.dim());
    }

    /// 这条断言守的是整个设计的安全网：`Unknown` 意味着我们不知道背景是什么，
    /// 这时候**绝不能**写死任何前景色——写死就有撞上某个主题背景色的可能，
    /// 也就是重演一次 Solarized 事故。只能用 DIM 修饰符让终端自己去暗化
    /// 默认前景色。不支持 DIM 的终端会忽略它，文字以正常亮度显示：不够弱，
    /// 但看得见。失败方向必须是「不够暗」，不能是「隐形」。
    ///
    /// 以后如果有人觉得 `Unknown` 太亮想「顺手」给它补一个灰，这个测试会拦住。
    #[test]
    fn unknown_never_pins_a_foreground_color() {
        let s = Theme::Unknown.dim();
        assert_eq!(s.fg, None);
        assert!(s.add_modifier.contains(Modifier::DIM));
    }

    /// 深色背景要亮灰、浅色背景要暗灰。搞反了就是在白底上写白字。
    #[test]
    fn dark_gets_a_lighter_gray_than_light() {
        let (Some(Color::Indexed(dark)), Some(Color::Indexed(light))) =
            (Theme::Dark.dim().fg, Theme::Light.dim().fg)
        else {
            panic!("Dark/Light 必须各自钉一个 256 色表里的灰");
        };
        assert!(
            dark > light,
            "深色背景上的灰（{dark}）必须比浅色背景上的灰（{light}）更亮"
        );
    }
}
```

- [ ] **Step 2: 注册模块，跑测试确认编译失败**

在 `src/lib.rs` 里按字母序插一行（`session` 之后、`ui` 之前）：

```rust
pub mod theme;
```

Run: `export PATH="$HOME/.cargo/bin:$PATH" && cargo test --lib theme 2>&1 | head -20`
Expected: 编译错误，`cannot find type Theme in this scope`（或等价的未定义符号报错）。

- [ ] **Step 3: 写最小实现**

在 `src/theme.rs` 里，`use` 之后、`#[cfg(test)] mod tests` 之前插入：

```rust
/// 终端背景的深浅。`Unknown` 不是错误状态，是一个一等公民：
/// 探测不出来的终端照样要能正常显示，见 `dim()`。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Theme {
    Dark,
    Light,
    Unknown,
}

impl Theme {
    /// 弱化文字（说明栏、不可用项、操作提示）用的样式。
    ///
    /// `Dark`/`Light` 钉 256 色表里的固定灰：走的是 256 色索引而不是 16 色的
    /// 具名色，所以不经过终端主题对 0–15 号色的重定义，不会再被某个主题
    /// 映射成背景色。245 偏亮压在深底上，241 偏暗压在浅底上。
    ///
    /// `Unknown` 一个颜色都不指定，只挂 DIM 修饰符——理由见
    /// `unknown_never_pins_a_foreground_color` 测试上的注释。
    pub fn dim(self) -> Style {
        match self {
            Theme::Dark => Style::default().fg(Color::Indexed(245)),
            Theme::Light => Style::default().fg(Color::Indexed(241)),
            Theme::Unknown => Style::default().add_modifier(Modifier::DIM),
        }
    }
}
```

- [ ] **Step 4: 跑测试确认通过**

Run: `export PATH="$HOME/.cargo/bin:$PATH" && cargo test --lib theme`
Expected: 3 个测试全 PASS（`each_theme_has_a_distinct_dim_style`、`unknown_never_pins_a_foreground_color`、`dark_gets_a_lighter_gray_than_light`）。

Run: `export PATH="$HOME/.cargo/bin:$PATH" && cargo build 2>&1 | grep -c warning`
Expected: `0`。（`Theme` 此刻还没有生产调用点，但它是 `pub` 的且模块已注册，不会触发 dead_code 警告。若真报了警告，不要加 `#[allow(dead_code)]` 糊过去——Task 4 会接上真正的调用点；把警告留到那时消失。）

- [ ] **Step 5: 提交**

```bash
git add src/theme.rs src/lib.rs
git commit -m "feat: add Theme enum with background-adaptive dim styles

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>"
```

---

### Task 2: 三个纯解析函数

探测链上真正容易出错的地方全在这里：十六进制怎么算、终止符有两种、`COLORFGBG` 有两种字段数。全做成不碰环境变量、不碰 I/O 的纯函数，这样能拿真表格测到边界，Task 3 只负责按顺序把它们串起来。

**Files:**
- Modify: `src/theme.rs`

**Interfaces:**
- Consumes: Task 1 的 `Theme`
- Produces:
  - `pub(crate) fn is_light(r: u16, g: u16, b: u16) -> bool` —— 入参是 16 位通道值（0–65535）
  - `pub(crate) fn parse_osc11(bytes: &[u8]) -> Option<(u16, u16, u16)>`
  - `pub(crate) fn parse_colorfgbg(s: &str) -> Option<Theme>`
  - `pub(crate) fn theme_from_override(v: Option<&str>) -> Option<Theme>`

- [ ] **Step 1: 写失败的测试**

把下面这些测试**加进** `src/theme.rs` 已有的 `mod tests` 里（放在 Task 1 那三个测试之后，`mod tests` 的右花括号之前）：

```rust
    /// 亮度公式的边界。阈值取 0.5，两类真实背景离它都很远。
    #[test]
    fn luminance_separates_real_terminal_backgrounds() {
        // 纯黑 / 纯白
        assert!(!is_light(0, 0, 0));
        assert!(is_light(0xffff, 0xffff, 0xffff));

        // Solarized Dark 的 base03 #002b36，算出来约 0.14
        assert!(!is_light(0x0000, 0x2b2b, 0x3636));
        // Solarized Light 的 base3 #fdf6e3，约 0.97
        assert!(is_light(0xfdfd, 0xf6f6, 0xe3e3));

        // 中灰偏两侧：0x7fff 归一化约 0.5，是阈值本身；用它两边各一档
        assert!(!is_light(0x7000, 0x7000, 0x7000));
        assert!(is_light(0x9000, 0x9000, 0x9000));
    }

    /// 绿色权重最大（0.7152），所以纯绿要判成亮，纯蓝（0.0722）要判成暗。
    /// 这条防的是把三个通道权重写错位置。
    #[test]
    fn luminance_weights_are_not_transposed() {
        assert!(is_light(0, 0xffff, 0));
        assert!(!is_light(0, 0, 0xffff));
        assert!(!is_light(0xffff, 0, 0));
    }

    /// OSC 11 的回复：4 位十六进制是最常见的形式，终止符 BEL。
    #[test]
    fn parses_four_digit_osc11_reply() {
        let reply = b"\x1b]11;rgb:0000/2b2b/3636\x07";
        assert_eq!(parse_osc11(reply), Some((0x0000, 0x2b2b, 0x3636)));
    }

    /// ST（`ESC \`）终止和 BEL 终止都得认——两种终端都存在，
    /// 只认一种就会在另一半终端上白白降级。
    #[test]
    fn parses_st_terminated_reply() {
        let reply = b"\x1b]11;rgb:ffff/ffff/ffff\x1b\\";
        assert_eq!(parse_osc11(reply), Some((0xffff, 0xffff, 0xffff)));
    }

    /// 位数不足的要按比例放大到 16 位，不能左边填零。
    /// `rgb:0/0/0` 的 `f` 是满值，补成 `0x000f` 就成了几乎全黑，判反。
    #[test]
    fn scales_short_hex_components_to_full_range() {
        assert_eq!(parse_osc11(b"\x1b]11;rgb:f/f/f\x07"), Some((0xffff, 0xffff, 0xffff)));
        assert_eq!(parse_osc11(b"\x1b]11;rgb:ff/ff/ff\x07"), Some((0xffff, 0xffff, 0xffff)));
        assert_eq!(parse_osc11(b"\x1b]11;rgb:00/00/00\x07"), Some((0, 0, 0)));
        // 两位的 0x80 应该放大到约半程，而不是 0x0080
        let (r, _, _) = parse_osc11(b"\x1b]11;rgb:80/80/80\x07").unwrap();
        assert!(r > 0x8000 && r < 0x8100, "0x80 应放大到约半程，实际 {r:#06x}");
    }

    /// 各种残缺和垃圾输入一律 None，绝不 panic——这是探测链降级的入口，
    /// 这里 panic 就等于让界面起不来。
    #[test]
    fn rejects_malformed_osc11_replies() {
        assert_eq!(parse_osc11(b""), None);
        assert_eq!(parse_osc11(b"\x1b]11;rgb:0000/2b2b\x07"), None); // 少一个通道
        assert_eq!(parse_osc11(b"\x1b]11;rgb:zzzz/0000/0000\x07"), None); // 非十六进制
        assert_eq!(parse_osc11(b"\x1b]11;rgb:\x07"), None); // 空的
        assert_eq!(parse_osc11(b"\x1b]11;rgb:0000/2b2b/3636"), None); // 没有终止符
        assert_eq!(parse_osc11(b"garbage without any osc at all"), None);
        assert_eq!(parse_osc11(b"\x1b]11;0000/2b2b/3636\x07"), None); // 少 rgb: 前缀
        assert_eq!(parse_osc11(b"\x1b]11;rgb:00000/0000/0000\x07"), None); // 5 位，超范围
    }

    /// COLORFGBG 是 rxvt/urxvt/konsole 这些不答 OSC 11 的终端留下的线索。
    /// 取**最后**一段当背景色号：rxvt 有时给三段（前景;default;背景）。
    #[test]
    fn parses_colorfgbg() {
        assert_eq!(parse_colorfgbg("15;0"), Some(Theme::Dark));
        assert_eq!(parse_colorfgbg("0;15"), Some(Theme::Light));
        assert_eq!(parse_colorfgbg("15;default;0"), Some(Theme::Dark));
        assert_eq!(parse_colorfgbg("0;default;7"), Some(Theme::Light));
        // 8 是亮黑，仍然算深底
        assert_eq!(parse_colorfgbg("7;8"), Some(Theme::Dark));
    }

    /// 认不出来的一律 None，交给下一级降级，不要瞎猜成 Dark。
    #[test]
    fn rejects_malformed_colorfgbg() {
        assert_eq!(parse_colorfgbg(""), None);
        assert_eq!(parse_colorfgbg("15"), None); // 没有分号
        assert_eq!(parse_colorfgbg("15;default"), None); // 背景段不是数字
        assert_eq!(parse_colorfgbg("15;999"), None); // 超出 0–15
        assert_eq!(parse_colorfgbg("nonsense"), None);
    }

    /// 环境变量是探测猜错时的出口，要宽容：大小写和空格都不该让它失效——
    /// 会去设这个变量的人是在照着文档敲，不是在写代码。
    #[test]
    fn parses_theme_override_leniently() {
        assert_eq!(theme_from_override(Some("dark")), Some(Theme::Dark));
        assert_eq!(theme_from_override(Some("light")), Some(Theme::Light));
        assert_eq!(theme_from_override(Some("DARK")), Some(Theme::Dark));
        assert_eq!(theme_from_override(Some("  Light  ")), Some(Theme::Light));
    }

    /// 非法值必须当成「没设」往下降级，不能落成 Dark——
    /// 把 `DCT_THEME=lite` 这种拼错当成明确指定「深色」是错得最难查的一种。
    #[test]
    fn ignores_invalid_theme_override() {
        assert_eq!(theme_from_override(None), None);
        assert_eq!(theme_from_override(Some("")), None);
        assert_eq!(theme_from_override(Some("lite")), None);
        assert_eq!(theme_from_override(Some("auto")), None);
        assert_eq!(theme_from_override(Some("1")), None);
    }
```

- [ ] **Step 2: 跑测试确认失败**

Run: `export PATH="$HOME/.cargo/bin:$PATH" && cargo test --lib theme 2>&1 | head -20`
Expected: 编译错误，`cannot find function is_light`（以及 `parse_osc11` / `parse_colorfgbg` / `theme_from_override`）。

- [ ] **Step 3: 写最小实现**

在 `src/theme.rs` 里 `impl Theme { ... }` 之后、`#[cfg(test)] mod tests` 之前插入：

```rust
/// 判深浅用的加权亮度，阈值 0.5。
///
/// 故意**不做** sRGB 反伽马：判深浅只需要一个能把两类背景分得开的标量，
/// 不需要物理意义上的亮度。真实配色离阈值都很远（Solarized Dark 约 0.14，
/// Solarized Light 约 0.97），多三次 `powf` 换不来任何判断上的差别。
pub(crate) fn is_light(r: u16, g: u16, b: u16) -> bool {
    let norm = |v: u16| f64::from(v) / f64::from(u16::MAX);
    0.2126 * norm(r) + 0.7152 * norm(g) + 0.0722 * norm(b) > 0.5
}

/// 从 OSC 11 的回复里抠出背景色的三个通道，缩放到 16 位。
///
/// 回复长这样：`ESC ] 11 ; rgb:RRRR/GGGG/BBBB` 后跟 BEL 或 ST（`ESC \`）。
/// 每个通道是 1–4 位十六进制，位数由终端决定，两种都见得到。
///
/// 全程不 panic、不返回错误：这是探测链的一环，任何异常都只是「这一级没
/// 拿到答案」，由调用方降级到下一级。
pub(crate) fn parse_osc11(bytes: &[u8]) -> Option<(u16, u16, u16)> {
    let s = std::str::from_utf8(bytes).ok()?;

    // 只认带 `rgb:` 前缀的形式。有些终端理论上能回 `#RRGGBB`，但实测没遇到，
    // 不为一个没见过的格式写没法验证的解析分支——认不出来会降级，不会出错。
    let after = s.split_once("rgb:")?.1;

    // 终止符必须在：没有终止符说明这次读取被超时截断，拿到的是半个回复，
    // 按它算颜色就是拿残缺数据猜背景。
    let body = after
        .split_once('\x07')
        .or_else(|| after.split_once('\x1b'))
        .map(|(b, _)| b)?;

    let mut parts = body.split('/');
    let r = parse_hex_component(parts.next()?)?;
    let g = parse_hex_component(parts.next()?)?;
    let b = parse_hex_component(parts.next()?)?;
    // 多出第四段说明格式不对，宁可降级也不要猜
    if parts.next().is_some() {
        return None;
    }
    Some((r, g, b))
}

/// 一个 1–4 位十六进制的通道值，按比例放大到满量程 0–65535。
///
/// 必须按比例，不能左填零：`rgb:f/f/f` 里的 `f` 是该位数下的**满值**（白），
/// 补成 `0x000f` 就成了几乎全黑，深浅判断直接反过来。
fn parse_hex_component(s: &str) -> Option<u16> {
    if s.is_empty() || s.len() > 4 {
        return None;
    }
    let v = u32::from_str_radix(s, 16).ok()?;
    let max = 16u32.pow(s.len() as u32) - 1;
    Some((v * u32::from(u16::MAX) / max) as u16)
}

/// `COLORFGBG` 形如 `15;0`（前景;背景），rxvt 有时给三段
/// （`15;default;0`）。背景色号一律取**最后**一段。
///
/// 0–6 和 8 是深色，7 和 9–15 是浅色。超出 0–15 的（256 色场景）不猜，
/// 返回 None 让调用方降级。
pub(crate) fn parse_colorfgbg(s: &str) -> Option<Theme> {
    let bg = s.rsplit(';').next()?;
    // 只有一段说明没有分号，那不是这个变量该有的格式
    if !s.contains(';') {
        return None;
    }
    match bg.trim().parse::<u8>().ok()? {
        0..=6 | 8 => Some(Theme::Dark),
        7 | 9..=15 => Some(Theme::Light),
        _ => None,
    }
}

/// `DCT_THEME` 的取值。宽容处理大小写和首尾空格：会去设这个变量的人
/// 是在照文档敲，不是在写代码。
///
/// 认不出来的值返回 None（= 当成没设，继续往下探测），**不能**落成某个
/// 默认值——把 `DCT_THEME=lite` 这种拼错当成明确指定「深色」，是错得最
/// 难查的一种，用户会以为自己已经把颜色定死了。
pub(crate) fn theme_from_override(v: Option<&str>) -> Option<Theme> {
    match v?.trim().to_ascii_lowercase().as_str() {
        "dark" => Some(Theme::Dark),
        "light" => Some(Theme::Light),
        _ => None,
    }
}
```

- [ ] **Step 4: 跑测试确认通过**

Run: `export PATH="$HOME/.cargo/bin:$PATH" && cargo test --lib theme`
Expected: 12 个测试全 PASS（Task 1 的 3 个 + 这一轮的 9 个），0 failed。

Run: `export PATH="$HOME/.cargo/bin:$PATH" && cargo build 2>&1 | grep -A2 warning`
Expected: **会有** `dead_code` 警告，四个新函数各一条（`is_light` / `parse_osc11` / `parse_colorfgbg` / `theme_from_override`）。它们是 `pub(crate)` 且此刻只有测试在用，非测试构建里确实没人调。

**不要**为此加 `#[allow(dead_code)]`，也不要为了消警告把它们改成 `pub`：Task 3 的 `detect_with` 会调全部四个，警告到那时自然消失。这里唯一要确认的是**除了这四条 dead_code 之外没有别的警告**（尤其不能有 unused import 或类型警告）。

- [ ] **Step 5: 提交**

```bash
git add src/theme.rs
git commit -m "feat: add pure parsers for OSC 11, COLORFGBG, and DCT_THEME

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>"
```

---

### Task 3: 带超时的 stdin 读取与四级探测链

把 Task 2 的解析函数按优先级串成 `detect()`。读 stdin 用 `poll(2)` 做超时，藏在一个 trait 后面，好让探测链本身能在测试里跑完整四级降级而不碰真终端。

**Files:**
- Modify: `src/theme.rs`

**Interfaces:**
- Consumes: Task 1 的 `Theme`；Task 2 的 `is_light` / `parse_osc11` / `parse_colorfgbg` / `theme_from_override`
- Produces:
  - `pub(crate) trait ReplyReader { fn read_reply(&mut self, deadline: Duration) -> Vec<u8>; }`
  - `pub(crate) fn detect_with<R: ReplyReader>(reader: &mut R, dct_theme: Option<&str>, colorfgbg: Option<&str>) -> Theme`
  - `pub fn detect() -> Theme` —— 读真环境变量 + 真 stdin，`ui.rs` 用这个

- [ ] **Step 1: 写失败的测试**

在 `src/theme.rs` 的 `mod tests` 里追加（仍在同一个 `mod tests` 内，右花括号之前）：

```rust
    /// 测试用的假读端：按剧本返回一段预设回复，或者返回空（= 终端一声不响，
    /// 真实世界里就是读到超时）。
    struct CannedReader {
        reply: Vec<u8>,
        calls: usize,
    }

    impl CannedReader {
        fn answering(reply: &[u8]) -> Self {
            CannedReader { reply: reply.to_vec(), calls: 0 }
        }
        /// 不答 OSC 11 的终端，读到超时拿到空字节
        fn silent() -> Self {
            CannedReader { reply: Vec::new(), calls: 0 }
        }
    }

    impl ReplyReader for CannedReader {
        fn read_reply(&mut self, _deadline: Duration) -> Vec<u8> {
            self.calls += 1;
            self.reply.clone()
        }
    }

    /// 第一级：环境变量指定了就用它，而且**不去查询终端**——用户已经明确
    /// 说了答案，再花 150ms 去问一遍是白等。
    #[test]
    fn override_wins_and_skips_the_query() {
        let mut r = CannedReader::answering(b"\x1b]11;rgb:ffff/ffff/ffff\x07");
        assert_eq!(detect_with(&mut r, Some("dark"), None), Theme::Dark);
        assert_eq!(r.calls, 0, "环境变量已经给出答案，不该再查询终端");
    }

    /// 环境变量还要压过 COLORFGBG。
    #[test]
    fn override_wins_over_colorfgbg() {
        let mut r = CannedReader::silent();
        assert_eq!(detect_with(&mut r, Some("light"), Some("15;0")), Theme::Light);
    }

    /// 第二级：OSC 11 答了就用它的结果。
    #[test]
    fn uses_osc11_reply_when_terminal_answers() {
        let mut dark = CannedReader::answering(b"\x1b]11;rgb:0000/2b2b/3636\x07");
        assert_eq!(detect_with(&mut dark, None, None), Theme::Dark);

        let mut light = CannedReader::answering(b"\x1b]11;rgb:fdfd/f6f6/e3e3\x07");
        assert_eq!(detect_with(&mut light, None, None), Theme::Light);
    }

    /// OSC 11 还要压过 COLORFGBG：问到终端本人的答案比环境变量里的陈旧线索可信
    /// （COLORFGBG 是登录时设的，用户中途换了配色它不会更新）。
    #[test]
    fn osc11_wins_over_colorfgbg() {
        let mut r = CannedReader::answering(b"\x1b]11;rgb:fdfd/f6f6/e3e3\x07");
        assert_eq!(detect_with(&mut r, None, Some("15;0")), Theme::Light);
    }

    /// 第三级：终端不答（超时读到空）就退回 COLORFGBG。
    #[test]
    fn falls_back_to_colorfgbg_when_terminal_is_silent() {
        let mut r = CannedReader::silent();
        assert_eq!(detect_with(&mut r, None, Some("15;0")), Theme::Dark);
        assert_eq!(detect_with(&mut CannedReader::silent(), None, Some("0;15")), Theme::Light);
    }

    /// 回复格式不对，也要能一路降到 COLORFGBG，而不是就地放弃。
    #[test]
    fn falls_back_to_colorfgbg_when_reply_is_garbage() {
        let mut r = CannedReader::answering(b"\x1b]11;rgb:zz/zz/zz\x07");
        assert_eq!(detect_with(&mut r, None, Some("0;15")), Theme::Light);
    }

    /// 第四级：什么线索都没有就是 Unknown。这必须是一个正常出口，
    /// 不是错误——`Unknown.dim()` 本身就是能用的样式。
    #[test]
    fn unknown_when_nothing_answers() {
        let mut r = CannedReader::silent();
        assert_eq!(detect_with(&mut r, None, None), Theme::Unknown);
    }

    /// 三级全是垃圾输入的组合拳：一样只能落到 Unknown，不许 panic。
    #[test]
    fn garbage_at_every_level_lands_on_unknown() {
        let mut r = CannedReader::answering(b"not an osc reply");
        assert_eq!(detect_with(&mut r, Some("mauve"), Some("not;numbers")), Theme::Unknown);
    }
```

- [ ] **Step 2: 跑测试确认失败**

Run: `export PATH="$HOME/.cargo/bin:$PATH" && cargo test --lib theme 2>&1 | head -20`
Expected: 编译错误，`cannot find trait ReplyReader` / `cannot find function detect_with`。

- [ ] **Step 3: 写最小实现**

在 `src/theme.rs` 顶部的 `use` 块里补两行：

```rust
use std::io::{Read, Write};
use std::time::{Duration, Instant};
```

然后在 `theme_from_override` 之后、`#[cfg(test)] mod tests` 之前插入：

```rust
/// OSC 11 查询的最长等待。不答这个查询的终端只付一次性的 150ms 启动代价，
/// 而不是挂在那里等。本地终端的往返是亚毫秒级，150ms 绰绰有余；对用户来说
/// 也还在「启动」这个心理窗口里面。
const QUERY_TIMEOUT: Duration = Duration::from_millis(150);

/// 把「发查询、在 deadline 内读回复」抽出来，只为了让 `detect_with` 能在
/// 测试里跑完整的四级降级——真实实现要一个 tty 和一个会答话的终端，
/// 两样都不该是单元测试的前提。
pub(crate) trait ReplyReader {
    /// 返回读到的字节；什么都没读到（超时、不是 tty、读失败）就返回空 Vec。
    /// **不返回 Result**：调用方对所有失败的处理都一样——降级，
    /// 用错误类型区分它们只会诱导出没人需要的分支。
    fn read_reply(&mut self, deadline: Duration) -> Vec<u8>;
}

/// 真实实现：往 stdout 写 OSC 11 查询，用 `poll(2)` 在 deadline 内读 stdin。
///
/// 必须在 `enable_raw_mode()` 之后用：非 raw 模式下这段回复会被行缓冲
/// （它不带换行，读不出来）并且被回显到屏幕上（用户会看见一串乱码）。
pub(crate) struct StdinReader;

impl ReplyReader for StdinReader {
    fn read_reply(&mut self, deadline: Duration) -> Vec<u8> {
        let mut out = std::io::stdout();
        // 写失败（stdout 被重定向/关闭）就没有查询可言，直接空手而归
        if out.write_all(b"\x1b]11;?\x07").is_err() || out.flush().is_err() {
            return Vec::new();
        }

        let start = Instant::now();
        let mut buf = Vec::new();
        loop {
            let Some(left) = deadline.checked_sub(start.elapsed()) else {
                // 超时。buf 里可能有半个回复，照样交出去——`parse_osc11`
                // 要求终止符必须在，残缺的会被它判成 None。
                return buf;
            };

            if !stdin_is_readable(left) {
                return buf;
            }

            let mut chunk = [0u8; 64];
            match std::io::stdin().read(&mut chunk) {
                Ok(0) | Err(_) => return buf,
                Ok(n) => {
                    buf.extend_from_slice(&chunk[..n]);
                    // 收到终止符就够了，不等满 deadline
                    if buf.contains(&0x07) || buf.windows(2).any(|w| w == b"\x1b\\") {
                        return buf;
                    }
                    // 封顶：用户在界面出来之前狂敲键盘的话，这里会一直有
                    // 字节可读。读满就走，不能让探测卡在一个喂不完的输入上。
                    if buf.len() >= 256 {
                        return buf;
                    }
                }
            }
        }
    }
}

/// stdin 在 `timeout` 内是否可读。`poll(2)` 而不是起线程去阻塞读：
/// 那个线程超时后仍卡在 `read` 上，之后会跟事件循环抢 stdin，把用户的
/// 按键吃掉——一个只在「终端不答 OSC 11」时才发作的偷键 bug。
fn stdin_is_readable(timeout: Duration) -> bool {
    let mut fd = libc::pollfd {
        fd: libc::STDIN_FILENO,
        events: libc::POLLIN,
        revents: 0,
    };
    // 上取整到毫秒：截断成 0 会让 poll 变成非阻塞轮询，在极短的剩余时间里
    // 空转。毫秒级的多等对 150ms 的总预算无关紧要。
    let ms = timeout.as_millis().min(i32::MAX as u128) as i32;
    let ms = if ms == 0 { 1 } else { ms };
    // 失败（含被信号打断的 EINTR）当成「没数据」：调用方会因此降级，
    // 而重试要另写一套超时记账，为一个 150ms 的尽力而为的查询不值得。
    unsafe { libc::poll(&mut fd, 1, ms) > 0 }
}

/// 按优先级探测背景深浅。四级降级的顺序和理由见设计文档。
///
/// 环境变量和读端都从参数进来，所以这个函数是可测的、也是纯粹的调度逻辑：
/// 不碰进程环境（`set_var` 是进程级的，并行测试之间会互相踩），不碰真 stdin。
pub(crate) fn detect_with<R: ReplyReader>(
    reader: &mut R,
    dct_theme: Option<&str>,
    colorfgbg: Option<&str>,
) -> Theme {
    // 1. 用户明说了就照办，而且不再去查询终端——他已经给了答案。
    if let Some(t) = theme_from_override(dct_theme) {
        return t;
    }

    // 2. 问终端本人。比 COLORFGBG 可信：那个变量是登录时设的，用户中途
    //    换了配色它不会更新。
    if let Some((r, g, b)) = parse_osc11(&reader.read_reply(QUERY_TIMEOUT)) {
        return if is_light(r, g, b) { Theme::Light } else { Theme::Dark };
    }

    // 3. 不答 OSC 11 的终端（rxvt/urxvt/konsole）留下的线索。
    if let Some(t) = colorfgbg.and_then(parse_colorfgbg) {
        return t;
    }

    // 4. 没有任何线索。不是错误——`Unknown.dim()` 是能用的样式。
    Theme::Unknown
}

/// `detect_with` 的生产入口：接真环境变量和真 stdin。
///
/// 必须在 `enable_raw_mode()` 之后、`EnterAlternateScreen` 之前调，
/// 两头都是硬约束，理由见 `ui.rs` 里调用点的注释。
pub fn detect() -> Theme {
    let dct = std::env::var("DCT_THEME").ok();
    let fgbg = std::env::var("COLORFGBG").ok();
    detect_with(&mut StdinReader, dct.as_deref(), fgbg.as_deref())
}
```

- [ ] **Step 4: 跑测试确认通过**

Run: `export PATH="$HOME/.cargo/bin:$PATH" && cargo test --lib theme`
Expected: 20 个测试全 PASS（前两个 task 的 12 个 + 这一轮的 8 个），0 failed。

Run: `export PATH="$HOME/.cargo/bin:$PATH" && cargo build 2>&1 | grep warning; echo "--- warnings above (none expected)"`
Expected: 没有 warning 行。

- [ ] **Step 5: 提交**

```bash
git add src/theme.rs
git commit -m "feat: detect terminal background via OSC 11 with poll(2) timeout

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>"
```

---

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

### Task 5: 真终端上肉眼验收

单元测试证明不了「在 Solarized Dark 上看得见」——最初那个 bug 本身就是全绿的测试没拦住的。这一步必须真跑起来看。

**Files:** 无（只跑和看）

**Interfaces:**
- Consumes: Task 4 之后完整可用的 `dct`

- [ ] **Step 1: 确认探测在当前终端的判断**

Run:
```bash
export PATH="$HOME/.cargo/bin:$PATH"
cargo build 2>&1 | tail -2
printf '\033]11;?\007'; sleep 0.3; echo
```
Expected: 终端回一串形如 `]11;rgb:xxxx/xxxx/xxxx` 的文字（可能夹在提示符里）。看到了说明本机终端答 OSC 11，第二级探测会生效。看不到也不算失败——记下来，说明本机会走 `COLORFGBG` 或 `Unknown` 分支。

- [ ] **Step 2: 深色背景下看选 agent 菜单**

把终端配色设成深色（Solarized Dark 最能复现原始 bug），然后跑 `dct`，按 `N` 打开选 agent 菜单。

逐项确认：
- 九行 agent 全部可见，包括未安装/未填密钥的那几个
- 不可用行的「（未安装）」「（未填密钥）」读得清
- 每行的说明栏（`Anthropic 官方`、`深度求索，套用 Claude 界面` 等）读得清，且明显弱于 agent 名字
- `▶` 就在当前选中行的文字旁边，不是孤零零悬在空行上

- [ ] **Step 3: 浅色背景下看同一个菜单**

把终端配色切成浅色（Solarized Light 或 Terminal.app 的默认亮色），重跑 `dct`，再按 `N`。确认上面四条同样成立——尤其是说明栏没有淡到看不见。

- [ ] **Step 4: 验证环境变量出口两个方向都管用**

Run: `export PATH="$HOME/.cargo/bin:$PATH" && DCT_THEME=light cargo run 2>/dev/null`
Expected: 在**深色**终端里强制用浅色背景的灰（241），说明栏会明显偏暗、偏难读。看到这个「变难读」正是证明覆盖生效了。按 `Ctrl+Q` 退出。

Run: `export PATH="$HOME/.cargo/bin:$PATH" && DCT_THEME=dark cargo run 2>/dev/null`
Expected: 说明栏恢复正常可读。退出。

Run: `export PATH="$HOME/.cargo/bin:$PATH" && DCT_THEME=nonsense cargo run 2>/dev/null`
Expected: 非法值被忽略，退回自动探测，显示和不设这个变量时一致。退出。

- [ ] **Step 5: 验证不答 OSC 11 的终端不会拖慢启动**

模拟一个不答查询的终端：把 stdin 接到 `/dev/null`，探测读不到任何东西，只能等满 150ms。

Run: `export PATH="$HOME/.cargo/bin:$PATH" && cargo build 2>&1 | tail -1 && time ./target/debug/dct --help`
Expected: 总耗时远小于 1 秒。（`--help` 不进 `run()`、不做探测，这里量的是基线。）真正的 150ms 上限已经由 `QUERY_TIMEOUT` 保证，Step 2/3 里手动启动时若感觉不到明显卡顿即为通过。

- [ ] **Step 6: 提交验收记录**

把 Step 1 和 Step 2/3 的实际观察补到设计文档末尾（本机终端答不答 OSC 11、深浅两种背景下的实际效果），然后：

```bash
git add docs/superpowers/specs/2026-08-04-dct-adaptive-color-design.md
git commit -m "docs: record manual verification of adaptive colors

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>"
```

---

## 附：这个计划**不**做的事

写下来是为了防止实现时顺手扩大范围：

- 不做可配置调色板 / 命名主题 / TOML 里的颜色项。目标用户是零编程经验的人。
- 不重映射具名 ANSI 色（Red/Cyan/Yellow/Green），也不碰会话画面里 agent 输出的颜色。
- 不做运行中重新探测（用户中途换终端配色需要重启 `dct`）。加它要处理探测和事件循环抢 stdin，代价远大于收益。
- 不加 `termbg` / `terminal-light` 依赖。
- 不做 sRGB 反伽马。判深浅只要一个分得开的标量。
