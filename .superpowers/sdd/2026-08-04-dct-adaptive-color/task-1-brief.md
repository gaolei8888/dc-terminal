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

