use anyhow::{Context, Result};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::Duration;

use dct::client::Client;
use dct::i18n::{text, Key, Lang};
use dct::proto::{daemon_status, socket_path, DaemonStatus};

const HELP: &str = "\
dct —— vibe coding 终端

用法：
  dct              打开会话看板（守护进程没在跑就自动拉起）
  dct ps           列出后台在跑的会话
  dct stop <会话号> 停掉某个会话，可以给多个
  dct stop --all   停掉全部会话
  dct kill <会话号> 强制杀掉，不给它收尾的时间；可以给多个
  dct kill --all   强制杀掉全部会话
  dct prune        把已经停掉的会话从列表里清掉
  dct install <名字> 装一个 agent（claude / codex / qwen / opencode）。
                   缺 Node 运行时会自己补上一份，只给 dct 自己用
  dct llm check    把配置里那条 LLM 连接真的跑一次，看通不通
  dct daemon       只跑守护进程，不开界面
  dct --version    看装的是哪一版
  dct --help       看这段

ps / stop / kill / prune 都不会拉起守护进程：问「有没有东西在跑」不该把
「没有」变成「有」。
";

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();

    match args.first().map(|s| s.as_str()) {
        None => run_ui(),
        // 第二个参数是 socket 路径，重启守护进程时由界面显式传进来
        // （见 `client::spawn_daemon`）。不传就还是从 HOME 推。
        Some("daemon") => {
            let sock = args.get(1).map(PathBuf::from).unwrap_or_else(socket_path);
            dct::daemon::run(&sock)
        }
        // ps / stop 走的是**已经在跑**的守护进程，连不上就如实说没有，
        // 绝不顺手拉起一个——见 `cli` 的模块注释。
        Some("ps") => dct::cli::run_ps(&socket_path(), cli_lang()),
        Some("stop") => {
            let target = dct::cli::parse_target_args(&args[1..], cli_lang(), "stop");
            let code = dct::cli::run_stop(&socket_path(), cli_lang(), target)?;
            // 停不成要让脚本看得出来。`dct stop 3 && 干别的` 这种写法很自然，
            // 而「3 号根本没停成」如果只体现在 stderr 上，后面那半句照样会跑。
            std::process::exit(code)
        }
        Some("kill") => {
            let target = dct::cli::parse_target_args(&args[1..], cli_lang(), "kill");
            let code = dct::cli::run_kill(&socket_path(), cli_lang(), target)?;
            std::process::exit(code)
        }
        // prune 不接参数：它只对已经停了的会话下手，那批东西不可能被误伤。
        Some("prune") => dct::cli::run_prune(&socket_path(), cli_lang()),
        // `install` 也不连守护进程：它装的是磁盘上的东西，跟有没有会话
        // 在跑无关。界面开一个 shell 会话把这一行敲进去，学生因此看得见
        // 整个过程——那正是这条命令要给他的东西。
        Some("install") => match args.get(1) {
            Some(name) => std::process::exit(dct::cli::run_install(name, cli_lang())),
            None => {
                eprintln!("要装哪个？比如：dct install claude");
                std::process::exit(2)
            }
        },
        // `llm check` 不连守护进程：它验的是 dct 自己直接打模型那条独立
        // 通路，跟会话、pty 都无关。
        Some("llm") if args.get(1).map(|s| s.as_str()) == Some("check") => {
            std::process::exit(dct::cli::llm_check(cli_lang()))
        }
        Some("--help") | Some("-h") => {
            println!("{HELP}");
            Ok(())
        }
        // 装好之后能问出「我装的是哪一版」，这是排查的第一句话。在这之前
        // 唯一的答案是去翻安装日志——而一条 `curl | sh` 装完就没有日志了。
        // 只印版本号一行，不印别的：这行会被人抄进聊天窗口发过来。
        Some("--version") | Some("-V") => {
            println!("dct {}", env!("CARGO_PKG_VERSION"));
            Ok(())
        }
        Some(other) => {
            eprintln!("不认识的命令：{other}\n\n{HELP}");
            std::process::exit(2);
        }
    }
}

/// `ps` / `stop` 用哪种语言。跟界面同一条路：设置里选过的优先，没选过看
/// 环境变量。这两条命令印的是给人看的话（状态词、「要停哪个」），跟界面
/// 说两种语言会很怪。
fn cli_lang() -> dct::i18n::Lang {
    let settings = dct::settings::settings_path_for_socket(&socket_path());
    dct::i18n::resolve(dct::settings::load_lang(&settings), &|k| {
        std::env::var(k).ok()
    })
}

fn run_ui() -> Result<()> {
    let sock = socket_path();
    let exe = std::env::current_exe()?;

    // 语言在这里定一次：main 是唯一同时知道 socket 路径（设置文件在它旁边）
    // 和真实环境变量的地方。定完交给 ui::run，界面自己不再去猜。
    let settings = dct::settings::settings_path_for_socket(&sock);
    let lang = dct::i18n::resolve(dct::settings::load_lang(&settings), &|k| {
        std::env::var(k).ok()
    });

    // 下面两句问答都用 `read_line`，而它要求终端处于行输入模式。上一个 dct
    // 如果是被硬杀掉的（任务管理器、`kill -9`、崩溃），终端会停在 raw mode
    // 里——那时候按 `y` 不回显、回车发的是 `\r`，`read_line` 等一个永远不来
    // 的换行，用户看到的是「问了我却不理我」。先把模式摆正，见 `sys::term`。
    dct::sys::term::ensure_line_mode();

    if Client::connect(&sock).is_err() {
        // 真的要拉起一个全新的守护进程之前问一次：上次它带着这些会话被
        // 关掉过，要不要把它们接回来。放在这里而不是让守护进程自己问——
        // 守护进程一旦真的被 `spawn_daemon` 拉起来，stdin/stdout/stderr
        // 全被接到 `/dev/null`（见 `spawn_daemon` 的注释：它要跟 TUI 共用
        // 同一个终端，任何一行输出都会糊到界面上），那时候已经没有终端
        // 可以问了。
        offer_to_resume_last_sessions(&sock, lang);

        dct::client::spawn_daemon(&exe, &sock).context("拉起守护进程失败")?;

        for _ in 0..50 {
            if Client::connect(&sock).is_ok() {
                break;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
    }

    let mut client =
        Client::connect(&sock).with_context(|| format!("连不上守护进程：{}", sock.display()))?;

    // 界面和守护进程是分开升级的。守护进程一活就是好几天（它活得久正是这个
    // 产品存在的理由），所以「新界面碰上旧守护进程」不是意外，是常态——而
    // 现场表现是按 n 弹一句「拿不到 agent 列表」，一个既不说明原因、也不
    // 告诉用户怎么办的死胡同。趁还没进界面，在这里问清楚。
    //
    // **`offer_to_resume_last_sessions` 故意不摆在这条分支上。** 它只在
    // 上面「压根没有守护进程在跑」那条分支问过一次——旧守护进程重启这条
    // 路，用户在 `offer_to_restart_stale_daemon` 里已经被明确告知「重启
    // 会断掉正在跑的会话」并且亲口按了 `y` 同意，那本身就是一次对「这些
    // 会话要断了」的知情同意，重启完了之后没道理再让他确认第二遍要不要
    // 接回来——`restart_daemon` 杀旧进程之前，daemon 一直在跑，`create`/
    // `stop`/`kill`/`prune` 早把 `last-sessions.toml` 维护得很新，新起来
    // 的那个守护进程原样按「有清单就恢复」的默认流程接回去就是了，不需要
    // 再问一遍。
    if daemon_status(client.protocol()) == DaemonStatus::Stale {
        client = offer_to_restart_stale_daemon(client, &sock, &exe, lang)?;
    }

    // 新装默认九宫格：列表在宽屏上是一屏留白，而九宫格直接给出每个
    // 会话在干什么——后者才是「一屏管好几个 agent」这件事的样子。
    let mode = dct::settings::load_view_mode(&settings).unwrap_or(dct::ui::ViewMode::Grid);
    dct::ui::run(client, std::env::current_dir()?, lang, sock.clone(), mode)
}

/// 这次要拉起的是一个全新的守护进程（`Client::connect` 刚判定连不上），
/// 而上次关掉的那个守护进程可能还留着一份 `last-sessions.toml`。进界面
/// 之前问一次：接回来，还是从空白看板开始。
///
/// **只在这里问，不在守护进程里问**——见调用点的注释。
///
/// 用户按 Enter（或者答案不是 `y`）：清单要清空，`daemon::run_with_manager`
/// 稍后读到的就是一份空清单，什么都不会恢复——这正是「拒绝恢复要清掉
/// 记录」这条规矩落地的地方。按 `y`：清单原样留着不动，交给守护进程
/// 启动时自己去读、自己去按 `(dir, profile)` 分组决定谁真的带上恢复参数
/// （`daemon::restore_last_sessions` / `last_sessions::group_for_resume`）——
/// 这里不重复算一遍分组，只是为了把状态词（继续/新开）显示给用户看。
///
/// 读不到 stdin（没有终端、被脚本调用……）**什么都不做**：不清空、也不
/// 假装用户回答了什么。这份清单本来就没有过期这回事（见 `last_sessions`
/// 模块的文档），保持原样比乱猜一个答案更安全。
fn offer_to_resume_last_sessions(sock: &Path, lang: Lang) {
    let path = dct::last_sessions::last_sessions_path_for_socket(sock);
    let records = dct::last_sessions::load(&path);
    if records.is_empty() {
        return;
    }

    let profiles_dir = dct::profile::profiles_dir_for_socket(sock);
    let (all, _) = dct::profile::all_profiles(&profiles_dir);
    let resume_flags = dct::last_sessions::group_for_resume(&records);

    println!("{}", text(Key::ResumeSessionsExplain, lang));
    for (rec, &resume) in records.iter().zip(resume_flags.iter()) {
        let label = all
            .iter()
            .find(|p| p.name == rec.profile)
            .map(|p| p.display_label(lang))
            .unwrap_or_else(|| rec.profile.clone());
        let marker = text(
            if resume {
                Key::ResumeSessionsWillContinue
            } else {
                Key::ResumeSessionsWillStartFresh
            },
            lang,
        );
        let name = if rec.tag.is_empty() {
            rec.dir.display().to_string()
        } else {
            format!("{}  ({})", rec.tag, rec.dir.display())
        };
        println!("  {label}  {name}  [{marker}]");
    }
    print!("{} ", text(Key::ResumeSessionsAsk, lang));
    let _ = std::io::stdout().flush();

    let mut answer = String::new();
    if std::io::stdin().read_line(&mut answer).is_err() {
        return;
    }
    if !answer.trim().eq_ignore_ascii_case("y") {
        // 拒绝恢复：清单要清空，不是留着等下次再问。落盘失败就算了——
        // 最坏的结果是下次又问一遍同样的问题，不影响这次已经决定
        // 「从空白看板开始」这件事。
        let _ = dct::last_sessions::clear(&path);
    }
}

/// 撞上旧守护进程时，进界面之前先跟用户说清楚。
///
/// 为什么问而不是直接换掉：重启会杀光所有正在跑的会话（pty 就在守护进程里），
/// 而「关掉窗口不影响会话」是这个产品的立身之本。擅自替用户做这个决定，比
/// 让他带着一个功能不全的旧守护进程接着用更糟。
///
/// 为什么在进 TUI 之前问：这时候还是一个普通终端，一句话一个回车就够了；
/// 进了界面之后同样的事要长成一个模态框、一套按键、一条恢复路径。
///
/// 用户说不，就照原样进去——他知道自己在用什么了，这是他的选择。
fn offer_to_restart_stale_daemon(
    client: Client,
    sock: &Path,
    exe: &Path,
    lang: Lang,
) -> Result<Client> {
    println!("{}\n", text(Key::StaleDaemonExplain, lang));
    print!("{} ", text(Key::StaleDaemonAsk, lang));
    let _ = std::io::stdout().flush();

    let mut answer = String::new();
    if std::io::stdin().read_line(&mut answer).is_err() {
        return Ok(client);
    }
    if !answer.trim().eq_ignore_ascii_case("y") {
        return Ok(client);
    }

    println!("{}", text(Key::StaleDaemonRestarting, lang));
    // 先把手里这条连接丢掉：它连着的是马上要被杀掉的那个进程。
    drop(client);

    match dct::client::restart_daemon(sock, exe) {
        Ok(()) => Client::connect(sock).context("重启之后连不上守护进程"),
        Err(_) => {
            // 没换成也得让他进得去——旧的还在跑，功能不全但会话都还在。
            // 唯一不能接受的结果是把人挡在门外。
            println!("{}", text(Key::StaleDaemonRestartFailed, lang));
            Client::connect(sock).context("连不上守护进程")
        }
    }
}
