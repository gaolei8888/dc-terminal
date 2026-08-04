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

