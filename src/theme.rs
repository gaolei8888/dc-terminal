//! 终端背景是深是浅，以及据此选出的弱化文字样式。
//!
//! 存在的理由是一个真实事故：界面上所有弱化文字原本用 `Color::DarkGray`
//! （ANSI 亮黑，8 号色），而 Solarized 一类主题把 8 号色定义成和背景同色，
//! 于是选 agent 菜单在这些主题下渲染成一片空白——六个不可用的 agent、
//! 每行的说明栏全部隐形，只剩一个悬空的 `▶`。（底部那条操作提示栏不在其中，
//! 它用的是具名色，不走 `DarkGray`。）
//!
//! 换成写死的 256 色灰能治好深色背景，但那个灰在浅色背景上同样接近隐形。
//! 一个写死的灰不可能同时适配深浅两种底色，所以这里让它跟着背景走。

use ratatui::style::{Color, Modifier, Style};
use std::time::Duration;
// 只有 Unix 那份「问终端要背景色」的实现在用：Windows 上不问（见下面
// `StdinReader` 的两份实现），一个字节都不写、也不等。
#[cfg(unix)]
use std::io::Write;
#[cfg(unix)]
use std::time::Instant;

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

/// 界面上那几档**语义色**。跟 `dim()` 一样跟着探测出来的背景走，理由也一样：
/// 这些字落在**终端自己的背景**上，一个写死的色号不可能同时在深浅两种底色上
/// 都读得清（`Indexed(252)` 那种近白色压在白底上就是隐形）。
impl Theme {
    /// 焦点、可按的动作。**取代原来满屏的 `Color::Cyan`**——那是 ANSI 6 号色，
    /// 归终端主题管，饱和度也停在 1990 年代。
    pub fn accent(self) -> Style {
        match self {
            // 110 压在深底上 6.59:1，24 压在浅底上 4.56:1（`every_semantic_color…`
            // 那条守卫算的）。挑的是同一个色相的两档明度，所以深浅两种终端上
            // 「这是可按的东西」看起来是同一个意思。
            Theme::Dark => Style::default().fg(Color::Indexed(110)),
            Theme::Light => Style::default().fg(Color::Indexed(24)),
            // **探不出背景时退回 ANSI 青，不是退回「没有颜色」。**
            //
            // 这里跟 `dim()` 分道扬镳，理由是 `bar_style` 头上那条：Windows
            // 上不问终端、`COLORFGBG` 也没人设，探测基本恒为 `Unknown`——
            // 让 accent 在这一档变成无色，等于整个 Windows 上的界面没有强调色，
            // 「写了一整条代码路径，而目标平台上的用户一次都看不到」。
            //
            // 退回具名色在这一档是安全的，理由同 `widgets::status_style`：
            // 终端主题本来就保证这几个具名色在自己背景上可读。`dim()` 踩的坑
            // 是 8 号亮黑被设成背景色，那是灰阶特有的问题，青色没有。
            Theme::Unknown => Style::default().fg(Color::Cyan),
        }
    }

    /// 选中那一行。比正文更亮/更暗一档，加粗。
    pub fn strong(self) -> Style {
        match self {
            Theme::Dark => Style::default()
                .fg(Color::Indexed(253))
                .add_modifier(Modifier::BOLD),
            Theme::Light => Style::default()
                .fg(Color::Indexed(235))
                .add_modifier(Modifier::BOLD),
            Theme::Unknown => Style::default().add_modifier(Modifier::BOLD),
        }
    }

    /// 会话状态：干活中。
    pub fn working(self) -> Style {
        match self {
            Theme::Dark => Style::default().fg(Color::Indexed(73)),
            Theme::Light => Style::default().fg(Color::Indexed(23)),
            Theme::Unknown => Style::default().fg(Color::Cyan),
        }
    }

    /// 会话状态：等你回答。**这一档是要人动手的**，所以取暖色。
    pub fn asking(self) -> Style {
        match self {
            Theme::Dark => Style::default().fg(Color::Indexed(179)),
            // 浅底上的琥珀只能往棕里走——够亮的琥珀在浅底上一律不及格
            // （整个 6×6×6 色立方里挑不出第二个）。94 是 4.50:1，压线过。
            Theme::Light => Style::default().fg(Color::Indexed(94)),
            Theme::Unknown => Style::default().fg(Color::Yellow),
        }
    }

    /// 会话状态：空闲。
    pub fn idle(self) -> Style {
        match self {
            Theme::Dark => Style::default().fg(Color::Indexed(108)),
            Theme::Light => Style::default().fg(Color::Indexed(22)),
            Theme::Unknown => Style::default().fg(Color::Green),
        }
    }

    /// **只给真的错误用。** 原来 `Color::Red` 有 25 处，其中不少只是「这里
    /// 需要注意」——红色用滥了，真出事的时候就没有一档能再重了。
    pub fn danger(self) -> Style {
        match self {
            Theme::Dark => Style::default().fg(Color::Indexed(174)),
            Theme::Light => Style::default().fg(Color::Indexed(124)),
            // **这一档 `Unknown` 反而要钉色，跟 accent/strong 相反。**
            // 「出事了」必须看得出是出事了，加粗传达不了这件事。ANSI 红
            // （1 号）不是 `dim()` 踩过的那个坑：踩坑的是 8 号亮黑被主题设成
            // 背景色，而没有哪个主题会把红设成背景——真那么干的话，终端里
            // 每一条报错都早就隐形了，那不是这个项目能兜的。
            Theme::Unknown => Style::default().fg(Color::Red),
        }
    }
}

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

    // 必须先见到 OSC 11 的头 `ESC ] 11 ;`，再去找 `rgb:`：读端喂进来的缓冲区
    // 不保证干净——可能是上一次没读完的转义序列残留，也可能是用户在查询
    // 应答之前敲的字符。只找 `rgb:` 子串会被这类巧合骗过，把不相关的字节
    // 当成背景色解析出来。
    let after_header = s.split_once("\x1b]11;")?.1;

    // 只认带 `rgb:` 前缀的形式。有些终端理论上能回 `#RRGGBB`，但实测没遇到，
    // 不为一个没见过的格式写没法验证的解析分支——认不出来会降级，不会出错。
    let after = after_header.split_once("rgb:")?.1;

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

/// OSC 11 查询的最长等待。这是**兜底**，不是常规路径：正常情况下读取由
/// 下面的 DA1 哨兵结束，比这快得多（本地终端往返亚毫秒级）。只有既不答
/// OSC 11、又不答 DA1 的终端才会真的等满 150ms——付一次性的启动代价，
/// 而不是挂在那里等，对用户来说也还在「启动」这个心理窗口里面。
const QUERY_TIMEOUT: Duration = Duration::from_millis(150);

/// 缓冲区里有没有一条完整的 DA1（Device Attributes）回复。
///
/// 形如 `ESC [ ? 62 ; 1 ; 6 c`（主 DA），有的终端还会用 `ESC [ > ... c`
/// （次 DA）的形式，两种都认。参数段只允许数字和分号，末尾必须是 `c`。
///
/// 判据里「末尾的 `c` 必须已经到齐」这一点是刻意的：回复可能被分成几次
/// `read` 送来，参数段读了一半时这里返回 false，调用方继续读，不会把半条
/// 回复当成读完。
/// Windows 上没有调用方——那边不问终端（见 `StdinReader` 的 Windows 实现）。
/// 留着而不是一起 `#[cfg(unix)]` 掉：它是一段纯解析，自己的单元测试在两个
/// 平台上都跑得动，而哪天真去实现 Windows 那半边时，第一件要用的就是它。
#[cfg_attr(windows, allow(dead_code))]
pub(crate) fn contains_da1(bytes: &[u8]) -> bool {
    let mut i = 0usize;
    while i + 2 < bytes.len() {
        if bytes[i] == 0x1b && bytes[i + 1] == b'[' && matches!(bytes[i + 2], b'?' | b'>') {
            let mut j = i + 3;
            while j < bytes.len() && (bytes[j].is_ascii_digit() || bytes[j] == b';') {
                j += 1;
            }
            if j < bytes.len() && bytes[j] == b'c' {
                return true;
            }
            // 参数段被别的字节打断（或者还没读完）：这一处不算 DA1，
            // 往后挪一格继续找，不要就此认定整个缓冲区里没有。
        }
        i += 1;
    }
    false
}

/// 把「发查询、在 deadline 内读回复」抽出来，只为了让 `detect_with` 能在
/// 测试里跑完整的四级降级——真实实现要一个 tty 和一个会答话的终端，
/// 两样都不该是单元测试的前提。
pub(crate) trait ReplyReader {
    /// 返回读到的字节；什么都没读到（超时、不是 tty、读失败）就返回空 Vec。
    /// **不返回 Result**：调用方对所有失败的处理都一样——降级，
    /// 用错误类型区分它们只会诱导出没人需要的分支。
    fn read_reply(&mut self, deadline: Duration) -> Vec<u8>;
}

/// 真实实现：往 stdout 写 OSC 11 查询 + 一条 DA1 哨兵，用 `poll(2)` 在
/// deadline 内读 stdin，读到 DA1 回复为止。
///
/// 必须在 `enable_raw_mode()` 之后用：非 raw 模式下这段回复会被行缓冲
/// （它不带换行，读不出来）并且被回显到屏幕上（用户会看见一串乱码）。
///
/// **为什么要有 DA1 哨兵**（`\x1b[c`，跟在 OSC 11 查询后面）：
/// 只靠超时结束读取，会在终端答得比 deadline 慢时（ssh/mosh 延迟、tmux
/// 透传、机器负载高）留下一份**没人读的回复**躺在 tty 队列里，界面起来后
/// 被 crossterm 当成用户输入读进去。crossterm 0.28 不解析 OSC，`\x1b]` 落到
/// 兜底分支变成 `Alt+']'`，后面每个字节各自变成一个 `Char` 事件——而回复里
/// 的十六进制位包含 `c` 和 `d`，看板上 `c` 是进密钥管理页、`d` 是删除密钥的
/// 第一下。也就是说「终端慢了 200ms」能一路走到「一把存好的 API key 被删」，
/// 用户什么都没做。
///
/// 哨兵能治好它，靠的是两条终端行为：DA1 是**所有**终端都答的，而且终端
/// **按顺序**答。所以「DA1 的回复到了」就等于「OSC 11 的回复要么已经在
/// 手上、要么永远不会来」——读到 DA1 就可以安全收工，队列里不会剩下东西。
/// 顺带还去掉了不答 OSC 11 的终端那份固定 150ms 开销：它们照样立刻答 DA1。
///
/// 别把它「简化」掉：删了它就把上面那条从慢终端到误删密钥的路重新打开。
///
/// 这道哨兵是**唯一**一层防线，后面没有第二层兜底（`ui.rs` 里 `is_plain_key`
/// 挡的是 Alt/Meta 组合键，跟这个无关，别指望它接得住）。已知还剩两条路
/// 没堵上：一是下面读循环的 256 字节上限可能在 DA1 回复到达前就把 buf
/// 交出去（比如启动时用户在疯狂敲键）；二是有的终端/多路复用器自己在本
/// 地应答 DA1、却把 OSC 11 转发给上游终端处理，这样「按顺序应答」的前提
/// 就不成立了。下次动这段读循环之前，先想清楚这两条怎么办。
pub(crate) struct StdinReader;

#[cfg(unix)]
impl ReplyReader for StdinReader {
    fn read_reply(&mut self, deadline: Duration) -> Vec<u8> {
        // stdin 不是 tty 时什么都别做。两个理由：一是不能拿 `libc::read` 去
        // 吞掉一段被重定向进来的 stdin（那是别人的数据）；二是这种情况下
        // crossterm 会退回去打开 `/dev/tty` 读键——它和我们写查询的这个
        // stdout 不是同一条队列，回复必然没人接，上面那个「回复变按键」的
        // 竞态就从「可能」变成「一定」。
        //
        // SAFETY: `isatty` 只读一个 fd 号，不碰内存、不改进程状态；
        // STDIN_FILENO 是常量 0，无论它是否有效都只影响返回值。
        if unsafe { libc::isatty(libc::STDIN_FILENO) } != 1 {
            return Vec::new();
        }

        let mut out = std::io::stdout();
        // 写失败（stdout 被重定向/关闭）就没有查询可言，直接空手而归。
        // 两条查询一次写出去：中间不能插别的输出，否则顺序保证就没了。
        if out.write_all(b"\x1b]11;?\x07\x1b[c").is_err() || out.flush().is_err() {
            return Vec::new();
        }

        let start = Instant::now();
        let mut buf = Vec::new();
        loop {
            let Some(left) = deadline.checked_sub(start.elapsed()) else {
                // 超时：终端既不答 OSC 11 也不答 DA1。buf 里可能有半个回复，
                // 照样交出去——`parse_osc11` 要求终止符必须在，残缺的会被它
                // 判成 None。
                return buf;
            };

            if !stdin_is_readable(left) {
                return buf;
            }

            let mut chunk = [0u8; 64];
            // 直接对裸 fd 用 `libc::read`，不走 `std::io::stdin()`：后者背后是
            // 标准库的全局 `BufReader`，一次系统调用可能比这 64 字节的目标缓冲
            // 读进更多字节，多出来的部分会滞留在那个缓冲区里——`poll` 看不到
            // 它（内核队列已经空了，下一轮 poll 会误判成「不可读」，探测因此
            // 白白降级），crossterm 的事件源也看不到它（它直接读裸 fd，不经过
            // `std::io::stdin`）。滞留在那儿的字节就是用户之后敲的键，界面起
            // 来后再也读不到——和当初不让开线程去阻塞读，要防的是同一类
            // 「吃键」故障，只是换了个门进来。
            let n =
                unsafe { libc::read(libc::STDIN_FILENO, chunk.as_mut_ptr().cast(), chunk.len()) };
            match n {
                i if i <= 0 => return buf,
                n => {
                    let n = n as usize;
                    buf.extend_from_slice(&chunk[..n]);
                    // 结束条件是 DA1 回复到了，**不是**见到 BEL/ST：按 BEL 收工
                    // 的话，一个走神敲进来的 Ctrl-G 就能把读取截断，真正的 OSC 11
                    // 回复留在队列里没人读——正是哨兵要防的那件事。
                    if contains_da1(&buf) {
                        return buf;
                    }
                    // 封顶：用户在界面出来之前狂敲键盘的话，这里会一直有
                    // 字节可读。读满就走，不能让探测卡在一个喂不完的输入上。
                    if buf.len() >= 256 {
                        return buf;
                    }
                }
            }
        }
    }
}

/// Windows 上**不问**，直接空手而归——于是 `detect_with` 落到下一级去。
///
/// 不是「还没做」，是这一步在 Windows 上的失败代价和 Unix 不一样。查询要
/// 先把 `\x1b]11;?` 写进 stdout：Windows Terminal 认得，会照规矩答；而
/// 老的 conhost（`cmd.exe` 直接开出来的那个窗口）不认，它会把这串控制符
/// **当普通文本原样打在屏幕上**，用户看到的是启动时闪过一行乱码。
///
/// 光判断终端是哪一个还不够，读回复那一半更麻烦：Windows 的控制台输入
/// 默认给的是按键事件而不是字节流，要拿到 VT 序列得先开
/// `ENABLE_VIRTUAL_TERMINAL_INPUT`，再做一次带超时的读。而这段读循环上面
/// 那一大段注释讲的正是它读错时的后果——**把用户敲的键吃掉**。为一个
/// 「猜背景色深浅」的尽力而为的功能，在另一个平台上重新打开一次那个洞，
/// 不值得。
///
/// 代价是明确的、也是有出口的：Windows 上背景色判定停在 `Theme::Unknown`，
/// 而 `Unknown` 按设计就是能用的那一档（只用 DIM，不写死任何前景色，见
/// `unknown_never_pins_a_foreground_color`）。用户想要准确的深浅，设
/// `DCT_THEME` 一句话说了算，那是第 1 级、优先级还更高。
#[cfg(windows)]
impl ReplyReader for StdinReader {
    fn read_reply(&mut self, _deadline: Duration) -> Vec<u8> {
        Vec::new()
    }
}

/// stdin 在 `timeout` 内是否可读。`poll(2)` 而不是起线程去阻塞读：
/// 那个线程超时后仍卡在 `read` 上，之后会跟事件循环抢 stdin，把用户的
/// 按键吃掉——一个只在「终端不答 OSC 11」时才发作的偷键 bug。
#[cfg(unix)]
fn stdin_is_readable(timeout: Duration) -> bool {
    let mut fd = libc::pollfd {
        fd: libc::STDIN_FILENO,
        events: libc::POLLIN,
        revents: 0,
    };
    // 上取整到毫秒：截断成 0 会让 poll 变成非阻塞轮询，在极短的剩余时间里
    // 空转。毫秒级的多等对 150ms 的总预算无关紧要。
    let ms = timeout.as_millis().min(i32::MAX as u128) as i32;
    let ms = if ms == 0 { 1 } else { ms };
    // 失败（含被信号打断的 EINTR）当成「没数据」：调用方会因此降级，
    // 而重试要另写一套超时记账，为一个 150ms 的尽力而为的查询不值得。
    unsafe { libc::poll(&mut fd, 1, ms) > 0 }
}

/// 按优先级探测背景深浅。四级降级的顺序和理由见设计文档。
///
/// 环境变量和读端都从参数进来，所以这个函数是可测的、也是纯粹的调度逻辑：
/// 不碰进程环境（`set_var` 是进程级的，并行测试之间会互相踩），不碰真 stdin。
pub(crate) fn detect_with<R: ReplyReader>(
    reader: &mut R,
    dct_theme: Option<&str>,
    colorfgbg: Option<&str>,
) -> Theme {
    // 1. 用户明说了就照办，而且不再去查询终端——他已经给了答案。
    if let Some(t) = theme_from_override(dct_theme) {
        return t;
    }

    // 2. 问终端本人。比 COLORFGBG 可信：那个变量是登录时设的，用户中途
    //    换了配色它不会更新。
    if let Some((r, g, b)) = parse_osc11(&reader.read_reply(QUERY_TIMEOUT)) {
        return if is_light(r, g, b) {
            Theme::Light
        } else {
            Theme::Dark
        };
    }

    // 3. 不答 OSC 11 的终端（rxvt/urxvt/konsole）留下的线索。
    if let Some(t) = colorfgbg.and_then(parse_colorfgbg) {
        return t;
    }

    // 4. 没有任何线索。不是错误——`Unknown.dim()` 是能用的样式。
    Theme::Unknown
}

/// `detect_with` 的生产入口：接真环境变量和真 stdin。
///
/// 必须在 `enable_raw_mode()` 之后、`EnterAlternateScreen` 之前调，
/// 两头都是硬约束，理由见 `ui.rs` 里调用点的注释。
pub fn detect() -> Theme {
    let dct = std::env::var("DCT_THEME").ok();
    let fgbg = std::env::var("COLORFGBG").ok();
    detect_with(&mut StdinReader, dct.as_deref(), fgbg.as_deref())
}

/// 256 色索引 → sRGB 三通道（0.0–1.0）。
///
/// 16–231 是 6×6×6 的色立方，232–255 是灰阶。0–15 **算不出来**：那 16 个槽
/// 的实际颜色由终端主题定义，没有固定的 RGB 可言——这也正是这个项目不许拿
/// 它们上屏的原因（见 `Theme::dim` 头上那段事故记录）。
#[cfg(test)]
pub(crate) fn srgb(i: u8) -> (f64, f64, f64) {
    let c = |v: u8| f64::from(v) / 255.0;
    if i >= 232 {
        let v = 8 + 10 * (i - 232);
        (c(v), c(v), c(v))
    } else {
        const STEP: [u8; 6] = [0, 95, 135, 175, 215, 255];
        let i = i - 16;
        (
            c(STEP[(i / 36) as usize]),
            c(STEP[((i % 36) / 6) as usize]),
            c(STEP[(i % 6) as usize]),
        )
    }
}

/// WCAG 相对亮度。
#[cfg(test)]
pub(crate) fn luminance(r: f64, g: f64, b: f64) -> f64 {
    let lin = |c: f64| {
        if c <= 0.04045 {
            c / 12.92
        } else {
            ((c + 0.055) / 1.055).powf(2.4)
        }
    };
    0.2126 * lin(r) + 0.7152 * lin(g) + 0.0722 * lin(b)
}

/// 两个亮度之间的 WCAG 对比度。正文字号要 4.5:1。
#[cfg(test)]
pub(crate) fn contrast(a: f64, b: f64) -> f64 {
    let (hi, lo) = if a > b { (a, b) } else { (b, a) };
    (hi + 0.05) / (lo + 0.05)
}

#[cfg(test)]
mod tests {

    /// **每一档语义色都要在它那种背景上读得清。** 门槛跟底栏那条守卫同一个
    /// 4.5:1（正文字号），算的是同一套 WCAG 公式——`srgb`/`luminance`/`contrast`
    /// 现在是共用的，不再各写一份。
    ///
    /// 最坏情况取「最浅的深底」和「最深的浅底」：深色主题里 235 号
    /// （#262626）已经比绝大多数深色终端背景浅，浅色主题里 252 号（#d0d0d0）
    /// 也比绝大多数浅色背景深。在这两头都过得去，中间就不会出事。
    #[test]
    fn every_semantic_color_is_readable_on_its_own_background() {
        let lum = |i: u8| {
            let (r, g, b) = srgb(i);
            luminance(r, g, b)
        };
        // 最浅的深底 / 最深的浅底。
        //
        // **浅底这一头原来取 252（#d0d0d0），那个数字定错了。** 项目自己
        // 早就在用的 `dim()`（浅色下是 241）压在 252 上只有 3.95:1——按那个
        // 假设，一段已经发货很久、而且是专门为了「在浅底上也看得清」才挑出来
        // 的颜色，反而不及格。说明 252 不是这个项目的标准，是我多设的一道坎：
        // 252 是块灰底，不是浅底。改成 254（#e4e4e4），`dim()` 在那儿是
        // 4.80:1，跟它当初被挑出来的意图对得上。
        let worst_dark_bg = lum(235);
        let worst_light_bg = lum(254);

        for (theme, bg) in [(Theme::Dark, worst_dark_bg), (Theme::Light, worst_light_bg)] {
            for (name, style) in [
                ("accent", theme.accent()),
                ("strong", theme.strong()),
                ("danger", theme.danger()),
                ("working", theme.working()),
                ("asking", theme.asking()),
                ("idle", theme.idle()),
            ] {
                let Some(Color::Indexed(i)) = style.fg else {
                    panic!("{theme:?} 的 {name} 不是 256 色索引：{:?}", style.fg);
                };
                let ratio = contrast(lum(i), bg);
                assert!(
                    ratio >= 4.5,
                    "{theme:?} 的 {name}（{i} 号）对比度只有 {ratio:.2}:1，正文字号要 4.5:1"
                );
            }
        }
    }

    /// 探不出背景时，三档会话状态**退回原来的具名色**。
    ///
    /// 这不是懒得挑颜色：Windows 上探测基本恒为 `Unknown`（`StdinReader` 的
    /// Windows 实现直接空手而归，`COLORFGBG` 也没人设），而这一档下
    /// 「跟随用户自己的终端配色」原来那条理由仍然成立——也确实没有别的
    /// 安全选择。钉住它，免得哪天有人把这三行顺手改成 `dim()`：那会让
    /// Windows 上整块看板的状态列变成一片灰。
    #[test]
    fn unknown_keeps_the_named_colors_for_session_states() {
        assert_eq!(Theme::Unknown.working().fg, Some(Color::Cyan));
        assert_eq!(Theme::Unknown.asking().fg, Some(Color::Yellow));
        assert_eq!(Theme::Unknown.idle().fg, Some(Color::Green));
    }

    /// 深浅两档里，三档状态**互相之间**也要分得开——不是只跟背景分得开。
    /// 三个颜色都过了对比度却彼此撞色的话，屏幕上就是三行看不出区别的字。
    #[test]
    fn the_three_states_are_distinguishable_from_each_other() {
        for theme in [Theme::Dark, Theme::Light] {
            let idx = |s: Style| match s.fg {
                Some(Color::Indexed(i)) => i,
                other => panic!("{theme:?} 下不是索引色：{other:?}"),
            };
            let (w, a, i) = (idx(theme.working()), idx(theme.asking()), idx(theme.idle()));
            assert!(
                w != a && a != i && w != i,
                "{theme:?} 下三档状态撞色了：working={w} asking={a} idle={i}"
            );
        }
    }

    /// `Unknown` 一个颜色都不钉——同 `dim()` 那条守卫。探不出背景的时候，
    /// 任何一个写死的色号都可能正好撞上终端的背景色，而这一档的用户
    /// （Windows 基本恒为 `Unknown`）没有第二次机会。
    #[test]
    fn unknown_pins_no_color_for_the_semantic_styles() {
        // `strong` 在这一档只加粗：加粗到处都看得见，而选中那一行本来就还有
        // 一个 `▶` 在指着，不靠颜色也认得出。
        assert_eq!(
            Theme::Unknown.strong().fg,
            None,
            "Unknown 的 strong 不该钉颜色"
        );
        // `accent` 和 `danger` 都**必须**有颜色，哪怕探不出背景——Windows 上
        // 探测恒为 `Unknown`，无色等于那边整个界面没有强调色。
        assert_eq!(
            Theme::Unknown.accent().fg,
            Some(Color::Cyan),
            "探不出背景的时候，强调色还是得有颜色（Windows 恒走这一档）"
        );
        // `danger` 是**故意**的例外：出事了必须看得出来，加粗说不了这件事。
        assert_eq!(
            Theme::Unknown.danger().fg,
            Some(Color::Red),
            "探不出背景的时候，报错还是得是红的"
        );
    }
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
        assert_eq!(
            parse_osc11(b"\x1b]11;rgb:f/f/f\x07"),
            Some((0xffff, 0xffff, 0xffff))
        );
        assert_eq!(
            parse_osc11(b"\x1b]11;rgb:ff/ff/ff\x07"),
            Some((0xffff, 0xffff, 0xffff))
        );
        assert_eq!(parse_osc11(b"\x1b]11;rgb:00/00/00\x07"), Some((0, 0, 0)));
        // 两位的 0x80 应该放大到约半程，而不是 0x0080
        let (r, _, _) = parse_osc11(b"\x1b]11;rgb:80/80/80\x07").unwrap();
        assert!(
            r > 0x8000 && r < 0x8100,
            "0x80 应放大到约半程，实际 {r:#06x}"
        );
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

    /// 缺 OSC 11 头、只是碰巧含有 `rgb:` 子串的缓冲区不能被当成合法回复
    /// 解析——`StdinReader` 读到的东西不保证干净，可能是无关转义序列的
    /// 残留或者用户提前敲的字符，里面出现 `rgb:` 纯属巧合。
    #[test]
    fn rejects_coincidental_rgb_without_osc11_header() {
        assert_eq!(parse_osc11(b"rgb:ffff/ffff/ffff\x07"), None);
        assert_eq!(parse_osc11(b"some stray text rgb:0000/0000/0000\x07"), None);
    }

    /// DA1 哨兵的两种形式都要认：主 DA（`ESC [ ? ... c`）和次 DA
    /// （`ESC [ > ... c`）。认不出来就等于哨兵失效，读取退回到只靠超时结束，
    /// 慢终端的回复又会漏成按键。
    #[test]
    fn recognizes_both_da1_reply_forms() {
        assert!(contains_da1(b"\x1b[?1;2c"));
        assert!(contains_da1(b"\x1b[?62;1;6;9;15;22c"));
        assert!(contains_da1(b"\x1b[>0;95;0c")); // 次 DA
        assert!(contains_da1(b"\x1b[?c")); // 参数段空的
    }

    /// 终端按顺序答，所以两条回复常常在同一次 `read` 里一起到。
    /// 这时既要认出哨兵，也要照样把 OSC 11 那半解析出来。
    #[test]
    fn recognizes_da1_arriving_together_with_the_osc11_reply() {
        let both = b"\x1b]11;rgb:0000/2b2b/3636\x07\x1b[?62;1;6c";
        assert!(contains_da1(both));
        assert_eq!(parse_osc11(both), Some((0x0000, 0x2b2b, 0x3636)));
    }

    /// 只有 OSC 11、哨兵还没到：必须返回 false，让读循环继续读下去。
    /// 回复里的十六进制位含 `c`，`3636\x07` 这种尾巴不能被误当成 DA1。
    #[test]
    fn does_not_mistake_an_osc11_only_buffer_for_da1() {
        assert!(!contains_da1(b"\x1b]11;rgb:cdcd/dddd/dddd\x07"));
        assert!(!contains_da1(b""));
        assert!(!contains_da1(b"\x1b[?62;1;6")); // 末尾的 c 还没到
        assert!(!contains_da1(b"\x1b[?62;1;6x")); // 结尾字符不对
        assert!(!contains_da1(b"\x1b[6n")); // 别的 CSI
        assert!(!contains_da1(b"cccc")); // 光秃秃的 c
    }

    /// 半条 DA1 被拆成两次 `read` 送来：先认不出，拼齐之后要认出来。
    /// 这一条守的是「不能把半条回复当成读完」。
    #[test]
    fn recognizes_da1_split_across_reads() {
        let mut buf = Vec::new();
        buf.extend_from_slice(b"\x1b]11;rgb:fdfd/f6f6/e3e3\x07\x1b[?62;");
        assert!(!contains_da1(&buf));
        buf.extend_from_slice(b"1;6c");
        assert!(contains_da1(&buf));
    }

    /// 缓冲区里带着哨兵的尾巴也要能正常解析出背景色——真实读取拿回来的
    /// 就是这个形状（OSC 11 回复 + DA1 回复拼在一起）。
    #[test]
    fn detects_theme_from_a_buffer_that_includes_the_da1_sentinel() {
        let mut r = CannedReader::answering(b"\x1b]11;rgb:0000/2b2b/3636\x07\x1b[?62;1;6c");
        assert_eq!(detect_with(&mut r, None, None), Theme::Dark);
    }

    /// 只有哨兵回复、终端没答 OSC 11：降级到 COLORFGBG，不能把 DA1 的
    /// 数字当成颜色解析出来。
    #[test]
    fn da1_only_reply_falls_through_to_colorfgbg() {
        assert_eq!(parse_osc11(b"\x1b[?62;1;6c"), None);
        let mut r = CannedReader::answering(b"\x1b[?62;1;6c");
        assert_eq!(detect_with(&mut r, None, Some("0;15")), Theme::Light);
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

    /// 测试用的假读端：按剧本返回一段预设回复，或者返回空（= 终端一声不响，
    /// 真实世界里就是读到超时）。
    struct CannedReader {
        reply: Vec<u8>,
        calls: usize,
    }

    impl CannedReader {
        fn answering(reply: &[u8]) -> Self {
            CannedReader {
                reply: reply.to_vec(),
                calls: 0,
            }
        }
        /// 不答 OSC 11 的终端，读到超时拿到空字节
        fn silent() -> Self {
            CannedReader {
                reply: Vec::new(),
                calls: 0,
            }
        }
    }

    impl ReplyReader for CannedReader {
        fn read_reply(&mut self, _deadline: Duration) -> Vec<u8> {
            self.calls += 1;
            self.reply.clone()
        }
    }

    /// 第一级：环境变量指定了就用它，而且**不去查询终端**——用户已经明确
    /// 说了答案，再花 150ms 去问一遍是白等。
    #[test]
    fn override_wins_and_skips_the_query() {
        let mut r = CannedReader::answering(b"\x1b]11;rgb:ffff/ffff/ffff\x07");
        assert_eq!(detect_with(&mut r, Some("dark"), None), Theme::Dark);
        assert_eq!(r.calls, 0, "环境变量已经给出答案，不该再查询终端");
    }

    /// 环境变量还要压过 COLORFGBG。
    #[test]
    fn override_wins_over_colorfgbg() {
        let mut r = CannedReader::silent();
        assert_eq!(
            detect_with(&mut r, Some("light"), Some("15;0")),
            Theme::Light
        );
    }

    /// 第二级：OSC 11 答了就用它的结果。
    #[test]
    fn uses_osc11_reply_when_terminal_answers() {
        let mut dark = CannedReader::answering(b"\x1b]11;rgb:0000/2b2b/3636\x07");
        assert_eq!(detect_with(&mut dark, None, None), Theme::Dark);
        // 只许查一次：查两遍会把启动代价翻倍（而且真实读端每次都要写一遍
        // 查询、等一遍回复），这条拦的是以后某次重构顺手多调一次。
        assert_eq!(dark.calls, 1);

        let mut light = CannedReader::answering(b"\x1b]11;rgb:fdfd/f6f6/e3e3\x07");
        assert_eq!(detect_with(&mut light, None, None), Theme::Light);
    }

    /// OSC 11 还要压过 COLORFGBG：问到终端本人的答案比环境变量里的陈旧线索可信
    /// （COLORFGBG 是登录时设的，用户中途换了配色它不会更新）。
    #[test]
    fn osc11_wins_over_colorfgbg() {
        let mut r = CannedReader::answering(b"\x1b]11;rgb:fdfd/f6f6/e3e3\x07");
        assert_eq!(detect_with(&mut r, None, Some("15;0")), Theme::Light);
    }

    /// 第三级：终端不答（超时读到空）就退回 COLORFGBG。
    #[test]
    fn falls_back_to_colorfgbg_when_terminal_is_silent() {
        let mut r = CannedReader::silent();
        assert_eq!(detect_with(&mut r, None, Some("15;0")), Theme::Dark);
        assert_eq!(
            detect_with(&mut CannedReader::silent(), None, Some("0;15")),
            Theme::Light
        );
    }

    /// 回复格式不对，也要能一路降到 COLORFGBG，而不是就地放弃。
    #[test]
    fn falls_back_to_colorfgbg_when_reply_is_garbage() {
        let mut r = CannedReader::answering(b"\x1b]11;rgb:zz/zz/zz\x07");
        assert_eq!(detect_with(&mut r, None, Some("0;15")), Theme::Light);
    }

    /// 第四级：什么线索都没有就是 Unknown。这必须是一个正常出口，
    /// 不是错误——`Unknown.dim()` 本身就是能用的样式。
    #[test]
    fn unknown_when_nothing_answers() {
        let mut r = CannedReader::silent();
        assert_eq!(detect_with(&mut r, None, None), Theme::Unknown);
    }

    /// 三级全是垃圾输入的组合拳：一样只能落到 Unknown，不许 panic。
    #[test]
    fn garbage_at_every_level_lands_on_unknown() {
        let mut r = CannedReader::answering(b"not an osc reply");
        assert_eq!(
            detect_with(&mut r, Some("mauve"), Some("not;numbers")),
            Theme::Unknown
        );
    }
}
