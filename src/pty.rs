use anyhow::{Context, Result};
use portable_pty::{Child, CommandBuilder, MasterPty, NativePtySystem, PtySize, PtySystem};
use std::io::{Read, Write};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

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
                    Ok(n) => rp.lock().unwrap().process(&buf[..n]),
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
        let mut w = self.writer.lock().unwrap();
        w.write_all(data)?;
        w.flush()?;
        Ok(())
    }

    pub fn screen_text(&self) -> String {
        self.parser.lock().unwrap().screen().contents()
    }

    pub fn is_alive(&self) -> bool {
        if !self.alive.load(Ordering::SeqCst) {
            return false;
        }
        match self.child.lock().unwrap().try_wait() {
            Ok(Some(_)) => {
                self.alive.store(false, Ordering::SeqCst);
                false
            }
            _ => true,
        }
    }

    pub fn kill(&mut self) -> Result<()> {
        self.child.lock().unwrap().kill().ok();
        self.alive.store(false, Ordering::SeqCst);
        Ok(())
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
}
