//! 直播观众链接（live-viewer-link）的端到端手工验收脚手架。
//!
//! 跟 `web_routes.rs::serve_for_a_manual_look` 是同一个诉求：老师侧的画面
//! 渲染是终端 ANSI，学生侧的画面渲染是浏览器 JS，这两件事最终只有人的眼睛
//! 能验收，而这个仓库的测试只有 cargo——不会为了这一件事引入一个浏览器
//! 自动化框架。所以给一条明路：把这条链路的三个进程（中转、守护进程、
//! 浏览器要打开的链接）真的跑起来，打出地址和二维码，然后挂着等人去看。
//!
//! # 为什么不是把 `dct_srv::serve` 直接链进这个测试二进制
//!
//! `dct-srv` 是异步的（axum + tokio），`dct` 这一侧从第一行到最后一行都是
//! 同步的——`crates/dct-srv/Cargo.toml` 里那句「`cargo tree -p dct` 里看
//! 不到 tokio」的注释记的正是 Task 1 定下的一条架构边界。把 `dct_srv::serve`
//! 当函数调用接进 `dct` 包的任何一个测试二进制，都要往这个包的
//! `[dev-dependencies]` 里加 `dct-srv`（连带 `tokio`），当场就把这条边界
//! 破了——而这次任务的规矩里也明确写着不许新增依赖。
//!
//! 所以这里换一条不新增依赖也能到达同一个目的地的路：`cargo run -p dct-srv`
//! 起一个真的中转子进程。`cargo run` 保证跑的是**工作区里此刻这一份
//! 源码**，跟 `serve_for_a_manual_look` 要"刚编出来的那份"是同一个诉求，
//! 也是这个仓库本来就在用的做法（`tests/common.rs` 里 `git_repo()` 同样
//! 是拿子进程做事，不是把 `git2` 链进来）。
//!
//! # 为什么不是"这台机器上真在跑的守护进程"
//!
//! `serve_for_a_manual_look` 连的是 `~/.dct/daemon.sock` 上那个可能已经
//! 跑了好几天的真实守护进程——那样做没问题，因为它要验的是网页这一层
//! （测试进程自己起了一个新的 `web::Server`），旧守护进程手里那份协议
//! 逻辑照样够用、不需要重启。直播这条线不一样：中转地址（`DCT_RELAY`）
//! 只在守护进程**启动那一刻**读一次（见 `daemon::run_with_manager` 里
//! `LiveState::new(crate::live::relay_base())` 那一行），推帧线程往后就
//! 认死了那个地址，没有任何办法在守护进程活着的时候让它改认一个新起的
//! 中转。这台机器上真在跑的那个系统守护进程八成还是这一整套直播功能落地
//! 之前编译、启动的（没人平白无故重启一个正常工作的守护进程），塞给它
//! `Request::LiveStart` 大概率连协议版本都对不上；就算凑巧对得上，它也
//! 没法把自己已经钉死的中转地址换成这次测试刚起的那个——唯一能让它重新
//! 读一次 `DCT_RELAY` 的办法是重启它，而重启就是杀掉它正管着的全部会话，
//! 对用户来说等于强行结束一个正在用的 agent，不能这么干。
//!
//! 所以这里照抄 `tests/common.rs::start_daemon()` 的做法：起一个全新的、
//! **真实**的守护进程（跟生产用的是同一份 `daemon::run`，不是任何测试
//! 假货），只是给它一个临时 home、并且在起它之前先把 `DCT_RELAY` 设成
//! 这次测试自己起的那个中转。它不是假守护进程，只是没有借用系统上那一个
//! ——那一个没法被这次测试指挥。这也顺带满足了"别拿用户正在跑的 agent
//! 当试验田"那条规矩：这个守护进程是这次测试专属的一次性进程，连里面
//! 会不会有真实会话这件事都不存在，往里面敲字符、停播、杀掉,都不会碰到
//! 任何用户的真实数据。仍然专门给这次验收开一个新会话（而不是复用某个
//! 已经存在的），是为了跟现有的模式保持一致，也让"哪个会话在直播"这件事
//! 在这份脚手架里一眼可辨。

use std::io::{BufRead, BufReader};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use dct::proto::{LiveReadiness, Request, Response};

mod common;

/// 子进程守卫：这份脚手架结束时把 `dct-srv` 子进程杀掉，不留孤儿进程
/// 占着端口——这台机器上跑这份脚手架的人多半会反复跑好几次。
struct ChildGuard(Child);

impl Drop for ChildGuard {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// 起一个真的中转子进程，返回 (守卫, 中转的 `http://127.0.0.1:PORT`)。
///
/// **第一次跑会连带编译 `dct-srv`**，可能要几十秒——这是有意的：跑的
/// 必须是工作区里此刻这份源码，不是上一次编译遗留在 `target/` 里的旧
/// 二进制。端口用 `:0` 让操作系统挑一个空闲的，跟 `dct-srv/src/main.rs`
/// 自己的默认用法一致（`must_be_loopback` 只认环回地址，`127.0.0.1` 满足）。
fn start_relay() -> (ChildGuard, String) {
    let mut child = Command::new("cargo")
        .args(["run", "--quiet", "-p", "dct-srv", "--", "127.0.0.1:0"])
        .stdout(Stdio::piped())
        // stderr 照原样打到这个测试自己的终端——编译失败、`must_be_loopback`
        // 拒绝之类的话要让跑这份脚手架的人直接看见，不用去猜。
        .stderr(Stdio::inherit())
        .spawn()
        .expect("起不来 `cargo run -p dct-srv`——先确认 `cargo build -p dct-srv` 能过");

    let stdout = child.stdout.take().expect("子进程没有可读的 stdout");
    let mut reader = BufReader::new(stdout);
    let mut line = String::new();
    reader
        .read_line(&mut line)
        .expect("读子进程 stdout 失败");
    if line.is_empty() {
        panic!("`dct-srv` 子进程还没打印监听地址就退出了——看上面的 stderr");
    }
    // 对应 `main.rs`：`println!("dct-srv 在 http://{local} 上，只收本机的连接")`。
    let addr = line
        .split_whitespace()
        .find(|tok| tok.starts_with("http://"))
        .unwrap_or_else(|| panic!("这一行里没找到地址：{line:?}"))
        .to_string();

    // 子进程往后不会再有多少输出（TTL 清扫线程是静默的），但把管道排干净
    // 总比赌它永远不会写满缓冲区更省心。
    std::thread::spawn(move || {
        let mut sink = String::new();
        while reader.read_line(&mut sink).unwrap_or(0) > 0 {
            sink.clear();
        }
    });

    (ChildGuard(child), addr)
}

/// **手工看一眼**用的脚手架，默认不跑（`#[ignore]`）。
///
/// ```text
/// cargo test --test live_end_to_end -- --ignored --nocapture serve_a_live_for_a_manual_look
/// ```
#[test]
#[ignore]
fn serve_a_live_for_a_manual_look() {
    let (relay_guard, relay_base) = start_relay();
    println!("MANUAL_NOTE 中转已经起来：{relay_base}");

    // **必须在起守护进程之前设好**：`daemon::run` 只在启动那一刻读一次
    // `DCT_RELAY`（见模块头那段说明）。这个测试文件只有这一条 `#[ignore]`
    // 测试会用到它，不会跟同一进程里别的测试并发踩踏。
    std::env::set_var("DCT_RELAY", &relay_base);

    let h = common::start_daemon();
    let mut c = h.client();

    // 给这次验收开一个全新的草稿会话——这个守护进程是这次测试自己起的
    // 一次性进程，本来就没有任何"用户正在用的 agent 会话"可言，但仍然
    // 单独开一个，好让"哪个会话在直播"在这份脚手架里一目了然。
    let scratch = tempfile::tempdir().unwrap();
    let id = match c
        .call(Request::Create {
            dir: scratch.path().display().to_string(),
            profile: "shell".into(),
            remember: false,
        })
        .expect("连不上刚起的守护进程")
    {
        Response::Created { id } => id,
        other => panic!("预期 Created，实际 {other:?}"),
    };

    // 给点动静：一个空提示符自己不会变，"画面跟着老师那边动"这条验收
    // 需要真的有字符敲进去。
    let _ = c.call(Request::Input {
        id,
        text: "echo hello from the teacher\n".into(),
    });

    let info = match c
        .call(Request::LiveStart {
            ids: vec![id],
            names: vec!["草稿".into()],
        })
        .expect("LiveStart 请求失败")
    {
        Response::Live(info) => info,
        other => panic!("预期 Live，实际 {other:?}"),
    };
    println!("MANUAL_URL {}", info.url);

    // 推帧线程按 `PUSH_INTERVAL`（500ms）才会去问一次中转「这场直播存在
    // 了没有」，`readiness` 从 `Pending` 翻到 `Ready` 需要等它。给 10 秒
    // 的余量——本地环回、没有真实网络延迟，正常应该几百毫秒内就翻过去。
    let mut ready = matches!(info.readiness, LiveReadiness::Ready);
    let deadline = Instant::now() + Duration::from_secs(10);
    while !ready && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(200));
        ready = match c.call(Request::LiveStatus).expect("LiveStatus 请求失败") {
            Response::Live(i) => matches!(i.readiness, LiveReadiness::Ready),
            other => panic!("预期 Live，实际 {other:?}"),
        };
    }
    if ready {
        println!("MANUAL_NOTE 中转已经确认收下这场直播（readiness = Ready）");
    } else {
        println!(
            "MANUAL_NOTE 10 秒内没等到中转确认（readiness 一直没到 Ready）——\
             下面的链接大概率打不开，如实报告这一点，不要假装它能用"
        );
    }

    print_qr(&info.url);
    match write_qr_svg(&info.url) {
        Ok(path) => println!("MANUAL_QR {}", path.display()),
        Err(e) => println!("MANUAL_NOTE 二维码写不出来：{e}"),
    }

    println!("MANUAL_NOTE 这条链接是这次验收专用的一次性链接——带着学生令牌，\
              中转和守护进程一起退出（这个测试进程一结束）就永久失效");
    // 这一行是操作指令（该做什么），不是验收项（该看到什么）——不占
    // MANUAL_CHECK 的编号，免得 `grep MANUAL_CHECK` 出来的清单里混进一条
    // 没有"对/错"可言的步骤。
    println!("MANUAL_NOTE 用两个浏览器标签页分别打开上面这条 MANUAL_URL，再往下看四条验收项");
    println!("MANUAL_CHECK 1 画面是不是跟着老师那边（这个终端窗口打出来的字）动");
    println!("MANUAL_CHECK 2 学生页上有没有任何能打字的地方——应该一个都没有");
    println!("MANUAL_CHECK 3 老师这边停播之后，学生页是不是显示「这场直播结束了」（5 分钟后这份脚手架会自动停播，见下面）");
    println!("MANUAL_CHECK 4 两个标签页都开着的时候，页面顶栏是不是显示「2 人在看」");
    println!("MANUAL_NOTE 接下来 5 分钟，这个测试会每隔 3 秒往会话里敲一行新内容");

    // 持续 5 分钟，每 3 秒敲一行新内容，给"画面跟着老师那边动"一个持续
    // 的、人看得出来的信号源（而不是敲一次就不再变的静态画面）。
    let keep_typing_until = Instant::now() + Duration::from_secs(5 * 60);
    let mut tick = 0u32;
    while Instant::now() < keep_typing_until {
        std::thread::sleep(Duration::from_secs(3));
        tick += 1;
        let _ = c.call(Request::Input {
            id,
            text: format!("echo tick {tick}\n"),
        });
    }

    println!("MANUAL_CHECK 3 5 分钟到，现在模拟老师停播——去学生页确认它显示\
              「这场直播结束了」（对应上面的验收项 3）");
    let _ = c.call(Request::LiveStop);

    // 给人 2 分钟去确认"已结束"这条界面文案，再收尾。
    std::thread::sleep(Duration::from_secs(2 * 60));

    println!("MANUAL_NOTE 收尾：杀掉草稿会话、停掉中转子进程");
    let _ = c.call(Request::Kill { id });
    let _ = c.call(Request::Prune);
    drop(relay_guard);
}

/// 把二维码画到 stdout 上。
///
/// 跟设置页里那块码是同一个 `qr` 模块、同一套颜色（16 号纯黑 / 231 号纯白，
/// 每一格自己钉死前景和背景）——照抄自 `web_routes.rs::print_qr`，理由
/// 也一样：颜色不能交给终端主题，深色主题下整块码会变成反相的。
fn print_qr(data: &str) {
    let Some(art) = dct::qr::render(data) else {
        println!("MANUAL_NOTE 这段地址编不成二维码");
        return;
    };
    for row in &art.rows {
        let mut line = String::new();
        for cell in row {
            let idx = |c: ratatui::style::Color| match c {
                ratatui::style::Color::Indexed(i) => i,
                _ => 0,
            };
            line.push_str(&format!(
                "\x1b[38;5;{}m\x1b[48;5;{}m▀",
                idx(cell.top),
                idx(cell.bottom)
            ));
        }
        line.push_str("\x1b[0m");
        println!("{line}");
    }
}

/// 把二维码写成一个 SVG 文件，返回路径。双击就能看，然后拿手机/平板扫。
/// 照抄自 `web_routes.rs::write_qr_svg`：用 SVG 不用 PNG，不引任何图片库。
fn write_qr_svg(data: &str) -> std::io::Result<std::path::PathBuf> {
    let art = dct::qr::render(data).ok_or_else(|| {
        std::io::Error::new(std::io::ErrorKind::InvalidInput, "这段地址编不成二维码")
    })?;
    let side = art.cols;
    const PX: usize = 10;
    let mut svg = format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{0}\" height=\"{0}\" \
         viewBox=\"0 0 {1} {1}\"><rect width=\"{1}\" height=\"{1}\" fill=\"#fff\"/>",
        side * PX,
        side
    );
    for (r, row) in art.rows.iter().enumerate() {
        for (c, cell) in row.iter().enumerate() {
            for (half, color) in [(0usize, cell.top), (1usize, cell.bottom)] {
                if color == dct::qr::DARK {
                    let y = r * 2 + half;
                    svg.push_str(&format!(
                        "<rect x=\"{c}\" y=\"{y}\" width=\"1\" height=\"1\" fill=\"#000\"/>"
                    ));
                }
            }
        }
    }
    svg.push_str("</svg>");

    let path = std::env::temp_dir().join("dct-manual-live-qr.svg");
    std::fs::write(&path, svg)?;
    Ok(path)
}
