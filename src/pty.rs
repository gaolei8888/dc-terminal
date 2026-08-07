use anyhow::Result;
use portable_pty::{Child, CommandBuilder, MasterPty, NativePtySystem, PtySize, PtySystem};
use serde::{Deserialize, Serialize};
use std::io::{Read, Write};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

/// 起 agent 之前必须从继承来的环境里摘掉的标记。
///
/// 守护进程常常是从某个 agent 自己的会话里被拉起来的（用户在 Claude Code 里
/// 敲 `dct`），而它一活就是好几天。这些变量的意思是「你是某个 agent 会话的
/// 子进程」，传给一个全新的会话就是**假的**，而 CLI 会照着它改行为——实测
/// 表现是每个新会话顶上挂一句「Transcript saving is off」，聊天记录一条不存。
///
/// 只列这一类身份标记。凭据（`ANTHROPIC_API_KEY` 之流）和登录态一律不动：
/// 那是「agent 能不能干活」，跟「它以为自己是谁」是两回事。
const INHERITED_SESSION_MARKERS: &[&str] = &[
    "CLAUDECODE",
    "CLAUDE_CODE_CHILD_SESSION",
    "CLAUDE_CODE_ENTRYPOINT",
    "CLAUDE_CODE_SSE_PORT",
];

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

/// 每个会话保留多少行滚出屏幕的内容。
///
/// 写死不做配置项：用户不该被问这个数字。vt100 的 `Cell` 约 36 字节，
/// 120 列一行约 4.2 KB，2000 行满载约 8.4 MB/会话。底下是 `VecDeque`，
/// 按实际用量增长，2000 是天花板不是预分配。
pub const SCROLLBACK_ROWS: usize = 2000;

/// pty 层看到的滚动事实。
///
/// `agent_owns` 是整个滚屏设计的分流开关：agent 开了鼠标上报就说明它自己
/// 管视口（Claude Code 就是这样），滚轮该转发给它；没开就由 dct 滚自己的
/// 缓冲（codex、命令行）。这两个真实 agent 在「用不用备用屏」上正好相反，
/// 所以判据只能是鼠标，不能是备用屏。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ScrollView {
    pub offset: usize,
    pub max: usize,
    pub agent_owns: bool,
    pub alt_screen: bool,
}

pub struct PtySession {
    parser: Arc<Mutex<vt100::Parser>>,
    writer: Arc<Mutex<Box<dyn Write + Send>>>,
    child: Arc<Mutex<Box<dyn Child + Send + Sync>>>,
    alive: Arc<AtomicBool>,
    _master: Box<dyn MasterPty + Send>,
}

impl PtySession {
    pub fn spawn(
        cmd: &[String],
        env: &std::collections::BTreeMap<String, String>,
        cwd: &Path,
        rows: u16,
        cols: u16,
    ) -> Result<PtySession> {
        anyhow::ensure!(!cmd.is_empty(), "启动命令为空");

        let pty = NativePtySystem::default()
            .openpty(PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(|_| {
                crate::proto::coded(crate::proto::ErrorCode::OperationFailed(
                    crate::proto::Operation::SpawnPty,
                ))
            })?;

        let mut builder = CommandBuilder::new(&cmd[0]);
        for a in &cmd[1..] {
            builder.arg(a);
        }
        builder.cwd(cwd);

        // 先摘掉「我正跑在别的 agent 会话里」这类标记，再加 profile 自己的环境
        // （顺序不能反：profile 想显式设回某一个，得说了算）。
        for k in INHERITED_SESSION_MARKERS {
            builder.env_remove(k);
        }

        // 除上面那几个之外只加不减：不清空继承来的环境。ANTHROPIC_BASE_URL
        // 这类是覆盖上去的，但 PATH / HOME / 各家 CLI 自己的登录态都得留着，
        // 清了 agent 就起不来。
        for (k, v) in env {
            builder.env(k, v);
        }

        // 报码不报句子。命令确实在 PATH 上但起不来（权限不对、架构不匹配、
        // 脚本头写错），底层错误对非程序员没有意义——带上命令名就够，
        // 他至少知道该去修哪个。
        let child = pty.slave.spawn_command(builder).map_err(|_| {
            crate::proto::coded(crate::proto::ErrorCode::CannotStart(cmd[0].clone()))
        })?;

        let parser = Arc::new(Mutex::new(vt100::Parser::new(rows, cols, SCROLLBACK_ROWS)));
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

    /// 跟着界面尺寸改 PTY 大小。不做这件事的话 agent 永远按初始宽度排版，
    /// 窗口再宽也只用得到左边那一块。vt100 解析器也要一起改，否则屏幕缓冲
    /// 和真实终端对不上。
    pub fn resize(&self, rows: u16, cols: u16) -> Result<()> {
        if rows == 0 || cols == 0 {
            return Ok(());
        }
        self._master.resize(PtySize {
            rows,
            cols,
            pixel_width: 0,
            pixel_height: 0,
        })?;
        self.parser
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .screen_mut()
            .set_size(rows, cols);
        Ok(())
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
                        text.to_string()
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

    /// 立刻 SIGKILL，不给宽限期。
    ///
    /// 跟 `kill()` 的差别只有那 200ms：`kill()` 走 portable-pty 的
    /// SIGHUP → 约 200ms → SIGKILL，好让 agent 有机会自己收尾（存盘、
    /// 恢复终端）。这条路给的是「敲了 stop 它还赖着不走」时的下一步，
    /// 那时候再等一次同样的 200ms 只是重复一遍已经失败过的事。
    ///
    /// **wait 一次是必须的**，跟 `kill()` 同一个理由：SIGKILL 之后不收尸
    /// 就留一个僵尸，而守护进程一活就是好几天。
    ///
    /// 拿不到 pid（子进程已经没了）不算失败：目标状态就是「它不在了」，
    /// 而它确实不在了。照样 wait 一次把可能存在的尸体收掉。
    pub fn kill_now(&mut self) -> Result<()> {
        let mut child = self.child.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(pid) = child.process_id() {
            // SAFETY: 只是给一个 pid 发信号。pid 来自我们自己 spawn 的子进程，
            // 最坏情况是它已经退出、信号发给一个不存在的进程（返回 ESRCH，
            // 下面的 wait 照样把尸体收掉）。
            unsafe {
                libc::kill(pid as libc::pid_t, libc::SIGKILL);
            }
        }
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

    /// 滚动并返回滚完之后的状态。正数往上翻进历史，负数往下。
    ///
    /// 钳位交给 vt100 自己做（`grid.rs:183-185` 会 `.min(scrollback.len())`），
    /// 我们只负责别让 i32 加法溢出。
    pub fn scroll_by(&self, rows: i32) -> ScrollView {
        let mut parser = self.parser.lock().unwrap_or_else(|e| e.into_inner());
        // 顺序不能反：probe_max 会把偏移拨到顶当副作用（见它自己的文档），
        // 先读 cur 再 probe_max，不然 cur 读到的永远是上一次探测剩下的
        // max，而不是调用方真正的当前位置——增量滚动会变成每次都跳到顶。
        let cur = parser.screen().scrollback();
        let max = probe_max(&mut parser);
        let target = if rows >= 0 {
            cur.saturating_add(rows as usize)
        } else {
            cur.saturating_sub(rows.unsigned_abs() as usize)
        };
        parser.screen_mut().set_scrollback(target.min(max));
        view_of(&parser, max)
    }

    pub fn scroll_to_bottom(&self) -> ScrollView {
        let mut parser = self.parser.lock().unwrap_or_else(|e| e.into_inner());
        // probe_max 会把偏移推到顶，所以归零必须在它之后
        let max = probe_max(&mut parser);
        parser.screen_mut().set_scrollback(0);
        view_of(&parser, max)
    }

    pub fn scroll_state(&self) -> ScrollView {
        let mut parser = self.parser.lock().unwrap_or_else(|e| e.into_inner());
        let cur = parser.screen().scrollback();
        let max = probe_max(&mut parser);
        parser.screen_mut().set_scrollback(cur);
        view_of(&parser, max)
    }

    /// 把一次鼠标事件编码成 agent 期望的转义序列并写进 PTY。
    // Task 9 实现：空壳先让 `Request::Mouse` 这条线路编译得过、跑得起来。
    pub fn write_mouse(&self, _ev: crate::proto::MouseForward) -> Result<()> {
        Ok(())
    }
}

/// vt100 不公开「现在攒了多少行历史」。但 `set_scrollback` 内部会
/// `.min(scrollback.len())` 钳一次，所以设一个大得离谱的值再读回来，
/// 读到的就是真实上限。三次字段写，不分配不拷贝。
///
/// **调用方负责把偏移放回去**——这个函数会改变它。
fn probe_max(parser: &mut vt100::Parser) -> usize {
    parser.screen_mut().set_scrollback(usize::MAX);
    parser.screen().scrollback()
}

fn view_of(parser: &vt100::Parser, max: usize) -> ScrollView {
    let screen = parser.screen();
    ScrollView {
        offset: screen.scrollback(),
        max,
        agent_owns: screen.mouse_protocol_mode() != vt100::MouseProtocolMode::None,
        alt_screen: screen.alternate_screen(),
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
            &Default::default(),
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
        let p = PtySession::spawn(
            &["cat".to_string()],
            &Default::default(),
            dir.path(),
            24,
            80,
        )
        .unwrap();
        p.write(b"ping-dct\n").unwrap();
        assert!(wait_for(&p, "ping-dct"));
    }

    #[test]
    fn reports_death() {
        let dir = tempfile::tempdir().unwrap();
        let p = PtySession::spawn(
            &["true".to_string()],
            &Default::default(),
            dir.path(),
            24,
            80,
        )
        .unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline && p.is_alive() {
            sleep(Duration::from_millis(50));
        }
        assert!(!p.is_alive());
    }

    /// SIGKILL 之后必须 `wait()` 一次把尸体收掉。守护进程一活就是好几天，
    /// 每强杀一个会话留一个僵尸的话，进程表会慢慢被填满，而用户看不到
    /// 任何症状——直到某天起不了新会话。
    #[test]
    fn kill_now_leaves_no_zombie() {
        let dir = tempfile::tempdir().unwrap();
        let mut p = PtySession::spawn(
            &["cat".to_string()],
            &Default::default(),
            dir.path(),
            24,
            80,
        )
        .unwrap();
        let pid = p.process_id().expect("刚起来的进程该有 pid");

        p.kill_now().unwrap();
        assert!(!p.is_alive(), "杀完就不该再报活着");

        // 收干净了的话，这个 pid 已经不在进程表里——`kill(pid, 0)` 只探测
        // 存在性，不真的发信号。僵尸仍然算「存在」，所以这条真的能分辨出
        // 「杀了但没收尸」。
        // SAFETY: 0 号信号不改变目标进程的任何状态，只做存在性检查。
        let alive_in_table = unsafe { libc::kill(pid as libc::pid_t, 0) } == 0;
        assert!(!alive_in_table, "{pid} 还留在进程表里，说明没 wait 收尸");
    }

    #[test]
    fn spawn_passes_env_to_the_child() {
        use std::collections::BTreeMap;
        let dir = tempfile::tempdir().unwrap();
        let mut env = BTreeMap::new();
        env.insert("DCT_TEST_MARKER".to_string(), "看得见我".to_string());

        let p = PtySession::spawn(
            &[
                "/bin/sh".to_string(),
                "-c".to_string(),
                "echo $DCT_TEST_MARKER; sleep 5".to_string(),
            ],
            &env,
            dir.path(),
            24,
            80,
        )
        .unwrap();

        assert!(
            wait_for(&p, "看得见我"),
            "profile 里的 env 必须传给子进程，否则换 base_url 的 agent 全起不来"
        );
    }

    #[test]
    fn drop_reaps_child_process() {
        let dir = tempfile::tempdir().unwrap();
        let pid = {
            let p = PtySession::spawn(
                &["cat".to_string()],
                &Default::default(),
                dir.path(),
                24,
                80,
            )
            .unwrap();
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

    /// 造一个吐 N 行然后挂着不退的会话
    fn spawn_lines(dir: &Path, n: usize) -> PtySession {
        PtySession::spawn(
            &[
                "/bin/sh".to_string(),
                "-c".to_string(),
                format!("i=1; while [ $i -le {n} ]; do echo line-$i; i=$((i+1)); done; sleep 30"),
            ],
            &Default::default(),
            dir,
            24,
            80,
        )
        .unwrap()
    }

    #[test]
    fn keeps_history_that_scrolled_off_the_screen() {
        let dir = tempfile::tempdir().unwrap();
        let p = spawn_lines(dir.path(), 100);
        assert!(wait_for(&p, "line-100"));

        // 屏幕只有 24 行，line-1 早就滚出去了
        assert!(!p.screen_text().contains("line-1\n"));

        p.scroll_by(90);
        assert!(
            p.screen_text().contains("line-1\n"),
            "往上翻 90 行应该能看见最早那行"
        );
    }

    #[test]
    fn history_is_capped_at_the_configured_size() {
        let dir = tempfile::tempdir().unwrap();
        let p = spawn_lines(dir.path(), SCROLLBACK_ROWS + 500);
        assert!(wait_for(&p, &format!("line-{}", SCROLLBACK_ROWS + 500)));

        let st = p.scroll_state();
        assert_eq!(st.max, SCROLLBACK_ROWS, "上限就是上限，不能无限涨");
    }

    #[test]
    fn scrolling_past_the_top_stops_at_the_top() {
        let dir = tempfile::tempdir().unwrap();
        let p = spawn_lines(dir.path(), 50);
        assert!(wait_for(&p, "line-50"));

        let st = p.scroll_by(i32::MAX);
        assert_eq!(st.offset, st.max, "翻到头就停在头，不能溢出");
    }

    #[test]
    fn scrolling_below_the_bottom_stops_at_the_bottom() {
        let dir = tempfile::tempdir().unwrap();
        let p = spawn_lines(dir.path(), 50);
        assert!(wait_for(&p, "line-50"));

        p.scroll_by(10);
        let st = p.scroll_by(-1000);
        assert_eq!(st.offset, 0, "往下翻过头就停在底部");
    }

    /// 回归测试：probe_max 会把偏移拨到顶当副作用，如果 scroll_by 先探测
    /// 上限再读“当前”偏移，读到的就永远是上限本身，而不是上一次滚动
    /// 停留的位置——每次增量滚动都会直接跳到最顶上。这里用一段远大于
    /// 步长的历史，确保两次小步滚动不会撞到顶（撞顶的话，错误实现和
    /// 正确实现会算出同一个答案，测不出问题）。
    #[test]
    fn scrolling_by_a_small_amount_twice_advances_instead_of_jumping_to_the_top() {
        let dir = tempfile::tempdir().unwrap();
        let p = spawn_lines(dir.path(), 200);
        assert!(wait_for(&p, "line-200"));

        let first = p.scroll_by(5);
        assert_eq!(first.offset, 5, "第一次滚 5 行应该刚好停在 5");

        let second = p.scroll_by(5);
        assert_eq!(second.offset, 10, "第二次滚 5 行应该接着往上，不是跳回顶");
    }

    /// 这条测的是 vt100 的行为，不是我们的代码——但整个「新输出时画面不动」
    /// 的设计都压在它上面（grid.rs:556-558）。它哪天变了，这里要第一个响。
    #[test]
    fn the_view_stays_put_when_new_output_arrives() {
        let dir = tempfile::tempdir().unwrap();
        let p = PtySession::spawn(
            &[
                "/bin/sh".to_string(),
                "-c".to_string(),
                "i=1; while [ $i -le 60 ]; do echo line-$i; i=$((i+1)); done; \
                 sleep 1; echo MARKER-NEW; sleep 30"
                    .to_string(),
            ],
            &Default::default(),
            dir.path(),
            24,
            80,
        )
        .unwrap();
        assert!(wait_for(&p, "line-60"));

        let before = p.scroll_by(30);
        assert!(wait_for_offset_to_grow(&p, before.offset));

        let after = p.scroll_state();
        assert!(
            after.offset > before.offset,
            "来了新行，偏移要跟着涨，画面才不动：{} -> {}",
            before.offset,
            after.offset
        );
    }

    fn wait_for_offset_to_grow(p: &PtySession, from: usize) -> bool {
        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline {
            if p.scroll_state().offset > from {
                return true;
            }
            sleep(Duration::from_millis(50));
        }
        false
    }

    #[test]
    fn an_alternate_screen_app_has_no_history_to_scroll() {
        let dir = tempfile::tempdir().unwrap();
        // ESC[?1049h 进备用屏，然后吐一堆行
        let p = PtySession::spawn(
            &[
                "/bin/sh".to_string(),
                "-c".to_string(),
                "printf '\\033[?1049h'; i=1; while [ $i -le 60 ]; do echo alt-$i; \
                 i=$((i+1)); done; sleep 30"
                    .to_string(),
            ],
            &Default::default(),
            dir.path(),
            24,
            80,
        )
        .unwrap();
        assert!(wait_for(&p, "alt-60"));

        let st = p.scroll_state();
        assert!(st.alt_screen, "应该认出它在备用屏上");
        assert_eq!(st.max, 0, "备用屏上没有历史，这跟真实终端一致");
    }

    /// 程序设了滚动区（DECSTBM）之后，vt100 不往 scrollback 里塞任何东西
    /// （grid.rs:551）。这不是我们能改的，但界面要能认出「这里翻不了」
    /// 而不是让用户对着一个没反应的滚轮猜。
    #[test]
    fn a_scroll_region_swallows_the_history() {
        let dir = tempfile::tempdir().unwrap();
        let p = PtySession::spawn(
            &[
                "/bin/sh".to_string(),
                "-c".to_string(),
                "printf '\\033[1;20r'; i=1; while [ $i -le 60 ]; do echo rgn-$i; \
                 i=$((i+1)); done; sleep 30"
                    .to_string(),
            ],
            &Default::default(),
            dir.path(),
            24,
            80,
        )
        .unwrap();
        assert!(wait_for(&p, "rgn-60"));
        assert_eq!(p.scroll_state().max, 0);
    }

    #[test]
    fn a_plain_shell_does_not_own_the_scrolling() {
        let dir = tempfile::tempdir().unwrap();
        let p = spawn_lines(dir.path(), 10);
        assert!(wait_for(&p, "line-10"));
        assert!(
            !p.scroll_state().agent_owns,
            "没开鼠标上报的程序，滚轮归 dct"
        );
    }

    #[test]
    fn an_app_that_asks_for_the_mouse_owns_the_scrolling() {
        let dir = tempfile::tempdir().unwrap();
        // ESC[?1000h 开鼠标上报，跟 Claude Code 实测抓到的一样
        let p = PtySession::spawn(
            &[
                "/bin/sh".to_string(),
                "-c".to_string(),
                "printf '\\033[?1000h'; echo mouse-on; sleep 30".to_string(),
            ],
            &Default::default(),
            dir.path(),
            24,
            80,
        )
        .unwrap();
        assert!(wait_for(&p, "mouse-on"));
        assert!(p.scroll_state().agent_owns);
    }
}
