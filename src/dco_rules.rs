//! 把 `dco rules --json` 的输出原样存成 `~/.dct/dco-rules.json`，给 dc-vault 的「规矩总表」页面读。
//!
//! 约定（dcv 那边定的）：
//! - 内容**一字不改**就是 dco 的输出；`schema_version` 由 dco 自己带，这里只核对是 1。
//! - 原子写：同目录先写临时文件，再改名。页面永远读不到半截文件。
//! - 什么时候写：守护进程启动时、`dct dco-rules` 手动刷新时、dco 换了版本时。
//! - 上限：文件 ≤ 1 MB、类别 ≤ 50、每类条目 ≤ 200。超了就**不写**，旧文件留着，说清楚为什么。
//! - dco 没装、跑失败、输出不是 JSON：都不写、不崩、不挡守护进程启动（在后台线程里跑，带超时）。
//! - 文件不存在页面自己会处理，dct 从来不删它。
//!
//! 跑 dco 这件事通过 [`Runner`] 注入，测试不需要真的 dco。

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// 文件最大多大（字节）。
pub const MAX_BYTES: usize = 1024 * 1024;
/// 最多多少类。
pub const MAX_CATEGORIES: usize = 50;
/// 每类最多多少条。
pub const MAX_ITEMS_PER_CATEGORY: usize = 200;
/// 页面认的 schema 版本。
pub const SCHEMA_VERSION: u64 = 1;
/// 每次跑 dco 最多等多久。
pub const TIMEOUT: Duration = Duration::from_secs(10);
/// 守护进程隔多久看一次 dco 换没换版本。
pub const CHECK_EVERY: Duration = Duration::from_secs(5 * 60);

/// 文件放哪：跟 socket 同一个目录（真守护进程就是 `~/.dct/`；测试的 socket 在临时目录，自动隔离）。
pub fn path_for_socket(socket: &Path) -> PathBuf {
    socket
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_default()
        .join("dco-rules.json")
}

/// 跑一次 dco 失败的几种样子。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RunError {
    NotInstalled,
    TimedOut,
    /// 退出码不是 0（或者根本起不来）。
    Failed(String),
    /// 输出比 [`MAX_BYTES`] 还大，读到一半就不读了。
    TooBig,
}

/// 怎么跑 dco。真的那份见 [`SystemRunner`]；测试里换成假的。
pub trait Runner {
    /// 跑 `dco <args>`，返回 stdout。输出超过 `max_bytes` 时返回 [`RunError::TooBig`]。
    fn run(&self, args: &[&str], timeout: Duration, max_bytes: usize) -> Result<Vec<u8>, RunError>;
}

/// 为什么没写。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Refused {
    Run(RunError),
    NotJson,
    BadShape(String),
    WrongSchema(String),
    TooManyCategories(usize),
    TooManyItems { category: String, count: usize },
    CannotWrite(String),
}

impl Refused {
    /// 给人看的一句话。
    pub fn message(&self) -> String {
        match self {
            Refused::Run(RunError::NotInstalled) => "这台电脑上没装 dco".to_string(),
            Refused::Run(RunError::TimedOut) => {
                format!("dco 超过 {} 秒没回话", TIMEOUT.as_secs())
            }
            Refused::Run(RunError::Failed(why)) => format!("dco 跑失败了（{why}）"),
            Refused::Run(RunError::TooBig) => {
                format!("dco 给的规矩超过 {} KB，太大了", MAX_BYTES / 1024)
            }
            Refused::NotJson => "dco 给的内容不是正常的 JSON".to_string(),
            Refused::BadShape(why) => format!("dco 给的内容格式不对（{why}）"),
            Refused::WrongSchema(v) => {
                format!("dco 给的格式版本是 {v}，这一版 dct 只认 {SCHEMA_VERSION}")
            }
            Refused::TooManyCategories(n) => {
                format!("dco 给了 {n} 类规矩，超过上限 {MAX_CATEGORIES} 类")
            }
            Refused::TooManyItems { category, count } => {
                format!("「{category}」这一类有 {count} 条，超过上限 {MAX_ITEMS_PER_CATEGORY} 条")
            }
            Refused::CannotWrite(why) => format!("写不进文件（{why}）"),
        }
    }
}

/// 写成功以后的小结。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Summary {
    pub categories: usize,
    pub items: usize,
    pub dco_version: Option<String>,
}

/// 核对 dco 的输出：是 JSON、`schema_version` 是 1、没超上限。**只看不改。**
pub fn check(bytes: &[u8]) -> Result<Summary, Refused> {
    if bytes.len() > MAX_BYTES {
        return Err(Refused::Run(RunError::TooBig));
    }
    let v: serde_json::Value = serde_json::from_slice(bytes).map_err(|_| Refused::NotJson)?;
    let obj = v
        .as_object()
        .ok_or_else(|| Refused::BadShape("最外层不是对象".into()))?;
    match obj.get("schema_version") {
        Some(s) if s.as_u64() == Some(SCHEMA_VERSION) => {}
        Some(s) => return Err(Refused::WrongSchema(s.to_string())),
        None => return Err(Refused::WrongSchema("（没写）".into())),
    }
    let cats = obj
        .get("categories")
        .and_then(|c| c.as_array())
        .ok_or_else(|| Refused::BadShape("没有 categories 列表".into()))?;
    if cats.len() > MAX_CATEGORIES {
        return Err(Refused::TooManyCategories(cats.len()));
    }
    let mut items = 0;
    for (i, c) in cats.iter().enumerate() {
        let name = c
            .get("title")
            .or_else(|| c.get("id"))
            .and_then(|t| t.as_str())
            .map(str::to_string)
            .unwrap_or_else(|| format!("第 {} 类", i + 1));
        let list = c
            .get("items")
            .and_then(|x| x.as_array())
            .ok_or_else(|| Refused::BadShape(format!("「{name}」没有 items 列表")))?;
        if list.len() > MAX_ITEMS_PER_CATEGORY {
            return Err(Refused::TooManyItems {
                category: name,
                count: list.len(),
            });
        }
        items += list.len();
    }
    Ok(Summary {
        categories: cats.len(),
        items,
        dco_version: obj
            .get("dco_version")
            .and_then(|s| s.as_str())
            .map(str::to_string),
    })
}

/// 原子写：同目录临时文件 → 改名。失败时不留临时文件，旧文件不动。
pub fn write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let dir = path.parent().unwrap_or(Path::new("."));
    std::fs::create_dir_all(dir)?;
    let tmp = path.with_extension("json.tmp");
    let result = (|| -> std::io::Result<()> {
        let mut f = std::fs::File::create(&tmp)?;
        f.write_all(bytes)?;
        f.sync_all()?;
        // 先关句柄再改名，Windows 上才改得动。
        drop(f);
        std::fs::rename(&tmp, path)
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    result
}

/// 跑一次 `dco rules --json`，核对通过就原样写进 `path`。
pub fn refresh(runner: &dyn Runner, path: &Path) -> Result<Summary, Refused> {
    let out = runner
        .run(&["rules", "--json"], TIMEOUT, MAX_BYTES)
        .map_err(Refused::Run)?;
    let summary = check(&out)?;
    write_atomic(path, &out).map_err(|e| Refused::CannotWrite(e.to_string()))?;
    Ok(summary)
}

/// 问 dco 是哪一版（`dco --version` 的那一行）。问不出就是 `None`。
pub fn version(runner: &dyn Runner) -> Option<String> {
    let out = runner.run(&["--version"], TIMEOUT, 4096).ok()?;
    let s = String::from_utf8_lossy(&out).trim().to_string();
    (!s.is_empty()).then_some(s)
}

/// 守护进程里那个「dco 换版本就重写」的判断。
///
/// 记的是**上次写成功时**的 dco 版本：失败了不记，下一轮还会再试；版本没变就什么都不做。
/// 刚起来的时候什么都没记，所以第一轮一定会写——这就是「启动时写一次」。
#[derive(Debug, Default)]
pub struct Watcher {
    written_for: Option<String>,
}

impl Watcher {
    /// 看一眼；该写就写。没动手返回 `None`，动了手返回结果。
    pub fn tick(&mut self, runner: &dyn Runner, path: &Path) -> Option<Result<Summary, Refused>> {
        // dco 没装（或者问不出版本）：这一轮什么都不做，等它装上了版本就「变了」。
        let now = version(runner)?;
        if self.written_for.as_deref() == Some(now.as_str()) {
            return None;
        }
        let r = refresh(runner, path);
        if r.is_ok() {
            self.written_for = Some(now);
        }
        Some(r)
    }
}

/// 一句话结果，CLI 和守护进程日志共用。
pub fn describe(r: &Result<Summary, Refused>) -> String {
    match r {
        Ok(s) => {
            let ver = s
                .dco_version
                .as_deref()
                .map(|v| format!("（dco {v}）"))
                .unwrap_or_default();
            format!("规矩总表已更新：{} 类、{} 条{ver}", s.categories, s.items)
        }
        Err(e) => format!("规矩总表没更新：{}。旧文件留着没动", e.message()),
    }
}

/// 真的去跑本机的 dco。
pub struct SystemRunner {
    pub home: Option<PathBuf>,
}

impl SystemRunner {
    pub fn new() -> Self {
        SystemRunner {
            home: crate::sys::home(),
        }
    }

    /// 先在 PATH 里找；找不到再看 `~/.local/bin/dco`（dco 的安装位置）——
    /// 从图形界面拉起的守护进程，PATH 里常常没有 `~/.local/bin`。
    fn locate(&self) -> Option<PathBuf> {
        if let Some(p) = crate::sys::fs::find_command("dco") {
            return Some(p);
        }
        let p = self.home.as_ref()?.join(".local").join("bin").join("dco");
        p.is_file().then_some(p)
    }
}

impl Default for SystemRunner {
    fn default() -> Self {
        Self::new()
    }
}

impl Runner for SystemRunner {
    fn run(&self, args: &[&str], timeout: Duration, max_bytes: usize) -> Result<Vec<u8>, RunError> {
        let exe = self.locate().ok_or(RunError::NotInstalled)?;
        run_with_timeout(&exe, args, timeout, max_bytes)
    }
}

/// 跑一个程序，最多等 `timeout`，stdout 最多收 `max_bytes`。超时就杀掉。
///
/// stdout 在另一个线程里读：dco 的输出有上百 KB，比管道缓冲大，不边跑边读它会卡住写不完。
pub fn run_with_timeout(
    exe: &Path,
    args: &[&str],
    timeout: Duration,
    max_bytes: usize,
) -> Result<Vec<u8>, RunError> {
    let mut child = std::process::Command::new(exe)
        .args(args)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                RunError::NotInstalled
            } else {
                RunError::Failed(e.to_string())
            }
        })?;
    let stdout = child.stdout.take().expect("stdout 是 piped");
    let cap = max_bytes as u64 + 1;
    let reader = std::thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = stdout.take(cap).read_to_end(&mut buf);
        buf
    });

    let deadline = Instant::now() + timeout;
    let status = loop {
        match child.try_wait() {
            Ok(Some(st)) => break st,
            Ok(None) if Instant::now() >= deadline => {
                let _ = child.kill();
                let _ = child.wait();
                let _ = reader.join();
                return Err(RunError::TimedOut);
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(50)),
            Err(e) => {
                let _ = child.kill();
                let _ = child.wait();
                let _ = reader.join();
                return Err(RunError::Failed(e.to_string()));
            }
        }
    };
    let buf = reader.join().unwrap_or_default();
    if buf.len() > max_bytes {
        return Err(RunError::TooBig);
    }
    if !status.success() {
        return Err(RunError::Failed(match status.code() {
            Some(c) => format!("退出码 {c}"),
            None => "被信号杀掉".to_string(),
        }));
    }
    Ok(buf)
}

/// `dct dco-rules`：手动刷新一次，印一行结果。成功退 0，没写退 1。
pub fn run_cli(args: &[String]) -> i32 {
    if !args.is_empty() {
        eprintln!("用法：dct dco-rules");
        return 2;
    }
    let path = path_for_socket(&crate::proto::socket_path());
    let r = refresh(&SystemRunner::new(), &path);
    let line = describe(&r);
    if r.is_ok() {
        println!("{line}");
        0
    } else {
        eprintln!("{line}");
        1
    }
}

/// 守护进程里那条后台线程：起来先写一次，以后每 [`CHECK_EVERY`] 看一眼 dco 换没换版本。
/// 同一句话不重复记，免得 dco 没装时每五分钟记一遍。
pub fn spawn_watcher(path: PathBuf) {
    let _ = std::thread::Builder::new()
        .name("dco-rules".into())
        .spawn(move || {
            let runner = SystemRunner::new();
            let mut w = Watcher::default();
            let mut last_line = String::new();
            loop {
                if let Some(r) = w.tick(&runner, &path) {
                    let line = describe(&r);
                    if line != last_line {
                        eprintln!("dct：{line}");
                        last_line = line;
                    }
                }
                std::thread::sleep(CHECK_EVERY);
            }
        });
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    /// 假的 dco：按参数给事先准备好的回答，并记下被问过什么。
    struct Fake {
        version: RefCell<Result<Vec<u8>, RunError>>,
        rules: RefCell<Result<Vec<u8>, RunError>>,
        calls: RefCell<Vec<String>>,
    }

    impl Fake {
        fn new(version: &str, rules: &str) -> Self {
            Fake {
                version: RefCell::new(Ok(version.as_bytes().to_vec())),
                rules: RefCell::new(Ok(rules.as_bytes().to_vec())),
                calls: RefCell::new(vec![]),
            }
        }
        fn rules_calls(&self) -> usize {
            self.calls
                .borrow()
                .iter()
                .filter(|c| *c == "rules --json")
                .count()
        }
    }

    impl Runner for Fake {
        fn run(&self, args: &[&str], _t: Duration, max: usize) -> Result<Vec<u8>, RunError> {
            let key = args.join(" ");
            self.calls.borrow_mut().push(key.clone());
            let r = if key == "--version" {
                self.version.borrow().clone()
            } else {
                self.rules.borrow().clone()
            };
            match r {
                Ok(b) if b.len() > max => Err(RunError::TooBig),
                other => other,
            }
        }
    }

    fn rules_json(cats: usize, items: usize) -> String {
        let cats: Vec<_> = (0..cats)
            .map(|c| {
                let items: Vec<_> = (0..items)
                    .map(|i| serde_json::json!({"id": format!("i{i}"), "title": "t"}))
                    .collect();
                serde_json::json!({"id": format!("c{c}"), "title": format!("类{c}"), "items": items})
            })
            .collect();
        serde_json::json!({"schema_version": 1, "dco_version": "0.1.0", "categories": cats})
            .to_string()
    }

    fn tmp() -> (tempfile::TempDir, PathBuf) {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("dco-rules.json");
        (d, p)
    }

    #[test]
    fn file_sits_next_to_the_socket() {
        assert_eq!(
            path_for_socket(Path::new("/h/.dct/daemon.sock")),
            PathBuf::from("/h/.dct/dco-rules.json")
        );
    }

    #[test]
    fn writes_dco_output_byte_for_byte() {
        let (_d, p) = tmp();
        // 故意带着奇怪的缩进和键序：写下去的必须一字不差，不能被重新排版。
        let raw = "{ \"schema_version\":1,\n  \"categories\":[{\"id\":\"x\",\"items\":[{\"id\":\"a\"}]}], \"z\":0 }\n";
        let s = refresh(&Fake::new("dco 0.1.0", raw), &p).unwrap();
        assert_eq!(std::fs::read_to_string(&p).unwrap(), raw);
        assert_eq!((s.categories, s.items), (1, 1));
    }

    #[test]
    fn limits_are_enforced_and_old_file_is_kept() {
        let cases = [
            rules_json(MAX_CATEGORIES + 1, 1),
            rules_json(1, MAX_ITEMS_PER_CATEGORY + 1),
        ];
        for bad in cases {
            let (_d, p) = tmp();
            std::fs::write(&p, "OLD").unwrap();
            let r = refresh(&Fake::new("v", &bad), &p);
            assert!(r.is_err(), "超上限必须拒绝");
            assert_eq!(std::fs::read_to_string(&p).unwrap(), "OLD");
        }
        // 刚好到上限的可以写。
        let (_d, p) = tmp();
        assert!(refresh(&Fake::new("v", &rules_json(MAX_CATEGORIES, 1)), &p).is_ok());
        assert!(refresh(&Fake::new("v", &rules_json(1, MAX_ITEMS_PER_CATEGORY)), &p).is_ok());
    }

    #[test]
    fn too_big_output_is_refused() {
        let (_d, p) = tmp();
        std::fs::write(&p, "OLD").unwrap();
        let pad = "x".repeat(MAX_BYTES);
        let big = format!("{{\"schema_version\":1,\"categories\":[],\"pad\":\"{pad}\"}}");
        let r = refresh(&Fake::new("v", &big), &p);
        assert_eq!(r, Err(Refused::Run(RunError::TooBig)));
        assert_eq!(std::fs::read_to_string(&p).unwrap(), "OLD");
        // 不经 runner 直接给 check 也一样拦。
        assert_eq!(check(big.as_bytes()), Err(Refused::Run(RunError::TooBig)));
    }

    #[test]
    fn invalid_json_wrong_schema_and_bad_shape_are_refused() {
        for (bad, want) in [
            ("not json", Refused::NotJson),
            ("[1]", Refused::BadShape("最外层不是对象".into())),
            (
                "{\"categories\":[]}",
                Refused::WrongSchema("（没写）".into()),
            ),
            (
                "{\"schema_version\":2,\"categories\":[]}",
                Refused::WrongSchema("2".into()),
            ),
            (
                "{\"schema_version\":1}",
                Refused::BadShape("没有 categories 列表".into()),
            ),
        ] {
            let (_d, p) = tmp();
            assert_eq!(refresh(&Fake::new("v", bad), &p), Err(want));
            assert!(!p.exists(), "没通过就不能写");
        }
    }

    #[test]
    fn missing_or_failing_dco_writes_nothing() {
        for e in [
            RunError::NotInstalled,
            RunError::TimedOut,
            RunError::Failed("退出码 1".into()),
        ] {
            let (_d, p) = tmp();
            let f = Fake::new("v", "");
            *f.rules.borrow_mut() = Err(e.clone());
            assert_eq!(refresh(&f, &p), Err(Refused::Run(e)));
            assert!(!p.exists());
        }
    }

    #[test]
    fn atomic_write_leaves_no_temp_file_and_replaces_old() {
        let (d, p) = tmp();
        std::fs::write(&p, "OLD").unwrap();
        write_atomic(&p, b"NEW").unwrap();
        assert_eq!(std::fs::read_to_string(&p).unwrap(), "NEW");
        let names: Vec<_> = std::fs::read_dir(d.path())
            .unwrap()
            .map(|e| e.unwrap().file_name().into_string().unwrap())
            .collect();
        assert_eq!(names, vec!["dco-rules.json".to_string()]);
    }

    #[test]
    fn atomic_write_failure_keeps_old_file_and_cleans_temp() {
        let (d, p) = tmp();
        std::fs::write(&p, "OLD").unwrap();
        // 临时文件的位置被一个目录占着：写不进去。旧文件必须原样留着。
        std::fs::create_dir(p.with_extension("json.tmp")).unwrap();
        assert!(write_atomic(&p, b"NEW").is_err());
        assert_eq!(std::fs::read_to_string(&p).unwrap(), "OLD");
        assert!(d.path().join("dco-rules.json.tmp").is_dir());
    }

    #[test]
    fn watcher_writes_on_first_tick_then_only_when_version_changes() {
        let (_d, p) = tmp();
        let f = Fake::new("dco 0.1.0", &rules_json(2, 3));
        let mut w = Watcher::default();
        assert!(matches!(w.tick(&f, &p), Some(Ok(_))), "启动第一轮一定写");
        assert!(w.tick(&f, &p).is_none(), "版本没变不重写");
        assert!(w.tick(&f, &p).is_none());
        assert_eq!(f.rules_calls(), 1);

        *f.version.borrow_mut() = Ok(b"dco 0.2.0\n".to_vec());
        assert!(matches!(w.tick(&f, &p), Some(Ok(_))), "换版本要重写");
        assert!(w.tick(&f, &p).is_none());
        assert_eq!(f.rules_calls(), 2);
    }

    #[test]
    fn watcher_does_nothing_without_dco_and_writes_once_it_appears() {
        let (_d, p) = tmp();
        let f = Fake::new("", &rules_json(1, 1));
        *f.version.borrow_mut() = Err(RunError::NotInstalled);
        let mut w = Watcher::default();
        assert!(w.tick(&f, &p).is_none());
        assert_eq!(f.rules_calls(), 0);
        assert!(!p.exists());

        *f.version.borrow_mut() = Ok(b"dco 0.1.0".to_vec());
        assert!(matches!(w.tick(&f, &p), Some(Ok(_))));
        assert!(p.exists());
    }

    #[test]
    fn watcher_retries_after_a_failed_write_on_the_same_version() {
        let (_d, p) = tmp();
        let f = Fake::new("dco 0.1.0", "not json");
        let mut w = Watcher::default();
        assert!(matches!(w.tick(&f, &p), Some(Err(Refused::NotJson))));
        *f.rules.borrow_mut() = Ok(rules_json(1, 1).into_bytes());
        assert!(
            matches!(w.tick(&f, &p), Some(Ok(_))),
            "上次没写成，同一版本也要再试"
        );
        assert!(w.tick(&f, &p).is_none());
    }

    #[test]
    fn describe_is_one_plain_line() {
        let ok = describe(&Ok(Summary {
            categories: 21,
            items: 146,
            dco_version: Some("0.1.0".into()),
        }));
        assert_eq!(ok, "规矩总表已更新：21 类、146 条（dco 0.1.0）");
        let bad = describe(&Err(Refused::Run(RunError::NotInstalled)));
        assert_eq!(bad, "规矩总表没更新：这台电脑上没装 dco。旧文件留着没动");
    }

    /// 拿本机真的 dco 跑一遍（写到临时目录，不碰 `~/.dct`）。CI 上没有 dco，所以默认不跑：
    /// `cargo test --lib dco_rules -- --ignored`。
    #[test]
    #[ignore]
    fn real_dco_output_is_written_verbatim() {
        let (_d, p) = tmp();
        let r = SystemRunner::new();
        let s = refresh(&r, &p).expect("本机要装着 dco");
        let direct = r.run(&["rules", "--json"], TIMEOUT, MAX_BYTES).unwrap();
        assert_eq!(std::fs::read(&p).unwrap(), direct);
        eprintln!("{}", describe(&Ok(s)));
        assert!(version(&r).is_some());
    }

    #[cfg(unix)]
    #[test]
    fn real_runner_times_out_and_reports_missing_program() {
        let r = run_with_timeout(
            Path::new("/bin/sleep"),
            &["5"],
            Duration::from_millis(200),
            100,
        );
        assert_eq!(r, Err(RunError::TimedOut));
        let r = run_with_timeout(Path::new("/没有/这个/程序"), &[], TIMEOUT, 100);
        assert_eq!(r, Err(RunError::NotInstalled));
        let r = run_with_timeout(Path::new("/bin/sh"), &["-c", "exit 3"], TIMEOUT, 100);
        assert_eq!(r, Err(RunError::Failed("退出码 3".into())));
        let r = run_with_timeout(Path::new("/bin/echo"), &["hello"], TIMEOUT, 3);
        assert_eq!(r, Err(RunError::TooBig));
        let r = run_with_timeout(Path::new("/bin/echo"), &["hi"], TIMEOUT, 100);
        assert_eq!(r, Ok(b"hi\n".to_vec()));
    }
}
