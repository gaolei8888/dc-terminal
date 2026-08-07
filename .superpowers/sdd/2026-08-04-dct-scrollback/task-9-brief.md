### Task 9: 鼠标事件编码

**Files:**
- Modify: `src/pty.rs`（`write_mouse` 与纯函数 `encode_mouse`）
- Test: `src/pty.rs` 的 `#[cfg(test)] mod tests`

**Interfaces:**
- Consumes: `crate::proto::{MouseForward, MouseForwardKind}`（Task 8）
- Produces:

```rust
impl PtySession {
    pub fn write_mouse(&self, ev: MouseForward) -> Result<()>;
}

/// 纯函数，好测。`None` 表示这个 agent 现在不收鼠标，什么都别发。
pub fn encode_mouse(
    mode: vt100::MouseProtocolMode,
    enc: vt100::MouseProtocolEncoding,
    ev: &MouseForward,
) -> Option<Vec<u8>>;
```

**SGR 编码（`?1006`，Claude Code 用的就是这个）：**
`ESC [ < <按钮码> ; <列+1> ; <行+1> M`（按下/滚轮）或 `m`（抬起）。
按钮码：左 0、中 1、右 2；滚轮上 64、滚轮下 65。
修饰键往按钮码上加：Shift +4、Alt +8、Ctrl +16。

**默认编码（`?1000` 不带 `?1006`）：**
`ESC [ M <32+按钮码> <32+列+1> <32+行+1>`，三个都是单字节。
列或行超过 223 就编不下，这种情况**不发**（返回 `None`）——
发一个截断的坐标比不发更糟，agent 会以为你点在别的地方。

- [ ] **Step 1: 写失败的测试**

```rust
    use crate::proto::{MouseForward, MouseForwardKind};

    fn ev(kind: MouseForwardKind, col: u16, row: u16) -> MouseForward {
        MouseForward {
            col,
            row,
            kind,
            shift: false,
            alt: false,
            ctrl: false,
        }
    }

    #[test]
    fn sgr_encodes_a_wheel_scroll() {
        let out = encode_mouse(
            vt100::MouseProtocolMode::AnyMotion,
            vt100::MouseProtocolEncoding::Sgr,
            &ev(MouseForwardKind::WheelUp, 10, 20),
        )
        .unwrap();
        // 坐标是 1 起算的，所以 10,20 变成 11,21
        assert_eq!(out, b"\x1b[<64;11;21M".to_vec());
    }

    #[test]
    fn sgr_wheel_down_uses_a_different_button_code() {
        let out = encode_mouse(
            vt100::MouseProtocolMode::AnyMotion,
            vt100::MouseProtocolEncoding::Sgr,
            &ev(MouseForwardKind::WheelDown, 0, 0),
        )
        .unwrap();
        assert_eq!(out, b"\x1b[<65;1;1M".to_vec());
    }

    #[test]
    fn sgr_marks_release_with_a_lowercase_m() {
        let out = encode_mouse(
            vt100::MouseProtocolMode::PressRelease,
            vt100::MouseProtocolEncoding::Sgr,
            &ev(MouseForwardKind::Release(0), 4, 5),
        )
        .unwrap();
        assert_eq!(out, b"\x1b[<0;5;6m".to_vec());
    }

    #[test]
    fn modifiers_are_added_to_the_button_code() {
        let mut e = ev(MouseForwardKind::Press(0), 0, 0);
        e.shift = true;
        e.ctrl = true;
        let out = encode_mouse(
            vt100::MouseProtocolMode::PressRelease,
            vt100::MouseProtocolEncoding::Sgr,
            &e,
        )
        .unwrap();
        // 0 + 4(shift) + 16(ctrl) = 20
        assert_eq!(out, b"\x1b[<20;1;1M".to_vec());
    }

    #[test]
    fn default_encoding_uses_the_single_byte_form() {
        let out = encode_mouse(
            vt100::MouseProtocolMode::PressRelease,
            vt100::MouseProtocolEncoding::Default,
            &ev(MouseForwardKind::Press(0), 10, 20),
        )
        .unwrap();
        // 32+0, 32+11, 32+21
        assert_eq!(out, vec![0x1b, b'[', b'M', 32, 43, 53]);
    }

    #[test]
    fn default_encoding_refuses_coordinates_it_cannot_express() {
        // 单字节形式最多到 223；发一个截断的坐标会让 agent 以为你点在别处
        assert!(encode_mouse(
            vt100::MouseProtocolMode::PressRelease,
            vt100::MouseProtocolEncoding::Default,
            &ev(MouseForwardKind::Press(0), 300, 5),
        )
        .is_none());
    }

    #[test]
    fn nothing_is_sent_when_the_agent_does_not_want_the_mouse() {
        assert!(encode_mouse(
            vt100::MouseProtocolMode::None,
            vt100::MouseProtocolEncoding::Sgr,
            &ev(MouseForwardKind::WheelUp, 1, 1),
        )
        .is_none());
    }

    /// X10（`?1000` 不带 release）只上报按下。发一个抬起事件过去，
    /// agent 会收到一个它没订阅的东西。
    #[test]
    fn x10_mode_drops_release_events() {
        assert!(encode_mouse(
            vt100::MouseProtocolMode::Press,
            vt100::MouseProtocolEncoding::Sgr,
            &ev(MouseForwardKind::Release(0), 1, 1),
        )
        .is_none());
    }
```

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test --lib pty:: -- --test-threads=1`
Expected: 编译失败，`cannot find function 'encode_mouse'`。

- [ ] **Step 3: 实现**

`src/pty.rs`：

```rust
/// 把一个鼠标事件编码成 agent 当前订阅的那种格式。
///
/// `None` 表示「什么都别发」，有三种情况：agent 根本没开鼠标上报；
/// 它只订阅了按下（X10）而这是个抬起；坐标大到默认编码装不下。
/// 三种都是「发出去比不发更糟」——agent 会收到它没订阅的东西，
/// 或者一个指向别处的坐标。
pub fn encode_mouse(
    mode: vt100::MouseProtocolMode,
    enc: vt100::MouseProtocolEncoding,
    ev: &crate::proto::MouseForward,
) -> Option<Vec<u8>> {
    use crate::proto::MouseForwardKind as K;
    use vt100::MouseProtocolMode as M;

    if mode == M::None {
        return None;
    }
    let is_release = matches!(ev.kind, K::Release(_));
    if is_release && mode == M::Press {
        return None;
    }

    let mut button = match ev.kind {
        K::WheelUp => 64,
        K::WheelDown => 65,
        K::Press(b) | K::Release(b) => u32::from(b),
    };
    if ev.shift {
        button += 4;
    }
    if ev.alt {
        button += 8;
    }
    if ev.ctrl {
        button += 16;
    }

    // 终端协议的坐标是 1 起算的，我们内部是 0 起算的
    let col = u32::from(ev.col) + 1;
    let row = u32::from(ev.row) + 1;

    match enc {
        vt100::MouseProtocolEncoding::Sgr => {
            let end = if is_release { 'm' } else { 'M' };
            Some(format!("\x1b[<{button};{col};{row}{end}").into_bytes())
        }
        // Utf8 跟 Default 的差别只在坐标怎么编码。按下界处理成
        // 「装不下就不发」对两者都成立，也就不用分开写。
        _ => {
            let b = 32u32.checked_add(button)?;
            let c = 32u32.checked_add(col)?;
            let r = 32u32.checked_add(row)?;
            if b > 255 || c > 255 || r > 255 {
                return None;
            }
            Some(vec![0x1b, b'[', b'M', b as u8, c as u8, r as u8])
        }
    }
}
```

`PtySession` 加：

```rust
    /// 把鼠标事件按 agent 当前的模式写进 PTY。它不收鼠标就什么都不做——
    /// 这是正常情况，不是错误。
    pub fn write_mouse(&self, ev: crate::proto::MouseForward) -> Result<()> {
        let bytes = {
            let parser = self.parser.lock().unwrap_or_else(|e| e.into_inner());
            let screen = parser.screen();
            encode_mouse(
                screen.mouse_protocol_mode(),
                screen.mouse_protocol_encoding(),
                &ev,
            )
        };
        match bytes {
            Some(b) => self.write(&b),
            None => Ok(()),
        }
    }
```

> 锁在 `encode_mouse` 之后就放掉了（那个块结束），`self.write()` 拿的是另一把锁。
> 同时握两把是死锁的开始，这个仓库在 `create()` 上已经吃过一次「持锁做慢操作」
> 的亏，别在这里重犯。

- [ ] **Step 4: 跑测试确认通过**

Run: `cargo test -- --test-threads=1`
Expected: 205 个，全绿。

- [ ] **Step 5: 格式与静态检查**

Run: `cargo fmt && cargo clippy --all-targets -- -D warnings`

- [ ] **Step 6: 提交**

```bash
git add src/pty.rs
git commit -m "feat: 鼠标事件按 agent 订阅的编码转发

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>"
```

---

