//! 集成测试共用的「起一个真守护进程」脚手架。
//!
//! `tests/projects_flow.rs` 和 `tests/profiles_flow.rs` 都要走真实的 unix
//! socket——协议层的往返、锁的粒度这些东西只有连真 daemon 才测得出来，
//! 直接调用内部函数会绕过 `serve()`/`handle()` 的编解码和线程边界。这份代码
//! 原来在 `projects_flow.rs` 里单独长了一份，`profiles_flow.rs` 需要一模一样
//! 的起法，所以抽到这里供两边共用。
//!
//! 没有抽给 `concurrency.rs`：那边要在起daemon 之前先往 `SessionManager`
//! 里 `register_profile` 一个测试专用的慢 profile，`start_daemon()` 这种
//! 「零参数、内部自己 new 一个 manager」的形状塞不下那个需求，硬塞会让这个
//! 共用脚手架长出一个只有一个调用方用得到的分支。

// 每个用到 `mod common` 的集成测试文件都会把这份代码单独编译成一个 crate，
// 而不是每个文件都用得上全部方法（比如 `daemon_roundtrip.rs` 不需要
// `git_repo()`）。逐个文件加 `#[allow(dead_code)]` 太啰嗦，这里整体放开。
#![allow(dead_code)]

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::thread::sleep;
use std::time::{Duration, Instant};

use dct::client::Client;
use dct::proto::{PairTick, Request, Response};

/// 一个已经起来的守护进程。`home` 是它的 `~/.dct` 替身，跟着这个 handle 的
/// 生命周期走——测试结束、handle 被 drop，临时目录才被清掉，所以只要 handle
/// 还活着，`sock` 和 `git_repo()` 建的目录就都还在。
pub struct DaemonHandle {
    home: tempfile::TempDir,
    pub sock: PathBuf,
}

/// 起一个全新的守护进程，用临时目录当它的 `~/.dct`——projects.json /
/// secrets.toml / profiles/ 全部落在这个临时目录里，不会碰到真实用户的数据。
pub fn start_daemon() -> DaemonHandle {
    let home = tempfile::tempdir().unwrap();
    let sock = home.path().join("daemon.sock");
    let s = sock.clone();
    std::thread::spawn(move || {
        let _ = dct::daemon::run(&s);
    });
    for _ in 0..50 {
        if sock.exists() {
            return DaemonHandle { home, sock };
        }
        sleep(Duration::from_millis(50));
    }
    panic!("守护进程没起来：{}", sock.display());
}

impl DaemonHandle {
    /// 每次都开一条新连接，跟真实 TUI 的用法一致：一条 `Client` 对应一条
    /// TCP/Unix 连接，不同测试里的 `c` 互不相扰。
    pub fn client(&self) -> Client {
        Client::connect(&self.sock).unwrap()
    }

    /// 建一个已初始化的 git 仓库，目录建在这个 handle 的临时 home 下面，
    /// 生命周期跟着 handle 走。`name` 只是给目录起个可读的名字，不影响内容——
    /// agent 会话要求是 git 仓库，shell 会话不要求，测试用哪个看具体场景。
    pub fn git_repo(&self, name: &str) -> PathBuf {
        let dir = self.home.path().join("repos").join(name);
        std::fs::create_dir_all(&dir).unwrap();
        let run = |args: &[&str]| {
            std::process::Command::new("git")
                .args(args)
                .current_dir(&dir)
                .output()
                .unwrap();
        };
        run(&["init", "-q"]);
        run(&["config", "user.email", "t@example.com"]);
        run(&["config", "user.name", "t"]);
        std::fs::write(dir.join("a.txt"), "hello\n").unwrap();
        run(&["add", "-A"]);
        run(&["commit", "-q", "-m", "init"]);
        dir
    }
}

/// 夹具要用的 POSIX 小工具，在 Windows 上从哪儿来。
///
/// **这是 `src/sys/testing.rs` 的一份影子。** 那一份是 `#[cfg(test)]` 的，
/// 只在库自己的单元测试里存在；集成测试链接的是不带 `cfg(test)` 编译出来的
/// 库，看不见它。为这件事给库开一个 feature、再让 dev-dependency 自引用来
/// 打开它，是一条为二十行代码付出的、很容易日后没人看懂的路——所以这里照抄
/// 一份，改那边的时候记得也看一眼这边。
///
/// 为什么不把脚本改写成 cmd.exe 的说法：见那个文件的开头。
pub fn posix_tool(name: &str) -> String {
    #[cfg(unix)]
    {
        if name == "sh" {
            return "/bin/sh".to_string();
        }
        name.to_string()
    }
    #[cfg(windows)]
    {
        // Git for Windows 自带一整套（`<Git>\usr\bin`）。dct 本来就要 git
        // 才能工作，所以凡是它跑得起来的机器上，这些工具一定在。
        let out = std::process::Command::new("where")
            .arg("git.exe")
            .output()
            .expect("找不到 git.exe——夹具要借用它自带的 POSIX 工具");
        // **别只看第一条，也别数层数。** `where` 会给出好几个 git.exe，
        // 顺序由 PATH 决定：这台机器上第一条是 `<Git>\mingw64\bin\git.exe`，
        // 往上两级是 `<Git>\mingw64`，那底下没有 `usr\bin`。第二条
        // `<Git>\cmd\git.exe` 往上两级才是对的。哪一条排在前面取决于跑
        // 测试的是哪个终端，所以「取第一条 + 上两级」这个写法在同一台机器
        // 上时灵时不灵。
        //
        // 改成：每一条候选都往上一级一级找，谁底下有 `usr\bin\<name>.exe`
        // 就用谁。层数不用猜，PATH 的顺序也不再重要。
        let found = String::from_utf8_lossy(&out.stdout)
            .lines()
            .map(|l| l.trim().to_string())
            .filter(|l| !l.is_empty())
            .find_map(|git| {
                std::path::Path::new(&git)
                    .ancestors()
                    .map(|root| root.join("usr").join("bin").join(format!("{name}.exe")))
                    .find(|p| p.is_file())
            });
        match found {
            Some(p) => p.display().to_string(),
            None => panic!(
                "在 git.exe 附近找不到 {name}.exe。夹具借的是 Git for Windows \
                 自带的那套 POSIX 工具（`<Git>\\usr\\bin`）；`where git.exe` \
                 给出的是：{}",
                String::from_utf8_lossy(&out.stdout).replace('\n', " ")
            ),
        }
    }
}

// ---------------------------------------------------------------------------
// 假网关：`tests/pair_flow.rs` 的脚手架。
//
// **手写在 `TcpListener` 上，不加依赖。** 这个仓库的依赖树里没有一行 C——
// 这正是 Windows 学生不用装 Visual Studio Build Tools 就能编译 dct 的原因。
// 引一个 HTTP 服务器/mock 框架进来会把这条属性用在一个测试文件上就打破。
// 配对走的又只是「发一个 JSON body，收一个 JSON body」这么单薄的一层协议，
// 手写起来比拉一个依赖更省事。

/// 一个假的训练营网关：`origin()` 就是它监听的 `http://127.0.0.1:<port>`，
/// 拿去写进测试 profile 的 `[api].base_url`。
///
/// 后台线程会一直 `accept()` 下去，直到测试进程退出——集成测试里每个文件
/// 编译成独立的二进制，进程结束线程自然收场，不需要显式关闭。
pub struct FakeGateway {
    port: u16,
}

impl FakeGateway {
    pub fn origin(&self) -> String {
        format!("http://127.0.0.1:{}", self.port)
    }
}

/// 读一条 HTTP/1.1 请求，只要它的请求行（`POST /admin/api/pair/poll ...`）。
///
/// **不解析 body。** 假网关的回答只看「敲的是 start 还是 poll 这条路」和
/// 「这是第几次敲」，从不看学生这边发了什么设备码——`pair_http.rs` 已经有
/// 单元测试钉住「dct 发出去的 body 长什么样」，这里不用重复验证。不读 body
/// 会让 `Content-Length` 之后的字节留在 TCP 缓冲区里，但因为每次请求后都
/// `connection: close`，那点残留字节跟着这条连接一起被扔掉，不会串到下一
/// 条请求头里。
fn read_request_path(stream: &mut TcpStream) -> String {
    let mut buf = Vec::new();
    let mut chunk = [0u8; 1024];
    loop {
        let n = stream.read(&mut chunk).unwrap_or(0);
        if n == 0 {
            break;
        }
        buf.extend_from_slice(&chunk[..n]);
        // 请求头以空行结束；请求行永远是第一行，早于这个空行到达。
        if buf.windows(4).any(|w| w == b"\r\n\r\n") {
            break;
        }
    }
    let text = String::from_utf8_lossy(&buf);
    text.lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .unwrap_or("")
        .to_string()
}

fn write_json_response(stream: &mut TcpStream, body: &str) {
    let resp = format!(
        "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
        body.len(),
        body
    );
    let _ = stream.write_all(resp.as_bytes());
    let _ = stream.flush();
}

/// `/admin/api/pair/start` 的固定回答。**`interval` 给 1，不是生产的 3**：
/// 那是网关侧真实的节奏，但测试没有理由为了跟一个节奏保持逼真而多等两秒
/// 一次——`pair::Machine::new` 把 `interval` 夹到最小 1 秒，1 已经是能测到
/// 「等」这件事的最小值。
const START_BODY: &str =
    r#"{"device_code":"d","user_code":"HJ4K-9QTZ","verify_path":"/pair","interval":1,"expires_in":30}"#;

/// 起一个后台线程，把「给定请求路径 → 该回什么 JSON body」这件事交给
/// 调用方的闭包决定。`fake_gateway` 和 `fake_gateway_slow_approve` 都是
/// 在这上面包一层不同的应答策略。
fn spawn_gateway<F>(mut respond: F) -> FakeGateway
where
    F: FnMut(&str) -> String + Send + 'static,
{
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { continue };
            let path = read_request_path(&mut stream);
            let body = respond(&path);
            write_json_response(&mut stream, &body);
        }
    });
    FakeGateway { port }
}

/// 一个按队列走的假网关：`/pair/start` 永远回 [`START_BODY`]；`/pair/poll`
/// 按 `poll_bodies` 的顺序逐条回，用完了就一直重复最后一条——轮询线程在
/// 收到终止态之前不会停，重复最后一条兜住它万一多敲了一次。
pub fn fake_gateway(poll_bodies: Vec<&'static str>) -> FakeGateway {
    let next = std::sync::Mutex::new(0usize);
    spawn_gateway(move |path| {
        if path.contains("/pair/start") {
            return START_BODY.to_string();
        }
        let mut i = next.lock().unwrap();
        let body = poll_bodies
            .get(*i)
            .or_else(|| poll_bodies.last())
            .copied()
            .unwrap_or(r#"{"status":"pending"}"#);
        *i += 1;
        body.to_string()
    })
}

/// 一个只在 `delay` 之后才批准的假网关，`cancelling_means_nothing_is_ever_written`
/// 专用：批准前的每一次 poll 都回 pending，批准之后才回 approved。
pub fn fake_gateway_slow_approve(delay: Duration) -> FakeGateway {
    let started_at = Instant::now();
    spawn_gateway(move |path| {
        if path.contains("/pair/start") {
            return START_BODY.to_string();
        }
        if started_at.elapsed() >= delay {
            r#"{"status":"approved","api_key":"sk-live-should-never-land-on-disk",
                "models":{"anthropic":{},"openai":{"default":"qwen3.5:35b","small_fast":"gemma4:31b"}},
                "platforms":{"qwen3.5:35b":"local"}}"#
                .to_string()
        } else {
            r#"{"status":"pending"}"#.to_string()
        }
    })
}

/// 一条已经连上假网关的守护进程。跟 [`start_daemon`] 的区别只有一点：
/// `home` 由调用方给定（测试要在起daemon *之前*往 `home/profiles/dc.toml`
/// 里写一份指向假网关的 profile），而不是内部自己 new 一个临时目录。
pub struct Daemon {
    client: std::sync::Mutex<Client>,
}

impl Daemon {
    /// 转发给底层 `Client::call`。用 `Mutex` 包一层是因为
    /// `Client::call` 要 `&mut self`，而测试里 `d.call(...)` 每次都是对
    /// 同一个 `d` 变量、不可变地调用——跟真实 TUI 用一条连接反复 `call`
    /// 的用法一致，也省得测试自己去处理可变借用。
    pub fn call(&self, req: Request) -> Response {
        self.client.lock().unwrap().call(req).unwrap()
    }
}

/// 起一个连着 `origin` 这个（真的或假的）网关的守护进程。
///
/// **`dc` 这份 profile 必须整份手写，不能只写 `[api]`。** `Profile::command`
/// 没有 `#[serde(default)]`（`profile.rs`），一份只有 `name`/`[api]` 的
/// 文件在 `load_dir` 眼里是「解析失败」，不是「一份不完整但能用的 profile」，
/// 那样 `pair_origin` 就找不到这个 profile，配对第一步就会失败在
/// `no_api_base_url` 上——错的位置离真正想测的东西很远，排查起来很绕。
pub fn daemon_with(home: &std::path::Path, origin: &str) -> Daemon {
    let profiles_dir = home.join("profiles");
    std::fs::create_dir_all(&profiles_dir).unwrap();
    std::fs::write(
        profiles_dir.join("dc.toml"),
        format!(
            "name = \"dc\"\ncommand = [\"echo\"]\n\n[api]\nbase_url = \"{origin}\"\nwire = \"anthropic\"\n"
        ),
    )
    .unwrap();

    let sock = home.join("daemon.sock");
    let s = sock.clone();
    std::thread::spawn(move || {
        let _ = dct::daemon::run(&s);
    });
    for _ in 0..50 {
        if sock.exists() {
            return Daemon {
                client: std::sync::Mutex::new(Client::connect(&sock).unwrap()),
            };
        }
        sleep(Duration::from_millis(50));
    }
    panic!("守护进程没起来：{}", sock.display());
}

/// 轮询 `Request::PairPoll` 直到不再是 `Waiting`。
///
/// **不能睡一个固定的时长再看一眼。** 配对是 daemon 后台线程在跑，界面
/// （这里是测试）读到的是一份缓存的 tick——睡多久才够全凭猜，猜少了测试
/// 就会在机器慢的时候随机失败，猜多了就是白白拖慢每一次跑测试。改成
/// 「一直问，问到状态变了为止」，快的时候几十毫秒就返回，慢的时候也不会
/// 因为差一点点就误判成失败。
pub fn wait_for_tick(d: &Daemon, profile: &str, timeout: Duration) -> PairTick {
    let deadline = Instant::now() + timeout;
    loop {
        if let Response::PairTick(tick) = d.call(Request::PairPoll {
            profile: profile.to_string(),
        }) {
            if !matches!(tick, PairTick::Waiting) {
                return tick;
            }
        }
        assert!(
            Instant::now() < deadline,
            "{timeout:?} 内配对都没有走出 Waiting"
        );
        sleep(Duration::from_millis(100));
    }
}
