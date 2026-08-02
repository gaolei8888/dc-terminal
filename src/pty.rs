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
        let mut child = self.child.lock().unwrap();
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
        self.child.lock().unwrap().process_id()
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
