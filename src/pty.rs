use anyhow::{Context, Result};
use portable_pty::{Child, CommandBuilder, MasterPty, NativePtySystem, PtySize, PtySystem};
use serde::{Deserialize, Serialize};
use std::io::{Read, Write};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

/// 终端颜色。跟 vt100 的表示一一对应，额外实现序列化好走协议。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum ScreenColor {
    #[default]
    Default,
    Idx(u8),
    Rgb(u8, u8, u8),
}

impl From<vt100::Color> for ScreenColor {
    fn from(c: vt100::Color) -> Self {
        match c {
            vt100::Color::Default => ScreenColor::Default,
            vt100::Color::Idx(i) => ScreenColor::Idx(i),
            vt100::Color::Rgb(r, g, b) => ScreenColor::Rgb(r, g, b),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct ScreenStyle {
    pub fg: ScreenColor,
    pub bg: ScreenColor,
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
    pub inverse: bool,
}

/// 一段样式相同的连续文字。按样式做游程合并，这样一屏通常只有几十个片段，
/// 而不是几千个 cell —— 走协议的开销才不会失控。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScreenSpan {
    pub text: String,
    pub style: ScreenStyle,
}

pub struct PtySession {
    parser: Arc<Mutex<vt100::Parser>>,
    writer: Arc<Mutex<Box<dyn Write + Send>>>,
    child: Arc<Mutex<Box<dyn Child + Send + Sync>>>,
    alive: Arc<AtomicBool>,
    _master: Box<dyn MasterPty + Send>,
}

impl PtySession {
    pub fn spawn(cmd: &[String], cwd: &Path, rows: u16, cols: u16) -> Result<PtySession> {
        anyhow::ensure!(!cmd.is_empty(), "启动命令为空");

        let pty = NativePtySystem::default()
            .openpty(PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .context("打开 PTY 失败")?;

        let mut builder = CommandBuilder::new(&cmd[0]);
        for a in &cmd[1..] {
            builder.arg(a);
        }
        builder.cwd(cwd);

        let child = pty
            .slave
            .spawn_command(builder)
            .with_context(|| format!("启动 {} 失败", cmd[0]))?;

        let parser = Arc::new(Mutex::new(vt100::Parser::new(rows, cols, 0)));
        let writer = Arc::new(Mutex::new(pty.master.take_writer()?));
        let alive = Arc::new(AtomicBool::new(true));

        let mut reader = pty.master.try_clone_reader()?;
        let rp = parser.clone();
        let ra = alive.clone();
        std::thread::spawn(move || {
            let mut buf = [0u8; 8192];
            loop {
                match reader.read(&mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => rp
                        .lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .process(&buf[..n]),
                }
            }
            ra.store(false, Ordering::SeqCst);
        });

        Ok(PtySession {
            parser,
            writer,
            child: Arc::new(Mutex::new(child)),
            alive,
            _master: pty.master,
        })
    }

    pub fn write(&self, data: &[u8]) -> Result<()> {
        let mut w = self.writer.lock().unwrap_or_else(|e| e.into_inner());
        w.write_all(data)?;
        w.flush()?;
        Ok(())
    }

    /// agent 屏幕里光标所在的 (行, 列)，0 起算。没有它 TUI 只能显示一张死截图，
    /// 用户看不出自己打的字会落在哪。
    pub fn cursor(&self) -> (u16, u16) {
        self.parser
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .screen()
            .cursor_position()
    }

    /// 带颜色和粗体等属性的整屏内容，一行一个 `Vec<ScreenSpan>`。
    /// 只传纯文本的话 agent 界面会变成单色，Claude Code 那种靠颜色区分的
    /// 输出基本没法看。
    pub fn screen_spans(&self) -> Vec<Vec<ScreenSpan>> {
        let parser = self.parser.lock().unwrap_or_else(|e| e.into_inner());
        let screen = parser.screen();
        let (rows, cols) = screen.size();

        (0..rows)
            .map(|r| {
                let mut line: Vec<ScreenSpan> = Vec::new();
                for c in 0..cols {
                    let Some(cell) = screen.cell(r, c) else {
                        continue;
                    };
                    // 宽字符占两格，第二格是延续位，再输出一次会把整行推歪
                    if cell.is_wide_continuation() {
                        continue;
                    }
                    let text = cell.contents();
                    let text = if text.is_empty() {
                        " ".to_string()
                    } else {
                        text
                    };
                    let style = ScreenStyle {
                        fg: cell.fgcolor().into(),
                        bg: cell.bgcolor().into(),
                        bold: cell.bold(),
                        italic: cell.italic(),
                        underline: cell.underline(),
                        inverse: cell.inverse(),
                    };
                    match line.last_mut() {
                        Some(last) if last.style == style => last.text.push_str(&text),
                        _ => line.push(ScreenSpan { text, style }),
                    }
                }
                line
            })
            .collect()
    }

    /// 屏幕上最后一行有内容的文字。看板靠它显示"这个 agent 此刻在干什么"——
    /// 只传一行，比整屏便宜得多，扫一眼全局时不需要完整画面。
    pub fn last_line(&self) -> String {
        let parser = self.parser.lock().unwrap_or_else(|e| e.into_inner());
        let screen = parser.screen();
        let (rows, _) = screen.size();
        (0..rows)
            .rev()
            .map(|r| screen.contents_between(r, 0, r + 1, 0))
            .find(|line| !line.trim().is_empty())
            .map(|line| line.trim().to_string())
            .unwrap_or_default()
    }

    pub fn screen_text(&self) -> String {
        self.parser
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .screen()
            .contents()
    }

    pub fn is_alive(&self) -> bool {
        if !self.alive.load(Ordering::SeqCst) {
            return false;
        }
        match self
            .child
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .try_wait()
        {
            Ok(Some(_)) => {
                self.alive.store(false, Ordering::SeqCst);
                false
            }
            Ok(None) => true,
            Err(_) => {
                // 子进程已经被回收（比如已经 wait 过一次），try_wait 会报错
                // 而不是返回 Ok(Some(_))：这种情况也必须判定为已死，不能默认存活。
                self.alive.store(false, Ordering::SeqCst);
                false
            }
        }
    }

    pub fn kill(&mut self) -> Result<()> {
        let mut child = self.child.lock().unwrap_or_else(|e| e.into_inner());
        // portable-pty 在 unix 上的 kill() 先发 SIGHUP 并给约 200ms 宽限期
        // 自行退出被回收；超时后退化为 SIGKILL，这条路径不会再 wait()。
        // 因此这里必须显式 wait 一次，否则子进程会变成僵尸。
        let _ = child.kill();
        let _ = child.wait();
        drop(child);
        self.alive.store(false, Ordering::SeqCst);
        Ok(())
    }

    pub fn process_id(&self) -> Option<u32> {
        self.child
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .process_id()
    }
}

impl Drop for PtySession {
    fn drop(&mut self) {
        // 会话被丢弃时必须回收子进程，否则常驻的守护进程每关一个
        // 会话就会留一个僵尸，直到进程重启才被清空。Drop 里不能 panic，
        // 所有错误都吞掉。
        let _ = self.kill();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::thread::sleep;
    use std::time::{Duration, Instant};

    fn wait_for(p: &PtySession, needle: &str) -> bool {
        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline {
            if p.screen_text().contains(needle) {
                return true;
            }
            sleep(Duration::from_millis(50));
        }
        false
    }

    #[test]
    fn captures_command_output() {
        let dir = tempfile::tempdir().unwrap();
        let p = PtySession::spawn(
            &["echo".to_string(), "hello-dct".to_string()],
            dir.path(),
            24,
            80,
        )
        .unwrap();
        assert!(wait_for(&p, "hello-dct"));
    }

    #[test]
    fn writes_input_to_process() {
        let dir = tempfile::tempdir().unwrap();
        let p = PtySession::spawn(&["cat".to_string()], dir.path(), 24, 80).unwrap();
        p.write(b"ping-dct\n").unwrap();
        assert!(wait_for(&p, "ping-dct"));
    }

    #[test]
    fn reports_death() {
        let dir = tempfile::tempdir().unwrap();
        let p = PtySession::spawn(&["true".to_string()], dir.path(), 24, 80).unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline && p.is_alive() {
            sleep(Duration::from_millis(50));
        }
        assert!(!p.is_alive());
    }

    #[test]
    fn drop_reaps_child_process() {
        let dir = tempfile::tempdir().unwrap();
        let pid = {
            let p = PtySession::spawn(&["cat".to_string()], dir.path(), 24, 80).unwrap();
            p.write(b"alive\n").unwrap();
            assert!(wait_for(&p, "alive"));
            p.process_id().expect("需要拿到子进程 pid")
        }; // 这里 drop

        // 给 Drop 一点时间完成回收
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let out = std::process::Command::new("ps")
                .args(["-o", "stat=", "-p", &pid.to_string()])
                .output()
                .unwrap();
            let stat = String::from_utf8_lossy(&out.stdout).trim().to_string();
            // 进程完全消失（ps 无输出）才算回收干净；Z 开头是僵尸
            if stat.is_empty() {
                break;
            }
            assert!(!stat.starts_with('Z'), "drop 之后子进程是僵尸: {stat}");
            assert!(
                Instant::now() < deadline,
                "drop 之后子进程没有被回收: {stat}"
            );
            sleep(Duration::from_millis(50));
        }
    }
}
