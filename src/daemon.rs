use anyhow::Result;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crate::bridge::Bridge;
use crate::channel::{telegram::Telegram, ChannelError};
use crate::profile::Profile;
use crate::profile::{all_profiles, command_exists, profiles_dir_for_socket, status_of};
use crate::projects::{store_path_for_socket, Store};
use crate::proto::{
    ErrorCode, InstallPrompt, PhoneBrokenReason, PhoneState, PhoneStatus, ProfileEntry, Request,
    Response, SecretPrompt,
};
use crate::secrets::{secrets_path_for_socket, SecretStore, PHONE_BOT_KEY, PHONE_TOKEN_KEY};
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

    // 手机通知这一页存在的全部理由是这一份状态——**不落盘**，落盘的只有
    // 令牌本身和 bot 名字（跟别的密钥同一个文件，见 `PHONE_TOKEN_KEY`/
    // `PHONE_BOT_KEY`）。有没有存过令牌决定开机时是 `Off` 还是
    // `WaitingForPairing`；bot 名字直接从磁盘读，**不打网络**——早先这里
    // 起过一个后台线程去 `getMe` 现查 bot 名字，被 dct-phone-channel
    // Task 4 fix round 1 的 Critical 3 判定为一次没人要求、没有测试、还
    // 会把预置了令牌的集成测试拖去打真实网络的多余动作，删掉了。
    // `apply_phone_set_token` 验证通过的那一刻就把 bot 名字跟着令牌一起
    // 存下来，开机直接读就是最新答案。
    let phone = Arc::new(Mutex::new(initial_phone_status(&secrets)));

    // dct-phone-channel Task 5：如果磁盘上已经有一份令牌（`WaitingForPairing`
    // 或者更早——`initial_phone_status` 刚刚已经读过一次同一个键），起一条
    // 真正在长轮询的 Bridge 线程。**这跟上面注释里删掉的那个「起线程去
    // getMe 现查 bot 名字」不是同一件事**：那是一次性的、会阻塞在「验证
    // 通不通过」上的同步调用，这里是异步的长轮询，不读不写 bot 名字，只
    // 是终于让「配对」这件事在开机时就有人在听——不然一个已经填过令牌、
    // 还没配对成功的用户，重启一次守护进程就再也等不到自己发的那条
    // Telegram 消息。
    let bridge_slot: PhoneBridgeSlot = Arc::new(Mutex::new(None));
    // 先落进一个命名变量，`secrets` 的 `MutexGuard` 才会在这条 `let` 语句
    // 结束时就释放——写成 `if let Some(token) = recover(secrets.lock())…
    // { start_phone_bridge(...) }` 的话，判别式里的临时 `MutexGuard` 会
    // 存活到整个块结束，`start_phone_bridge` 内部虽然锁的是另一把锁
    // （`bridge_slot`，不会自己锁死），但这个模式本身就是
    // `Request::PhoneUnpair` 那个真锁死过的 bug 的同类写法，见那边留的
    // 详细注释——这里改成一样的防御写法，不留着同一个隐患等下一次真的
    // 撞上。
    let saved_token = recover(secrets.lock())
        .get(PHONE_TOKEN_KEY)
        .map(str::to_string);
    if let Some(token) = saved_token {
        start_phone_bridge(
            Arc::new(Bridge::new(Arc::new(Telegram::new(&token)))),
            phone.clone(),
            &bridge_slot,
        );
    }

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
        let bs = bridge_slot.clone();
        std::thread::spawn(move || {
            if let Err(e) = serve(conn, m, s, sec, pd, ph, bs) {
                eprintln!("连接处理失败: {e}");
            }
        });
    }
    Ok(())
}

/// 当前生效的手机 Bridge（如果手机通知功能开着的话）。**换令牌、取消
/// 配对、整个关掉都要经过这里**——`bridge::Bridge` 自己只知道怎么处理
/// 一条消息，「现在该用哪一个 Bridge、该不该起一条新线程」是这个槽位和
/// 它旁边那几个调用点（`start_phone_bridge`、`PhoneUnpair`、
/// `PhoneDisable`）的事。`None` 意味着手机通知没开，或者刚被整个关掉。
type PhoneBridgeSlot = Arc<Mutex<Option<Arc<Bridge>>>>;

/// 把一个刚验证过的令牌接成一条真正在跑的手机通道：把 `Bridge` 塞进共享
/// 槽、起一条轮询线程。三个调用点，见各自调用处的注释：开机时磁盘上已经
/// 有令牌（`run_with_manager`）、`PhoneSetToken` 验证成功、`PhoneUnpair`
/// 从 `Broken { BotBlocked }` 恢复（复用同一个 `Bridge`，令牌没变不用
/// 重新建 Telegram 客户端）。
fn start_phone_bridge(bridge: Arc<Bridge>, phone: Arc<Mutex<PhoneStatus>>, slot: &PhoneBridgeSlot) {
    *recover(slot.lock()) = Some(bridge.clone());
    std::thread::spawn(move || crate::bridge::run(bridge, phone));
}

/// 手机通知刚启动时的状态：有没有存过令牌决定 `Off` 还是
/// `WaitingForPairing`，bot 名字直接从磁盘读（`apply_phone_set_token`
/// 验证通过的那一刻已经把它跟令牌一起存下来了）。**不打网络**——见本文件
/// `run_with_manager` 里那段注释，起一个后台线程去 `getMe` 现查 bot 名字
/// 是这里删掉的一版旧设计，被判定为没人要求、没有测试、还会把预置了
/// 令牌的集成测试拖去打真实网络。
fn initial_phone_status(secrets: &Mutex<SecretStore>) -> PhoneStatus {
    let sec = recover(secrets.lock());
    let has_token = sec.get(PHONE_TOKEN_KEY).is_some();
    PhoneStatus {
        state: if has_token {
            PhoneState::WaitingForPairing
        } else {
            PhoneState::Off
        },
        bot: sec.get(PHONE_BOT_KEY).map(str::to_string),
        owner: None,
    }
}

/// 令牌验证的结果：好使就是 `Ok` 附带 bot 名（`get_me` 成功时**总是**
/// 拿得到一个真实用户名，这不是 `Option`——这个签名本身就是
/// dct-phone-channel Task 4 fix round 2 的 Minor 5 修复：旧签名是
/// `(PhoneState, Option<String>)`，`apply_phone_set_token` 落盘那一支要写
/// `bot.as_deref().unwrap_or("")`，这个 `""` 兜底今天确实到不了（`Ok` 分支
/// 唯一的产出者），但「到不了」是控制流碰巧配合，不是类型逼出来的。现在
/// 落盘那一支拿到的是 `String`，压根没有空字符串可写。
///
/// 失败就是 `Err` 附带一个**已经成文**的 `PhoneState::Broken`——纯判定
/// 逻辑，传输层由调用方注入（同 `verify.rs::verify_with` 的路子）——测试
/// 才能覆盖「令牌好使」「令牌失效」「连不上」三种结果，不用真打 Telegram
/// 的网络。
///
/// **`Broken` 的 `message` 字段绝不能把 `token` 拼进去**：这是
/// `PhoneState::Broken` 唯一的安全承诺，`phone_broken_text_never_contains_the_token`
/// 这条测试钉着它。`reason` 字段是三种真实成因的判定，直接决定
/// `ui/phone.rs::next_step` 该给哪一句话——这是 dct-phone-channel Task 4
/// fix round 1 的根因修复：以前 `Broken` 只有一句写死的字符串，`BadToken`
/// 和 `Unreachable`（后来发现的 Important 1）、`Blocked`（Critical 2 指向
/// 的那句「own message and own next step」）说的是同一句话，用户没法照着
/// 做对的事。
///
/// **`ChannelError::Blocked`（403）在这条路径上映成 `BadToken`，不是
/// `BotBlocked`**——这是 fix round 2 的 Important 2 修复。`PhoneState::
/// has_confirmed_token()` 把 `BotBlocked` 当成「磁盘上有一份确认有效的
/// 令牌」，但这条函数（唯一的 `apply_phone_set_token` 调用方）验证失败
/// 从不落盘：如果这里真的产出 `BotBlocked`，`has_confirmed_token()` 就会
/// 对着一个磁盘上根本不存在的令牌撒谎，`r` 会把用户带回 Critical 2 那个
/// 死胡同——PhoneUnpair 会把状态推成 `WaitingForPairing`，`bot` 却只能是
/// `None`（这条路径压根没拿到真实用户名），status_line 又画出「@?」。
/// 与其指望 Telegram 永远不会在一次没有 chat 上下文的 `getMe` 调用上答
/// 403（`getMe` 问的是"这个令牌是谁"，不是"这个 chat 屏蔽了没"，403
/// 在这里没有语义），不如让这条路径**压根不产出** `BotBlocked`——那个
/// 原因只该由 Task 5 的 Bridge 在一次真正配对之后、往已知 chat 发消息
/// 失败时构造，那时候磁盘上确实有一份令牌，`has_confirmed_token()` 的
/// 承诺才成立。`msg::phone_blocked`/`Key::PhoneNextStepBlocked` 仍然留着
/// ——Task 5 要用，`i18n.rs` 自己的测试直接调用 `msg::phone_blocked` 钉住
/// 它没坏。
fn phone_verify_token(
    token: &str,
    lang: crate::i18n::Lang,
    get_me: &dyn Fn(&str) -> Result<String, ChannelError>,
) -> Result<String, PhoneState> {
    match get_me(token) {
        Ok(bot) => Ok(bot),
        Err(ChannelError::BadToken) | Err(ChannelError::Blocked) => Err(PhoneState::Broken {
            reason: PhoneBrokenReason::BadToken,
            message: crate::i18n::msg::phone_token_invalid(lang),
        }),
        Err(ChannelError::Unreachable) | Err(ChannelError::Malformed) => Err(PhoneState::Broken {
            reason: PhoneBrokenReason::Unreachable,
            message: crate::i18n::msg::phone_unreachable(lang),
        }),
    }
}

/// `Request::PhoneSetToken` 的完整处理：验证、（验过了才）落盘、更新内存
/// 状态。**从 `handle()` 的 match 臂里拆出来单独一个函数，传输层照旧注入**
/// ——这条请求真正碰网络的只有 `get_me` 这一步，`secrets`/`phone` 都是普通
/// 的内存对象，拆开之后不用起真实 Telegram 网络也能测「验证失败真的没有
/// 碰磁盘上的令牌」这条关键行为，不然这条行为只能在 `handle()` 里跟一次
/// 真实网络请求焊在一起，测不到。
fn apply_phone_set_token(
    token: &str,
    lang: crate::i18n::Lang,
    secrets: &Mutex<SecretStore>,
    phone: &Mutex<PhoneStatus>,
    get_me: &dyn Fn(&str) -> Result<String, ChannelError>,
) -> anyhow::Result<Response> {
    match phone_verify_token(token, lang, get_me) {
        Ok(bot) => {
            // 令牌和 bot 名字一起落盘：`initial_phone_status` 开机时直接读
            // 这两个键，不再打网络去现查 bot 名字（见 `run_with_manager` 头上
            // 那段注释）——两个写操作里如果只有第一个成功，重启后会读到一个
            // 有令牌没 bot 名字的状态，「等你在 Telegram 里给 @? 发条消息」
            // 的老问题会从另一个角度冒出来，所以两次 `set` 的结果都要检查。
            //
            // **两次 `secrets.lock()` 必须是两条分开的语句**——`Mutex` 不可
            // 重入，第一次 `recover(secrets.lock())` 产生的 `MutexGuard` 是这
            // 条语句里的一个临时值，Rust 的临时值销毁规则是"整条语句结束时才
            // 释放"，如果写成一条链式表达式（`.and_then` 闭包里再 `lock()`
            // 同一把锁），第一把锁在闭包执行时还没释放，第二次 `lock()` 会
            // 自己把自己锁死——这曾经是本函数一个真实的自死锁 bug。
            let first = recover(secrets.lock()).set(PHONE_TOKEN_KEY, token);
            first
                .and_then(|_| recover(secrets.lock()).set(PHONE_BOT_KEY, &bot))
                .map(|_| {
                    let mut ph = recover(phone.lock());
                    // 新令牌等于新的 bot、新的一轮配对——上一次配上的主人
                    // （如果有）不该继续留着，那会让通知发去一个跟这份新
                    // 令牌毫不相干的旧 chat。
                    *ph = PhoneStatus {
                        state: PhoneState::WaitingForPairing,
                        bot: Some(bot),
                        owner: None,
                    };
                    Response::Phone(ph.clone())
                })
        }
        Err(state) => {
            // 验证失败**不**碰令牌：用户是在填一个新令牌，如果这个新令牌
            // 本身不好使，不该把磁盘上原有的令牌（如果有的话）连带抹掉
            // ——界面上 `Enter` 键只在没有一份「确认有效」的令牌时才会被
            // 提供（见 `PhoneState::has_confirmed_token`），也就是说走到
            // 这条分支时磁盘上原本要么没有令牌、要么已经是坏的，这里不存在
            // 「一个还能用的令牌被覆盖」的风险。`bot` 写成 `None` 同理：
            // `phone_verify_token` 的 `Err` 分支从不产出一个真实用户名。
            let mut ph = recover(phone.lock());
            *ph = PhoneStatus {
                state,
                bot: None,
                owner: None,
            };
            Ok(Response::Phone(ph.clone()))
        }
    }
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
    bridge_slot: PhoneBridgeSlot,
) -> Result<()> {
    let mut out = stream.try_clone()?;
    let reader = BufReader::new(stream);
    for line in reader.lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        let resp = match serde_json::from_str::<Request>(&line) {
            Ok(req) => handle(
                req,
                &mgr,
                &store,
                &secrets,
                &profiles_dir,
                &phone,
                &bridge_slot,
            ),
            Err(e) => Response::Error(ErrorCode::BadRequest(e.to_string())),
        };
        writeln!(out, "{}", serde_json::to_string(&resp)?)?;
        out.flush()?;
    }
    Ok(())
}

fn handle(
    req: Request,
    mgr: &Arc<SessionManager>,
    store: &Arc<Mutex<Store>>,
    secrets: &Arc<Mutex<SecretStore>>,
    profiles_dir: &Path,
    phone: &Arc<Mutex<PhoneStatus>>,
    bridge_slot: &PhoneBridgeSlot,
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
        // 纯读，不碰任何状态、不打网络——手机通知页每一轮轮询都要问它，
        // 必须快。真正的网络往返只发生在 `PhoneSetToken` 里。
        Request::PhoneStatus => Ok(Response::Phone(recover(phone.lock()).clone())),
        Request::PhoneSetToken { token, lang } => {
            let resp =
                apply_phone_set_token(&token, lang, secrets, phone, &|t| Telegram::new(t).get_me());
            // 验证成功才起 Bridge——`apply_phone_set_token` 已经把落盘和
            // 内存状态都判过一遍了，这里只看它的判定结果，不重新判一次。
            // Task 5 之前这条请求成功之后没有任何人会去听 Telegram：
            // `WaitingForPairing` 只是一句好看的状态文案，实际上永远等不到
            // 配对——起这条线程才是这个状态第一次真正生效。
            if matches!(
                &resp,
                Ok(Response::Phone(PhoneStatus {
                    state: PhoneState::WaitingForPairing,
                    ..
                }))
            ) {
                start_phone_bridge(
                    Arc::new(Bridge::new(Arc::new(Telegram::new(&token)))),
                    phone.clone(),
                    bridge_slot,
                );
            }
            resp
        }
        Request::PhoneUnpair => {
            let mut ph = recover(phone.lock());
            // **只有 `has_confirmed_token()` 为真才真的推进状态**——这条
            // 守卫必须跟 `view::phone_key_has_effect` 给出同一个答案（两者
            // 都读 `PhoneState::has_confirmed_token`，唯一共用的判断）。
            //
            // 这不是可选的加固：这里曾经不管这个条件，任何非 `Off` 状态
            // 一律被推成 `WaitingForPairing`——包括 `Broken { BadToken }`/
            // `Broken { Unreachable }`，而这两种情形下 `apply_phone_set_token`
            // 验证失败根本没有落盘。后果是页面自称在等配对，等的却是一个
            // 磁盘上不存在的令牌；界面上 `Enter` 又不再提供（同一份判断，
            // 都读 `has_confirmed_token`），用户没有任何出路，只能靠猜
            // `x`。这是 dct-phone-channel Task 4 fix round 1 的 Critical 2，
            // `phone_unpair_on_a_bad_token_is_a_no_op` 钉着它。
            let had_confirmed_token = ph.state.has_confirmed_token();
            let mut restart_needed = false;
            if had_confirmed_token {
                // `Broken { BotBlocked }` 是这三个「有确认令牌」的状态里
                // 唯一一个背后没有活着的轮询线程的——那条线程往被拉黑的
                // chat 发确认消息失败之后已经自己退出了（见
                // `bridge.rs::send_pairing_confirmation`）。只清 `owner`
                // 不够，等配对得真有人在听，见下面 `restart_needed` 那段。
                restart_needed = matches!(
                    ph.state,
                    PhoneState::Broken {
                        reason: PhoneBrokenReason::BotBlocked,
                        ..
                    }
                );
                ph.state = PhoneState::WaitingForPairing;
                ph.owner = None;
                // bot **不清空**：这条分支只有在原状态已经带着一个「曾经
                // 确认有效」的 bot 名字时才会走到——`Paired`/
                // `WaitingForPairing`/`Broken { BotBlocked }` 都只能由一次
                // 成功的 `apply_phone_set_token` 铺路，一定带着真实 bot
                // 名字。
            }
            let out = ph.clone();
            drop(ph); // 下面要拿 bridge_slot 的锁，不留着 phone 的锁跨过去
            if had_confirmed_token {
                // `set_destination` 的第二个、也是最后一个合法调用点——
                // 见 `bridge.rs` 模块头注释。`bridge_slot` 在这个分支下
                // 今天唯一可能是 `None` 的情形是测试手写状态、从没经过
                // 真实 `PhoneSetToken`——生产路径上 `has_confirmed_token()`
                // 为真必然意味着曾经有过一次成功的 `start_phone_bridge`。
                //
                // **`bridge_slot.lock()` 的结果先落进一个命名变量，不能
                // 写成 `if let Some(bridge) = recover(bridge_slot.lock())
                // .clone() { … }`。** `if let`/`match` 的判别式里创建的临时值
                // ——这里是 `bridge_slot.lock()` 返回的 `MutexGuard`——
                // 存活到整个块结束，不是判别式求值完就释放；`restart_needed`
                // 为真时块里的 `start_phone_bridge` 又会去 `recover(slot
                // .lock())` 同一把锁，自己把自己锁死。**这是真锁死过的
                // bug**：加了 `phone_unpair_from_bot_blocked_restarts_
                // polling_on_the_same_bridge` 这条测试之后，`cargo test`
                // 在这条分支上原地挂死，ps 里能看到测试进程跑了几分钟
                // 没退出——不是这条新测试写错了，是它第一次真正走到了这条
                // 分支（之前所有 `PhoneUnpair` 测试用的都是空 `bridge_slot`，
                // 从没进过这个 `if let`）。
                let bridge = recover(bridge_slot.lock()).clone();
                if let Some(bridge) = bridge {
                    bridge.unpair();
                    if restart_needed {
                        // 令牌没变，复用同一个 Bridge 换一条新线程接着听。
                        start_phone_bridge(bridge, phone.clone(), bridge_slot);
                    }
                }
            }
            Ok(Response::Phone(out))
        }
        Request::PhoneDisable => {
            // 两条分开的语句，同一个理由见 `apply_phone_set_token` 里那条
            // 长注释：`Mutex` 不可重入，写成一条链式表达式会让第一次
            // `secrets.lock()` 的 `MutexGuard`（这条表达式里的临时值，
            // 活到整条语句结束）在 `.and_then` 闭包再次 `lock()` 同一把锁
            // 时还没释放，自己把自己锁死。
            let first = recover(secrets.lock()).remove(PHONE_TOKEN_KEY);
            first
                .and_then(|_| recover(secrets.lock()).remove(PHONE_BOT_KEY))
                .map(|_| {
                    let mut ph = recover(phone.lock());
                    *ph = PhoneStatus {
                        state: PhoneState::Off,
                        bot: None,
                        owner: None,
                    };
                    // 彻底关掉：清空共享槽，把旧 Bridge（如果还在跑）标记
                    // 成 retired——下一次它检查这个标记（循环顶端，或者
                    // 处理完一条消息之后）就会安静退出，不会在令牌已经被
                    // 用户删掉之后还把状态悄悄改回 `Paired`。见
                    // `bridge.rs::Bridge::retire` 头注释里那条有边界的
                    // 承诺：这不保证立刻停，线程可能正卡在一次 `poll()`
                    // 网络调用里。
                    if let Some(bridge) = recover(bridge_slot.lock()).take() {
                        bridge.retire();
                    }
                    Response::Phone(ph.clone())
                })
        }
    };
    r.unwrap_or_else(|e| Response::Error(to_code(e)))
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
    use crate::channel::{Channel, Incoming};
    use std::time::Instant;

    /// 大多数测试根本不关心手机通知——给它们一个干净的 `Off` 状态垫底，
    /// 不用在每个既有 `handle()` 调用点里重复拼这三行。
    fn test_phone() -> Arc<Mutex<PhoneStatus>> {
        Arc::new(Mutex::new(PhoneStatus {
            state: PhoneState::Off,
            bot: None,
            owner: None,
        }))
    }

    /// 同上，给 `bridge_slot`：绝大多数测试不关心真实 Bridge，一个空槽
    /// 就够了——`handle()` 里所有摸 `bridge_slot` 的分支在它是 `None`
    /// 时都是安全的空操作（`PhoneUnpair`/`PhoneDisable` 只在拿到
    /// `Some(bridge)` 时才会调用它，`PhoneSetToken` 自己会话真的建一个）。
    fn test_bridge_slot() -> PhoneBridgeSlot {
        Arc::new(Mutex::new(None))
    }

    /// 假渠道，只用来观察 `handle()` 里那几条摸 `bridge_slot` 的分支有没有
    /// 真的调用 `bridge.rs` 暴露出来的公开方法（`accept`/`unpair`/`poll`）
    /// ——故意不跟 `bridge.rs` 自己测试模块里的 `FakeChannel` 共用：那边测
    /// 的是 `Bridge` 内部的判定逻辑本身，这里测的是「daemon.rs 该不该调它」，
    /// 两者关注点不同，共用一个私有测试类型要么得放宽它的可见性、要么得
    /// 建一条跨模块的测试专用导出，两者都比在这里重写十几行更容易引入
    /// 意外耦合。
    #[derive(Default)]
    struct RecordingChannel {
        destinations: Mutex<Vec<Option<i64>>>,
        poll_calls: Mutex<u32>,
        poll_script: Mutex<std::collections::VecDeque<Result<Vec<Incoming>, ChannelError>>>,
    }

    impl Channel for RecordingChannel {
        fn send(&self, _text: &str) -> Result<crate::channel::MsgId, ChannelError> {
            Ok(0)
        }

        fn poll(&self, _timeout: Duration) -> Result<Vec<Incoming>, ChannelError> {
            *recover(self.poll_calls.lock()) += 1;
            // 跟 `bridge.rs::FakeChannel` 同一个理由：脚本耗尽必须回一个
            // 终态错误，不能回空批次——不然一个没预备脚本的测试会让轮询
            // 线程真的转成死循环，把测试挂起而不是让它快速失败。
            recover(self.poll_script.lock())
                .pop_front()
                .unwrap_or(Err(ChannelError::BadToken))
        }

        fn set_destination(&self, chat: Option<i64>) {
            recover(self.destinations.lock()).push(chat);
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
            &test_bridge_slot(),
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

        let phone = test_phone();
        let labels = |lang| match handle(
            Request::Profiles { lang },
            &mgr,
            &store,
            &secrets,
            profiles_dir.path(),
            &phone,
            &test_bridge_slot(),
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
        let phone2 = test_phone();
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
                &phone2,
                &test_bridge_slot(),
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
                &test_bridge_slot(),
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
            &test_bridge_slot(),
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
            &test_bridge_slot(),
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

    // ———— 手机通知（Task 4）————
    //
    // `phone_verify_token` 是判定逻辑本身，传输层被注入（同
    // `verify_with`/`verify.rs` 的路子）：这里覆盖「令牌好使」「令牌被拒」
    // 「连不上」三种结果，不用真打 Telegram 的网络。`handle()` 里真正调
    // `Telegram::new(t).get_me()` 走真实传输的那一支，跟 `send_real`/
    // `Request::VerifySecret` 一样不在单元测试范围内——那是实测那一步验的
    // 东西。

    #[test]
    fn phone_verify_token_marks_a_good_token_waiting_for_pairing() {
        let bot = phone_verify_token("tok", crate::i18n::Lang::Zh, &|_| {
            Ok("my_dct_bot".to_string())
        });
        assert_eq!(bot.as_deref(), Ok("my_dct_bot"));
    }

    #[test]
    fn phone_verify_token_marks_a_bad_token_broken() {
        let state = phone_verify_token("tok", crate::i18n::Lang::Zh, &|_| {
            Err(ChannelError::BadToken)
        })
        .expect_err("坏令牌应该是 Err");
        assert!(matches!(
            state,
            PhoneState::Broken {
                reason: PhoneBrokenReason::BadToken,
                ..
            }
        ));
        let PhoneState::Broken { message, .. } = state else {
            unreachable!()
        };
        assert!(!message.is_empty(), "Broken 必须带一句人话，不能是空字符串");
    }

    #[test]
    fn phone_verify_token_marks_network_trouble_broken_too() {
        for e in [ChannelError::Unreachable, ChannelError::Malformed] {
            let state = phone_verify_token("tok", crate::i18n::Lang::Zh, &move |_| Err(e))
                .expect_err("连不上应该是 Err");
            assert!(
                matches!(
                    state,
                    PhoneState::Broken {
                        reason: PhoneBrokenReason::Unreachable,
                        ..
                    }
                ),
                "{e:?} 也该是 Broken{{Unreachable}}"
            );
        }
    }

    /// **fix round 2 的 Important 2。** 403 在这条「刚填令牌」的路径上
    /// 映成 `BadToken`，**不是** `BotBlocked`——`getMe` 没有 chat 上下文，
    /// 一个真实的 403 只会在 Task 5 的 Bridge 往已配对的 chat 发消息时
    /// 出现，那时候磁盘上确实有令牌。如果这里让它映成 `BotBlocked`，
    /// `PhoneState::has_confirmed_token()` 就会对着一个从没落盘过的令牌
    /// 撒谎——`apply_phone_set_token` 对所有 `Broken` 一律不落盘，两边
    /// 对不上就是 Critical 2 的死胡同重演一遍：`r` 把状态推成
    /// `WaitingForPairing`，`bot` 却是 `None`（这条路径压根拿不到真实
    /// 用户名），画面又是「@?」。
    #[test]
    fn phone_verify_token_maps_blocked_to_bad_token_not_bot_blocked() {
        let state = phone_verify_token("tok", crate::i18n::Lang::Zh, &|_| {
            Err(ChannelError::Blocked)
        })
        .expect_err("403 应该是 Err");
        assert!(
            matches!(
                state,
                PhoneState::Broken {
                    reason: PhoneBrokenReason::BadToken,
                    ..
                }
            ),
            "403 在这条路径上不该产出 BotBlocked，得到 {state:?}"
        );
    }

    /// **不是巧合，是两句不同的话。** `BadToken`（令牌本身不好使）和
    /// `Unreachable`/`Malformed`（连不上/读不懂）该说的下一步完全不一样
    /// ——前者是「重新填一遍」，后者是「等会儿再试」。只断言两边都是
    /// `Broken(_)` 分不出这两句话有没有被悄悄换成同一句（比如都写成
    /// `phone_unreachable`），这条测试直接比对消息内容本身，钉死
    /// `phone_verify_token` 按错误类型分派到了不同的 `msg::` 函数。
    #[test]
    fn bad_token_and_network_trouble_produce_different_messages() {
        let bad = phone_verify_token("tok", crate::i18n::Lang::Zh, &|_| {
            Err(ChannelError::BadToken)
        })
        .expect_err("坏令牌应该是 Err");
        let unreachable = phone_verify_token("tok", crate::i18n::Lang::Zh, &|_| {
            Err(ChannelError::Unreachable)
        })
        .expect_err("连不上应该是 Err");
        let PhoneState::Broken {
            message: bad_msg, ..
        } = bad
        else {
            unreachable!()
        };
        let PhoneState::Broken {
            message: unreachable_msg,
            ..
        } = unreachable
        else {
            unreachable!()
        };
        assert_ne!(
            bad_msg, unreachable_msg,
            "令牌失效和连不上网络不该说同一句话——下一步完全不同"
        );
        assert_eq!(
            bad_msg,
            crate::i18n::msg::phone_token_invalid(crate::i18n::Lang::Zh)
        );
        assert_eq!(
            unreachable_msg,
            crate::i18n::msg::phone_unreachable(crate::i18n::Lang::Zh)
        );
    }

    /// **安全属性，不是巧合。** `phone_verify_token` 的两个 `Broken` 分支
    /// 都是固定文案，压根不读 `token` 参数——这条测试用一个看起来像真实
    /// Telegram 令牌的字符串去调用它，确认返回的 `Broken` 消息里一个字符
    /// 都没有它。守护进程是唯一决定用户看到什么文字的地方（`PhoneState::
    /// Broken` 的文档注释），这条纪律必须钉在这一层，不能只指望界面那边
    /// 的纵深防御（`ui/phone.rs` 的 `status_line`/`next_step` 故意不读
    /// payload）。
    #[test]
    fn phone_broken_text_never_contains_the_token() {
        let real_looking_token = "123456789:AAH-super-secret-telegram-token";
        for err in [
            ChannelError::BadToken,
            ChannelError::Blocked,
            ChannelError::Unreachable,
            ChannelError::Malformed,
        ] {
            let state = phone_verify_token(real_looking_token, crate::i18n::Lang::Zh, &move |_| {
                Err(err)
            })
            .expect_err(&format!("{err:?} 应该是 Err"));
            let PhoneState::Broken { message, .. } = state else {
                panic!("{err:?} 应该是 Broken")
            };
            assert!(
                !message.contains(real_looking_token),
                "令牌漏进了 Broken 文案：{message}"
            );
        }
    }

    #[test]
    fn initial_phone_status_is_off_without_a_saved_token() {
        let secrets = Mutex::new(SecretStore::load(
            &tempfile::tempdir().unwrap().path().join("secrets.toml"),
        ));
        assert_eq!(initial_phone_status(&secrets).state, PhoneState::Off);
    }

    #[test]
    fn initial_phone_status_is_waiting_for_pairing_with_a_saved_token() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = SecretStore::load(&dir.path().join("secrets.toml"));
        store.set(PHONE_TOKEN_KEY, "some-token").unwrap();
        let secrets = Mutex::new(store);
        let status = initial_phone_status(&secrets);
        assert_eq!(status.state, PhoneState::WaitingForPairing);
        // 没顺带存 bot 名字（这条测试只塞了令牌）时兜底成 None——这是一个
        // 正常但不该常见的情形（`apply_phone_set_token` 成功时永远把两个
        // 键一起存），不是需要打网络才能知道的「还不知道」。
        assert!(status.bot.is_none());
    }

    /// **正常路径。** `apply_phone_set_token` 成功时把令牌和 bot 名字一起
    /// 存下来，`initial_phone_status` 开机直接读，**不打网络**——这是
    /// dct-phone-channel Task 4 fix round 1 删掉 `spawn_phone_startup_refresh`
    /// 之后唯一的 bot 名字来源。少了这条覆盖，「重启后 bot 名字还在」这个
    /// 事实全靠人读代码相信，测不出回归。
    #[test]
    fn initial_phone_status_reads_the_bot_name_from_disk_without_any_network_call() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = SecretStore::load(&dir.path().join("secrets.toml"));
        store.set(PHONE_TOKEN_KEY, "some-token").unwrap();
        store.set(PHONE_BOT_KEY, "my_dct_bot").unwrap();
        let secrets = Mutex::new(store);
        let status = initial_phone_status(&secrets);
        assert_eq!(status.state, PhoneState::WaitingForPairing);
        assert_eq!(status.bot.as_deref(), Some("my_dct_bot"));
    }

    /// 令牌好使：落盘、内存状态推进到 `WaitingForPairing`、主人清空
    /// （新令牌等于新的一轮配对）。
    #[test]
    fn apply_phone_set_token_saves_a_good_token() {
        let secrets = Mutex::new(SecretStore::load(
            &tempfile::tempdir().unwrap().path().join("secrets.toml"),
        ));
        let phone = Mutex::new(PhoneStatus {
            state: PhoneState::Off,
            bot: None,
            owner: None,
        });

        let resp = apply_phone_set_token(
            "good-token",
            crate::i18n::Lang::Zh,
            &secrets,
            &phone,
            &|_| Ok("my_dct_bot".to_string()),
        )
        .unwrap();

        match resp {
            Response::Phone(status) => {
                assert_eq!(status.state, PhoneState::WaitingForPairing);
                assert_eq!(status.bot.as_deref(), Some("my_dct_bot"));
                assert!(status.owner.is_none());
            }
            other => panic!("期待 Response::Phone，得到 {other:?}"),
        }
        assert_eq!(
            recover(secrets.lock()).get(PHONE_TOKEN_KEY),
            Some("good-token"),
            "验证通过的令牌必须落盘，不然重启就没了"
        );
        assert_eq!(
            recover(secrets.lock()).get(PHONE_BOT_KEY),
            Some("my_dct_bot"),
            "bot 名字也必须落盘——不存的话，重启后 initial_phone_status \
             要么打一次网络去现查（Critical 3 的根因），要么永远显示 @?"
        );
    }

    /// **关键行为，专门有一条测试盯着。** 令牌被拒时**不能**把它写进
    /// `secrets.toml`——磁盘上原来的令牌（如果有）必须原封不动。这条只能
    /// 靠拆出 `apply_phone_set_token` 才测得到：留在 `handle()` 里的话，
    /// 这一支会先打一次真实的 `Telegram::new(t).get_me()` 网络请求，测试
    /// 要么连不上网直接假失败，要么就得连真网络——两者都不是单元测试该做
    /// 的事。
    #[test]
    fn apply_phone_set_token_does_not_save_a_bad_token() {
        let secrets = Mutex::new(SecretStore::load(
            &tempfile::tempdir().unwrap().path().join("secrets.toml"),
        ));
        let phone = Mutex::new(PhoneStatus {
            state: PhoneState::Off,
            bot: None,
            owner: None,
        });

        let resp = apply_phone_set_token(
            "bad-token",
            crate::i18n::Lang::Zh,
            &secrets,
            &phone,
            &|_| Err(ChannelError::BadToken),
        )
        .unwrap();

        assert!(matches!(
            resp,
            Response::Phone(PhoneStatus {
                state: PhoneState::Broken { .. },
                ..
            })
        ));
        assert!(
            recover(secrets.lock()).get(PHONE_TOKEN_KEY).is_none(),
            "验证失败的令牌绝不能落盘"
        );
        assert!(
            recover(secrets.lock()).get(PHONE_BOT_KEY).is_none(),
            "验证失败也不该落一个 bot 名字下来"
        );
    }

    #[test]
    fn phone_status_reflects_whatever_is_in_the_shared_cell() {
        let mgr = Arc::new(SessionManager::new());
        let secrets = Arc::new(Mutex::new(SecretStore::load(
            &tempfile::tempdir().unwrap().path().join("secrets.toml"),
        )));
        let store = Arc::new(Mutex::new(Store::load(
            &tempfile::tempdir().unwrap().path().join("projects.json"),
        )));
        let profiles_dir = tempfile::tempdir().unwrap();
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
            &test_bridge_slot(),
        );
        match resp {
            Response::Phone(status) => {
                assert_eq!(status.state, PhoneState::Paired);
                assert_eq!(status.bot.as_deref(), Some("my_dct_bot"));
                assert_eq!(status.owner.as_deref(), Some("lei"));
            }
            other => panic!("期待 Response::Phone，得到 {other:?}"),
        }
    }

    /// `r`（重新配对）把主人忘掉、退回等配对，令牌和 bot 名字不动。
    #[test]
    fn phone_unpair_forgets_the_owner_but_keeps_the_token_alive() {
        let mgr = Arc::new(SessionManager::new());
        let secrets = Arc::new(Mutex::new(SecretStore::load(
            &tempfile::tempdir().unwrap().path().join("secrets.toml"),
        )));
        let store = Arc::new(Mutex::new(Store::load(
            &tempfile::tempdir().unwrap().path().join("projects.json"),
        )));
        let profiles_dir = tempfile::tempdir().unwrap();
        let phone = Arc::new(Mutex::new(PhoneStatus {
            state: PhoneState::Paired,
            bot: Some("my_dct_bot".into()),
            owner: Some("lei".into()),
        }));

        let resp = handle(
            Request::PhoneUnpair,
            &mgr,
            &store,
            &secrets,
            profiles_dir.path(),
            &phone,
            &test_bridge_slot(),
        );
        match resp {
            Response::Phone(status) => {
                assert_eq!(status.state, PhoneState::WaitingForPairing);
                assert_eq!(status.bot.as_deref(), Some("my_dct_bot"), "bot 不该被忘掉");
                assert!(status.owner.is_none(), "主人要被忘掉，这才是重新配对");
            }
            other => panic!("期待 Response::Phone，得到 {other:?}"),
        }
    }

    /// 没填过令牌时按 r 没有意义——不能凭空造出一个 `WaitingForPairing`，
    /// 那会让界面显示一件用户从没做过的事（填过令牌）。
    #[test]
    fn phone_unpair_on_off_stays_off() {
        let mgr = Arc::new(SessionManager::new());
        let secrets = Arc::new(Mutex::new(SecretStore::load(
            &tempfile::tempdir().unwrap().path().join("secrets.toml"),
        )));
        let store = Arc::new(Mutex::new(Store::load(
            &tempfile::tempdir().unwrap().path().join("projects.json"),
        )));
        let profiles_dir = tempfile::tempdir().unwrap();
        let phone = test_phone(); // Off

        let resp = handle(
            Request::PhoneUnpair,
            &mgr,
            &store,
            &secrets,
            profiles_dir.path(),
            &phone,
            &test_bridge_slot(),
        );
        match resp {
            Response::Phone(status) => assert_eq!(status.state, PhoneState::Off),
            other => panic!("期待 Response::Phone，得到 {other:?}"),
        }
    }

    /// **回归测试，dct-phone-channel Task 4 fix round 1 的 Critical 2。**
    /// `r` 在 `Broken { BadToken }`（或 `Unreachable`）下必须是空操作——
    /// `apply_phone_set_token` 验证失败根本没有落盘，这两种情形下磁盘上
    /// 压根没有一份「确认有效」的令牌。旧代码不管这个条件，任何非 `Off`
    /// 状态一律被推成 `WaitingForPairing`：页面自称在等配对，等的却是一个
    /// 不存在的 bot（`bot: None`，状态行只能显示「@?」），而这句话正是
    /// `PhoneNextStepBadToken`/`PhoneNextStepUnreachable` 告诉用户去按的
    /// 那个键——一个跟着提示走却把自己带进死胡同的陷阱。
    #[test]
    fn phone_unpair_on_a_bad_token_is_a_no_op() {
        for reason in [PhoneBrokenReason::BadToken, PhoneBrokenReason::Unreachable] {
            let mgr = Arc::new(SessionManager::new());
            let secrets = Arc::new(Mutex::new(SecretStore::load(
                &tempfile::tempdir().unwrap().path().join("secrets.toml"),
            )));
            let store = Arc::new(Mutex::new(Store::load(
                &tempfile::tempdir().unwrap().path().join("projects.json"),
            )));
            let profiles_dir = tempfile::tempdir().unwrap();
            let phone = Arc::new(Mutex::new(PhoneStatus {
                state: PhoneState::Broken {
                    reason,
                    message: "x".into(),
                },
                bot: None,
                owner: None,
            }));

            let resp = handle(
                Request::PhoneUnpair,
                &mgr,
                &store,
                &secrets,
                profiles_dir.path(),
                &phone,
                &test_bridge_slot(),
            );
            match resp {
                Response::Phone(status) => assert!(
                    matches!(status.state, PhoneState::Broken { reason: r, .. } if r == reason),
                    "{reason:?}：r 必须原地不动，不能伪造出 WaitingForPairing"
                ),
                other => panic!("期待 Response::Phone，得到 {other:?}"),
            }
        }
    }

    /// `r` 在 `Broken { BotBlocked }` 下**要**推进——令牌本身没坏，只是这个
    /// bot 被拉黑了，重新配对是有意义的动作，而且要留着原来那个 bot 名字
    /// （不是伪造一个新的、也不是清空）。这条状态从 Task 5 起真的会被构造
    /// （`bridge.rs::send_pairing_confirmation`），但这条测试仍然手写状态、
    /// 用一个空 `bridge_slot`——它钉的是状态转移本身（`state`/`bot` 对不
    /// 对），不是「有没有真的再起一条轮询线程」；那部分见下面
    /// `phone_unpair_from_bot_blocked_restarts_polling_on_the_same_bridge`。
    #[test]
    fn phone_unpair_on_a_blocked_bot_repairs_and_keeps_the_bot_name() {
        let mgr = Arc::new(SessionManager::new());
        let secrets = Arc::new(Mutex::new(SecretStore::load(
            &tempfile::tempdir().unwrap().path().join("secrets.toml"),
        )));
        let store = Arc::new(Mutex::new(Store::load(
            &tempfile::tempdir().unwrap().path().join("projects.json"),
        )));
        let profiles_dir = tempfile::tempdir().unwrap();
        let phone = Arc::new(Mutex::new(PhoneStatus {
            state: PhoneState::Broken {
                reason: PhoneBrokenReason::BotBlocked,
                message: "x".into(),
            },
            bot: Some("my_dct_bot".into()),
            owner: None,
        }));

        let resp = handle(
            Request::PhoneUnpair,
            &mgr,
            &store,
            &secrets,
            profiles_dir.path(),
            &phone,
            &test_bridge_slot(),
        );
        match resp {
            Response::Phone(status) => {
                assert_eq!(status.state, PhoneState::WaitingForPairing);
                assert_eq!(status.bot.as_deref(), Some("my_dct_bot"), "bot 不该被忘掉");
            }
            other => panic!("期待 Response::Phone，得到 {other:?}"),
        }
    }

    /// `PhoneUnpair` 在「有确认令牌」的分支上必须真的碰一下 `bridge_slot`
    /// 里那个 Bridge——只改 `phone` 状态槽、不通知 `Bridge` 自己会留下一个
    /// 活着的旧目的地。`unpair()` 是 `Channel::set_destination` 唯一合法
    /// 的第二个调用点（见 `bridge.rs` 模块头注释「`set_destination` 只从
    /// 这里调用」），这条测试钉住 `daemon.rs` 真的走到了那个调用点，不是
    /// 只钉「协议层面的状态转移对了」——上面那几条 `phone_unpair_*` 测试
    /// 用的都是空 `bridge_slot`，看不到这一步有没有发生。
    #[test]
    fn phone_unpair_tells_a_present_bridge_to_forget_its_destination() {
        let mgr = Arc::new(SessionManager::new());
        let secrets = Arc::new(Mutex::new(SecretStore::load(
            &tempfile::tempdir().unwrap().path().join("secrets.toml"),
        )));
        let store = Arc::new(Mutex::new(Store::load(
            &tempfile::tempdir().unwrap().path().join("projects.json"),
        )));
        let profiles_dir = tempfile::tempdir().unwrap();
        let phone = Arc::new(Mutex::new(PhoneStatus {
            state: PhoneState::Paired,
            bot: Some("my_dct_bot".into()),
            owner: Some("lei".into()),
        }));

        let ch = Arc::new(RecordingChannel::default());
        let bridge = Arc::new(Bridge::new(ch.clone()));
        // 让这个 Bridge 先真的配对一次，制造出「已经有出站目的地」的现实
        // 起点——不然测的只是「在一个从没配过对的 Bridge 上调 unpair 不
        // 炸」，跟这条测试想钉的东西无关。
        bridge.accept(&Incoming {
            text: "在吗".into(),
            reply_to: None,
            chat_id: 111,
        });
        let bridge_slot: PhoneBridgeSlot = Arc::new(Mutex::new(Some(bridge)));

        handle(
            Request::PhoneUnpair,
            &mgr,
            &store,
            &secrets,
            profiles_dir.path(),
            &phone,
            &bridge_slot,
        );

        assert_eq!(
            *recover(ch.destinations.lock()),
            vec![Some(111), None],
            "unpair 必须真的把出站目的地清空，不然旧配对没被真的撤销"
        );
    }

    /// 反过来：`had_confirmed_token` 为假时（比如令牌被撤销之后自然落到的
    /// `Broken { BadToken }`），`PhoneUnpair` 不该碰 `bridge_slot` 里的
    /// Bridge——那个状态意味着背后的轮询线程早就因为同一个 401 自己停了，
    /// `unpair()` 在这里没有对象可作用；真正的出路是用户重新填一遍令牌
    /// （`Enter`），不是 `r`，见 `phone_unpair_on_a_bad_token_is_a_no_op`。
    /// 这条测试补的是「就算 `bridge_slot` 里凑巧还留着一个 Bridge，也不能
    /// 被误碰」——协议层面的 no-op 之外，再钉一层「渠道真的没被动」。
    #[test]
    fn phone_unpair_leaves_a_present_bridge_alone_when_there_was_no_confirmed_token() {
        let mgr = Arc::new(SessionManager::new());
        let secrets = Arc::new(Mutex::new(SecretStore::load(
            &tempfile::tempdir().unwrap().path().join("secrets.toml"),
        )));
        let store = Arc::new(Mutex::new(Store::load(
            &tempfile::tempdir().unwrap().path().join("projects.json"),
        )));
        let profiles_dir = tempfile::tempdir().unwrap();
        let phone = Arc::new(Mutex::new(PhoneStatus {
            state: PhoneState::Broken {
                reason: PhoneBrokenReason::BadToken,
                message: "x".into(),
            },
            bot: None,
            owner: None,
        }));

        let ch = Arc::new(RecordingChannel::default());
        let bridge = Arc::new(Bridge::new(ch.clone()));
        bridge.accept(&Incoming {
            text: "在吗".into(),
            reply_to: None,
            chat_id: 111,
        });
        let bridge_slot: PhoneBridgeSlot = Arc::new(Mutex::new(Some(bridge)));

        handle(
            Request::PhoneUnpair,
            &mgr,
            &store,
            &secrets,
            profiles_dir.path(),
            &phone,
            &bridge_slot,
        );

        assert_eq!(
            *recover(ch.destinations.lock()),
            vec![Some(111)],
            "没有确认过的令牌，unpair 不该再碰这个 Bridge"
        );
    }

    /// `restart_needed`（`Broken { BotBlocked }` 分支）必须真的再起一条
    /// 轮询线程，不能只把状态改回 `WaitingForPairing` 骗界面——那条线程在
    /// 配对确认被拒（403）时已经自己退出了（`bridge.rs::
    /// send_pairing_confirmation`），只清 `owner`/目的地没有人在背后听，
    /// 用户在 Telegram 里发的下一条消息会石沉大海。用一个只回一次终态
    /// 错误的假渠道，让新线程 `poll()` 一次就自己收敛——测的是「真的又
    /// 调用了一次 `poll()`」，不是「进程没崩」。
    #[test]
    fn phone_unpair_from_bot_blocked_restarts_polling_on_the_same_bridge() {
        let mgr = Arc::new(SessionManager::new());
        let secrets = Arc::new(Mutex::new(SecretStore::load(
            &tempfile::tempdir().unwrap().path().join("secrets.toml"),
        )));
        let store = Arc::new(Mutex::new(Store::load(
            &tempfile::tempdir().unwrap().path().join("projects.json"),
        )));
        let profiles_dir = tempfile::tempdir().unwrap();
        let phone = Arc::new(Mutex::new(PhoneStatus {
            state: PhoneState::Broken {
                reason: PhoneBrokenReason::BotBlocked,
                message: "x".into(),
            },
            bot: Some("my_dct_bot".into()),
            owner: None,
        }));

        let ch = Arc::new(RecordingChannel::default());
        recover(ch.poll_script.lock()).push_back(Err(ChannelError::BadToken));
        let bridge = Arc::new(Bridge::new(ch.clone()));
        let bridge_slot: PhoneBridgeSlot = Arc::new(Mutex::new(Some(bridge)));

        handle(
            Request::PhoneUnpair,
            &mgr,
            &store,
            &secrets,
            profiles_dir.path(),
            &phone,
            &bridge_slot,
        );

        // 新线程是异步起的，`handle()` 返回时不保证它已经跑完——有界等待
        // 而不是立刻断言，同 `tests/signal_restore.rs::wait_until_*` 一个
        // 道理：真实调度延迟是毫秒级的，两秒的上限只是不让一次真的失败
        // 挂起整个测试跑不完。
        let start = Instant::now();
        while *recover(ch.poll_calls.lock()) == 0 && start.elapsed() < Duration::from_secs(2) {
            std::thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(
            *recover(ch.poll_calls.lock()),
            1,
            "PhoneUnpair 的 BotBlocked 分支必须真的再起一条轮询线程"
        );
    }

    /// `phone_unpair_forgets_the_owner_but_keeps_the_token_alive`（上面那条）
    /// 只测了 `Paired` 起点。`WaitingForPairing` 是这条分支上另一个真实
    /// 起点——`apply_phone_set_token` 验证成功、还没收到第一条配对消息时
    /// 就是这个状态，补上。
    ///
    /// **`owner` 起点特意设成 `Some(..)`，不是原来的 `None`（fix round 2
    /// 的 Minor 3）。** 原来那个起点本身就是 `None`，断言「还是
    /// `WaitingForPairing`、bot 没丢」不会因为 `owner` 而红——删掉
    /// `daemon.rs` 那个 `if` 分支的整个函数体（守卫连同它保护的赋值一起
    /// 消失），`ph` 原地不动，这条测试照样绿：`state`/`bot` 没变过，
    /// `owner` 本来就是 `None`。现在起点带着一个真实的 `owner`，「`r`
    /// 必须清掉它」这条断言只有守卫真的执行了赋值才会成立。
    #[test]
    fn phone_unpair_from_waiting_for_pairing_stays_waiting_and_keeps_the_bot() {
        let mgr = Arc::new(SessionManager::new());
        let secrets = Arc::new(Mutex::new(SecretStore::load(
            &tempfile::tempdir().unwrap().path().join("secrets.toml"),
        )));
        let store = Arc::new(Mutex::new(Store::load(
            &tempfile::tempdir().unwrap().path().join("projects.json"),
        )));
        let profiles_dir = tempfile::tempdir().unwrap();
        let phone = Arc::new(Mutex::new(PhoneStatus {
            state: PhoneState::WaitingForPairing,
            bot: Some("my_dct_bot".into()),
            owner: Some("lei".into()),
        }));

        let resp = handle(
            Request::PhoneUnpair,
            &mgr,
            &store,
            &secrets,
            profiles_dir.path(),
            &phone,
            &test_bridge_slot(),
        );
        match resp {
            Response::Phone(status) => {
                assert_eq!(status.state, PhoneState::WaitingForPairing);
                assert_eq!(status.bot.as_deref(), Some("my_dct_bot"));
                assert!(status.owner.is_none(), "r 是重新配对，上一个主人必须被忘掉");
            }
            other => panic!("期待 Response::Phone，得到 {other:?}"),
        }
    }

    /// `x`（整个关掉）要把令牌从磁盘上删掉，不只是内存里的状态复位——
    /// 不删的话，下次守护进程重启，`initial_phone_status` 又会看见这份
    /// 令牌，把一个用户已经明确关掉的功能悄悄打开。
    #[test]
    fn phone_disable_deletes_the_token_and_resets_to_off() {
        let mgr = Arc::new(SessionManager::new());
        let secrets_path = tempfile::tempdir().unwrap().path().join("secrets.toml");
        let mut disk = SecretStore::load(&secrets_path);
        disk.set(PHONE_TOKEN_KEY, "some-token").unwrap();
        disk.set(PHONE_BOT_KEY, "my_dct_bot").unwrap();
        let secrets = Arc::new(Mutex::new(disk));
        let store = Arc::new(Mutex::new(Store::load(
            &tempfile::tempdir().unwrap().path().join("projects.json"),
        )));
        let profiles_dir = tempfile::tempdir().unwrap();
        let phone = Arc::new(Mutex::new(PhoneStatus {
            state: PhoneState::WaitingForPairing,
            bot: Some("my_dct_bot".into()),
            owner: None,
        }));

        let resp = handle(
            Request::PhoneDisable,
            &mgr,
            &store,
            &secrets,
            profiles_dir.path(),
            &phone,
            &test_bridge_slot(),
        );
        match resp {
            Response::Phone(status) => {
                assert_eq!(status.state, PhoneState::Off);
                assert!(status.bot.is_none());
                assert!(status.owner.is_none());
            }
            other => panic!("期待 Response::Phone，得到 {other:?}"),
        }
        assert!(
            recover(secrets.lock()).get(PHONE_TOKEN_KEY).is_none(),
            "令牌必须真的从磁盘上删掉，不能只改内存状态"
        );
        assert!(
            recover(secrets.lock()).get(PHONE_BOT_KEY).is_none(),
            "bot 名字也要一起删掉，不然重新填令牌之前它还留在磁盘上"
        );
    }

    /// `x` 必须真的让旧 Bridge 停下来，不只是把它从槽里摘掉——摘掉不等于
    /// 停掉：如果背后那条轮询线程还活着，它迟早会再收到一条消息，把
    /// `phone` 状态槽悄悄改回 `Paired`，而用户看到的却是自己刚刚点掉的
    /// `Off`。`retire()` 的效果全在一个私有原子量里，靠
    /// `is_retired_for_test()`（`bridge.rs` 里一个 `#[cfg(test)]` 开孔）
    /// 直接确认调用生效，而不是从「槽变空了」反推「一定调用过 retire」
    /// ——那两件事在代码里是分开写的两条语句，各自都可能被漏掉。
    #[test]
    fn phone_disable_retires_a_present_bridge_and_clears_the_slot() {
        let mgr = Arc::new(SessionManager::new());
        let secrets_path = tempfile::tempdir().unwrap().path().join("secrets.toml");
        let mut disk = SecretStore::load(&secrets_path);
        disk.set(PHONE_TOKEN_KEY, "some-token").unwrap();
        disk.set(PHONE_BOT_KEY, "my_dct_bot").unwrap();
        let secrets = Arc::new(Mutex::new(disk));
        let store = Arc::new(Mutex::new(Store::load(
            &tempfile::tempdir().unwrap().path().join("projects.json"),
        )));
        let profiles_dir = tempfile::tempdir().unwrap();
        let phone = Arc::new(Mutex::new(PhoneStatus {
            state: PhoneState::Paired,
            bot: Some("my_dct_bot".into()),
            owner: Some("lei".into()),
        }));

        let ch = Arc::new(RecordingChannel::default());
        let bridge = Arc::new(Bridge::new(ch));
        assert!(!bridge.is_retired_for_test(), "起点必须不是已经退休的");
        let bridge_slot: PhoneBridgeSlot = Arc::new(Mutex::new(Some(bridge.clone())));

        handle(
            Request::PhoneDisable,
            &mgr,
            &store,
            &secrets,
            profiles_dir.path(),
            &phone,
            &bridge_slot,
        );

        assert!(
            bridge.is_retired_for_test(),
            "PhoneDisable 必须真的调用 Bridge::retire，不能只清空槽位"
        );
        assert!(
            recover(bridge_slot.lock()).is_none(),
            "槽位也要清空——不然下一次 PhoneSetToken/PhoneUnpair 会摸到一个已经退休的 Bridge"
        );
    }
}
