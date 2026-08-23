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
///
/// 名单不是照着文档抄的，是**从一个真在跑的守护进程的环境块里读出来的**
/// ——它当时已经活了半天，手上攥着的正是下面这些。其中
/// `CLAUDE_CODE_MESSAGING_*` 那一对值得单说：它们是一条**还通着的**
/// 本地 IPC 管道加上进它的令牌，指回当初拉起守护进程的那个 Claude Code
/// 会话。传给一个新 agent 有两重错——身份是假的，而且白送出去一把它
/// 完全用不上的钥匙。摘掉它不属于「凭据不动」那一条：那条说的是**这个**
/// agent 自己干活要用的东西，这一对不是。
const INHERITED_SESSION_MARKERS: &[&str] = &[
    "AI_AGENT",
    "CLAUDECODE",
    "CLAUDE_CODE_CHILD_SESSION",
    "CLAUDE_CODE_ENTRYPOINT",
    "CLAUDE_CODE_MESSAGING_SOCKET",
    "CLAUDE_CODE_MESSAGING_TOKEN",
    "CLAUDE_CODE_SESSION_ID",
    "CLAUDE_CODE_SSE_PORT",
    "CLAUDE_PID",
];

/// 同样要摘掉的还有继承来的**显示假设**。来源和上面那组是同一个，东西不是
/// 同一件：那组说的是「你是谁」，这组说的是「你待的地方什么都显示不出来」。
///
/// Claude Code 给自己的每个子进程设 `NO_COLOR=1`，而守护进程常常正是从那里
/// 被拉起来的（用户在 Claude Code 里敲 `dct`），它一活好几天，这一个值就跟着
/// 传给之后每一个新会话。
///
/// 传过去是假的：agent 跑在 dct 自己开的 pty 里，那是一块真屏幕，颜色一路
/// 完整地到得了界面（`screen_spans` 三种色都保留：16 色、256 色、24 位真彩，
/// Windows 上穿过 ConPTY 也一样）。留着它，claude / codex 这些 CLI 会主动
/// 放弃上色，整个会话退成一片单色——**看上去像 dct 把颜色弄丢了，其实是
/// agent 压根没上色**。
///
/// 真想要不上色的 agent，在 profile 的 `env` 里把 `NO_COLOR` 显式写回去：
/// 摘除在前、profile 在后，写回来的那一份说了算（见 `spawn`）。
const INHERITED_DISPLAY_ASSUMPTIONS: &[&str] = &["NO_COLOR"];

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
/// 写死不做配置项：用户不该被问这个数字。vt100 0.16 的 `Cell` 正好 32
/// 字节（crate 自己拿 `const _: () = assert!(size_of::<Cell>() == 32)`
/// 钉死的，不是估的），120 列一行约 3.75 KB，2000 行满载约 7.5 MB/会话。
/// 底下是 `VecDeque`，按实际用量增长，2000 是天花板不是预分配——但
/// `vt100::Row::new` 是 `vec![Cell::new(); cols]`，一整行的 cell 一次性
/// 分配好，不是按字符数惰性长的，所以「按用量增长」说的是行数（有多少行
/// 曾经滚出过屏幕），不是每行占的字节数。
///
/// 停掉但没被 `prune` 的会话，parser（连带它这 2000 行上限的缓冲）会一直
/// 活着：`SessionManager::stop` 只杀子进程，`Session` 和它的 parser 要等
/// 有人显式调 `prune`，而这条路径没有自动触发者。所以一个「跑过一阵、
/// 已经停了、还没被清理」的会话，从这份滚屏加进来之前只占几十 KB（没有
/// 滚屏缓冲的年代），现在可能占到几 MB——这个代价是故意咽下去的：停掉的
/// 会话还能被附加进去回看它做过什么（`u` 回滚、`d` 看改动都要读这份历史），
/// 提前把它砍掉就是砍掉这个功能。写在这儿是为了不让下一个读到内存占用
/// 报表偏高的人凭空怀疑这是个泄漏。
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

        // profile 里写的是「用户认得的名字」（`claude`），这里换成「这台机器上
        // 真正启动得起来的那条命令」。Unix 上这一步什么都不做；Windows 上它
        // 是 `claude` → `cmd.exe /c C:/.../claude.cmd` 那次翻译，少了它菜单上
        // 认得出的 agent 一个都起不来（见 `sys::shell::launch_argv`）。
        let cmd = crate::sys::shell::launch_argv(cmd);

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

        // 先摘掉「我正跑在别的 agent 会话里」这类标记和它顺带带来的显示假设，
        // 再加 profile 自己的环境（顺序不能反：profile 想显式设回某一个，
        // 得说了算）。
        for k in INHERITED_SESSION_MARKERS
            .iter()
            .chain(INHERITED_DISPLAY_ASSUMPTIONS)
        {
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

    /// agent 有没有把光标藏起来（`?25l`）。
    ///
    /// 干活中的 agent 基本都藏光标：Claude Code 画那个转圈的时候光标是关着
    /// 的，而 vt100 里那个坐标仍然跟着每一次重绘在满屏乱跑。dct 以前不问
    /// 这件事、每帧都把真实终端的光标按到那个坐标上，屏幕上就多出一个
    /// 到处蹦的方块——它不是 agent 画面的一部分，是 dct 自己画上去的。
    ///
    /// 跟 `cursor()` 分成两个方法而不是让它返回 `Option`：调用方是分开的
    /// 两件事（一个填坐标、一个决定画不画），而且 `cursor()` 的形状钉在
    /// `ScreenSnapshot` 和协议里，动它要连着改一串。
    pub fn cursor_hidden(&self) -> bool {
        self.parser
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .screen()
            .hide_cursor()
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
            // pid 来自我们自己 spawn 的子进程，最坏情况是它已经退出、这一下
            // 打在一个不存在的进程上（Unix 返回 ESRCH，Windows 拿不到句柄），
            // 两边都是无害的空操作，下面的 wait 照样把尸体收掉。
            crate::sys::proc::hard_kill(pid);
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
        // 锁在上面那个块结束时就放掉了，`self.write()` 拿的是另一把锁。
        // 同时握两把是死锁的开始，这个仓库在 `create()` 上已经吃过一次
        // 「持锁做慢操作」的亏，别在这里重犯。
        match bytes {
            Some(b) => self.write(&b),
            None => Ok(()),
        }
    }
}

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

    // 硬件按钮/滚轮方向对应的按钮号。
    let raw_button = match ev.kind {
        K::WheelUp => 64,
        K::WheelDown => 65,
        K::Press(b) | K::Release(b) => u32::from(b),
    };
    let mut modifiers = 0;
    if ev.shift {
        modifiers += 4;
    }
    if ev.alt {
        modifiers += 8;
    }
    if ev.ctrl {
        modifiers += 16;
    }

    // SGR 在 release 时照实发按钮号——这也是 SGR 存在的理由之一：legacy
    // 协议做不到。legacy 协议（Default/Utf8）的 Cb/Cx/Cy 结构里没有「哪个
    // 按钮松开了」这回事，xterm 规定 release 一律用哨兵值 3；重用按钮号
    // 会让 agent 把「松开」读成「同一个按钮又按了一次」，拖拽/选区状态
    // 就跟着错位——这正是本函数文档说的「发出去比不发更糟」，只是这次
    // 错的是事件类型而不是坐标。两种编码的按钮号分开算，别混用。
    let sgr_button = raw_button + modifiers;
    let legacy_button = (if is_release { 3 } else { raw_button }) + modifiers;

    // 终端协议的坐标是 1 起算的，我们内部是 0 起算的
    let col = u32::from(ev.col) + 1;
    let row = u32::from(ev.row) + 1;

    match enc {
        vt100::MouseProtocolEncoding::Sgr => {
            let end = if is_release { 'm' } else { 'M' };
            Some(format!("\x1b[<{sgr_button};{col};{row}{end}").into_bytes())
        }
        vt100::MouseProtocolEncoding::Utf8 => {
            // ?1005 把 32+值 当 Unicode 码点、按 UTF-8 编码，不是原始字节——
            // 值一旦到 128 就跨进两字节范围（0x80..=0x7FF）。照 Default 那样
            // 直接吐单字节，只要列号 >= 96（32+97=129）就会吐出一个不合法
            // 的独立字节，把 agent 之后的整段解析都带崩，不只是这一次
            // 事件坐标读错。
            //
            // 两字节 UTF-8 能装下的最大码点是 0x7FF（2047），减掉固定加的
            // 32，单个值最大能到 2015——这也是 xterm 文档里「行列最大到
            // 2015」的来历。超过就装不下，宁可不发。
            const UTF8_MOUSE_MAX: u32 = 2015;
            if legacy_button > UTF8_MOUSE_MAX || col > UTF8_MOUSE_MAX || row > UTF8_MOUSE_MAX {
                return None;
            }
            let mut s = String::from("\x1b[M");
            s.push(char::from_u32(32 + legacy_button)?);
            s.push(char::from_u32(32 + col)?);
            s.push(char::from_u32(32 + row)?);
            Some(s.into_bytes())
        }
        vt100::MouseProtocolEncoding::Default => {
            // 单字节形式一个值最多编到 255（= 32+223），也就是值本身不能
            // 超过 223；发一个装不下的坐标比不发更糟，agent 会以为你点在
            // 别处。
            let b = 32u32.checked_add(legacy_button)?;
            let c = 32u32.checked_add(col)?;
            let r = 32u32.checked_add(row)?;
            if b > 255 || c > 255 || r > 255 {
                return None;
            }
            Some(vec![0x1b, b'[', b'M', b as u8, c as u8, r as u8])
        }
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
            &[crate::sys::testing::tool("echo"), "hello-dct".to_string()],
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
            &[crate::sys::testing::tool("cat")],
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
            &[crate::sys::testing::tool("true")],
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
            &[crate::sys::testing::tool("cat")],
            &Default::default(),
            dir.path(),
            24,
            80,
        )
        .unwrap();
        let pid = p.process_id().expect("刚起来的进程该有 pid");

        p.kill_now().unwrap();
        assert!(!p.is_alive(), "杀完就不该再报活着");

        // 收干净了的话，这个 pid 已经不在进程表里。Unix 上这是 `kill(pid, 0)`
        // 的存在性探测，而僵尸仍然算「存在」——所以这条真的能分辨出「杀了但
        // 没收尸」，那正是它要守的东西。
        let alive_in_table = crate::sys::proc::alive(pid);
        assert!(!alive_in_table, "{pid} 还留在进程表里，说明没 wait 收尸");
    }

    #[test]
    fn spawn_passes_env_to_the_child() {
        use std::collections::BTreeMap;
        let dir = tempfile::tempdir().unwrap();
        let mut env = BTreeMap::new();
        env.insert("DCT_TEST_MARKER".to_string(), "看得见我".to_string());

        let p = PtySession::spawn(
            &crate::sys::testing::sh_c("echo $DCT_TEST_MARKER; sleep 5"),
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

    /// `ps -o stat=` 是 Unix 的问法，Windows 上没有对应物（那边也没有
    /// 僵尸进程这个概念——句柄关掉进程就没了）。这条测试守的是 Unix 的
    /// 收尸路径，按平台跳过而不是改写。
    #[test]
    #[cfg(unix)]
    fn drop_reaps_child_process() {
        let dir = tempfile::tempdir().unwrap();
        let pid = {
            let p = PtySession::spawn(
                &[crate::sys::testing::tool("cat")],
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
            &crate::sys::testing::sh_c(&format!(
                "i=1; while [ $i -le {n} ]; do echo line-$i; i=$((i+1)); done; sleep 30"
            )),
            &Default::default(),
            dir,
            24,
            80,
        )
        .unwrap()
    }

    /// 名单上的每一个都得真的被摘掉。脚本是从常量本身生成的——以后往
    /// 名单里加一个，这条测试自动跟着覆盖，不会出现「加了但没接上」。
    ///
    /// 进程级环境的注意事项同下面那条。
    #[test]
    fn a_new_agent_does_not_inherit_session_markers() {
        for k in INHERITED_SESSION_MARKERS {
            std::env::set_var(k, "leaked");
        }
        let refs: String = INHERITED_SESSION_MARKERS
            .iter()
            .map(|k| format!("${k}"))
            .collect();
        let dir = tempfile::tempdir().unwrap();
        let p = PtySession::spawn(
            &crate::sys::testing::sh_c(&format!("echo \"markers=[{refs}]\"; sleep 5")),
            &Default::default(),
            dir.path(),
            24,
            80,
        )
        .unwrap();
        assert!(
            wait_for(&p, "markers=[]"),
            "会话标记必须在起 agent 之前全部摘掉，屏幕上是：{}",
            p.screen_text()
        );
    }

    /// agent 起来的时候手上不能还攥着 `NO_COLOR`：守护进程多半是从另一个
    /// agent 会话里被拉起来的，那边设了这个值，传下来就会让每个新会话的
    /// CLI 主动放弃上色。见 `INHERITED_DISPLAY_ASSUMPTIONS`。
    ///
    /// **这条测试改的是进程级的环境**（`env` 参数只能加不能减，摘除发生在
    /// 继承那一步，绕不过去）。并行跑的别的测试因此可能也看到 `NO_COLOR`，
    /// 没有一条测试会因为它变红——它只影响子进程上不上色，而其余夹具都是
    /// `sh` 脚本，本来就不上色。
    #[test]
    fn a_new_agent_does_not_inherit_no_color() {
        std::env::set_var("NO_COLOR", "1");
        let dir = tempfile::tempdir().unwrap();
        let p = PtySession::spawn(
            &crate::sys::testing::sh_c("echo \"no-color=[$NO_COLOR]\"; sleep 5"),
            &Default::default(),
            dir.path(),
            24,
            80,
        )
        .unwrap();
        assert!(
            wait_for(&p, "no-color=[]"),
            "NO_COLOR 必须在起 agent 之前被摘掉，屏幕上是：{}",
            p.screen_text()
        );
    }

    /// 反过来的另一半：profile 显式写回来的那一份必须赢。摘除在前、profile
    /// 在后，这个顺序就是「用户说了算」的全部实现。
    #[test]
    fn a_profile_can_put_no_color_back() {
        std::env::set_var("NO_COLOR", "1");
        let dir = tempfile::tempdir().unwrap();
        let mut env = std::collections::BTreeMap::new();
        env.insert("NO_COLOR".to_string(), "1".to_string());
        let p = PtySession::spawn(
            &crate::sys::testing::sh_c("echo \"no-color=[$NO_COLOR]\"; sleep 5"),
            &env,
            dir.path(),
            24,
            80,
        )
        .unwrap();
        assert!(
            wait_for(&p, "no-color=[1]"),
            "profile 里写死的 NO_COLOR 该照旧生效，屏幕上是：{}",
            p.screen_text()
        );
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
            &crate::sys::testing::sh_c(
                "i=1; while [ $i -le 60 ]; do echo line-$i; i=$((i+1)); done; \
                 sleep 1; echo MARKER-NEW; sleep 30",
            ),
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
            &crate::sys::testing::sh_c(
                "printf '\\033[?1049h'; i=1; while [ $i -le 60 ]; do echo alt-$i; \
                 i=$((i+1)); done; sleep 30",
            ),
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
            &crate::sys::testing::sh_c(
                "printf '\\033[1;20r'; i=1; while [ $i -le 60 ]; do echo rgn-$i; \
                 i=$((i+1)); done; sleep 30",
            ),
            &Default::default(),
            dir.path(),
            24,
            80,
        )
        .unwrap();
        assert!(wait_for(&p, "rgn-60"));

        // 这一条两个平台的答案不一样，而且**不一样是对的**。
        //
        // Unix 上我们读到的是程序写出来的原始字节流，DECSTBM 原样到达
        // vt100，于是它照规矩不往 scrollback 里放东西——历史就是没有。
        //
        // Windows 上中间隔着 ConPTY：滚动区是 conhost 自己解释掉的，它按
        // 那个区域算好版面，再把结果重新渲染成一串普通的输出发给我们。
        // 我们这头的 vt100 从头到尾没见过 DECSTBM，看到的只是一行行普通
        // 文字，于是历史照常攒下来。
        //
        // 对用户来说 Windows 这边反而更好（真的翻得动）。会因此在 Windows
        // 上失灵的是「这个程序自己管画面，翻不了」那句提示的**这一个**触发
        // 条件；另一个条件——备用屏——是照常穿透 ConPTY 的，见
        // `an_alternate_screen_app_has_no_history_to_scroll`，那条在两个平台
        // 上都绿。
        #[cfg(unix)]
        assert_eq!(p.scroll_state().max, 0, "设了滚动区就不该有历史");
        #[cfg(windows)]
        assert!(
            p.scroll_state().max > 0,
            "ConPTY 把滚动区解释掉了，我们这头该看到普通的历史"
        );
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
            &crate::sys::testing::sh_c("printf '\\033[?1000h'; echo mouse-on; sleep 30"),
            &Default::default(),
            dir.path(),
            24,
            80,
        )
        .unwrap();
        assert!(wait_for(&p, "mouse-on"));
        assert!(p.scroll_state().agent_owns);
    }

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

    /// legacy（非 SGR）协议里 release 用哨兵值 3，不是按钮号——单字节协议
    /// 没法告诉 agent 到底松开的是哪个按钮。重用按钮号会让 agent 把
    /// 「松开」读成「同一个按钮又按了一次」，拖拽/选区状态就会跟着错位。
    #[test]
    fn default_encoding_uses_the_release_sentinel_not_the_button_number() {
        let out = encode_mouse(
            vt100::MouseProtocolMode::PressRelease,
            vt100::MouseProtocolEncoding::Default,
            &ev(MouseForwardKind::Release(0), 0, 0),
        )
        .unwrap();
        // 32+3, 32+1, 32+1
        assert_eq!(out, vec![0x1b, b'[', b'M', 35, 33, 33]);
    }

    /// 同一个坐标上，按下和松开在 legacy 编码下必须是两个不同的字节串——
    /// 如果按钮号被直接照抄，这两个事件会长得一模一样，agent 完全无法
    /// 区分（这就是 CRITICAL 那个 bug 的可观测症状）。
    #[test]
    fn default_encoding_release_differs_from_a_press_at_the_same_spot() {
        let press = encode_mouse(
            vt100::MouseProtocolMode::PressRelease,
            vt100::MouseProtocolEncoding::Default,
            &ev(MouseForwardKind::Press(0), 0, 0),
        )
        .unwrap();
        let release = encode_mouse(
            vt100::MouseProtocolMode::PressRelease,
            vt100::MouseProtocolEncoding::Default,
            &ev(MouseForwardKind::Release(0), 0, 0),
        )
        .unwrap();
        assert_ne!(
            press, release,
            "agent 必须能分清「按下」和「松开」，否则选区/拖拽状态会错位"
        );
    }

    /// 单字节形式能表达的最大值是 223（编码字节 = 32+223 = 255，正好是
    /// u8 的上限）；wire 坐标 1 起算，所以内部 0 起算的列号最大能到 222。
    /// 这条和下面那条各自钉住边界的一侧，防止「> 255」被悄悄改成
    /// 「> 256」之类还能让旧测试蒙混过关的错误。
    #[test]
    fn default_encoding_accepts_the_largest_column_it_can_express() {
        let out = encode_mouse(
            vt100::MouseProtocolMode::PressRelease,
            vt100::MouseProtocolEncoding::Default,
            &ev(MouseForwardKind::Press(0), 222, 0),
        )
        .unwrap();
        // 32+0, 32+223, 32+1
        assert_eq!(out, vec![0x1b, b'[', b'M', 32, 255, 33]);
    }

    #[test]
    fn default_encoding_rejects_the_column_one_past_the_boundary() {
        assert!(encode_mouse(
            vt100::MouseProtocolMode::PressRelease,
            vt100::MouseProtocolEncoding::Default,
            &ev(MouseForwardKind::Press(0), 223, 0),
        )
        .is_none());
    }

    /// `legacy_button` 是 Default 和 Utf8 共用的同一个变量，这里钉住
    /// release 哨兵没有在 Utf8 分支被漏掉（万一以后有人把两个分支的
    /// 按钮计算拆开重写）。
    #[test]
    fn utf8_encoding_also_uses_the_release_sentinel() {
        let out = encode_mouse(
            vt100::MouseProtocolMode::PressRelease,
            vt100::MouseProtocolEncoding::Utf8,
            &ev(MouseForwardKind::Release(0), 0, 0),
        )
        .unwrap();
        // 32+3, 32+1, 32+1，全部落在单字节 UTF-8 范围内
        assert_eq!(out, vec![0x1b, b'[', b'M', 35, 33, 33]);
    }

    /// ?1005 把 32+值 当 Unicode 码点编码成 UTF-8。一旦列号让这个码点
    /// 跨过 128（列号 >= 96 时，32 + 97 = 129），就必须变成两字节——
    /// 如果退化成 Default 那样的单字节形式，会吐出一个不合法的独立
    /// 字节，把 agent 之后的整段解析带崩。
    #[test]
    fn utf8_encoding_uses_multiple_bytes_once_the_column_passes_127() {
        let out = encode_mouse(
            vt100::MouseProtocolMode::PressRelease,
            vt100::MouseProtocolEncoding::Utf8,
            &ev(MouseForwardKind::Press(0), 96, 0),
        )
        .unwrap();
        // 按钮=32(单字节)，列=32+97=129=U+0081(两字节 0xC2 0x81)，行=32+1=33(单字节)
        assert_eq!(out, vec![0x1b, b'[', b'M', 32, 0xC2, 0x81, 33]);
    }

    /// 两字节 UTF-8 能装下的最大码点是 0x7FF（2047），减掉固定加的 32，
    /// 单个值最大能到 2015；再大就要三字节，我们没实现也不该假装能编，
    /// 发不出去比编错更安全。
    #[test]
    fn utf8_encoding_refuses_coordinates_past_the_two_byte_ceiling() {
        assert!(encode_mouse(
            vt100::MouseProtocolMode::PressRelease,
            vt100::MouseProtocolEncoding::Utf8,
            &ev(MouseForwardKind::Press(0), 2016, 0),
        )
        .is_none());
    }
}
