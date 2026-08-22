use anyhow::Result;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::proto::{Request, Response};
use crate::sys::ipc::Stream;

/// 读超时：TUI 主循环在 `term.draw` 之前会先发 `List`，守护进程一卡住整个界面
/// 就会跟着冻结，连 `q` 都按不动。5 秒对正常操作留了充分余量。
const READ_TIMEOUT: Duration = Duration::from_secs(5);

/// 换掉 socket 那头的守护进程：先请它走，走不动就硬来，然后拉一个新的
/// 起来，确认新的真的能服务了才返回。
///
/// **这会杀掉所有正在跑的会话**，因为 pty 就在守护进程里。调用方必须先问过
/// 用户——这是这个产品最不该擅自做的一件事（「关掉窗口不影响会话」是它的
/// 立身之本）。
pub fn restart_daemon(socket: &Path, exe: &Path) -> Result<()> {
    let old = Client::connect(socket)
        .ok()
        .and_then(|c| c.peer_pid())
        .ok_or_else(|| crate::proto::coded(crate::proto::ErrorCode::DaemonNotResponding))?;

    // 先礼后兵：守护进程收到 SIGTERM 会自己把 pty 收拾干净。给两秒，
    // 还赖着就硬来。
    //
    // **Windows 上没有「先礼」这一档**——`ask_to_stop` 在那边就是硬杀
    // （见 `sys::proc` 开头）。这里的两段式于是退化成一段，代价写在
    // 那个文件里：pty 子进程要靠 job object 兜底，而不是靠守护进程
    // 自己收尾。
    crate::sys::proc::ask_to_stop(old);
    let gone = wait_up_to(Duration::from_secs(2), || !process_alive(old));
    if !gone {
        crate::sys::proc::hard_kill(old);
        wait_up_to(Duration::from_secs(2), || !process_alive(old));
    }

    spawn_daemon(exe, socket)?;

    // 「连得上」不等于「能服务」：旧进程死掉时留下的 socket 文件照样摆在那。
    // 一定要真发一条请求，而且要发 Hello——新起来的那个必须是同一号协议，
    // 否则这次重启什么也没解决。
    let up = wait_up_to(Duration::from_secs(5), || {
        Client::connect(socket)
            .ok()
            .and_then(|mut c| c.protocol())
            .is_some_and(|v| v == crate::proto::PROTOCOL_VERSION)
    });
    if !up {
        return Err(crate::proto::coded(
            crate::proto::ErrorCode::DaemonNotResponding,
        ));
    }
    Ok(())
}

/// 拉起一个守护进程，脱离当前终端。
///
/// 「脱离」具体怎么做按平台分（`sys::proc::spawn_detached`）：Unix 上是
/// `setsid`，Windows 上是 `DETACHED_PROCESS`。两边要的是同一件事——关掉
/// 终端窗口不能把它一起带走，而那正是这个产品存在的理由。
///
/// socket 路径显式传给子进程，不让它自己从 `HOME` 推：重启这条路上「换掉的是
/// 哪个 socket」必须只有一个说法。
pub fn spawn_daemon(exe: &Path, socket: &Path) -> Result<()> {
    let mut cmd = std::process::Command::new(exe);
    cmd.arg("daemon")
        .arg(socket)
        // 守护进程的输出必须全部丢弃：它和 TUI 共用同一个终端，
        // 任何一行 stderr 都会直接糊在界面上。
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    crate::sys::proc::spawn_detached(&mut cmd)?;
    Ok(())
}

fn process_alive(pid: u32) -> bool {
    crate::sys::proc::alive(pid)
}

fn wait_up_to(limit: Duration, mut cond: impl FnMut() -> bool) -> bool {
    let deadline = std::time::Instant::now() + limit;
    while std::time::Instant::now() < deadline {
        if cond() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    cond()
}

/// 一条到守护进程的连接。
///
/// **超时之后必须丢掉连接重连，不能接着用。** 超时只是"这次没等到"，
/// 迟到的响应仍然会留在 socket 里；接着发下一个请求就会读到上一次的
/// 响应，从此每次都差一格，界面会永远显示错的东西。丢掉重连是唯一
/// 能保证请求和响应对得上的办法。
pub struct Client {
    socket: PathBuf,
    conn: Option<Conn>,
}

struct Conn {
    reader: BufReader<Stream>,
    writer: Stream,
}

impl Client {
    pub fn connect(socket: &Path) -> Result<Client> {
        let mut c = Client {
            socket: socket.to_path_buf(),
            conn: None,
        };
        c.reconnect()?;
        Ok(c)
    }

    fn reconnect(&mut self) -> Result<()> {
        let stream = Stream::connect(&self.socket)
            .map_err(|_| crate::proto::coded(crate::proto::ErrorCode::DaemonNotResponding))?;
        stream
            .set_read_timeout(Some(READ_TIMEOUT))
            .map_err(|_| crate::proto::coded(crate::proto::ErrorCode::DaemonNotResponding))?;
        self.conn = Some(Conn {
            reader: BufReader::new(stream.try_clone()?),
            writer: stream,
        });
        Ok(())
    }

    /// 「你是几号协议？」`None` 表示答不上来——老到不认识 `Hello` 的守护进程
    /// 会解析失败，而**答不上来本身就是答案**：那一定不是跟本界面同一份源码
    /// 编出来的。
    pub fn protocol(&mut self) -> Option<u32> {
        match self.call(Request::Hello) {
            Ok(Response::Hello { protocol }) => Some(protocol),
            _ => None,
        }
    }

    /// socket 那头那个进程的 pid。
    ///
    /// 不靠进程名匹配（旧守护进程可能是从完全不同的路径起的），也不靠守护
    /// 进程自己配合——需要换掉的恰恰是那些老到不认识任何新请求的。怎么问
    /// 按平台分，各自能问到什么强度见 `sys::ipc::peer_pid_of`。
    pub fn peer_pid(&self) -> Option<u32> {
        let stream = &self.conn.as_ref()?.writer;
        crate::sys::ipc::peer_pid_of(stream, &self.socket)
    }

    pub fn call(&mut self, req: Request) -> Result<Response> {
        if self.conn.is_none() {
            self.reconnect()?;
        }
        match self.try_call(&req) {
            Ok(resp) => Ok(resp),
            Err(e) => {
                // 任何一次读写出错（含超时）都意味着这条连接的请求/响应
                // 可能已经错位，不能再用。下次调用会自动重连。
                self.conn = None;
                Err(e)
            }
        }
    }

    fn try_call(&mut self, req: &Request) -> Result<Response> {
        let conn = self
            .conn
            .as_mut()
            .ok_or_else(|| crate::proto::coded(crate::proto::ErrorCode::DaemonNotResponding))?;
        writeln!(conn.writer, "{}", serde_json::to_string(req)?)?;
        conn.writer.flush()?;

        let mut line = String::new();
        let n = conn
            .reader
            .read_line(&mut line)
            .map_err(|_| crate::proto::coded(crate::proto::ErrorCode::DaemonNotResponding))?;
        if n == 0 {
            return Err(crate::proto::coded(
                crate::proto::ErrorCode::DaemonNotResponding,
            ));
        }
        Ok(serde_json::from_str(&line)?)
    }
}
