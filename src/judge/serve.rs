//! 本机 judge 服务的启停：`dct judge up | down | status`，以及 `dct answer` 连不上时的自动启动。
//!
//! 一切「怎么启动」都来自 `[judge.serve]`（argv、环境变量），dct 里不写任何路径或厂商名。
//! 只碰 dct 自己起的进程：pid 记在 `~/.dct/judge-serve.pid`，停之前要核对这个 pid 还活着、
//! 命令行跟起它时读到的一模一样；不是就不动。base_url 必须是本机回环地址，否则一律不起不停。
//! 起之前看空闲内存，低于 `min_free_percent` 就不起（服务要占几 GB，用户吃过内存压力的亏）。
//!
//! `idle_stop_minutes`（空闲自动停）**还没做**，写了会被忽略。

use super::{is_loopback_host, ChoiceAnswer, ChoiceQuestion, JudgeBackend, JudgeError};
use crate::config::{JudgeConfig, ServeConfig};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// 英文 / 中文一句话。两边都必须有（测试里用断言钉住）。
fn t(en: &str, zh: &str) -> String {
    debug_assert!(
        en.is_ascii() && !en.is_empty(),
        "en part must be plain ASCII: {en}"
    );
    debug_assert!(
        zh.chars().any(|c| c > '\u{2e80}'),
        "zh part must have CJK: {zh}"
    );
    format!("{en} / {zh}")
}

/// 记在 pid 文件里的东西。**没有密钥**：命令行是 argv，本来就是 `ps` 能看见的。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PidRecord {
    pub pid: u32,
    pub started_unix: u64,
    /// 配置里 argv 的 sha256，`status` 用它看「配置改过没有」。
    pub cmd_hash: String,
    /// 刚起来时从系统读到的该进程命令行；停之前必须还一样。空 = 读不到，永不杀。
    pub cmdline: String,
}

#[derive(Debug, Clone)]
pub struct Paths {
    pub pid: PathBuf,
    pub log: PathBuf,
    pub lock: PathBuf,
    pub err: PathBuf,
}

impl Paths {
    pub fn in_dir(d: &Path) -> Paths {
        Paths {
            pid: d.join("judge-serve.pid"),
            log: d.join("judge-serve.log"),
            lock: d.join("judge-serve.lock"),
            err: d.join("judge-serve.err"),
        }
    }
    pub fn system() -> Paths {
        let sock = crate::proto::socket_path();
        Paths::in_dir(sock.parent().unwrap_or(Path::new(".")))
    }
}

/// 可替换的外部世界：测试注入内存读数和节奏。
pub struct Deps<'a> {
    /// 空闲内存百分比；`None` = 读不到（不拦）。
    pub free_percent: &'a dyn Fn() -> Option<u8>,
    pub poll: Duration,
    /// SIGTERM 之后等多久再 SIGKILL。
    pub grace: Duration,
}

impl Deps<'_> {
    pub fn system() -> Deps<'static> {
        Deps {
            free_percent: &system_free_percent,
            poll: Duration::from_millis(250),
            grace: Duration::from_secs(5),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum UpOutcome {
    AlreadyHealthy,
    Started { pid: u32, secs: f64 },
}

#[derive(Debug, Clone, PartialEq)]
pub enum ServeError {
    NotConfigured,
    NotLoopback(String),
    LowMemory {
        free: u8,
        min: u8,
    },
    Spawn(String),
    Exited {
        code: Option<i32>,
        log: PathBuf,
    },
    Timeout {
        secs: u64,
        pid: u32,
        log: PathBuf,
    },
    /// 我们起的进程还活着但一直不健康（起得太久了）。
    Hung {
        pid: u32,
    },
    /// 另一个 dct 正在起，等了也没好。
    OtherStarterTimeout {
        secs: u64,
    },
    Io(String),
    /// 读不到进程命令行，没法确认「这是我们起的那个」，所以不杀。
    CannotVerify {
        pid: u32,
    },
    StillAlive {
        pid: u32,
    },
}

impl std::fmt::Display for ServeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            ServeError::NotConfigured => t(
                "[judge.serve] command is not set; dct does not start or stop anything",
                "没写 [judge.serve] command；dct 不会启动也不会停止任何东西",
            ),
            ServeError::NotLoopback(h) => t(
                &format!("[judge] base_url host \"{h}\" is not loopback (127.0.0.1 / localhost / ::1); dct only starts or stops a judge on this machine"),
                &format!("[judge] base_url 的主机「{h}」不是本机回环地址（127.0.0.1 / localhost / ::1）；dct 只启停本机上的 judge"),
            ),
            ServeError::LowMemory { free, min } => t(
                &format!("not starting the judge: free memory is {free}%, below min_free_percent = {min}%"),
                &format!("不启动 judge：空闲内存只有 {free}%，低于 min_free_percent = {min}%"),
            ),
            ServeError::Spawn(e) => t(
                &format!("could not start the judge command: {e}"),
                &format!("judge 命令起不来：{e}"),
            ),
            ServeError::Exited { code, log } => {
                let c = code.map_or("signal".to_string(), |c| c.to_string());
                t(
                    &format!("the judge process exited right after starting (exit {c}); see {}", log.display()),
                    &format!("judge 进程刚起就退出了（退出码 {c}）；看 {}", log.display()),
                )
            }
            ServeError::Timeout { secs, pid, log } => t(
                &format!("the judge did not answer /health within {secs}s; pid {pid} is left running (see {}; `dct judge down` stops it)", log.display()),
                &format!("judge 在 {secs} 秒内没有答 /health；进程 {pid} 还留着（看 {}；`dct judge down` 可停掉）", log.display()),
            ),
            ServeError::Hung { pid } => t(
                &format!("the judge started by dct (pid {pid}) is alive but not answering /health; `dct judge down` stops it"),
                &format!("dct 起的 judge（进程 {pid}）还活着但不答 /health；`dct judge down` 可停掉"),
            ),
            ServeError::OtherStarterTimeout { secs } => t(
                &format!("another dct is starting the judge and it is not ready after {secs}s"),
                &format!("另一个 dct 正在启动 judge，{secs} 秒后还没好"),
            ),
            ServeError::Io(e) => t(&format!("file error: {e}"), &format!("文件操作失败：{e}")),
            ServeError::CannotVerify { pid } => t(
                &format!("cannot read the command line of pid {pid} on this system, so I cannot confirm it is the one dct started; not stopping it"),
                &format!("在这个系统上读不到进程 {pid} 的命令行，没法确认它是 dct 起的那个；不停它"),
            ),
            ServeError::StillAlive { pid } => t(
                &format!("pid {pid} is still alive after SIGKILL"),
                &format!("进程 {pid} 在 SIGKILL 之后仍然活着"),
            ),
        };
        f.write_str(&s)
    }
}

// ---------- 小工具 ----------

fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

fn health_url(base_url: &str) -> String {
    let b = base_url.trim().trim_end_matches('/');
    let b = b.strip_suffix("/v1/systemone").unwrap_or(b);
    format!("{b}/health")
}

/// `GET {base_url}/health` 回 2xx 才算好。
pub fn healthy(base_url: &str) -> bool {
    let agent = crate::sys::tls::agent_builder()
        .timeout(Duration::from_secs(2))
        .timeout_connect(Duration::from_secs(1))
        .build();
    agent.get(&health_url(base_url)).call().is_ok()
}

fn wait_healthy(base_url: &str, budget: Duration, poll: Duration) -> bool {
    let t0 = Instant::now();
    loop {
        if healthy(base_url) {
            return true;
        }
        if t0.elapsed() >= budget {
            return false;
        }
        std::thread::sleep(poll);
    }
}

fn cmd_hash(argv: &[String]) -> String {
    let mut h = Sha256::new();
    for a in argv {
        h.update(a.as_bytes());
        h.update([0]);
    }
    h.finalize().iter().map(|b| format!("{b:02x}")).collect()
}

/// 进程的命令行（空格拼好的一行）。Unix 用 `ps`；别处读不到 = `None`。
pub fn process_cmdline(pid: u32) -> Option<String> {
    #[cfg(unix)]
    {
        let out = Command::new("ps")
            .args(["-o", "command=", "-p", &pid.to_string()])
            .output()
            .ok()?;
        let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
        (out.status.success() && !s.is_empty()).then_some(s)
    }
    #[cfg(not(unix))]
    {
        let _ = pid;
        None
    }
}

fn process_rss_kb(pid: u32) -> Option<u64> {
    #[cfg(unix)]
    {
        let out = Command::new("ps")
            .args(["-o", "rss=", "-p", &pid.to_string()])
            .output()
            .ok()?;
        String::from_utf8_lossy(&out.stdout).trim().parse().ok()
    }
    #[cfg(not(unix))]
    {
        let _ = pid;
        None
    }
}

// ---------- 内存 ----------

/// `memory_pressure` 的 "System-wide memory free percentage: 36%"。
pub fn parse_memory_pressure(out: &str) -> Option<u8> {
    let rest = out.split("free percentage:").nth(1)?;
    rest.trim_start().split('%').next()?.trim().parse().ok()
}

/// `/proc/meminfo` 的 MemAvailable / MemTotal。
pub fn parse_meminfo(s: &str) -> Option<u8> {
    let kb = |key: &str| -> Option<u64> {
        s.lines()
            .find_map(|l| l.strip_prefix(key))?
            .trim()
            .trim_end_matches("kB")
            .trim()
            .parse()
            .ok()
    };
    let (avail, total) = (kb("MemAvailable:")?, kb("MemTotal:")?);
    (total > 0).then(|| (avail * 100 / total).min(100) as u8)
}

/// 空闲内存百分比；读不到（别的系统、命令缺）= `None`，不拦。
pub fn system_free_percent() -> Option<u8> {
    #[cfg(target_os = "macos")]
    {
        let out = Command::new("memory_pressure").output().ok()?;
        parse_memory_pressure(&String::from_utf8_lossy(&out.stdout))
    }
    #[cfg(target_os = "linux")]
    {
        parse_meminfo(&std::fs::read_to_string("/proc/meminfo").ok()?)
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        None
    }
}

// ---------- pid 文件、锁 ----------

fn read_pid(paths: &Paths) -> Option<PidRecord> {
    serde_json::from_str(&std::fs::read_to_string(&paths.pid).ok()?).ok()
}

/// 这个记录指的进程还活着、命令行还是起它时那一行。
fn owned(rec: &PidRecord) -> bool {
    !rec.cmdline.is_empty()
        && crate::sys::proc::alive(rec.pid)
        && process_cmdline(rec.pid).as_deref() == Some(rec.cmdline.as_str())
}

fn write_file(path: &Path, text: &str) -> Result<(), ServeError> {
    use std::io::Write;
    let mut f = crate::sys::fs::create_private(path).map_err(|e| ServeError::Io(e.to_string()))?;
    f.write_all(text.as_bytes())
        .map_err(|e| ServeError::Io(e.to_string()))
}

/// 两个 dct 同时想起服务时只让一个起。持有者死了、或锁老得过分，就当没人持有。
struct Lock(PathBuf);

impl Lock {
    fn try_acquire(path: &Path, stale_after: Duration) -> std::io::Result<Option<Lock>> {
        for _ in 0..2 {
            match std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(path)
            {
                Ok(mut f) => {
                    use std::io::Write;
                    let _ = write!(f, "{}", std::process::id());
                    return Ok(Some(Lock(path.to_path_buf())));
                }
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                    let holder_dead = std::fs::read_to_string(path)
                        .ok()
                        .and_then(|s| s.trim().parse::<u32>().ok())
                        .is_some_and(|p| !crate::sys::proc::alive(p));
                    let too_old = std::fs::metadata(path)
                        .and_then(|m| m.modified())
                        .ok()
                        .and_then(|m| m.elapsed().ok())
                        .is_some_and(|age| age > stale_after);
                    if !(holder_dead || too_old) {
                        return Ok(None);
                    }
                    let _ = std::fs::remove_file(path);
                }
                Err(e) => return Err(e),
            }
        }
        Ok(None)
    }
}

impl Drop for Lock {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

// ---------- up ----------

fn loopback_check(jc: &JudgeConfig) -> Result<(), ServeError> {
    let host = crate::llm::creds::host_of(&jc.base_url).unwrap_or_default();
    if is_loopback_host(&host) {
        Ok(())
    } else {
        Err(ServeError::NotLoopback(host))
    }
}

/// 没在跑就起；已经健康就什么都不做。
pub fn up(jc: &JudgeConfig, paths: &Paths, d: &Deps) -> Result<UpOutcome, ServeError> {
    let sc = jc
        .serve
        .as_ref()
        .filter(|s| !s.command.is_empty())
        .ok_or(ServeError::NotConfigured)?;
    loopback_check(jc)?;
    if healthy(&jc.base_url) {
        return Ok(UpOutcome::AlreadyHealthy);
    }
    let r = start(jc, sc, paths, d);
    match &r {
        Ok(_) => {
            let _ = std::fs::remove_file(&paths.err);
        }
        Err(e) => {
            let _ = write_file(&paths.err, &e.to_string());
        }
    }
    r
}

fn start(
    jc: &JudgeConfig,
    sc: &ServeConfig,
    paths: &Paths,
    d: &Deps,
) -> Result<UpOutcome, ServeError> {
    let timeout = Duration::from_secs(sc.ready_timeout_secs.max(1));
    let Some(_lock) = Lock::try_acquire(&paths.lock, timeout + Duration::from_secs(30))
        .map_err(|e| ServeError::Io(e.to_string()))?
    else {
        // 别人正在起：等它，不再起第二个。
        return if wait_healthy(&jc.base_url, timeout, d.poll) {
            Ok(UpOutcome::AlreadyHealthy)
        } else {
            Err(ServeError::OtherStarterTimeout {
                secs: timeout.as_secs(),
            })
        };
    };
    if healthy(&jc.base_url) {
        return Ok(UpOutcome::AlreadyHealthy);
    }
    if let Some(rec) = read_pid(paths) {
        if owned(&rec) {
            // 我们起的、还没好：只等它剩下的时间，不再起第二个。
            let left = timeout.saturating_sub(Duration::from_secs(
                now_unix().saturating_sub(rec.started_unix),
            ));
            return if wait_healthy(&jc.base_url, left, d.poll) {
                Ok(UpOutcome::AlreadyHealthy)
            } else {
                Err(ServeError::Hung { pid: rec.pid })
            };
        }
        let _ = std::fs::remove_file(&paths.pid); // 记录指向的进程已经不在了
    }
    if let Some(free) = (d.free_percent)() {
        if free < sc.min_free_percent {
            return Err(ServeError::LowMemory {
                free,
                min: sc.min_free_percent,
            });
        }
    }

    let log =
        crate::sys::fs::create_private(&paths.log).map_err(|e| ServeError::Io(e.to_string()))?;
    let log2 = log.try_clone().map_err(|e| ServeError::Io(e.to_string()))?;
    let mut cmd = Command::new(&sc.command[0]);
    cmd.args(&sc.command[1..])
        .envs(&sc.env)
        .stdin(Stdio::null())
        .stdout(log)
        .stderr(log2);
    crate::sys::proc::no_console(&mut cmd);
    let t0 = Instant::now();
    let mut child =
        crate::sys::proc::spawn_detached(&mut cmd).map_err(|e| ServeError::Spawn(e.to_string()))?;
    let pid = child.id();
    // exec 已经完成，此刻读到的就是真正的命令行（脚本会显示成「解释器 脚本」）。
    let mut cmdline = String::new();
    for _ in 0..10 {
        if let Some(c) = process_cmdline(pid) {
            cmdline = c;
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    let rec = PidRecord {
        pid,
        started_unix: now_unix(),
        cmd_hash: cmd_hash(&sc.command),
        cmdline,
    };
    write_file(&paths.pid, &serde_json::to_string(&rec).unwrap_or_default())?;
    // 回收子进程：不然它退出后留个僵尸，`alive` 会一直说它还在。
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(child.wait());
    });
    loop {
        if healthy(&jc.base_url) {
            return Ok(UpOutcome::Started {
                pid,
                secs: t0.elapsed().as_secs_f64(),
            });
        }
        if let Ok(st) = rx.try_recv() {
            // 别的 dct 可能已经改写了 pid 文件，只清自己那条。
            if read_pid(paths).is_some_and(|r| r.pid == pid) {
                let _ = std::fs::remove_file(&paths.pid);
            }
            return Err(ServeError::Exited {
                code: st.ok().and_then(|s| s.code()),
                log: paths.log.clone(),
            });
        }
        if t0.elapsed() >= timeout {
            return Err(ServeError::Timeout {
                secs: timeout.as_secs(),
                pid,
                log: paths.log.clone(),
            });
        }
        std::thread::sleep(d.poll);
    }
}

// ---------- down ----------

#[derive(Debug, Clone, PartialEq)]
pub enum DownOutcome {
    Stopped(u32),
    /// 没有 dct 起的进程；`foreign_healthy` = 但 base_url 上有别的服务在答。
    NothingToStop {
        foreign_healthy: bool,
    },
    /// pid 文件指向的进程已经不在，记录已清掉。
    StaleRemoved(u32),
    /// 这个 pid 现在是另一个进程（命令行对不上）：没动它，记录已清掉。
    Mismatch(u32),
}

/// 只停 dct 自己起的那一个。
pub fn down(jc: &JudgeConfig, paths: &Paths, d: &Deps) -> Result<DownOutcome, ServeError> {
    loopback_check(jc)?;
    let Some(rec) = read_pid(paths) else {
        return Ok(DownOutcome::NothingToStop {
            foreign_healthy: healthy(&jc.base_url),
        });
    };
    if !crate::sys::proc::alive(rec.pid) {
        let _ = std::fs::remove_file(&paths.pid);
        return Ok(DownOutcome::StaleRemoved(rec.pid));
    }
    if rec.cmdline.is_empty() {
        return Err(ServeError::CannotVerify { pid: rec.pid });
    }
    match process_cmdline(rec.pid) {
        None => return Err(ServeError::CannotVerify { pid: rec.pid }),
        Some(now) if now != rec.cmdline => {
            let _ = std::fs::remove_file(&paths.pid);
            return Ok(DownOutcome::Mismatch(rec.pid));
        }
        Some(_) => {}
    }
    crate::sys::proc::ask_to_stop(rec.pid);
    if !wait_gone(rec.pid, d.grace, d.poll) {
        crate::sys::proc::hard_kill(rec.pid);
        if !wait_gone(rec.pid, Duration::from_secs(2), d.poll) {
            return Err(ServeError::StillAlive { pid: rec.pid });
        }
    }
    let _ = std::fs::remove_file(&paths.pid);
    Ok(DownOutcome::Stopped(rec.pid))
}

fn wait_gone(pid: u32, budget: Duration, poll: Duration) -> bool {
    let t0 = Instant::now();
    while crate::sys::proc::alive(pid) {
        if t0.elapsed() >= budget {
            return false;
        }
        std::thread::sleep(poll.min(Duration::from_millis(100)));
    }
    true
}

// ---------- status ----------

#[derive(Debug, Clone, Serialize)]
pub struct Status {
    pub base_url: String,
    pub healthy: bool,
    /// 有一个 dct 起的、还活着、命令行对得上的进程。
    pub owned: bool,
    pub pid: Option<u32>,
    pub uptime_secs: Option<u64>,
    pub rss_kb: Option<u64>,
    pub log: PathBuf,
    pub last_error: Option<String>,
    pub autostart: bool,
    /// 起它之后配置里的 command 改过没有。
    pub config_changed: Option<bool>,
}

pub fn status(jc: &JudgeConfig, paths: &Paths) -> Status {
    let rec = read_pid(paths).filter(owned);
    let sc = jc.serve.as_ref();
    Status {
        base_url: jc.base_url.clone(),
        healthy: healthy(&jc.base_url),
        owned: rec.is_some(),
        pid: rec.as_ref().map(|r| r.pid),
        uptime_secs: rec
            .as_ref()
            .map(|r| now_unix().saturating_sub(r.started_unix)),
        rss_kb: rec.as_ref().and_then(|r| process_rss_kb(r.pid)),
        log: paths.log.clone(),
        last_error: std::fs::read_to_string(&paths.err)
            .ok()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty()),
        autostart: sc.is_some_and(|s| s.autostart && !s.command.is_empty()),
        config_changed: rec
            .as_ref()
            .zip(sc)
            .map(|(r, s)| r.cmd_hash != cmd_hash(&s.command)),
    }
}

// ---------- 自动启动 ----------

type Starter = dyn Fn() -> Result<UpOutcome, ServeError> + Send + Sync;

/// 第一次发现 judge 连不上（`Unavailable`）时启动一次，再重试一次；起不来就原样返回
/// 原来的错——上层照旧往下一级走。同一个进程里只试一次。
pub struct AutoStart {
    inner: Arc<dyn JudgeBackend>,
    starter: Arc<Starter>,
    tried: AtomicBool,
}

impl JudgeBackend for AutoStart {
    fn choice(&self, state: &str, q: &ChoiceQuestion) -> Result<ChoiceAnswer, JudgeError> {
        let first = self.inner.choice(state, q);
        if !matches!(first, Err(JudgeError::Unavailable(_)))
            || self.tried.swap(true, Ordering::SeqCst)
        {
            return first;
        }
        eprintln!(
            "{}",
            t(
                "[judge] unreachable; starting the local judge (this can take up to a minute)...",
                "[judge] 连不上；正在启动本机 judge（最多可能要一分钟）……"
            )
        );
        match (self.starter)() {
            Ok(_) => self.inner.choice(state, q),
            Err(e) => {
                eprintln!("[judge] {e}");
                first
            }
        }
    }
}

/// `autostart = true` 且写了 command 才包一层；否则原样返回，永远不会启动任何东西。
pub fn wrap(
    jc: &JudgeConfig,
    inner: Arc<dyn JudgeBackend>,
    starter: Arc<Starter>,
) -> Arc<dyn JudgeBackend> {
    match &jc.serve {
        Some(s) if s.autostart && !s.command.is_empty() => Arc::new(AutoStart {
            inner,
            starter,
            tried: AtomicBool::new(false),
        }),
        _ => inner,
    }
}

/// 生产用：真的 `up`。
pub fn wrap_system(jc: &JudgeConfig, inner: Arc<dyn JudgeBackend>) -> Arc<dyn JudgeBackend> {
    let jc2 = jc.clone();
    wrap(
        jc,
        inner,
        Arc::new(move || up(&jc2, &Paths::system(), &Deps::system())),
    )
}

// ---------- 命令行 ----------

const USAGE: &str =
    "usage: dct judge up | down | status [--json]\n用法：dct judge up | down | status [--json]";

pub fn run_cli(args: &[String]) -> i32 {
    let (sub, rest) = match args.split_first() {
        Some((s, r)) => (s.as_str(), r),
        None => {
            eprintln!("{USAGE}");
            return 2;
        }
    };
    let json = rest == ["--json"];
    if !rest.is_empty() && !(sub == "status" && json) {
        eprintln!("{USAGE}");
        return 2;
    }
    let paths = Paths::system();
    let cfg = match crate::explore_cli::load_config(&crate::config::config_path_for_socket(
        &crate::proto::socket_path(),
    )) {
        Ok(c) => c,
        Err(m) => {
            eprintln!("{m}");
            return 1;
        }
    };
    let Some(jc) = cfg.judge else {
        eprintln!("{}", super::NO_CONFIG_HINT);
        return 1;
    };
    let d = Deps::system();
    match sub {
        "up" => match up(&jc, &paths, &d) {
            Ok(UpOutcome::AlreadyHealthy) => {
                println!(
                    "{}",
                    t(
                        &format!(
                            "the judge is already healthy at {}; nothing started",
                            jc.base_url
                        ),
                        &format!("judge 已经在 {} 正常运行；没有再起", jc.base_url)
                    )
                );
                0
            }
            Ok(UpOutcome::Started { pid, secs }) => {
                println!(
                    "{}",
                    t(
                        &format!(
                            "the judge is up at {} (pid {pid}, ready in {secs:.1}s)",
                            jc.base_url
                        ),
                        &format!(
                            "judge 已启动：{}（进程 {pid}，{secs:.1} 秒就绪）",
                            jc.base_url
                        )
                    )
                );
                0
            }
            Err(e) => {
                eprintln!("{e}");
                1
            }
        },
        "down" => match down(&jc, &paths, &d) {
            Ok(o) => {
                let (msg, code) = match o {
                    DownOutcome::Stopped(p) => (
                        t(&format!("stopped the judge started by dct (pid {p})"), &format!("已停止 dct 起的 judge（进程 {p}）")),
                        0,
                    ),
                    DownOutcome::NothingToStop { foreign_healthy: false } => (
                        t("no judge started by dct; nothing to stop", "没有 dct 起的 judge；没有要停的"),
                        0,
                    ),
                    DownOutcome::NothingToStop { foreign_healthy: true } => (
                        t(
                            &format!("something else answers at {}; dct did not start it, so it is left alone", jc.base_url),
                            &format!("{} 上有别的服务在答；不是 dct 起的，不动它", jc.base_url),
                        ),
                        1,
                    ),
                    DownOutcome::StaleRemoved(p) => (
                        t(&format!("the judge dct started (pid {p}) is already gone; record removed"), &format!("dct 起的 judge（进程 {p}）已经不在了；记录已清掉")),
                        0,
                    ),
                    DownOutcome::Mismatch(p) => (
                        t(&format!("pid {p} is now a different process; not touching it, stale record removed"), &format!("进程 {p} 现在是另一个程序；没有动它，过期的记录已清掉")),
                        1,
                    ),
                };
                if code == 0 {
                    println!("{msg}")
                } else {
                    eprintln!("{msg}")
                }
                code
            }
            Err(e) => {
                eprintln!("{e}");
                1
            }
        },
        "status" => {
            let s = status(&jc, &paths);
            if json {
                println!("{}", serde_json::to_string(&s).unwrap_or_default());
            } else {
                print_status(&s);
            }
            if s.healthy {
                0
            } else {
                1
            }
        }
        _ => {
            eprintln!("{USAGE}");
            2
        }
    }
}

fn print_status(s: &Status) {
    let yn = |b: bool| if b { ("yes", "是") } else { ("no", "否") };
    let line = |en: String, zh: String| println!("{}", t(&en, &zh));
    line(
        format!("base_url: {}", s.base_url),
        format!("地址：{}", s.base_url),
    );
    line(
        format!("healthy: {}", yn(s.healthy).0),
        format!("健康：{}", yn(s.healthy).1),
    );
    line(
        format!("started by dct: {}", yn(s.owned).0),
        format!("dct 起的：{}", yn(s.owned).1),
    );
    if let Some(p) = s.pid {
        line(format!("pid: {p}"), format!("进程：{p}"));
    }
    if let Some(u) = s.uptime_secs {
        line(format!("uptime: {u}s"), format!("已运行：{u} 秒"));
    }
    if let Some(r) = s.rss_kb {
        line(
            format!("memory (RSS): {} MB", r / 1024),
            format!("内存（RSS）：{} MB", r / 1024),
        );
    }
    if s.config_changed == Some(true) {
        line(
            "note: [judge.serve] command changed since this server started".into(),
            "注意：这个服务启动之后 [judge.serve] command 改过了".into(),
        );
    }
    line(
        format!("autostart: {}", yn(s.autostart).0),
        format!("自动启动：{}", yn(s.autostart).1),
    );
    line(
        format!("log: {}", s.log.display()),
        format!("日志：{}", s.log.display()),
    );
    if let Some(e) = &s.last_error {
        println!("last error / 上次出错: {e}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::sync::atomic::AtomicU32;

    /// 假服务：测试二进制自己再起一份，只跑下面这个测试，监听端口、答 /health。
    /// 没有环境变量时是个空测试（普通跑测试时什么也不做）。
    #[test]
    fn fake_server_entry() {
        let Ok(port) = std::env::var("DCT_FAKE_JUDGE_PORT") else {
            return;
        };
        if let Ok(mark) = std::env::var("DCT_FAKE_JUDGE_MARK") {
            let mut f = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(mark)
                .unwrap();
            writeln!(f, "started").unwrap();
        }
        if let Ok(ms) = std::env::var("DCT_FAKE_JUDGE_DELAY_MS") {
            std::thread::sleep(Duration::from_millis(ms.parse().unwrap()));
        }
        if std::env::var("DCT_FAKE_JUDGE_EXIT").is_ok() {
            std::process::exit(3);
        }
        serve_health(
            TcpListener::bind(format!("127.0.0.1:{port}")).unwrap(),
            true,
        );
    }

    fn serve_health(l: TcpListener, forever: bool) {
        for s in l.incoming() {
            let Ok(mut s) = s else { continue };
            let mut buf = [0u8; 1024];
            let _ = s.read(&mut buf);
            let _ =
                s.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok");
            if !forever {
                break;
            }
        }
    }

    fn free_port() -> u16 {
        TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap()
            .port()
    }

    /// 进程内的健康服务（不是 dct 起的）。
    fn foreign_server() -> u16 {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let p = l.local_addr().unwrap().port();
        std::thread::spawn(move || serve_health(l, true));
        p
    }

    fn fake_cfg(port: u16, extra_env: &[(&str, &str)]) -> JudgeConfig {
        let exe = std::env::current_exe()
            .unwrap()
            .to_string_lossy()
            .to_string();
        let mut env: BTreeMap<String, String> = BTreeMap::new();
        env.insert("DCT_FAKE_JUDGE_PORT".into(), port.to_string());
        for (k, v) in extra_env {
            env.insert((*k).into(), (*v).into());
        }
        JudgeConfig {
            backend: "jev".into(),
            base_url: format!("http://127.0.0.1:{port}"),
            model: "m".into(),
            api_key_env: None,
            threshold: 0.8,
            timeout_ms: 1000,
            gate_tiers: None,
            model_veto: None,
            serve: Some(ServeConfig {
                command: vec![
                    exe,
                    "--exact".into(),
                    "judge::serve::tests::fake_server_entry".into(),
                    "--test-threads=1".into(),
                ],
                env,
                ready_timeout_secs: 20,
                autostart: false,
                min_free_percent: 15,
            }),
        }
    }

    fn deps(free: &dyn Fn() -> Option<u8>) -> Deps<'_> {
        Deps {
            free_percent: free,
            poll: Duration::from_millis(20),
            grace: Duration::from_secs(3),
        }
    }
    const PLENTY: &dyn Fn() -> Option<u8> = &|| Some(80);

    fn tmp() -> (tempfile::TempDir, Paths) {
        let d = tempfile::tempdir().unwrap();
        let p = Paths::in_dir(d.path());
        (d, p)
    }

    #[test]
    fn start_status_stop_cycle_touches_only_its_own_process() {
        let (_d, paths) = tmp();
        let jc = fake_cfg(free_port(), &[]);
        let UpOutcome::Started { pid, .. } = up(&jc, &paths, &deps(PLENTY)).unwrap() else {
            panic!("should have started")
        };
        let rec = read_pid(&paths).unwrap();
        assert_eq!(rec.pid, pid);
        assert!(rec.cmdline.contains("fake_server_entry"), "{}", rec.cmdline);
        assert!(healthy(&jc.base_url));
        // 日志 0600
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let m = std::fs::metadata(&paths.log).unwrap().permissions().mode();
            assert_eq!(m & 0o777, 0o600);
        }
        let s = status(&jc, &paths);
        assert!(s.healthy && s.owned && s.pid == Some(pid) && s.uptime_secs.is_some());
        assert_eq!(s.config_changed, Some(false));
        // 再 up：已经健康，不再起
        assert_eq!(
            up(&jc, &paths, &deps(PLENTY)).unwrap(),
            UpOutcome::AlreadyHealthy
        );
        assert_eq!(read_pid(&paths).unwrap().pid, pid);

        assert_eq!(
            down(&jc, &paths, &deps(PLENTY)).unwrap(),
            DownOutcome::Stopped(pid)
        );
        assert!(!crate::sys::proc::alive(pid));
        assert!(!paths.pid.exists());
        assert!(!healthy(&jc.base_url));
    }

    #[test]
    fn an_already_healthy_server_is_never_spawned_over_or_stopped() {
        let (_d, paths) = tmp();
        let port = foreign_server();
        let mut jc = fake_cfg(port, &[]);
        jc.serve.as_mut().unwrap().command = vec!["/definitely/not/there".into()];
        assert_eq!(
            up(&jc, &paths, &deps(PLENTY)).unwrap(),
            UpOutcome::AlreadyHealthy
        );
        assert!(!paths.pid.exists() && !paths.log.exists());
        assert_eq!(
            down(&jc, &paths, &deps(PLENTY)).unwrap(),
            DownOutcome::NothingToStop {
                foreign_healthy: true
            }
        );
        assert!(healthy(&jc.base_url), "the foreign server must still be up");
    }

    #[test]
    fn non_loopback_base_url_is_refused_for_up_and_down() {
        let (_d, paths) = tmp();
        let mut jc = fake_cfg(free_port(), &[]);
        jc.base_url = "http://192.0.2.1:8796".into();
        assert!(
            matches!(up(&jc, &paths, &deps(PLENTY)), Err(ServeError::NotLoopback(h)) if h == "192.0.2.1")
        );
        assert!(matches!(
            down(&jc, &paths, &deps(PLENTY)),
            Err(ServeError::NotLoopback(_))
        ));
        for ok in ["http://127.0.0.1:1", "http://localhost:1", "http://[::1]:1"] {
            jc.base_url = ok.into();
            assert!(loopback_check(&jc).is_ok(), "{ok}");
        }
        assert!(!paths.pid.exists());
    }

    #[test]
    fn low_memory_refuses_and_unknown_memory_does_not() {
        let (_d, paths) = tmp();
        let jc = fake_cfg(free_port(), &[]);
        let low: &dyn Fn() -> Option<u8> = &|| Some(5);
        let e = up(&jc, &paths, &deps(low)).unwrap_err();
        assert_eq!(e, ServeError::LowMemory { free: 5, min: 15 });
        assert!(!paths.pid.exists());
        assert!(status(&jc, &paths).last_error.unwrap().contains("5%"));
        // 读不到内存 = 不拦
        let unknown: &dyn Fn() -> Option<u8> = &|| None;
        assert!(matches!(
            up(&jc, &paths, &deps(unknown)),
            Ok(UpOutcome::Started { .. })
        ));
        down(&jc, &paths, &deps(PLENTY)).unwrap();
        assert!(!paths.err.exists() || status(&jc, &paths).last_error.is_none());
    }

    fn dead_pid() -> u32 {
        let mut c = Command::new("true").spawn().unwrap();
        let pid = c.id();
        c.wait().unwrap();
        pid
    }

    #[test]
    fn a_pid_file_with_a_dead_pid_is_cleaned_up_by_down_and_replaced_by_up() {
        let (_d, paths) = tmp();
        let jc = fake_cfg(free_port(), &[]);
        let stale = PidRecord {
            pid: dead_pid(),
            started_unix: now_unix(),
            cmd_hash: "x".into(),
            cmdline: "gone".into(),
        };
        write_file(&paths.pid, &serde_json::to_string(&stale).unwrap()).unwrap();
        assert!(!status(&jc, &paths).owned);
        assert_eq!(
            down(&jc, &paths, &deps(PLENTY)).unwrap(),
            DownOutcome::StaleRemoved(stale.pid)
        );
        assert!(!paths.pid.exists());

        write_file(&paths.pid, &serde_json::to_string(&stale).unwrap()).unwrap();
        let UpOutcome::Started { pid, .. } = up(&jc, &paths, &deps(PLENTY)).unwrap() else {
            panic!()
        };
        assert_eq!(read_pid(&paths).unwrap().pid, pid);
        down(&jc, &paths, &deps(PLENTY)).unwrap();
    }

    #[test]
    fn a_live_pid_with_a_different_command_line_is_not_killed() {
        let (_d, paths) = tmp();
        let jc = fake_cfg(free_port(), &[]);
        let mut other = Command::new("sleep").arg("30").spawn().unwrap();
        let rec = PidRecord {
            pid: other.id(),
            started_unix: now_unix(),
            cmd_hash: "x".into(),
            cmdline: "some other thing --flag".into(),
        };
        write_file(&paths.pid, &serde_json::to_string(&rec).unwrap()).unwrap();
        assert_eq!(
            down(&jc, &paths, &deps(PLENTY)).unwrap(),
            DownOutcome::Mismatch(rec.pid)
        );
        assert!(
            other.try_wait().unwrap().is_none(),
            "the unrelated process must survive"
        );
        // 命令行记录是空的（当初没读到）：同样不杀
        let blank = PidRecord {
            cmdline: String::new(),
            ..rec
        };
        write_file(&paths.pid, &serde_json::to_string(&blank).unwrap()).unwrap();
        assert!(matches!(
            down(&jc, &paths, &deps(PLENTY)),
            Err(ServeError::CannotVerify { .. })
        ));
        assert!(other.try_wait().unwrap().is_none());
        other.kill().unwrap();
        other.wait().unwrap();
    }

    #[test]
    fn concurrent_starters_spawn_exactly_one_server() {
        let (_d, paths) = tmp();
        let mark = _d.path().join("mark");
        let jc = fake_cfg(
            free_port(),
            &[
                ("DCT_FAKE_JUDGE_MARK", mark.to_str().unwrap()),
                ("DCT_FAKE_JUDGE_DELAY_MS", "400"),
            ],
        );
        let (jc, paths) = (Arc::new(jc), Arc::new(paths));
        let hs: Vec<_> = (0..3)
            .map(|_| {
                let (jc, paths) = (jc.clone(), paths.clone());
                std::thread::spawn(move || up(&jc, &paths, &deps(PLENTY)))
            })
            .collect();
        let rs: Vec<_> = hs.into_iter().map(|h| h.join().unwrap().unwrap()).collect();
        assert_eq!(
            rs.iter()
                .filter(|r| matches!(r, UpOutcome::Started { .. }))
                .count(),
            1,
            "{rs:?}"
        );
        assert_eq!(std::fs::read_to_string(&mark).unwrap().lines().count(), 1);
        assert!(!paths.lock.exists(), "lock released");
        down(&jc, &paths, &deps(PLENTY)).unwrap();
    }

    #[test]
    fn a_stale_lock_from_a_dead_process_does_not_block() {
        let (_d, paths) = tmp();
        std::fs::write(&paths.lock, dead_pid().to_string()).unwrap();
        let l = Lock::try_acquire(&paths.lock, Duration::from_secs(600)).unwrap();
        assert!(l.is_some());
        // 持有者活着（就是我们自己）时拿不到
        assert!(Lock::try_acquire(&paths.lock, Duration::from_secs(600))
            .unwrap()
            .is_none());
    }

    #[test]
    fn a_start_that_times_out_reports_it_and_leaves_the_process_to_down() {
        let (_d, paths) = tmp();
        let mut jc = fake_cfg(free_port(), &[("DCT_FAKE_JUDGE_DELAY_MS", "30000")]);
        jc.serve.as_mut().unwrap().ready_timeout_secs = 1;
        let e = up(&jc, &paths, &deps(PLENTY)).unwrap_err();
        let ServeError::Timeout { pid, .. } = e else {
            panic!("{e:?}")
        };
        assert!(crate::sys::proc::alive(pid));
        assert!(status(&jc, &paths).last_error.is_some());
        // 再来一次：不会起第二个，而是认出这是我们的、说它不健康
        assert!(
            matches!(up(&jc, &paths, &deps(PLENTY)), Err(ServeError::Hung { pid: p }) if p == pid)
        );
        assert_eq!(
            down(&jc, &paths, &deps(PLENTY)).unwrap(),
            DownOutcome::Stopped(pid)
        );
    }

    #[test]
    fn a_server_that_exits_immediately_is_an_error_and_leaves_no_pid_file() {
        let (_d, paths) = tmp();
        let jc = fake_cfg(free_port(), &[("DCT_FAKE_JUDGE_EXIT", "1")]);
        let e = up(&jc, &paths, &deps(PLENTY)).unwrap_err();
        assert!(
            matches!(e, ServeError::Exited { code: Some(3), .. }),
            "{e:?}"
        );
        assert!(!paths.pid.exists());
        let mut bad = jc.clone();
        bad.serve.as_mut().unwrap().command = vec!["/definitely/not/there".into()];
        assert!(matches!(
            up(&bad, &paths, &deps(PLENTY)),
            Err(ServeError::Spawn(_))
        ));
    }

    #[test]
    fn without_a_command_nothing_is_ever_started() {
        let (_d, paths) = tmp();
        let mut jc = fake_cfg(free_port(), &[]);
        jc.serve.as_mut().unwrap().command.clear();
        assert_eq!(
            up(&jc, &paths, &deps(PLENTY)),
            Err(ServeError::NotConfigured)
        );
        jc.serve = None;
        assert_eq!(
            up(&jc, &paths, &deps(PLENTY)),
            Err(ServeError::NotConfigured)
        );
    }

    // ----- 自动启动 -----

    struct Flaky {
        calls: AtomicU32,
        /// 第几次调用起才成功（1 = 一开始就成功）。
        ok_from: u32,
    }
    impl JudgeBackend for Flaky {
        fn choice(&self, _: &str, q: &ChoiceQuestion) -> Result<ChoiceAnswer, JudgeError> {
            let n = self.calls.fetch_add(1, Ordering::SeqCst) + 1;
            if n >= self.ok_from {
                Ok(ChoiceAnswer {
                    choice: q.criteria[0].0.clone(),
                    probabilities: vec![],
                    answer_confidence: 1.0,
                })
            } else {
                Err(JudgeError::Unavailable("refused".into()))
            }
        }
    }
    fn q() -> ChoiceQuestion {
        ChoiceQuestion {
            instructions: "i".into(),
            criteria: vec![("a".into(), None), ("b".into(), None)],
        }
    }
    fn counting(res: Result<UpOutcome, ServeError>) -> (Arc<Starter>, Arc<AtomicU32>) {
        let n = Arc::new(AtomicU32::new(0));
        let n2 = n.clone();
        (
            Arc::new(move || {
                n2.fetch_add(1, Ordering::SeqCst);
                res.clone()
            }),
            n,
        )
    }
    fn auto_cfg(autostart: bool) -> JudgeConfig {
        let mut jc = fake_cfg(1, &[]);
        jc.serve.as_mut().unwrap().autostart = autostart;
        jc
    }

    #[test]
    fn autostart_false_or_missing_never_starts_anything() {
        let (starter, n) = counting(Ok(UpOutcome::AlreadyHealthy));
        let inner = Arc::new(Flaky {
            calls: AtomicU32::new(0),
            ok_from: 99,
        });
        let w = wrap(&auto_cfg(false), inner.clone(), starter.clone());
        assert!(w.choice("s", &q()).is_err());
        let mut none = auto_cfg(true);
        none.serve = None;
        assert!(wrap(&none, inner.clone(), starter.clone())
            .choice("s", &q())
            .is_err());
        let mut nocmd = auto_cfg(true);
        nocmd.serve.as_mut().unwrap().command.clear();
        assert!(wrap(&nocmd, inner, starter).choice("s", &q()).is_err());
        assert_eq!(n.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn autostart_true_starts_once_and_retries_once() {
        let (starter, n) = counting(Ok(UpOutcome::Started { pid: 1, secs: 0.1 }));
        let inner = Arc::new(Flaky {
            calls: AtomicU32::new(0),
            ok_from: 2,
        });
        let w = wrap(&auto_cfg(true), inner.clone(), starter);
        assert_eq!(w.choice("s", &q()).unwrap().choice, "a");
        assert_eq!(
            (n.load(Ordering::SeqCst), inner.calls.load(Ordering::SeqCst)),
            (1, 2)
        );
        // 已经好了：后面的调用不再启动
        assert!(w.choice("s", &q()).is_ok());
        assert_eq!(n.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn autostart_that_cannot_start_falls_through_like_today_and_does_not_retry_storm() {
        for err in [
            ServeError::LowMemory { free: 1, min: 15 },
            ServeError::Timeout {
                secs: 1,
                pid: 1,
                log: "x".into(),
            },
        ] {
            let (starter, n) = counting(Err(err));
            let inner = Arc::new(Flaky {
                calls: AtomicU32::new(0),
                ok_from: 99,
            });
            let w = wrap(&auto_cfg(true), inner.clone(), starter);
            assert!(
                matches!(w.choice("s", &q()), Err(JudgeError::Unavailable(m)) if m == "refused")
            );
            assert!(w.choice("s", &q()).is_err());
            assert_eq!(
                (n.load(Ordering::SeqCst), inner.calls.load(Ordering::SeqCst)),
                (1, 2),
                "one start attempt, no inner retry after failed start"
            );
        }
    }

    #[test]
    fn only_unreachable_triggers_a_start() {
        struct Slow;
        impl JudgeBackend for Slow {
            fn choice(&self, _: &str, _: &ChoiceQuestion) -> Result<ChoiceAnswer, JudgeError> {
                Err(JudgeError::Timeout)
            }
        }
        let (starter, n) = counting(Ok(UpOutcome::AlreadyHealthy));
        let w = wrap(&auto_cfg(true), Arc::new(Slow), starter);
        assert_eq!(w.choice("s", &q()), Err(JudgeError::Timeout));
        assert_eq!(n.load(Ordering::SeqCst), 0);
    }

    // ----- 读数、文案、配置 -----

    #[test]
    fn memory_readings_parse() {
        assert_eq!(
            parse_memory_pressure("x\nSystem-wide memory free percentage: 36%\n"),
            Some(36)
        );
        assert_eq!(parse_memory_pressure("nope"), None);
        assert_eq!(
            parse_meminfo("MemTotal:  1000 kB\nMemFree: 1 kB\nMemAvailable:  250 kB\n"),
            Some(25)
        );
        assert_eq!(parse_meminfo("MemTotal: 0 kB\nMemAvailable: 0 kB\n"), None);
        assert_eq!(parse_meminfo("garbage"), None);
    }

    #[test]
    fn every_error_has_both_languages() {
        let all = [
            ServeError::NotConfigured,
            ServeError::NotLoopback("h".into()),
            ServeError::LowMemory { free: 1, min: 2 },
            ServeError::Spawn("e".into()),
            ServeError::Exited {
                code: None,
                log: "l".into(),
            },
            ServeError::Timeout {
                secs: 1,
                pid: 2,
                log: "l".into(),
            },
            ServeError::Hung { pid: 1 },
            ServeError::OtherStarterTimeout { secs: 1 },
            ServeError::Io("e".into()),
            ServeError::CannotVerify { pid: 1 },
            ServeError::StillAlive { pid: 1 },
        ];
        for e in all {
            let s = e.to_string();
            assert!(
                s.contains(" / ")
                    && s.chars().any(|c| c > '\u{2e80}')
                    && s.chars().any(|c| c.is_ascii_alphabetic()),
                "{s}"
            );
        }
    }

    #[test]
    fn serve_config_parses_with_defaults_and_is_off_when_absent() {
        let c = crate::config::Config::from_toml("[judge]\nbase_url = \"http://127.0.0.1:1\"\n")
            .unwrap();
        assert!(c.judge.unwrap().serve.is_none());
        let c = crate::config::Config::from_toml(
            "[judge]\nbase_url = \"http://127.0.0.1:1\"\n[judge.serve]\ncommand = [\"/x\", \"a b\"]\nenv = { K = \"v\" }\nidle_stop_minutes = 5\n",
        )
        .unwrap();
        let s = c.judge.unwrap().serve.unwrap();
        assert_eq!(s.command, ["/x", "a b"]);
        assert_eq!(
            (s.ready_timeout_secs, s.autostart, s.min_free_percent),
            (60, false, 15)
        );
        assert_eq!(s.env["K"], "v");
    }

    #[test]
    fn health_url_ignores_a_systemone_suffix_and_trailing_slash() {
        assert_eq!(
            health_url("http://127.0.0.1:8796/"),
            "http://127.0.0.1:8796/health"
        );
        assert_eq!(
            health_url("http://127.0.0.1:8796/v1/systemone"),
            "http://127.0.0.1:8796/health"
        );
    }
}
