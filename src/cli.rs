//! `dct ps` / `dct stop`：不开界面，直接从普通终端看会话、停会话。
//!
//! 为什么需要它：守护进程活得比界面久（那正是它存在的理由），所以「有东西
//! 在后台跑着」和「我现在能看见它」是两回事。在这之前，想知道有哪些 agent
//! 还活着、想把它们停掉，只能靠 `ps` 加 `pkill` 去认进程——而那条路认的是
//! **进程**不是**会话**，`pkill -f "dct daemon"` 会把所有会话一起带走，用户
//! 分不出自己杀掉的是哪一个。
//!
//! 两条命令都**不会**顺手拉起守护进程。`dct` 开界面时拉是对的（用户就是要
//! 用它），但「问问有没有东西在跑」不该把「没有」变成「现在有了」。
//!
//! 渲染和参数解析都抽成纯函数，跟 socket 无关，所以能直接测——`dct stop`
//! 是不可撤销的，「哪些参数意味着停掉全部」这件事必须在没有守护进程的
//! 情况下也测得到。

use std::path::Path;

use anyhow::Result;

use crate::client::Client;
use crate::i18n::{text, Key, Lang};
use crate::proto::{Request, Response};
use crate::session::{SessionInfo, SessionState};

/// `dct stop` / `dct kill` 的参数解析结果。两条命令的参数形状一模一样，
/// 规矩也一模一样——**要哪个必须明写**——所以共用一份解析。
#[derive(Debug, PartialEq, Eq)]
pub enum Target {
    /// 这几个会话
    Ids(Vec<u32>),
    /// 全部
    All,
    /// 参数不对，把该说的话说清楚
    Usage(String),
}

/// 解析 `dct stop` / `dct kill` 后面的参数。
///
/// **不给参数不等于「全部」。** 这两条命令都不可撤销，而它们又都是最容易被
/// 手滑敲出来的形式——默认成全部的话，用户想停一个却停光了所有 agent，
/// 正在跑的活全断。要全部必须明写 `--all`。
///
/// `cmd` 只影响用法提示里印的是 `dct stop 3` 还是 `dct kill 3`：用户敲的是
/// 哪条命令，回话里就得是哪条。印错了等于让他去解一个他没问的问题。
pub fn parse_target_args(args: &[String], lang: Lang, cmd: &str) -> Target {
    if args.is_empty() {
        return Target::Usage(crate::i18n::msg::needs_a_target(lang, cmd));
    }
    if args.iter().any(|a| a == "--all") {
        // `--all` 跟具体 id 混着给，说明用户自己也没想清楚要停什么。
        // 与其猜一个，不如让他重敲一遍——这条命令撤不回来。
        if args.len() > 1 {
            return Target::Usage(crate::i18n::msg::all_takes_no_ids(lang, cmd));
        }
        return Target::All;
    }
    let mut ids = Vec::new();
    for a in args {
        match a.parse::<u32>() {
            Ok(n) => ids.push(n),
            Err(_) => return Target::Usage(crate::i18n::msg::not_a_session_id(lang, a)),
        }
    }
    Target::Ids(ids)
}

fn status_word(s: SessionState, lang: Lang) -> &'static str {
    text(
        match s {
            SessionState::Working => Key::StatusWorking,
            SessionState::Asking => Key::StatusAsking,
            SessionState::Idle => Key::StatusIdle,
            SessionState::Stopped => Key::StatusStopped,
            SessionState::Failed => Key::StatusFailed,
            SessionState::Unknown => Key::StatusUnknown,
        },
        lang,
    )
}

/// 一行一个会话。空列表给一句话，不是一张空表。
///
/// 不对齐成表格：状态词是中文（双宽），项目路径长短差得远，按字符数补空格
/// 在终端里反而歪得更厉害。这是给人一眼扫的，不是给 `awk` 切的——真要切的
/// 话字段之间的双空格也够用。
pub fn render_ps(sessions: &[SessionInfo], lang: Lang) -> String {
    if sessions.is_empty() {
        return text(Key::NoSessionsRunning, lang).into();
    }
    sessions
        .iter()
        .map(|s| {
            let line = format!(
                "{}  {}  {}  {}",
                s.id,
                s.profile,
                status_word(s.state, lang),
                s.dir
            );
            // 「在干什么」可能是空的（比如刚起来、或者已经停了），空的就不拖
            // 一条尾巴出来
            if s.activity.trim().is_empty() {
                line
            } else {
                format!("{line}  {}", s.activity.trim())
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// 连上已经在跑的守护进程。**连不上就是连不上**，不拉起新的——见模块注释。
fn connect(sock: &Path) -> Option<Client> {
    Client::connect(sock).ok()
}

pub fn run_ps(sock: &Path, lang: Lang) -> Result<()> {
    let Some(mut c) = connect(sock) else {
        println!("{}", text(Key::NoDaemonRunning, lang));
        return Ok(());
    };
    match c.call(Request::List)? {
        Response::Sessions(v) => {
            println!("{}", render_ps(&v, lang));
            Ok(())
        }
        other => anyhow::bail!("守护进程答非所问：{other:?}"),
    }
}

/// 返回值是进程退出码：有任何一个会话没停成就是非零。
///
/// 逐个停而不是遇错就停手：用户敲 `dct stop 3 4 5` 的意思是这三个都别跑了，
/// 3 号已经没了不该连累 4、5 还留着。
pub fn run_stop(sock: &Path, lang: Lang, target: Target) -> Result<i32> {
    run_on_targets(sock, lang, target, Force::No)
}

/// 跟 `run_stop` 走同一条路，只把请求换成 `Kill`。
///
/// 共用而不是抄一份：两条命令唯一的差别是发给守护进程的那一个请求，
/// 而**周围那一圈**（`--all` 要先问一遍谁还活着、逐个发不中途放弃、
/// 有一个失败就退非零）全都一样。抄一份的话，将来改退出码只会改对一半。
pub fn run_kill(sock: &Path, lang: Lang, target: Target) -> Result<i32> {
    run_on_targets(sock, lang, target, Force::Yes)
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Force {
    No,
    Yes,
}

fn run_on_targets(sock: &Path, lang: Lang, target: Target, force: Force) -> Result<i32> {
    let target = match target {
        Target::Usage(msg) => {
            eprintln!("{msg}");
            return Ok(2);
        }
        t => t,
    };

    let Some(mut c) = connect(sock) else {
        println!("{}", text(Key::NoDaemonRunning, lang));
        return Ok(0);
    };

    let ids = match target {
        // 「全部」= 所有还没停的。已经停了的不必再停一次，也不该因为
        // 它们而让退出码变成非零。
        Target::All => match c.call(Request::List)? {
            Response::Sessions(v) => v
                .iter()
                .filter(|s| s.state != SessionState::Stopped)
                .map(|s| s.id)
                .collect(),
            other => anyhow::bail!("守护进程答非所问：{other:?}"),
        },
        Target::Ids(v) => v,
        Target::Usage(_) => unreachable!("上面已经处理过了"),
    };

    if ids.is_empty() {
        println!("{}", text(Key::NoSessionsRunning, lang));
        return Ok(0);
    }

    let mut bad = 0;
    for id in ids {
        let req = match force {
            Force::No => Request::Stop { id },
            Force::Yes => Request::Kill { id },
        };
        match c.call(req) {
            Ok(Response::Ok) => println!(
                "{}",
                match force {
                    Force::No => crate::i18n::msg::stopped_session(lang, id),
                    Force::Yes => crate::i18n::msg::killed_session(lang, id),
                }
            ),
            Ok(Response::Error(ref e)) => {
                bad += 1;
                eprintln!("{}", crate::i18n::msg::error(lang, e));
            }
            Ok(other) => {
                bad += 1;
                eprintln!("守护进程答非所问：{other:?}");
            }
            Err(e) => {
                bad += 1;
                eprintln!("{e}");
            }
        }
    }
    Ok(if bad > 0 { 1 } else { 0 })
}

/// `dct restart` 的参数解析结果。
#[derive(Debug, PartialEq, Eq)]
pub enum Restart {
    /// 先问一句再动手
    Ask,
    /// `-y`：不问，直接换
    Yes,
    /// 参数不对
    Usage(String),
}

/// 解析 `dct restart` 后面的参数。认的只有 `-y` / `--yes`。
///
/// 不认识的参数一律是用法错误，**不是「当没看见」**——理由写在
/// `i18n::msg::restart_takes_no_args` 上。
pub fn parse_restart_args(args: &[String], lang: Lang) -> Restart {
    let mut yes = false;
    for a in args {
        match a.as_str() {
            "-y" | "--yes" => yes = true,
            other => return Restart::Usage(crate::i18n::msg::restart_takes_no_args(lang, other)),
        }
    }
    if yes {
        Restart::Yes
    } else {
        Restart::Ask
    }
}

/// `dct restart`：把守护进程换成当前这个二进制，不开界面。
///
/// 为什么要有它：换掉守护进程这条路本来只有一个入口——界面启动时撞上旧版本
/// 弹的那句问话（`main::offer_to_restart_stale_daemon`）。而「我刚 `cargo
/// build` 完，想让后台跑上新的」跟「版本号对不上」是两件事：前者版本可能一样
/// （同一个 commit 改了个字符串又编了一遍），根本触发不了那句问话，用户只剩
/// `pkill -f "dct daemon"` 可用——而那条路认的是进程不是会话，跟本模块开头
/// 说的是同一个问题。
///
/// **它跟 `ps`/`stop` 一样不会拉起守护进程。** 本来没东西在跑的时候，
/// `restart` 想要的那个东西（换掉在跑的那个）压根不存在；顺手起一个等于
/// 把「重启」偷偷变成「启动」，而启动是 `dct` 自己的事。
///
/// 返回值是退出码：换成了 0，参数不对 2，没换成 1。**「本来就没东西在跑」
/// 是 0**，跟 `ps` 同一条规矩：那是个正常答案，不是错误。
pub fn run_restart(sock: &Path, exe: &Path, lang: Lang, args: Restart) -> Result<i32> {
    let ask = match args {
        Restart::Usage(msg) => {
            eprintln!("{msg}");
            return Ok(2);
        }
        Restart::Ask => true,
        Restart::Yes => false,
    };

    let Some(mut c) = connect(sock) else {
        println!("{}", text(Key::NoDaemonRunning, lang));
        return Ok(0);
    };

    if ask {
        // 先把要被断掉的东西摆出来再问。「会断掉正在跑的会话」是一句抽象的
        // 话，而「3 号 claude 正在 ~/proj 里干活」是用户真正要衡量的代价。
        if let Ok(Response::Sessions(v)) = c.call(Request::List) {
            let live: Vec<SessionInfo> = v
                .into_iter()
                .filter(|s| s.state != SessionState::Stopped)
                .collect();
            if !live.is_empty() {
                println!("{}", render_ps(&live, lang));
            }
        }
        println!("{}", text(Key::RestartExplain, lang));
        print!("{} ", text(Key::RestartAsk, lang));
        let _ = std::io::Write::flush(&mut std::io::stdout());

        // stdin 读不到（脚本、cron、`< /dev/null`）就当没答应。这条命令会
        // 杀掉所有正在跑的 agent，没人在场的时候**默认不动**才是对的——
        // 无人值守要重启，请明写 `-y`。
        let mut answer = String::new();
        let said_yes = std::io::BufRead::read_line(&mut std::io::stdin().lock(), &mut answer)
            .is_ok()
            && answer.trim().eq_ignore_ascii_case("y");
        if !said_yes {
            println!("{}", text(Key::RestartCancelled, lang));
            return Ok(0);
        }
    }

    // 手里这条连接连着的正是马上要被杀掉的那个进程，先丢掉。
    drop(c);
    println!("{}", text(Key::StaleDaemonRestarting, lang));
    match crate::client::restart_daemon(sock, exe) {
        Ok(()) => {
            println!("{}", text(Key::RestartDone, lang));
            Ok(0)
        }
        Err(_) => {
            eprintln!("{}", text(Key::RestartFailed, lang));
            Ok(1)
        }
    }
}

/// `dct prune`：把已经停掉的会话从名册上抹掉。
///
/// 不接参数、也没有 `--all`：这条命令本来就只对「已经停了的」下手，
/// 而那批东西不可能被误伤——它们已经不跑了。
pub fn run_prune(sock: &Path, lang: Lang) -> Result<()> {
    let Some(mut c) = connect(sock) else {
        println!("{}", text(Key::NoDaemonRunning, lang));
        return Ok(());
    };
    match c.call(Request::Prune)? {
        // 一个都没清跟清了几个是两句话：印「清掉 0 个」会让人以为
        // 命令没生效，而事实是本来就没有可清的。
        Response::Pruned(0) => println!("{}", text(Key::NothingToPrune, lang)),
        Response::Pruned(n) => println!("{}", crate::i18n::msg::pruned(lang, n)),
        other => anyhow::bail!("守护进程答非所问：{other:?}"),
    }
    Ok(())
}

/// 一个 provider 只能拿到**它自己厂商**的 OAuth，绝不能拿别家的。
///
/// CRITICAL（见 review）：这里曾经把 kimi / glm / deepseek / qwen-api 也映射
/// 到 `read_claude_oauth()`，而 `send_real`（`src/llm/http.rs`）会把凭据
/// 塞进 `Authorization: Bearer` 头直接打给这些 profile 自己的 `[api].base_url`
/// （api.moonshot.cn / open.bigmodel.cn / api.deepseek.com /
/// dashscope.aliyuncs.com）——等于把用户的 Anthropic 登录态发给了四家
/// 跟 Anthropic 毫无关系的第三方服务器。claude 本身又没有 `[api]` 块
/// （它走官方端点，靠 CLI 自己登录），所以那个分支唯一能真正走到的效果
/// 就是把 token 发给别家。
///
/// 规则钉死：**一个 CLI 的 OAuth 只能给它自己的端点用。** kimi/glm/
/// deepseek/qwen-api 跟用户没有任何 OAuth 关系，只能走用户自己填的 key
/// （`resolve::resolve` 里 key 优先于 OAuth 那条顺序保证了这一点）。
/// 不要再把它们加回 claude 或 codex 的分支。
///
/// **按名字挑只是第一道关。** 名字是用户可以手写的（
/// `~/.dct/profiles/claude.toml` 里塞一个 `[api]`、设置文件里写一行
/// `base_url`），所以每份凭据都带着**出处**（`BorrowedFrom`）一起返回，
/// 由 `resolve::select_credential` 拿它去比对**目的地主机**——那才是凭据
/// 真正会去的地方。两道关一起才关得住这一类问题。
///
/// `claude`/`codex` 两个闭包注入是为了测试不用碰真实 Keychain / `auth.json`。
fn oauth_lookup(
    name: &str,
    claude: &dyn Fn() -> Option<crate::llm::creds::Borrowed>,
    codex: &dyn Fn() -> Option<crate::llm::creds::Borrowed>,
) -> Option<crate::llm::creds::Borrowed> {
    match name {
        "claude" => claude(),
        "codex" => codex(),
        _ => None,
    }
}

/// `dct llm check`：把配置里那条 LLM 连接真的跑一次。
///
/// 这条命令**就是**「配置写完还要真打端点验过」那条验收标准的载体。
///
/// `lang` 跟 `ps`/`stop`/`prune` 同一条路（`main::cli_lang()`）：这条命令印的
/// 也是给人看的话，跟界面说两种语言会很怪。
pub fn llm_check(lang: Lang) -> i32 {
    let socket = crate::proto::socket_path();
    let config_path = crate::config::config_path_for_socket(&socket);
    let cfg = crate::config::Config::load(&config_path);
    let secrets =
        crate::secrets::SecretStore::load(&crate::secrets::secrets_path_for_socket(&socket));
    let profiles_dir = crate::profile::profiles_dir_for_socket(&socket);
    let (custom, _) = crate::profile::all_profiles(&profiles_dir);
    let lookup = |n: &str| {
        custom
            .iter()
            .find(|p| p.name == n)
            .cloned()
            .or_else(|| crate::profile::Profile::builtin(n))
    };
    let oauth = |n: &str| {
        use crate::llm::creds::{BorrowedFrom, Credential};
        oauth_lookup(
            n,
            &|| {
                crate::llm::creds::read_claude_oauth()
                    .map(|t| (BorrowedFrom::ClaudeCli, Credential::Bearer(t)))
            },
            &|| crate::llm::creds::read_codex_auth().map(|c| (BorrowedFrom::CodexCli, c)),
        )
    };

    // 没写 `[llm]` 就是没开——这是绝大多数用户的正常状态，不是「配置不对」。
    // 见 `config.rs` 头注释：出错解释会把终端里的原始内容送给模型，必须是
    // 用户自己主动写下 `[llm]` 才算数，这里不能替他去猜一份默认配置来验。
    let Some(llm) = &cfg.llm else {
        // 路径是真的从 socket 推出来的那一个，不是一句「设置文件」——
        // 零编程经验的用户没法对「设置文件」这四个字采取任何行动。
        println!("{}", crate::i18n::msg::llm_not_enabled(lang, &config_path));
        return 1;
    };

    println!(
        "{}",
        crate::i18n::msg::llm_using(
            lang,
            &llm.provider,
            llm.transport == crate::config::Transport::Http
        )
    );

    let backend = match crate::llm::resolve::resolve(llm, &lookup, &secrets, &oauth) {
        Ok(b) => b,
        Err(e) => {
            println!(
                "{}",
                crate::i18n::msg::llm_cannot_connect(
                    lang,
                    &crate::i18n::msg::llm_problem(lang, &e)
                )
            );
            return 1;
        }
    };

    let p = crate::llm::Prompt {
        system: "你是一个只回答一个词的助手。".into(),
        user: "回答「好」这一个字，不要别的。".into(),
        max_tokens: 16,
    };
    match crate::llm::complete_with_timeout(backend, p, std::time::Duration::from_secs(60)) {
        Ok(answer) => {
            println!("{}", crate::i18n::msg::llm_works(lang, &answer));
            0
        }
        Err(e) => {
            println!("{}", crate::i18n::msg::llm_call_failed(lang, e));
            1
        }
    }
}

/// 把下载进度印在终端上。**百分比压在同一行**：这条命令是在一个 PTY
/// 会话里跑给学生看的，一行一个百分比会把几十屏刷过去，而他真正要看的
/// 是这一行之前和之后的那两句话。
#[derive(Default)]
struct TermProgress {
    /// 上一次印出去的百分比。
    ///
    /// 不记这个的话，每收一块（64 KB）就印一次——50 MB 就是八百多次。
    /// 在终端里靠 `\r` 盖掉还看得过去，但这条命令的输出会被重定向
    /// （日志、抓屏工具、`dct install claude > out.txt`），那时候 `\r`
    /// 不再是「回到行首」，八百个百分比会原样堆成几十 KB。只在整数
    /// 百分比真的变了的时候印，上限一百次。
    last: std::cell::Cell<u64>,
}

impl crate::runtime::Progress for TermProgress {
    fn line(&self, text: &str) {
        println!("{text}");
        let _ = std::io::Write::flush(&mut std::io::stdout());
    }

    fn percent(&self, done: u64, total: Option<u64>) {
        // 对面没给 Content-Length 时不要编一个百分比出来，改报已下多少。
        // 编一个假的进度条比承认不知道更糟——那条永远走不到头的进度条
        // 会让人一直等下去。
        let (step, unit) = match total {
            Some(t) if t > 0 => (done * 100 / t, "%"),
            // 对面没给 Content-Length 时按 MB 报，每涨一 MB 说一次。
            _ => (done / (1024 * 1024), " MB"),
        };
        if step == self.last.get() {
            return;
        }
        self.last.set(step);
        print!("\r  {step}{unit}   ");
        let _ = std::io::Write::flush(&mut std::io::stdout());
    }

    fn done(&self) {
        println!();
    }
}

/// 把 `FetchError` 翻成一句给人看的话。
///
/// 网络类的失败后面补一句镜像提示：那是这类失败里唯一一条学生自己走得通
/// 的路，而它需要两个他不可能猜到的地址。校验和对不上和磁盘写不进不补，
/// 换镜像对那两种情况没有帮助，多印一段只会让人往错的方向使劲。
fn fetch_problem(lang: Lang, e: &crate::runtime::FetchError) -> String {
    use crate::runtime::FetchError as F;
    let hint = || {
        crate::i18n::msg::mirror_hint(
            lang,
            crate::runtime::CN_NODE_BASE,
            crate::runtime::CN_NPM_REGISTRY,
        )
    };
    match e {
        F::Unreachable { url } => format!(
            "{}

{}",
            crate::i18n::msg::download_unreachable(lang, url),
            hint()
        ),
        F::Corrupt => crate::i18n::msg::download_corrupt(lang),
        F::NoAssetForPlatform => crate::i18n::msg::no_node_for_platform(lang),
        F::CannotUnpack => crate::i18n::msg::cannot_unpack(lang),
        F::CannotWrite => crate::i18n::msg::cannot_write_runtime(lang, "~/.dct"),
    }
}

/// npm 该去哪个仓库拿包。国内课堂设 `DCT_NPM_REGISTRY`，别处不设就是官方。
///
/// 跟 `DCT_NODE_BASE` 分成两个变量而不是一个「中国模式」开关：这两件事
/// 会分别失效（镜像站可能只镜像了其中一样），而一个开关同时管两样的话，
/// 出问题时没法只换一半。
fn npm_registry() -> Option<String> {
    std::env::var("DCT_NPM_REGISTRY")
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

/// `dct install <agent>`：把一个 agent 装到能用为止。
///
/// **这条命令存在的理由，是把「学生看到的第一句英文报错」换掉。** 在它
/// 之前，选中一个没装的 agent 会在一个 shell 会话里跑 `npm i -g …`，
/// 而一台只装了 dct 的电脑上没有 npm，于是学生得到的是
/// 「npm 不是内部或外部命令」——一句操作系统的原话，既不说缺什么，
/// 也不说下一步该干嘛。
///
/// 现在这一行由 dct 自己走完：缺运行时就先下一份自带的，再装 agent，
/// **最后真的再查一遍那个命令是不是找得到了**。最后这一步不能省：
/// 「npm 说成功了」和「敲得出这个命令了」不是同一件事——npm 装到别的
/// prefix 去、或者包本身没带 bin，都会长成「成功但没有」。
pub fn run_install(name: &str, lang: Lang) -> i32 {
    let socket = crate::proto::socket_path();
    let runtime = crate::runtime::runtime_dir_for_socket(&socket);
    let profiles_dir = crate::profile::profiles_dir_for_socket(&socket);
    let (custom, _) = crate::profile::all_profiles(&profiles_dir);

    let Some(p) = custom
        .iter()
        .find(|p| p.name == name)
        .cloned()
        .or_else(|| crate::profile::Profile::builtin(name))
    else {
        eprintln!("{}", crate::i18n::msg::unknown_agent(lang, name));
        return 2;
    };

    // 先把自带的运行时挂上，再问任何「装没装」的问题。顺序反了的话，
    // 上一次装好的 agent 会被报成没装——它就在自带运行时那个目录里。
    crate::runtime::activate(&runtime);

    let label = p.display_label(lang);
    let Some(cmd0) = p.command.first().cloned() else {
        eprintln!("{}", crate::i18n::msg::agent_has_no_installer(lang, &label));
        return 1;
    };

    if crate::profile::command_exists(&cmd0) {
        println!(
            "{}",
            crate::i18n::msg::agent_already_installed(lang, &label)
        );
        return 0;
    }

    let Some(spec) = p.install.clone() else {
        eprintln!("{}", crate::i18n::msg::agent_has_no_installer(lang, &label));
        return 1;
    };

    let mut argv = spec.command.clone();
    if argv.is_empty() {
        eprintln!("{}", crate::i18n::msg::agent_has_no_installer(lang, &label));
        return 1;
    }

    // 只有 npm 那条路要运行时。将来有人写一个 `[install]` 跑别的东西
    // （brew、pip、winget），不该被拖去下一份 Node。
    let uses_npm = argv[0] == "npm" || argv[0] == "npx";
    if uses_npm && !crate::profile::command_exists("npm") {
        if let Err(e) = crate::runtime::ensure_node(&runtime, lang, &TermProgress::default()) {
            eprintln!("{}", fetch_problem(lang, &e));
            return 1;
        }
        println!("{}", crate::i18n::msg::node_ready(lang));
    }

    if uses_npm {
        if let Some(reg) = npm_registry() {
            argv.push("--registry".into());
            argv.push(reg);
        }
    }

    println!("{}", crate::i18n::msg::installing_agent(lang, &label));

    // Windows 上 npm 装出来的是 `.cmd`，而 CreateProcess 只启动真正的
    // 可执行映像——`launch_argv` 就是为这件事存在的（见 `sys::shell`）。
    let argv = crate::sys::shell::launch_argv(&argv);
    let ran = std::process::Command::new(&argv[0])
        .args(&argv[1..])
        .status();

    match ran {
        Ok(st) if st.success() => {}
        _ => {
            eprintln!("{}", crate::i18n::msg::install_failed(lang, &label));
            return 1;
        }
    }

    // 装完再查一遍。见函数头注释：这一步是这条命令跟直接敲 npm 的全部差别。
    if crate::profile::command_exists(&cmd0) {
        println!("{}", crate::i18n::msg::install_succeeded(lang, &label));
        0
    } else {
        eprintln!(
            "{}",
            crate::i18n::msg::install_finished_but_missing(lang, &cmd0)
        );
        1
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    /// `dct restart` 的参数：只认 `-y` / `--yes`，别的一律用法错误。
    ///
    /// 钉死「不认识的参数不等于没参数」：`dct restart --all` 要是被当成裸
    /// `dct restart` 跑，用户以为自己限定了范围，实际连所有会话一起换掉。
    #[test]
    fn restart_only_accepts_the_yes_flag() {
        let lang = Lang::Zh;
        assert_eq!(parse_restart_args(&args(&[]), lang), Restart::Ask);
        assert_eq!(parse_restart_args(&args(&["-y"]), lang), Restart::Yes);
        assert_eq!(parse_restart_args(&args(&["--yes"]), lang), Restart::Yes);
        assert!(matches!(
            parse_restart_args(&args(&["--all"]), lang),
            Restart::Usage(_)
        ));
        assert!(matches!(
            parse_restart_args(&args(&["3"]), lang),
            Restart::Usage(_)
        ));
        // `-y` 混着不认识的参数也是用法错误：不许因为看见了 `-y` 就把
        // 后面那个看不懂的东西忽略掉。
        assert!(matches!(
            parse_restart_args(&args(&["-y", "--all"]), lang),
            Restart::Usage(_)
        ));
    }

    /// CRITICAL fix pin: `oauth_lookup` 曾经把 kimi/glm/deepseek/qwen-api
    /// 也映射到 claude 的 OAuth，等于把用户的 Anthropic 登录态发给了四家
    /// 毫不相关的第三方服务器（见 `oauth_lookup` 上的注释）。这里钉死
    /// 只有 claude 拿得到 claude 自己的 token、codex 拿得到 codex 自己的
    /// token，其余名字一律 `None`——不管注入的 `claude`/`codex` 闭包
    /// 返回什么。两个闭包都是假的，测试不碰真实 Keychain / `auth.json`。
    #[test]
    fn oauth_lookup_never_offers_one_vendors_token_to_another() {
        use crate::llm::creds::{BorrowedFrom, Credential};

        let claude = || {
            Some((
                BorrowedFrom::ClaudeCli,
                Credential::Bearer("claude-token".into()),
            ))
        };
        let codex = || {
            Some((
                BorrowedFrom::CodexCli,
                Credential::Bearer("codex-token".into()),
            ))
        };

        assert_eq!(
            oauth_lookup("claude", &claude, &codex),
            Some((
                BorrowedFrom::ClaudeCli,
                Credential::Bearer("claude-token".into())
            ))
        );
        assert_eq!(
            oauth_lookup("codex", &claude, &codex),
            Some((
                BorrowedFrom::CodexCli,
                Credential::Bearer("codex-token".into())
            ))
        );
        for vendor in ["kimi", "glm", "deepseek", "qwen-api"] {
            assert_eq!(
                oauth_lookup(vendor, &claude, &codex),
                None,
                "{vendor} 没有自己的 OAuth 关系，不该拿到别家的凭据"
            );
        }
    }

    /// **不给参数不能等于全停。** `dct stop` 是最容易手滑敲出来的形式，
    /// 而停会话撤不回来——默认全停的话，用户想停一个会把所有 agent 停光。
    #[test]
    fn a_bare_stop_asks_what_to_stop_instead_of_stopping_everything() {
        match parse_target_args(&args(&[]), Lang::Zh, "stop") {
            Target::Usage(m) => assert!(!m.trim().is_empty(), "得说清该怎么用"),
            other => panic!("空参数必须是用法提示，实际 {other:?}"),
        }
    }

    /// `kill` 更凶（不给收尾时间），同一条规矩更要成立。
    #[test]
    fn a_bare_kill_asks_what_to_kill_instead_of_killing_everything() {
        match parse_target_args(&args(&[]), Lang::Zh, "kill") {
            Target::Usage(m) => assert!(!m.trim().is_empty(), "得说清该怎么用"),
            other => panic!("空参数必须是用法提示，实际 {other:?}"),
        }
    }

    /// 用户敲的是哪条命令，用法提示里就得印哪条。印着 `dct stop 3` 去回答
    /// 一个敲了 `dct kill` 的人，等于把他推去解一个他没问的问题。
    #[test]
    fn the_usage_hint_names_the_command_you_actually_typed() {
        for (cmd, other) in [("kill", "stop"), ("stop", "kill")] {
            for bad in [args(&[]), args(&["--all", "3"])] {
                match parse_target_args(&bad, Lang::Zh, cmd) {
                    Target::Usage(m) => {
                        assert!(m.contains(&format!("dct {cmd}")), "提示里该印 {cmd}：{m}");
                        assert!(!m.contains(&format!("dct {other}")), "不该印 {other}：{m}");
                    }
                    other => panic!("该是用法提示，实际 {other:?}"),
                }
            }
        }
    }

    #[test]
    fn ids_are_parsed_in_the_order_given() {
        assert_eq!(
            parse_target_args(&args(&["3", "4", "5"]), Lang::Zh, "stop"),
            Target::Ids(vec![3, 4, 5])
        );
    }

    #[test]
    fn all_means_all() {
        assert_eq!(
            parse_target_args(&args(&["--all"]), Lang::Zh, "stop"),
            Target::All
        );
    }

    /// `--all` 和具体 id 混着给，说明用户自己也没想清楚。猜一个的代价是
    /// 停掉了他没打算停的东西。
    #[test]
    fn all_mixed_with_ids_is_refused_rather_than_guessed() {
        match parse_target_args(&args(&["--all", "3"]), Lang::Zh, "stop") {
            Target::Usage(_) => {}
            other => panic!("混着给必须拒绝，实际 {other:?}"),
        }
    }

    #[test]
    fn kill_all_takes_no_ids() {
        match parse_target_args(&args(&["--all", "3"]), Lang::Zh, "kill") {
            Target::Usage(m) => assert!(m.contains("kill"), "{m}"),
            other => panic!("混着给必须拒绝，实际 {other:?}"),
        }
    }

    #[test]
    fn a_non_number_says_so_instead_of_being_skipped() {
        match parse_target_args(&args(&["claude"]), Lang::Zh, "stop") {
            Target::Usage(m) => assert!(m.contains("claude"), "得点名是哪个参数不对：{m}"),
            other => panic!("非数字必须报错，实际 {other:?}"),
        }
    }

    fn s(id: u32, state: SessionState, activity: &str) -> SessionInfo {
        SessionInfo {
            id,
            profile: "claude".into(),
            dir: "/w/dc-terminal".into(),
            state,
            activity: activity.into(),
            is_agent: true,
            tag: String::new(),
        }
    }

    #[test]
    fn ps_lists_one_session_per_line_with_its_state() {
        let out = render_ps(
            &[
                s(1, SessionState::Working, "正在读 src/main.rs"),
                s(2, SessionState::Stopped, ""),
            ],
            Lang::Zh,
        );
        let lines: Vec<&str> = out.lines().collect();
        assert_eq!(lines.len(), 2, "一行一个会话：{out}");
        assert!(lines[0].contains('1') && lines[0].contains("干活中"));
        assert!(lines[0].contains("正在读 src/main.rs"));
        assert!(lines[1].contains("已停止"));
    }

    /// 没有会话时给一句话。打印一张只有表头的空表，用户会以为命令坏了。
    #[test]
    fn ps_with_nothing_running_says_so_in_words() {
        let out = render_ps(&[], Lang::Zh);
        assert!(!out.trim().is_empty());
        assert!(!out.contains('\n'), "一句话就够：{out}");
    }

    /// 「在干什么」是空的时候不要拖一条空尾巴——行尾多出来的空格在终端里
    /// 看不见，复制粘贴时才发现。
    #[test]
    fn an_empty_activity_does_not_leave_trailing_space() {
        let out = render_ps(&[s(1, SessionState::Stopped, "")], Lang::Zh);
        assert_eq!(out, out.trim_end(), "行尾不该有空格：{out:?}");
    }
}
