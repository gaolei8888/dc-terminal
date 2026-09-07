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

/// `run_restart` 干完之后，还剩什么要 `main` 接着做。
#[derive(Debug, PartialEq, Eq)]
pub enum Restarted {
    /// 事情办完了（或者压根没往下走），按这个退出码退出。
    Exit(i32),
    /// 后台本来就空着，没有「旧的」可换。剩下的活是**启动**，而启动要开
    /// 界面、要问「接不接回上次的会话」——那些都长在 `main::run_ui` 上，
    /// 这里只把球传回去。
    StartFresh,
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
/// **后台空着的时候它会起一个**，跟 `ps`/`stop` 那条「绝不顺手拉起」的规矩
/// 不一样，这是故意的：`ps` 问的是「有没有东西在跑」，把「没有」变成「有」
/// 是答非所问；而 `restart` 是一句祈使句——用户要的是「敲完这行，后台跑着
/// 新的」，后台此刻空不空是过程细节，不是他要的答案。拿这个当「重启不是
/// 启动」拦下来，用户只会照着提示再敲一次 `dct`，那一步不给他任何东西。
///
/// 返回 `StartFresh` 的时候**这里一句话都还没干**：连守护进程都没拉，因为
/// 拉起之前还有一句「要不要接回上次的会话」要问（`main::run_ui`）。
pub fn run_restart(sock: &Path, exe: &Path, lang: Lang, args: Restart) -> Result<Restarted> {
    let ask = match args {
        Restart::Usage(msg) => {
            eprintln!("{msg}");
            return Ok(Restarted::Exit(2));
        }
        Restart::Ask => true,
        Restart::Yes => false,
    };

    let Some(mut c) = connect(sock) else {
        // 没有旧的可换，也就没有「会断掉正在跑的会话」这个代价——所以这条
        // 路**不问 y/n**，`-y` 在这里没有意义：要问的那句话（接不接回上次
        // 的会话）在 `run_ui` 里，那是另一个问题。
        println!("{}", text(Key::RestartNothingToRestart, lang));
        return Ok(Restarted::StartFresh);
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
            return Ok(Restarted::Exit(0));
        }
    }

    // 手里这条连接连着的正是马上要被杀掉的那个进程，先丢掉。
    drop(c);
    println!("{}", text(Key::StaleDaemonRestarting, lang));
    match crate::client::restart_daemon(sock, exe) {
        Ok(()) => {
            println!("{}", text(Key::RestartDone, lang));
            Ok(Restarted::Exit(0))
        }
        Err(_) => {
            eprintln!("{}", text(Key::RestartFailed, lang));
            Ok(Restarted::Exit(1))
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
/// `dct install git`：把那份便携 git 装到能用为止。
///
/// 跟 `run_install` 装 agent 那条路一样，**最后真的再查一遍**：
/// 「解压说成功了」和「敲得出 git 了」不是同一件事。
fn run_install_git(runtime: &std::path::Path, lang: Lang) -> i32 {
    // 先把自带的运行时挂上再问「装没装」——顺序反了的话，上一次装好的
    // 那份 git 会被报成没装，它就在 `~/.dct/runtime/git` 里。
    crate::runtime::activate(runtime);

    if crate::git::available() {
        println!("{}", crate::i18n::msg::git_already_installed(lang));
        return 0;
    }

    if let Err(e) = crate::runtime::ensure_git(runtime, lang, &TermProgress::default()) {
        eprintln!("{}", git_fetch_problem(lang, &e));
        return 1;
    }

    if crate::git::available() {
        println!("{}", crate::i18n::msg::git_ready(lang));
        0
    } else {
        // 装完了却还是跑不起来。极少见，但不能报成成功——那会让下一步
        // 失败在一句跟原因对不上的话上。
        eprintln!(
            "{}",
            crate::i18n::msg::install_finished_but_missing(lang, "git")
        );
        1
    }
}

/// 同 `fetch_problem`，但「这个平台没有现成的包」那一支说的是 git 的事。
///
/// 不复用那一支：它印的是「自己装一份 Node」，而走到这里的用户缺的是
/// git，照着那句话做解决不了他的问题。
fn git_fetch_problem(lang: Lang, e: &crate::runtime::FetchError) -> String {
    use crate::runtime::FetchError as F;
    match e {
        F::NoAssetForPlatform => crate::i18n::msg::git_missing_install_it_yourself(lang),
        // 下不到时给的镜像变量也得是 git 那个，不是 Node 那两个。
        F::Unreachable { url } => format!(
            "{}\n\n{}",
            crate::i18n::msg::download_unreachable(lang, url),
            crate::i18n::msg::git_mirror_hint(lang)
        ),
        other => fetch_problem(lang, other),
    }
}

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

/// `dct install <agent> [--update]` 的参数解析结果。
#[derive(Debug)]
pub enum InstallArgs {
    /// 装上（没装才装，装了就说一声）
    Install(String),
    /// 升到最新版
    Update(String),
    /// 参数不对，把该说的话说清楚
    Usage(String),
}

/// 解析 `dct install` 后面的参数。认的只有一个 `--update`。
///
/// 跟 `parse_restart_args` 一个脾气：**不认识的参数是用法错误，不是
/// 「当没看见」**。`dct install claude --dry-run` 要是被当成裸
/// `dct install claude` 跑，用户以为自己什么都没动，实际真装下去了。
/// 同一个开关给两遍、给两个名字，都归到同一条路上——那都说明用户对
/// 自己敲了什么没有把握，猜一个不如让他重敲。
pub fn parse_install_args(args: &[String], lang: Lang) -> InstallArgs {
    let mut name: Option<String> = None;
    let mut update = false;
    for a in args {
        match a.as_str() {
            "--update" if !update => update = true,
            s if !s.starts_with('-') && name.is_none() => name = Some(s.to_string()),
            _ => return InstallArgs::Usage(crate::i18n::msg::install_usage(lang)),
        }
    }
    match (name, update) {
        (Some(n), false) => InstallArgs::Install(n),
        (Some(n), true) => InstallArgs::Update(n),
        (None, _) => InstallArgs::Usage(crate::i18n::msg::install_usage(lang)),
    }
}

/// 这个 profile 有几个还活着的会话。
///
/// `Stopped` 不算——进程已经没了，底下换文件伤不到它。不在名单里的 profile
/// 也不算：更新 claude 的时候 codex 在跑，跟这件事无关，拦下来只会让用户
/// 去关一个他根本不必关的东西。
///
/// 收的是**一串**名字而不是一个，因为被换掉的是**包**不是 profile：
/// `dc` 和 `qwen` 跑的是同一个二进制、同一个 npm 包（见 `sibling_profiles`）。
///
/// **回的是每个会话的 profile 名，不是个数。** 长度就是「有几个」，去重之后
/// 就是「都是谁」——拒绝那句话两样都要：只报个数的话，`dct install kimi
/// --update` 被两个 claude 会话拦住时只能说「2 个 Kimi 会话」，而一个 Kimi
/// 会话都没有，用户会去找两个不存在的东西。
pub(crate) fn live_sessions_of(sessions: &[SessionInfo], profiles: &[String]) -> Vec<String> {
    sessions
        .iter()
        .filter(|s| profiles.iter().any(|n| n == &s.profile) && s.state != SessionState::Stopped)
        .map(|s| s.profile.clone())
        .collect()
}

/// 把一串 profile 名翻成菜单上那些名字，**按出现顺序去重**。
///
/// 拒绝那句话里要列的就是它。认不出的名字原样留着：那说明会话是一个已经被
/// 删掉的自定义 profile 起的，印它的原名总比印一个空括号强。
pub(crate) fn distinct_labels(
    all: &[crate::profile::Profile],
    profile_names: &[String],
    lang: Lang,
) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for name in profile_names {
        let label = all
            .iter()
            .find(|p| &p.name == name)
            .map(|p| p.display_label(lang))
            .unwrap_or_else(|| name.clone());
        if !out.contains(&label) {
            out.push(label);
        }
    }
    out
}

/// 跟 `cmd0` 跑同一条命令的 profile 有哪些（包含它自己）。
///
/// 这是「更新会动到谁」的正确判据。`npm i -g <pkg>` 换掉的是磁盘上那个包，
/// 而好几个 profile 可能共用它：`kimi`/`glm`/`deepseek`/`qwen-api` 的
/// `command[0]` 都是 `claude`，`dc` 的是 `qwen`。只按 profile 名数会话的话，
/// 更新 `qwen` 时一个在跑的 `dc` 会话拦不住，而它那个 node 进程底下的文件
/// 正是要被换掉的那批。
///
/// **不写死配对**，从传进来的 profile 全集里算——哪天有人改了 toml、加了
/// 新的，这里自动跟上，不用有人记得回来改一份清单。
pub(crate) fn sibling_profiles(all: &[crate::profile::Profile], cmd0: &str) -> Vec<String> {
    all.iter()
        .filter(|p| p.command.first().map(String::as_str) == Some(cmd0))
        .map(|p| p.name.clone())
        .collect()
}

/// 有会话在跑就给出该说的那句话，没有就是 `None`（可以往下更新）。
///
/// **整段判断都在这儿，`run_update` 里不留任何拼装。** 这个函数**拿不到**
/// 被更新那个 agent 的名字——那正是重点：句子里该出现的是那些会话自己的
/// 名字，而只要 `label` 还递得进来，就总有人会顺手用它，于是又变回
/// 「先关掉 2 个 Kimi 会话」这种指向不存在东西的话。递不进来就写不出来。
pub(crate) fn refusal_for_running_sessions(
    all: &[crate::profile::Profile],
    sessions: &[SessionInfo],
    family: &[String],
    lang: Lang,
) -> Option<String> {
    let running = live_sessions_of(sessions, family);
    if running.is_empty() {
        return None;
    }
    let who = distinct_labels(all, &running, lang);
    Some(crate::i18n::msg::update_refused_sessions_running(
        lang,
        running.len(),
        &who,
    ))
}

/// 问守护进程要会话列表的结果。
#[derive(Debug)]
pub(crate) enum Sessions {
    /// 问出来了。空列表是一个**确定的答案**（守护进程没起 = 确实没有会话）。
    Known(Vec<SessionInfo>),
    /// 守护进程在，但这句话它答不上来。**有没有会话是未知的**。
    Unusable,
}

/// 把 `List` 的结果翻成「能不能动手」。抽成纯函数是因为这是条安全边界：
/// 判错的代价是在一堆活着的进程底下换文件，而它不该只能靠真守护进程测。
///
/// **答不上来 ≠ 没有会话。** `Client::connect` 只是连上 socket，**没有任何
/// 版本握手**（见 `client::Client::connect`），所以对面是个旧守护进程时
/// 连得上、`List` 才失败——回一个解析不了的 `SessionInfo`，或者干脆答非
/// 所问。那种失败必须拦住更新。
///
/// **旧守护进程不是唯一的成因**：一个好端端的守护进程正忙着，这条连接
/// 5 秒读超时（`client.rs`）也落到这里。两种都是「不知道」，拦法一样，
/// 但对用户说的那句话不能咬定是哪一种——见
/// `msg::cannot_confirm_open_sessions`。
pub(crate) fn sessions_from(r: Result<Response>) -> Sessions {
    match r {
        Ok(Response::Sessions(v)) => Sessions::Known(v),
        _ => Sessions::Unusable,
    }
}

/// 问守护进程要会话列表。
///
/// **连不上（守护进程没起）→ 空列表**：没有守护进程就没有会话，这不是
/// 「查不到」而是「确实没有」。把这一种也拦住，等于让一个刚开机的人先去
/// 起一个他不需要的后台进程。连上了却答不出来是另一回事，见 `sessions_from`。
fn sessions_now(socket: &Path) -> Sessions {
    match Client::connect(socket) {
        Err(_) => Sessions::Known(Vec::new()),
        Ok(mut c) => sessions_from(c.call(Request::List)),
    }
}

/// 装完（或更新完）告诉守护进程重查版本。
///
/// 发不出去不算错——守护进程没起的话，它起来 30 秒后自己会查。没有这一句
/// 的话，装完之后界面上那条「有新版本」还会挂着，要等一天才消失。
///
/// **重查全部，不是只查刚动过的那个**：换掉一个包会让共用它的那几个
/// profile 一起变（`dc` 和 `qwen`），只报刚敲的那个名字的话，另一个的
/// 提示还挂着。一次全量扫描很便宜，而且永远是对的。
fn recheck(socket: &Path) {
    if let Ok(mut c) = Client::connect(socket) {
        let _ = c.call(Request::RecheckUpdates { profile: None });
    }
}

/// 跑一条 `npm i -g …`：缺 npm 先补一份自带的 Node，追加镜像，Windows 上
/// 经过 `launch_argv`（npm 装出来的是 `.cmd`，见 `sys::shell`）。
///
/// `banner` 是补完 Node、真正动手之前对用户说的那句话（「正在装 X」/
/// 「正在更新 X」）。**由这里印而不是调用方印**，为的是顺序：先说运行时
/// 准备好了，再说开始装——反过来的话，用户会先看到「正在装 Claude Code」，
/// 然后卡在一段跟 Claude Code 无关的下载上。
fn run_npm_global(mut argv: Vec<String>, runtime: &Path, lang: Lang, banner: &str) -> NpmRun {
    if !crate::profile::command_exists("npm") {
        if let Err(e) = crate::runtime::ensure_node(runtime, lang, &TermProgress::default()) {
            eprintln!("{}", fetch_problem(lang, &e));
            return NpmRun::NoNpm;
        }
        println!("{}", crate::i18n::msg::node_ready(lang));
    }

    if let Some(reg) = crate::update::npm_registry_override() {
        argv.push("--registry".into());
        argv.push(reg);
    }

    if spawn_status(&argv, banner) {
        NpmRun::Ok
    } else {
        NpmRun::Failed
    }
}

/// 印一句「接下来这一段是它自己在说话」，然后跑，回它成没成。
///
/// Windows 上 npm 装出来的是 `.cmd`，而 CreateProcess 只启动真正的可执行
/// 映像——`launch_argv` 就是为这件事存在的（见 `sys::shell`）。
fn spawn_status(argv: &[String], banner: &str) -> bool {
    println!("{banner}");
    let argv = crate::sys::shell::launch_argv(argv);
    std::process::Command::new(&argv[0])
        .args(&argv[1..])
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// `run_npm_global` 的三种结局。**「失败」和「压根没跑成」得分开**：
/// 前者后面该接一句「上面那几行是安装程序自己说的话」（`install_failed`），
/// 而后者上面那几行是 dct 自己在说下载运行时出了什么事——对它补那一句
/// 等于把用户指向一段根本不存在的安装程序输出。
enum NpmRun {
    Ok,
    Failed,
    /// 连 npm 都没弄到，为什么已经印过了。
    NoNpm,
}

/// `dct install <agent> --update`：把一个已经装好的 agent 升到最新版。
///
/// 走的是跟安装一模一样的那条路（缺 npm 先补自带的 Node、同一个
/// `--registry`、跑完真的再读一遍版本），差别只有两处：动手之前先确认
/// 没有会话在跑，以及追溯不到 npm 包时明说「不是 dct 装的」而不是硬装。
fn run_update(
    socket: &Path,
    runtime: &Path,
    all: &[crate::profile::Profile],
    p: &crate::profile::Profile,
    label: &str,
    cmd0: &str,
    lang: Lang,
) -> i32 {
    // 先问会话再干别的：正在跑的 node 进程底下换文件，Qwen Code 这种
    // 几百个文件的包会在下一次 require 时读到新旧混杂的东西。**拒绝，
    // 不是警告**——警告了照样换，坏的还是同一件事。
    //
    // 数的是**跑同一条命令的所有 profile**，不是 `p.name` 一个：被换掉的是
    // 磁盘上那个包，而 `dc` 和 `qwen` 共用它（见 `sibling_profiles`）。
    let family = sibling_profiles(all, cmd0);
    let sessions = match sessions_now(socket) {
        Sessions::Known(v) => v,
        // 守护进程在、却没给出会话列表。旧版本是一种可能，正忙着、5 秒
        // 读超时是另一种——两种都到这儿，所以那句话不咬定是哪一种。
        // **这一条必须拦住**：「不知道」被当成「没有」的代价，正是这个
        // 检查要防的那件事。
        Sessions::Unusable => {
            eprintln!("{}", crate::i18n::msg::cannot_confirm_open_sessions(lang));
            return 1;
        }
    };
    if let Some(msg) = refusal_for_running_sessions(all, &sessions, &family, lang) {
        eprintln!("{msg}");
        return 1;
    }

    // 自己没有 `[install]` 的（kimi/glm/deepseek/qwen-api）跟着 owner 的包
    // 走——磁盘上那份 `claude` 就是 dct 装的（见 `update::package_for`）。
    // 横幅上仍然印 `label`：用户敲的是 `dct install kimi --update`，此刻
    // 该看见「正在更新 Kimi」，包名是实现细节。
    let Some(package) = crate::update::package_for(p, all) else {
        eprintln!("{}", crate::i18n::msg::not_managed_by_dct(lang, label));
        return 1;
    };

    let Some(exe) = crate::sys::fs::find_command(cmd0) else {
        // 没装就走安装路——`--update` 对一个没装的 agent 就是「装上」。
        // 让他先敲一遍 `dct install` 再敲一遍 `--update` 没有任何意义。
        return run_install(&p.name, false, lang);
    };
    let Some(before) = crate::update::installed_version(&exe, &package) else {
        // profile 里写着 npm，但机器上这一份追溯不到 npm 包：官方安装器
        // 装的那种。硬跑 `npm i -g` 会装出第二份，PATH 上先找到哪个全看
        // 运气——那比不更新糟得多。
        eprintln!("{}", crate::i18n::msg::not_managed_by_dct(lang, label));
        return 1;
    };

    let argv = vec![
        "npm".to_string(),
        "i".into(),
        "-g".into(),
        format!("{package}@latest"),
    ];
    let banner = crate::i18n::msg::updating_agent(lang, label);
    match run_npm_global(argv, runtime, lang, &banner) {
        NpmRun::Ok => {}
        NpmRun::Failed => {
            eprintln!("{}", crate::i18n::msg::install_failed(lang, label));
            return 1;
        }
        NpmRun::NoNpm => return 1,
    }

    // 跑完再读一遍。「npm 说成功」和「版本变了」不是一回事：仓库上就是
    // 这一版的时候 npm 一样是成功退出的。
    let after = crate::sys::fs::find_command(cmd0)
        .and_then(|e| crate::update::installed_version(&e, &package));
    recheck(socket);
    match after {
        Some(v) if v != before => {
            println!("{}", crate::i18n::msg::update_succeeded(lang, label, &v));
            0
        }
        // 已经是最新的**不是失败**：用户要的是「让它最新」，而它已经最新了。
        Some(_) => {
            println!("{}", crate::i18n::msg::already_latest(lang, label));
            0
        }
        None => {
            eprintln!(
                "{}",
                crate::i18n::msg::install_finished_but_missing(lang, cmd0)
            );
            1
        }
    }
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
///
/// `update = true` 时走的是 `run_update`：把「命令找得到就说装好了、退出」
/// 那一步换成「升到最新版」。
pub fn run_install(name: &str, update: bool, lang: Lang) -> i32 {
    let socket = crate::proto::socket_path();
    let runtime = crate::runtime::runtime_dir_for_socket(&socket);

    // `dct install git` 走的是另一条路：git 不是 agent，没有 profile，
    // 也不经过 npm。放在这里而不是另开一个子命令，是因为用户要问的问题
    // 是同一个——「帮我把缺的那样东西装上」——而多一个子命令就多一个
    // 得先知道它存在才用得上的东西。
    if name == "git" {
        return run_install_git(&runtime, lang);
    }

    let profiles_dir = crate::profile::profiles_dir_for_socket(&socket);
    // **内置的加磁盘上的，全都在这里面**（`all_profiles` 已经把两边并过了），
    // 不是只有用户自己写的那几个。`run_update` 要拿整份清单去问两件事：
    // 这条命令归谁所有（`update::package_for`），以及还有谁跑着同一个二进制
    // （`sibling_profiles`）——少一个都会算错。
    let (all, _) = crate::profile::all_profiles(&profiles_dir);

    let Some(p) = all
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

    // `--update` 要在「已经装好了」这句话之前拐弯：那句话正是它要替换掉的
    // 那一步。放在后面的话，更新永远走不到——能更新的东西一定是装好的。
    if update {
        return run_update(&socket, &runtime, &all, &p, &label, &cmd0, lang);
    }

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

    // `--registry` 不在这里追加了：那是 npm 专有的一步，跟补 Node、
    // Windows 上包一层一起搬进了 `run_npm_global`。
    let argv = spec.command.clone();
    if argv.is_empty() {
        eprintln!("{}", crate::i18n::msg::agent_has_no_installer(lang, &label));
        return 1;
    }

    // 只有 npm 那条路要运行时。将来有人写一个 `[install]` 跑别的东西
    // （brew、pip、winget），不该被拖去下一份 Node，也不该被塞 `--registry`。
    let uses_npm = argv[0] == "npm" || argv[0] == "npx";
    let banner = crate::i18n::msg::installing_agent(lang, &label);
    let ok = if uses_npm {
        // 跟 `--update` 共用同一段（`run_npm_global`）：补 Node、加镜像、
        // Windows 上包一层，三件事在两条路上必须一模一样。
        match run_npm_global(argv, &runtime, lang, &banner) {
            NpmRun::Ok => true,
            NpmRun::Failed => false,
            // 没弄到 npm，为什么已经印过了，不要再补一句「装程序说的话」。
            NpmRun::NoNpm => return 1,
        }
    } else {
        spawn_status(&argv, &banner)
    };

    if !ok {
        eprintln!("{}", crate::i18n::msg::install_failed(lang, &label));
        return 1;
    }

    // 装完再查一遍。见函数头注释：这一步是这条命令跟直接敲 npm 的全部差别。
    if crate::profile::command_exists(&cmd0) {
        // 顺手让守护进程重查版本。不发的话，界面上这个 agent 的版本要等到
        // 下一轮（最多一天）才对得上。
        recheck(&socket);
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

    /// 一份最小的会话信息，测试里只覆盖自己关心的那一两个字段。
    /// 加新字段时只有这一处要改。
    fn test_session_info() -> SessionInfo {
        SessionInfo {
            id: 1,
            profile: "claude".into(),
            dir: "/w/dc-terminal".into(),
            state: SessionState::Idle,
            activity: String::new(),
            is_agent: true,
            tag: String::new(),
        }
    }

    fn s(id: u32, state: SessionState, activity: &str) -> SessionInfo {
        SessionInfo {
            id,
            state,
            activity: activity.into(),
            ..test_session_info()
        }
    }

    /// `dct install <agent>` 只认 `--update` 一个开关，别的一律用法错误。
    ///
    /// 钉死「不认识的参数不等于没参数」，跟 `parse_restart_args` 一个脾气：
    /// `dct install claude --dry-run` 要是被当成裸 `dct install claude` 跑，
    /// 用户以为自己什么都没动，实际真装下去了。
    #[test]
    fn install_args_accept_only_the_update_flag() {
        let lang = Lang::Zh;
        assert!(
            matches!(parse_install_args(&args(&["claude"]), lang), InstallArgs::Install(n) if n == "claude")
        );
        assert!(
            matches!(parse_install_args(&args(&["claude", "--update"]), lang), InstallArgs::Update(n) if n == "claude")
        );
        assert!(
            matches!(parse_install_args(&args(&["--update", "claude"]), lang), InstallArgs::Update(n) if n == "claude")
        );
        assert!(matches!(
            parse_install_args(&args(&[]), lang),
            InstallArgs::Usage(_)
        ));
        assert!(matches!(
            parse_install_args(&args(&["claude", "--foo"]), lang),
            InstallArgs::Usage(_)
        ));
        assert!(matches!(
            parse_install_args(&args(&["claude", "codex"]), lang),
            InstallArgs::Usage(_)
        ));
        // 同一个开关给两遍也是用法错误：说明用户对自己敲了什么没有把握。
        assert!(matches!(
            parse_install_args(&args(&["claude", "--update", "--update"]), lang),
            InstallArgs::Usage(_)
        ));
        // 只给开关不给名字，不知道要更新谁。
        assert!(matches!(
            parse_install_args(&args(&["--update"]), lang),
            InstallArgs::Usage(_)
        ));
    }

    fn names(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    /// 「这几个 profile 还有几个会话活着」——`--update` 拒不拒绝就看它。
    ///
    /// 三条边界都得钉：**不相干的 profile 不算**（更新 claude 的时候 codex
    /// 在跑，跟它无关），**`Stopped` 不算**（进程已经没了，底下换文件伤不到
    /// 它），**同一条命令的兄弟要算**（`dc` 跑的就是 `qwen` 那个二进制）。
    /// 数错任何一边的代价都是真的：多算了就是用户明明什么都没开却更新不了，
    /// 少算了就是在活着的 node 进程底下换文件。
    #[test]
    fn live_sessions_count_only_this_profile_and_only_the_living() {
        let s = |profile: &str, state| SessionInfo {
            profile: profile.into(),
            state,
            ..test_session_info()
        };
        let list = vec![
            s("claude", SessionState::Working),
            s("claude", SessionState::Idle),
            s("claude", SessionState::Stopped),
            s("codex", SessionState::Working),
            s("dc", SessionState::Working),
        ];
        assert_eq!(live_sessions_of(&list, &names(&["claude"])).len(), 2);
        assert_eq!(live_sessions_of(&list, &names(&["codex"])).len(), 1);
        assert_eq!(live_sessions_of(&list, &names(&["qwen"])).len(), 0);

        // 更新 `qwen` 的时候一个在跑的 `dc` 会话必须拦得住：两个 profile 跑的
        // 是同一个二进制、同一个 npm 包，`dc` 那个 node 进程底下的文件正是
        // 要被换掉的那批。
        assert_eq!(live_sessions_of(&list, &names(&["qwen", "dc"])).len(), 1);
        // 但 `codex` 仍然不该被牵连——它是另一个包。
        assert_eq!(
            live_sessions_of(&list, &names(&["claude", "kimi", "glm"])).len(),
            2
        );
        // 一个名字都不给 = 没有兄弟可数，不该把所有会话都算进来。
        assert_eq!(live_sessions_of(&list, &[]).len(), 0);

        // 回的是**每个会话的 profile 名**，不只是个数——拒绝那句话要拿它
        // 说清楚到底是谁在跑。顺序照会话列表，重复不去（`claude` 两个）。
        assert_eq!(
            live_sessions_of(&list, &names(&["claude", "dc"])),
            names(&["claude", "claude", "dc"])
        );
    }

    /// 「跑同一条命令的 profile 有哪些」——`--update` 要拦谁，全看它。
    ///
    /// **不写死配对**，从真实的内置 profile 里算：`kimi`/`glm`/`deepseek`/
    /// `qwen-api` 的 `command[0]` 都是 `claude`，`dc` 的是 `qwen`。哪天有人
    /// 改了 toml 或加了新的，这里跟着变，不用有人记得回来改一份清单。
    #[test]
    fn siblings_are_the_profiles_that_run_the_same_command() {
        let all = crate::profile::Profile::builtins();

        let claude_side = sibling_profiles(&all, "claude");
        for n in ["claude", "kimi", "glm", "deepseek", "qwen-api"] {
            assert!(
                claude_side.contains(&n.to_string()),
                "{n} 跑的就是 claude：{claude_side:?}"
            );
        }
        assert!(!claude_side.contains(&"codex".to_string()));
        assert!(!claude_side.contains(&"qwen".to_string()));

        let qwen_side = sibling_profiles(&all, "qwen");
        for n in ["qwen", "dc"] {
            assert!(
                qwen_side.contains(&n.to_string()),
                "{n} 跑的就是 qwen：{qwen_side:?}"
            );
        }
        assert!(!qwen_side.contains(&"claude".to_string()));

        // 自己一定在里面——更新 codex 时它自己的会话当然要算。
        assert_eq!(sibling_profiles(&all, "codex"), vec!["codex".to_string()]);
        // 没有 profile 跑这条命令：空的，不是「全部」。
        assert!(sibling_profiles(&all, "nobody-runs-this").is_empty());
    }

    /// 会话数是 1 的时候英文不能印 `session(s)`：这句话的读者是零基础的人，
    /// 括号里的 `(s)` 是写给程序员看的偷懒写法。
    #[test]
    fn the_refusal_sentence_reads_naturally_for_one_session() {
        let labels = names(&["Claude"]);
        let one = crate::i18n::msg::update_refused_sessions_running(Lang::En, 1, &labels);
        assert!(one.contains("session"), "{one}");
        assert!(!one.contains("(s)"), "单数不该印 (s)：{one}");
        assert!(!one.contains("sessions"), "1 个不该用复数：{one}");

        let many = crate::i18n::msg::update_refused_sessions_running(Lang::En, 2, &labels);
        assert!(many.contains("2 running"), "{many}");
        assert!(many.contains("sessions"), "多个要用复数：{many}");
        assert!(!many.contains("(s)"), "{many}");
    }

    /// 好几个兄弟一起被数进来时，名字要全列出来，中英各用各的顿号。
    #[test]
    fn the_refusal_sentence_names_every_agent_it_counted() {
        let labels = names(&["Claude", "Kimi"]);
        let zh = crate::i18n::msg::update_refused_sessions_running(Lang::Zh, 3, &labels);
        assert!(zh.contains("Claude、Kimi"), "中文用顿号连：{zh}");
        assert!(zh.contains('3'), "{zh}");

        let en = crate::i18n::msg::update_refused_sessions_running(Lang::En, 3, &labels);
        assert!(en.contains("Claude, Kimi"), "英文用逗号连：{en}");
    }

    /// **拒绝那句话必须点名用户真找得到的东西。**
    ///
    /// `dct install kimi --update` 撞上两个 `claude` 会话时，一个 Kimi 会话
    /// 都不存在——印「先关掉正在跑的 2 个 Kimi 会话」等于让用户去找两个
    /// 不存在的东西。要印的是那两个会话**自己**的名字。
    #[test]
    fn the_refusal_names_the_sessions_that_exist_not_the_agent_being_updated() {
        let all = crate::profile::Profile::builtins();
        let live = vec![
            SessionInfo {
                profile: "claude".into(),
                state: SessionState::Working,
                ..test_session_info()
            },
            SessionInfo {
                profile: "claude".into(),
                state: SessionState::Idle,
                ..test_session_info()
            },
        ];

        // 更新 kimi：兄弟里包含 claude，所以这两个会话拦得住它。走的是
        // `run_update` 真正调的那个函数，不是把几步拼起来自己模拟一遍——
        // 拼装本身出过错（把被更新那个的 label 递进去），得由测试盯着。
        let family = sibling_profiles(&all, "claude");
        let sentence = refusal_for_running_sessions(&all, &live, &family, Lang::Zh)
            .expect("两个 claude 会话活着，必须拦住");

        assert!(sentence.contains('2'), "得说清有几个：{sentence}");
        assert!(
            sentence.contains("Claude"),
            "得点名真在跑的那个：{sentence}"
        );
        assert!(
            !sentence.contains("Kimi"),
            "一个 Kimi 会话都没有，不该让用户去找：{sentence}"
        );

        // 都停了就不该拦——那时候底下换文件伤不到任何东西。
        let stopped = vec![SessionInfo {
            profile: "claude".into(),
            state: SessionState::Stopped,
            ..test_session_info()
        }];
        assert!(refusal_for_running_sessions(&all, &stopped, &family, Lang::Zh).is_none());
    }

    /// 去重按出现顺序，认不出的 profile 名原样留着——总比印一个空括号强。
    #[test]
    fn labels_are_deduplicated_in_order_and_fall_back_to_the_raw_name() {
        let all = crate::profile::Profile::builtins();
        assert_eq!(
            distinct_labels(&all, &names(&["dc", "qwen", "dc"]), Lang::Zh),
            names(&["DC", "Qwen Code"])
        );
        assert_eq!(
            distinct_labels(&all, &names(&["ghost"]), Lang::Zh),
            names(&["ghost"])
        );
    }

    /// 守护进程在、但这句话它答不上来（旧版本、协议对不上）→ **拒绝**。
    ///
    /// 这一条是安全边界，不是体验问题：`Client::connect` 只是连上 socket，
    /// **没有任何版本握手**，所以对面是个旧守护进程时连得上、`List` 才失败。
    /// 把那种失败当成「没有会话」，就是在一堆活着的进程底下换文件。
    /// 连都连不上（守护进程没起）是另一回事——那时确实没有会话。
    #[test]
    fn a_daemon_that_cannot_answer_blocks_the_update_but_a_missing_one_does_not() {
        // 守护进程答得上来：拿它的答案。
        assert!(
            matches!(
                sessions_from(Ok(Response::Sessions(vec![test_session_info()]))),
                Sessions::Known(v) if v.len() == 1
            ),
            "答得上来就该照答案办"
        );
        assert!(matches!(
            sessions_from(Ok(Response::Sessions(Vec::new()))),
            Sessions::Known(v) if v.is_empty()
        ));

        // 连上了却答不出这句话——旧守护进程正是这样：`List` 解析失败，或者
        // 回一个别的东西。这时候「有没有会话」是**未知**，不是「没有」。
        assert!(
            matches!(
                sessions_from(Err(anyhow::anyhow!("解析不了"))),
                Sessions::Unusable
            ),
            "答不上来 = 不知道有没有会话 = 不能动手"
        );
        assert!(matches!(
            sessions_from(Ok(Response::Ok)),
            Sessions::Unusable
        ));
        assert!(matches!(
            sessions_from(Ok(Response::Error(crate::proto::ErrorCode::NoSuchSession(
                1
            )))),
            Sessions::Unusable
        ));
    }

    /// 用法提示得把那个开关写出来——用户看不到就等于没有这个功能。
    #[test]
    fn the_install_usage_hint_mentions_the_update_flag() {
        for lang in [Lang::Zh, Lang::En] {
            match parse_install_args(&args(&["claude", "--foo"]), lang) {
                InstallArgs::Usage(m) => {
                    assert!(m.contains("--update"), "用法里该有 --update：{m}");
                    assert!(m.contains("dct install"), "用法里该有命令本身：{m}");
                }
                other => panic!("该是用法提示，实际 {other:?}"),
            }
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
