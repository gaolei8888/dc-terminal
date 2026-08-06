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
  dct           打开会话看板（守护进程没在跑就自动拉起）
  dct daemon    只跑守护进程，不开界面
  dct --help    看这段
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
        Some("--help") | Some("-h") => {
            println!("{HELP}");
            Ok(())
        }
        Some(other) => {
            eprintln!("不认识的命令：{other}\n\n{HELP}");
            std::process::exit(2);
        }
    }
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

    if Client::connect(&sock).is_err() {
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
    if daemon_status(client.protocol()) == DaemonStatus::Stale {
        client = offer_to_restart_stale_daemon(client, &sock, &exe, lang)?;
    }

    // 新装默认九宫格：列表在宽屏上是一屏留白，而九宫格直接给出每个
    // 会话在干什么——后者才是「一屏管好几个 agent」这件事的样子。
    let mode = dct::settings::load_view_mode(&settings).unwrap_or(dct::ui::ViewMode::Grid);
    dct::ui::run(client, std::env::current_dir()?, lang, sock.clone(), mode)
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
