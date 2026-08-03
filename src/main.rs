use anyhow::{Context, Result};
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
        // 守护进程的输出必须全部丢弃：它和 TUI 共用同一个终端，
        // 任何一行 stderr 都会直接糊在界面上。
        Command::new(std::env::current_exe()?)
            .arg("daemon")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .context("拉起守护进程失败")?;

        for _ in 0..50 {
            if Client::connect(&sock).is_ok() {
                break;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
    }

    let client =
        Client::connect(&sock).with_context(|| format!("连不上守护进程：{}", sock.display()))?;
    dct::ui::run(client, std::env::current_dir()?)
}
