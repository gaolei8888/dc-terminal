//! 真字节走真解码路径：往伪终端写转义序列，用 crossterm 的 `event::read` 读出来。
//! 不构造 KeyEvent——要验证的恰恰是「字节 -> KeyEvent」这一步（xterm 的
//! Shift+F3 是 `ESC[1;2R`，上游 crossterm 把它当光标位置回复丢掉，见 vendor/README.md）。
//!
//! 整个进程只能有一个 crossterm 事件源，所以只有一个 #[test]。
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyModifiers};
use std::io::Write;
use std::os::fd::FromRawFd;
use std::time::Duration;

fn next(timeout_ms: u64) -> Option<Event> {
    event::poll(Duration::from_millis(timeout_ms))
        .unwrap()
        .then(|| event::read().unwrap())
}

fn key(code: KeyCode, m: KeyModifiers) -> Option<Event> {
    Some(Event::Key(KeyEvent::new(code, m)))
}

#[test]
fn escape_sequences_decode_through_the_real_path() {
    let (mut master, slave) = unsafe {
        let (mut m, mut s) = (0, 0);
        assert_eq!(
            libc::openpty(&mut m, &mut s, std::ptr::null_mut(), std::ptr::null_mut(), std::ptr::null_mut()),
            0
        );
        (std::fs::File::from_raw_fd(m), s)
    };
    unsafe { assert!(libc::dup2(slave, 0) == 0) };
    crossterm::terminal::enable_raw_mode().unwrap();

    use KeyCode::F;
    let (none, shift, alt, ctrl) = (
        KeyModifiers::NONE,
        KeyModifiers::SHIFT,
        KeyModifiers::ALT,
        KeyModifiers::CONTROL,
    );
    // (字节, 期望的唯一一个事件)
    let table: &[(&[u8], KeyEvent)] = &[
        (b"\x1b[1;2R", KeyEvent::new(F(3), shift)),
        (b"\x1b[1;3R", KeyEvent::new(F(3), alt)),
        (b"\x1b[1;4R", KeyEvent::new(F(3), shift | alt)),
        (b"\x1b[1;5R", KeyEvent::new(F(3), ctrl)),
        (b"\x1b[1;6R", KeyEvent::new(F(3), ctrl | shift)),
        (b"\x1b[1;7R", KeyEvent::new(F(3), ctrl | alt)),
        (b"\x1b[1;8R", KeyEvent::new(F(3), ctrl | alt | shift)),
        (b"\x1bOR", KeyEvent::new(F(3), none)),
        (b"\x1b[13~", KeyEvent::new(F(3), none)),
        (b"\x1b[13;2~", KeyEvent::new(F(3), shift)),
        (b"\x1b[13;5~", KeyEvent::new(F(3), ctrl)),
        (b"\x1b[19~", KeyEvent::new(F(8), none)),
    ];
    for (bytes, want) in table {
        master.write_all(bytes).unwrap();
        assert_eq!(next(500), Some(Event::Key(*want)), "bytes {bytes:?}");
        assert_eq!(next(50), None, "extra event after {bytes:?}");
    }

    // 真正的光标位置回复照旧不是按键：行 > 1，或者列在 2..=8 之外。
    for reply in [&b"\x1b[12;40R"[..], b"\x1b[1;1R", b"\x1b[1;9R", b"\x1b[24;2R"] {
        master.write_all(reply).unwrap();
        master.write_all(b"x").unwrap();
        assert_eq!(next(500), key(KeyCode::Char('x'), none), "reply {reply:?}");
        assert_eq!(next(50), None);
    }

    // 序列被拆成两次 read 也照样认。
    master.write_all(b"\x1b[1;").unwrap();
    std::thread::sleep(Duration::from_millis(100));
    master.write_all(b"2R").unwrap();
    assert_eq!(next(500), key(F(3), shift));

    // 粘贴、鼠标、焦点夹着它：别的事件不受影响，粘贴里的字节原样留在文本里。
    master
        .write_all(b"\x1b[<0;5;1M\x1b[1;5R\x1b[I\x1b[200~a\x1b[1;2Rb\x1b[201~\x1b[1;2R")
        .unwrap();
    assert!(matches!(next(500), Some(Event::Mouse(_))));
    assert_eq!(next(500), key(F(3), ctrl));
    assert_eq!(next(500), Some(Event::FocusGained));
    assert_eq!(next(500), Some(Event::Paste("a\x1b[1;2Rb".into())));
    assert_eq!(next(500), key(F(3), shift));
    assert_eq!(next(50), None);

    crossterm::terminal::disable_raw_mode().unwrap();
}
