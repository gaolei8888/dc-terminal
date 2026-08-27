//! 网页上那一排虚拟键 → 送进 PTY 的字节。
//!
//! **这里不写第二张映射表。** 名字先翻成一个 crossterm 的 `KeyEvent`，再交给
//! `ui::key_to_input`——桌面端每一次按键走的就是那个函数。
//!
//! 为什么这一条是硬要求：两份表漂了的症状是「手机上按方向键，agent 收到别的
//! 东西」，而那种 bug 在桌面端永远看不见，在手机上也不会报错——只是 agent
//! 的行为莫名其妙。而且这类漂移几乎必然发生：`key_to_input` 里的
//! `Ctrl+A..Z → 0x01..0x1a`、`Backspace → 0x7f`（不是 `\x08`）、
//! `BackTab → CSI Z` 这些细节，没有人会在改动的时候想起来"手机那边还有一份"。
//!
//! 所以这个模块只做一件事：**把一个名字翻成一次按键**。字节长什么样，
//! 从头到尾只有 `key_to_input` 说得算。

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

/// 网页允许送上来的键名。
///
/// 这是一张**白名单**，不是「能翻就翻」：网页那边送什么字符串上来是用户
/// 那一侧决定的，而这一路的终点是往用户自己的终端里敲字节。名字对不上就
/// 拒掉，比"尽量猜一个"安全，也比让 `key_to_input` 去兜底清楚——那个函数
/// 的输入本来是真实键盘事件，不是一段可能来自任何人的文本。
pub const NAMES: &[&str] = &[
    "Enter",
    "Esc",
    "Tab",
    "BackTab",
    "Backspace",
    "Up",
    "Down",
    "Left",
    "Right",
    "Home",
    "End",
    "PageUp",
    "PageDown",
    "Delete",
];

/// 一个键名对应的字节。认不出来返回 `None`（调用方拒掉这次请求）。
///
/// `Ctrl+X` 单独认一档：写死 `Ctrl+C` 一个的话，`Ctrl+D`（结束输入）、
/// `Ctrl+U`（清行）这些在手机上就永远按不出来，而它们跟 `Ctrl+C` 是同一
/// 类东西——用户不会理解为什么只有一个能按。
pub fn bytes_for(name: &str) -> Option<String> {
    crate::ui::key_to_input(&event_for(name)?)
}

fn event_for(name: &str) -> Option<KeyEvent> {
    if let Some(rest) = name.strip_prefix("Ctrl+") {
        // 只认单个 ASCII 字母。别的（`Ctrl+F5`、`Ctrl+`）交给 `key_to_input`
        // 也是 `None`，但在这儿就拦掉，理由同 `NAMES` 那段：这一路的输入
        // 不可信，能在早一点拒绝就早一点。
        let mut chars = rest.chars();
        let c = chars.next()?;
        if chars.next().is_some() || !c.is_ascii_alphabetic() {
            return None;
        }
        return Some(KeyEvent::new(
            KeyCode::Char(c.to_ascii_lowercase()),
            KeyModifiers::CONTROL,
        ));
    }
    let code = match name {
        "Enter" => KeyCode::Enter,
        "Esc" => KeyCode::Esc,
        "Tab" => KeyCode::Tab,
        "BackTab" => KeyCode::BackTab,
        "Backspace" => KeyCode::Backspace,
        "Up" => KeyCode::Up,
        "Down" => KeyCode::Down,
        "Left" => KeyCode::Left,
        "Right" => KeyCode::Right,
        "Home" => KeyCode::Home,
        "End" => KeyCode::End,
        "PageUp" => KeyCode::PageUp,
        "PageDown" => KeyCode::PageDown,
        "Delete" => KeyCode::Delete,
        _ => return None,
    };
    Some(KeyEvent::new(code, KeyModifiers::NONE))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 白名单上的每一个名字都得真的翻得出字节来。
    #[test]
    fn every_allowed_name_maps_to_something() {
        for name in NAMES {
            assert!(bytes_for(name).is_some(), "{name} 翻不出字节");
        }
    }

    /// **手机上按下去的键，跟桌面上按下同一个键，送进 PTY 的必须是同一串
    /// 字节。** 这条不是"顺手比一下"——两份表漂了的症状是手机上按方向键
    /// agent 收到别的东西，而那在桌面端永远看不见。
    #[test]
    fn a_key_from_the_phone_is_the_same_key_as_from_the_keyboard() {
        let same = |name: &str, code: KeyCode| {
            let desktop = crate::ui::key_to_input(&KeyEvent::new(code, KeyModifiers::NONE));
            assert_eq!(bytes_for(name), desktop, "{name} 两边不一样");
        };
        same("Enter", KeyCode::Enter);
        same("Esc", KeyCode::Esc);
        same("Tab", KeyCode::Tab);
        same("BackTab", KeyCode::BackTab);
        same("Backspace", KeyCode::Backspace);
        same("Up", KeyCode::Up);
        same("Down", KeyCode::Down);
        same("Left", KeyCode::Left);
        same("Right", KeyCode::Right);
        same("Home", KeyCode::Home);
        same("End", KeyCode::End);
        same("PageUp", KeyCode::PageUp);
        same("PageDown", KeyCode::PageDown);
        same("Delete", KeyCode::Delete);

        for c in ['c', 'd', 'u', 'z'] {
            let desktop =
                crate::ui::key_to_input(&KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL));
            assert_eq!(
                bytes_for(&format!("Ctrl+{}", c.to_ascii_uppercase())),
                desktop,
                "Ctrl+{c} 两边不一样"
            );
        }
    }

    /// 几个**具体**的字节，写死在这里。
    ///
    /// 上面那条比的是「两边一样」，可两边一起错了它照样绿。这一条钉的是
    /// 「对」：`Esc` 是 0x1b，方向键是 CSI 序列，`Backspace` 是 0x7f
    /// （**不是** 0x08——agent 那头认的是前者），`Ctrl+C` 是 0x03。
    #[test]
    fn the_bytes_are_the_ones_a_terminal_actually_sends() {
        assert_eq!(bytes_for("Esc").as_deref(), Some("\x1b"));
        assert_eq!(bytes_for("Up").as_deref(), Some("\x1b[A"));
        assert_eq!(bytes_for("Down").as_deref(), Some("\x1b[B"));
        assert_eq!(bytes_for("Right").as_deref(), Some("\x1b[C"));
        assert_eq!(bytes_for("Left").as_deref(), Some("\x1b[D"));
        assert_eq!(bytes_for("Tab").as_deref(), Some("\t"));
        assert_eq!(bytes_for("Backspace").as_deref(), Some("\x7f"));
        assert_eq!(bytes_for("Ctrl+C").as_deref(), Some("\x03"));
        assert_eq!(bytes_for("Ctrl+D").as_deref(), Some("\x04"));
    }

    /// **`Enter` 是空串，不是 `\n`。**
    ///
    /// 这是仓库里那条两步约定的一半（见 `SessionWriter::type_into` 和
    /// `ui::grid::send_reply`）：先把文字写进去，再单独送一次空字符串当回车，
    /// 而**空的那一次才会打检查点**、才会让 agent 真的开始干这一轮。
    /// 翻成 `\n` 的话，手机上发出去的每一句话都没有检查点——`u` 撤不回来，
    /// 而那是 dct 敢让 agent 关掉所有确认的全部理由。
    #[test]
    fn enter_is_the_empty_string_because_that_is_what_takes_the_checkpoint() {
        assert_eq!(bytes_for("Enter").as_deref(), Some(""));
    }

    /// 白名单之外一律拒。网页那边送什么上来是用户那一侧决定的，
    /// 而这一路的终点是往用户自己的终端里敲字节。
    #[test]
    fn anything_not_on_the_list_is_refused() {
        for name in [
            "", "F2", "Ctrl+", "Ctrl+F5", "Ctrl+ab", "ctrl+c", "Char(a)", "Enter\n", "Ctrl+1",
            "\x1b[A",
        ] {
            assert!(bytes_for(name).is_none(), "{name:?} 不该被接受");
        }
    }
}
