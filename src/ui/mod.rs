use anyhow::Result;
use crossterm::cursor::SetCursorStyle;
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use crossterm::event::{
    DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste, EnableMouseCapture,
};
use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use ratatui::prelude::*;
use ratatui::widgets::{Block, Borders, ListState, Paragraph};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::Duration;

use crate::client::Client;
use crate::proto::{ProfileEntry, Request, Response};
use crate::theme::Theme;

mod widgets;
use widgets::short_path;
pub use widgets::{status_label, status_style, Msg};

mod app;
use app::App;

mod attach;
mod board;
mod grid;
mod keys;
mod pair_view;
mod phone;
mod pick;
mod secret;
mod settings_view;
mod web;

mod view;
use view::SecretPhase;
pub use view::{
    clean_secret, decide_delete_key, digit_index, pick_action, quick_start_target, secret_rows,
    verify_message, verify_outcome_applies_to, PickAction, ViewMode,
};
use view::{escape_hint, idle_help, message_after_transition, session_ended_notice, PairPhase, View};

/// 启动时探测出来的终端背景。`run()` 设一次，之后只读。
///
/// 用全局而不是给 `DrawInput` 之类的渲染入参加字段：主题是进程级配置，
/// 启动后不变，塞进每帧的入参是把一个常量伪装成状态；而渲染函数散在
/// `board`/`grid`/`pick`/`secret` 四个模块里，加一个必填字段就是几十处
/// 纯噪音的改动（测试里的构造点尤其多）。
static THEME: OnceLock<Theme> = OnceLock::new();

/// 探测终端背景并记下来。`run()` 在 `enable_raw_mode()` 之后、
/// `EnterAlternateScreen` 之前调，只调一次。
pub fn init_theme() {
    let _ = THEME.set(crate::theme::detect());
    // 见 `SOLID` 的文档：环境变量只在这里读一次，渲染路径上一次都不读。
    let _ = SOLID.set(std::env::var_os("NO_COLOR").is_none());
}

/// 弱化文字（说明栏、提示、不可用项、九宫格里没聚焦的格子）统一用这个样式。
///
/// 不能用 `Color::DarkGray`：它是 ANSI 亮黑（8 号色），Solarized Dark 等主题
/// 把 8 号色设成和背景同色，整段文字直接隐形——选 agent 菜单里所有不可用项和
/// 说明栏就这样消失过，只剩一个悬空的 ▶。
///
/// 也不能写死一个 256 色的灰：那治好了深色背景，却在浅色背景上同样接近隐形。
/// 一个写死的灰不可能同时适配深浅两种底色，所以跟着探测出来的背景走
/// （`Dark` 用偏亮的灰、`Light` 用偏暗的灰、探不出来就用终端自己的 DIM
/// 属性，见 `theme::Theme::dim`）。
///
/// 没探测过就按 `Unknown` 算——那是三种取值里最保守的一个（只挂 DIM 修饰符，
/// 不钉任何颜色），所以测试和任何绕过 `run()` 的路径都能正常渲染。
pub fn dim() -> Style {
    THEME.get().copied().unwrap_or(Theme::Unknown).dim()
}

/// 这一刻算出来的主题。探测没跑过就按 `Unknown` 算（最保守的一档）。
pub fn theme_now() -> Theme {
    THEME.get().copied().unwrap_or(Theme::Unknown)
}

/// 焦点、可按的动作。取代原来满屏的 `Color::Cyan`，理由见 `Theme::accent`。
pub fn accent() -> Style {
    THEME.get().copied().unwrap_or(Theme::Unknown).accent()
}

/// 选中那一行。
pub fn strong() -> Style {
    THEME.get().copied().unwrap_or(Theme::Unknown).strong()
}

/// **只给真的错误用**，理由见 `Theme::danger`。
pub fn danger() -> Style {
    THEME.get().copied().unwrap_or(Theme::Unknown).danger()
}

/// 标题条/底栏的实色样式。返回 `None` 表示这一档不可用，调用方退回画横线。
///
/// **这里刻意不看 `THEME`**，跟 `dim()` 正相反，理由是两者画在不同的地方：
/// `dim()` 的字落在**终端自己的背景**上，所以必须知道那个背景是深是浅，
/// 否则挑出来的灰可能和它同色（Solarized 那次事故）。实色条自己**铺背景**，
/// fg 和 bg 一起给，终端底色已经被盖住了——对比度是构造出来的，不是猜出来的。
/// 一对固定的灰在深浅两种终端上同样可读，不需要探测。
///
/// 这一点在 Windows 上是决定性的：那边不问终端（`theme::StdinReader` 的
/// Windows 实现直接空手而归），`COLORFGBG` 也没人设，于是探测基本恒为
/// `Theme::Unknown`。要是让实色条跟着 `THEME` 走，它在 Windows 上就永远
/// 不会生效——写了一整条代码路径，而目标平台上的用户一次都看不到。
///
/// 色号只取 232–255 那段灰阶，理由和 `Theme::dim` 挑 245/241 一样：终端
/// 主题重定义的是 0–15 号具名色，灰阶那 24 级没有主题去动。
///
/// `NO_COLOR` 下返回 `None`：那时候整条实色条会塌成一片没有边界的空白，
/// 而横线是纯字符，不依赖任何颜色。这是**唯一**需要保留横线画法的场景。
///
/// **`NO_COLOR` 只在 `init_theme()` 里读一次**，和 `THEME` 存进同一类全局，
/// 理由和它一样：这是进程级配置，启动后不变。更要紧的是不能在每帧渲染时
/// 现读环境变量——`std::env` 是进程全局的，而 `pty.rs` 的测试会
/// `set_var("NO_COLOR", "1")`（那是它要验的东西）。渲染时现读的话，UI 测试
/// 画出来的是横线还是实色条，取决于同一进程里哪个测试先跑到，整批 UI 测试
/// 就成了随调度翻脸的薛定谔测试。这个坑第一次写就踩了：全量跑能过，
/// 单独跑 `the_key_letters_are_bold` 直接挂。
static SOLID: OnceLock<bool> = OnceLock::new();

/// 标题条/底栏的配色。用户在设置页里选，落盘存着（`settings::save_bar_theme`）。
///
/// **色号只取两段：232–255 的灰阶，和 16–231 的 6×6×6 色立方。**避开 0–15 那
/// 十六个具名色，理由是 `theme.rs` 开头那次事故——终端主题重定义的正是这
/// 十六个，Solarized 把 8 号色设成和背景同色，用到它的地方整片隐形。色立方
/// 和灰阶没有主题去动，选出来是什么就是什么。
///
/// `Lines` 是一等公民，不是「关掉功能」：终端不支持 256 色、用户就是喜欢
/// 线条、或者要把画面截图贴到不认底色的地方，都该有这条退路。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BarTheme {
    /// 不画实色条，退回上下横线。`NO_COLOR` 下强制走这一档。
    Lines,
    Gray,
    Blue,
    Indigo,
    Teal,
    Green,
    Olive,
    Amber,
    Crimson,
    Magenta,
    Purple,
    Slate,
    /// 给浅色终端用的：深字压浅底，跟其余几档正好反过来。
    Light,
    /// 第二档浅色：暖一点的纸色底。`Light` 那档是中性灰白，压在暖色调
    /// 终端（Solarized Light、各种「纸」主题）上会显出一块冷色补丁。
    Paper,
}

impl BarTheme {
    pub fn all() -> &'static [BarTheme] {
        // 顺序是**冷 → 暖 → 浅 → 横线**，不是加进来的先后。用户在 F6 里是
        // 一路按着 ↓ 挑的，相邻两档差得越小，他越容易看出自己想要哪一档；
        // 把新加的几档堆在末尾，等于让他在一串没有关系的颜色里逐个试。
        &[
            BarTheme::Gray,
            BarTheme::Blue,
            BarTheme::Indigo,
            BarTheme::Teal,
            BarTheme::Green,
            BarTheme::Olive,
            BarTheme::Amber,
            BarTheme::Crimson,
            BarTheme::Magenta,
            BarTheme::Purple,
            BarTheme::Slate,
            BarTheme::Light,
            BarTheme::Paper,
            BarTheme::Lines,
        ]
    }

    pub fn code(self) -> &'static str {
        match self {
            BarTheme::Lines => "lines",
            BarTheme::Gray => "gray",
            BarTheme::Blue => "blue",
            BarTheme::Indigo => "indigo",
            BarTheme::Teal => "teal",
            BarTheme::Green => "green",
            BarTheme::Olive => "olive",
            BarTheme::Amber => "amber",
            BarTheme::Crimson => "crimson",
            BarTheme::Magenta => "magenta",
            BarTheme::Purple => "purple",
            BarTheme::Slate => "slate",
            BarTheme::Light => "light",
            BarTheme::Paper => "paper",
        }
    }

    /// 认不出来返回 `None`——老版本存的、手改坏了都算「没有可用的选择」，
    /// 跟 `Lang::from_code`/`ViewMode::from_code` 同一个约定。
    pub fn from_code(s: &str) -> Option<BarTheme> {
        BarTheme::all().iter().copied().find(|t| t.code() == s)
    }

    /// 这一档的实色样式；`Lines` 没有，返回 `None`。
    pub fn style(self) -> Option<Style> {
        // 每一对都是「深底 + 同色系的浅字」（`Light`/`Paper` 反过来）。挑的
        // 时候只有一条硬标准：对比度——由 `every_bar_theme_is_readable` 按
        // WCAG 的公式算，低于 4.5:1 的一律不许进来。**眼睛在这件事上不可靠**，
        // 尤其是深红、琥珀这种一眼看着很沉、算出来却够亮的颜色。
        let (bg, fg) = match self {
            BarTheme::Lines => return None,
            BarTheme::Gray => (236, 252),
            BarTheme::Blue => (24, 253),
            BarTheme::Indigo => (17, 189),
            BarTheme::Teal => (23, 195),
            BarTheme::Green => (22, 253),
            BarTheme::Olive => (58, 230),
            BarTheme::Amber => (94, 230),
            BarTheme::Crimson => (52, 224),
            BarTheme::Magenta => (89, 225),
            BarTheme::Purple => (53, 253),
            BarTheme::Slate => (60, 255),
            BarTheme::Light => (253, 236),
            BarTheme::Paper => (230, 238),
        };
        Some(
            Style::default()
                .bg(Color::Indexed(bg))
                .fg(Color::Indexed(fg)),
        )
    }

    /// 这一档在设置页里显示成什么。跟 `Lang::native_name` 不同——配色没有
    /// 「用它自己的语言写」这回事，走正常的 i18n 词条。
    pub(crate) fn label(self, lang: crate::i18n::Lang) -> &'static str {
        use crate::i18n::{text, Key};
        match self {
            BarTheme::Lines => text(Key::ThemeLines, lang),
            BarTheme::Gray => text(Key::ThemeGray, lang),
            BarTheme::Blue => text(Key::ThemeBlue, lang),
            BarTheme::Indigo => text(Key::ThemeIndigo, lang),
            BarTheme::Teal => text(Key::ThemeTeal, lang),
            BarTheme::Green => text(Key::ThemeGreen, lang),
            BarTheme::Olive => text(Key::ThemeOlive, lang),
            BarTheme::Amber => text(Key::ThemeAmber, lang),
            BarTheme::Crimson => text(Key::ThemeCrimson, lang),
            BarTheme::Magenta => text(Key::ThemeMagenta, lang),
            BarTheme::Purple => text(Key::ThemePurple, lang),
            BarTheme::Slate => text(Key::ThemeSlate, lang),
            BarTheme::Light => text(Key::ThemeLight, lang),
            BarTheme::Paper => text(Key::ThemePaper, lang),
        }
    }
}

/// 某一档配色最终画出来是什么样。**不带任何全局可变状态**：当前选的是
/// 哪一档存在 `App::bar` 里，理由见那个字段的文档（全局的话，一个改配色的
/// 测试会在别的测试 `draw()` 和断言之间把值换掉）。
///
/// `NO_COLOR` 一票否决：那时候实色条会塌成一片没有边界的空白，而用户存的
/// 那一档配色照旧留在盘上，把环境变量去掉就回来了。没初始化过就按「有颜色」
/// 算——测试和任何绕过 `run()` 的路径都走默认档，而且这个默认是**定值**，
/// 不看环境（不确定性正是 `SOLID` 要根除的东西）。
pub fn bar_style(t: BarTheme) -> Option<Style> {
    if !SOLID.get().copied().unwrap_or(true) {
        return None;
    }
    t.style()
}

/// 还原终端：退出 raw mode、关掉括号粘贴、离开 alternate screen。
///
/// 抽成自由函数是因为有两个调用方——`TerminalGuard::drop` 和信号线程。
/// 两份各自维护的清理代码迟早会漂移，而漂移的后果是用户拿到一个半还原的终端。
///
/// 两步都 `let _ =` 吞错：`Drop` 里不能 panic，而且这里能做的补救本来就只有
/// 「尽量多还原一点」。
fn restore_terminal() {
    let _ = disable_raw_mode();
    // 无条件关鼠标捕获，不管这次运行有没有真的开过：没开过时多发一次关闭
    // 序列是无害的，而漏关会让用户的终端从此点哪儿都冒出 SGR 乱码——
    // 比翻不了历史严重得多，所以这里不像捕获本身那样只在会话里才动作。
    let _ = execute!(
        std::io::stdout(),
        DisableMouseCapture,
        DisableBracketedPaste,
        // 把光标形状还给用户自己的设置。跟上面关鼠标捕获同一个道理：
        // 无条件发，没改过时多发一次无害，而漏还会让用户**退出 dct 之后**
        // 的终端一直顶着我们挑的那个形状——那种「是不是它把我终端弄坏了」
        // 的怀疑，比少一个功能难洗得多。
        SetCursorStyle::DefaultUserShape,
        LeaveAlternateScreen
    );
}

/// 这一帧要不要开关鼠标捕获。`None` = 上一帧和这一帧「在不在会话里」
/// 没变，什么都不用做。
///
/// 抽成纯函数是因为副作用（`execute!` 往 stdout 写转义序列）没法单测，
/// 判断「变没变」这件事能测——而且判断错了后果不轻：漏开会让滚轮/点击
/// 走终端自己的选中逻辑而不是这套协议，漏关会让用户退回看板之后连
/// 拖选文字复制都做不了。
fn mouse_capture_transition(was_attached: bool, is_attached: bool) -> Option<bool> {
    if was_attached == is_attached {
        None
    } else {
        Some(is_attached)
    }
}

/// 这一帧该不该抓鼠标。三个条件全真才抓。
///
/// 抽成纯函数的理由同 `mouse_capture_transition`：副作用（往 stdout 写转义
/// 序列）没法单测，判断能测——而且判断错了两个方向都难受：漏关，用户在会话里
/// 连拖选复制都做不了；漏开，agent 收不到它明明订阅了的鼠标事件。
///
/// `agent_subscribed` 来自 `App.scroll.agent_owns`，**不新开一条判据**。
/// 那个字段的语义就是「agent 自己攥着鼠标」，跟这里问的是同一个事实；
/// 各读各的，迟早会分叉成「dct 抓着鼠标却不肯滚」这种自相矛盾的状态。
fn wants_mouse_capture(attached: bool, agent_subscribed: bool, copy_mode: bool) -> bool {
    attached && agent_subscribed && !copy_mode
}

/// 这一轮 `Screen` 请求的结果决定下一帧的 `app.scroll`：拿到新画面就用
/// 新的一份，请求失败（断连、或者这次干脆没拿到 `Response::Screen`）就
/// 原样保留上一帧的值，不清空。
///
/// 断连不代表 agent 放弃了它订阅的鼠标协议，只代表这一轮没问到它现在的
/// 状态——`agent_owns` 一旦被错误地复位成 `false`，`wants_mouse_capture`
/// 就会在断连的每一帧里都把捕获关掉再开回来，那是往 stdout 反复写转义
/// 序列，断连时这是最吵的一种失败。
///
/// 抽成纯函数是因为它现在能被真正测到：喂一个 `Err`，断言拿回来的还是
/// 传进去的那个 `previous`——而不是像内联在 `run()` 里那样，测试只能
/// 设置一个字段（`app.connected`）再重新算一遍同一个纯函数，两次调用
/// 参数完全相同，永远不可能失败，也就什么都没测到。
fn scroll_after_screen_call(
    previous: crate::session::ScrollState,
    result: &Result<Response>,
) -> crate::session::ScrollState {
    match result {
        Ok(Response::Screen { scroll, .. }) => *scroll,
        _ => previous,
    }
}

/// 兜底恢复终端状态。ratatui 的 `Terminal` 不会在 `Drop` 里自动退出 raw
/// mode / alternate screen；`run()` 的主循环里到处都是 `?`，一旦某次
/// `client.call`/`term.draw` 出错就会直接从函数返回，跳过写在循环末尾的清理代码，
/// 把用户的终端卡在 raw mode（回显、行缓冲全关）。这个 guard 保证不管是提前
/// `return`/`?`、正常 `break`，还是 panic 展开，`Drop` 都会跑一次。
///
/// 它盖不住的只剩信号——那条交给 `spawn_signal_restore`。
struct TerminalGuard;

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        restore_terminal();
    }
}

/// 让「被外面杀掉」也能还原终端。
///
/// 具体怎么办按平台分（`sys::signal`）：Unix 上是一个 `sigwait` 线程收
/// SIGTERM/SIGINT/SIGHUP，Windows 上是一个控制台处理函数收 Ctrl+C 和
/// 「点了窗口的叉」。两边为什么都不在处理函数里直接干活、为什么不用标志位
/// 让主循环自己退，理由写在那个文件开头——两个系统的约束不同，但结论一样。
///
/// 这是 `TerminalGuard` 盖不住的那一半：`Drop` 在被杀时不跑。
fn spawn_signal_restore() {
    crate::sys::signal::restore_terminal_when_killed(restore_terminal);
}

pub fn run(
    client: Client,
    default_dir: PathBuf,
    lang: crate::i18n::Lang,
    socket: PathBuf,
    view_mode: ViewMode,
) -> Result<()> {
    // 必须在 enable_raw_mode 之前装：装早了无害（还没进 raw mode 时
    // restore_terminal() 没有副作用，多发一次 LeaveAlternateScreen 也无害），
    // 装晚了就有一个「已经进 raw mode 但信号还没被接管」的真空窗口。
    // 跟 TerminalGuard 提前构造是同一个理由。
    spawn_signal_restore();
    enable_raw_mode()?;
    // 必须在 EnterAlternateScreen / Terminal::new 之前构造：这样即便它们俩失败，
    // raw mode 也还是能被 Drop 恢复。
    let _guard = TerminalGuard;
    // 探测终端背景，位置被两头夹死：
    // - 必须在 enable_raw_mode() 之后：OSC 11 的回复是终端塞进 stdin 的一串
    //   字节，非 raw 模式下会被行缓冲（它不带换行，读不出来）并且被回显到
    //   屏幕上（用户会看见乱码）。
    // - 必须在 EnterAlternateScreen 之前：万一有字节漏到屏幕上，此刻还在主屏、
    //   还没开始画界面，脏字符会被随后的 alternate screen 切换盖掉；反过来就是
    //   把乱码糊在已经画好的界面上。
    // 在 TerminalGuard 之后是为了万一探测里有什么 panic，raw mode 仍能恢复。
    init_theme();
    let mut stdout = std::io::stdout();
    // 开括号粘贴：不开的话粘贴的文字会一个字符一个事件地进来，
    // 粘一段话就是几百次往返，慢到没法用。
    //
    // 顺带把光标钉成不闪的那一档。会话里那个光标是 dct 自己画上去的，
    // 用来告诉你「你打的字会落在这儿」（`ui::attach` 里的 `cursor_at`），
    // 而它闪不闪一直是终端自己的默认——多数终端默认就是闪。
    //
    // **agent 的意图在这里拿不到，所以只能由 dct 定。** 终端里表达
    // 「光标别闪」的那个序列是 DECSCUSR（`CSI Ps SP q`），而 `vt100`
    // 0.16 压根不跟踪它：agent 就算发过，也在解析时被丢掉了，屏幕快照里
    // 没有这个信息。（对比 `hide_cursor`，那个它是跟踪的，所以「agent 藏
    // 起来的光标不要画」做得到——见 `pty::cursor_hidden`。）
    //
    // 挑竖线而不是方块：这个光标画在 agent 自己的画面上，方块会把底下那个
    // 字盖掉，竖线不会。退出时用 `DefaultUserShape` 还回用户自己的设置，
    // 不是硬塞一个我们以为的默认值回去。
    execute!(
        stdout,
        EnterAlternateScreen,
        EnableBracketedPaste,
        SetCursorStyle::SteadyBar
    )?;
    let mut term = Terminal::new(CrosstermBackend::new(stdout))?;

    let mut app = App::new(client, default_dir, lang, socket, view_mode);
    // 用户存过的配色。没存过就留在 `App::new` 给的默认档，跟 `load_view_mode`
    // 那边一样：「盘上没有可用的选择」由调用方定默认，settings 只管读写。
    if let Some(t) =
        crate::settings::load_bar_theme(&crate::settings::settings_path_for_socket(&app.socket))
    {
        app.bar = t;
    }
    // 鼠标捕获只在会话里开：看板不需要滚，而开着捕获会让终端原生的选中
    // 复制失效——把这个代价限制在真正需要它的地方。见下面 `term.draw`
    // 之前那段每帧检查一次的逻辑，以及它为什么不挂在某一个「进入会话」的
    // 分支上（进 `View::Attached` 的路不止一条：`enter_session`、密钥验证
    // 通过后直接建会话、九宫格里……挂哪个分支都会漏另一条）。
    let mut mouse_captured = false;
    // 「开机那次兜底补启动目录」有没有做过。一次性的：做完之后用户 `x` 掉
    // 所有组是他自己的选择，不该被这段逻辑一次次撤销回来。
    let mut seeded = false;

    // 有标签是因为下面排空鼠标事件那段需要从一个嵌套的 `while` 里跳回
    // 这个循环的顶部，而不是跳回 `while` 自己——普通的无标签 `continue`
    // 已经覆盖了这个函数里所有别的 `continue`，不用因为加了一个标签
    // 就把它们全部改成 `continue 'main`。
    'main: loop {
        // 收后台验证的结果，必须在 term.draw 之前——通过了要直接把视图
        // 切成新开的会话，不然用户看见的这一帧还是「正在验证…」，多闪一下。
        if let Some(rx) = &app.verify_rx {
            if let Ok((sent_profile, sent_buf, outcome)) = rx.try_recv() {
                // 不管接下来用不用得上这个结果，先把 Receiver 收掉：
                // 它已经出结果了，没有第二次可读。
                app.verify_rx = None;
                if let View::EnterSecret {
                    profile,
                    label,
                    prompt,
                    buf,
                    return_to_settings,
                    pairable,
                    ..
                } = app.view.clone()
                {
                    // 这条结果只有在「发起验证时的 (profile, buf)」跟「此刻
                    // 屏幕上这一份 (profile, buf)」完全一致时才有落点——见
                    // 上面声明 `verify_rx` 时的注释和
                    // `verify_outcome_applies_to` 的文档注释。用户可能在这次
                    // 网络探测跑着的时候已经 Esc 退出去，甚至绕回来
                    // 在另一个 agent 身上重新填了密钥；这时候视图仍然是
                    // `EnterSecret`，光看"是不是这个变体"分不出是不是同一个
                    // 请求，必须把 profile 和 buf 都比对上。不满足就直接
                    // 扔掉，不切视图——套在一个不相干的 profile/密钥上
                    // 比什么都不做更危险（见 CRITICAL 1 的复现步骤）。
                    if verify_outcome_applies_to(&sent_profile, &sent_buf, &profile, &buf) {
                        app.view = match verify_message(outcome, app.lang) {
                            Some(m) => View::EnterSecret {
                                profile,
                                label,
                                prompt,
                                buf,
                                phase: SecretPhase::Failed(m),
                                return_to_settings,
                                pairable,
                            },
                            // 通过：先存盘。存密钥必须先于「开会话」/「回设置页」两条
                            // 后续路径都成立的前提——回设置页要读一份刷新过的 has_secret
                            // 才能显示「已配」，开会话是从磁盘上已经存好的密钥里现读
                            // 一份给新会话用的（见 daemon.rs），顺序反了新会话拿到的
                            // 还是空密钥。
                            None => match app.client().and_then(|c| {
                                c.call(Request::SetSecret {
                                    profile: profile.clone(),
                                    value: buf.clone(),
                                })
                            }) {
                                Ok(Response::Ok) if return_to_settings => {
                                    // 从设置页进来的是「改配置」，不是「开工」——
                                    // 存完直接回设置页，不建会话。这里**不能**甩一个
                                    // 空壳指望循环收尾那段通用重拉逻辑去补：那段逻辑
                                    // 挂在按键处理之后，而这整段 verify_rx 分支跑在
                                    // 循环顶部、不受「这一轮有没有按键」摆布——如果
                                    // 用户这时候没再按键，`event::poll` 超时会直接
                                    // `continue` 到下一轮循环顶部，跳过收尾，空壳会
                                    // 一直空着，直到用户偶然按下一个键才被补上（手测
                                    // 时真的复现了：改完密钥，界面卡在一屏空列表，
                                    // 直到按了 Esc 再按 c 才刷出来）。直接现查一遍，
                                    // 光标顺手定在刚改的这一行上。
                                    //
                                    // 改完给一句确认：这一行本身会从「未配」翻成
                                    // 「已配」，但删除那条路径有「已删除 X 的密钥」
                                    // 的消息条打底，改密钥这条路径原来什么都不说，
                                    // 是同一对镜像操作里唯一没反馈的一半——补齐。
                                    app.message =
                                        crate::i18n::msg::secret_saved(app.lang, &label).into();
                                    refetch_secrets(&mut app, Some(&profile))
                                }
                                Ok(Response::Ok) => {
                                    let dir = app.current_dir().display().to_string();
                                    match create_session(&mut app, &dir, &profile, true) {
                                        Ok(Response::Created { id }) => {
                                            app.need_sessions = true; // 会话标题要显示项目名
                                            View::Attached(id)
                                        }
                                        Ok(Response::Error(ref e)) => View::EnterSecret {
                                            profile,
                                            label,
                                            prompt,
                                            buf,
                                            phase: SecretPhase::Failed(crate::i18n::msg::error(
                                                app.lang, e,
                                            )),
                                            return_to_settings,
                                            pairable,
                                        },
                                        _ => View::EnterSecret {
                                            profile,
                                            label,
                                            prompt,
                                            buf,
                                            phase: SecretPhase::Failed(
                                                crate::i18n::text(
                                                    crate::i18n::Key::SessionOpenFailed,
                                                    app.lang,
                                                )
                                                .into(),
                                            ),
                                            return_to_settings,
                                            pairable,
                                        },
                                    }
                                }
                                Ok(Response::Error(ref e)) => View::EnterSecret {
                                    profile,
                                    label,
                                    prompt,
                                    buf,
                                    phase: SecretPhase::Failed(crate::i18n::msg::error(
                                        app.lang, e,
                                    )),
                                    return_to_settings,
                                    pairable,
                                },
                                _ => View::EnterSecret {
                                    profile,
                                    label,
                                    prompt,
                                    buf,
                                    phase: SecretPhase::Failed(
                                        crate::i18n::text(
                                            crate::i18n::Key::SecretNotSaved,
                                            app.lang,
                                        )
                                        .into(),
                                    ),
                                    return_to_settings,
                                    pairable,
                                },
                            },
                        };
                    }
                    // else：profile 或 buf 对不上——这条结果对应的是一个用户
                    // 已经离开的请求，扔了，不切视图。
                }
                // else：视图现在压根就不是 EnterSecret 了（比如用户按 Esc
                // 提前离开，切到了看板/选择器/设置页）。同样没有落点，扔了。
            }
        }

        // 手机令牌验证的结果，同上面 `verify_rx` 一个理由：必须在 `term.draw`
        // 之前收，不然用户看见的这一帧还是「正在验证…」，多闪一下。
        if let Some(rx) = &app.phone_verify_rx {
            if let Ok(status) = rx.try_recv() {
                app.phone_verify_rx = None;
                app.phone_buf = None;
                // 用户可能已经 Esc 退出手机页去了别处——这条结果没有落点，
                // 扔了，不切视图（同 `verify_rx` 收尾那段「视图对不上就
                // 不应用」的道理，只是这里判的是「还在不在这一页」而不是
                // 「profile/buf 对不对得上」，因为手机页同一时间只可能有
                // 一次在飞的验证）。
                if matches!(app.view, View::Phone { .. }) {
                    app.view = View::Phone { status };
                }
            }
        }
        // 手机页要能眼看着状态从「等配对」变成「已连上」——配对本身是
        // 异步的（守护进程一直轮询直到用户在 Telegram 里发消息），没有这
        // 一段轮询，用户守着这一页也看不到任何变化。跟九宫格的 300ms 节流
        // 同一个理由：这是「偶尔扫一眼」的东西，不是打字的地方，不用跟
        // 会话视图一样 16ms 一刷。正在打字/验证中不用刷——那两种临时态
        // 不该被一次后台轮询悄悄打断。
        if matches!(app.view, View::Phone { .. })
            && app.phone_buf.is_none()
            && app.phone_verify_rx.is_none()
        {
            let due = app
                .phone_last_fetch
                .is_none_or(|t| t.elapsed() >= Duration::from_millis(300));
            if due {
                if let Ok(Response::Phone(status)) =
                    app.client().and_then(|c| c.call(Request::PhoneStatus))
                {
                    app.view = View::Phone { status };
                    app.connected = true;
                }
                app.phone_last_fetch = Some(std::time::Instant::now());
            }
        }

        // 配对起步的结果，同上面 `verify_rx`/`phone_verify_rx` 一个理由：
        // 必须在 `term.draw` 之前收，不然这一帧画的还是「正在联系网关…」。
        if let Some(rx) = &app.pair_start_rx {
            if let Ok((stamped_profile, outcome)) = rx.try_recv() {
                app.pair_start_rx = None;
                // 用户可能已经 Esc/`p` 离开了配对屏，或者（理论上）在
                // 这条起步结果飞着的时候又开了另一条——用 profile 现比对
                // 一遍，同 `verify_rx` 收尾那段「视图对不上就不应用」的
                // 道理，不满足就扔掉，不切视图。
                if let View::Pair {
                    profile,
                    phase: PairPhase::Starting,
                } = app.view.clone()
                {
                    if profile == stamped_profile {
                        app.view = pair_view::apply_started(&mut app, profile, outcome);
                    }
                }
            }
        }
        // 等码的那一屏要能眼看着状态从「等着」变成「成功」/「过期」——
        // 配对本身是异步的（守护进程在后台线程里一直轮询网关），没有这
        // 一段轮询，用户守着这一页也看不到任何变化。跟手机页 300ms 一轮
        // 同一个理由，只是这里按 500ms（配对是「等几分钟」的事，比手机
        // 通知的「等一下」更松，没必要刷得那么勤）。
        if let View::Pair {
            profile,
            phase: PairPhase::Waiting { .. },
        } = app.view.clone()
        {
            let due = app
                .pair_last_fetch
                .is_none_or(|t| t.elapsed() >= Duration::from_millis(500));
            if due {
                if let Ok(Response::PairTick(tick)) = app
                    .client()
                    .and_then(|c| c.call(Request::PairPoll { profile: profile.clone() }))
                {
                    if let View::Pair { phase: current, .. } = app.view.clone() {
                        app.view = pair_view::apply_tick(app.lang, profile, current, tick);
                    }
                }
                app.pair_last_fetch = Some(std::time::Instant::now());
            }
        }

        let attached = matches!(app.view, View::Attached(_));
        if app.need_sessions || !attached {
            match app.client().and_then(|c| c.call(Request::List)) {
                Ok(Response::Sessions(v)) => {
                    app.set_sessions(v);
                    app.connected = true;
                    // `pinned` 跟会话列表同一个节奏拉，不是每帧拉：看板上出现
                    // 哪些组 = 有在跑的会话的 ∪ pinned 的，只刷新其中一半会让另一半
                    // 停在旧答案上（别的 dct 窗口 `p` 上来的项目永远不出现，
                    // `x` 掉的项目永远不消失）。
                    if let Ok(Response::Projects { pinned, .. }) =
                        app.client().and_then(|c| c.call(Request::Projects))
                    {
                        adopt_pinned(&mut app, pinned);
                    }
                    // 只在 `List` 成功这一支里问 `LastProfile`：守护进程连不上
                    // 的时候连问都不问，断线期间不会每轮空转一遍。
                    refresh_project_profiles(&mut app);
                    // 开机第一次拉完（而且是**成功**拉完）才补启动目录。放在
                    // 失败路径上的话，第一轮连不上就会本地摆一个组上去，等到
                    // 真连上、守护进程报回一份空的 `pinned`，它又被同步掉——
                    // 看板重新变空，而这个一次性的补位已经用掉了。
                    if !seeded {
                        seeded = true;
                        seed_start_project(&mut app);
                    }
                }
                _ => app.connected = false,
            }
            app.need_sessions = false;
        }
        if app.list_state.selected().is_none() && !app.rows.is_empty() {
            app.list_state.select(Some(0));
        }
        // 光标也可能是**没按键**落到一个新组上的：开机第一次拉完列表落在第 0
        // 行、上一行那句兜底、`refresh_rows` 的锚点回落。这几条同样得把脚下
        // 那个组 pin 住，否则它下一轮就可能自己没了。
        //
        // 按键那一路在循环末尾另有一次调用：只留这一处的话，「用户挪到 b」和
        // 「下一轮 List 报回 b 的会话停了」之间隔着一次 `set_sessions`，b 会
        // 在被 pin 之前先消失一次。
        pin_cursor_group(&mut app);
        // 会话可能在两轮之间消失（自己退了、被 s 停掉清了），焦点必须跟着
        // 收回来。不收的话 grid::move_focus 会拿到一个越界的下标——它的
        // debug_assert 就是为这条路径设的，而 release 下越界会算出一个荒唐
        // 的页长，格子全乱。收在这里（拉完列表、画之前）是唯一能保证
        // 渲染和按键看到的是同一个合法焦点的地方。
        if let View::Grid { focus, .. } = app.view {
            let last = app.grid_sessions().len().saturating_sub(1);
            if focus > last {
                app.view = View::grid(last);
            }
        }
        if let View::Grid { focus, .. } = app.view {
            let page = grid::page_of(focus);
            let start = page * grid::TILES_PER_PAGE;
            let ids: Vec<u32> = app
                .grid_sessions()
                .iter()
                .skip(start)
                .take(grid::TILES_PER_PAGE)
                .map(|s| s.id)
                .collect();
            // 300ms 一轮就够：格子是扫一眼的东西，不是打字的地方（附加视图
            // 的 16ms 是为了跟手，这里没有手要跟）。只有「翻了页（或刚进来）」
            // 才插队立刻取一次——那时候手里这批画面画的是别的会话，等满
            // 300ms 就是让新的一页空白着晾用户小半秒。这个条件取完就自己
            // 消掉，绕过节流最多一次。
            let page_changed = app.grid_page != Some(page);
            let due = page_changed
                || app
                    .grid_last_fetch
                    .is_none_or(|t| t.elapsed() >= Duration::from_millis(300));
            if due {
                match app.client().and_then(|c| c.call(Request::Screens { ids })) {
                    Ok(Response::Screens { screens }) => {
                        app.grid_screens = screens;
                        app.connected = true;
                    }
                    // 老守护进程不认识 Screens。列表视图还能用，退回去并
                    // 说清怎么修——别让用户对着一屏空格子猜。（`dct restart`
                    // 还不存在，所以只能说退出再启动。）
                    //
                    // 敢把 Error 一律诊断成「守护进程是旧版本」而不看里面写了
                    // 什么，靠的是一条事实：daemon 侧 `Screens` 那条分支
                    // （`daemon.rs` 的 `handle`）返回的永远是 `Ok`，`mgr.screens()`
                    // 不会失败——所以能走到这里的 Error 只可能是 `serve` 的
                    // 请求解析失败，而新客户端发的请求老守护进程解析不了，就是
                    // 版本对不上。**哪天 `screens()` 变成可能失败的，这句诊断就
                    // 成了假话**，那时必须改成把 Error 里的原文说给用户听。
                    Ok(Response::Error(_)) => {
                        app.message = Msg::err(
                            crate::i18n::text(crate::i18n::Key::DaemonTooOld, app.lang).into(),
                        );
                        app.view = home_view(&app);
                        app.need_sessions = true;
                    }
                    _ => app.connected = false,
                }
                app.grid_page = Some(page);
                app.grid_last_fetch = Some(std::time::Instant::now());
            }
        } else {
            // 离开九宫格就把「手里这批画面是哪一页的」忘掉，这样下次进来
            // `page_changed` 一定成立，第一帧插队立刻取一次，不用干等 300ms。
            // 这一句在 `grid_screens` 空不空之外：第一次取画面就失败的时候，
            // 画面是空的而 `grid_page` 已经被写上了，若跟着 `is_empty` 一起
            // 跳过重置，300ms 内退出再进来就是对着一屏空白熬满节流。
            app.grid_page = None;
            // 画面也扔掉。留着的话，下次再按 g 进来的第一帧画的是上一次的
            // 旧画面（可能是几分钟前的，甚至是已经没了的会话）。收在这里
            // 而不是在每个「离开九宫格」的按键分支里各清一次：出口有 g、
            // Esc、Enter 放大、n/p/c 弹出的那几个视图……漏一个就是一帧
            // 残影，而这一条判断覆盖了全部。
            app.grid_screens.clear();
        }
        if let View::Attached(id) = &app.view {
            let id = *id;
            // 把 agent 画面区的真实大小告诉它。不做的话它永远按初始宽度排版，
            // 窗口再宽也只用左边一块。
            //
            // 尺寸**从 `attach::draw` 记下来的那一份拿**（`app.screen_area`），
            // 不在这里按边框和底栏的行数手算。这里原来写的是
            // `height - (2 + 3)`，而那是布局算式的一份手抄件——两处分叉时
            // 没有任何东西会报错，症状是内容区底部凭空多出一块黑边。
            // 详见 `App::screen_area` 的文档。
            //
            // 还没画过一帧就跳过这一次 resize（**只跳这件事，不跳整轮循环**：
            // `term.draw` 在下面，跳掉整轮就永远画不出第一帧，`screen_area`
            // 也就永远填不上，直接转成死循环）。下一轮就有真实尺寸了，
            // 晚一帧远好过发一个猜出来的数——发错了 agent 会按错的宽度
            // 重排一次版，用户看得见那一次跳动。
            let (cols, rows) = app.screen_area.unwrap_or((0, 0));
            if app.sent_size != Some((id, rows, cols))
                && rows > 0
                && cols > 0
                && app
                    .client()
                    .and_then(|c| c.call(Request::Resize { id, rows, cols }))
                    .is_ok()
            {
                app.sent_size = Some((id, rows, cols));
            }
            let screen_result = app.client().and_then(|c| c.call(Request::Screen { id }));
            // 按键和滚轮怎么分流、底栏那句提示写什么、这一帧该不该抓鼠标，
            // 都看 `app.scroll` 这一份——每 16ms 跟着 `Screen` 一起刷新，
            // 滞后最多一帧，够用了。独立于下面这个 match 先算好：请求失败
            // 时这个 match 走的是 `_` 分支，不会碰 `scroll`，见
            // `scroll_after_screen_call` 头上的注释。
            app.scroll = scroll_after_screen_call(app.scroll, &screen_result);
            match screen_result {
                Ok(Response::Screen {
                    lines,
                    cursor,
                    cursor_hidden,
                    state,
                    ..
                }) => {
                    app.screen = lines;
                    app.screen_cursor = cursor;
                    app.screen_cursor_hidden = cursor_hidden;
                    app.connected = true;
                    // agent 自己退出之后不能把用户留在这里：那是一张纯空白页
                    // （agent 在 alternate screen 里画，退出时恢复的主屏从来
                    // 没被写过），底栏还写着「其余按键都发给 agent」，而他敲的
                    // 每个键都掉进一个死掉的 pty 里无声消失。
                    if let Some(notice) = session_ended_notice(id, state, app.lang) {
                        app.view = home_view(&app);
                        // 回看板得重新拉一次 List：贴在会话里这一路都没拉，
                        // 手里的 sessions 是进会话之前那份，缺的正是「这个
                        // 会话已经没了」这条更新。
                        app.need_sessions = true;
                        // 会话正常结束不是错误，用普通提示，不是红字
                        app.message = notice.into();
                        // 下一个会话的尺寸要重新协商：sent_size 记的是刚退出
                        // 的这个 id，留着会让新会话第一帧按错的宽度排版。
                        app.sent_size = None;
                    } else if state == crate::session::SessionState::Failed {
                        // 已经拿到这个会话**这一次**失败的解释了：不再问、
                        // 也不再碰 app.message——见 `App::explained_failure`
                        // 上的注释，这是不让附加视图 16ms 一帧焊死消息栏的
                        // 关键。只有还没拿到答案时才问一次。
                        let already_have = app
                            .explained_failure
                            .as_ref()
                            .is_some_and(|(cached_id, _)| *cached_id == id);
                        if !already_have {
                            // 出错解释是异步算出来的（daemon 侧丢给了后台
                            // 线程，不在 tick() 里等模型），这里问到才显示；
                            // 没问到就什么都不做——今天原本就没有这条提示，
                            // 界面必须长得一模一样，不能因为这个功能露出一个
                            // 新的空白/报错态。
                            if let Ok(Response::Explanation(Some(text))) = app
                                .client()
                                .and_then(|c| c.call(Request::Explanation { id }))
                            {
                                app.message =
                                    Msg::err(crate::i18n::msg::session_failure_explained(
                                        app.lang, id, &text,
                                    ));
                                app.explained_failure = Some((id, text));
                            }
                        }
                    } else if app
                        .explained_failure
                        .as_ref()
                        .is_some_and(|(cached_id, _)| *cached_id == id)
                    {
                        // 这个会话不再是 Failed 了（恢复了）：把缓存忘掉。
                        // 不忘的话，它下次再坏，`already_have` 会一直是
                        // true，新的一次失败永远问不出新的解释。
                        app.explained_failure = None;
                    }
                }
                _ => app.connected = false,
            }
        }

        // 抓不抓鼠标不再只看「在不在会话里」：agent 没订阅鼠标的会话
        // （codex、shell）里抓着它，唯一的效果是把终端的拖选复制废掉，
        // 换来一个 PageUp/PageDown/End 已经能做的滚轮。
        //
        // 检查一次三个条件的合取有没有变，而不是在每个能改变它们的分支
        // 各开关一次——那样的分支太多、太容易漏（上面 `mouse_captured`
        // 声明处的注释列了几条）。放在 `term.draw` 之前是因为这一轮循环里
        // 所有会改 `app.view` 的代码（`verify_rx` 收尾、`Screen` 探测发现
        // 会话已结束……）到这里都已经跑完，此刻的 `app.view` 就是即将画出来
        // 的那一帧；而且这一轮的 `Screen` 响应已经落进 `app.scroll` 了，
        // `agent_owns` 就是这一帧的事实。
        let is_attached = matches!(app.view, View::Attached(_));
        let want = wants_mouse_capture(is_attached, app.scroll.agent_owns, app.copy_mode);
        if let Some(enable) = mouse_capture_transition(mouse_captured, want) {
            let _ = if enable {
                execute!(std::io::stdout(), EnableMouseCapture)
            } else {
                execute!(std::io::stdout(), DisableMouseCapture)
            };
            mouse_captured = enable;
        }

        term.draw(|f| draw(f, &mut app))?;

        // 会话里要跟手：刷新慢了，你敲的字要等下一轮才显示，每次按键都像卡了一下。
        // 看板不需要这么勤快，150ms 足够，也省得每轮都去锁一遍所有会话。
        let tick = if attached { 16 } else { 150 };
        if !event::poll(Duration::from_millis(tick))? {
            continue;
        }
        let mut ev = event::read()?;
        // 鼠标事件在这里先摘出来单独处理，而且可能不止吃掉这一个：见下面
        // 的注释。这个 `while` 结束之后，`ev` 保证不再是 `Event::Mouse`，
        // 后面的 Paste/Key 分支照旧用它。
        //
        // 循环体里对 `handle_mouse` 的调用不违反房规（见 `attach::handle_key`
        // 头上「永远不要 continue」那条）——它压根不 continue，只是被这个
        // `while` 循环调用；`continue 'main` 才是真正跳过循环末尾清理
        // `message` 那段的地方，而 `handle_mouse` 内部**不许**碰
        // `app.message`，那条约束写在它自己的文档注释上，不受这里怎么
        // 调用它影响。
        while let Event::Mouse(m) = ev {
            let acted = attach::handle_mouse(&mut app, m);
            if acted {
                // 真的送出了一次请求（滚动了、转发了点击/松开）：状态可能
                // 变了，照旧走一次完整的循环体去重新取一遍 Screen、重绘。
                continue 'main;
            }
            // 没做事的绝大多数是纯移动——`EnableMouseCapture` 打开的
            // `?1003h` 是任意移动追踪，跟 agent 有没有订阅无关，鼠标扫一下
            // 80 列宽的窗口就是几十个这种事件，`handle_mouse` 早就把它们
            // 原地丢掉了（见它的文档）。问题是外层 `continue` 会把「取一次
            // Screen、画一帧」全套重放一遍——用一个被丢弃的小事件换来一次
            // 昂贵的守护进程往返和终端重绘，跟当初「移动事件不转发是为了
            // 省流量」的初衷正好背道而驰。这里原地看一眼有没有紧跟着到达
            // 的下一个事件：有就继续在这个 `while` 里处理掉，没有就老实
            // 结束这一轮——不需要刷新时最多等到下一次自然的 16ms/150ms
            // tick，不会更旧，只是不为每一个被丢弃的移动事件单独刷一次。
            if !event::poll(Duration::from_millis(0))? {
                continue 'main;
            }
            ev = event::read()?;
        }
        // 粘贴整段一次发完，不能拆成一个个字符
        if let Event::Paste(text) = ev {
            match &mut app.view {
                View::Attached(id) => {
                    let id = *id;
                    // 这里不走 `app.client()`：它需要 `&mut self`（整个 App），
                    // 跟上面 `&mut app.view` 这个字段级借用同时活着会撞借用检查——
                    // 直接查字段，`None` 归到跟真实断线一样的失败路径。
                    let failed = match app.client.as_mut() {
                        Some(c) => !text.is_empty() && c.call(Request::Input { id, text }).is_err(),
                        None => !text.is_empty(),
                    };
                    if failed {
                        app.message = Msg::err(
                            crate::i18n::text(crate::i18n::Key::PasteNotSent, app.lang).into(),
                        );
                    }
                }
                // 手输路径态：粘贴直接进输入框。从别处拷一条路径粘进来一步到位，
                // 这是不做目录浏览器的底气。trim 掉换行——从终端或文件管理器
                // 拷路径经常带一个尾随换行，不去掉会拼出一个不存在的目录。
                View::PickProject(p) if p.typing_path.is_some() => {
                    if let Some(buf) = p.typing_path.as_mut() {
                        buf.push_str(text.trim());
                    }
                }
                // 密钥十有八九是粘进来的，不是敲的——用户拿到手的字符串通常带
                // 引号、Bearer 前缀、尾随换行，clean_secret 统一洗一遍。
                // Verifying 期间不接：那次验证已经把当时的 buf 发出去了，
                // 这时候再改只会让用户误以为下一次回车用的是新值。
                View::EnterSecret { buf, phase, .. }
                    if !matches!(phase, SecretPhase::Verifying) =>
                {
                    buf.push_str(&clean_secret(&text));
                }
                _ => {}
            }
            continue;
        }
        let Event::Key(key) = ev else {
            continue;
        };
        if key.kind != KeyEventKind::Press {
            continue;
        }

        // 处理这次按键前拍个快照，处理完之后用来判断 message 该不该清——
        // 见 message_after_transition 的注释。
        let view_kind_before = std::mem::discriminant(&app.view);
        let message_text_before = app.message.text.clone();
        let message_error_before = app.message.error;

        // 这里以前先拦一道全局 Ctrl+Q（「退一层，一直按就退到头」）。它没了：
        // 每个视图的退路都写在底栏左段上，而且每个视图都已经有一个——会话
        // 视图 F2、其余视图 Esc、看板和九宫格 `q`。少一个全局键的代价是
        // 「猜不到就得看底栏」，收益是 0x11 重新归 agent，而且底栏上写的键
        // 就是全部的键，没有第二套暗规则。
        //
        // 现在每个键都直接进视图自己的处理函数。必须 clone：分支里要给 view
        // 赋值，match &view 会被借用检查器拒掉。
        match app.view.clone() {
            View::Board => board::handle_key(&mut app, key)?,
            View::PickProfile { .. } => pick::handle_key(&mut app, key)?,
            View::PickProject(_) => pick::handle_key(&mut app, key)?,
            View::Attached(_) => attach::handle_key(&mut app, key)?,
            View::Grid { .. } => grid::handle_key(&mut app, key)?,
            View::Keys { .. } => keys::handle_key(&mut app, key)?,
            View::Settings { .. } => settings_view::handle_key(&mut app, key)?,
            View::EnterSecret { .. } => secret::handle_key(&mut app, key)?,
            View::Secrets { .. } => secret::handle_key(&mut app, key)?,
            View::Phone { .. } => phone::handle_key(&mut app, key)?,
            View::Web => web::handle_key(&mut app, key)?,
            View::Pair { .. } => pair_view::handle_key(&mut app, key)?,
        }
        // 按键**可能**把光标挪到了另一个项目上（方向键、Tab、数字键、F3、
        // 九宫格里的方向键……）。挪到哪就 pin 哪，理由见 `pin_cursor_group`。
        // 放在整个 match 之后而不是逐个按键分支里：那些分支散在四个模块里，
        // 漏一个就是一个「这个项目会在你手没动的时候消失」的洞，而且不会有
        // 任何编译期信号。已经 pin 上的组直接返回，所以这里不花任何往返。
        pin_cursor_group(&mut app);

        // 退出必须在这里落地，不能拖到循环末尾的收尾代码之后。现在有三条路
        // 会置 quit：看板上按 q、
        // 九宫格里按 q。走到下面的 needs_*_refetch / message_after_transition
        // 也不会有副作用，但那是这三条路各自的巧合，不是那段代码保证的——
        // 而且退出点还会再增加（九宫格那条就是后加的）。在这里 break 直接
        // 还原了原来 `break Ok(())` 的位置：退出这件事不依赖任何关于「谁能
        // 置 quit」的假设，往后新加的退出点也不会在退出前多打一次
        // Request::Profiles、多改一次 app.message。
        if app.quit {
            break;
        }

        // 好几条路都能把 view 换成一个空的 PickProfile——`EnterSecret` 的 Esc
        // 分支拿不到 daemon 连接，只能给个
        // entries: vec![] 的空壳，约定见它的文档注释），EnterSecret 自己的
        // Esc 分支也直接手搭了同一个空壳。两条路都得补，所以放在这里统一
        // 收口，而不是在每个「退回选择器」的地方各查一次——漏一个分支就是
        // 一屏空白，用户会以为自己一个 agent 都没装。
        let needs_profile_refetch =
            matches!(&app.view, View::PickProfile { entries, .. } if entries.is_empty());
        if needs_profile_refetch {
            // 补壳子的这一路也要问一次：空壳是 `EnterSecret` 的 Esc 搭出来的，
            // 它压根不知道当前项目是什么样。
            let no_git = current_is_not_a_repo(&app);
            app.view = match app
                .client()
                .and_then(|c| c.call(Request::Profiles { lang }))
            {
                Ok(Response::Profiles { entries, warnings }) => {
                    let warning = join_warnings(&warnings, lang);
                    // 只做 LLM 后端的那些不进这一屏——见 `view::agent_rows`。
                    let entries = view::agent_rows(&entries);
                    let mut state = ListState::default();
                    if !entries.is_empty() {
                        state.select(Some(0));
                    }
                    View::PickProfile {
                        entries,
                        state,
                        warning,
                        no_git,
                    }
                }
                Ok(Response::Error(ref e)) => View::PickProfile {
                    entries: Vec::new(),
                    state: ListState::default(),
                    warning: Some(crate::i18n::msg::error(lang, e)),
                    no_git,
                },
                _ => View::PickProfile {
                    entries: Vec::new(),
                    state: ListState::default(),
                    warning: Some(
                        crate::i18n::text(crate::i18n::Key::CannotListAgents, app.lang).into(),
                    ),
                    no_git,
                },
            };
        }

        // 同样的空壳套路用在 Secrets 上：EnterSecret 的 Esc 从设置页
        // 那条分支进来时、以及验证成功后回设置页时，都是先甩一个空壳占位，
        // 这里补一次 Profiles 把数据填上。`Secrets` 没有 `warning` 字段
        // （跟 `PickProfile` 不一样，见它的字段注释——密钥页的错误反馈走的
        // 是 `message`），拉取失败就直接退回看板并把原因放进 `message`，
        // 总比让用户卡在一屏永远拉不出数据的空列表上强。
        let needs_secrets_refetch =
            matches!(&app.view, View::Secrets { entries, .. } if entries.is_empty());
        if needs_secrets_refetch {
            app.view = match app
                .client()
                .and_then(|c| c.call(Request::Profiles { lang }))
            {
                Ok(Response::Profiles { entries, .. }) => {
                    let mut state = ListState::default();
                    if !secret_rows(&entries).is_empty() {
                        state.select(Some(0));
                    }
                    View::Secrets {
                        entries,
                        state,
                        pending_delete: None,
                    }
                }
                Ok(Response::Error(ref e)) => {
                    app.message = Msg::err(crate::i18n::msg::error(app.lang, e));
                    home_view(&app)
                }
                _ => {
                    app.message = Msg::err(
                        crate::i18n::text(crate::i18n::Key::CannotListSecrets, app.lang).into(),
                    );
                    home_view(&app)
                }
            };
        }

        // 视图变了就把上一屏的残留消息清掉，好让「按视图给提示」的 idle_help
        // 露出来；除非这条消息本身就是这次切换的操作结果（见函数注释）。
        //
        // CRITICAL：这段清理必须原样留在循环末尾，不能挪进任何按键分支——
        // e0ba1ec 就是在这里翻的车：一句普通的「已切到 X」盖掉了屏幕上
        // 唯一告诉用户怎么退出的行。退出本身在上面已经 `break` 掉了，走不到
        // 这里；这段清理只服务于还要继续循环的那些迭代。
        let view_changed = std::mem::discriminant(&app.view) != view_kind_before;
        let message_changed =
            app.message.text != message_text_before || app.message.error != message_error_before;
        app.message = message_after_transition(view_changed, message_changed, app.message);
    }

    Ok(())
}

/// 用系统默认方式打开一个网址，成功了返回 `true`。
///
/// 三个系统三个说法：macOS 是 `open`，Linux 桌面环境一般是 `xdg-open`，
/// Windows 是让 cmd.exe 去 `start`。挨个试，全都失败了才认输——用户按下
/// Ctrl+O 是在等申领页面弹出来，悄无声息什么都不做，他分不清是自己按错了
/// 键还是这台机器就是打不开浏览器（调用方在拿到 `false` 时要把这句话说
/// 出来，见按键处理里的注释）。
fn open_url(url: &str) -> bool {
    #[cfg(windows)]
    {
        // `start` 不是一个程序，是 cmd.exe 的内建命令，所以必须经它。
        // 中间那个空字符串是 `start` 的「窗口标题」参数：不给的话，一个
        // 带引号的网址会被 start 当成标题，于是什么都不打开。
        let comspec = std::env::var("ComSpec").unwrap_or_else(|_| "cmd.exe".to_string());
        let mut c = std::process::Command::new(comspec);
        // 界面是全屏 alt-screen，cmd.exe 继承同一个控制台会把窗口糊花。
        crate::sys::proc::no_console(&mut c);
        c.args(["/c", "start", "", url]).spawn().is_ok()
    }
    #[cfg(not(windows))]
    {
        ["open", "xdg-open"]
            .iter()
            .any(|cmd| std::process::Command::new(cmd).arg(url).spawn().is_ok())
    }
}

/// 把一次按键翻译成要送进 agent 的字节。返回 `None` 表示这个键不转发。
///
/// 空串是与 `session::send_input` 约定的"回车"信号——只有它会触发检查点，
/// 逐字符输入不会产生提交。所以回车必须返回 `Some(String::new())` 而不是 "\r"。
pub fn key_to_input(key: &KeyEvent) -> Option<String> {
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    let alt = key.modifiers.contains(KeyModifiers::ALT);
    let s = match key.code {
        KeyCode::Enter => String::new(),
        KeyCode::Char(c) if ctrl => {
            // Ctrl+Q 这里没有例外：它曾是 dct 自己的逃生键，被这一层扣着不发。
            // 逃生键改成 F2 独一份之后，0x11 就该跟别的 Ctrl 组合一样进 agent。
            // Ctrl+A..Ctrl+Z -> 0x01..0x1a，其余 Ctrl 组合不转发
            let lower = c.to_ascii_lowercase();
            if lower.is_ascii_lowercase() {
                char::from(lower as u8 - b'a' + 1).to_string()
            } else {
                return None;
            }
        }
        KeyCode::Char(c) => c.to_string(),
        KeyCode::Backspace => "\x7f".into(),
        KeyCode::Tab => "\t".into(),
        KeyCode::BackTab => "\x1b[Z".into(),
        KeyCode::Up => "\x1b[A".into(),
        KeyCode::Down => "\x1b[B".into(),
        KeyCode::Right => "\x1b[C".into(),
        KeyCode::Left => "\x1b[D".into(),
        KeyCode::Home => "\x1b[H".into(),
        KeyCode::End => "\x1b[F".into(),
        KeyCode::PageUp => "\x1b[5~".into(),
        KeyCode::PageDown => "\x1b[6~".into(),
        KeyCode::Delete => "\x1b[3~".into(),
        KeyCode::Insert => "\x1b[2~".into(),
        // Esc 必须转发：agent 拿它做取消、清空、关弹窗
        KeyCode::Esc => "\x1b".into(),
        _ => return None,
    };

    // **Alt（meta）要补上 ESC 前缀。** 终端几十年的约定是「meta 发 ESC」：
    // `Alt+X` 送出去的是 `ESC` 后面跟着 `X` 两个字节。不补的话，`Alt+V` 到
    // agent 手里就是一个光秃秃的 `v`——用户按的是贴图，agent 收到的是
    // 「他打了个字母」。
    //
    // 受害的远不止贴图：readline 的 `Alt+B`/`Alt+F`（按词移动）、`Alt+D`
    // （删词），以及任何用 Alt 的 TUI，在 dct 底下全都退化成普通字母。
    // **而且这种坏法不报错**——键是通的、字符也进去了，只是意思变了。
    //
    // **只给 `Char` 补前缀，别的键一个都不补**，两条理由：
    //
    // - 回车那一支返回的是**空串**，那是跟 `session::send_input` 约定的
    //   「提交」信号（只有它打检查点）。给它补前缀，`Alt+Enter` 就成了一个
    //   纯 `ESC`——用户想换行，agent 收到「取消」。
    // - 方向键那些本来就是 CSI 序列，而 xterm 对它们的修饰键编码是写进参数
    //   （`Alt+Up` 是 `CSI 1;3A`），不是在前面贴一个 ESC。贴了是另一种编码，
    //   agent 认不出来。要支持那一档得单独写，这里不假装支持。
    if alt && matches!(key.code, KeyCode::Char(_)) {
        return Some(format!("\x1b{s}"));
    }
    Some(s)
}

/// 底栏画按键表要知道的「现在能干什么」。
///
/// 「当前是哪个会话」在两个视图里问法不同：列表问光标停在哪一行
/// （`selected_session()`，停在组头上就是「没选中」），九宫格问焦点在哪一格
/// （`grid_sessions()` + `focus`）——跟 `session_action` 的两个调用点是同一条
/// 分岔，理由也一样。收在这里一次，就不会有「底栏按列表算、按键按格子算」
/// 这种两边不一致。
fn help_ctx(app: &App) -> view::HelpCtx {
    help_ctx_for(app, &app.view)
}

/// 同上，但按**指定的视图**算。
///
/// `?` 浮层要用这一支：它自己是 `View::Keys { from }`，而它列的是 `from`
/// 那一屏能按什么。拿 `app.view` 去算，从九宫格按 `?` 之后问到的会是列表
/// 光标的状态——那正是这一屏存在的意义（回答「我**现在**能按什么」）被
/// 悄悄答错的方式。
fn help_ctx_for(app: &App, view: &View) -> view::HelpCtx {
    // 列表问光标停在哪一行——**停在组头上就是「没选中」**（`selected_session()`
    // 自己就是这么定义的）。这一点是 `Enter 进会话` 写不写的唯一依据：组头行
    // 上按 Enter 什么都不会发生，写出来就是屏幕上写着一个按不动的键。
    let cur = match view {
        View::Grid { focus, .. } => app.grid_sessions().get(*focus).cloned(),
        _ => app.selected_session().cloned(),
    };
    view::HelpCtx {
        selected: cur.map(|s| view::SelectedSession {
            is_agent: s.is_agent,
            state: s.state,
        }),
        // `x` 只拿得掉「pinned 且没有在跑的会话」的组，跟 `unpin_current`
        // 的两条守卫逐条对上——底栏说什么就得真能做到什么。判据是同一个
        // `has_live_session`，不是另写一个 `sessions.is_empty()`。
        can_remove: app
            .current_group()
            .map(|g| g.pinned && !g.has_live_session())
            .unwrap_or(false),
        // 跟 `jump_project` 逐条对上：0 个组直接 return，1 个组
        // `rem_euclid(1)` 恒为 0（原地不动）。两种都不该写这个键。
        can_switch_project: app.groups.len() > 1,
        // 修复 6：只有手机页会用到这个字段（`idle_help` 的 `View::Phone`
        // 分支），但算它不需要知道自己是不是在手机页上——`phone_buf` 只在
        // 那一页会被置成 `Some`，其它任何视图下这里恒为 `false`。
        phone_editing: app.phone_buf.is_some(),
        web_on: app.web.on,
    }
}

/// `p` 选定之后：告诉守护进程记下来、更新本地 `pinned`、重算行、把光标
/// 送到那个组上、回家视图，再为它开一个新终端。
///
/// **`p` 不是「把光标挪过去」。** 那件事是 `Tab`（零弹窗、一个键）。`p` 是
/// 「我要去那个项目干活」：把它摆上看板、光标落进去，然后直接问「用哪个
/// agent」。「当前项目」是光标动了的后果，不是 `p` 自己另有一套写法。
///
/// 抽成一个函数是因为选择器有两条确认路径（列表选中、手输路径），而这五步
/// 必须整套发生。分开写的话，漏掉重算的那条路会让屏幕停在旧的一屏，而用户
/// 刚刚明确选了一个项目——那正是上一版被判为「混乱」的手感。
///
/// **摆上来之后紧接着开一个新终端。** `p` 是用户唯一一次明确说出「我要去
/// 那个项目」的地方（`Tab`/`1`…`9` 只是把光标挪过去看一眼，不在这条路上），
/// 而他去那儿几乎总是为了在那儿干活。停在看板上等他再按一次 `N` 是白让人
/// 走一步：他刚刚才做完「选哪个项目」这个决定，接着要做的「用哪个 agent」
/// 是同一件事的下半句。
///
/// 走 `N` 那一支（选择器）而不是 `n`（直开上次那个）：新摆上来的项目多半
/// 压根没有「上次那个 agent」，而**已有的项目也未必想再用同一个**——换项目
/// 常常正是换了一件事。让他看一眼列表，代价是一次 `Enter`。
///
/// Esc 从选择器退出去落在看板上（见 `pick::handle_pick_profile`），项目已经
/// 切好了——「只想换个项目看看」这条老路还在，只是多按一个键。
pub(crate) fn pin_project(app: &mut App, dir: std::path::PathBuf) {
    add_project(app, dir);
    // 先回家视图再开选择器，顺序不能反：`open_new_session` 拉不到 agent
    // 列表时**不动 `view`**，而这一刻的 `view` 还是项目选择器——用户会被
    // 留在他刚刚确认过的那一屏上，只多出一句红字，连自己已经切过去了都
    // 看不出来。
    app.view = home_view(app);
    open_new_session(app, KeyCode::Char('N'));
}

/// 当前项目**不是** git 仓库吗——选 agent 那一屏拿它决定要不要写那句
/// 「不是 git 仓库 —— 按 g 初始化」，以及 `g` 按下去算不算数。
///
/// 判据跟守护进程建会话时用的是**同一个函数**（`session.rs` 里
/// `profile.is_agent && !git::is_repo(dir)`），不另写一份：分成两份的话，
/// 界面说得通、Enter 下去却被拒（或者反过来）只是时间问题。
///
/// 客户端自己问 git、不走协议：守护进程永远在同一台机器上（unix socket /
/// 命名管道），而目录浏览器早就在客户端这边判 `.git`（`view::list_dirs`）。
/// 走协议要给 `Request` 加变体，那是要升 `PROTOCOL_VERSION` 的——为一个
/// 本地文件系统问题让所有旧界面停摆，不值。
pub(crate) fn current_is_not_a_repo(app: &App) -> bool {
    !crate::git::is_repo(&app.current_dir())
}

/// 装入守护进程报回来的那份 `pinned`，**但绝不把光标脚下那个组一起冲掉**。
///
/// 整份赋值是必须的：另一个 dct 窗口 `p` 上来的项目要出现，`x` 掉的要消失。
/// 但赋完之后紧跟着的 `refresh_rows` 会按新的 `pinned` 重算成员，而成员规则
/// 是「有在跑的会话 ∪ pinned」——光标脚下那个组要是只靠 pin 留着（会话都停了、
/// 或者压根没有会话），这一下就把它抹掉了，光标掉回第 0 行。**当前项目在用户
/// 没按键的时候变了**，而起因只是一次 IPC 往返。
///
/// 两条路都真实存在，不是想象出来的：
///
/// - `PinProject` 掉在半路上。`client.rs` 的超时会把连接整个丢掉、下一次调用
///   透明重连，于是紧接着的 `List`/`Projects` 全都成功——那条 pin 就这么没了，
///   没有任何错误浮到界面上。
/// - 另一个 dct 窗口对同一个项目按了 `x`。
///
/// 所以「组不塌陷」不能押在一次往返成功上。补回来的同时把那条请求也重发一次：
/// 只补本地的话，守护进程每一轮都会再报一份没有它的列表，这里就每一轮都要
/// 重算一遍行（`refresh_rows` 对每个会话目录都要 `canonicalize`）。
pub(crate) fn adopt_pinned(app: &mut App, fresh: Vec<String>) {
    if !projects_changed(&app.pinned, &fresh) {
        return;
    }
    // 先记下来：`current_group()` 读的是**这一刻**的 `groups`，赋值之后
    // 那个组可能就查不到了。
    let keep = app
        .current_group()
        .map(|g| g.display_dir.display().to_string());
    app.pinned = fresh;
    if let Some(d) = keep {
        let key = view::canon(Path::new(&d));
        if !app.pinned.iter().any(|p| view::canon(Path::new(p)) == key) {
            let _ = app
                .client()
                .and_then(|c| c.call(Request::PinProject { dir: d.clone() }));
            app.pinned.push(d);
        }
    }
    app.refresh_rows();
}

/// **光标落在哪个项目上，就把哪个项目 pin 住。**
///
/// 这一条是「组不塌陷」在新的成员规则下唯一的支点。组留在看板上的理由是
/// 「有在跑的会话 ∪ pinned」（见 `view::group_sessions`），于是一个没被 pin
/// 的组，最后一个会话自己跑完停掉的那一刻就没了——而那可能正是光标**此刻
/// 站着**的组：`find_anchor` 找不到会话行，也找不到同 dir 的组头，只能退回
/// 第 0 行。**当前项目在用户没碰键盘的时候变了**，接着的 `n`/`x` 作用在
/// 另一个项目上。那正是整条分支要消灭的缺陷，也正好违反 spec §三的
/// 「组不塌陷」。
///
/// 让光标所在的组恒为 pinned，这件事在结构上就不可能发生——不必给
/// `find_anchor` 或 `group_sessions` 加一条「除非光标在里面」的特例（那种
/// 特例意味着「看板上有哪些组」要看光标在哪，两个本该独立的东西缠在一起）。
///
/// 顺带把升级路径关上：`pinned` 是这条分支才有的东西，升级后第一次运行时
/// 每个既有项目都是没 pin 的，而 `seed_start_project` 因为看板不空而不动手。
/// 光标第一次落下就把自己那个项目 pin 上了。
///
/// **按「组变了没有」发请求，不是按「光标动了没有」**：判据就是这个组自己
/// 的 `pinned` 标志——已经 pin 上的组直接返回，所以方向键在同一个组里上下
/// 走一百下，一次往返都不会有。
pub(crate) fn pin_cursor_group(app: &mut App) {
    let Some(g) = app.current_group() else {
        return;
    };
    if g.pinned {
        return;
    }
    // 存**原始拼写**，同 `add_project`：`pinned` 同时是组头 name/parent 的
    // 显示来源，canon 只用于比较。
    let d = g.display_dir.display().to_string();
    // 落盘失败不拦路：本地先记上，用户这一次照样不会被抽走脚下的组。
    let _ = app
        .client()
        .and_then(|c| c.call(Request::PinProject { dir: d.clone() }));
    // `g.pinned` 为假就意味着 `app.pinned` 里没有任何一条跟它 canon 相等
    // （`group_sessions` 就是这么算出这个标志的），所以直接 push 不会重复。
    app.pinned.push(d);
    app.refresh_rows();
}

/// 上面那五步里**不动视图**的前四步。
///
/// 拆出来只为一个调用方：`seed_start_project`。它跑在后台路径上（主循环
/// 第一次 `List` 成功之后），而 `pin_project` 最后那一行是「用户刚在选择器
/// 里选定了一个项目，把他送回家」——这两件事只是碰巧共用前四步。
pub(crate) fn add_project(app: &mut App, dir: std::path::PathBuf) {
    let d = dir.display().to_string();
    // 落盘失败不拦路：pinned 是便利性状态，本地先摆上，用户这一次照样能用。
    // 下一轮拉取会把守护进程那边的真相同步回来（见 `run` 里的 `Projects`）。
    let _ = app
        .client()
        .and_then(|c| c.call(Request::PinProject { dir: d.clone() }));
    // 按归一化后的路径判重，不按字面：同一个项目用两种拼法（走符号链接、
    // 不走）敲进来是同一个组，`pinned` 里却会多出一条永远没人用的死行。
    let key = view::canon(&dir);
    if !app.pinned.iter().any(|p| view::canon(Path::new(p)) == key) {
        // 存**原始拼写**：`pinned` 同时是组头 name/parent 的显示来源
        // （见 `view::group_sessions`），归一化只用于比较。
        app.pinned.push(d);
    }
    app.refresh_rows();
    if let Some(gi) = app.groups.iter().position(|g| g.dir == key) {
        goto_project(app, gi);
    }
}

/// `x`：把光标所在的组从看板上拿掉。
///
/// 还有**在跑**的会话就拒绝，红字说一句。「顺便把这些会话都停掉」是个用户
/// 没要求过的复合破坏动作，而 `s` 已经能一个一个停——一次拒绝比一次多做
/// 好解释得多。
///
/// **已停止的会话不算数**（判据是 `ProjectGroup::has_live_session`，跟
/// `can_remove` 共用一个）。它没有进程，拿掉这个组毁不掉任何还活着的东西。
/// 按「有没有会话」拒绝的话会拒出一个死局：一个只剩已停止会话的项目永远
/// 下不了看板，而 `x` 连写都不写，那句「先停掉才能移除」指的又正是用户
/// 刚做过的事——唯一的出路是去另一个终端敲 `dct prune`。
///
/// **返回「真的拿掉了吗」。** 三条 return 里有两条是拒绝，一条是无事可做，
/// 它们跟成功那条对屏幕的影响完全不同，调用方必须分得开：九宫格那边成功之后
/// 要重新对齐光标（光标站的组没了），而**拒绝之后绝不能动光标**——那一刻
/// 底栏正写着「这个项目还有会话，先停掉才能移除」，一句「什么都没发生」的话
/// 配上一个偷偷换掉的当前项目，是最坏的组合：用户接着按 `n`，会话开进了
/// 另一个项目，而屏幕刚刚告诉过他这一下没生效。
#[must_use = "拒绝和成功对光标的处理不同，调用方必须分开处理"]
pub(crate) fn unpin_current(app: &mut App) -> bool {
    let Some(g) = app.current_group() else {
        return false;
    };
    if g.has_live_session() {
        app.message = Msg::err(crate::i18n::text(crate::i18n::Key::GroupNotEmpty, app.lang).into());
        return false;
    }
    // 组能出现在看板上只有两个理由：有**在跑**的会话、或者被 pin 了
    // （见 `group_sessions` 的成员规则）。上面已经排掉前者，所以走到这儿的
    // 必然是 pinned——真不是（结构上到不了）就什么都不做，而不是发一个
    // 守护进程认不出的 unpin。
    if !g.pinned {
        return false;
    }
    let key = g.dir.clone();
    let d = key.display().to_string();
    let _ = app
        .client()
        .and_then(|c| c.call(Request::UnpinProject { dir: d }));
    // **按归一化后的路径删，不按字面。** `pinned` 里存的是用户当初敲的
    // 拼写，而 `g.dir` 是 canon 之后的分组键——macOS 上 `/tmp/x` 就是
    // `/private/tmp/x`。字面比对删不掉的话，`x` 看起来像「按了没反应」：
    // 组消失一帧，下一次重算又原样回来。
    app.pinned.retain(|p| view::canon(Path::new(p)) != key);
    app.refresh_rows();
    true
}

/// 开机兜底：看板上一个组都没有时，把启动目录摆上去。
///
/// 全新安装、或者第一次跑这一版时 `pinned` 是空的、也还没有任何会话。
/// 没有这一步，用户第一眼看到的是一个连光标都落不下去的空盒子。摆上启动
/// 目录之后，「看板上一个组都没有」这个状态在开机路径上就不存在了——光标
/// 永远有地方落，`n` 永远有目标。
///
/// 已经有组了就不碰：启动目录跟用户手头这些项目未必有关系，硬塞一行进去
/// 只是噪音。
///
/// **这是一条后台路径，不许换用户正看着的那一屏。** 它挂在主循环第一次
/// `List` **成功**之后——守护进程要是慢上几轮（或者头几轮压根连不上），
/// 这中间用户完全可能已经按 `N` 开了选择器、按 `l` 进了设置页。那时候
/// 走 `pin_project`（它最后一行无条件 `app.view = home_view(app)`）会把人
/// 从他自己打开的那一屏拽回看板，而他没有按过任何键。今天走不到只是因为
/// 第一轮 `List` 几乎总是一次就成——那是运气，不是设计。
///
/// 本来就在看板/九宫格上才重算落点：那时候「回家」是恒等式，唯一的作用
/// 是让刚摆上去的组在九宫格里也有个合理的焦点。
pub(crate) fn seed_start_project(app: &mut App) {
    if !app.groups.is_empty() {
        return;
    }
    add_project(app, app.start_dir.clone());
    if matches!(app.view, View::Board | View::Grid { .. }) {
        app.view = home_view(app);
    }
}

/// 守护进程报回来的 `pinned` 跟手里这份**指的是不是同一组项目**。
///
/// 不能直接 `!=` 就认字面：守护进程存的是归一化后的路径（见
/// `projects::key_of`），而刚被 `pin_project` 摆上去的那一条存的是用户敲的
/// 原始拼写。字面比对会判成「变了」，于是每次拉取都把用户的拼写换成
/// `/private/tmp/...` 这种归一化结果——组头下面那行灰字会在用户没做任何事
/// 的时候自己变一次样，还白搭一次 `refresh_rows`（它对每个会话目录都要
/// `canonicalize` 一次）。
///
/// 先比字面（绝大多数情况下两边一模一样，这条路一次 `canonicalize` 都不做），
/// 只有字面不同才去做归一化比较——那是真的可能变了，值得那几次系统调用。
fn projects_changed(have: &[String], fresh: &[String]) -> bool {
    if have == fresh {
        return false;
    }
    let keys = |v: &[String]| {
        let mut k: Vec<PathBuf> = v.iter().map(|p| view::canon(Path::new(p))).collect();
        k.sort();
        k.dedup();
        k
    };
    keys(have) != keys(fresh)
}

/// 还没问过「上次用的是哪个 agent」的那些组。
///
/// 单独抽出来是因为这里是**唯一**能挡住每帧一次阻塞往返的地方，而它挡不挡
/// 得住只看两个集合够不够全：拿到答案的（`profiles`）、以及问过但守护进程
/// 说「没有记录」的（`profiles_asked`）。只看前者的话，一个确实没有记录的
/// 项目永远不会被写进 `profiles`，于是每一轮都要为它重发一次请求——看板
/// 150ms 一轮，守护进程一忙界面就会一顿一顿。**负答案必须也缓存。**
fn profiles_to_fetch(app: &App) -> Vec<String> {
    app.groups
        .iter()
        .map(|g| g.dir.display().to_string())
        .filter(|d| !app.profiles.contains_key(d) && !app.profiles_asked.contains(d))
        .collect()
}

/// 把每个组「上次用的 agent」补齐，底栏的 `n 新建 <agent>` 要用。
///
/// 只为**还没问过**的组各发一次请求（见 `profiles_to_fetch`），所以稳定态
/// 下这个函数一次往返都不发。调用点在 `run` 里 `List` 成功那一支里面：
/// 守护进程连不上的时候连问都不问，也就不会在断线期间每轮空转一遍。
pub(crate) fn refresh_project_profiles(app: &mut App) {
    let mut got = false;
    for d in profiles_to_fetch(app) {
        match app
            .client()
            .and_then(|c| c.call(Request::LastProfile { dir: d.clone() }))
        {
            Ok(Response::LastProfile(answer)) => {
                // 问到了就记下「问过了」，不管答案是有还是没有——这一条
                // 就是负答案的缓存。
                app.profiles_asked.insert(d.clone());
                if let Some(p) = answer {
                    app.profiles.insert(d, p);
                    got = true;
                }
            }
            // 请求本身没成，不算问过：这不是「没有记录」，而且这一轮多半
            // 整条连接都断了，剩下的组再问也是白问，直接收手。
            _ => break,
        }
    }
    // 只在真拿到新东西时才重算。`refresh_rows` 会对每个会话目录做一次
    // `canonicalize`（真实的文件系统调用），无条件每轮再来一遍纯属白烧。
    if got {
        app.refresh_rows();
    }
}

/// **开会话的唯一入口。** 界面里没有第二处发得出建会话请求
/// （`every_create_goes_through_the_one_helper_that_updates_the_cache` 钉着
/// 这一条，靠扫源码——它数的就是下面那一行）。
///
/// 唯一的理由就是这个函数末尾那三行：`remember: true` 的 `Create` 会让守护
/// 进程记下「这个项目上次用的 agent」，而手里那份缓存（`profiles` /
/// `profiles_asked`）**不会**自己知道。`profiles_to_fetch` 只问没问过的项目，
/// 所以缓存里那个旧值不是「晚一轮才更新」，是**这一整次 dct 运行都不会再更新**：
/// 项目 A 记着 claude、底栏写着 `n 新建 claude`，用户按 `N` 选了 codex，
/// 守护进程记的是 codex，底栏却一直写 claude 直到重启——而底栏那一句正是
/// 「按 n 会开出什么」的承诺。
///
/// 三个调用点各记一次的写法（原来就是那样，而且三处里只有一处记了）撑不住
/// 下一个调用点：新加一条建会话的路的人没有任何提示要去补这三行。收到一处，
/// 忘不掉。
pub(crate) fn create_session(
    app: &mut App,
    dir: &str,
    profile: &str,
    remember: bool,
) -> Result<Response> {
    let r = app.client().and_then(|c| {
        c.call(Request::Create {
            dir: dir.to_string(),
            profile: profile.to_string(),
            remember,
        })
    });
    // 只有真建成了才跟着记：守护进程侧也是这条规矩（`daemon.rs` 里
    // `if r.is_ok()`），建失败的目录不该留下「上次用的是它」。
    // `remember: false` 是「帮你装 CLI」那条路径开的 shell 会话，守护进程
    // 不记，这里同样不能记——记了下次按 `n` 会掉进一个命令行。
    if remember && matches!(r, Ok(Response::Created { .. })) {
        app.profiles.insert(dir.to_string(), profile.to_string());
        app.profiles_asked.insert(dir.to_string());
    }
    // 复制模式不能从上一个会话粘到这个新建的来——跟 `enter_session` 对应
    // 位置的注释是同一件事：这里和 `enter_session` 一起，是「进」这一侧
    // 仅有的两个入口，合起来才盖得住所有会落到 `View::Attached` 的路径
    // （选择器建会话、密钥验证通过后建会话、`n`/`N` 快速建会话都经这里）。
    // 不看 `r` 是否真的建成才复位：建失败时调用方本来就不会真的切进
    // `View::Attached`，复位一次不占谁的便宜，也不用为了「只在成功时」
    // 再包一层判断。
    app.copy_mode = false;
    r
}

/// 进一个会话。
///
/// 这里**不再**改写任何「当前项目」——它不再是一个字段，而是光标所在的
/// 那个组。从别的组进一个会话时，光标本来就已经在那个组里了（不然那一行
/// 根本不会被选中），所以既没有什么可改，也没有什么可报告。
pub(crate) fn enter_session(app: &mut App, id: u32) {
    // 会话标题要显示项目名
    app.need_sessions = true;
    app.view = View::Attached(id);
    // 每次「进入」都当成一次全新的观察：`explained_failure` 缓存的是
    // 「上一次贴在这个会话里时看到的解释」，如果这个会话在用户离开、
    // 又回来之间恢复过、再坏过一次，缓存里那份还是上上次失败的旧话，
    // 不清掉的话 `run()` 主循环会以为「问过了」，永远不会去问新的那次。
    app.explained_failure = None;
    // 上一个会话的复制模式不能粘到这一个来。**在「进入」这一侧复位**——
    // 进 `View::Attached` 的路不止一条（看板 Enter、九宫格 Enter、F3 都走
    // 这个函数，但选择器建会话、密钥验证通过后建会话、`n`/`N` 快速建会话
    // 走的是 `create_session`，见 `run()` 里鼠标捕获那段注释）。真正撑住
    // 「复位一次就漏不掉」的不是某一个函数是唯一入口，而是**进的这一侧
    // 一共只有 `enter_session` 和 `create_session` 两个构造器，合起来盖住
    // 所有会落到 `View::Attached` 的路径**——这里复位一次，`create_session`
    // 里再复位一次，两处就覆盖完了。反过来，离开有三条路，其中一条走的是
    // 各视图自己的 Esc/F2 分支——它们散在四个模块里，为这一个
    // 字段改它的签名不值，所以选在「进」而不是「出」的这几处写。
    //
    // 留在看板上的那个 `copy_mode` 是无害的：`wants_mouse_capture` 的第一个
    // 条件就是「贴在会话里」，不在会话里时它压根不参与判断。
    app.copy_mode = false;
    // CONTROLLER RULING：进入会话视图一律落在底部，不管上次离开时翻到
    // 哪儿了、也不管这一次算不算「换了个尺寸」。
    //
    // 原来落地在哪儿全看 `run()` 主循环里那条按 `sent_size` 判断要不要发
    // `Request::Resize` 的分支——`SessionManager::resize` 顺手把 scroll 归了
    // 零，于是「回不回底部」变成了一个跟尺寸缓存打不打得上的意外结果：
    // 切到另一个会话再回来，id 变了，`sent_size` 不匹配，触发 Resize，
    // 顺带回到底部；按 F2 回看板、什么都没点、又直接 Enter 回同一个会话，
    // `sent_size` 还对得上，Resize 不发，人就诡异地卡在几十行之前翻到的
    // 地方——同一个「回来看看」的意图，走两条路给两种结果。
    //
    // 离开了再回来，用户要看的是最新输出，不是他上次读到一半的历史——
    // 一种可预期的行为好过两种碰运气的。所以这里直接、明确地发一次
    // `Scroll::Bottom`，不再借 `sent_size`/`Resize` 的副作用捎带出来。
    // 失败就静默：跟 `handle_key` 里滚动请求失败的处理一样，这不是用户
    // 当下敲的一个键，没反应分不清是卡顿还是断连，下一帧的 `Screen`
    // 探测自然会把 `connected` 标成假。
    let _ = app.client().and_then(|c| {
        c.call(Request::Scroll {
            id,
            by: crate::session::ScrollBy::Bottom,
        })
    });
}

/// 把守护进程报回来的一串警告码组成一行人话。
///
/// 拼接（`；`）发生在这里而不是 daemon 侧：daemon 连用哪种语言都不知道，
/// 更不知道该用哪个分隔符——中文用顿号式的全角分号，英文该用 `; `。
fn join_warnings(
    warnings: &[crate::proto::WarningCode],
    lang: crate::i18n::Lang,
) -> Option<String> {
    if warnings.is_empty() {
        return None;
    }
    let sep = match lang {
        crate::i18n::Lang::Zh => "；",
        crate::i18n::Lang::En => "; ",
    };
    Some(
        warnings
            .iter()
            .map(|w| crate::i18n::msg::warning(lang, w))
            .collect::<Vec<_>>()
            .join(sep),
    )
}

/// 浮层占的那块地方：各方向取「终端的一大半」和一个上限里的较小者，居中。
///
/// 设上限是因为在一块 200 列的屏幕上，一个铺满 80% 的对话框反而更难扫——
/// 眼睛要横跨整个屏幕才能把一行读完。
///
/// 终端小到放不下时**退化成全屏**，而不是显示一句「窗口太小」：选项目是
/// 用户此刻非做不可的事，挡住他没有意义（跟九宫格不同——那一屏本来就是
/// 「看一眼」，看不了就先别看）。
fn popup_area(area: Rect) -> Rect {
    // 再小就放不下一行「名字 + 路径」了。比这还小的终端，浮层已经没有
    // 「浮」的意义——全屏给他。
    const MIN_COLS: u16 = 40;
    const MIN_ROWS: u16 = 8;
    if area.width < MIN_COLS || area.height < MIN_ROWS {
        return area;
    }
    // **上限从 100 收到 72。** 那 100 是给左右两栏定的；现在只有一个列表，
    // 一行最长也就是「名字 + 一段路径」，再宽出去的部分全是空白——而一个
    // 横跨大半个屏幕的空框子，读起来比一个真正的对话框费劲得多。
    let w = (area.width.saturating_mul(4) / 5).clamp(MIN_COLS, 72);
    let h = (area.height.saturating_mul(3) / 4).clamp(MIN_ROWS, 24);
    Rect {
        x: area.x + (area.width - w) / 2,
        y: area.y + (area.height - h) / 2,
        width: w,
        height: h,
    }
}

/// `g` 键：在列表和九宫格之间切换，并且**记住**。
///
/// 两个模式共用这一个函数，切换和落盘绑在一起——分开写的话，某一边忘了
/// 落盘，用户就会遇到「有时记得、有时不记得」这种最难描述的 bug。
///
/// 落盘失败只在底栏说一句，不挡住切换本身：模式已经切了，屏幕上就是新的
/// 那个，硬要因为写不了文件把它切回去反而更费解。
pub(crate) fn toggle_view_mode(app: &mut App) {
    // 离开九宫格前先把列表光标对到焦点格上；`home_view` 负责反方向。
    // 两个视图对「当前是哪个会话」的认知必须一致，否则切一下模式，
    // 接下来的 `s`（停止）就毁在另一个会话上——这个键不可撤销。
    if matches!(app.view, View::Grid { .. }) {
        sync_board_cursor_from_grid(app);
    }
    app.view_mode = app.view_mode.toggled();
    app.view = home_view(app);
    let path = crate::settings::settings_path_for_socket(&app.socket);
    if let Err(e) = crate::settings::save_view_mode(&path, app.view_mode) {
        app.message = Msg::err(
            e.downcast_ref::<crate::proto::CodedError>()
                .map(|c| crate::i18n::msg::error(app.lang, &c.0))
                .unwrap_or_else(|| e.to_string()),
        );
    }
}

/// 用户选的那个模式对应的视图。**所有「回看板」的地方都必须走这里。**
///
/// 硬编码 `View::Board` 的每一处都是一个「明明在九宫格里，某次操作完却被
/// 甩回列表」的 bug——而且是偶发、复现不了的那种。
///
/// 顺带让两个模式对「当前是哪个会话」的认知保持一致：进九宫格时焦点落在
/// 列表光标那一行。反方向由 `sync_board_cursor_from_grid` 负责。
pub(crate) fn home_view(app: &App) -> View {
    match app.view_mode {
        ViewMode::List => View::Board,
        // 同样按会话 id 对——理由见 `sync_board_cursor_from_grid`。
        //
        // 光标那一行在九宫格里没有对应物时（停在组头上、或者停在一个已停止
        // 的会话上），退而问**同一个项目的第一格**——问的是
        // `grid::first_tile_of_current_group`，也就是 `Tab`/数字键换项目时
        // 用的那一份，不另写一份。
        //
        // 这一步不是锦上添花：`Tab`/`1`…`9` 按设计就把光标落在**组头**上
        // （spec 规则 3），所以「`Tab` 到 B 再按 `g`」是日常路径，不是边角。
        // 直接落回第 0 格的话，第一帧 `▶` 就在 A 的格子上，而底栏念的是 B——
        // `Enter`/`i`/`s`/`u`/`d` 作用在 A，`n`/`x` 作用在 B。
        //
        // 那个项目在九宫格里一个格子都没有时才落回第 0 格：这一支必须交出
        // 一个下标，而第 0 格至少是确定的。两个指针于是指着不同的项目，
        // 由 `sync_board_cursor_from_grid` 的守卫收尾。
        ViewMode::Grid => View::grid(
            app.selected_session()
                .and_then(|s| app.grid_sessions().iter().position(|g| g.id == s.id))
                .or_else(|| grid::first_tile_of_current_group(app))
                .unwrap_or(0),
        ),
    }
}

/// `l` 键：打开设置页，光标落在「语言」这一项上——它现在是设置项列表里的
/// 第一项，不再是语言本身的列表（见 `settings_view::SettingsItem`）。
pub(crate) fn open_settings(app: &mut App) {
    let mut state = ListState::default();
    state.select(Some(0));
    app.view = View::Settings { state, sub: None };
}

/// 光标移动的通用版本：只认列表长度，不认列表里装的是什么。
/// 项目选择器和会话看板共用它。
fn move_sel_n(st: &mut ListState, len: usize, delta: i32) {
    if len == 0 {
        st.select(None);
        return;
    }
    let cur = st.selected().unwrap_or(0) as i32;
    let next = (cur + delta).clamp(0, len as i32 - 1);
    st.select(Some(next as usize));
}

/// 上下走一行。行里既有组头也有会话行，两种都能停——空组只有组头，
/// 停不上去的话那个项目就永远选不中，`n` 也就去不了。
pub(crate) fn move_row(app: &mut App, delta: i32) {
    move_sel_n(&mut app.list_state, app.rows.len(), delta);
}

/// 跳到下 / 上一个项目的组头。到头回绕：四个项目里 `Tab` 转一圈回到起点，
/// 比「按到底就不动了」好解释。
pub(crate) fn jump_project(app: &mut App, delta: i32) {
    if app.groups.is_empty() {
        return;
    }
    let cur = app
        .list_state
        .selected()
        .and_then(|i| view::group_of(&app.rows, i))
        .unwrap_or(0) as i32;
    let n = app.groups.len() as i32;
    let next = (cur + delta).rem_euclid(n) as usize;
    goto_project(app, next);
}

/// 直达第 N 个项目（0 基）。越界什么都不做——按了 `7` 而只有三个项目时，
/// 不动比跳到最后一个更好懂。
pub(crate) fn goto_project(app: &mut App, gi: usize) {
    if gi >= app.groups.len() {
        return;
    }
    if let Some(i) = app.rows.iter().position(|r| *r == view::Row::Header(gi)) {
        app.list_state.select(Some(i));
    }
}

/// 折叠 / 展开光标所在的组。折完把光标收到组头上——不然它会停在一行
/// 已经不存在的会话上。
pub(crate) fn toggle_collapse(app: &mut App) {
    let Some(gi) = app
        .list_state
        .selected()
        .and_then(|i| view::group_of(&app.rows, i))
    else {
        return;
    };
    app.groups[gi].collapsed = !app.groups[gi].collapsed;
    app.rows = view::flatten(&app.groups);
    goto_project(app, gi);
}

/// 把列表光标指到某个会话所在的那一行。**「指向第 N 号会话」只有这一份
/// 实现**，所有需要它的入口（九宫格回列表、F3 跨会话跳）都走这里。
///
/// 各写各的迟早会分叉，而分叉出来的样子就是「光标留在原来那个项目上、
/// 人已经在另一个项目的会话里」——分组之后光标是「当前项目」唯一的答案处，
/// 它跟人不同步就等于屏幕在说谎。
///
/// 先在**组**里找，再换算成行：会话在 `rows` 和 `grid_sessions()` 两个集合
/// 里的下标没有任何对应关系（列表里夹着组头行），而**折叠的组根本不贡献
/// 会话行**（见 `view::flatten`）。只在 `rows` 里找的话，目标会话所在的组
/// 一旦是折叠的，这里就一行都找不到、光标一动不动——而屏幕上看得见的指针
/// （九宫格的 `▶`、F3 送进去的那个会话）已经走了。那正是「屏幕和状态各说
/// 各话」：底栏念着 A，`n` 开在 A，人却在 B 里。
///
/// **找到了就把那个组展开**，而不是退而求其次去选它的组头。两条理由：
///
/// - 组头行上 `selected_session()` 是 `None`，于是 `Enter`/`s`/`u`/`d` 全都
///   失去作用对象，底栏也不再写 `Enter`——用户明明正盯着那个会话（放大的
///   那一屏、`▶` 指着的那一格），屏幕却说「这里没有选中的会话」。折叠是一个
///   显示偏好，不该把这几个键吃掉。
/// - 走到这里的每一条路都是**用户按了键**：方向键挪格、F3 换会话、`x` 删组。
///   「后台事件不许换项目」那条不变量管的是没有按键的那一半，展开一个组是
///   这次按键看得见的后果，不是背着用户发生的。
///
/// 找不到（会话刚没了）就什么都不做——乱指一个比不动更糟。
pub(crate) fn point_cursor_at_session(app: &mut App, id: u32) {
    let Some((gi, si)) = app.groups.iter().enumerate().find_map(|(gi, g)| {
        g.sessions
            .iter()
            .position(|s| s.id == id)
            .map(|si| (gi, si))
    }) else {
        return;
    };
    if app.groups[gi].collapsed {
        app.groups[gi].collapsed = false;
        app.rows = view::flatten(&app.groups);
    }
    if let Some(i) = app
        .rows
        .iter()
        .position(|r| *r == view::Row::Session(gi, si))
    {
        app.list_state.select(Some(i));
    }
}

/// 离开九宫格之前，把列表光标挪到当前焦点格上。
///
/// 两个视图对「当前是哪个会话」的认知必须一致——`board.rs` 的 `g` 分支
/// 已经做了列表→九宫格那一半，这是反过来的另一半。少了它，用户盯着第 5 格
/// 回到列表时光标还停在第一行，下一个 `s`（停止）或 `u`（回滚）
/// 就毁在另一个会话上——这两个键都不可撤销，不能指望用户自己看出来。
///
/// 抽成函数是因为出口不止一个（`g`、Enter 放大）。
/// 不在九宫格里就什么都不做，调用方不必先判视图。
///
/// **只用在「用户没有碰过焦点」的那几个出口上**（`g`、`Enter`）。
/// 方向键那条路不走这里，它直接 `point_cursor_at_session`——理由见下面那条
/// 守卫，以及 `grid::point_cursor_at_focus`。
///
/// 守卫：**焦点指着的那一格，不属于光标此刻所在的项目时，什么都不做。**
///
/// 说清楚这条判据**实际**回答的是什么，别把它读成更大的承诺：它问的是
/// 「两个指针指的是不是同一个项目」，不是「焦点是不是陈旧的」。走到这里
/// 时两者不一致，来源有两类：
///
/// - **焦点动不了的那些落点**（见 `grid::first_tile_of_current_group`）：
///   `Tab`/数字键走到一个没有活会话的项目、`p` 刚摆上一个新项目、
///   `home_view` 因为当前项目在九宫格里一个格子都没有而回落到第 0 格。
///   这几种情况下 `focus` 指的是**上一个**项目，判光标赢就是对的。
/// - **下标平移**：另一个 dct 窗口 prune 掉一个会话之后，格子整体前移，
///   `focus` 这个下标不再指着原来那一格。这条守卫**盖不住**它——它只比
///   「两个指针是不是同一个项目」，平移到同项目的另一格照样放行，而画在
///   屏幕上的 `▶` 可能已经落到别的项目的格子上了。那是这一版之前就有的
///   老问题（`Enter` 放大的是漂过去的那一格），明确不在射程内，也没在
///   这一版里修。
///
/// 方向键那条路不经过这里（它永远两个一起挪），所以上面两类之外没有第三类。
///
/// 一旦不一致，这条判据一律**判光标赢**。这是对的，但理由不是「光标更正确」，
/// 而是它更**耐久**：`refresh_rows` 是按身份（锚点）把光标找回原位的，而对
/// `focus` 只做了一次下标夹取（`app.rs` 里 `min(grid_last)`）。会话增删导致
/// 下标平移时，光标还指着原来那个东西，`focus` 已经指到别的格子上去了。
///
/// 判据**不能**换成「这个组有没有活格子」（曾经就是那么写的）。两个问题只在
/// 一种场景下答案相同，往两个方向都会岔开，而且两边都咬人：
///
/// - 会话**全停**的项目也没有活格子，但用户在那儿按方向键是一次显式的指点
///   动作，光标必须跟着走。按「有没有活格子」挡的话，光标会被冻在那个已停止
///   的会话上，接下来的 `s`/`u` 就毁在它身上——而这两个键都不可撤销，正是
///   本函数开篇警告的那种事故。
/// - 反过来，那个空项目一旦拿到会话（后台轮询捎回来的、另一个 dct 窗口开的），
///   「有没有活格子」立刻变成「有」，守卫就开了——可**没有任何东西会把焦点
///   挪进那个项目**，它还是旧的，`g` 照样把用户送回上一个项目。
///
/// 判断从**数据**上现算，不另立一个「焦点是陈旧的」标志位：标志位得在每一次
/// 焦点移动、每一次 `refresh_rows` 收拢焦点之后记得清掉，漏一处就又是同一个
/// bug，而且不会有任何编译期信号。现算的这条在所有调用点上同时成立，包括
/// 以后新加的出口。
pub(crate) fn sync_board_cursor_from_grid(app: &mut App) {
    let View::Grid { focus, .. } = app.view else {
        return;
    };
    // **按会话 id 对，不按下标。** 九宫格不画已停止的会话，所以两个集合的
    // 下标根本对不上——第 2 格可能是列表的第 4 行。对错了的话，回列表后
    // 接下来的 `s`（停止）或 `u`（回滚）就毁在另一个会话上，而这两个键都
    // 不可撤销。
    let Some(id) = app.grid_sessions().get(focus).map(|s| s.id) else {
        return;
    };
    // 焦点那一格属于光标所在的组吗？不属于 = 焦点是陈旧的，别拿它改写光标。
    if !app
        .current_group()
        .is_some_and(|g| g.sessions.iter().any(|s| s.id == id))
    {
        return;
    }
    point_cursor_at_session(app, id);
}

/// `n`（开上次那个 agent）/ `N`（挑一个 agent）。
///
/// 看板和九宫格是同一块看板的两种画法，这四个「开东西」的键
/// （`n`/`N`/`p`/`c`）在两边必须一模一样，所以整段逻辑只留一份。
/// `code` 区分大小写 n：小写才去问 daemon 上次记的是哪个 agent。
pub(crate) fn open_new_session(app: &mut App, code: KeyCode) {
    // entries 带的是完整信息（label/note/status/密钥提示/安装提示），
    // 渲染时把置灰项和原因画出来、四种状态各自路由到哪，见
    // pick_action 和 View::PickProfile 的按键分支。n 和 N 都要这份
    // 列表——n 拿它判断上次那个 agent 现在还在不在 Ready，N 拿它渲染
    // 选择器——所以只拉一次，不分两条路各拉各的。
    let lang = app.lang;
    // 开这一屏之前问一次「当前项目是不是 git 仓库」，问一次就够——
    // 理由（每帧现算等于每秒开十几个 git 子进程）见 `View::PickProfile::no_git`。
    let no_git = current_is_not_a_repo(app);
    match app
        .client()
        .and_then(|c| c.call(Request::Profiles { lang }))
    {
        Ok(Response::Profiles { entries, warnings }) => {
            let warning = join_warnings(&warnings, app.lang);
            // 同上：这一屏是「挑一个 agent 开会话」，纯后端没有会话可开。
            let entries = view::agent_rows(&entries);
            // 把「拉完列表但没能直开」的三种落点（选择器为空、建会话失败
            // 两种）收在一处，省得同一段 ListState 初始化抄三遍——那种
            // 抄法迟早有一份漏了空表守卫。
            let picker = |entries: Vec<ProfileEntry>, warning: Option<String>| {
                let mut state = ListState::default();
                // daemon 目前总是至少返回九个内置 profile，这里空表分支
                // 基本走不到；但选中一个不存在的下标，按 Enter 就是
                // entries[0] 越界 panic——这种最坏结果不该只靠"实践中
                // 到不了"兜底，一行守卫不值钱。
                if !entries.is_empty() {
                    state.select(Some(0));
                }
                View::PickProfile {
                    entries,
                    state,
                    warning,
                    no_git,
                }
            };
            // 大写 N 一定要看一眼选择器，不查上次用的是谁；
            // 小写 n 才去问 daemon 上次记的是哪个 agent。
            let last = if code == KeyCode::Char('n') {
                let dir = app.current_dir().display().to_string();
                match app
                    .client()
                    .and_then(|c| c.call(Request::LastProfile { dir }))
                {
                    Ok(Response::LastProfile(l)) => l,
                    _ => None,
                }
            } else {
                None
            };
            match quick_start_target(last.as_deref(), &entries) {
                Some(name) => {
                    // 同 View::PickProfile 里 PickAction::Start 那支：
                    // 「n」等价于「已经替用户选好了上次那个」，建完直接
                    // 进会话，不用再让他确认一遍。
                    let dir = app.current_dir().display().to_string();
                    // 缓存由 `create_session` 自己跟上，这里不再各记一份。
                    match create_session(app, &dir, &name, true) {
                        Ok(Response::Created { id }) => {
                            app.need_sessions = true; // 会话标题要显示项目名
                            app.view = View::Attached(id);
                        }
                        Ok(Response::Error(ref e)) => {
                            app.message = Msg::err(crate::i18n::msg::error(app.lang, e));
                            app.view = picker(entries, warning);
                        }
                        _ => {
                            app.message = Msg::err(
                                crate::i18n::text(crate::i18n::Key::CreateFailed, app.lang).into(),
                            );
                            app.view = picker(entries, warning);
                        }
                    }
                }
                None => app.view = picker(entries, warning),
            }
        }
        // 列表都拿不到，直开和选择器都没法走，只能告诉用户这次干瞪眼——
        // 视图不变，走到循环末尾 message_after_transition 会把这条消息
        // 原样留住（同其他分支，不用 continue 抢跑跳过收尾）。
        Ok(Response::Error(ref e)) => app.message = Msg::err(crate::i18n::msg::error(app.lang, e)),
        _ => {
            app.message =
                Msg::err(crate::i18n::text(crate::i18n::Key::CannotListAgents, app.lang).into())
        }
    }
}

/// `p` 的目录浏览器从哪儿开起：**当前项目的上一级**。用户按 `p` 多半是想
/// 去「旁边那个」项目，从上一级开始找是最短的路。
///
/// 用组的 `display_dir`（用户敲的那条），**不是** `current_dir()`——后者是
/// 分组键，已经 canon 过：macOS 上用户 pin 的 `/tmp/x` 会让浏览器顶着
/// `/private/tmp` 打开，而他从没见过这个路径。canon 只用于比较。
fn browse_start_dir(app: &App) -> PathBuf {
    let cur = app
        .current_group()
        .map(|g| g.display_dir.clone())
        // 一个组都没有（还没拉到列表的头一帧）：退回启动目录，同 `current_dir`。
        .unwrap_or_else(|| app.start_dir.clone());
    cur.parent().map(|x| x.to_path_buf()).unwrap_or(cur)
}

/// `p`：换项目。看板和九宫格共用，同 `open_new_session`。
pub(crate) fn open_project_picker(app: &mut App) {
    // 拿不到列表就不进选择器：进去看见一片空白，用户会以为
    // 自己从来没开过项目。
    match app.client().and_then(|c| c.call(Request::Projects)) {
        Ok(Response::Projects {
            recent: mut all, ..
        }) => {
            // 全新守护进程列表是空的，补上启动目录，
            // 保证第一次用也不会看到空列表。
            let start = app.start_dir.display().to_string();
            if !all.contains(&start) {
                all.push(start);
            }
            app.view = View::PickProject(crate::ui::view::ProjectPicker::new(
                all,
                browse_start_dir(app),
            ));
        }
        Ok(Response::Error(ref e)) => app.message = Msg::err(crate::i18n::msg::error(app.lang, e)),
        _ => {
            app.message =
                Msg::err(crate::i18n::text(crate::i18n::Key::CannotListProjects, app.lang).into())
        }
    }
}

/// `c`：密钥设置页。看板和九宫格共用，同 `open_new_session`。
pub(crate) fn open_secrets(app: &mut App) {
    // 拿不到列表就不进设置页：留在原地给一句错误，总比弹进一个既没数据、
    // 又没地方显示错误的空白页强（`View::Secrets` 没有 `warning` 字段，
    // 见其字段注释）。
    let lang = app.lang;
    match app
        .client()
        .and_then(|c| c.call(Request::Profiles { lang }))
    {
        Ok(Response::Profiles { entries, .. }) => {
            let mut state = ListState::default();
            if !secret_rows(&entries).is_empty() {
                state.select(Some(0));
            }
            app.view = View::Secrets {
                entries,
                state,
                pending_delete: None,
            };
        }
        Ok(Response::Error(ref e)) => app.message = Msg::err(crate::i18n::msg::error(app.lang, e)),
        _ => {
            app.message =
                Msg::err(crate::i18n::text(crate::i18n::Key::CannotListSecrets, app.lang).into())
        }
    }
}

/// 对某个会话做 `s`（停止）/ `u`（回滚）/ `d`（看改动），返回要显示的消息。
///
/// 看板和九宫格是同一套语义作用在不同的「当前会话」上（列表是选中行，
/// 九宫格是焦点格），所以发请求和拼消息这段只留一份：两边各抄一份的话，
/// 哪天改了 diff 的措辞或者错误分支，只会改到其中一半。
///
/// `code` 之外的按键返回空消息——调用方只在这三个键上调它，落到那条兜底
/// 说明分派写漏了；这时候不动 `message` 比编一句话给用户看更诚实。
pub(crate) fn session_action(app: &mut App, code: KeyCode, id: u32) -> Msg {
    let req = match code {
        KeyCode::Char('s') => Request::Stop { id },
        KeyCode::Char('u') => Request::Undo { id },
        KeyCode::Char('d') => Request::Diff { id },
        _ => return "".into(),
    };
    match app.client().and_then(|c| c.call(req)) {
        Ok(Response::Ok) => crate::i18n::text(crate::i18n::Key::ActionDone, app.lang).into(),
        Ok(Response::Diff(v)) if v.is_empty() => {
            crate::i18n::text(crate::i18n::Key::NoChanges, app.lang).into()
        }
        Ok(Response::Diff(v)) => v
            .iter()
            .map(|f| format!("{} +{} -{}", f.path, f.added, f.removed))
            .collect::<Vec<_>>()
            .join("  ")
            .into(),
        Ok(Response::Error(ref e)) => Msg::err(crate::i18n::msg::error(app.lang, e)),
        _ => Msg::err(crate::i18n::text(crate::i18n::Key::RequestFailed, app.lang).into()),
    }
}

/// 密钥页要展示的数据总在变——改完一条、删完一条都要照一份新的 `has_secret`
/// 才对得上。改/删/刚打开页面这三个调用点都要拉同一份数据，区别只在光标
/// 该落在哪：`focus` 给了 profile 名字就尽量把光标定在它原来那一行上
/// （删完/改完还盯着同一个 profile，比每次都弹回第一行顺手），不给就落在
/// 第一行（刚打开页面，没有"原来"）。
///
/// 拉取失败时退化成一个空 `entries` 的壳——同各视图 Esc 分支对
/// `PickProfile`/`Secrets` 的约定：循环收尾那段通用重拉逻辑看到空壳会自己
/// 再补一次，这里不需要重复一份「失败了怎么办」的判断。
fn refetch_secrets(app: &mut App, focus: Option<&str>) -> View {
    // 直接查字段而不是走 `app.client()`：调用方往往还在同一个 `match` 里
    // 借着 `app` 的别的字段（比如 `entries`/`state` 已经从 `app.view` 解构
    // 出来），走一个吃 `&mut self` 的方法会跟这些借用打架。`None` 归到跟
    // 下面 `_ =>` 一样的失败落点——同真实断线共用一条路径，不新增分支。
    let lang = app.lang;
    let result = app
        .client
        .as_mut()
        .map(|c| c.call(Request::Profiles { lang }));
    match result {
        Some(Ok(Response::Profiles { entries, .. })) => {
            let rows = secret_rows(&entries);
            let mut state = ListState::default();
            if !rows.is_empty() {
                let idx = focus
                    .and_then(|name| rows.iter().position(|(n, _)| n == name))
                    .unwrap_or(0);
                state.select(Some(idx));
            }
            // 重拉之后不管改的还是删的都已经落定，武装状态没有意义可言了
            // ——不管刚才 pending_delete 是什么，新的一屏都从「没有武装」
            // 开始。
            View::Secrets {
                entries,
                state,
                pending_delete: None,
            }
        }
        _ => View::Secrets {
            entries: Vec::new(),
            state: ListState::default(),
            pending_delete: None,
        },
    }
}

/// 左段固定占的列数：最长的一条是英文密钥页的 "Esc settings" = 12。
/// 中文那边最长的是「Esc 回看板」= 3 + 1 + 中文 3 字 × 2 = 10，会话视图的
/// 「F2 回看板」只有 9。写死而不是每帧算：左段宽度跟着文案跳动会让右段的
/// 消息忽宽忽窄。`escape_hint_cols_fits_every_view` 会把这个数钉死在
/// 「正好等于最长文案」上，改文案就得跟着改这里。
const ESCAPE_HINT_COLS: u16 = 12;

/// 底栏中段：当前项目占的列数。按显示宽度截断，CJK 项目名同样算两列。
///
/// 24 列是「放得下那个名词 + 一个常见项目名 + 一段父目录」跟「别把右段挤没」
/// 之间的折中，怎么用满见 `bar_chip` 和 `widgets::project_label`。
///
/// 从 16 提到 24 是牌子加上名词（`project` 8 列 / `项目` 5 列，含分隔的空格）
/// 那次：16 列的预算减掉牌子自己两列留白之后只剩 14，而一个再普通不过的
/// 11 列项目名加上英文那个名词就是 19——名词于是**永远**让不到位，改动等于
/// 没做。这个数是照最窄的那一档（80 列）反推的，见
/// `the_three_actions_all_fit_at_eighty_columns`：80 列上左段 14、中段 26、
/// 右段还剩 40，离 `ACTION_MIN_COLS` 的 28 还有余量。
const PROJECT_COLS: u16 = 24;

/// 右段无论如何要留的列数。
///
/// 28 = 英文那条滚动提示的完整宽度（`i18n::msg::scrolled_up`，
/// "↑ Scrolled up 40 line(s) · press End to jump back down" 缩短之后的版本）。
/// 它是右段里唯一「少一个字就没法用」的内容：按键表放不下的键在 `?` 后面
/// 找得回来，普通消息还能折行，只有这一句既折不了（整句都是单空格，
/// `wrap_help` 只认双空格断点）又必须读全——用户正翻在历史里，这句话是他
/// 回到底部的唯一说明。由 `the_way_back_survives_a_narrow_terminal` 盯着。
///
/// `pub(crate)`：`i18n.rs` 里 `CopyModeShort` 的守卫测试也要拿它当基准——
/// 复制模式提示是右段另一条「少一个字就没法用」的内容（全屏唯一写着 F4
/// 的地方），短文案必须放得进这同一条底线。
pub(crate) const ACTION_MIN_COLS: u16 = 28;

/// 底栏三段各占多少列（`inner` 是去掉左右边框之后的可用宽度）。
///
/// 抽成纯函数是因为这个数要用两遍：一遍算「右段折出几行」（底栏高度按它留），
/// 一遍真的切布局。两处各算一次的话，宽度算法一旦分叉，留的行和画的行就对
/// 不上，句尾会被静默吃掉。
///
/// 让位顺序是**中段先让、左段永不让**：左段是用户卡住时唯一的出路，右段至少
/// 得留下那扇 `?` 门。注意这里让的是**终端太窄**，跟「消息/断连提示能不能盖
/// 掉中段」是两码事——后者的答案永远是不能，那件事由布局本身保证：消息只画
/// 在右段里。
fn bar_widths(inner: u16) -> (u16, u16, u16) {
    let escape = (ESCAPE_HINT_COLS + 2).min(inner); // +2 是和中段之间的间隔
    let rest = inner - escape;
    let project = (PROJECT_COLS + 2).min(rest.saturating_sub(ACTION_MIN_COLS));
    (escape, project, rest - project)
}

/// 画一帧界面。内容区（`chunks[0]`）按当前视图分派给各自模块的 `draw`；
/// 底部栏（`chunks[1]`：逃生键 + 消息/帮助文案）不分视图，统一在这里画。
fn draw(f: &mut Frame, app: &mut App) {
    // 提示必须跟着视图走。底部栏原来不分视图，进了会话仍写着看板的按键表，
    // 而那些键在会话视图里全部被转发给 agent——用户照着按 n，字母 n 会落进
    // Claude Code 的输入框。显示做不到的操作比不显示更糟。
    //
    // 逃生键那一截已经挪进左段常驻，这里不再重复。
    //
    // 算在布局之前：底栏要多高，取决于这段文字折成几行。
    //
    // 按键表和消息走两条路：按键表是一排结构化的条目（键名要加粗、放不下的
    // 按优先级丢，永远一行），消息是一句人话（一个字都不能丢，宁可折行）。
    //
    // 右段可用宽度先算出来：`bar_keys` 要拿它决定 `n` 那条写不写得下 agent 名。
    // 跟下面切 `bar` 时算的是同一个数（同一个函数），不一致的话，按高度预留的
    // 行数就对不上真正折出来的行数，末尾几个键照样会被吃掉。
    // 底栏这个块只画上下边框（`Borders::TOP | Borders::BOTTOM`），左右
    // 不再吃掉列——复制文字不该带上边框字符。这里因此直接用整个宽度，
    // 不用像改动前那样再减 2 补偿左右边框。
    let (_, _, action_cols) = bar_widths(f.area().width);
    let help_cols = action_cols as usize;
    let (bar, style) = if !app.connected {
        (
            BarContent::Text(crate::i18n::text(crate::i18n::Key::StaleData, app.lang).to_string()),
            danger(),
        )
    } else if app.message.text.is_empty() {
        // 会话视图里，滚动提示是持续状态（「翻到哪儿了」「下面有新内容」），
        // 按键表是「还能干什么」——两者抢的是同一行，而滚动提示更具体。
        // 只在 `message` 为空这一支里问它：`message` 优先在外层 if/else
        // 链上已经保证了（见函数头注释），这里不用重复判断。
        // 复制模式压过滚动提示：这时候滚轮根本不归 dct 管，那条提示是错的。
        // 压不过错误消息——外层 if/else 链已经保证了这一点。
        let hint = match &app.view {
            // 浮层开着时这一行归浮层的三个键（`bar_keys`）——滚动提示写的是
            // 「翻到哪儿了」，而这时候翻页键根本不归 agent 也不归 dct 的滚动，
            // 归浮层。
            View::Attached(_) if app.theme_pick.is_some() => None,
            View::Attached(_) if app.copy_mode => {
                // 底栏右段只保证 ACTION_MIN_COLS 列，而这条提示是全屏唯一
                // 写着 F4 的地方——截掉尾巴等于把用户关在一个看不见也出不去
                // 的模式里。长文案说清楚「为什么」（鼠标交还给了终端），量
                // 得下就用；量不下换成放得进 ACTION_MIN_COLS 的短文案，两种
                // 语言都保证放得下（`copy_mode_short_fits_the_action_floor_in_every_language`）。
                let long = crate::i18n::text(crate::i18n::Key::CopyMode, app.lang);
                let chosen = if widgets::display_width(long) <= help_cols {
                    long
                } else {
                    crate::i18n::text(crate::i18n::Key::CopyModeShort, app.lang)
                };
                Some(chosen.to_string())
            }
            View::Attached(_) => attach::scroll_hint(&app.scroll, app.lang),
            _ => None,
        };
        match hint {
            Some(h) => (BarContent::Text(h), Style::default()),
            None => (BarContent::Keys(bar_keys(app, help_cols)), Style::default()),
        }
    } else if app.message.error {
        (BarContent::Text(app.message.text.clone()), danger())
    } else {
        (BarContent::Text(app.message.text.clone()), Style::default())
    };

    // 按键表永远一行：多一行底栏就多吃一行内容区，而九宫格在 80×24 下只差
    // 一行就跌破 `grid.rs` 的 `MIN_ROWS`，整屏换成一句「窗口太小」。放不下的
    // 键落进 `?` 浮层，不再挤占版面（见 `widgets::fit_help`）。
    let help_lines: Vec<Line> = match &bar {
        BarContent::Keys(items) => vec![Line::from(widgets::help_spans(&widgets::fit_help(
            items, help_cols,
        )))],
        BarContent::Text(t) => widgets::wrap_help(t, help_cols)
            .into_iter()
            .map(Line::from)
            .collect(),
    };

    // 底栏高度 = 上下边框 + 文案真正折出来的行数。
    //
    // 这里原来写死 4 行（两行文字），于是「按键表能不能放下」变成了一件
    // 靠人算、靠单测事后兜的事：往表里加一个键，末尾的 `d 改动` 就被右端
    // 悄悄截掉，而 u/s/d 里有两个是不可撤销的操作。屏幕上没写却真的管用的
    // 键，就是等着用户误按。按内容算高度之后，这一类 bug 在结构上不再可能
    // 发生——按键表恒为一行，只有长消息才会把底栏撑高。
    //
    // 上限三分之一屏：再窄的终端里还是会截，但那时候继续让位只会把内容区
    // 挤没。
    // 实色档留**一行空白**在色条上方，横线档要上下两行边框。
    //
    // 那一行空白不是装饰：色条直接贴着 agent 画面最后一行时，两块东西挤成
    // 一坨，眼睛分不出哪儿是 agent 的输出、哪儿是 dct 的按键表——而 agent
    // 的内容在哪一行结束是它自己定的，dct 这边没法指望它留白。横线档不需要
    // 这一行，因为那条横线自己就是视觉间隔。
    //
    // 也别把这行空白改成「色条高两行、文字居中」：那样色条会占掉两行的
    // 底色面积，比现在更抢眼，而多出来的面积一个字都不承载。
    let bar_chrome = if bar_style(app.bar).is_some() { 1 } else { 2 };
    let bar_of = |lines: u16| {
        let h = (lines.max(1) + bar_chrome).min((f.area().height / 3).max(3));
        Layout::vertical([Constraint::Min(3), Constraint::Length(h)]).split(f.area())
    };
    let chunks = bar_of(help_lines.len() as u16);

    // **会话画面的高度不跟着底栏的行数走。** 底栏是按文案真正折出几行算高的
    // （见上面那段），于是一句折成两行的消息会把会话内容区挤矮一行——而
    // 内容区的高度就是 dct 发给 agent 的 `Resize`（见 `run()` 里那段）。
    // 一条转瞬即逝的提示因此变成「窗口矮一行、消息过期又高回来」两次真的
    // 改尺寸：agent 每次都要整屏重绘一遍，屏幕上看得见跳动，而 Claude Code
    // 那种按上一帧行数往上抬光标的渲染器，抬错一次就把输入框画到别处去。
    //
    // 所以这一档消息**盖**在会话画面上，不去改它的尺寸：底栏长高的那一两行
    // 暂时遮住 agent 画面的最后一两行，消息一过就露出来。遮住是可逆的，
    // 改尺寸不是。只有会话视图这么办——别的视图里内容是 dct 自己画的，
    // 挤矮一行没有任何代价。
    let attached = bar_of(1)[0];

    // 穷尽匹配而不是 if/else 链：少一个 View 变体的分支，if/else 链的兜底
    // `else` 会悄悄把新变体也归给 secret::draw，画出一片空白也照样编译通过；
    // match 会在加变体的那一刻直接编译报错，逼着调用点补上。跟 `run()` 里
    // 按键分发那个 `match app.view.clone()` 用的是同一个理由，这里同样
    // 必须 `.clone()`——各分支要把 `app` 借给 `board::draw`/`pick::draw`
    // 等函数，`match &app.view` 留着的借用会跟这些调用打架。
    match app.view.clone() {
        View::Board => board::draw(f, chunks[0], app),
        View::Attached(_) => attach::draw(f, attached, app),
        View::Grid { .. } => grid::draw(f, chunks[0], app),
        View::PickProfile { .. } => pick::draw(f, chunks[0], app),
        // 选项目是**浮层**：先照常画用户的家视图，再在中间盖一层。
        // 背后留着看板是重点——用户要看得出自己只是叠了一层，Esc 一下
        // 就回去了。全屏接管正是上一版被判为「混乱」的一部分。
        View::PickProject(_) => {
            let home = home_view(app);
            let saved = std::mem::replace(&mut app.view, home);
            match app.view {
                View::Grid { .. } => grid::draw(f, chunks[0], app),
                _ => board::draw(f, chunks[0], app),
            }
            app.view = saved;
            let popup = popup_area(chunks[0]);
            f.render_widget(ratatui::widgets::Clear, popup);
            pick::draw(f, popup, app);
        }
        // 全部按键也是**浮层**，跟项目选择器同一种呈现：背后留着按 `?` 之前
        // 那一屏，用户看得出自己只是叠了一层。背后画的是 `from` 而不是
        // `home_view()`——从九宫格进来就该看见九宫格，连焦点格都不该动。
        View::Keys { from } => {
            let saved = std::mem::replace(&mut app.view, (*from).clone());
            match app.view {
                View::Grid { .. } => grid::draw(f, chunks[0], app),
                _ => board::draw(f, chunks[0], app),
            }
            app.view = saved;
            keys::draw(f, chunks[0], app);
        }
        View::EnterSecret { .. } | View::Secrets { .. } => secret::draw(f, chunks[0], app),
        View::Settings { .. } => settings_view::draw(f, chunks[0], app),
        View::Phone { .. } => phone::draw(f, chunks[0], app),
        View::Web => web::draw(f, chunks[0], app),
        View::Pair { .. } => pair_view::draw(f, chunks[0], app),
    }

    // 边框上不再挂「当前项目：…」这个标题：标题跟框内是两块地方，而用户
    // 要的是「我在哪」和「这里能干什么」挨在一起读。项目现在是框内的中段。
    // 实色档：整条底栏铺一层背景，不画边框。铺的是 `chunks[1]` 整块而不是
    // 逐段刷——三段之间的间隔列也得有底色，否则色条会被切成三截。
    // 底栏这一块先擦干净再画。平时是空操作（每帧的缓冲本来就是干净的），
    // 只有会话视图里底栏比常态高的那几帧不是——那时候它盖着 agent 画面的
    // 最后一两行（见上面 `attached` 那段），不擦就是两层字叠在一起。
    f.render_widget(ratatui::widgets::Clear, chunks[1]);
    let inner = match bar_style(app.bar) {
        Some(bar) => {
            // 上面那一行留白，色条铺在剩下的部分。留白**不铺底色**——它要的
            // 就是和终端背景一样，才起得到间隔的作用。
            let rows =
                Layout::vertical([Constraint::Length(1), Constraint::Min(0)]).split(chunks[1]);
            f.render_widget(Block::default().style(bar), rows[1]);
            rows[1]
        }
        None => {
            let block = Block::default().borders(Borders::TOP | Borders::BOTTOM);
            let inner = block.inner(chunks[1]);
            f.render_widget(block, chunks[1]);
            inner
        }
    };

    // 横向拆三段，各有各的固定职责：**逃生键 / 当前项目 / 至多三个动作**。
    // 前两段永不让位，只有第三段会被消息、断连提示、宽度不够替换掉。
    //
    // 拆之前是一整行按优先级二选一，于是「已切到 X」这类完全正常的操作反馈
    // 会把整张按键表连同「q 退出」一起顶掉，而消息只在切视图时才清——用户
    // 不知道怎么切视图正是他卡住的原因，于是退出提示永久消失。项目名后来
    // 挂在边框标题上，同一句消息照样能盖掉它（标题只有一行，消息一长就顶
    // 上去了），用户于是既不知道自己在哪，也不知道 `n` 会开在哪。
    // 拆成三段之后这两件事在结构上都不可能再发生。
    let (esc_w, project_w, _) = bar_widths(inner.width);
    let bar = Layout::horizontal([
        Constraint::Length(esc_w),
        Constraint::Length(project_w),
        Constraint::Min(0),
    ])
    .split(inner);
    f.render_widget(
        Paragraph::new(escape_hint(&app.view, app.lang)).style(accent()),
        bar[0],
    );
    // 中段：当前项目。**永不让位。**
    //
    // 写的是组的 `name`/`parent`（来自**未归一化**的原始路径），不是
    // `current_dir()`——后者是分组键，已经 canon 过，`/tmp/x` 会显示成
    // `/private/tmp/x`：归一化只用于比较，永不用于显示。
    let (name, parent) = match app.current_group() {
        Some(g) => (g.name.clone(), g.parent.clone()),
        // 一个组都没有（只可能是还没拉到会话列表的头一帧）：退回启动目录。
        // 中段宁可写个稍后会被修正的地方，也不能空着——空着的那一帧里，
        // 用户看不出 `n` 会把新会话开在哪。
        None => (
            app.start_dir
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_else(|| app.start_dir.display().to_string()),
            app.start_dir
                .parent()
                .map(|p| short_path(&p.display().to_string()))
                .unwrap_or_default(),
        ),
    };
    // **反白成一块牌子**，不是加粗了事。加粗的项目名跟旁边同样加粗的按键名
    // 在同一条底栏上长得几乎一样，用户要先知道「中段是项目」才认得出它——
    // 而这正是他不知道的那件事。反白是这一整屏里唯一一块底色反过来的地方，
    // 不需要任何先验知识就跳出来。
    //
    // 用 `REVERSED` 而不是挑一个具名色：底栏六档配色（`BarTheme`）的底色从
    // 236 到 253 都有，`Light` 那档还是深字压浅底。任何写死的前景色都会在
    // 某一档上糊掉，而反白是相对当前底色定义的，六档全都对。`Lines` 档没有
    // 实色底，反白拿到的是终端自己的默认前景/背景，同样是一块牌子。
    //
    // 前后各垫一个空格：反白的字紧贴着别的字会看不出边界。这两列从**文字
    // 预算**里出，不从段宽里出——段宽是布局的事，改它会连带动到右段。
    let chip = bar_chip(
        &name,
        &parent,
        project_w.saturating_sub(4) as usize, // -2 和右段的间隔，-2 牌子自己的留白
        app.lang,
    );
    f.render_widget(
        Paragraph::new(Line::from(Span::styled(
            format!(" {chip} "),
            Style::default().add_modifier(Modifier::REVERSED | Modifier::BOLD),
        ))),
        bar[1],
    );
    // 折行而不是截断：截断会把句尾那几个键悄悄抹掉，而用户没有任何线索
    // 知道自己少看了几个键。折行用 `wrap_help` 而不是 ratatui 的 `Wrap`，
    // 理由见那个函数——`Wrap` 会把「Tab 换项目」拆成行尾一个孤零零的 `Tab`。
    //
    // 用上面按 `help_cols` 折好的那一份，不在这里重折：底栏高度就是按它
    // 的行数留的，两处各折一次的话，宽度算法一旦分叉，留的行和画的行就对
    // 不上了。
    debug_assert_eq!(
        help_cols, bar[2].width as usize,
        "预留底栏高度用的宽度必须跟真正画的宽度一致"
    );
    f.render_widget(Paragraph::new(help_lines).style(style), bar[2]);
}

/// 底栏中段牌子上的字：`项目 dc/dc-terminal`，写不下名词就只写路径。
///
/// 名词（`project` / `项目`）是从**文字预算**里出的，不是从段宽里出的——
/// 段宽是布局的事，动它会连带把右段那三个键挤掉一个（`bar_widths` 里
/// `PROJECT_COLS` 和 `ACTION_MIN_COLS` 的那笔账）。所以窄下来时让位的
/// 顺序是：父目录（`project_label` 自己退），然后这个名词，最后才是名字
/// 里的字符。**牌子本身在任何宽度上都在场**：空着的那一帧里，用户看不出
/// `n` 会把新会话开在哪。
///
/// 名词让位的门槛是「名字自己还写得全吗」，不是「加上名词还剩几列」：
/// 名词占了 8 列却把项目名截成 `dc-ter…`，等于用一个所有项目都一样的词
/// 换掉了唯一能区分项目的信息。
fn bar_chip(name: &str, parent: &str, cols: usize, lang: crate::i18n::Lang) -> String {
    let label = crate::i18n::text(crate::i18n::Key::ProjectChipLabel, lang);
    let need = widgets::display_width(label) + 1;
    if need + widgets::display_width(name) <= cols {
        format!(
            "{label} {}",
            widgets::project_label(name, parent, cols - need)
        )
    } else {
        widgets::project_label(name, parent, cols)
    }
}

/// 底栏右段这一帧的按键表，`n` 那一条带上这个项目上次用的 agent 名。
///
/// 抽出来是因为 `idle_help` 给的是**静态词条**（`&'static str` 的表），而
/// agent 名是运行时才知道的——只能在这一层替换掉。看得见 agent 名是有用的：
/// 按 `n` 到底会开出什么，用户不必先记住自己上次在这个项目里用的是谁。
///
/// `cols` 是右段的可用宽度：**agent 名放不下就不写**。让位的是一句说明的
/// 后半截，不是一个键——三个键在任何宽度上都原样在场。这条区分是这次改造
/// 的要点：一个**键**在 100 列上有、在 80 列上没有，用户就学不会这一行；
/// 而 `n 新建` 后面少一个名字，他仍然知道 `n` 是新建，按下去还会弹选择器。
fn bar_keys(app: &App, cols: usize) -> Vec<crate::i18n::HelpItem> {
    // 配色浮层开着时，底栏写的是浮层自己那三个键。会话那四条 F 键这时候
    // 一个都按不动（`attach::handle_key` 先把键交给浮层），继续写着它们
    // 就是在宣传按不动的键——这个仓库反复警惕的正是这件事。
    if app.theme_pick.is_some() {
        return crate::i18n::help_items(
            &[
                ("↑↓", crate::i18n::Key::Select),
                ("Enter", crate::i18n::Key::Confirm),
                ("Esc", crate::i18n::Key::Cancel),
            ],
            app.lang,
        );
    }
    let items = idle_help(&app.view, app.lang, help_ctx(app));
    // 这个项目从没开过会话就只写 `n 新建`——按下去会弹 agent 选择器，
    // 那正是该有的行为，所以不必编一个名字出来。
    let Some(agent) = app.current_group().and_then(|g| g.last_profile.clone()) else {
        return items;
    };
    let mut named = items.clone();
    for it in named.iter_mut() {
        if it.key == "n" {
            // `n 新建 claude`——括号去掉，agent 名直接跟在后面。底栏每一列
            // 都金贵，一对括号占两列却什么都没说。
            it.label_owned = Some(format!("{} {}", it.label, agent));
        }
    }
    if help_width(&named) <= cols {
        named
    } else {
        items
    }
}

/// 一排提示连着分隔符一共占几列。跟 `widgets::fit_help` 的算法必须一致——
/// 一个说「放得下」另一个说「放不下」的话，多出来的那截会被右端静默截掉。
fn help_width(items: &[crate::i18n::HelpItem]) -> usize {
    items.iter().map(widgets::item_width).sum::<usize>() + 2 * items.len().saturating_sub(1)
}

/// 底栏右段这一帧要显示的东西。
///
/// 分成两支而不是一律当字符串：按键表要给键名加粗（加粗是 `Span` 一级的事，
/// 拼成字符串就切不回来了），而且放不下时按优先级丢；消息是一句人话，一个字
/// 都不能丢，宁可折行也不能截。
enum BarContent {
    Keys(Vec<crate::i18n::HelpItem>),
    Text(String),
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::{SessionInfo, SessionState};
    use app::App;

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn ctrl(c: char) -> KeyEvent {
        KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL)
    }

    /// 起一个真守护进程，socket 和它的 `~/.dct` 替身都落在临时目录里
    /// （`projects.json` 跟着 socket 走，见 `projects::store_path_for_socket`），
    /// 绝不会碰用户真实的那份。返回的 `TempDir` 要接住：它一被丢弃，
    /// socket 就跟着没了。
    fn start_daemon_for_test() -> (PathBuf, tempfile::TempDir) {
        use std::time::{Duration, Instant};
        let home = tempfile::tempdir().unwrap();
        let sock = home.path().join("daemon.sock");
        let s = sock.clone();
        std::thread::spawn(move || {
            let _ = crate::daemon::run(&s);
        });
        let deadline = Instant::now() + Duration::from_secs(5);
        while !sock.exists() {
            assert!(Instant::now() < deadline, "daemon 没起来");
            std::thread::sleep(Duration::from_millis(20));
        }
        (sock, home)
    }

    fn sess_at(id: u32, dir: &str) -> SessionInfo {
        SessionInfo {
            id,
            profile: "claude".into(),
            dir: dir.into(),
            state: SessionState::Idle,
            activity: String::new(),
            is_agent: true,
            tag: String::new(),
        }
    }

    /// **一个「确实没有记录」的项目只能被问一次。** 只用 `profiles` 当缓存
    /// 的话，它的键永远不会被写进去，于是每一轮拉取都要为它重发一次
    /// `LastProfile`——看板 150ms 一轮，守护进程一忙界面就一顿一顿。
    /// 负答案必须也被记住，这条测试盯的就是这件事。
    #[test]
    fn a_project_with_no_recorded_agent_is_only_asked_once() {
        let (mut app, _d) = App::test_app();
        app.set_sessions(vec![sess_at(1, "/w/a"), sess_at(2, "/w/b")]);
        assert_eq!(profiles_to_fetch(&app).len(), 2, "一开始两个都要问");

        // 守护进程对两个都答「没有记录」——`profiles` 不会多出任何一条
        for d in profiles_to_fetch(&app) {
            app.profiles_asked.insert(d);
        }

        assert!(
            profiles_to_fetch(&app).is_empty(),
            "问过就不再问，哪怕答案是「没有」"
        );
    }

    /// 已经知道答案的组也不再问。
    #[test]
    fn a_project_whose_agent_is_already_known_is_not_asked_again() {
        let (mut app, _d) = App::test_app();
        app.set_sessions(vec![sess_at(1, "/w/a")]);
        let d = app.groups[0].dir.display().to_string();
        app.profiles.insert(d, "codex".into());

        assert!(profiles_to_fetch(&app).is_empty());
    }

    /// 新出现的组要问一次——不然刚 `p` 上来的项目，底栏永远写不出
    /// 「上次用的那个 agent」。
    #[test]
    fn a_group_that_just_appeared_is_asked_once() {
        let (mut app, _d) = App::test_app();
        app.set_sessions(vec![sess_at(1, "/w/a")]);
        for d in profiles_to_fetch(&app) {
            app.profiles_asked.insert(d);
        }
        assert!(profiles_to_fetch(&app).is_empty(), "前提：都问过了");

        app.set_sessions(vec![sess_at(1, "/w/a"), sess_at(2, "/w/新来的")]);

        assert_eq!(profiles_to_fetch(&app).len(), 1, "只问新来的那一个");
    }

    /// 守护进程报回来的是归一化后的路径，手里这份是用户敲的原始拼写——
    /// 指的其实是同一组项目。判成「变了」的话，每一轮拉取都会把用户的拼写
    /// 换成 `/private/...`，组头下面那行灰字会在用户什么都没做的时候自己
    /// 变一次样，还白搭一次 `refresh_rows`（它对每个会话目录都要
    /// `canonicalize` 一次）。
    /// 符号链接：Windows 上建它要开发者模式或管理员权限，摆不出这个现场。
    #[test]
    #[cfg(unix)]
    fn the_same_projects_spelled_differently_do_not_count_as_a_change() {
        let d = tempfile::tempdir().unwrap();
        let real = d.path().join("目标");
        std::fs::create_dir(&real).unwrap();
        let link = d.path().join("软链");
        std::os::unix::fs::symlink(&real, &link).unwrap();

        let mine = vec![link.display().to_string()];
        let theirs = vec![view::canon(&link).display().to_string()];
        assert_ne!(mine, theirs, "前提：两条路径字面不同");
        assert!(!projects_changed(&mine, &theirs), "指的是同一个项目");

        let other = vec![d.path().join("另一个").display().to_string()];
        assert!(projects_changed(&mine, &other), "真换了就得认");
    }

    /// **折叠的组里也指得到会话。**
    ///
    /// 折叠的组一行会话都不贡献（`view::flatten`），所以只在 `rows` 里搜的
    /// 实现会一无所获、光标一动不动——而调用方（方向键、F3、`x`）已经把人
    /// 送进那个会话/那一格了。结果就是底栏念着 A、`▶` 在 B，`n` 开进 A。
    #[test]
    fn pointing_at_a_session_inside_a_collapsed_group_opens_that_group() {
        let (mut app, _d) = App::test_app();
        app.set_sessions(vec![sess_at(1, "/w/a"), sess_at(2, "/w/b")]);
        // 光标停到 b 的组头上，把 b 折起来
        app.list_state.select(Some(2));
        toggle_collapse(&mut app);
        assert!(app.groups[1].collapsed, "前提：b 是折叠的");
        // 回到 a
        app.list_state.select(Some(0));
        assert!(app.current_dir().ends_with("a"), "前提：当前项目是 a");

        point_cursor_at_session(&mut app, 2);

        assert!(
            app.current_dir().ends_with("b"),
            "当前项目必须跟着走到 b：{}",
            app.current_dir().display()
        );
        assert_eq!(
            app.selected_session().map(|s| s.id),
            Some(2),
            "而且要真的停在那个会话上——停在组头上的话 Enter/s/u/d 全没了对象"
        );
        assert!(!app.groups[1].collapsed, "指进去就得展开，不然那一行不存在");
    }

    /// `Tab` 落在**组头**上是设计（spec 规则 3），所以「`Tab` 到 B 再按 `g`」
    /// 是日常路径。`home_view` 的九宫格分支要是在组头上直接回落第 0 格，
    /// 第一帧 `▶` 就在 A 的格子上，而底栏念的是 B——`Enter`/`i`/`s`/`u`/`d`
    /// 作用在 A，`n`/`x` 作用在 B，其中两个键不可撤销。
    #[test]
    fn entering_the_grid_from_a_header_lands_on_that_project_first_tile() {
        let (mut app, _d) = App::test_app();
        app.set_sessions(vec![
            sess_at(1, "/w/a"),
            sess_at(2, "/w/b"),
            sess_at(3, "/w/b"),
        ]);
        app.view_mode = ViewMode::Grid;
        // 行：[组头 a, 1, 组头 b, 2, 3]——`Tab` 一下落在 b 的组头上
        jump_project(&mut app, 1);
        assert!(app.current_dir().ends_with("b"), "前提：当前项目是 b");
        assert!(app.selected_session().is_none(), "前提：光标在组头上");

        let View::Grid { focus, .. } = home_view(&app) else {
            panic!("九宫格模式下该回九宫格");
        };

        assert_eq!(
            app.grid_sessions()[focus].dir,
            "/w/b",
            "焦点必须落在当前项目的第一格，不是第 0 格"
        );
    }

    /// **`p` 的浏览器从用户敲的那条路径开起，不是归一化之后的那条。**
    ///
    /// 分组键是 canon 过的（`/tmp/x` 在 macOS 上就是 `/private/tmp/x`），
    /// 拿它去开浏览器的话，用户按下 `p` 看到的顶栏是一条他从没见过的路径。
    /// canon 只用于比较。
    /// 符号链接：Windows 上建它要开发者模式或管理员权限，摆不出这个现场。
    #[test]
    #[cfg(unix)]
    fn the_project_browser_opens_where_the_user_typed_not_where_canon_points() {
        let tmp = tempfile::tempdir().unwrap();
        let nested = tmp.path().join("nested");
        std::fs::create_dir(&nested).unwrap();
        let real = nested.join("真实名字");
        std::fs::create_dir(&real).unwrap();
        let typed = tmp.path().join("我敲的名字");
        std::os::unix::fs::symlink(&real, &typed).unwrap();

        let (mut app, _d) = App::test_app();
        app.set_sessions(vec![sess_at(1, &typed.display().to_string())]);
        assert_eq!(
            app.current_dir(),
            view::canon(&real),
            "前提：分组键是归一化之后那条"
        );

        assert_eq!(
            browse_start_dir(&app),
            tmp.path(),
            "浏览器该开在用户敲的那条路径的上一级"
        );
    }

    /// **建完会话，底栏那句 `n 新建 <agent>` 必须立刻说真话。**
    ///
    /// `profiles_to_fetch` 只问**没问过**的项目，所以缓存里的旧值不是「晚一轮
    /// 才更新」，是这一整次运行都不会再更新：项目记着 claude、底栏写着
    /// `n 新建 claude`，用户按 `N` 选了别的，守护进程记的是新的那个，底栏
    /// 却一直写 claude 直到重启 dct。
    ///
    /// 起一个真守护进程：这条路只有真的走完一次 `Create` 才有东西可对，
    /// 断开的 `App` 上 `create_session` 直接失败，什么都证明不了。
    #[test]
    fn creating_a_session_updates_the_local_agent_cache() {
        use crate::client::Client;

        let (sock, _home) = start_daemon_for_test();
        let work = tempfile::tempdir().unwrap();
        let dir = work.path().display().to_string();
        let mut app = App::new(
            Client::connect(&sock).unwrap(),
            work.path().to_path_buf(),
            crate::i18n::Lang::Zh,
            sock.clone(),
            ViewMode::List,
        );
        // 这个项目此刻记着 claude，底栏据此写 `n 新建 claude`
        app.profiles.insert(dir.clone(), "claude".into());
        app.profiles_asked.insert(dir.clone());

        // 用户按 N 挑了另一个 agent（`shell` 是唯一一定装得上的那个）
        let r = create_session(&mut app, &dir, "shell", true);
        assert!(
            matches!(r, Ok(Response::Created { .. })),
            "前提：会话真的建起来了：{r:?}"
        );

        assert_eq!(
            app.profiles.get(&dir).map(String::as_str),
            Some("shell"),
            "缓存还写着旧 agent，底栏会一直承诺一个 `n` 不会开出来的东西"
        );
    }

    /// `remember: false` 那条路（「帮你装 CLI」开的 shell 会话）不能进缓存——
    /// 守护进程侧也不记，两边记的东西一旦不一样，底栏说的就不是守护进程会做的。
    #[test]
    fn an_install_window_never_becomes_the_projects_agent() {
        use crate::client::Client;

        let (sock, _home) = start_daemon_for_test();
        let work = tempfile::tempdir().unwrap();
        let dir = work.path().display().to_string();
        let mut app = App::new(
            Client::connect(&sock).unwrap(),
            work.path().to_path_buf(),
            crate::i18n::Lang::Zh,
            sock.clone(),
            ViewMode::List,
        );

        let r = create_session(&mut app, &dir, "shell", false);
        assert!(matches!(r, Ok(Response::Created { .. })), "{r:?}");

        assert!(
            app.profiles.is_empty() && app.profiles_asked.is_empty(),
            "装 CLI 的那个 shell 会话不是用户选的 agent，不该被记住"
        );
    }

    /// `create_session` 是「进」这一侧另一个构造器（另一个是 `enter_session`，
    /// 见 `attach::tests::entering_a_session_always_starts_outside_copy_mode`）。
    /// 选择器建会话、密钥验证通过后建会话、`n`/`N` 快速建会话都经这里落进
    /// `View::Attached`，`enter_session` 一个都不会经过——如果这里不复位，
    /// 从一个开着复制模式的会话按 F2 回看板、再按 `n` 新建一个会话，新会话
    /// 会带着上一个会话的复制模式出生，鼠标捕获照样是关的，而用户不知道
    /// 为什么鼠标点了没反应。
    #[test]
    fn create_session_resets_copy_mode_for_a_freshly_created_session() {
        use crate::client::Client;

        let (sock, _home) = start_daemon_for_test();
        let work = tempfile::tempdir().unwrap();
        let dir = work.path().display().to_string();
        let mut app = App::new(
            Client::connect(&sock).unwrap(),
            work.path().to_path_buf(),
            crate::i18n::Lang::Zh,
            sock.clone(),
            ViewMode::List,
        );
        app.copy_mode = true;

        let r = create_session(&mut app, &dir, "shell", false);
        assert!(matches!(r, Ok(Response::Created { .. })), "前提：{r:?}");

        assert!(!app.copy_mode, "上一个会话的复制模式不能粘到新建的这一个上");
    }

    /// **建会话只有一个入口。** 三个调用点各记一次缓存的写法撑不住下一个
    /// 调用点——新加一条建会话的路的人没有任何提示要去补那几行，而漏了之后
    /// 底栏会安静地承诺一个 `n` 不会开出来的 agent。这条守卫把「别忘了」
    /// 换成一个编译不过之外的、看得见的红。
    #[test]
    fn every_create_goes_through_the_one_helper_that_updates_the_cache() {
        // 拼出来，免得这条测试自己的源码被下面的扫描数进去
        let needle = concat!("Request::", "Create");
        for (name, src) in [
            ("pick.rs", include_str!("pick.rs")),
            ("board.rs", include_str!("board.rs")),
            ("grid.rs", include_str!("grid.rs")),
            ("attach.rs", include_str!("attach.rs")),
            ("secret.rs", include_str!("secret.rs")),
        ] {
            assert!(
                !src.contains(needle),
                "{name} 自己发了 Create，绕过了 create_session 的缓存更新"
            );
        }
        // mod.rs 的生产代码部分（`#[cfg(test)]` 之前）只该有 `create_session`
        // 里的那一处。测试代码里另有一处直接对守护进程发 Create，那是在测
        // 别的东西，不走界面这条路。
        let prod = include_str!("mod.rs")
            .split("\n#[cfg(test)]\nmod tests {")
            .next()
            .unwrap();
        assert_eq!(
            prod.matches(needle).count(),
            1,
            "mod.rs 里只该有 create_session 那一处 Create"
        );
    }

    /// **任何一档配色都不许碰 0–15 号具名色。**
    ///
    /// 这是 `theme.rs` 那次事故的成文版：终端主题重定义的正是这十六个，
    /// Solarized 把 8 号色设成和背景同色，用到它的地方整片隐形。实色条把
    /// 前景和背景一起钉死，踩中的话是一条满宽的、糊成一片的带子——屏幕上
    /// 最显眼的元素变成最不可读的那个。往 `BarTheme` 里加档时，这条守卫
    /// 替人记着。
    #[test]
    fn no_bar_theme_uses_a_remappable_named_color() {
        for t in BarTheme::all() {
            let Some(style) = t.style() else { continue };
            for (what, c) in [("bg", style.bg), ("fg", style.fg)] {
                let Some(Color::Indexed(i)) = c else {
                    panic!("{:?} 的 {what} 必须是 256 色索引，实际 {c:?}", t);
                };
                assert!(
                    i >= 16,
                    "{:?} 的 {what} 用了 {i} 号色——0–15 会被终端主题改写",
                    t
                );
            }
        }
    }

    /// **每一档配色都得能读。** 实色条自己铺底色，对比度是构造出来的
    /// （见 `bar_style` 的文档），所以它到底够不够，眼睛说了不算——深红、
    /// 琥珀这类颜色看着很沉，算出来却够亮；反过来「看着挺亮」的组合也
    /// 常常不够。这条守卫按 WCAG 的相对亮度公式算，门槛 4.5:1（正文字号
    /// 那一档，不是大字那一档——底栏的字就是正文字号）。
    ///
    /// 加一档配色的成本因此是固定的：颜色对不对由这里判，不用再靠一次
    /// 「我在自己的终端上看着还行」。
    #[test]
    fn every_bar_theme_is_readable() {
        // 公式不再在这儿抄一份：`theme::srgb`/`luminance`/`contrast` 是共用的，
        // 语义色那条守卫（`every_semantic_color_is_readable_on_its_own_background`）
        // 算的是同一套。两份 WCAG 实现迟早会漂，而漂的那天没人会发现——
        // 两条守卫都还是绿的，只是它们量的不再是同一件事。
        let luminance = |i: u8| {
            let (r, g, b) = crate::theme::srgb(i);
            crate::theme::luminance(r, g, b)
        };

        for t in BarTheme::all() {
            let Some(style) = t.style() else { continue };
            let idx = |c: Option<Color>| match c {
                Some(Color::Indexed(i)) => i,
                other => panic!("{t:?} 的颜色不是 256 色索引：{other:?}"),
            };
            let ratio = crate::theme::contrast(luminance(idx(style.bg)), luminance(idx(style.fg)));
            assert!(
                ratio >= 4.5,
                "{t:?} 的对比度只有 {ratio:.2}:1，正文字号要 4.5:1 才读得清"
            );
        }
    }

    /// 配色码存盘要能原样读回来，认不出的要被拒。跟 `Lang` / `ViewMode`
    /// 的同名守卫一个道理：码一旦写进用户的 settings.json 就是对外契约。
    #[test]
    fn bar_theme_codes_round_trip_and_unknown_ones_are_rejected() {
        for t in BarTheme::all() {
            assert_eq!(BarTheme::from_code(t.code()), Some(*t), "{:?} 没转回来", t);
        }
        assert_eq!(BarTheme::from_code("chartreuse"), None);
        assert_eq!(BarTheme::from_code(""), None);
    }

    /// `Lines` 那一档必须真的没有实色样式——它是「退回横线」的开关，
    /// 给它一个样式就等于把那条退路悄悄堵死。
    #[test]
    fn the_lines_theme_has_no_solid_style() {
        assert!(BarTheme::Lines.style().is_none());
        for t in BarTheme::all().iter().filter(|t| **t != BarTheme::Lines) {
            assert!(t.style().is_some(), "{:?} 该有实色样式", t);
        }
    }

    /// Important (a)/(b) 回归点，UI 这一侧：`explained_failure` 缓存必须在
    /// **每次进入**会话时清空——不然一个「恢复了、又坏了一次」的会话，会
    /// 一直顶着用户上一次贴在这里时看到的旧解释，`already_have` 永远为真，
    /// 新的一次失败问不出新答案。
    #[test]
    fn entering_a_session_forgets_any_previously_cached_explanation() {
        let (mut app, _dir) = App::test_app();
        let dir = app.start_dir.display().to_string();
        app.set_sessions(vec![SessionInfo {
            id: 1,
            profile: "claude".into(),
            dir,
            state: SessionState::Failed,
            activity: String::new(),
            is_agent: true,
            tag: String::new(),
        }]);
        app.explained_failure = Some((1, "上一次贴在这里时看到的旧解释".into()));

        enter_session(&mut app, 1);

        assert_eq!(
            app.explained_failure, None,
            "进入会话必须当成一次全新的观察，不能继续顶着上一次的缓存"
        );
    }

    /// F7 回归测试：re-entering 落在底部必须是 `enter_session` 自己主动做的
    /// 一件事，不能是靠 `run()` 主循环里 `sent_size` 变了才顺带触发的
    /// `Resize` 副作用——不然「F3 直接切会话」（id 变了，`sent_size` 跟着
    /// 变）和「F2 回看板、原地 Enter 回同一个会话」（id 没变，`sent_size`
    /// 照样对得上，`Resize` 根本不会发）会给出两种不同的结果，同一个「回
    /// 来看看」的意图，一半时候把用户按第一种方式接回底部，另一半晾在
    /// 半空。
    ///
    /// 这条测试故意不走 `run()`、不发任何 `Resize`，只直接调
    /// `enter_session`：起一个真守护进程，攒出足够滚屏、真的往上翻一截，
    /// 确认 offset 确实大于零；然后单独调 `enter_session`（模拟"已经在看
    /// 这个会话，只是重新点了一下进来"，没有任何尺寸变化可以触发
    /// Resize），如果 offset 归零了，只能是这次改动新加的那次显式
    /// `Request::Scroll { by: Bottom }` 干的。
    #[test]
    fn entering_a_session_always_lands_at_the_bottom_even_without_a_resize() {
        use crate::client::Client;
        use crate::profile::Profile;
        use crate::proto::{Request, Response};
        use crate::session::{ScrollBy, SessionManager};
        use std::collections::BTreeMap;
        use std::sync::Arc;
        use std::time::{Duration, Instant};

        // 这条测试原来靠内置的 `shell` profile（`/bin/zsh`）攒 200 行滚屏——
        // 那是开发者自己的登录 shell，会 source 真实的 `~/.zshrc`，起多快
        // 全看那份 rc 文件有多重，满载并行跑 `cargo test` 时常常在「等 200
        // 行攒够」的 5 秒期限里输掉，是一次假红，不是这条测试真的抓到了
        // bug。改成测试自己注册的 profile：`/bin/sh --noediting`——`--noediting`
        // 关掉 GNU Readline，shell 不会在不确定的时刻把终端切成 raw 模式；
        // `ENV=/dev/null` 摁死 posix 模式下 sh 的启动脚本，不读任何 rc；
        // `PS1` 钉死成固定串，等它出现就知道 shell 已经能收输入了。
        // 详见 `.superpowers/sdd/2026-08-09-dct-session-auto-name/followup-2-brief.md`。
        const PROMPT: &str = "dct-test$ ";
        let mut env = BTreeMap::new();
        env.insert("ENV".to_string(), "/dev/null".to_string());
        env.insert("PS1".to_string(), PROMPT.to_string());
        let test_shell = Profile {
            name: "scroll-test-shell".into(),
            command: crate::sys::testing::sh_argv(&["--noediting"]),
            is_agent: false,
            idle_pattern: None,
            busy_pattern: None,
            error_pattern: None,
            env,
            secret: None,
            install: None,
            headless: None,
            api: None,
            label: Default::default(),
            note: Default::default(),
            resume_args: Default::default(),
            pairable: false,
            backend_only: false,
        };

        let home = tempfile::tempdir().unwrap();
        let sock = home.path().join("daemon.sock");
        let mgr = Arc::new(SessionManager::new());
        mgr.register_profile(test_shell.clone());
        let s = sock.clone();
        std::thread::spawn(move || {
            let _ = crate::daemon::run_with_manager(&s, mgr);
        });
        let deadline = Instant::now() + Duration::from_secs(5);
        while !sock.exists() {
            assert!(Instant::now() < deadline, "daemon 没起来");
            std::thread::sleep(Duration::from_millis(20));
        }

        let mut c = Client::connect(&sock).unwrap();
        let workdir = tempfile::tempdir().unwrap();
        let id = match c
            .call(Request::Create {
                dir: workdir.path().display().to_string(),
                profile: test_shell.name.clone(),
                remember: false,
            })
            .unwrap()
        {
            Response::Created { id } => id,
            other => panic!("预期 Created，实际 {other:?}"),
        };

        let screen = |c: &mut Client| -> (String, crate::session::ScrollState) {
            match c.call(Request::Screen { id }).unwrap() {
                Response::Screen { lines, scroll, .. } => {
                    let text = lines
                        .iter()
                        .flat_map(|l| l.iter())
                        .map(|s| s.text.as_str())
                        .collect::<String>();
                    (text, scroll)
                }
                other => panic!("预期 Screen，实际 {other:?}"),
            }
        };

        // 提示符出来之前发的字会被吞掉——等它出现，再攒滚屏内容。
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let (text, _) = screen(&mut c);
            if text.contains(PROMPT) {
                break;
            }
            assert!(Instant::now() < deadline, "测试 shell 的提示符一直没出来");
            std::thread::sleep(Duration::from_millis(20));
        }

        // 攒够滚屏内容：跟 `session.rs` 里 `scrolling_session` 用的是同一种
        // POSIX 循环，不挑具体 shell。
        c.call(Request::Input {
            id,
            text: "i=1; while [ $i -le 200 ]; do echo line-$i; i=$((i+1)); done\n".into(),
        })
        .unwrap();

        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let (text, scroll) = screen(&mut c);
            if text.contains("line-200") && scroll.max > 0 {
                break;
            }
            assert!(Instant::now() < deadline, "没等到滚屏内容攒够");
            std::thread::sleep(Duration::from_millis(50));
        }

        c.call(Request::Scroll {
            id,
            by: ScrollBy::Rows(20),
        })
        .unwrap();
        let (_, scroll) = screen(&mut c);
        assert!(scroll.offset > 0, "先确认真的往上翻了，不然下面测的是空话");

        // 模拟「已经在看这个会话，重新进来一次」——不经过 `run()`，直接调
        // `enter_session`：这条路径上没有任何 Resize 会发生。
        let mut app = App::new(
            Client::connect(&sock).unwrap(),
            workdir.path().to_path_buf(),
            crate::i18n::Lang::Zh,
            sock.clone(),
            ViewMode::List,
        );
        app.set_sessions(vec![SessionInfo {
            id,
            profile: test_shell.name.clone(),
            dir: workdir.path().display().to_string(),
            state: SessionState::Working,
            activity: String::new(),
            is_agent: false,
            tag: String::new(),
        }]);
        app.view = View::Board;

        enter_session(&mut app, id);

        // `enter_session` 发的 Scroll 请求是异步落地的——给它一点时间，
        // 不是靠 sleep 赌运气，是轮询到超时才认输。
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let (_, scroll_after) = screen(&mut c);
            if scroll_after.offset == 0 {
                return;
            }
            assert!(
                Instant::now() < deadline,
                "重新进入会话必须落在底部，不能停在离开前翻到的地方：offset={}",
                scroll_after.offset
            );
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    #[test]
    fn other_ctrl_combos_still_reach_the_agent() {
        // 别误伤：Ctrl+C 是 Claude Code 的中断键，Ctrl+B 是它的「转后台」，
        // 两个都必须继续透传。
        assert_eq!(key_to_input(&ctrl('c')), Some("\u{3}".to_string()));
        assert_eq!(key_to_input(&ctrl('b')), Some("\u{2}".to_string()));
    }

    #[test]
    fn arrow_keys_are_forwarded_as_escape_sequences() {
        assert_eq!(key_to_input(&key(KeyCode::Up)).as_deref(), Some("\x1b[A"));
        assert_eq!(key_to_input(&key(KeyCode::Down)).as_deref(), Some("\x1b[B"));
        assert_eq!(
            key_to_input(&key(KeyCode::Right)).as_deref(),
            Some("\x1b[C")
        );
        assert_eq!(key_to_input(&key(KeyCode::Left)).as_deref(), Some("\x1b[D"));
    }

    #[test]
    fn editing_keys_are_forwarded() {
        assert_eq!(
            key_to_input(&key(KeyCode::Backspace)).as_deref(),
            Some("\x7f")
        );
        assert_eq!(key_to_input(&key(KeyCode::Tab)).as_deref(), Some("\t"));
        assert_eq!(
            key_to_input(&key(KeyCode::Delete)).as_deref(),
            Some("\x1b[3~")
        );
    }

    #[test]
    fn enter_sends_empty_string_so_checkpoint_fires() {
        // 空串是与 session::send_input 约定的回车信号，只有它会打检查点
        assert_eq!(key_to_input(&key(KeyCode::Enter)).as_deref(), Some(""));
    }

    #[test]
    fn ctrl_letters_become_control_bytes() {
        let c = KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL);
        assert_eq!(key_to_input(&c).as_deref(), Some("\u{3}"));
        let a = KeyEvent::new(KeyCode::Char('a'), KeyModifiers::CONTROL);
        assert_eq!(key_to_input(&a).as_deref(), Some("\u{1}"));
    }

    #[test]
    fn plain_chars_pass_through() {
        assert_eq!(key_to_input(&key(KeyCode::Char('x'))).as_deref(), Some("x"));
        assert_eq!(
            key_to_input(&key(KeyCode::Char('中'))).as_deref(),
            Some("中")
        );
    }

    #[test]
    fn esc_is_forwarded_to_the_agent() {
        // agent 靠 Esc 做取消/清空/关弹窗，抢走它会让 agent 的交互失灵。
        // 返回看板用 F2。
        assert_eq!(key_to_input(&key(KeyCode::Esc)).as_deref(), Some("\u{1b}"));
    }

    /// `draw()` 是唯一没有靠 client/daemon 就能跑起来的部分——用 `TestBackend`
    /// 把几种 View（看板 / profile 选择弹窗 / 会话屏幕 / 填密钥）实际渲染
    /// 一遍，确认不 panic。这不是端到端验证（没有真的起 daemon、走键盘事件
    /// 循环），但能拦住“布局越界”“空列表 unwrap”这类会在真实交互里当场
    /// 炸掉的问题。这里只是把每种 View 都过一遍顶层 `draw()` 的分派，
    /// 用的多是空/最小 fixture；某个视图内容本身的渲染细节（置灰、原因
    /// 文案、红字警告、密钥打点、二次确认提示……）需要更讲究的 fixture，
    /// 那类测试跟着各自的模块走——目前 `pick.rs` 有 `PickProfile`/
    /// `PickProject` 的渲染细节测试，`secret.rs` 有 `EnterSecret`/
    /// `Secrets` 的；`board.rs`/`attach.rs` 还没有自己的渲染细节测试，
    /// 它们的内容渲染目前只被这条烟雾测试覆盖到"不 panic"这一层。
    #[test]
    fn draw_does_not_panic_for_all_views() {
        use ratatui::backend::TestBackend;

        let sessions = vec![
            SessionInfo {
                id: 1,
                profile: "claude".into(),
                dir: "/tmp/a".into(),
                state: SessionState::Working,
                activity: "正在读取 src/main.rs".into(),
                is_agent: true,
                tag: String::new(),
            },
            SessionInfo {
                id: 2,
                profile: "shell".into(),
                dir: "/tmp/b".into(),
                state: SessionState::Asking,
                activity: "要用哪个方案？".into(),
                is_agent: true,
                tag: String::new(),
            },
        ];

        let mut term = Terminal::new(TestBackend::new(80, 24)).unwrap();
        let (mut app, _dir) = App::test_app();

        // 看板视图，含空消息
        app.view = View::Board;
        app.set_sessions(sessions.clone());
        app.message = Msg::from("");
        app.connected = true;
        term.draw(|f| draw(f, &mut app)).unwrap();
        // 看板视图，带提示消息
        app.message = Msg::from("完成");
        term.draw(|f| draw(f, &mut app)).unwrap();
        // 看板为空列表也不能 panic
        app.set_sessions(Vec::new());
        app.message = Msg::from("");
        term.draw(|f| draw(f, &mut app)).unwrap();
        // 断连状态：底部提示和边框都要切到断连样式，也不能 panic
        app.set_sessions(sessions.clone());
        app.connected = false;
        term.draw(|f| draw(f, &mut app)).unwrap();
        app.connected = true;

        // 九宫格：格子内容的渲染细节在 grid.rs 自己的测试里，这里只过一遍
        // 顶层分派（含底栏那截）
        app.view = View::grid(0);
        term.draw(|f| draw(f, &mut app)).unwrap();

        // profile 选择弹窗
        let mut pick_state = ListState::default();
        pick_state.select(Some(0));
        app.view = View::PickProfile {
            entries: Vec::new(),
            state: pick_state,
            warning: Some("secrets.toml 读不了".into()),
            no_git: false,
        };
        term.draw(|f| draw(f, &mut app)).unwrap();

        // 已进入会话的屏幕视图
        app.view = View::Attached(1);
        term.draw(|f| draw(f, &mut app)).unwrap();
        // 已进入会话但断连了
        app.connected = false;
        term.draw(|f| draw(f, &mut app)).unwrap();
        app.connected = true;

        // 填密钥视图，三个阶段各画一遍：打字中 / 验证中 / 失败
        for phase in [
            SecretPhase::Typing,
            SecretPhase::Verifying,
            SecretPhase::Failed("这个密钥用不了，可能是复制的时候少了一段".into()),
        ] {
            app.view = View::EnterSecret {
                profile: "kimi".into(),
                label: "Kimi".into(),
                prompt: crate::proto::SecretPrompt {
                    hint: "去 platform.moonshot.cn 生成一个".into(),
                    url: Some("https://platform.moonshot.cn".into()),
                },
                buf: "sk-abc123".into(),
                phase,
                return_to_settings: false,
                pairable: false,
            };
            term.draw(|f| draw(f, &mut app)).unwrap();
        }
    }

    /// 断连时底部提示必须覆盖普通帮助文案 / 残留的 action 消息——否则用户会盯着
    /// 一句“完成”或按键提示看，误以为守护进程还活着。这里不渲染像素，只检查
    /// `draw()` 写进 buffer 的文字内容确实包含断连提示。
    #[test]
    fn disconnected_state_shows_warning_in_bottom_bar() {
        use ratatui::backend::TestBackend;

        let mut term = Terminal::new(TestBackend::new(80, 24)).unwrap();
        let (mut app, _dir) = App::test_app();
        app.view = View::Board;
        app.message = Msg::from("完成");
        app.connected = false;

        term.draw(|f| draw(f, &mut app)).unwrap();
        // ratatui 给宽字符（中文）后面那个 cell 塞的是 " "（`Cell::reset`），
        // 不是空串，所以逐 cell 拼出来的文本每个汉字后面都夹了一个空格
        // （"守 护 进 程..."）。去掉空白之后再做子串匹配，两边都做同样的
        // 归一化，不影响判断力。
        let content: String = buffer_text(term.backend().buffer())
            .chars()
            .filter(|c| !c.is_whitespace())
            .collect();
        assert!(
            content.contains("守护进程连不上"),
            "断连时底部应显示明确提示，实际内容（已去空白）: {content}"
        );
        assert!(
            !content.contains("完成"),
            "断连提示必须盖过残留的旧 action 消息，实际内容（已去空白）: {content}"
        );
    }

    #[test]
    fn move_sel_n_clamps_at_both_ends() {
        let mut st = ListState::default();
        st.select(Some(0));

        move_sel_n(&mut st, 3, -1);
        assert_eq!(st.selected(), Some(0), "顶端再往上不动");

        move_sel_n(&mut st, 3, 1);
        move_sel_n(&mut st, 3, 1);
        move_sel_n(&mut st, 3, 1);
        assert_eq!(st.selected(), Some(2), "底端再往下不动");

        // 空列表不能 panic，也不能选中不存在的行
        let mut empty = ListState::default();
        move_sel_n(&mut empty, 0, 1);
        assert_eq!(empty.selected(), None);
    }

    fn buffer_text(buf: &ratatui::buffer::Buffer) -> String {
        let area = buf.area;
        let mut s = String::new();
        for y in 0..area.height {
            for x in 0..area.width {
                if let Some(cell) = buf.cell((x, y)) {
                    s.push_str(cell.symbol());
                }
            }
            s.push('\n');
        }
        s
    }

    /// 底栏左段的文字。宽字符在 TestBackend 里只占首个 cell，
    /// 所以统一滤掉空白再找子串，跟既有的 bottom_bar_help_follows_the_view 一致。
    fn bar_text(term: &Terminal<ratatui::backend::TestBackend>) -> String {
        buffer_text(term.backend().buffer())
            .chars()
            .filter(|c| !c.is_whitespace())
            .collect()
    }

    /// `ESCAPE_HINT_COLS` 是写死的，`escape_hint` 的文案却会跟着功能改。
    /// 两者一旦脱节，左段会把逃生键**静默截断**——而逃生键正是用户卡住时
    /// 唯一的出路，截断了不会报错、只会让人退不出来。所以这里穷举所有视图，
    /// 要求常量真的容得下最长的那一条。
    /// 一台开着一个 agent 会话、光标停在它上面的 dct。
    ///
    /// 按键表现在跟着「能不能按」走（`idle_help` 的 `HelpCtx`），所以断言
    /// 「某个键在不在屏幕上」之前必须先把那个键**能按**的前提摆出来——
    /// 空看板上没有 `s 停止` 不是 bug，是这次改动的目的。
    fn app_with_one_agent_session(view: View) -> (App, tempfile::TempDir) {
        let (mut app, dir) = App::test_app();
        app.connected = true;
        app.set_sessions(vec![SessionInfo {
            id: 1,
            profile: "claude".into(),
            dir: "/tmp/a".into(),
            state: SessionState::Working,
            activity: String::new(),
            is_agent: true,
            tag: String::new(),
        }]);
        // 第 0 行是组头，第 1 行才是那个会话。停在组头上等于「没选中会话」，
        // s/u/d 也就都不该写在底栏上——那是对的行为，但不是这些测试要问的。
        app.list_state.select(Some(1));
        app.view = view;
        (app, dir)
    }

    /// 同上，但看板上有**两个**项目，光标停在第一个项目的那个会话上。
    ///
    /// `Tab 换项目` 只在有第二个项目可跳时才写（`jump_project` 在一个组上
    /// 原地打转），所以任何关于 `Tab` 的断言都必须用这一档——用单项目的
    /// 那个 fixture 去断言 `Tab` 在场，等于把一个按不动的键钉死在测试里。
    fn app_with_two_projects(view: View) -> (App, tempfile::TempDir) {
        let (mut app, dir) = App::test_app();
        app.connected = true;
        app.set_sessions(vec![
            SessionInfo {
                id: 1,
                profile: "claude".into(),
                dir: "/tmp/a".into(),
                state: SessionState::Working,
                activity: String::new(),
                is_agent: true,
                tag: String::new(),
            },
            SessionInfo {
                id: 2,
                profile: "claude".into(),
                dir: "/tmp/b".into(),
                state: SessionState::Working,
                activity: String::new(),
                is_agent: true,
                tag: String::new(),
            },
        ]);
        // 行序：组头 a / 会话 1 / 组头 b / 会话 2
        app.list_state.select(Some(1));
        app.view = view;
        (app, dir)
    }

    /// 80 列（最常见的下限）下，右段那三条动作必须整整齐齐都在。
    ///
    /// 这条盯的是「三段各占多宽」这个预算：中段和左段是定死的，右段剩多少
    /// 全看那两个常量。谁把 `PROJECT_COLS` 调大一点，80 列上第三个键就会
    /// 被右端**静默**吃掉——不报错、不换行，只是没了。
    #[test]
    fn the_three_actions_all_fit_at_eighty_columns() {
        use ratatui::backend::TestBackend;

        // 两种语言都要测。中文是双宽字符但字少，英文是单宽但词长——
        // 谁更挤不是想当然的：`Tab 换项目` 是 10 列，`Tab switch project`
        // 是 18 列。只测中文的话，英文用户的底栏在 80 列上会悄悄少一个键，
        // 而这正是这次改造要消灭的那件事。
        //
        // 记着 agent 名的项目是**更挤**的那一档（`n 新建 claude` 比
        // `n 新建` 宽 7 列），所以两档都过一遍。
        for lang in [crate::i18n::Lang::Zh, crate::i18n::Lang::En] {
            for agent in [None, Some("claude")] {
                let (mut app, _dir) = app_with_two_projects(View::Board);
                app.lang = lang;
                if let Some(a) = agent {
                    remember_agent(&mut app, a);
                }
                let mut term = Terminal::new(TestBackend::new(80, 24)).unwrap();
                term.draw(|f| draw(f, &mut app)).unwrap();
                let c = bar_text(&term);

                let want: [&str; 4] = match lang {
                    crate::i18n::Lang::Zh => ["Enter进会话", "n新建", "Tab换项目", "?…"],
                    _ => ["Enteropen", "nnew", "Tabproject", "?…"],
                };
                for key in want {
                    assert!(
                        c.contains(key),
                        "{lang:?}/{agent:?} 下「{key}」被截掉了：{c}"
                    );
                }
            }
        }
    }

    /// 只有一个项目时不写 `Tab 换项目`：`jump_project` 算的是
    /// `(cur + 1).rem_euclid(1)` = 0，光标原地停在同一个组头上。而「只有
    /// 一个项目」正是第一次用 dct 的默认状态——那一屏上写着一个按下去
    /// 毫无反应的键，用户学到的第一件事就是底栏会骗人。
    #[test]
    fn a_lone_project_does_not_advertise_switching_to_another() {
        use ratatui::backend::TestBackend;

        let (mut app, _dir) = app_with_one_agent_session(View::Board);
        assert_eq!(app.groups.len(), 1, "这个 fixture 只有一个项目");
        let mut term = Terminal::new(TestBackend::new(120, 24)).unwrap();
        term.draw(|f| draw(f, &mut app)).unwrap();
        let c = bar_text(&term);
        assert!(!c.contains("Tab"), "只有一个项目，Tab 什么都不做：{c}");

        // 有第二个项目就该写出来，否则这条测试等于把功能测没了
        let (mut app, _dir) = app_with_two_projects(View::Board);
        let mut term = Terminal::new(TestBackend::new(120, 24)).unwrap();
        term.draw(|f| draw(f, &mut app)).unwrap();
        let c = bar_text(&term);
        assert!(c.contains("Tab换项目"), "有第二个项目就该写：{c}");
    }

    /// 左段和中段永不让位：一条长消息、断连状态，都不能把「我在哪个项目」
    /// 顶掉。老版本正是「已切到 X」这类消息把项目信息整个盖掉的。
    ///
    /// 断言的是**完整的显示串**（`/tmp/a`）而不只是项目名：`dir` 是 canon
    /// 过的分组键，macOS 上那是 `/private/tmp/a`——中段一旦改成画 `dir`，
    /// 这条就会红。
    #[test]
    fn the_project_segment_survives_a_long_message_and_a_disconnect() {
        use ratatui::backend::TestBackend;

        let (mut app, _d) = app_with_one_agent_session(View::Board);
        let mut term = Terminal::new(TestBackend::new(100, 24)).unwrap();

        app.message = "x".repeat(400).into();
        term.draw(|f| draw(f, &mut app)).unwrap();
        assert!(bar_text(&term).contains("/tmp/a"), "长消息不能盖掉项目名");

        app.connected = false;
        let mut term = Terminal::new(TestBackend::new(100, 24)).unwrap();
        term.draw(|f| draw(f, &mut app)).unwrap();
        let c = bar_text(&term);
        assert!(c.contains("/tmp/a"), "断连也不能盖掉项目名：{c}");
        assert!(c.contains("q退出"), "断连也不能盖掉逃生键：{c}");
        assert!(
            !c.contains("/private/"),
            "中段画的必须是用户敲的那条路径，不是 canon 之后的：{c}"
        );
    }

    /// 中段画的是**用户敲的那条路径**，不是归一化之后的。
    ///
    /// macOS 上 `/tmp` 是指向 `/private/tmp` 的符号链接，所以这里特意用一个
    /// **真实存在**的临时目录（建在 `/tmp` 下）：`canonicalize` 只有对真的
    /// 存在的路径才会给出不同的答案。上一条测试用的 `/tmp/a` 并不存在，
    /// canon 会原样退回，那条断言因此抓不住「中段改成画 `g.dir`」这个改动
    /// ——这一条才抓得住。Linux 上 `/tmp` 不是符号链接，断言照样成立，
    /// 只是不吃劲。
    #[test]
    fn the_project_segment_never_shows_the_canonical_path() {
        use ratatui::backend::TestBackend;

        let real = tempfile::Builder::new()
            .prefix("dct-bar-")
            .tempdir_in("/tmp")
            .unwrap();
        let typed = real.path().display().to_string();

        let (mut app, _d) = App::test_app();
        app.connected = true;
        app.set_sessions(vec![SessionInfo {
            id: 1,
            profile: "claude".into(),
            dir: typed.clone(),
            state: SessionState::Working,
            activity: String::new(),
            is_agent: true,
            tag: String::new(),
        }]);
        app.list_state.select(Some(1));
        app.view = View::Board;

        let mut term = Terminal::new(TestBackend::new(100, 24)).unwrap();
        term.draw(|f| draw(f, &mut app)).unwrap();
        let c = bar_text(&term);
        assert!(
            !c.contains("/private/"),
            "canon 只用于比较，永不用于显示：{c}"
        );
    }

    /// 右段硬上限 3 条动作 + 一个 `?`。终端再宽也不多塞——一行的内容随宽度
    /// 变化本身就是不可预期的：用户在 80 列上学会的那一行，到 200 列上会
    /// 多出几个键、位置全变。
    #[test]
    fn the_action_segment_never_exceeds_three_keys_plus_the_door() {
        use ratatui::backend::TestBackend;

        for w in [80u16, 120, 200] {
            let (mut app, _d) = app_with_one_agent_session(View::Board);
            let mut term = Terminal::new(TestBackend::new(w, 24)).unwrap();
            term.draw(|f| draw(f, &mut app)).unwrap();
            let items = idle_help(&app.view, app.lang, help_ctx(&app));
            assert!(
                items.len() <= 4,
                "{w} 列下右段有 {} 条，超过 3 个动作 + ?",
                items.len()
            );
        }
    }

    /// 光标停在组头上时不该写 `Enter 进会话`——按下去没有对象。
    ///
    /// 这条从 `help_ctx` 一路测到文案：`selected_session()` 在组头行上返回
    /// `None`，`help_ctx` 得把它带下去，`board_keys` 才不会写那一条。中间
    /// 任何一环用「有没有行」代替「有没有选中会话」，这条就会红。
    #[test]
    fn a_header_row_does_not_advertise_entering_a_session() {
        let (mut app, _d) = app_with_one_agent_session(View::Board);
        app.list_state.select(Some(0));
        assert!(app.selected_session().is_none(), "第 0 行该是组头");
        let items = idle_help(&app.view, app.lang, help_ctx(&app));
        let joined = crate::i18n::help_text(&items);
        assert!(!joined.contains("Enter"), "组头行上不写 Enter：{joined}");
        assert!(joined.contains("n 新建"), "组头行上最该按的是它：{joined}");
    }

    /// `n` 那一条要带上这个项目上次用的 agent 名。按下去到底会开出什么，
    /// 用户不该先去记自己上次在这个项目里用的是谁。
    #[test]
    fn the_new_key_names_the_agent_it_would_start() {
        use ratatui::backend::TestBackend;

        let (mut app, _dir) = app_with_one_agent_session(View::Board);
        remember_agent(&mut app, "claude");
        let mut term = Terminal::new(TestBackend::new(120, 24)).unwrap();
        term.draw(|f| draw(f, &mut app)).unwrap();
        let c = bar_text(&term);
        assert!(c.contains("n新建claude"), "`n` 该说清会开哪个 agent：{c}");
    }

    /// 给 `/tmp/a` 这个项目记上「上次用的是谁」，`n 新建 <agent>` 才有名字可写。
    /// 键是**归一化后**的路径——`group_sessions` 就是拿它当分组键去查 `profiles` 的。
    fn remember_agent(app: &mut App, name: &str) {
        app.profiles.insert(
            view::canon(Path::new("/tmp/a")).display().to_string(),
            name.into(),
        );
        app.refresh_rows();
    }

    /// 三段的宽度预算：左段永不让位，右段至少留得下那扇门，中段是让位的
    /// 那一个——但让的是**终端太窄**，不是让给消息。
    #[test]
    fn the_bar_gives_up_the_project_before_the_escape_hint_or_the_door() {
        // 左段是 ESCAPE_HINT_COLS + 2 = 14：逃生键从「Ctrl+Q（F2） 回看板」
        // 收敛成「F2 回看板」之后窄了 7 列，那 7 列全归右段。
        // 常见宽度：三段都拿到自己那份（中段 = PROJECT_COLS + 2 = 26）
        assert_eq!(bar_widths(98), (14, 26, 58));
        assert_eq!(bar_widths(78), (14, 26, 38));
        // 55 列终端（`the_way_back_survives_a_narrow_terminal` 那一档）：
        // 中段缩到只剩几列，好让右段完整放下那句「怎么回到底部」
        assert_eq!(bar_widths(53), (14, 11, 28));
        // 再窄下去中段整个让掉，左段和右段一列不动
        let (esc, proj, act) = bar_widths(38);
        assert_eq!(esc, 14, "逃生键永不让位");
        assert_eq!(proj, 0, "窄到这份上，让的是中段");
        assert_eq!(esc + proj + act, 38, "三段必须正好铺满，不能有空隙");
        // 窄到荒谬也不能 panic、不能溢出
        for w in [0u16, 1, 5, 20, 22, 25] {
            let (e, p, a) = bar_widths(w);
            assert_eq!(e + p + a, w, "{w} 列下三段加起来不等于总宽");
        }
    }

    /// 反过来的一半：空看板上那六个键一个都不该在，而 `n 新建` 必须在——
    /// 这时候它是屏幕上唯一有意义的动作。
    #[test]
    fn an_empty_board_only_offers_what_can_actually_be_done() {
        use ratatui::backend::TestBackend;

        let (mut app, _dir) = App::test_app();
        app.connected = true;
        app.view = View::Board;
        let mut term = Terminal::new(TestBackend::new(80, 24)).unwrap();
        term.draw(|f| draw(f, &mut app)).unwrap();
        let c = bar_text(&term);

        for key in ["↑↓选择", "Enter进会话", "s停止", "u回滚", "d改动"] {
            assert!(!c.contains(key), "空看板上「{key}」按不动，不该写：{c}");
        }
        assert!(c.contains("n新建"), "空看板上唯一该按的键：{c}");
        assert!(c.contains("?…"), "门永远在：{c}");
    }

    /// 被截掉的键必须有个去处，而那扇门本身**绝不能**被截掉。
    /// 丢了它，`p/N/a/c/l` 就成了「屏幕上没写却真管用」的键。
    #[test]
    fn the_door_to_the_rest_of_the_keys_is_always_on_screen() {
        use ratatui::backend::TestBackend;

        for view in [View::Board, View::grid(0)] {
            for width in [40u16, 60, 80, 200] {
                let (mut app, _dir) = app_with_one_agent_session(view.clone());
                let mut term = Terminal::new(TestBackend::new(width, 24)).unwrap();
                term.draw(|f| draw(f, &mut app)).unwrap();
                let c = bar_text(&term);
                assert!(c.contains("?…"), "{width} 列下 `? …` 不见了：{c}");
            }
        }
    }

    /// 键名要加粗。这一行是「字母 + 中文」交替，不给字母一点重量的话，
    /// 用户得逐个词去认哪个是能按的键——这正是用户看着截图说「太啰嗦」的
    /// 那种啰嗦。
    #[test]
    fn the_key_letters_are_bold() {
        use ratatui::backend::TestBackend;

        let (mut app, _dir) = app_with_two_projects(View::Board);
        let mut term = Terminal::new(TestBackend::new(120, 24)).unwrap();
        term.draw(|f| draw(f, &mut app)).unwrap();

        let buf = term.backend().buffer();
        let row = bar_first_line(&term, app.bar); // 底栏第一行正文就是按键表
        let bold: String = (0..buf.area.width)
            .filter_map(|x| buf.cell((x, row)))
            .filter(|c| c.style().add_modifier.contains(Modifier::BOLD))
            .map(|c| c.symbol().to_string())
            .collect();
        assert!(bold.contains('n'), "`n` 该是粗的：{bold:?}");
        assert!(bold.contains('T'), "`Tab` 也该是粗的：{bold:?}");
        assert!(
            !bold.contains('新'),
            "只有键名该加粗，说明不该跟着粗：{bold:?}"
        );
    }

    /// 底栏顶边所在的行号。
    ///
    /// 原来靠左上角 `┌` 认——那时候左右还画边框，`┌` 是唯一的角。现在这些
    /// 块只画上下边框（`Borders::TOP | Borders::BOTTOM`），没有左右边框
    /// 就没有角字符可认，只剩横线字符 `─`。但 `─` 不是底栏独有的：内容区
    /// 自己的块也有上下边框，第 0 列上可能不止两条横线，最后一行终端本身
    /// 也未必是横线——**只有底栏的下边框保证总在屏幕最后一行**（底栏是
    /// 垂直切分出来的最后一块，`Layout::vertical` 保证它贴到 `f.area()`
    /// 的底边）。从那一行往上找**最近**的一条横线就是底栏顶边——按键表/
    /// 消息那几行内容不会以 `─` 开头，中间不会有别的横线插进来。
    /// 底栏**第一行正文**（按键表/消息那一行）的行号。
    ///
    /// 实色档下原来那个锚点没有了：底栏不画边框，屏幕最后一行是正文而不是
    /// `─`。改成按底色认——底栏整块铺了 `bar_style()` 的背景，内容区没有，
    /// 所以从最后一行往上数、底色还是它的那些行就是底栏。这个判据在两档下
    /// 都成立，横线档只是多一步跳过边框行。
    fn bar_first_line(term: &Terminal<ratatui::backend::TestBackend>, theme: BarTheme) -> u16 {
        let buf = term.backend().buffer();
        let bottom_edge = buf.area.height - 1;

        let Some(bar) = bar_style(theme) else {
            // 横线档：下边框贴着屏幕最后一行，往上最近的那条横线是顶边，
            // 顶边下面一行才是正文。
            assert_eq!(
                buf.cell((0, bottom_edge)).map(|c| c.symbol()),
                Some("─"),
                "横线档下，底栏下边框总该贴着屏幕最后一行"
            );
            let top = (0..bottom_edge)
                .rev()
                .find(|y| buf.cell((0, *y)).map(|c| c.symbol()) == Some("─"))
                .expect("底栏顶边总该在屏幕上");
            return top + 1;
        };

        assert_eq!(
            buf.cell((0, bottom_edge)).map(|c| c.bg),
            Some(bar.bg.expect("实色条必须给出背景色")),
            "实色档下，底栏底色总该铺到屏幕最后一行"
        );
        // 从最后一行往上走，底色一变就到内容区了。
        (0..=bottom_edge)
            .rev()
            .take_while(|y| buf.cell((0, *y)).map(|c| c.bg) == bar.bg)
            .last()
            .expect("底栏至少有一行正文")
    }

    /// 底栏正文占几行。按键表恒为一行，只有长消息会把它撑高。
    fn bar_lines(term: &Terminal<ratatui::backend::TestBackend>, theme: BarTheme) -> u16 {
        let h = term.backend().buffer().area.height;
        let chrome = if bar_style(theme).is_some() { 0 } else { 1 }; // 横线档还有一行下边框
        h - bar_first_line(term, theme) - chrome
    }

    /// 按键表**永远只占一行**（加上下边框共 3 行），多窄都一样。
    ///
    /// 这是用户那句「太啰嗦」的直接答复，也是一条结构约束：底栏每多一行，
    /// 内容区就少一行，而九宫格在 80×24 下只差一行就跌破 `grid.rs` 的
    /// `MIN_ROWS`，整屏换成一句「窗口太小」。折行的那一版把这件事变成了
    /// 「往表里加键之前先手算一遍宽度」，人肉记忆守不住。
    #[test]
    fn the_key_bar_is_always_one_line() {
        use ratatui::backend::TestBackend;

        for view in [View::Board, View::grid(0)] {
            for width in [40u16, 60, 80, 100, 160] {
                let (mut app, _dir) = App::test_app();
                app.view = view.clone();
                let mut term = Terminal::new(TestBackend::new(width, 24)).unwrap();
                term.draw(|f| draw(f, &mut app)).unwrap();
                assert_eq!(
                    bar_lines(&term, app.bar),
                    1,
                    "{width} 列下按键表折行了，底栏不再是一行"
                );
            }
        }
    }

    /// 消息**不**跟着按键表一起被压成一行。
    ///
    /// 两者共用底栏这一格，但性质不同：按键表放不下的键在 `?` 浮层里找得
    /// 回来，而一句「XXX 不是会话号」被截掉就是真的没了。所以消息照旧走
    /// `wrap_help` 折行，底栏该长就长。
    #[test]
    fn a_long_message_still_wraps_instead_of_being_squeezed_into_one_line() {
        use ratatui::backend::TestBackend;

        let (mut app, _dir) = App::test_app();
        app.connected = true;
        app.view = View::Board;
        app.message = format!("{}  {}", "很长的一句话".repeat(5), "另外半句".repeat(5)).into();
        let mut term = Terminal::new(TestBackend::new(80, 24)).unwrap();
        term.draw(|f| draw(f, &mut app)).unwrap();
        assert!(
            bar_lines(&term, app.bar) > 1,
            "长消息该把底栏撑高，而不是被压成一行截掉一半"
        );
    }

    /// 底栏不许把九宫格挤到画不出来。
    ///
    /// 底栏高度改成按内容算之后，按键表每多折一行，内容区就少一行。80×24
    /// 恰好卡在边界上：内容区 20 行 = `grid.rs` 的 `MIN_ROWS`，再少一行整个
    /// 九宫格就换成一句「窗口太小」。往九宫格那份按键表里加一个键就会触发，
    /// 而这在单测里不加一条断言是看不出来的——按键表的测试只数键，不看格子
    /// 还在不在。
    #[test]
    fn the_bottom_bar_never_squeezes_the_grid_off_the_screen() {
        use ratatui::backend::TestBackend;

        let (mut app, _dir) = App::test_app();
        app.connected = true;
        app.set_sessions(vec![SessionInfo {
            id: 1,
            profile: "claude".into(),
            dir: "/tmp/a".into(),
            state: SessionState::Working,
            activity: String::new(),
            is_agent: true,
            tag: String::new(),
        }]);
        app.view = View::grid(0);

        // 80×24 是最常见的终端下限，这一条必须在这个尺寸上成立
        let mut term = Terminal::new(TestBackend::new(80, 24)).unwrap();
        term.draw(|f| draw(f, &mut app)).unwrap();
        let screen = bar_text(&term);
        assert!(
            !screen.contains("窗口太小"),
            "80×24 下九宫格必须画得出来，底栏不能把它挤没：{screen}"
        );
        assert!(screen.contains("claude"), "格子该在屏幕上：{screen}");
    }

    /// 底栏的内容**不随终端宽度变化**。
    ///
    /// 这条取代了老的 `a_wide_terminal_shows_more_keys`（那条要求 160 列比
    /// 80 列多露几个键）。「宽了就多塞几个」听着像是把空间用满，实际后果是
    /// 同一个键在 80 列上没有、在 160 列上有：用户在自己那台终端上学会的
    /// 一行，换台机器就变了，而他没有任何线索知道为什么。现在多出来的宽度
    /// 留白，键表恒定。
    #[test]
    fn the_bar_says_the_same_thing_at_every_width() {
        use ratatui::backend::TestBackend;

        // **两种语言各跑一遍。** 只测一种语言的宽度测试抓不住这一类 bug，
        // 而这一类 bug 正是这个任务存在的理由：中文的一行 37 列、英文的
        // 35 列，两者跟 80 列下那 39 列的余量完全不同，谁先崩不是想当然的。
        for lang in [crate::i18n::Lang::Zh, crate::i18n::Lang::En] {
            let mut seen: Option<Vec<&str>> = None;
            for width in [80u16, 100, 160, 240] {
                let (mut app, _dir) = app_with_two_projects(View::Board);
                app.lang = lang;
                remember_agent(&mut app, "claude");
                let mut term = Terminal::new(TestBackend::new(width, 24)).unwrap();
                term.draw(|f| draw(f, &mut app)).unwrap();
                // 底栏只画上下边框，宽度不再扣左右边框那 2 列。
                let (_, _, cols) = bar_widths(width);
                // 比的是**键**，不是整行文字：`n` 后面那个 agent 名放不下时会
                // 让位，那是有意的（见 `bar_keys`）——让的是半句说明，不是一个键。
                let keys: Vec<&str> =
                    widgets::fit_help(&bar_keys(&app, cols as usize), cols as usize)
                        .iter()
                        .map(|i| i.key)
                        .collect();
                assert_eq!(keys.len(), 4, "{lang:?}/{width} 列下少了一个键：{keys:?}");
                match &seen {
                    None => seen = Some(keys),
                    Some(first) => assert_eq!(first, &keys, "{lang:?}/{width} 列下键表变了"),
                }
            }
        }
    }

    /// 九宫格的按键表跟看板同一条上限，但候选键是它自己的：`i 回一句` 是这个
    /// 视图独有的能力，不写就找不到，所以它排在 `Tab` 前面并把 `Tab` 挤出
    /// 那三个位子（`board_keys` 的 `truncate(3)`）。
    ///
    /// **用两个项目的 fixture。** 单项目那一档里 `can_switch_project` 是
    /// false，`Tab` 压根没进候选表，于是 `!contains("Tab换项目")` 无论
    /// `board_keys` 怎么改都是绿的——这条断言本来要盯的是那条上限，
    /// 用单项目 fixture 就变成了一句永真的话。
    ///
    /// **两种语言都要数键。** 中文那几条字面（`i回一句`…）只在 `Lang::Zh`
    /// 下成立，英文标签一旦变长（`i reply once` 比 `i回一句` 宽 5 列），
    /// 80 列上会被 `fit_help` 悄悄丢掉一条，而整套测试全绿。所以除了那几条
    /// 字面，再直接数一遍两种语言下真正画出来的条数——中文这一行在 80 列上
    /// 正好占满 39 列的可用宽度，一列余量都没有。
    #[test]
    fn the_grid_keeps_its_own_key_on_screen_at_eighty_columns() {
        use ratatui::backend::TestBackend;

        for lang in [crate::i18n::Lang::Zh, crate::i18n::Lang::En] {
            let (mut app, _dir) = app_with_two_projects(View::grid(0));
            app.lang = lang;
            let mut term = Terminal::new(TestBackend::new(80, 24)).unwrap();
            term.draw(|f| draw(f, &mut app)).unwrap();
            // 底栏只画上下边框，宽度不再扣左右边框那 2 列。
            let (_, _, cols) = bar_widths(80);
            let keys: Vec<&str> = widgets::fit_help(&bar_keys(&app, cols as usize), cols as usize)
                .iter()
                .map(|i| i.key)
                .collect();
            assert_eq!(
                keys.len(),
                4,
                "{lang:?} 下 80 列放不下九宫格那四条：{keys:?}"
            );
        }

        let (mut app, _dir) = app_with_two_projects(View::grid(0));
        let mut term = Terminal::new(TestBackend::new(80, 24)).unwrap();
        term.draw(|f| draw(f, &mut app)).unwrap();
        let c = bar_text(&term);

        for key in ["q退出", "Enter放大", "i回一句", "n新建", "?…"] {
            assert!(c.contains(key), "九宫格按键表里的「{key}」被截掉了：{c}");
        }
        // `Tab` 九宫格是绑着的（`grid::handle_key`），这里没有它是因为三条
        // 动作已经满了——它在 `?` 浮层里，那扇门就在这一行的尾巴上。
        assert!(
            !c.contains("Tab换项目"),
            "三条动作的上限破了：底栏又开始随宽度忽隐忽现：{c}"
        );
        // `x` 也绑着，但光标那个组还有正在跑的会话，`unpin_current` 会拒绝
        assert!(!c.contains("x移除"), "非空组拿不掉，不该写：{c}");
    }

    /// 选项目是**浮层**不是全屏接管：画完之后，屏幕上必须**同时**看得到
    /// 浮层的内容和背后的看板。上一版全屏接管，用户按 p 的那一刻整个
    /// 界面消失，只剩一个几乎全空的框——这正是「完全是混乱的」的一部分。
    #[test]
    fn the_project_picker_is_an_overlay_not_a_takeover() {
        use ratatui::backend::TestBackend;
        let (mut app, _dir) = App::test_app();
        app.set_sessions(vec![SessionInfo {
            id: 1,
            profile: "claude".into(),
            dir: "/w/proj".into(),
            state: SessionState::Idle,
            activity: "背后的看板".into(),
            is_agent: true,
            tag: String::new(),
        }]);
        app.view = View::PickProject(crate::ui::view::ProjectPicker::new(
            vec!["/w/other".to_string()],
            PathBuf::from("/w"),
        ));

        let mut term = Terminal::new(TestBackend::new(120, 30)).unwrap();
        term.draw(|f| draw(f, &mut app)).unwrap();
        let c = bar_text(&term);

        assert!(c.contains("背后的看板"), "背后的看板必须还看得见：{c}");
        assert!(c.contains("最近"), "浮层自己也要画出来：{c}");
    }

    /// 终端小到放不下浮层时退化成全屏，而不是显示一句「窗口太小」：
    /// 选项目是用户此刻非做不可的事，挡住他没有意义。
    #[test]
    fn a_tiny_terminal_gets_the_picker_full_screen_rather_than_a_refusal() {
        let full = Rect {
            x: 0,
            y: 0,
            width: 20,
            height: 6,
        };
        assert_eq!(popup_area(full), full);
    }

    /// 宽屏上浮层不该铺满：一个横跨 200 列的对话框，眼睛要扫过整个屏幕
    /// 才能读完一行。
    #[test]
    fn a_wide_terminal_gets_a_bounded_centered_popup() {
        let full = Rect {
            x: 0,
            y: 0,
            width: 200,
            height: 60,
        };
        let p = popup_area(full);
        assert!(p.width <= 100, "宽度要有上限：{}", p.width);
        assert!(p.height <= 24, "高度要有上限：{}", p.height);
        assert!(p.x > 0 && p.y > 0, "要居中，不能贴边");
        assert_eq!(p.x + p.width / 2, full.width / 2, "水平居中");
    }

    #[test]
    fn escape_hint_cols_fits_every_view() {
        use unicode_width::UnicodeWidthStr;
        let views = [
            View::Board,
            View::Attached(1),
            View::grid(0),
            View::PickProfile {
                entries: Vec::new(),
                state: ListState::default(),
                warning: None,
                no_git: false,
            },
            View::PickProject(crate::ui::view::ProjectPicker {
                filter: String::new(),
                typing_path: None,
                ..crate::ui::view::ProjectPicker::new(Vec::new(), std::path::PathBuf::from("/tmp"))
            }),
            View::PickProject(crate::ui::view::ProjectPicker {
                filter: String::new(),
                typing_path: Some(String::new()),
                ..crate::ui::view::ProjectPicker::new(Vec::new(), std::path::PathBuf::from("/tmp"))
            }),
            View::Secrets {
                entries: Vec::new(),
                state: ListState::default(),
                pending_delete: None,
            },
            View::Settings {
                state: ListState::default(),
                sub: None,
            },
            View::Keys {
                from: Box::new(View::Board),
            },
            // 填密钥有两条退路（回设置页 / 回选择器），两条文案都要量
            View::EnterSecret {
                profile: String::new(),
                label: String::new(),
                prompt: crate::proto::SecretPrompt {
                    hint: String::new(),
                    url: None,
                },
                buf: String::new(),
                phase: view::SecretPhase::Typing,
                return_to_settings: true,
                pairable: false,
            },
            View::EnterSecret {
                profile: String::new(),
                label: String::new(),
                prompt: crate::proto::SecretPrompt {
                    hint: String::new(),
                    url: None,
                },
                buf: String::new(),
                phase: view::SecretPhase::Typing,
                return_to_settings: false,
                pairable: false,
            },
            View::Phone {
                status: crate::proto::PhoneStatus {
                    state: crate::proto::PhoneState::Off,
                    bot: None,
                    owner: None,
                },
            },
        ];
        // 两种语言都要量。常量是写死的，而译文长度各不相同——只量中文的话，
        // 哪天某种语言的逃生键更长，就会在那种语言下被静默截断。
        for l in crate::i18n::Lang::all() {
            for v in &views {
                let hint = escape_hint(v, *l);
                assert!(
                    hint.width() <= ESCAPE_HINT_COLS as usize,
                    "{l:?} 下逃生键「{hint}」宽 {} 列，放不进 ESCAPE_HINT_COLS = {ESCAPE_HINT_COLS}",
                    hint.width()
                );
            }
        }
        // 常量不能比需要的更宽：多占的每一列都是从右段的消息里抢的
        let widest = crate::i18n::Lang::all()
            .iter()
            .flat_map(|l| views.iter().map(move |v| escape_hint(v, *l).width()))
            .max()
            .unwrap();
        assert_eq!(
            widest, ESCAPE_HINT_COLS as usize,
            "ESCAPE_HINT_COLS 应当正好等于最长文案的宽度"
        );
    }

    #[test]
    fn escape_hint_survives_a_long_message() {
        use ratatui::backend::TestBackend;

        // 真实事故：在看板上按 p 换项目，「已切到 …」这条消息把整张按键表
        // 顶掉，其中就包括「q 退出」。用户从此没有任何地方能看到怎么退出。
        let mut term = Terminal::new(TestBackend::new(100, 24)).unwrap();
        let (mut app, _dir) = App::test_app();
        app.view = View::Board;
        app.message = Msg::from(
            "已切到 ~/work/dc/dc-terminal，这条消息故意写得很长很长很长很长很长".to_string(),
        );
        term.draw(|f| draw(f, &mut app)).unwrap();
        let c = bar_text(&term);
        assert!(
            c.contains("q退出"),
            "消息再长也不能把退出提示挤掉——这正是用户卡住的那一屏：{c}"
        );
    }

    #[test]
    fn escape_hint_survives_a_disconnect() {
        use ratatui::backend::TestBackend;

        // 出事的那一刻恰恰是最需要逃生提示的时候，断连提示不能把它顶掉。
        let mut term = Terminal::new(TestBackend::new(100, 24)).unwrap();
        let (mut app, _dir) = App::test_app();
        app.view = View::Attached(1);
        app.connected = false;
        term.draw(|f| draw(f, &mut app)).unwrap();
        let c = bar_text(&term);
        assert!(c.contains("F2回看板"), "断连时逃生提示必须还在：{c}");
        assert!(c.contains("连不上"), "断连提示本身也要显示：{c}");
    }

    /// 底栏要写着「我在哪个项目」——而那个项目就是光标所在的那个组，
    /// 不再是一个可以跟屏幕上的列表对不上的字段。
    #[test]
    fn bottom_bar_shows_current_project() {
        use ratatui::backend::TestBackend;

        let mut term = Terminal::new(TestBackend::new(80, 24)).unwrap();
        let (mut app, _dir) = App::test_app();
        app.set_sessions(vec![SessionInfo {
            id: 1,
            profile: "claude".into(),
            dir: "/Users/lei/work/dc/dc-terminal".into(),
            state: SessionState::Idle,
            activity: String::new(),
            is_agent: true,
            tag: String::new(),
        }]);
        app.view = View::Board;
        assert!(
            app.current_dir().ends_with("dc-terminal"),
            "前提：光标落在这个项目的组里"
        );
        term.draw(|f| draw(f, &mut app)).unwrap();

        let content: String = buffer_text(term.backend().buffer())
            .chars()
            .filter(|c| !c.is_whitespace())
            .collect();
        assert!(
            content.contains("dc-terminal"),
            "底部必须显示当前项目，实际（已去空白）: {content}"
        );
    }

    /// **牌子上要有那个名词。** 反白的 `dc/dc-terminal` 无疑是某个东西，
    /// 但底栏分三段这件事本身得先知道——而不知道它的人正是找不着自己在哪
    /// 的那个人。
    #[test]
    fn the_project_chip_says_that_it_is_a_project() {
        use ratatui::backend::TestBackend;

        for (lang, want) in [
            (crate::i18n::Lang::Zh, "项目"),
            (crate::i18n::Lang::En, "project"),
        ] {
            let mut term = Terminal::new(TestBackend::new(100, 24)).unwrap();
            let (mut app, _dir) = App::test_app();
            app.lang = lang;
            app.set_sessions(vec![SessionInfo {
                id: 1,
                profile: "claude".into(),
                dir: "/Users/lei/work/dc/dc-terminal".into(),
                state: SessionState::Idle,
                activity: String::new(),
                is_agent: true,
                tag: String::new(),
            }]);
            app.view = View::Board;
            term.draw(|f| draw(f, &mut app)).unwrap();

            let c = bar_text(&term);
            assert!(c.contains(want), "{lang:?}：牌子上少了那个名词：{c}");
            assert!(c.contains("dc-terminal"), "{lang:?}：项目名还得在：{c}");
        }
    }

    /// **窄下来时让位的是那个名词，不是名字。** 名词在所有项目上都长一样，
    /// 名字是唯一区分得出项目的东西——反过来让位等于用一个没信息量的词换掉
    /// 了唯一有信息量的那部分。牌子自己在任何宽度上都在场。
    #[test]
    fn the_chip_drops_the_label_before_it_drops_the_name() {
        let lang = crate::i18n::Lang::En;
        // 宽到写得下名词 + 一段父目录
        let roomy = bar_chip("dc-terminal", "~/work/dc", 24, lang);
        assert!(roomy.starts_with("project "), "宽的时候要写名词：{roomy}");
        assert!(roomy.contains("dc-terminal"), "名字永远在：{roomy}");

        // 刚好装不下「名词 + 名字」：名词整个让掉，名字一个字都不许少
        let tight = bar_chip("dc-terminal", "~/work/dc", 16, lang);
        assert!(!tight.contains("project"), "窄的时候名词该让位：{tight}");
        assert!(
            tight.ends_with("dc-terminal"),
            "让位换来的是完整的名字：{tight}"
        );

        // 连名字都装不下了才截名字，牌子还是不空
        let cramped = bar_chip("dc-terminal", "~/work/dc", 6, lang);
        assert!(!cramped.is_empty(), "牌子永远不空：{cramped}");
        assert!(cramped.starts_with("dc-ter"), "截的是名字的尾巴：{cramped}");
    }

    /// 中文那个名词只有 4 列（`项目`），英文是 7 列（`project`）——同一个
    /// 宽度下中文因此比英文多留得住一段父目录。这条钉住的是「名词的宽度算
    /// 的是显示列数，不是字符数」。
    #[test]
    fn the_label_is_measured_in_columns_not_characters() {
        let zh = bar_chip("dc-terminal", "~/work/dc", 20, crate::i18n::Lang::Zh);
        let en = bar_chip("dc-terminal", "~/work/dc", 20, crate::i18n::Lang::En);
        assert!(zh.starts_with("项目 "), "中文牌子：{zh}");
        assert!(
            widgets::display_width(&zh) <= 20 && widgets::display_width(&en) <= 20,
            "两种语言都不许超预算：{zh} / {en}"
        );
        assert!(
            zh.contains("dc/dc-terminal"),
            "中文名词短，同一预算下父目录留得住：{zh}"
        );
    }

    /// 光写着还不够——它得**一眼看得见**。底栏上项目名旁边全是同样加粗的
    /// 按键名，不反白的话用户要先知道「中段是项目」才认得出它，而那正是他
    /// 不知道的那件事。
    #[test]
    fn the_current_project_is_reversed_so_it_stands_out() {
        use ratatui::backend::TestBackend;

        let mut term = Terminal::new(TestBackend::new(80, 24)).unwrap();
        let (mut app, _dir) = App::test_app();
        app.set_sessions(vec![SessionInfo {
            id: 1,
            profile: "claude".into(),
            dir: "/Users/lei/work/dc/dc-terminal".into(),
            state: SessionState::Idle,
            activity: String::new(),
            is_agent: true,
            tag: String::new(),
        }]);
        app.view = View::Board;
        term.draw(|f| draw(f, &mut app)).unwrap();

        let buf = term.backend().buffer();
        let area = buf.area;
        // 项目名头一个字母所在的那一格必须是反白的。找「有字的反白格」而不是
        // 整行扫：牌子前后垫的空格也是反白的，只认空格的话，任何一片反白底
        // 都能让这条测试通过。
        let reversed = (0..area.height).any(|y| {
            (0..area.width).any(|x| {
                buf.cell((x, y))
                    .map(|c| {
                        c.symbol() == "d" && c.style().add_modifier.contains(Modifier::REVERSED)
                    })
                    .unwrap_or(false)
            })
        });
        assert!(reversed, "底栏中段的项目名要反白成一块牌子");
    }

    #[test]
    fn error_message_is_red() {
        use ratatui::backend::TestBackend;

        let mut term = Terminal::new(TestBackend::new(80, 24)).unwrap();
        let (mut app, _dir) = App::test_app();
        app.view = View::Board;
        app.message = Msg::err("不是一个目录".into());
        term.draw(|f| draw(f, &mut app)).unwrap();

        let buf = term.backend().buffer();
        let area = buf.area;
        let red = (0..area.height).any(|y| {
            (0..area.width).any(|x| {
                buf.cell((x, y))
                    .map(|c| c.style().fg == Some(Color::Red) && c.symbol() != " ")
                    .unwrap_or(false)
            })
        });
        assert!(red, "错误提示必须用红字，否则跟成功提示长得一样");
    }

    #[test]
    fn f2_is_not_forwarded_but_esc_is() {
        // F2 是逆转键，dct 自己吃掉；Esc 必须还给 agent——
        // Claude Code 靠 Esc 取消/清空/关弹窗。
        assert_eq!(key_to_input(&key(KeyCode::F(2))), None);
        assert_eq!(key_to_input(&key(KeyCode::Esc)).as_deref(), Some("\u{1b}"));
        // Ctrl+B 是 Claude Code 的「转后台」，也必须透传
        let ctrl_b = KeyEvent::new(KeyCode::Char('b'), KeyModifiers::CONTROL);
        assert_eq!(key_to_input(&ctrl_b).as_deref(), Some("\u{2}"));
    }

    #[test]
    fn f3_is_never_forwarded_to_the_agent() {
        // F3 在附加视图里被 dct 自己吃掉（跳到下一个在跑的会话），
        // 落进 key_to_input 的通配臂本来就返回 None——这条测试钉住这件事，
        // 免得以后有人改这个函数时不小心让它开始转发。
        assert_eq!(key_to_input(&key(KeyCode::F(3))), None);
    }

    #[test]
    fn bottom_bar_help_follows_the_view() {
        use ratatui::backend::TestBackend;

        let mut term = Terminal::new(TestBackend::new(100, 24)).unwrap();
        // `u 回滚` 只在选中了一个 agent 会话时才写，所以这里必须真的有一个
        // ——空看板上没有那个键不是这条测试要抓的 bug（见
        // `an_empty_board_only_offers_what_can_actually_be_done`）。
        let (mut app, _dir) = app_with_one_agent_session(View::Board);

        let text_of = |term: &Terminal<TestBackend>| -> String {
            buffer_text(term.backend().buffer())
                .chars()
                .filter(|c| !c.is_whitespace())
                .collect()
        };

        // 会话视图：绝不能显示看板的按键表——那些键在这里全被转给 agent
        app.view = View::Attached(1);
        term.draw(|f| draw(f, &mut app)).unwrap();
        let c = text_of(&term);
        // 会话视图的逃生键只有 F2 一个（Esc 归 agent，Ctrl+Q 已经不存在）。
        // 它写在左段的逃生键上——这是用户能看到的唯一一条退路。
        assert!(c.contains("F2回看板"), "会话视图要给出逆转键提示：{c}");
        assert!(
            c.contains("F3下一个会话"),
            "F3 是九宫格快速跳转的入口，提示里丢了就没人知道：{c}"
        );
        assert!(!c.contains("n新建"), "会话视图不能显示看板按键表：{c}");

        // 看板视图：仍然显示看板的按键表。
        // 必须换一个全新的 TestBackend：ratatui 画宽字符（中文）时只写首个 cell，
        // 跳过被覆盖的第二个 cell，所以复用同一个 backend 时上一帧的残字会留在
        // 那些空位里，拼出「n新回建看…」这种把两帧混在一起的假文本。真实终端上
        // 宽字符本来就盖住两列，不存在这个问题——这纯粹是测试后端的假象。
        let mut term = Terminal::new(TestBackend::new(100, 24)).unwrap();
        app.view = View::Board;
        term.draw(|f| draw(f, &mut app)).unwrap();
        let c = text_of(&term);
        assert!(c.contains("n新建"), "看板要显示自己的按键表：{c}");
    }

    /// 一句长消息可以**盖**住会话画面的最后一行，但绝不许把它挤矮：
    /// 内容区的高度就是 dct 发给 agent 的尺寸，一条转瞬即逝的提示不该
    /// 变成两次真的 resize（agent 每次都整屏重绘，Claude Code 那种按上
    /// 一帧行数抬光标的渲染器抬错一次就把输入框画丢，见 `pty::resize_parser`）。
    #[test]
    fn a_long_message_covers_the_agent_screen_instead_of_shrinking_it() {
        use ratatui::backend::TestBackend;

        let mut term = Terminal::new(TestBackend::new(60, 24)).unwrap();
        let (mut app, _dir) = app_with_one_agent_session(View::Attached(1));

        term.draw(|f| draw(f, &mut app)).unwrap();
        let quiet = app.screen_area.expect("画过一帧就该记下尺寸");

        // 60 列的窗口里底栏右段只有 28 列（`bar_widths`），而 `wrap_help`
        // 是按**两个空格**断词的，所以这句话得这么造：折三行左右，够让
        // 底栏长高，又不到「三分之一屏」那个上限被截掉。
        app.message = format!("{}END", "xxxxxx  ".repeat(8)).into();
        term.draw(|f| draw(f, &mut app)).unwrap();

        assert_eq!(
            app.screen_area,
            Some(quiet),
            "消息只该盖住画面，不该改 agent 的尺寸"
        );
        // 消息真的折了好几行才算数：底栏还是一行的话，这条测试什么都没测到。
        assert!(
            buffer_text(term.backend().buffer()).contains("END"),
            "长消息该整句显示出来（底栏为它长高了），否则这条断言是空的"
        );
    }

    #[test]
    fn a_scroll_hint_takes_over_the_bottom_bar_when_there_is_history() {
        use ratatui::backend::TestBackend;

        let mut term = Terminal::new(TestBackend::new(100, 24)).unwrap();
        let (mut app, _dir) = app_with_one_agent_session(View::Attached(1));
        app.scroll = crate::session::ScrollState {
            agent_owns: false,
            alt_screen: false,
            max: 500,
            offset: 40,
            new_lines: 0,
        };

        term.draw(|f| draw(f, &mut app)).unwrap();
        let c = buffer_text(term.backend().buffer())
            .chars()
            .filter(|c| !c.is_whitespace())
            .collect::<String>();

        // `App::test_app()` 默认 `Lang::Zh`，所以断言实际渲染出来的整句话
        // （`i18n::msg::scrolled_up` 的中文版，空白已被上面的 filter 去掉），
        // 不是随便抓两个数字——数字对了但拼错了别的字、或者 offset 算错但
        // 凑巧还是两位数，光查「有没有 4 和 0」都抓不出来。
        assert!(
            c.contains("已往上翻40行·按End回到底部"),
            "翻到哪儿了、怎么回去都要原样写在底栏：{c}"
        );
        assert!(
            !c.contains("F3下一个会话"),
            "有滚动提示可显示时，不该再挤按键表：{c}"
        );
    }

    /// 消息和滚动提示抢同一行时消息赢——消息是对用户刚才那个动作的回应，
    /// 滚动提示是持续状态，盖掉前者会让用户以为自己那步操作没反应。
    #[test]
    fn a_message_beats_the_scroll_hint() {
        use ratatui::backend::TestBackend;

        let mut term = Terminal::new(TestBackend::new(100, 24)).unwrap();
        let (mut app, _dir) = app_with_one_agent_session(View::Attached(1));
        app.scroll = crate::session::ScrollState {
            agent_owns: false,
            alt_screen: false,
            max: 500,
            offset: 40,
            new_lines: 0,
        };
        app.message = "已切到某个项目".into();

        term.draw(|f| draw(f, &mut app)).unwrap();
        let c = buffer_text(term.backend().buffer())
            .chars()
            .filter(|c| !c.is_whitespace())
            .collect::<String>();

        assert!(c.contains("已切到某个项目"), "消息该赢，没显示出来：{c}");
        assert!(!c.contains("按End回到底部"), "滚动提示不该盖过消息：{c}");
    }

    /// 模式看不见就是下一个隐形状态，而这个仓库刚花一整轮改造消灭掉那种东西。
    #[test]
    fn copy_mode_says_so_in_the_bar() {
        use ratatui::backend::TestBackend;

        for lang in [crate::i18n::Lang::Zh, crate::i18n::Lang::En] {
            let (mut app, _d) = app_with_one_agent_session(View::Attached(1));
            app.lang = lang;
            app.copy_mode = true;
            let mut term = Terminal::new(TestBackend::new(100, 24)).unwrap();
            term.draw(|f| draw(f, &mut app)).unwrap();

            let bar = bar_text(&term);
            let hint = crate::i18n::text(crate::i18n::Key::CopyMode, lang);
            assert!(
                bar.contains(&hint.replace(' ', "")),
                "{lang:?} 下底栏要写着复制模式：{bar}"
            );
        }
    }

    /// 优先级：错误消息 > 复制模式 > 滚动提示。
    ///
    /// 复制模式压过滚动提示，是因为在复制模式下滚轮根本不归 dct 管，
    /// 那条提示这时候是错的；而错误消息压过复制模式，是因为出错是一次性的、
    /// 不说就再也没机会说，复制模式则是个持续状态，下一帧还会写。
    #[test]
    fn an_error_beats_copy_mode_which_beats_the_scroll_hint() {
        use ratatui::backend::TestBackend;

        let (mut app, _d) = app_with_one_agent_session(View::Attached(1));
        app.copy_mode = true;
        app.scroll = crate::session::ScrollState {
            offset: 5,
            max: 100,
            ..Default::default()
        };
        let mut term = Terminal::new(TestBackend::new(100, 24)).unwrap();

        term.draw(|f| draw(f, &mut app)).unwrap();
        let hint = crate::i18n::text(crate::i18n::Key::CopyMode, app.lang);
        assert!(
            bar_text(&term).contains(&hint.replace(' ', "")),
            "复制模式压过滚动提示"
        );

        app.message = Msg::err("出事了".into());
        term.draw(|f| draw(f, &mut app)).unwrap();
        assert!(bar_text(&term).contains("出事了"), "错误消息压过复制模式");
    }

    /// 80 不是最窄的受支持宽度（那是下面 `MIN_COLS` 那条测试的 60），而是
    /// 长文案必须仍然放得下的那道线：右段在这个宽度下只有 39 列，而
    /// `wrap_help` 不拆单空格的句子——写长了不会折行，会被 `Paragraph`
    /// 悄悄切掉尾巴。两种语言都要在这个宽度下把复制模式的提示完整放出来，
    /// 一个字都不能少；再窄下去（见下面 60 列那条）才轮到短文案接手。
    #[test]
    fn copy_mode_hint_survives_eighty_columns_in_both_languages() {
        use ratatui::backend::TestBackend;

        for lang in [crate::i18n::Lang::Zh, crate::i18n::Lang::En] {
            let (mut app, _d) = app_with_one_agent_session(View::Attached(1));
            app.lang = lang;
            app.copy_mode = true;
            let mut term = Terminal::new(TestBackend::new(80, 24)).unwrap();
            term.draw(|f| draw(f, &mut app)).unwrap();

            let bar = bar_text(&term);
            let hint = crate::i18n::text(crate::i18n::Key::CopyMode, lang);
            assert!(
                bar.contains(&hint.replace(' ', "")),
                "{lang:?} 在 80 列下要完整显示复制模式提示，不能被切掉尾巴：{bar}"
            );
        }
    }

    /// 60 是 `grid::MIN_COLS`，一个受支持的宽度、也是常见的 tmux 分屏宽度。
    /// 这里右段只剩 `ACTION_MIN_COLS`（28）列，长文案放不下，必须换成短文案
    /// ——而短文案本身也必须完整出现，不能被 `Paragraph` 悄悄切掉尾巴。
    #[test]
    fn copy_mode_short_hint_survives_sixty_columns_in_both_languages() {
        use ratatui::backend::TestBackend;

        for lang in [crate::i18n::Lang::Zh, crate::i18n::Lang::En] {
            let (mut app, _d) = app_with_one_agent_session(View::Attached(1));
            app.lang = lang;
            app.copy_mode = true;
            let mut term = Terminal::new(TestBackend::new(60, 24)).unwrap();
            term.draw(|f| draw(f, &mut app)).unwrap();

            let bar = bar_text(&term);
            let short = crate::i18n::text(crate::i18n::Key::CopyModeShort, lang);
            assert!(
                bar.contains(&short.replace(' ', "")),
                "{lang:?} 在 60 列下要完整显示复制模式的短文案：{bar}"
            );
        }
    }

    /// 会话视图里有两个键**不写在底栏上就等于不存在**：`F4`（唯一能进复制
    /// 模式的入口）和 `F5`（唯一能把剪贴板里的图交给 agent 的入口）。这一层
    /// 按 `?` 打不开浮层（附加视图里所有键都转发给 agent），复制模式那句提示
    /// 还要等 `copy_mode` 已经是真的才画得出来——底栏是它们唯一的露面机会。
    ///
    /// `F3` 不在这条守卫里，是**故意**的：三条里只有它是快捷方式（退回看板
    /// 再进另一个会话是等价的两步），所以 `idle_help` 把它排在最先被丢的位置，
    /// 60 列那一档它确实会让出去。理由写在 `idle_help` 的 `View::Attached`
    /// 分支上。
    ///
    /// 两个宽度都要测，不能只测 80：`fit_help` 是按预算从前面**丢**的，
    /// 60 列这一档右段落到 `ACTION_MIN_COLS`（28）——两条在这个地板上挤不挤
    /// 得下，只有真的在这个宽度画一遍才知道；只测 80 列的话，`F4`/`F5`
    /// 在窄终端上被 `fit_help` 悄悄丢掉的回归会一路绿灯漏过去。
    #[test]
    fn attached_view_bar_keeps_both_f4_and_f5_at_eighty_and_sixty_columns() {
        use ratatui::backend::TestBackend;

        for width in [80u16, 60u16] {
            for lang in [crate::i18n::Lang::Zh, crate::i18n::Lang::En] {
                let (mut app, _d) = app_with_one_agent_session(View::Attached(1));
                app.lang = lang;
                let mut term = Terminal::new(TestBackend::new(width, 24)).unwrap();
                term.draw(|f| draw(f, &mut app)).unwrap();

                let bar = bar_text(&term);
                assert!(
                    bar.contains("F5"),
                    "{width} 列 {lang:?} 下 F5 不见了——这是这一层唯一能粘贴图片的入口：{bar}"
                );
                assert!(
                    bar.contains("F4"),
                    "{width} 列 {lang:?} 下 F4 不见了——这是这一层唯一能进复制模式的入口：{bar}"
                );
            }
        }
    }

    /// F6 回归测试：英文滚动提示走的是 `BarContent::Text`，`wrap_help`
    /// 只在连续两个空格的地方才折行，而这句提示全是单空格，放不下就不是
    /// 折行，是被 `Paragraph`（没挂 `.wrap()`）直接从右边截断。原来的
    /// "↑ Scrolled up 40 line(s) · press End to jump back down" 有 54 列，
    /// 底栏右段宽度是「终端总宽 − 23」，55 列的终端只有 32 列可用——刚好
    /// 卡在「怎么回去」那半句中间，`End` 整个词被截没了：这个宽度算出的
    /// 切点（`old[:32]`）落在 "press " 之后、"End" 之前，用户会看到自己
    /// 正翻着历史，却读不到任何一个字告诉他怎么回去。缩短后的新版本
    /// （28 列）在同样的宽度下完整放得下。
    #[test]
    fn the_way_back_survives_a_narrow_terminal() {
        use ratatui::backend::TestBackend;

        let mut term = Terminal::new(TestBackend::new(55, 24)).unwrap();
        let (mut app, _dir) = app_with_one_agent_session(View::Attached(1));
        app.lang = crate::i18n::Lang::En;
        app.scroll = crate::session::ScrollState {
            agent_owns: false,
            alt_screen: false,
            max: 500,
            offset: 40,
            new_lines: 0,
        };

        term.draw(|f| draw(f, &mut app)).unwrap();
        let c = buffer_text(term.backend().buffer())
            .chars()
            .filter(|c| !c.is_whitespace())
            .collect::<String>();

        assert!(
            c.contains("End"),
            "55 列的终端上，用户翻到哪儿都不该看不到怎么回去：{c}"
        );
    }

    /// 副作用（`execute!` 往 stdout 写转义序列）没法单测，但「这一帧该不该
    /// 动一下捕获」这个判断能测——`run()` 每帧都靠它决定要不要发
    /// `EnableMouseCapture`/`DisableMouseCapture`，判断错了要么漏开
    /// （滚轮/点击走了终端自己的逻辑）要么漏关（退回看板还拖选不了文字）。
    #[test]
    fn mouse_capture_toggles_only_on_a_real_transition() {
        assert_eq!(mouse_capture_transition(false, false), None);
        assert_eq!(mouse_capture_transition(true, true), None);
        assert_eq!(mouse_capture_transition(false, true), Some(true));
        assert_eq!(mouse_capture_transition(true, false), Some(false));
    }

    /// 三个条件的真值表，八种组合全列。
    ///
    /// 穷举而不是挑几个代表：这个函数错一格的后果是「用户在会话里复制不了」
    /// 或者「agent 收不到它订阅的鼠标」，两种都不会 panic、不会报错，只会让人
    /// 觉得工具坏了却说不清哪儿坏。八行断言比一句「应该没问题」便宜得多。
    #[test]
    fn mouse_is_captured_only_when_all_three_conditions_hold() {
        // attached, agent_subscribed, copy_mode -> want
        assert!(wants_mouse_capture(true, true, false));
        assert!(!wants_mouse_capture(true, true, true), "复制模式一票否决");
        assert!(
            !wants_mouse_capture(true, false, false),
            "agent 不要鼠标就别抓——抓了用户就白白丢了拖选复制"
        );
        assert!(!wants_mouse_capture(true, false, true));
        assert!(!wants_mouse_capture(false, true, false), "看板上永远不抓");
        assert!(!wants_mouse_capture(false, true, true));
        assert!(!wants_mouse_capture(false, false, false));
        assert!(!wants_mouse_capture(false, false, true));
    }

    /// 断连（或者这一轮压根没拿到 `Response::Screen`）时，`app.scroll`
    /// 必须原样保留上一帧的值，不能被复位——复位会让 `wants_mouse_capture`
    /// 在断连的每一帧里反复翻转捕获状态，那是往 stdout 反复写转义序列，
    /// 断连时这是最吵的一种失败。
    ///
    /// 这里直接喂 `scroll_after_screen_call` 一个 `Err`，断言拿回来的就是
    /// 传进去的 `previous`。早先这个位置放的是一个只会通过的假测试：它把
    /// `app.connected` 设成 `false`——一个 `wants_mouse_capture` 根本不读
    /// 的字段——然后拿完全相同的参数把同一个纯函数又调用了一遍；任何确定性
    /// 函数在这种写法下都必然通过，哪怕 `run()` 的断连分支真被改成把
    /// `app.scroll` 复位成 `Default`（这正是它宣称要防住的回归），那个
    /// 测试也照样是绿的。抽出 `scroll_after_screen_call` 就是为了让这条
    /// 属性能被真正测到：破坏它的实现（比如把 `_ => previous` 改成
    /// `_ => crate::session::ScrollState::default()`），这个测试会红。
    #[test]
    fn a_failed_screen_call_does_not_flip_the_capture_state() {
        let previous = crate::session::ScrollState {
            agent_owns: true,
            max: 10,
            offset: 3,
            ..Default::default()
        };
        let failed: Result<Response> = Err(anyhow::anyhow!("daemon unreachable"));

        assert_eq!(scroll_after_screen_call(previous, &failed), previous);
    }

    /// 反过来：请求真的成功时，`scroll` 必须换成新的一份，不能沿用旧的。
    /// 跟上一条测试各守一半——少了这一条，把 `scroll_after_screen_call`
    /// 整个写成 `|previous, _| previous` 也能让「断连不翻转」那条测试通过。
    #[test]
    fn a_successful_screen_call_replaces_the_scroll_state() {
        let previous = crate::session::ScrollState::default();
        let fresh = crate::session::ScrollState {
            agent_owns: true,
            max: 10,
            offset: 3,
            ..Default::default()
        };
        let ok: Result<Response> = Ok(Response::Screen {
            lines: Vec::new(),
            cursor: (0, 0),
            cursor_hidden: false,
            state: SessionState::Idle,
            scroll: fresh,
        });

        assert_eq!(scroll_after_screen_call(previous, &ok), fresh);
    }

    /// 配色浮层开着时，底栏写的是**浮层自己那三个键**。
    ///
    /// 会话那几条 F 键这时候一个都按不动（浮层是模态的，见
    /// `attach::the_picker_is_modal_and_swallows_everything_else`），
    /// 继续写着它们就是在宣传按不动的键。
    #[test]
    fn the_bottom_bar_hands_the_line_to_the_open_color_picker() {
        let (mut app, _d) = App::test_app();
        app.view = View::Attached(1);

        let before = bar_keys(&app, 80);
        assert!(
            before.iter().any(|i| i.key == "F4"),
            "前提：平时这一行写的是会话的 F 键"
        );

        attach::handle_key(&mut app, KeyEvent::new(KeyCode::F(6), KeyModifiers::NONE)).unwrap();
        let during = bar_keys(&app, 80);

        let keys: Vec<&str> = during.iter().map(|i| i.key).collect();
        assert_eq!(keys, vec!["↑↓", "Enter", "Esc"]);
    }

    #[test]
    fn a_fresh_app_is_not_in_copy_mode() {
        let (app, _d) = App::test_app();
        assert!(!app.copy_mode);
    }
}
