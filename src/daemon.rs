use anyhow::Result;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crate::channel::telegram::Telegram;
use crate::channel::{ChannelError, Event};
use crate::profile::Profile;
use crate::profile::{all_profiles, command_exists, profiles_dir_for_socket, status_of};
use crate::projects::{store_path_for_socket, Store};
use crate::proto::{
    ErrorCode, InstallPrompt, PhoneState, PhoneStatus, ProfileEntry, Request, Response,
    SecretPrompt,
};
use crate::secrets::{secrets_path_for_socket, SecretStore, PHONE_OWNER_KEY, PHONE_TOKEN_KEY};
use crate::session::{recover, SessionManager};
use crate::verify::{send_probe, verify_with, VerifyOutcome};

pub fn run(socket: &Path) -> Result<()> {
    let mgr = SessionManager::new();
    // 生死簿只在真正的守护进程里落盘。单元测试自己 `new()` 一个 manager，
    // 拿到的是不记账的那种——绝不能去写用户真实的 `~/.dct/sessions.log`。
    mgr.journal
        .set_path(crate::proto::journal_path_for_socket(socket));
    run_with_manager(socket, Arc::new(mgr))
}

/// 供测试注入自定义 `SessionManager`（比如预先 `register_profile` 一个专供测试用的慢
/// profile），`run()` 只是用一个全新的 manager 调用它。
///
/// 这里不再包一层 `Mutex<SessionManager>`：`SessionManager` 自己已经是内部可变的
/// （见 `session.rs` 的注释），各方法自己负责该锁多细的粒度。如果外面再套一把大锁，
/// 就白做了那些细粒度设计——一个连接处理慢请求时又会把其它连接一起卡住。
pub fn run_with_manager(socket: &Path, mgr: Arc<SessionManager>) -> Result<()> {
    // 权限必须收紧到只有属主可访问。这个 socket 能开会话、能往会话里发任意
    // 输入——谁连得上，谁就能在这台机器上以你的身份执行任意命令。默认的 0755
    // 意味着同机器的其它账号都能连。
    if let Some(parent) = socket.parent() {
        std::fs::create_dir_all(parent)?;
        std::fs::set_permissions(parent, std::fs::Permissions::from_mode(0o700))?;
    }
    let _ = std::fs::remove_file(socket);
    let listener = UnixListener::bind(socket)?;
    std::fs::set_permissions(socket, std::fs::Permissions::from_mode(0o600))?;

    // 存放位置跟着 socket 走，测试把 socket 放临时目录就自动隔离，
    // 不会去动真实的 ~/.dct/projects.json / ~/.dct/secrets.toml / ~/.dct/profiles/。
    let store = Arc::new(Mutex::new(Store::load(&store_path_for_socket(socket))));
    let secrets = Arc::new(Mutex::new(SecretStore::load(&secrets_path_for_socket(
        socket,
    ))));
    let profiles_dir = profiles_dir_for_socket(socket);
    // Ruling 3：这份状态槽是 `Request::PhoneStatus` 唯一的答案来源，也是
    // Task 5 的 bridge 线程要写的那个地方——两边共用同一把 `Mutex`，谁先谁后
    // 都不会看到半份数据。启动时只看密钥仓里有没有令牌：**不**在这里打一次
    // `getMe`（daemon 启动不该依赖网络才能起来，况且这条路径没法在单测里
    // 避开真实网络）。bot 用户名和是否已经配上人，都要等 bridge 真的跑起来
    // 之后才补全——这是 Task 5 的范围，这里先给一个诚实但不完整的初值。
    let phone = {
        let s = recover(secrets.lock());
        Arc::new(Mutex::new(initial_phone_status(&s)))
    };
    // 重启时密钥仓里已经有令牌：得立刻把 bridge 起起来，不然 `bot` 会一直
    // 停在 `None`——`ui/phone.rs::status_line` 那条「正在重新接上」的诚实
    // 文案只在这段窗口够短的前提下才站得住（Task 4 遗留发现，Task 5 来关）。
    // 没有令牌就什么都不做：`Bridge::new` 需要一个真的渠道，编不出一个。
    //
    // 这个槽是整个手机通道生命周期唯一的入口——重启、`PhoneSetToken`、
    // `PhoneUnpair`、`PhoneDisable` 全部通过它改变"当前是哪个 bridge 在跑"。
    // 任何时刻这里最多只有一个 `Some`，见 `bridge::replace`/`stop_current`
    // 的文档注释（C2/C3 的修复）。
    let bridge: Arc<Mutex<Option<crate::bridge::BridgeHandle>>> = Arc::new(Mutex::new(None));
    start_phone_bridge(&secrets, &phone, &bridge, &|token| {
        Arc::new(Telegram::new(token)) as Arc<dyn crate::channel::Channel>
    });

    // Ruling 10：把 `tick()` 那头 unbounded 的 `mpsc::Sender<Event>` 接到
    // 这唯一常驻的消费者线程上——它读的是 `bridge` 这个槽，不是某一个具体
    // 的 `Bridge` 实例，换令牌/关掉都不会让它跟着重启或者留下第二条线程，
    // 见 `bridge::spawn_event_consumer` 的文档。`bridge` 此刻可能还是
    // `None`（没写过令牌）——没关系，消费者每次收到事件才现查槽里是谁。
    let (event_tx, event_rx) = std::sync::mpsc::channel::<Event>();
    mgr.set_event_sink(event_tx);
    crate::bridge::spawn_event_consumer(event_rx, bridge.clone());

    // 出错解释要用的后端：进程一启动就 resolve 一次，不是每次会话失败才现查
    // ——`tick()` 绝不能在判失败的那一刻还去做「找后端」这种可能失败的活。
    // 抽成独立函数是为了能不起真实 socket/listener 就单测「没写 [llm] 就不该
    // 装后端」这条 Critical 修复本身，见下面 `install_llm_backend` 和它的测试。
    install_llm_backend(socket, &profiles_dir, &mgr);

    let tick_mgr = mgr.clone();
    std::thread::spawn(move || loop {
        std::thread::sleep(Duration::from_millis(200));
        tick_mgr.tick();
    });

    for conn in listener.incoming() {
        let conn = conn?;
        let m = mgr.clone();
        let s = store.clone();
        let sec = secrets.clone();
        let pd = profiles_dir.clone();
        let ph = phone.clone();
        let br = bridge.clone();
        std::thread::spawn(move || {
            if let Err(e) = serve(conn, m, s, sec, pd, ph, br) {
                eprintln!("连接处理失败: {e}");
            }
        });
    }
    Ok(())
}

/// 守护进程刚起来（或者刚被 `run_with_manager` 构造出来）时，手机通知该
/// 处在哪个状态——只看密钥仓里有没有令牌，理由见调用点的注释。
fn initial_phone_status(secrets: &SecretStore) -> PhoneStatus {
    match secrets.get(PHONE_TOKEN_KEY) {
        Some(_) => PhoneStatus {
            state: PhoneState::WaitingForPairing,
            bot: None,
            owner: None,
        },
        None => PhoneStatus {
            state: PhoneState::Off,
            bot: None,
            owner: None,
        },
    }
}

/// 密钥仓里那份持久化的 owner 到底是什么状态——**三种情况必须分得清**，
/// 不能只有"有"和"没有"两种：
///
/// - `None`：从没配对过（或者是一次全新的令牌），**可以**打开配对。
/// - `Known`：配对完成过，值读得出来，直接拿去恢复，不重新打开配对。
/// - `Corrupt`：这个键**存在**，但读不出一个合法的 chat id（F3）。这
///   **不等于** `None`——`.and_then(...).ok()` 那种"解析失败就退化成
///   `None`"的写法会把它悄悄合并进"可以打开配对"，而"配对信息读不出来"
///   和"从没配过对"完全是两件事：前者曾经有一个真主人，只是这一条记录
///   坏了；把它当成后者处理，等于在 Ruling 9 明确禁止的地方又把门打开
///   给了任何人。正确的处理是拒绝启动配对，把这件事说给用户听。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StartupOwner {
    None,
    Known(i64),
    Corrupt,
}

/// 重启时该拿哪个 chat id 当 `Bridge` 的 `owner`——**必须**读密钥仓里
/// 持久化的那份，不能镶死成 `None`。镶死成 `None` 就是 C1：bot 用户名
/// 公开可搜，攻击者只要趁 dct 关着抢先发消息，重启后一旦 `owner` 又从
/// `None` 起步，`Bridge` 就会重新把"谁先发消息"当成配对的依据。
fn startup_bridge_owner(secrets: &SecretStore) -> StartupOwner {
    match secrets.get(PHONE_OWNER_KEY) {
        None => StartupOwner::None,
        Some(v) => match v.parse::<i64>() {
            Ok(id) => StartupOwner::Known(id),
            // F3：读不出来**绝不能**退化成 `None`——那等于把"这条记录坏了"
            // 当成"从没配过对"，重新把配对的门打开给第一个发消息的人。
            Err(_) => StartupOwner::Corrupt,
        },
    }
}

/// 配对完成那一刻，`Bridge` 要把 chat id 落盘用的回调。**跟令牌用同一个
/// 密钥仓、同一套原子写**——没有必要另起一份存储机制。落盘失败（比如
/// 磁盘满了）不往上抛：手机通知这条链路的原则跟 `journal.rs` 一样，
/// 记不下来是记账的事，不该连累到这条消息本身已经被正确处理这件事；
/// 唯一的代价是下次重启这段"重新打开配对"的窗口会意外地再出现一次，
/// 这一点在 stderr 留痕，方便事后查。
fn persist_owner_closure(secrets: Arc<Mutex<SecretStore>>) -> Box<dyn Fn(i64) + Send + Sync> {
    Box::new(move |chat_id: i64| {
        if let Err(e) = recover(secrets.lock()).set(PHONE_OWNER_KEY, &chat_id.to_string()) {
            eprintln!("手机配对的主人 id 落盘失败，重启后配对可能重新打开: {e}");
        }
    })
}

/// 守护进程启动时（或者被测试直接调用时）该不该起一个 bridge、起哪个。
/// **`make_channel` 是唯一的网络入口**，被抽成参数是因为这条路径以前完全
/// 没法在单测里验证"重启到底有没有认得持久化的 owner"——真实的
/// `Telegram::new` 会在后台线程里真的打一次网络。测试注入一个不碰网络、
/// 光靠自己的字段回答问题的假渠道，就能直接钉住 `owner` 参数传对了没有，
/// 不用管这次网络请求成不成功。
fn start_phone_bridge(
    secrets: &Arc<Mutex<SecretStore>>,
    phone: &Arc<Mutex<PhoneStatus>>,
    bridge: &Arc<Mutex<Option<crate::bridge::BridgeHandle>>>,
    make_channel: &dyn Fn(&str) -> Arc<dyn crate::channel::Channel>,
) {
    let (token, owner_state) = {
        let s = recover(secrets.lock());
        match s.get(PHONE_TOKEN_KEY) {
            Some(token) => (Some(token.to_string()), startup_bridge_owner(&s)),
            None => (None, StartupOwner::None),
        }
    };
    let Some(token) = token else {
        return;
    };

    // **F3 的修复。** 配对信息读不出来，不是"从没配过对"——绝不能当成
    // `None` 打开配对，那样任何人都能抢在真主人前面重新配对成功。老实
    // 拒绝起 bridge，把这件事说给用户听，让他知道该怎么办（重新粘贴一遍
    // 令牌：`Request::PhoneSetToken` 会清掉这条坏记录，见那边的注释）。
    let owner = match owner_state {
        StartupOwner::None => None,
        StartupOwner::Known(id) => Some(id),
        StartupOwner::Corrupt => {
            recover(phone.lock()).state =
                PhoneState::Broken("手机配对信息读不出来了，去设置页重新粘贴一遍令牌".to_string());
            return;
        }
    };

    let ch = make_channel(&token);
    crate::bridge::replace(
        bridge,
        ch,
        phone.clone(),
        owner,
        persist_owner_closure(secrets.clone()),
    );
}

/// **`cfg.llm` 是 `None` 就什么都不做**：不 resolve、不装后端、也不打印
/// 任何一行——这是绝大多数用户的正常状态（没写过 `[llm]`），不是一种
/// 「本来该配却没配好」的错误。见 `config.rs` 头注释：出错解释会把一个
/// 失败会话屏幕上的原始内容送给模型，这必须是用户自己写下 `[llm]` 才
/// 触发的动作，不能因为「什么都没配」就替他打开、把他终端里的东西发
/// 给第三方。只有用户确实写了 `[llm]` 却指向一个连不上的后端时，才值得
/// 在 stderr 上留一行——那时候他大概率是想用这功能的，只是配错了。
fn install_llm_backend(socket: &Path, profiles_dir: &Path, mgr: &SessionManager) {
    let Some(llm) =
        &crate::config::Config::load(&crate::config::config_path_for_socket(socket)).llm
    else {
        return;
    };
    let llm_secrets = SecretStore::load(&secrets_path_for_socket(socket));
    let (custom, _) = all_profiles(profiles_dir);
    let lookup = |n: &str| {
        custom
            .iter()
            .find(|p| p.name == n)
            .cloned()
            .or_else(|| Profile::builtin(n))
    };
    match crate::llm::resolve::resolve(llm, &lookup, &llm_secrets, &startup_oauth) {
        Ok(b) => {
            mgr.set_backend(Some(b));
            mgr.set_llm_problem(None);
        }
        Err(e) => {
            // stderr 这一行只有 `dct daemon` 前台跑的时候看得见：界面进程
            // 拉起守护进程时把 stderr 接到了 `/dev/null`（`spawn_daemon`——
            // 不然每一行都会糊在 TUI 上）。所以真正让用户看到的是记下来的
            // 这个码，`Request::Profiles` 会把它当成一条警告顶上去。
            eprintln!("dct: 出错解释开着，但连不上（{e:?}），会话照常跑");
            mgr.set_backend(None);
            mgr.set_llm_problem(Some(e));
        }
    }
}

/// 只有 claude/codex 有自己的 OAuth 关系，别的 provider 只能走用户自己填的
/// key（`resolve::resolve` 里 key 优先于 OAuth 那条顺序保证了这一点）。跟
/// `cli.rs::oauth_lookup` 是同一条规则，见那边的注释——**不要**把 kimi/glm/
/// deepseek/qwen-api 也映射到这两个查询上，那等于把用户的 Anthropic/OpenAI
/// 登录态发给几家跟它们毫无关系的第三方服务器。
///
/// 每份凭据都带着**出处**（`BorrowedFrom`）一起返回：按名字挑只是第一道关，
/// 名字是用户可以手写的，真正管用的是 `resolve::select_credential` 拿这个
/// 出处去比对目的地主机。
///
/// 单独写一份而不是复用 `cli.rs::oauth_lookup`：那边把两个查询做成了可注入
/// 的闭包参数，是为了单测能绕开真实 Keychain / `auth.json`；守护进程启动
/// 只跑一次真实查询，没有这个诉求，硬套那个签名反而要在这里现造两个闭包。
fn startup_oauth(name: &str) -> Option<crate::llm::creds::Borrowed> {
    use crate::llm::creds::{BorrowedFrom, Credential};
    match name {
        "claude" => crate::llm::creds::read_claude_oauth()
            .map(|t| (BorrowedFrom::ClaudeCli, Credential::Bearer(t))),
        "codex" => crate::llm::creds::read_codex_auth().map(|c| (BorrowedFrom::CodexCli, c)),
        _ => None,
    }
}

fn serve(
    stream: UnixStream,
    mgr: Arc<SessionManager>,
    store: Arc<Mutex<Store>>,
    secrets: Arc<Mutex<SecretStore>>,
    profiles_dir: PathBuf,
    phone: Arc<Mutex<PhoneStatus>>,
    bridge: Arc<Mutex<Option<crate::bridge::BridgeHandle>>>,
) -> Result<()> {
    let mut out = stream.try_clone()?;
    let reader = BufReader::new(stream);
    for line in reader.lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        let resp = match serde_json::from_str::<Request>(&line) {
            Ok(req) => handle(req, &mgr, &store, &secrets, &profiles_dir, &phone, &bridge),
            Err(e) => Response::Error(ErrorCode::BadRequest(e.to_string())),
        };
        writeln!(out, "{}", serde_json::to_string(&resp)?)?;
        out.flush()?;
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn handle(
    req: Request,
    mgr: &Arc<SessionManager>,
    store: &Arc<Mutex<Store>>,
    secrets: &Arc<Mutex<SecretStore>>,
    profiles_dir: &Path,
    phone: &Arc<Mutex<PhoneStatus>>,
    bridge: &Arc<Mutex<Option<crate::bridge::BridgeHandle>>>,
) -> Response {
    let r: anyhow::Result<Response> = match req {
        // 不碰任何状态，也不该失败：界面拿它判断「我该不该跟你说话」。
        Request::Hello => Ok(Response::Hello {
            protocol: crate::proto::PROTOCOL_VERSION,
        }),
        Request::List => Ok(Response::Sessions(mgr.list())),
        Request::Profiles { lang } => {
            let (all, mut warnings) = all_profiles(profiles_dir);
            let sec = recover(secrets.lock());
            if let Some(e) = sec.load_error() {
                // 密钥文件读不了要顶到界面上。静默的话用户会以为密钥丢了，
                // 而且这时候所有写入都被拒，他改什么都没反应。
                //
                // IMPORTANT 4（最终整分支 code review）：以前这里无条件拼一句
                // 「检查一下 {path}」——对权限错误说得通（去看看那个文件），
                // 但套在密钥文件损坏的情形上就是让用户去手改一个 README 明说
                // 不支持手改的文件。`load_error()` 现在返回的已经是一句自足、
                // 说清楚该干什么的中文（见 `SecretStore::load` 的注释），这里
                // 只负责把路径带上，不再叠加任何暗示"去编辑它"的措辞。
                warnings.insert(0, e.clone());
            }
            // 用户开了出错解释却接不上，这条原因只有这一条路能走到他眼前
            // （守护进程的 stderr 被丢弃了）。排在密钥/profile 那些警告后面：
            // 那几条是「你现在要用的东西坏了」，这条是「一个增强功能没生效」。
            if let Some(p) = mgr.llm_problem() {
                warnings.push(crate::proto::WarningCode::LlmUnavailable(p));
            }
            let entries = all
                .iter()
                .map(|p| {
                    // 只查一次，分别喂给 status_of（装没装排在密钥前面，见
                    // profile.rs 的注释）和 has_secret（密钥页要的是这个事实
                    // 本身，不能从 status 反推——见 ProfileEntry::has_secret
                    // 的注释）。两处用同一次查询结果，不会因为中间密钥文件
                    // 被并发改过而看到两个不一致的答案。
                    let has_secret = sec.get(&p.name).is_some();
                    ProfileEntry {
                        name: p.name.clone(),
                        label: p.display_label(lang),
                        note: p.display_note(lang),
                        status: status_of(p, &all, has_secret, &command_exists, lang),
                        secret: p.secret.as_ref().map(|s| SecretPrompt {
                            hint: s.hint.get(lang).unwrap_or("").to_string(),
                            url: s.url.clone(),
                        }),
                        install: p.install.as_ref().map(|i| InstallPrompt {
                            command: i.command.clone(),
                            note: i.note.get(lang).unwrap_or("").to_string(),
                        }),
                        has_secret,
                    }
                })
                .collect();
            Ok(Response::Profiles { entries, warnings })
        }
        Request::Projects => {
            let st = recover(store.lock());
            Ok(Response::Projects {
                recent: st.list(),
                pinned: st.pinned(),
            })
        }
        Request::Create {
            dir,
            profile,
            remember,
        } => {
            let dir = PathBuf::from(dir);
            // 只借一眼这一个 profile 对应的密钥，锁拿完立刻放：`create()` 接下来要
            // 起 PTY 子进程，agent profile 还要跑一次 git checkpoint，这些都是慢
            // 操作（同样的原则见 session.rs::create 顶上的注释和它引用的
            // 「以下全是慢操作」那段）。锁如果跟着这段慢操作一起持有，Task 8 加的
            // SetSecret/DeleteSecret 就会被一个正在建的慢会话挡在门外——
            // 而这两个操作本身其实只需要极短时间。
            let secret = recover(secrets.lock()).get(&profile).map(str::to_string);
            // resolve_profile 要认得磁盘上的自定义 profile，不止内置那九个——
            // 否则「UI 上看着能用」和「create() 说没这个 profile」会对不上。
            let (all, _) = all_profiles(profiles_dir);
            let r = mgr
                .create(&dir, &profile, secret.as_deref(), &all)
                .map(|id| Response::Created { id });
            // 只有建成功了才记账。建失败的目录进了「最近项目」，
            // 下次还会被选中、还会失败。这把 store 锁跟上面的 secrets 锁完全无关，
            // 特意没有嵌套在一起拿，理由同上：不能让一把锁的持有时间绑架另一把。
            if r.is_ok() {
                let mut st = recover(store.lock());
                st.touch(&dir);
                // remember=false 是「帮你装 CLI」那条路径：它开的 shell 会话
                // 不是用户选的 agent，记了下次按 n 会掉进命令行。
                if remember {
                    st.set_last_profile_for(std::path::Path::new(&dir), &profile);
                }
            }
            r
        }
        Request::Input { id, text } => mgr.send_input(id, &text).map(|_| Response::Ok),
        Request::Screen { id } => mgr.screen(id).map(|snap| Response::Screen {
            lines: snap.lines,
            cursor: snap.cursor,
            state: snap.state,
            scroll: snap.scroll,
        }),
        Request::Screens { ids } => Ok(Response::Screens {
            screens: mgr.screens(&ids),
        }),
        Request::Resize { id, rows, cols } => mgr.resize(id, rows, cols).map(|_| Response::Ok),
        Request::Scroll { id, by } => mgr.scroll(id, by).map(Response::Scrolled),
        Request::Mouse { id, event } => mgr.forward_mouse(id, event).map(|_| Response::Ok),
        Request::Stop { id } => mgr.stop(id).map(|_| Response::Ok),
        Request::Kill { id } => mgr.kill(id).map(|_| Response::Ok),
        Request::Prune => Ok(Response::Pruned(mgr.prune())),
        Request::Undo { id } => mgr.undo(id).map(|_| Response::Ok),
        Request::Diff { id } => mgr.diff(id).map(Response::Diff),
        Request::SetSecret { profile, value } => recover(secrets.lock())
            .set(&profile, &value)
            .map(|_| Response::Ok),
        Request::DeleteSecret { profile } => recover(secrets.lock())
            .remove(&profile)
            .map(|_| Response::Ok),
        Request::LastProfile { dir } => Ok(Response::LastProfile(
            recover(store.lock()).last_profile_for(std::path::Path::new(&dir)),
        )),
        Request::PinProject { dir } => {
            recover(store.lock()).pin(std::path::Path::new(&dir));
            Ok(Response::Ok)
        }
        Request::UnpinProject { dir } => {
            recover(store.lock()).unpin(std::path::Path::new(&dir));
            Ok(Response::Ok)
        }
        // 永远不失败：没有解释（没配后端、还没算完、算失败了）跟「问不到」
        // 是同一件事，界面不用区分，统一显示今天就有的那句失败提示。
        Request::Explanation { id } => Ok(Response::Explanation(mgr.explanation(id))),
        Request::VerifySecret { profile, value } => {
            let (all, _) = all_profiles(profiles_dir);
            let spec = all
                .iter()
                .find(|p| p.name == profile)
                .and_then(|p| p.secret.as_ref())
                .and_then(|s| s.verify.as_ref());
            match spec {
                // 没声明 verify 的 profile 直接放行，不是错误
                None => Ok(Response::Verify(VerifyOutcome::Ok)),
                Some(v) => Ok(Response::Verify(verify_with(&v.url, &value, &send_probe))),
            }
        }
        Request::PhoneStatus => Ok(Response::Phone(recover(phone.lock()).clone())),
        // 打真网络（Telegram 的 `getMe`）——界面必须把这条请求丢给后台线程，
        // 同 `VerifySecret` 的道理，见该请求的文档注释。
        Request::PhoneSetToken { token } => {
            let status = match Telegram::new(&token).get_me() {
                Ok(bot) => {
                    // 先验证、验证通过才落盘：跟 `VerifySecret`→`SetSecret`
                    // 那条路径反过来——这里是一条请求做完两件事，但顺序
                    // 上的道理相同，一个不通过的令牌不该写进密钥仓。
                    if let Err(e) = recover(secrets.lock()).set(PHONE_TOKEN_KEY, &token) {
                        return Response::Error(to_code(e));
                    }
                    // 这是一个新令牌——不管上一个令牌配没配过对，这个新的
                    // bot 谁都还没认过，必须清掉旧的持久化主人，否则重启
                    // 之后 `startup_bridge_owner` 会把上一个 bot 的 chat id
                    // 错当成这个新 bot 的主人。
                    let _ = recover(secrets.lock()).remove(PHONE_OWNER_KEY);
                    // 令牌验证过、也落盘了——现在真的开始听：不起这个线程，
                    // 用户填完令牌之后就是对着「等配对」发呆，永远等不到
                    // bridge 去认那第一条消息。`owner` 传 `None`：这次填的
                    // 是一个全新的令牌，配对必须重新打开，从这次填完之后
                    // 第一个发消息的人开始认。
                    //
                    // 用 `replace()` 而不是 `spawn()`——如果之前已经有一个
                    // bridge 在跑（比如重新填了一遍令牌），必须先把旧的
                    // 停掉，不然两条轮询线程同时认领同一个 bot 的消息，
                    // 各自配对到不同的人身上（C3）。
                    let ch: Arc<dyn crate::channel::Channel> = Arc::new(Telegram::new(&token));
                    crate::bridge::replace(
                        bridge,
                        ch,
                        phone.clone(),
                        None,
                        persist_owner_closure(secrets.clone()),
                    );
                    PhoneStatus {
                        state: crate::proto::PhoneState::WaitingForPairing,
                        bot: Some(bot),
                        owner: None,
                    }
                }
                Err(e) => PhoneStatus {
                    state: crate::proto::PhoneState::Broken(phone_set_token_failure_message(e)),
                    bot: None,
                    owner: None,
                },
            };
            *recover(phone.lock()) = status.clone();
            Ok(Response::Phone(status))
        }
        // 换一台手机：令牌还在，只是把「谁是主人」忘掉，退回等配对。
        // **C2 的修复**：以前这里只改了 `phone` 这个给界面看的缓存，
        // 活着的 `Bridge::owner` 毫不知情，旧主人事实上继续掌握着通道，
        // 新手机永远配不上。现在必须同时：(1) 清掉密钥仓里持久化的主人，
        // 不然下次重启又把旧主人读回来；(2) 直接摸到那个还在跑的 bridge，
        // 让它当场忘掉旧主人，不用重启线程。
        Request::PhoneUnpair => {
            if let Err(e) = recover(secrets.lock()).remove(PHONE_OWNER_KEY) {
                return Response::Error(to_code(e));
            }
            if let Some(h) = recover(bridge.lock()).as_ref() {
                h.unpair();
            }
            let mut st = recover(phone.lock());
            st.owner = None;
            st.state = crate::proto::PhoneState::WaitingForPairing;
            Ok(Response::Phone(st.clone()))
        }
        // 整个关掉：删令牌、状态槽退回 Off。删不掉密钥（文件坏了）不该假装
        // 关成功了——那样用户以为关了，令牌其实还在磁盘上。
        // **C2 的修复**：以前这里完全没碰轮询线程——令牌从磁盘上删掉了，
        // 但内存里那个 `Bridge` 攥着自己的 `Telegram` 克隆继续长轮询，
        // 用户以为关掉了，找到这个用户名的人其实还能继续敲字。现在必须
        // 真的把线程停掉。
        Request::PhoneDisable => {
            if let Err(e) = recover(secrets.lock()).remove(PHONE_TOKEN_KEY) {
                return Response::Error(to_code(e));
            }
            let _ = recover(secrets.lock()).remove(PHONE_OWNER_KEY);
            crate::bridge::stop_current(bridge);
            let status = PhoneStatus {
                state: crate::proto::PhoneState::Off,
                bot: None,
                owner: None,
            };
            *recover(phone.lock()) = status.clone();
            Ok(Response::Phone(status))
        }
    };
    r.unwrap_or_else(|e| Response::Error(to_code(e)))
}

/// `PhoneSetToken` 打 `getMe` 没成功时，给用户看的那句人话。**这里就是
/// 那句「已经成文的人话」被写出来的地方**——`PhoneState::Broken` 的文档
/// 注释说的就是这个函数：拼这句话的人绝不能把 `token`/`ChannelError` 的
/// 原始内容带进去。`ui::phone::status_line`/`next_step` 出于防御性根本
/// 不读这个字符串（见那两个函数的注释），所以这里的措辞今天还传不到
/// 屏幕上，但契约先立在这——哪天那两个函数改成读它，这条契约不能补。
fn phone_set_token_failure_message(e: ChannelError) -> String {
    match e {
        ChannelError::BadToken => {
            "这个令牌用不了，去 BotFather 那边确认一下，重新粘贴一遍".to_string()
        }
        ChannelError::Unreachable => "连不上 Telegram，检查一下网络，然后重试".to_string(),
        ChannelError::Malformed => "Telegram 的回应读不懂，稍后再试一次".to_string(),
    }
}

/// 把内部错误还原成给界面的码。`downcast` 拿不到码的，说明这条路径还没归类——
/// 照抄原文走 `Internal`，界面原样显示。这样迁移可以一条一条来，不必等到
/// 每一条都归好类才敢合并。
fn to_code(e: anyhow::Error) -> ErrorCode {
    match e.downcast::<crate::proto::CodedError>() {
        Ok(c) => c.0,
        Err(e) => ErrorCode::Internal(e.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;

    /// 大多数 `handle()` 测试根本不关心手机通知，就是要传一个空壳槽。
    fn test_phone() -> Arc<Mutex<PhoneStatus>> {
        Arc::new(Mutex::new(PhoneStatus {
            state: PhoneState::Off,
            bot: None,
            owner: None,
        }))
    }

    /// 同上——大多数测试不关心手机通知，`bridge` 槽给个空的就行。
    fn test_bridge() -> Arc<Mutex<Option<crate::bridge::BridgeHandle>>> {
        Arc::new(Mutex::new(None))
    }

    /// 一个不碰网络、光靠自己字段回答问题的假渠道——测试"重启到底认不
    /// 认得持久化的 owner"这条链路用，永远不该真的被轮询/发送触发到
    /// 网络（`start_phone_bridge` 只把它交给 `Bridge`，`Bridge::new` 里
    /// 不会立刻调用它，真正调用发生在后台线程，跟这些测试的同步断言
    /// 无关）。
    struct StubChannel;
    impl crate::channel::Channel for StubChannel {
        fn send(&self, _to: i64, _text: &str) -> Result<crate::channel::MsgId, ChannelError> {
            panic!("这条测试链路不该真的发消息")
        }
        fn poll(
            &self,
            _timeout: Duration,
        ) -> std::result::Result<Vec<crate::channel::Incoming>, ChannelError> {
            panic!("这条测试链路不该真的打网络轮询")
        }
        fn get_me(&self) -> std::result::Result<String, ChannelError> {
            panic!("这条测试链路不该真的打网络验证令牌")
        }
        fn drain(&self, _timeout: Duration) -> std::result::Result<usize, ChannelError> {
            panic!("这条测试链路不该真的打网络清空积压")
        }
    }

    /// 造一个文件足够多的仓库，让 agent 会话建立时的首次 git checkpoint 慢到能
    /// 测出来。手法照抄 `tests/concurrency.rs` 的 `init_big_repo`——那边已经验证过
    /// 8000 个文件在这台机器的规模下够慢、够稳。
    fn init_big_repo(path: &Path, n: usize) {
        let run = |args: &[&str]| {
            std::process::Command::new("git")
                .args(args)
                .current_dir(path)
                .output()
                .unwrap();
        };
        run(&["init", "-q"]);
        run(&["config", "user.email", "t@example.com"]);
        run(&["config", "user.name", "t"]);
        std::fs::create_dir_all(path.join("files")).unwrap();
        for i in 0..n {
            std::fs::write(
                path.join("files").join(format!("f{i}.txt")),
                format!("{i}\n"),
            )
            .unwrap();
        }
        run(&["add", "-A"]);
        run(&["commit", "-q", "-m", "init"]);
    }

    // 用 cat 冒充 agent：能收输入、不会自己退出，is_agent = true 才会触发
    // create() 里的 git checkpoint。
    fn fake_agent() -> Profile {
        Profile {
            name: "daemon-lock-fake".into(),
            command: vec!["cat".into()],
            is_agent: true,
            idle_pattern: None,
            busy_pattern: None,
            error_pattern: None,
            env: Default::default(),
            secret: None,
            install: None,
            headless: None,
            api: None,
            label: Default::default(),
            note: Default::default(),
        }
    }

    /// 回归测试，对应审查发现「原始 OS 报错会红字出现在选择器标题上」：
    /// `Request::Profiles` 拼 warning 时不能只把 `SecretStore::load_error()`
    /// 的文案接上就完事——那句话本身已经是人话了（见 secrets.rs），但这里
    /// 还要点名是哪个文件，且组装出来的整句不能再夹带任何英文系统原话。
    #[test]
    fn profiles_warning_names_the_broken_secrets_file_in_chinese() {
        let secrets_dir = tempfile::tempdir().unwrap();
        let secrets_path = secrets_dir.path().join("secrets.toml");
        std::fs::write(&secrets_path, "这不是 TOML {{{").unwrap();

        let mgr = Arc::new(SessionManager::new());
        let secrets = Arc::new(Mutex::new(SecretStore::load(&secrets_path)));
        let store_dir = tempfile::tempdir().unwrap();
        let store = Arc::new(Mutex::new(Store::load(
            &store_dir.path().join("projects.json"),
        )));
        let profiles_dir = tempfile::tempdir().unwrap();

        let resp = handle(
            Request::Profiles {
                lang: crate::i18n::Lang::Zh,
            },
            &mgr,
            &store,
            &secrets,
            profiles_dir.path(),
            &test_phone(),
            &test_bridge(),
        );

        match resp {
            Response::Profiles { warnings, .. } => {
                // 守护进程报的是**码**。它点名了是哪个文件，但一个字的
                // 文案都不组——句子由界面用 `i18n::msg::warning` 组出来。
                let w = warnings
                    .iter()
                    .find(|w| matches!(w, crate::proto::WarningCode::SecretsCorrupt { .. }))
                    .expect("密钥文件读坏了必须有 warning");
                let crate::proto::WarningCode::SecretsCorrupt { path } = w else {
                    unreachable!()
                };
                assert_eq!(
                    path,
                    &secrets_path.display().to_string(),
                    "要点名是哪个文件"
                );

                // 组出来的那句话仍然要满足原来的两条约束：一行、不带
                // toml 库自带的图形化 Display。
                let line = crate::i18n::msg::warning(crate::i18n::Lang::Zh, w);
                assert!(!line.contains('\n'), "不能是多行栈追踪：{line}");
                assert!(
                    !line.contains("TOML parse error"),
                    "toml 库自带的图形化 Display 不能漏出来：{line}"
                );
            }
            other => panic!("期待 Response::Profiles，得到 {other:?}"),
        }
    }

    /// 守护进程不该替用户决定语言。它是常驻的、可能同时服务多个界面的进程，
    /// 「谁的语言是什么」不是它的状态——界面在请求里带上，它照着取就行。
    /// 以前这里硬编码 `Lang::Zh`，于是 profile 的 `en` 文案写了也永远没人读。
    #[test]
    fn profiles_are_labelled_in_the_language_the_client_asked_for() {
        let mgr = Arc::new(SessionManager::new());
        let secrets_dir = tempfile::tempdir().unwrap();
        let secrets = Arc::new(Mutex::new(SecretStore::load(
            &secrets_dir.path().join("secrets.toml"),
        )));
        let store_dir = tempfile::tempdir().unwrap();
        let store = Arc::new(Mutex::new(Store::load(
            &store_dir.path().join("projects.json"),
        )));
        let profiles_dir = tempfile::tempdir().unwrap();

        let labels = |lang| match handle(
            Request::Profiles { lang },
            &mgr,
            &store,
            &secrets,
            profiles_dir.path(),
            &test_phone(),
            &test_bridge(),
        ) {
            Response::Profiles { entries, .. } => entries
                .into_iter()
                .map(|e| format!("{}|{}", e.label, e.note))
                .collect::<Vec<_>>()
                .join("  "),
            other => panic!("期待 Response::Profiles，得到 {other:?}"),
        };

        let zh = labels(crate::i18n::Lang::Zh);
        let en = labels(crate::i18n::Lang::En);
        assert!(
            zh.contains("命令行"),
            "中文下 shell 的名字是「命令行」：{zh}"
        );
        assert_ne!(zh, en, "换了语言，菜单文案必须真的跟着变");
    }

    /// 回归测试，对应审查发现「密钥仓的锁被握过了整个 create()」：以前 `handle()`
    /// 会在建会话的整段慢流程（PTY 起进程、agent 场景下的 git checkpoint）期间
    /// 一直攥着 secrets 锁。Task 8 加了 SetSecret/DeleteSecret，这两个本该极快的
    /// 操作绝不能被一个正在建的慢会话堵住排队。
    ///
    /// 直接调用 `handle()` 而不是走真实 socket，是为了最直接地量最本质的东西：
    /// 慢 `Create` 跑在一个线程时，另一个线程单纯去锁 `secrets` 这把 `Mutex`
    /// 本身，应该几乎立即拿到，不必等 `Create` 收工。
    #[test]
    fn create_does_not_hold_the_secrets_lock_across_the_slow_work() {
        let repo = tempfile::tempdir().unwrap();
        init_big_repo(repo.path(), 8000);

        let mgr = Arc::new(SessionManager::new());
        mgr.register_profile(fake_agent());

        let secrets_dir = tempfile::tempdir().unwrap();
        let secrets = Arc::new(Mutex::new(SecretStore::load(
            &secrets_dir.path().join("secrets.toml"),
        )));
        let store_dir = tempfile::tempdir().unwrap();
        let store = Arc::new(Mutex::new(Store::load(
            &store_dir.path().join("projects.json"),
        )));
        // 空目录：这条测试不关心磁盘 profile，`daemon-lock-fake` 只在
        // `mgr` 的 `extra_profiles` 里注册过。
        let profiles_dir = tempfile::tempdir().unwrap();

        let mgr2 = mgr.clone();
        let store2 = store.clone();
        let secrets2 = secrets.clone();
        let repo_path = repo.path().display().to_string();
        let profiles_dir_path = profiles_dir.path().to_path_buf();
        let create_handle = std::thread::spawn(move || {
            let t = Instant::now();
            let resp = handle(
                Request::Create {
                    dir: repo_path,
                    profile: "daemon-lock-fake".into(),
                    remember: true,
                },
                &mgr2,
                &store2,
                &secrets2,
                &profiles_dir_path,
                &test_phone(),
                &test_bridge(),
            );
            (t.elapsed(), resp)
        });

        // 给慢 Create 一点时间真正进到 git checkpoint 里
        std::thread::sleep(Duration::from_millis(150));

        let t = Instant::now();
        drop(recover(secrets.lock()));
        let lock_wait = t.elapsed();

        let (create_elapsed, create_resp) = create_handle.join().unwrap();

        assert!(
            matches!(create_resp, Response::Created { .. }),
            "Create 应该最终成功，实际 {create_resp:?}"
        );
        assert!(
            create_elapsed > Duration::from_millis(300),
            "场景失真：Create 耗时应显著大于 300ms 才能验证不阻塞，实际 {create_elapsed:?}"
        );
        assert!(
            lock_wait < Duration::from_millis(100),
            "secrets 锁被慢 Create 攥着不放：等了 {lock_wait:?}（同期 Create 耗时 {create_elapsed:?}）"
        );
    }

    // 冒充一个会报错的 agent：echo BOOM 之后常驻，好让 tick() 判成 Failed
    // 而不是 Stopped（同 session.rs::tests::failing_agent）。
    fn failing_agent() -> Profile {
        Profile {
            name: "daemon-explain-fake".into(),
            command: vec!["/bin/sh".into(), "-c".into(), "echo BOOM; sleep 5".into()],
            is_agent: true,
            idle_pattern: None,
            busy_pattern: None,
            error_pattern: Some("BOOM".into()),
            env: Default::default(),
            secret: None,
            install: None,
            headless: None,
            api: None,
            label: Default::default(),
            note: Default::default(),
        }
    }

    struct FixedAnswer;
    impl crate::llm::Backend for FixedAnswer {
        fn complete(&self, _p: &crate::llm::Prompt) -> Result<String, crate::llm::LlmError> {
            Ok("这个命令没配好，重开一次就行。".into())
        }
    }

    /// 回归测试：`Request::Explanation` 真的接到了 `mgr.explanation()`，不是
    /// 一条只在类型上存在、`handle()` 里没人接的死请求。
    #[test]
    fn explanation_request_is_wired_to_the_session_manager() {
        let repo = tempfile::tempdir().unwrap();
        {
            let run = |args: &[&str]| {
                std::process::Command::new("git")
                    .args(args)
                    .current_dir(repo.path())
                    .output()
                    .unwrap();
            };
            run(&["init", "-q"]);
            run(&["config", "user.email", "t@example.com"]);
            run(&["config", "user.name", "t"]);
            std::fs::write(repo.path().join("a.txt"), "hi\n").unwrap();
            run(&["add", "-A"]);
            run(&["commit", "-q", "-m", "init"]);
        }

        let mgr = Arc::new(SessionManager::new());
        mgr.register_profile(failing_agent());
        mgr.set_backend(Some(Arc::new(FixedAnswer)));
        let secrets = Arc::new(Mutex::new(SecretStore::load(
            &tempfile::tempdir().unwrap().path().join("secrets.toml"),
        )));
        let store = Arc::new(Mutex::new(Store::load(
            &tempfile::tempdir().unwrap().path().join("projects.json"),
        )));
        let profiles_dir = tempfile::tempdir().unwrap();

        let id = mgr
            .create(repo.path(), "daemon-explain-fake", None, &[])
            .unwrap();

        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            mgr.tick();
            let resp = handle(
                Request::Explanation { id },
                &mgr,
                &store,
                &secrets,
                profiles_dir.path(),
                &test_phone(),
                &test_bridge(),
            );
            if let Response::Explanation(Some(text)) = resp {
                assert_eq!(text, "这个命令没配好，重开一次就行。");
                return;
            }
            assert!(Instant::now() < deadline, "Explanation 请求一直没接到解释");
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    /// 没有对应会话时，`Explanation` 也要老老实实回 `None`，不能 panic
    /// 或者报错——「问不到」跟「没配后端」在界面眼里是同一件事。
    #[test]
    fn explanation_for_an_unknown_session_is_none_not_an_error() {
        let mgr = Arc::new(SessionManager::new());
        let secrets = Arc::new(Mutex::new(SecretStore::load(
            &tempfile::tempdir().unwrap().path().join("secrets.toml"),
        )));
        let store = Arc::new(Mutex::new(Store::load(
            &tempfile::tempdir().unwrap().path().join("projects.json"),
        )));
        let profiles_dir = tempfile::tempdir().unwrap();

        let resp = handle(
            Request::Explanation { id: 9999 },
            &mgr,
            &store,
            &secrets,
            profiles_dir.path(),
            &test_phone(),
            &test_bridge(),
        );
        assert!(matches!(resp, Response::Explanation(None)));
    }

    /// **Critical 回归测试.** 没写 `[llm]`（这里就是没有 `config.toml` 这个
    /// 文件——「不存在」和「写了但没这一段」在 `config.rs` 里是同一件事）
    /// 是绝大多数用户的正常状态，出错解释必须整个关着：`install_llm_backend`
    /// 压根不能调 `resolve()`，更不能装上一个会把终端内容发出去的后端。
    ///
    /// 断言的是 `backend_is_set()` 这个布尔值，不是「问一次真实网络/CLI
    /// 会不会成功」——默认 provider 是 `claude` + `Transport::Cli`，如果
    /// Critical 那个 bug 还在，这条路径的 `resolve()` 本来就会成功（`Cli`
    /// 传输不需要凭据），间接测法（等一个 explanation 出现）反而会被「这台
    /// 机器上到底装没装 claude CLI」这种环境噪音污染，钉不住真正的问题。
    #[test]
    fn no_llm_section_means_no_backend_is_installed() {
        let dir = tempfile::tempdir().unwrap();
        let socket = dir.path().join("daemon.sock"); // 旁边故意不写 config.toml
        let mgr = SessionManager::new();

        install_llm_backend(&socket, &dir.path().join("profiles"), &mgr);

        assert!(
            !mgr.backend_is_set(),
            "没写 [llm] 就不该装后端——这是隐私边界，不是默认值的事"
        );
    }

    /// 反过来钉住「写了就真的开」：不能为了堵上面那条回归，把功能焊死关掉。
    /// `[llm]` 段里什么字段都不填，靠的是 `LlmConfig` 自己的默认值
    /// （provider claude、transport Cli），这条路径不需要任何真实凭据就该
    /// 成功——`Transport::Cli` 只是记下命令，不在这一步真的起子进程。
    #[test]
    fn a_bare_llm_section_does_install_a_backend() {
        let dir = tempfile::tempdir().unwrap();
        let socket = dir.path().join("daemon.sock");
        std::fs::write(crate::config::config_path_for_socket(&socket), "[llm]\n").unwrap();
        let mgr = SessionManager::new();

        install_llm_backend(&socket, &dir.path().join("profiles"), &mgr);

        assert!(
            mgr.backend_is_set(),
            "写了 [llm]（哪怕是空的）就该是一次显式的开——这条不能被上一条回归测试误伤"
        );
        assert!(mgr.llm_problem().is_none(), "接上了就不该留着一条抱怨");
    }

    /// 用户开了出错解释、却配错了，**这件事必须走得到他眼前**。
    ///
    /// 守护进程那句 `eprintln!` 是看不见的：界面进程拉起它的时候把 stderr
    /// 接到了 `/dev/null`（`client::spawn_daemon`，不然每一行都会糊在 TUI
    /// 上）。所以原因要记在守护进程上，并且跟着 `Request::Profiles` 一起
    /// 顶到界面的警告栏——这条测试钉的就是这条通路，从「配错了」一直到
    /// 「界面拿到一条能读的警告」。
    #[test]
    fn a_broken_llm_setting_reaches_the_user_instead_of_going_silent() {
        let dir = tempfile::tempdir().unwrap();
        let socket = dir.path().join("daemon.sock");
        std::fs::write(
            crate::config::config_path_for_socket(&socket),
            "[llm]\nprovider = \"根本没有这个\"\n",
        )
        .unwrap();
        let mgr = Arc::new(SessionManager::new());
        let profiles_dir = dir.path().join("profiles");

        install_llm_backend(&socket, &profiles_dir, &mgr);

        assert!(!mgr.backend_is_set(), "连不上就不该装后端");
        let secrets = Arc::new(Mutex::new(SecretStore::load(
            &dir.path().join("secrets.toml"),
        )));
        let store = Arc::new(Mutex::new(Store::load(&dir.path().join("projects.json"))));
        let resp = handle(
            Request::Profiles {
                lang: crate::i18n::Lang::Zh,
            },
            &mgr,
            &store,
            &secrets,
            &profiles_dir,
            &test_phone(),
            &test_bridge(),
        );
        let Response::Profiles { warnings, .. } = resp else {
            panic!("期待 Response::Profiles");
        };
        let w = warnings
            .iter()
            .find(|w| matches!(w, crate::proto::WarningCode::LlmUnavailable(_)))
            .expect("配错了却一个警告都没有——用户会以为功能坏了却查不到任何线索");
        let line = crate::i18n::msg::warning(crate::i18n::Lang::Zh, w);
        assert!(
            line.contains("根本没有这个"),
            "要点名是设置里的哪个名字写错了：{line}"
        );
    }

    /// `bare_handle_deps` 的返回值：调用 `handle()` 要用的三个壳
    /// （mgr/store/secrets）以及一个空 profiles 目录。
    type BareHandleDeps = (
        Arc<SessionManager>,
        Arc<Mutex<Store>>,
        Arc<Mutex<SecretStore>>,
        tempfile::TempDir,
    );

    /// 拼齐调用 `handle()` 要用的三个壳（mgr/store/secrets）以及一个空
    /// profiles 目录，手机通知的这几条测试都不关心这几个，只是签名要它们。
    fn bare_handle_deps() -> BareHandleDeps {
        let mgr = Arc::new(SessionManager::new());
        let store = Arc::new(Mutex::new(Store::load(
            &tempfile::tempdir().unwrap().path().join("projects.json"),
        )));
        let secrets = Arc::new(Mutex::new(SecretStore::load(
            &tempfile::tempdir().unwrap().path().join("secrets.toml"),
        )));
        let profiles_dir = tempfile::tempdir().unwrap();
        (mgr, store, secrets, profiles_dir)
    }

    /// Ruling 3：`Request::PhoneStatus` 读的是共享状态槽，不是它自己现算的
    /// 东西——外面把槽里的值改了，请求立刻看到新值。
    #[test]
    fn phone_status_reads_the_shared_slot() {
        let (mgr, store, secrets, profiles_dir) = bare_handle_deps();
        let phone = Arc::new(Mutex::new(PhoneStatus {
            state: PhoneState::Paired,
            bot: Some("my_dct_bot".into()),
            owner: Some("lei".into()),
        }));

        let resp = handle(
            Request::PhoneStatus,
            &mgr,
            &store,
            &secrets,
            profiles_dir.path(),
            &phone,
            &test_bridge(),
        );

        match resp {
            Response::Phone(status) => {
                assert_eq!(status.state, PhoneState::Paired);
                assert_eq!(status.owner.as_deref(), Some("lei"));
            }
            other => panic!("期待 Response::Phone，得到 {other:?}"),
        }
    }

    /// `PhoneUnpair` 忘记主人、退回等配对，但**不**碰令牌——密钥仓里的
    /// `PHONE_TOKEN_KEY` 必须原样留着，用户只是想换一台手机，不是想关掉。
    ///
    /// **C2 的回归测试。** 以前这条测试只看 `phone` 那个给界面看的缓存，
    /// 从没验证过真正活着的 `Bridge` 有没有被改到——那正是审查发现的
    /// 漏洞：`phone` 显示"等配对"，但 `Bridge::owner` 里旧主人纹丝不动，
    /// 新手机永远配不上。这里真起一个 bridge（`owner` 先设成旧主人），
    /// 调用 `Request::PhoneUnpair` 之后直接问这个 bridge：陌生人现在
    /// 该能重新配对成功。
    #[test]
    fn phone_unpair_forgets_the_owner_but_keeps_the_token() {
        let (mgr, store, secrets, profiles_dir) = bare_handle_deps();
        recover(secrets.lock())
            .set(PHONE_TOKEN_KEY, "123456:AAH-tok")
            .unwrap();
        recover(secrets.lock()).set(PHONE_OWNER_KEY, "111").unwrap();
        let phone = Arc::new(Mutex::new(PhoneStatus {
            state: PhoneState::Paired,
            bot: Some("my_dct_bot".into()),
            owner: Some("lei".into()),
        }));
        let bridge_handle = crate::bridge::spawn(
            Arc::new(StubChannel),
            phone.clone(),
            Some(111),
            Box::new(|_| {}),
        );
        let bridge = Arc::new(Mutex::new(Some(bridge_handle)));

        let resp = handle(
            Request::PhoneUnpair,
            &mgr,
            &store,
            &secrets,
            profiles_dir.path(),
            &phone,
            &bridge,
        );

        match resp {
            Response::Phone(status) => {
                assert_eq!(status.state, PhoneState::WaitingForPairing);
                assert_eq!(status.owner, None, "换手机要忘掉旧主人");
            }
            other => panic!("期待 Response::Phone，得到 {other:?}"),
        }
        assert_eq!(
            recover(secrets.lock()).get(PHONE_TOKEN_KEY),
            Some("123456:AAH-tok"),
            "令牌不该被 Unpair 碰"
        );
        assert_eq!(
            recover(secrets.lock()).get(PHONE_OWNER_KEY),
            None,
            "持久化的主人必须被清掉，否则下次重启又把旧主人读回来"
        );
        let guard = recover(bridge.lock());
        let h = guard.as_ref().expect("bridge 不该被 Unpair 停掉");
        assert_eq!(
            h.accept(&crate::channel::Incoming {
                text: "新手机先发".into(),
                reply_to: None,
                chat_id: 222,
            }),
            crate::bridge::Accepted::Paired(222),
            "Unpair 必须真的改到活着的 Bridge，不能只改 phone 这个界面缓存"
        );
    }

    /// `PhoneDisable` 是真正的关掉：令牌从密钥仓里删掉，状态槽退回 `Off`。
    ///
    /// **C2 的回归测试。** 以前 disable 完全不碰轮询线程——令牌从磁盘上
    /// 删了，内存里的 `Bridge` 攥着自己那份 `Telegram` 克隆继续长轮询，
    /// 用户以为关掉了，找到这个用户名的人其实还能继续敲字。这里验证
    /// `PhoneDisable` 处理完之后，bridge 槽必须真的空了。
    #[test]
    fn phone_disable_deletes_the_token_and_resets_the_slot() {
        let (mgr, store, secrets, profiles_dir) = bare_handle_deps();
        recover(secrets.lock())
            .set(PHONE_TOKEN_KEY, "123456:AAH-tok")
            .unwrap();
        let phone = Arc::new(Mutex::new(PhoneStatus {
            state: PhoneState::Paired,
            bot: Some("my_dct_bot".into()),
            owner: Some("lei".into()),
        }));
        let bridge_handle = crate::bridge::spawn(
            Arc::new(StubChannel),
            phone.clone(),
            Some(111),
            Box::new(|_| {}),
        );
        let bridge = Arc::new(Mutex::new(Some(bridge_handle)));

        let resp = handle(
            Request::PhoneDisable,
            &mgr,
            &store,
            &secrets,
            profiles_dir.path(),
            &phone,
            &bridge,
        );

        match resp {
            Response::Phone(status) => assert_eq!(status.state, PhoneState::Off),
            other => panic!("期待 Response::Phone，得到 {other:?}"),
        }
        assert!(
            recover(bridge.lock()).is_none(),
            "关掉之后 bridge 槽必须真的空了，不能留着一个还在跑的轮询线程"
        );
        assert_eq!(
            recover(secrets.lock()).get(PHONE_TOKEN_KEY),
            None,
            "关掉之后令牌必须真的从磁盘上没了"
        );
    }

    /// `initial_phone_status`：密钥仓里有令牌就该是等配对，没有就是关着——
    /// 这是守护进程重启后，`PhoneStatus` 在 bridge 补上真实数据之前的诚实初值。
    #[test]
    fn initial_phone_status_follows_whether_a_token_is_stored() {
        let empty = SecretStore::load(&tempfile::tempdir().unwrap().path().join("secrets.toml"));
        assert_eq!(initial_phone_status(&empty).state, PhoneState::Off);

        let tmp = tempfile::tempdir().unwrap();
        let mut with_token = SecretStore::load(&tmp.path().join("secrets.toml"));
        with_token.set(PHONE_TOKEN_KEY, "tok").unwrap();
        assert_eq!(
            initial_phone_status(&with_token).state,
            PhoneState::WaitingForPairing
        );
    }

    /// `startup_bridge_owner`：密钥仓里有持久化的主人就读出来，没有（还
    /// 没配对过，或者是一个字都读不出来的垃圾值）就老实说 `None`——
    /// **绝不能因为解析失败就崩，也绝不能编一个假的主人出来**，两种情况
    /// 都只是"这次重启要重新走配对"，不是错误。
    #[test]
    fn startup_bridge_owner_distinguishes_absent_known_and_corrupt() {
        let empty = SecretStore::load(&tempfile::tempdir().unwrap().path().join("secrets.toml"));
        assert_eq!(startup_bridge_owner(&empty), StartupOwner::None);

        let tmp = tempfile::tempdir().unwrap();
        let mut with_owner = SecretStore::load(&tmp.path().join("secrets.toml"));
        with_owner.set(PHONE_OWNER_KEY, "555").unwrap();
        assert_eq!(startup_bridge_owner(&with_owner), StartupOwner::Known(555));

        // **F3 的钉子。** 键存在，但读不出一个合法的 chat id——这**不是**
        // "从没配过对"，不能退化成 `StartupOwner::None`。之前的实现用
        // `.and_then(...).ok()` 把这条路径悄悄合并进了 `None`，等于把
        // "配对信息坏了"错当成"可以随便谁来配对"，Ruling 9 明确禁止。
        let tmp2 = tempfile::tempdir().unwrap();
        let mut garbage = SecretStore::load(&tmp2.path().join("secrets.toml"));
        garbage.set(PHONE_OWNER_KEY, "这不是数字").unwrap();
        assert_eq!(
            startup_bridge_owner(&garbage),
            StartupOwner::Corrupt,
            "读不出来必须是一个明确的、配对不能开的状态，不能悄悄退化成 None"
        );
    }

    /// **F3 的端到端回归测试。** 密钥仓里有令牌，但持久化的 owner 是一条
    /// 读不出来的坏记录——`start_phone_bridge` 绝不能把这种情况当成
    /// "从没配过对"去打开配对：不该起任何 bridge（没有 bridge 就没有
    /// 任何人能被 `accept()` 判成主人，这是最强的保护），要把这件事说给
    /// 用户听，让他知道下一步该干什么。
    #[test]
    fn startup_refuses_to_open_pairing_when_the_persisted_owner_is_corrupt() {
        let secrets = Arc::new(Mutex::new(SecretStore::load(
            &tempfile::tempdir().unwrap().path().join("secrets.toml"),
        )));
        recover(secrets.lock())
            .set(PHONE_TOKEN_KEY, "123456:AAH-tok")
            .unwrap();
        recover(secrets.lock())
            .set(PHONE_OWNER_KEY, "这不是数字")
            .unwrap();
        let phone = test_phone();
        let bridge: Arc<Mutex<Option<crate::bridge::BridgeHandle>>> = Arc::new(Mutex::new(None));

        start_phone_bridge(&secrets, &phone, &bridge, &|_token| {
            Arc::new(StubChannel) as Arc<dyn crate::channel::Channel>
        });

        assert!(
            recover(bridge.lock()).is_none(),
            "配对信息读不出来时不该起任何 bridge——那等于把\"读不出来\"当成\"随便谁来都行\""
        );
        let st = recover(phone.lock());
        match &st.state {
            PhoneState::Broken(text) => {
                assert!(!text.is_empty());
                assert!(
                    text.chars().any(|c| ('\u{4e00}'..='\u{9fff}').contains(&c)),
                    "该是写给人看的一句话: {text}"
                );
            }
            other => panic!("该停在 Broken，得到 {other:?}"),
        }
    }

    /// **C1 的端到端回归测试，钉在守护进程启动这条真实路径上。**
    ///
    /// 独立安全评审指出：`daemon.rs` 重启时如果把 `owner` 镶死成
    /// `None`（哪怕 `bridge.rs` 自己的 `accept()`/`drain_backlog` 完全
    /// 正确），配对照样会在每次重启后重新打开——持久化的主人从没被读
    /// 回来过。这条测试把 `start_phone_bridge`（真实启动路径 `run_with_
    /// manager` 用的同一个函数）跟一个不碰网络的假渠道接起来，直接问
    /// 起出来的那个 bridge：陌生人先发消息，该不该被拒。
    ///
    /// 如果有人把 `start_phone_bridge` 里的 `startup_bridge_owner(&s)`
    /// 改回硬编码的 `None`，这条测试必须失败——见任务报告里跑这个
    /// 变异之后的实际结果。
    #[test]
    fn startup_uses_the_persisted_owner_and_never_reopens_pairing() {
        let secrets = Arc::new(Mutex::new(SecretStore::load(
            &tempfile::tempdir().unwrap().path().join("secrets.toml"),
        )));
        recover(secrets.lock())
            .set(PHONE_TOKEN_KEY, "123456:AAH-tok")
            .unwrap();
        recover(secrets.lock()).set(PHONE_OWNER_KEY, "555").unwrap();
        let phone = test_phone();
        let bridge: Arc<Mutex<Option<crate::bridge::BridgeHandle>>> = Arc::new(Mutex::new(None));

        start_phone_bridge(&secrets, &phone, &bridge, &|_token| {
            Arc::new(StubChannel) as Arc<dyn crate::channel::Channel>
        });

        let guard = recover(bridge.lock());
        let handle = guard
            .as_ref()
            .expect("密钥仓里有令牌，重启就该起一个 bridge");
        assert_eq!(
            handle.accept(&crate::channel::Incoming {
                text: "我抢在真主人前面".into(),
                reply_to: None,
                chat_id: 999,
            }),
            crate::bridge::Accepted::Rejected,
            "重启时如果不认持久化的 owner，这里会把陌生人判成 Paired——正是 C1"
        );
        assert_eq!(
            handle.accept(&crate::channel::Incoming {
                text: "我才是主人".into(),
                reply_to: None,
                chat_id: 555,
            }),
            crate::bridge::Accepted::FromOwner
        );
    }
}
