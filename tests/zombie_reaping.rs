//! agent **自己退出**之后，它的进程必须被回收，不能留成僵尸。
//!
//! 按 `s` 停止那条路一直是好的（`stop()` 走 `pty.kill()`，里面显式 `wait()`）。
//! 漏的是另一条：agent 自己走掉——`/exit`、崩溃、shell 里 `exit`。那时读线程
//! 读到 EOF 只置了个 `alive` 标志，没有也不能 `wait()`；而 `is_alive()` 一看
//! 标志就短路返回，里面的 `try_wait()` 走不到。会话随即被判成 `Stopped`，
//! 之后 `tick()` 再也不看它，`Session` 又一直留在 map 里所以 `Drop` 也不跑。
//! 子进程就这么挂着，直到守护进程重启。
//!
//! 而守护进程**一活就是好几天**——它活得久正是这个产品存在的理由。所以这不是
//! 「反正进程会退出」能糊弄过去的事：每一个自己退出的 agent 都会永久占掉一个
//! 进程表项，攒够了整台机器都开不出新进程。
//!
//! 这条测试问的是操作系统，不是我们自己的状态字段：拿到子进程真实 pid，
//! 直接看 `ps` 报的进程状态里有没有 `Z`。断言我们自己的 `SessionState` 是没用的
//! ——它在出 bug 的那一版里同样是 `Stopped`，一切看起来都对。

//! **这一整个文件只在 Unix 上有意义**：僵尸进程是 Unix 特有的东西。父进程
//! 不 `wait()` 就留一个尸体占着进程表项——而 Windows 上进程退出后留下的是
//! 一个内核对象，最后一个句柄关掉它就没了，没有「必须收尸」这一步，也就
//! 没有这条测试要防的那个泄漏（见 `sys::proc` 开头）。
#![cfg(unix)]

use std::time::{Duration, Instant};

use dct::proto::{Request, Response};
use dct::session::SessionState;

mod common;

/// 当前进程的直接子进程。
///
/// `common::start_daemon` 是在**本测试进程里起了个线程**跑守护进程，所以
/// 会话的 PTY 子进程挂在测试进程自己名下。协议里没有暴露子进程 pid，
/// 而这条测试非要那个真实 pid 不可——只有它能去问操作系统。
/// **必须连命令名一起取，不能只取 pid。** 这个函数自己就要跑 `ps`，而那个
/// `ps` 在运行期间同样是本进程的子进程；只按 pid 做差集的话，「新出现的子
/// 进程」很可能抓到的是那个转瞬即逝的 `ps`——它一退就被 `output()` 回收，
/// 于是这条测试不管有没有僵尸都会通过。
fn own_children() -> Vec<(u32, String)> {
    let out = std::process::Command::new("ps")
        .args(["-axo", "pid=,ppid=,comm="])
        .output()
        .expect("ps 跑不起来");
    let me = std::process::id();
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter_map(|l| {
            let mut it = l.split_whitespace();
            let pid: u32 = it.next()?.parse().ok()?;
            let ppid: u32 = it.next()?.parse().ok()?;
            let comm = it.collect::<Vec<_>>().join(" ");
            // `ps` 自己不算——见上面
            (ppid == me && !comm.ends_with("/ps") && comm != "ps").then_some((pid, comm))
        })
        .collect()
}

/// `ps` 报的进程状态码。进程已经彻底没了 → `None`。
///
/// macOS 和 Linux 的 `ps -o state=` 都用同一套字母，僵尸都是 `Z` 开头
/// （macOS 会给出 `Z+` 这种带修饰的形式，所以用 `starts_with`）。
fn process_state(pid: u32) -> Option<String> {
    let out = std::process::Command::new("ps")
        .args(["-o", "state=", "-p", &pid.to_string()])
        .output()
        .ok()?;
    let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if s.is_empty() {
        None
    } else {
        Some(s)
    }
}

#[test]
fn an_agent_that_exits_on_its_own_does_not_become_a_zombie() {
    let h = common::start_daemon();
    let workdir = tempfile::tempdir().unwrap();
    let mut c = h.client();

    let before = own_children();
    let id = match c
        .call(Request::Create {
            dir: workdir.path().display().to_string(),
            profile: "shell".into(),
            remember: false,
        })
        .unwrap()
    {
        Response::Created { id } => id,
        other => panic!("预期 Created，实际 {other:?}"),
    };

    // 等 shell 起来，拿到它的真实 pid。`Screen` 顺带证明会话确实活着。
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        match c.call(Request::Screen { id }).unwrap() {
            Response::Screen { lines, .. } => {
                let text: String = lines
                    .iter()
                    .flat_map(|l| l.iter().map(|s| &s.text))
                    .cloned()
                    .collect();
                if !text.trim().is_empty() {
                    break;
                }
            }
            other => panic!("预期 Screen，实际 {other:?}"),
        }
        assert!(Instant::now() < deadline, "shell 一直没起来");
        std::thread::sleep(Duration::from_millis(50));
    }
    let seen: Vec<u32> = before.iter().map(|(p, _)| *p).collect();
    let (pid, comm) = own_children()
        .into_iter()
        .find(|(p, _)| !seen.contains(p))
        .expect("找不到这个会话新起的子进程");
    println!("会话 {id} 的子进程：pid={pid} comm={comm}");
    assert!(
        !process_state(pid).unwrap_or_default().starts_with('Z'),
        "还没退出就先是僵尸了，说明测试本身有问题"
    );

    // **让它自己退出**，不走 Request::Stop——那条路本来就是好的。
    c.call(Request::Input {
        id,
        text: "exit".into(),
    })
    .unwrap();
    c.call(Request::Input {
        id,
        text: String::new(),
    })
    .unwrap();

    // 等守护进程的 tick 把它判成停止。回收就发生在那一轮里。
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        match c.call(Request::Screen { id }).unwrap() {
            Response::Screen {
                state: SessionState::Stopped,
                ..
            } => break,
            Response::Screen { .. } => {}
            other => panic!("预期 Screen，实际 {other:?}"),
        }
        assert!(
            Instant::now() < deadline,
            "会话自己退出了，守护进程却没判成停止"
        );
        std::thread::sleep(Duration::from_millis(50));
    }

    // 回收之后：进程要么彻底没了（None），要么至少不是 Z。
    // 给一点余量——wait() 和 ps 之间隔着调度。
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        match process_state(pid) {
            None => return,
            Some(s) if !s.starts_with('Z') => return,
            Some(s) => {
                assert!(
                    Instant::now() < deadline,
                    "子进程 {pid} 停在 `{s}` 状态没被回收——这就是僵尸泄漏"
                );
            }
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}
