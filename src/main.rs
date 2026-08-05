use anyhow::{Context, Result};
use std::os::unix::process::CommandExt;
use std::process::{Command, Stdio};
use std::time::Duration;

use dct::client::Client;
use dct::proto::socket_path;

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
        Some("daemon") => dct::daemon::run(&socket_path()),
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

    if Client::connect(&sock).is_err() {
        let mut cmd = Command::new(std::env::current_exe()?);
        cmd.arg("daemon")
            // 守护进程的输出必须全部丢弃：它和 TUI 共用同一个终端，
            // 任何一行 stderr 都会直接糊在界面上。
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());

        // setsid：让守护进程脱离当前终端会话，自成一个新会话。
        // 不这么做的话它跟 TUI 在同一个 session 里，关掉终端窗口时
        // SIGHUP 会把它一起带走——而"关掉窗口不影响会话"正是这个产品
        // 存在的理由。
        unsafe {
            cmd.pre_exec(|| {
                if libc::setsid() == -1 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }

        cmd.spawn().context("拉起守护进程失败")?;

        for _ in 0..50 {
            if Client::connect(&sock).is_ok() {
                break;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
    }

    let client =
        Client::connect(&sock).with_context(|| format!("连不上守护进程：{}", sock.display()))?;
    {
        // 语言在这里定一次：main 是唯一同时知道 socket 路径（设置文件在它旁边）
        // 和真实环境变量的地方。定完交给 ui::run，界面自己不再去猜。
        let lang = dct::i18n::resolve(
            dct::settings::load_lang(&dct::settings::settings_path_for_socket(&sock)),
            &|k| std::env::var(k).ok(),
        );
        dct::ui::run(client, std::env::current_dir()?, lang, sock.clone())
    }
}
