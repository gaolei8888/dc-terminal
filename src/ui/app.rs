use std::path::PathBuf;

use anyhow::{anyhow, Result};
use ratatui::widgets::ListState;

use crate::client::Client;
use crate::pty::ScreenSpan;
use crate::session::SessionInfo;
use crate::verify::VerifyOutcome;

use super::view::View;
use super::widgets::Msg;

/// `run()` 主循环那一屏的全部状态。拆出来是为了让 `run()` 本身只剩终端
/// 生命周期管理（raw mode / alternate screen / signal 还原），状态怎么变
/// 全部收在这。
pub struct App {
    // `client` 是 `Option<Client>` 而不是 `Client`：测试要能构造一个没有
    // 连接的 App（见 `new_disconnected`），不能要求测试先起一个真实的
    // 守护进程。所有需要调用它的地方走 `client()`，`None` 时统一返回
    // 「守护进程连不上」——这跟真实断线走的是同一条错误路径，不为
    // 「构造不出连接」单独开一条新分支。
    pub client: Option<Client>,
    pub view: View,
    pub list_state: ListState,
    // 守护进程返回的**全量**列表。除了重算 `visible`，界面不该直接读它——
    // 读了就会把别的项目的会话画上屏，那正是这套机制要修的 bug。
    pub sessions: Vec<SessionInfo>,
    // 当前作用域下真正上屏的那些，由 `refresh_visible()` 从 `sessions` 派生。
    //
    // 派生出一份 Vec 而不是留一个过滤器函数或一张下标映射表：`board`/`grid`/
    // `mod` 里的光标、焦点、`session_action` 全都是**按下标**索引会话的。
    // 换成一份已经筛好的 Vec，这些调用点逻辑一个字都不用改；映射表则要求
    // 每一处都记得转换一次，漏一处就是「选中第 3 行、停掉第 5 个会话」
    // 这种最难查的 bug。
    pub visible: Vec<SessionInfo>,
    pub scope: super::view::Scope,
    pub message: Msg,
    pub screen: Vec<Vec<ScreenSpan>>,
    pub screen_cursor: (u16, u16),
    // 九宫格当前页那几个会话的画面，按 id 跟格子配对。跟 `screen` 分开存：
    // 那一份是附加视图正在放大的**单个**会话，两者的刷新节奏和来源消息
    // 都不一样（16ms 的 `Screen` vs 300ms 的 `Screens`），共用一个字段
    // 只会让退出九宫格再进会话时看到上一屏的残影。
    pub grid_screens: Vec<crate::proto::ScreenEntry>,
    // 上一次批量取画面的时刻，用来把九宫格的刷新压到 300ms 一轮。
    // `None` = 还没取过，下一轮立刻取。
    pub grid_last_fetch: Option<std::time::Instant>,
    // `grid_screens` 装的是哪一页。翻页之后要立刻取一次新页的画面，否则
    // 新的一页会空白着晾用户小半秒；用「页码变了」这个一次性信号来触发，
    // 而不是「有哪个格子还没有画面」——后者在会话刚好消失的那一瞬会一直
    // 为真，把 300ms 的节流整个绕过去，退化成每个 tick 一次阻塞往返。
    pub grid_page: Option<usize>,
    // 上次告诉 agent 的画面尺寸，变了才发 Resize，避免每帧一次多余请求
    pub sent_size: Option<(u32, u16, u16)>,
    // 连不上守护进程 / 请求失败时置 false，看板上要能看出数据是陈旧的，
    // 不能让用户以为界面上的"干活中"还代表当前真实状态。每次循环开头的
    // List（以及 Attached 视图下的 Screen）调用是唯一的真相来源——它总在
    // 当次的 term.draw 之前重新算一遍，所以不需要（也不应该）预置初值。
    pub connected: bool,
    // 进了会话就不用再每轮拉 List：它是给看板用的，而且服务端要逐个锁会话、
    // 取每个会话的最后一行，纯属浪费。只在看板上、或刚从会话里退出来时拉一次。
    pub need_sessions: bool,
    // 密钥验证是网络调用，不能在按键循环里直接跑——会话视图 16ms 一刷，
    // 一次阻塞就是整个界面冻住。丢给后台线程，主循环每轮 try_recv。
    // 放在 View 外面是因为 View 要 Clone（`match view.clone()`），而
    // `mpsc::Receiver` 不能 Clone，没法塞进一个要 Clone 的枚举里。
    //
    // 元组里带着发起这次验证时的 (profile, buf)，不是只传一个 `VerifyOutcome`
    // ——这是 CRITICAL 1 code review 发现的事故的修复：验证是异步的，结果
    // 送回来的这一刻，屏幕上未必还是发起验证时的那个视图。用户可能已经
    // Ctrl+Q/Esc 退出去，甚至绕回来在另一个 agent 身上重新填了密钥；旧写法
    // 只看"现在还是不是 EnterSecret 视图"，对不上具体是哪一个 profile、哪一份
    // 密钥，就会把一次过期的验证结果套在一个完全不相干的 profile 上，
    // 写出一份用户从没见过、甚至是空的"密钥"。把发起时的身份跟结果一起带
    // 回来，收的时候现比对一遍（见 `verify_outcome_applies_to`），这样不管
    // 是哪条退出路径忘了清 `verify_rx`，错位的结果都应用不到屏幕上去——
    // 不必把这条防线押在"每个退出分支都记得清 receiver"这种容易漏改的
    // 纪律上。
    pub verify_rx: Option<std::sync::mpsc::Receiver<(String, String, VerifyOutcome)>>,
    // start_dir 是 dct 启动时的目录，只用来解析用户敲进来的相对路径，永不改变。
    // current_dir 是「新会话开在哪」，选择器会改它。
    /// 界面语言。启动时由 `i18n::resolve` 定一次（DCT_LANG > 存过的设置 >
    /// 系统 locale > En），设置页改它时同时写盘。守护进程不持有这个——
    /// 它是常驻的、可能同时服务多个界面的进程，见 `Request::Profiles`。
    pub lang: crate::i18n::Lang,
    /// 守护进程 socket 的路径。设置文件就在它旁边（见
    /// `settings::settings_path_for_socket`），所以改语言时要用到它。
    /// 存路径而不是存设置文件路径：将来别的「跟着 socket 走」的文件
    /// （profiles 目录、projects.json）也都从它推导，只留一个源头。
    pub socket: PathBuf,
    pub start_dir: PathBuf,
    pub current_dir: PathBuf,
    pub quit: bool,
}

impl App {
    /// 两个构造函数共用的字段初值——除了 `client` 之外的每一项，`new` 和
    /// `new_disconnected` 必须给出完全一样的答案。拆出来是因为一旦两份
    /// 分开抄，改 `new` 忘了同步改测试用的那份，测试就会在悄悄测一个跟
    /// 生产环境不一样的初值，形同没测（这正是本函数存在的原因，见
    /// `a_fresh_app_starts_on_the_board_with_nothing_stale`）。
    fn new_inner(
        client: Option<Client>,
        default_dir: PathBuf,
        lang: crate::i18n::Lang,
        socket: PathBuf,
    ) -> App {
        App {
            client,
            view: View::Board,
            list_state: ListState::default(),
            sessions: Vec::new(),
            visible: Vec::new(),
            // 每次启动都从「只看当前项目」开始。作用域不持久化：它是个
            // 临时的查看动作，不是配置。
            scope: super::view::Scope::CurrentProject,
            message: "".into(),
            screen: Vec::new(),
            screen_cursor: (0, 0),
            grid_screens: Vec::new(),
            grid_last_fetch: None,
            grid_page: None,
            sent_size: None,
            // 每轮循环开头的 List 调用是唯一的真相来源，它总在当次
            // term.draw 之前重新算一遍，所以这里给什么都会被立刻覆盖。
            connected: true,
            need_sessions: true,
            verify_rx: None,
            lang,
            socket,
            start_dir: default_dir.clone(),
            current_dir: default_dir,
            quit: false,
        }
    }

    pub fn new(
        client: Client,
        default_dir: PathBuf,
        lang: crate::i18n::Lang,
        socket: PathBuf,
    ) -> App {
        Self::new_inner(Some(client), default_dir, lang, socket)
    }

    /// 只给测试用：不需要一个活的守护进程就能构造。
    #[cfg(test)]
    pub fn new_disconnected(sock: PathBuf, default_dir: PathBuf) -> App {
        Self::new_inner(None, default_dir, crate::i18n::Lang::Zh, sock)
    }

    /// 只给测试用：`board`/`attach`/`pick`/`secret` 里 `draw()`/`handle_key()`
    /// 的单测要喂一个 `App`，但用不上真实守护进程，也不关心 `current_dir`
    /// 具体是哪——用临时目录垫一个就行。
    ///
    /// 必须把 `TempDir` guard 跟 `App` 一起交出去：`tempfile::tempdir()` 返回的
    /// 目录在 guard 被 drop 的那一刻就从磁盘上删掉。如果这里只取
    /// `dir.path()` 垫两个字段就让 `dir` 在函数结束时被丢弃，`current_dir`
    /// 存的会是一个刚被删除的路径——大多数单测只是把这个路径当字符串用，
    /// 不会踩到，但只要有一天哪个测试真的碰了文件系统（`is_dir()`、
    /// `expand_path` 之类），就会在一个已经不存在的目录上悄无声息地失败。
    /// 调用方把返回的 `TempDir` 存在自己的局部变量里，让它活到测试函数
    /// 结束——不接住这个返回值，目录一样会被立刻删掉。
    #[cfg(test)]
    pub(crate) fn test_app() -> (App, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let app = Self::new_disconnected(dir.path().join("s.sock"), dir.path().to_path_buf());
        (app, dir)
    }

    /// 装入守护进程刚返回的会话列表。**赋值和重算必须成对发生**，所以
    /// 主循环走这条路而不是直接写 `app.sessions = v`——直接赋值的话，
    /// 这一帧还会拿着上一轮的 `visible` 去画，刚开的会话要等下一轮才出现。
    pub fn set_sessions(&mut self, v: Vec<SessionInfo>) {
        self.sessions = v;
        self.refresh_visible();
    }

    /// 从 `sessions` 重算 `visible`，然后把光标和焦点收拢进新的范围。
    ///
    /// 只该在三个地方调用，多一处是白算，少一处是屏幕说谎：
    /// 拉到新的会话列表之后、`scope` 被切换时、`current_dir` 被改变时。
    pub fn refresh_visible(&mut self) {
        self.visible = super::view::visible_sessions(&self.sessions, self.scope, &self.current_dir);

        // 收拢到**末项**而不是首项：用户的注意力在列表尾部时，弹回第一行
        // 会让他以为自己选中的会话被删了。
        match self.visible.len() {
            0 => {
                // 零个 item 上还留着 Some(0)，List 会画一条悬空的高亮
                self.list_state.select(None);
                if let View::Grid { focus } = &mut self.view {
                    *focus = 0;
                }
            }
            n => {
                let last = n - 1;
                // 只在越界时才动它。没选中过就保持没选中——这里不负责
                // 「替用户选一个」，那是 `move_sel` 和进入视图时的事。
                if let Some(i) = self.list_state.selected() {
                    if i > last {
                        self.list_state.select(Some(last));
                    }
                }
                if let View::Grid { focus } = &mut self.view {
                    *focus = (*focus).min(last);
                }
            }
        }
    }

    /// 拿到活的守护进程连接；构造时没能连上（目前只有测试会这样构造）就
    /// 报「守护进程连不上」——跟真实断线共用同一条错误路径，调用方不用
    /// 为“压根没连过”单独判一次。
    pub fn client(&mut self) -> Result<&mut Client> {
        self.client
            .as_mut()
            .ok_or_else(|| anyhow!("守护进程连不上"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 新建的 App 必须落在看板上、没有陈旧消息、认为自己是连着的。
    /// 这三个初值任何一个错了，用户开机第一眼看到的就是错的。
    #[test]
    fn a_fresh_app_starts_on_the_board_with_nothing_stale() {
        let dir = tempfile::tempdir().unwrap();
        let sock = dir.path().join("nope.sock");
        // 连不上也要能构造：构造 App 不该有副作用
        let app = App::new_disconnected(sock, dir.path().to_path_buf());
        assert!(matches!(app.view, View::Board));
        assert_eq!(app.message.text, "");
        assert!(!app.quit);
        assert!(app.need_sessions, "开机第一轮必须拉一次会话列表");
    }

    fn sess(id: u32, dir: &str) -> SessionInfo {
        SessionInfo {
            id,
            profile: "claude".into(),
            dir: dir.into(),
            state: crate::session::SessionState::Idle,
            activity: String::new(),
        }
    }

    /// 过滤会让列表变短，而光标是个下标。不收拢的话，`grid.rs` 里
    /// `move_focus` 的 `total - page_start` 在 release 下会**下溢**，
    /// debug 下则直接撞上那条 `debug_assert!(focus < total)`。
    #[test]
    fn refresh_visible_clamps_the_cursor_into_the_new_range() {
        let (mut app, _dir) = App::test_app();
        app.current_dir = PathBuf::from("/w/a");
        app.sessions = vec![
            sess(1, "/w/a"),
            sess(2, "/w/b"),
            sess(3, "/w/b"),
            sess(4, "/w/b"),
            sess(5, "/w/a"),
        ];
        app.list_state.select(Some(4));
        app.view = View::Grid { focus: 4 };

        app.refresh_visible();

        assert_eq!(app.visible.len(), 2, "当前项目只有两个会话");
        assert_eq!(
            app.list_state.selected(),
            Some(1),
            "光标收拢到末项而不是首项——注意力在列表尾部时弹回第一行，\
             用户会以为自己选中的会话没了"
        );
        assert!(matches!(app.view, View::Grid { focus: 1 }), "焦点同样收拢");
    }

    /// 作用域空了就没有格子可焦。光标必须是 `None` 而不是 `Some(0)`：
    /// `Some(0)` 会让列表在零个 item 上画高亮条。
    #[test]
    fn refresh_visible_drops_the_cursor_when_nothing_is_visible() {
        let (mut app, _dir) = App::test_app();
        app.current_dir = PathBuf::from("/w/a");
        app.sessions = vec![sess(1, "/w/b")];
        app.list_state.select(Some(0));
        app.view = View::Grid { focus: 3 };

        app.refresh_visible();

        assert!(app.visible.is_empty());
        assert_eq!(app.list_state.selected(), None);
        assert!(matches!(app.view, View::Grid { focus: 0 }));
    }

    /// 主循环每轮拿到新的会话列表时走这条路。存在的理由是「赋值 + 重算」
    /// 必须成对发生 —— 直接写 `app.sessions = v` 的话，屏幕会拿旧的
    /// `visible` 再画一帧，刚开的会话要等下一轮才出现。
    #[test]
    fn set_sessions_recomputes_the_visible_list() {
        let (mut app, _dir) = App::test_app();
        app.current_dir = PathBuf::from("/w/a");

        app.set_sessions(vec![sess(1, "/w/a"), sess(2, "/w/b")]);

        assert_eq!(
            app.visible.iter().map(|s| s.id).collect::<Vec<_>>(),
            vec![1],
            "拿到新列表的同一时刻就要筛好，不能留到下一帧"
        );
    }

    /// start_dir 只用来解析用户敲的相对路径，永不改变；current_dir 是
    /// 「下一个会话开在哪」，选择器会改它。两者一开始相同但不是同一个东西，
    /// 合并成一个字段会让「换了项目之后再敲相对路径」解析到错的基准目录。
    #[test]
    fn start_dir_and_current_dir_are_separate_fields() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = App::new_disconnected(dir.path().join("s.sock"), dir.path().to_path_buf());
        app.current_dir = PathBuf::from("/somewhere/else");
        assert_eq!(app.start_dir, dir.path());
    }
}
