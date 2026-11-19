//! 常驻会话：`~/.dct/config.toml` 里的 `[[keep]]`。
//!
//! 这里**只**读 `keep` 这一个键，不碰 `[llm]` 等其它段，也不走 `Config` 的
//! 整体反序列化：一条写坏的 `[[keep]]` 只该丢掉它自己，不能拖垮别的条目，
//! 更不能让 `[llm]` 读不出来（反过来也一样：`Config::load` 不认识 `keep`
//! 这个键，照样读 `[llm]`）。坏条目跳过并回报 `WarningCode::KeepEntryInvalid`。

use crate::proto::{KeepEntryProblem, WarningCode};
use crate::profile::Profile;
use crate::proto::KeptSessionReason;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// 一条常驻会话的声明。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Keep {
    pub name: String,
    pub profile: String,
    /// 已展开 `~`。
    pub dir: PathBuf,
}

fn expand(dir: &str, home: &Path) -> PathBuf {
    if dir == "~" {
        home.to_path_buf()
    } else if let Some(rest) = dir.strip_prefix("~/") {
        home.join(rest)
    } else {
        PathBuf::from(dir)
    }
}

/// 取一个非空字符串字段；缺了、类型不对、全空白都算缺。
fn text_field(t: &toml::Table, key: &str) -> Option<String> {
    let s = t.get(key)?.as_str()?.trim();
    (!s.is_empty()).then(|| s.to_string())
}

/// 只解析 config.toml 源文本里的 `keep` 数组。坏条目跳过并回报，
/// 重名保留先写的那条。整份文本解析不了时返回空（`Config::load` 已经报过）。
pub fn parse(src: &str, home: &Path) -> (Vec<Keep>, Vec<WarningCode>) {
    let Ok(root) = src.parse::<toml::Table>() else {
        return (Vec::new(), Vec::new());
    };
    let Some(keep) = root.get("keep") else {
        return (Vec::new(), Vec::new());
    };
    let invalid = |index: usize, reason: KeepEntryProblem| WarningCode::KeepEntryInvalid {
        index,
        reason,
    };
    let Some(entries) = keep.as_array() else {
        return (Vec::new(), vec![invalid(0, KeepEntryProblem::KeepNotAnArray)]);
    };
    let mut out: Vec<Keep> = Vec::new();
    let mut warnings = Vec::new();
    for (i, entry) in entries.iter().enumerate() {
        let index = i + 1;
        let Some(t) = entry.as_table() else {
            warnings.push(invalid(index, KeepEntryProblem::NotATable));
            continue;
        };
        // 名字写了但全是空白，要说「名字是空的」，不是「没写名字」
        if t.get("name")
            .and_then(|v| v.as_str())
            .is_some_and(|s| s.trim().is_empty())
        {
            warnings.push(invalid(index, KeepEntryProblem::EmptyName));
            continue;
        }
        let Some(name) = text_field(t, "name") else {
            warnings.push(invalid(index, KeepEntryProblem::MissingField("name".into())));
            continue;
        };
        let Some(profile) = text_field(t, "profile") else {
            warnings.push(invalid(index, KeepEntryProblem::MissingField("profile".into())));
            continue;
        };
        let Some(dir) = text_field(t, "dir") else {
            warnings.push(invalid(index, KeepEntryProblem::MissingField("dir".into())));
            continue;
        };
        if out.iter().any(|k| k.name == name) {
            warnings.push(invalid(index, KeepEntryProblem::DuplicateName(name)));
            continue;
        }
        out.push(Keep {
            name,
            profile,
            dir: expand(&dir, home),
        });
    }
    (out, warnings)
}

/// 读文件。不存在、读不了、解析不了都是「没有常驻会话」且不报警：
/// 文件坏了这件事 `Config::load` 已经说过一次。
pub fn load(config_path: &Path, home: &Path) -> (Vec<Keep>, Vec<WarningCode>) {
    match std::fs::read_to_string(config_path) {
        Ok(src) => parse(&src, home),
        Err(_) => (Vec::new(), Vec::new()),
    }
}

/// 看守线程看名册的那一面。真的实现（Task 3）包在 `SessionManager` 外面；
/// 这里只依赖这几个动作，测试就能换一个随意摆布的假名册、配手拨的时钟，
/// 不用真开进程、不用真睡。
pub trait Roster {
    /// 正在接回上次的会话，或者正准备平稳重启。这时开会话，前者会跟接回
    /// 线程抢着开同一个，后者会在重启前凭空多出一个。
    fn paused(&self) -> bool;
    /// 名册上「它的会话」：名字、profile、规范化后的目录三项都对上，且没停。
    fn find(&self, k: &Keep) -> Option<u32>;
    /// 名册上另有一个活着的会话也叫这个名字，但 profile 或目录不同。
    /// `dct send` 碰到两个同名会话会报「有歧义」，所以要告诉用户。
    fn name_taken_elsewhere(&self, k: &Keep) -> bool;
    /// 这个会话此刻的对话 id。会话一停就从名册上抹掉、连对话 id 一起没了，
    /// 所以只能趁它活着时问。
    fn conversation_of(&self, id: u32) -> Option<String>;
    /// 开会话。`want_id` 是原来的编号；名册上若还残留着这个编号的已停止格子，
    /// 实现要先把它拿掉再开。错误文本只用来记日志，用户看到的是警告码。
    fn open(&self, k: &Keep, conversation: Option<&str>, want_id: Option<u32>) -> Result<u32, String>;
    fn warn(&self, w: WarningCode);
}

/// 第一次重开前等多久；之后每多一次「秒退」翻一倍。
pub const RESTART_BASE: Duration = Duration::from_secs(5);
/// 等待的上限：再怎么崩，五分钟也会再试一次。
pub const RESTART_CAP: Duration = Duration::from_secs(300);
/// 开起来活够这么久才算「好了」：连续失败次数清零，已报过的警告可以再报。
pub const HEALTHY_AFTER: Duration = Duration::from_secs(60);
/// 按对话 id 接回后这么快就退出，多半是对话文件没了——下次别再带这个 id，
/// 否则会一直「接回 → 秒退」。跟 `watch_resumed_by_id` 的 20 秒同一个道理。
pub const RESUME_FAIL_WINDOW: Duration = Duration::from_secs(20);
/// 连续失败到这个次数报一次「反复崩溃」。照样按退避重开，只是让用户知道。
pub const CRASH_WARN_AFTER: u32 = 3;

/// 第 `failures` 次失败之后要等多久：5 秒起翻倍，封顶 5 分钟。
fn backoff(failures: u32) -> Duration {
    RESTART_BASE
        .checked_mul(1u32.checked_shl(failures).unwrap_or(u32::MAX))
        .unwrap_or(RESTART_CAP)
        .min(RESTART_CAP)
}

/// 一条 keep 的记忆。会话停了之后名册上什么都不剩，编号和对话 id 只能靠这里。
#[derive(Default)]
struct Watched {
    /// 最后一次见到它活着（或我们开出来）时的编号。重开时沿用，编号不变。
    id: Option<u32>,
    conversation: Option<String>,
    /// 见过它活着，或者我们开过它。没见过的直接开，见过的要先等一等。
    seen: bool,
    /// 我们上一次成功开它的时刻；不是我们开的（比如平稳重启接回的）为 `None`。
    opened_at: Option<Instant>,
    /// 上一次开时带了对话 id，而且这个 profile 真的按 id 接回。
    resumed: bool,
    failures: u32,
    /// 已经发现它停了、排好的重开时刻。`None` = 还没发现它停。
    next_try: Option<Instant>,
    /// 已经报过的原因，同一种原因不连着每 5 秒报一遍。
    warned: Vec<KeptSessionReason>,
}

impl Watched {
    fn warn_once(&mut self, name: &str, reason: KeptSessionReason, r: &dyn Roster) {
        if !self.warned.contains(&reason) {
            self.warned.push(reason);
            r.warn(WarningCode::KeptSessionProblem {
                name: name.to_string(),
                reason,
            });
        }
    }

    /// 问题消失了：下次再出就再报。
    fn clear(&mut self, reason: KeptSessionReason) {
        self.warned.retain(|w| *w != reason);
    }

    /// 记一次失败并排下一次重开。
    fn fail(&mut self, now: Instant, name: &str, r: &dyn Roster) {
        self.failures += 1;
        if self.failures >= CRASH_WARN_AFTER {
            self.warn_once(name, KeptSessionReason::CrashLooping, r);
        }
        self.next_try = Some(now + backoff(self.failures));
    }
}

/// 常驻会话的看守。自己不看钟、不开线程：每轮由调用方（daemon 里每 5 秒）
/// 带着当前时刻调一次 `step`，测试就能手动拨钟。
#[derive(Default)]
pub struct KeepWatch {
    by_name: HashMap<String, Watched>,
}

impl KeepWatch {
    pub fn new() -> Self {
        Self::default()
    }

    /// 走一轮。`keeps` 和 `profiles` 每轮现读，改配置不用重启后台。
    pub fn step(&mut self, now: Instant, keeps: &[Keep], profiles: &[Profile], r: &dyn Roster) {
        if r.paused() {
            return;
        }
        // 配置里删掉的不再管；再加回来就是一条新的，不带旧编号旧对话
        self.by_name.retain(|n, _| keeps.iter().any(|k| &k.name == n));
        for k in keeps {
            let w = self.by_name.entry(k.name.clone()).or_default();
            Self::step_one(w, now, k, profiles, r);
        }
    }

    fn step_one(w: &mut Watched, now: Instant, k: &Keep, profiles: &[Profile], r: &dyn Roster) {
        // 撞名不拦着开：用户配了常驻，就该有；只提醒 `dct send` 会分不清
        if r.name_taken_elsewhere(k) {
            w.warn_once(&k.name, KeptSessionReason::NameClash, r);
        } else {
            w.clear(KeptSessionReason::NameClash);
        }

        if let Some(id) = r.find(k) {
            w.id = Some(id);
            w.seen = true;
            w.next_try = None;
            // 问不到就留着上次的：可能只是这一刻还没写出来
            if let Some(c) = r.conversation_of(id) {
                w.conversation = Some(c);
            }
            // 能在名册上对上，目录和 profile 自然都在
            w.clear(KeptSessionReason::DirGone);
            w.clear(KeptSessionReason::ProfileGone);
            if w.opened_at.is_none_or(|t| now.duration_since(t) >= HEALTHY_AFTER) {
                w.failures = 0;
                w.warned.retain(|r| *r == KeptSessionReason::NameClash);
            }
            return;
        }

        // 刚发现它停了：判一下是不是秒退，排好重开时刻，这一轮先不开
        if w.seen && w.next_try.is_none() {
            let lived = w.opened_at.map(|t| now.duration_since(t));
            if w.resumed && lived.is_some_and(|d| d < RESUME_FAIL_WINDOW) {
                w.conversation = None;
            }
            if lived.is_some_and(|d| d < HEALTHY_AFTER) {
                w.fail(now, &k.name, r);
            } else {
                w.next_try = Some(now + backoff(w.failures));
            }
            return;
        }
        if w.next_try.is_some_and(|t| now < t) {
            return;
        }

        // 目录和 profile 不在：不开，不算失败（不是它崩了），每轮再看，回来了马上开。
        // 目录不替用户建：写错路径时凭空多出一个文件夹更让人糊涂。
        if !k.dir.is_dir() {
            w.warn_once(&k.name, KeptSessionReason::DirGone, r);
            return;
        }
        w.clear(KeptSessionReason::DirGone);
        let Some(profile) = profiles.iter().find(|p| p.name == k.profile) else {
            w.warn_once(&k.name, KeptSessionReason::ProfileGone, r);
            return;
        };
        w.clear(KeptSessionReason::ProfileGone);

        let conversation = w.conversation.clone();
        match r.open(k, conversation.as_deref(), w.id) {
            Ok(id) => {
                w.id = Some(id);
                w.seen = true;
                w.opened_at = Some(now);
                w.resumed = conversation.is_some() && profile.resumes_by_id();
                w.next_try = None;
            }
            Err(_) => {
                w.opened_at = None;
                w.resumed = false;
                w.warn_once(&k.name, KeptSessionReason::StartFailed, r);
                w.fail(now, &k.name, r);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn home() -> PathBuf {
        PathBuf::from("/home/u")
    }

    fn problem(w: &WarningCode) -> (usize, &KeepEntryProblem) {
        match w {
            WarningCode::KeepEntryInvalid { index, reason } => (*index, reason),
            other => panic!("应当是 KeepEntryInvalid：{other:?}"),
        }
    }

    #[test]
    fn two_good_entries_are_read_in_order() {
        let src = r#"
[[keep]]
name = "a"
profile = "claude"
dir = "/w/a"
[[keep]]
name = "b"
profile = "codex"
dir = "/w/b"
"#;
        let (k, w) = parse(src, &home());
        assert!(w.is_empty());
        assert_eq!(
            k,
            vec![
                Keep {
                    name: "a".into(),
                    profile: "claude".into(),
                    dir: "/w/a".into()
                },
                Keep {
                    name: "b".into(),
                    profile: "codex".into(),
                    dir: "/w/b".into()
                },
            ]
        );
    }

    #[test]
    fn tilde_is_expanded_against_the_given_home() {
        let src = r#"
[[keep]]
name = "a"
profile = "p"
dir = "~/work/x"
[[keep]]
name = "b"
profile = "p"
dir = "~"
"#;
        let (k, _) = parse(src, &home());
        assert_eq!(k[0].dir, PathBuf::from("/home/u/work/x"));
        assert_eq!(k[1].dir, PathBuf::from("/home/u"));
    }

    #[test]
    fn an_entry_missing_a_field_is_skipped_and_the_rest_survive() {
        let src = r#"
[[keep]]
name = "a"
dir = "/w/a"
[[keep]]
name = "b"
profile = "p"
dir = "/w/b"
"#;
        let (k, w) = parse(src, &home());
        assert_eq!(k.len(), 1);
        assert_eq!(k[0].name, "b");
        assert_eq!(w.len(), 1);
        assert_eq!(
            problem(&w[0]),
            (1, &KeepEntryProblem::MissingField("profile".into()))
        );
    }

    #[test]
    fn a_blank_name_is_reported_as_empty() {
        let src = "[[keep]]\nname = \"  \"\nprofile = \"p\"\ndir = \"/w\"\n";
        let (k, w) = parse(src, &home());
        assert!(k.is_empty());
        assert_eq!(problem(&w[0]), (1, &KeepEntryProblem::EmptyName));
    }

    #[test]
    fn a_repeated_name_keeps_the_first() {
        let src = r#"
[[keep]]
name = "a"
profile = "p1"
dir = "/w/1"
[[keep]]
name = "a"
profile = "p2"
dir = "/w/2"
"#;
        let (k, w) = parse(src, &home());
        assert_eq!(k.len(), 1);
        assert_eq!(k[0].profile, "p1");
        assert_eq!(
            problem(&w[0]),
            (2, &KeepEntryProblem::DuplicateName("a".into()))
        );
    }

    #[test]
    fn no_keep_key_is_empty_and_silent() {
        let (k, w) = parse("[llm]\nprovider = \"claude\"\n", &home());
        assert!(k.is_empty() && w.is_empty());
    }

    #[test]
    fn a_keep_of_the_wrong_type_warns_once_and_does_not_panic() {
        let (k, w) = parse("keep = 3\n", &home());
        assert!(k.is_empty());
        assert_eq!(w.len(), 1);
        assert_eq!(problem(&w[0]), (0, &KeepEntryProblem::KeepNotAnArray));
        let (k, w) = parse("keep = [1, 2]\n", &home());
        assert!(k.is_empty());
        assert_eq!(w.len(), 2);
        assert_eq!(problem(&w[0]), (1, &KeepEntryProblem::NotATable));
        assert_eq!(problem(&w[1]), (2, &KeepEntryProblem::NotATable));
    }

    #[test]
    fn surrounding_spaces_are_trimmed() {
        let src = "[[keep]]\nname = \"  a \"\nprofile = \" p\"\ndir = \" /w \"\n";
        let (k, w) = parse(src, &home());
        assert!(w.is_empty());
        assert_eq!(k[0].name, "a");
        assert_eq!(k[0].profile, "p");
        assert_eq!(k[0].dir, PathBuf::from("/w"));
    }

    #[test]
    fn duplicates_are_detected_after_trimming() {
        let src = "[[keep]]\nname = \"a\"\nprofile = \"p\"\ndir = \"/1\"\n\
                   [[keep]]\nname = \" a \"\nprofile = \"p\"\ndir = \"/2\"\n";
        let (k, w) = parse(src, &home());
        assert_eq!(k.len(), 1);
        assert_eq!(
            problem(&w[0]),
            (2, &KeepEntryProblem::DuplicateName("a".into()))
        );
    }

    #[test]
    fn a_non_string_field_counts_as_missing() {
        let src = "[[keep]]\nname = \"a\"\nprofile = 5\ndir = \"/w\"\n";
        let (k, w) = parse(src, &home());
        assert!(k.is_empty());
        assert_eq!(
            problem(&w[0]),
            (1, &KeepEntryProblem::MissingField("profile".into()))
        );
    }

    #[test]
    fn other_sections_are_ignored() {
        let src = r#"
[llm]
provider = "claude"
[menu]
hide = ["x"]
[[keep]]
name = "a"
profile = "p"
dir = "/w"
"#;
        let (k, w) = parse(src, &home());
        assert_eq!(k.len(), 1);
        assert!(w.is_empty());
    }

    #[test]
    fn load_treats_missing_and_unparseable_files_as_empty() {
        let d = tempfile::tempdir().unwrap();
        assert_eq!(load(&d.path().join("nope.toml"), &home()), (vec![], vec![]));
        let p = d.path().join("c.toml");
        std::fs::write(&p, "[[keep\nname =").unwrap();
        assert_eq!(load(&p, &home()), (vec![], vec![]));
        std::fs::write(&p, "[[keep]]\nname=\"a\"\nprofile=\"p\"\ndir=\"~/x\"\n").unwrap();
        let (k, _) = load(&p, &home());
        assert_eq!(k[0].dir, PathBuf::from("/home/u/x"));
    }
}

#[cfg(test)]
mod watch_tests {
    use super::*;
    use std::cell::{Cell, RefCell};

    /// 一次 `open` 调用：名字、带的对话 id、要的编号。
    type OpenCall = (String, Option<String>, Option<u32>);

    /// 假名册：`alive` 里有谁，`find` 就找到谁；`open` 只记账，不会自己让
    /// 会话「活过来」——活没活由测试自己摆，才能演「刚开就死」。
    #[derive(Default)]
    struct Fake {
        paused: Cell<bool>,
        alive: RefCell<HashMap<String, u32>>,
        conversations: RefCell<HashMap<u32, String>>,
        clash: Cell<bool>,
        open_fails: Cell<bool>,
        opens: RefCell<Vec<OpenCall>>,
        warnings: RefCell<Vec<WarningCode>>,
        next_id: Cell<u32>,
    }

    impl Roster for Fake {
        fn paused(&self) -> bool {
            self.paused.get()
        }
        fn find(&self, k: &Keep) -> Option<u32> {
            self.alive.borrow().get(&k.name).copied()
        }
        fn name_taken_elsewhere(&self, _k: &Keep) -> bool {
            self.clash.get()
        }
        fn conversation_of(&self, id: u32) -> Option<String> {
            self.conversations.borrow().get(&id).cloned()
        }
        fn open(&self, k: &Keep, conversation: Option<&str>, want_id: Option<u32>) -> Result<u32, String> {
            self.opens
                .borrow_mut()
                .push((k.name.clone(), conversation.map(str::to_string), want_id));
            if self.open_fails.get() {
                return Err("boom".into());
            }
            Ok(want_id.unwrap_or_else(|| {
                self.next_id.set(self.next_id.get() + 1);
                100 + self.next_id.get()
            }))
        }
        fn warn(&self, w: WarningCode) {
            self.warnings.borrow_mut().push(w);
        }
    }

    struct H {
        w: KeepWatch,
        r: Fake,
        keeps: Vec<Keep>,
        profiles: Vec<Profile>,
        t0: Instant,
        dir: tempfile::TempDir,
    }

    impl H {
        fn new(profile: &str) -> H {
            let dir = tempfile::tempdir().unwrap();
            H {
                w: KeepWatch::new(),
                r: Fake::default(),
                keeps: vec![Keep {
                    name: "tutor".into(),
                    profile: profile.into(),
                    dir: dir.path().to_path_buf(),
                }],
                profiles: Profile::builtins(),
                t0: Instant::now(),
                dir,
            }
        }
        fn at(&mut self, s: u64) {
            self.w
                .step(self.t0 + Duration::from_secs(s), &self.keeps, &self.profiles, &self.r);
        }
        fn opens(&self) -> Vec<OpenCall> {
            self.r.opens.borrow().clone()
        }
        fn live(&self, id: u32) {
            self.r.alive.borrow_mut().insert("tutor".into(), id);
        }
        fn die(&self) {
            self.r.alive.borrow_mut().clear();
        }
        fn talk(&self, id: u32, c: &str) {
            self.r.conversations.borrow_mut().insert(id, c.into());
        }
        fn warned(&self, reason: KeptSessionReason) -> usize {
            self.r
                .warnings
                .borrow()
                .iter()
                .filter(|w| {
                    matches!(w, WarningCode::KeptSessionProblem { name, reason: r }
                        if name == "tutor" && *r == reason)
                })
                .count()
        }
        /// 从 `from` 起每秒走一轮，直到又开了一次；返回开在第几秒。
        fn next_open_from(&mut self, from: u64) -> u64 {
            let before = self.opens().len();
            for s in from..from + 1000 {
                self.at(s);
                if self.opens().len() > before {
                    return s;
                }
            }
            panic!("从 {from} 起一千秒都没再开");
        }
    }

    #[test]
    fn a_fresh_keep_is_opened_once_with_nothing_remembered() {
        let mut h = H::new("claude");
        h.at(0);
        assert_eq!(h.opens(), vec![("tutor".to_string(), None, None)]);
        h.live(101);
        h.at(5);
        h.at(10);
        assert_eq!(h.opens().len(), 1);
    }

    #[test]
    fn a_session_that_stops_comes_back_after_5s_with_its_number_and_conversation() {
        let mut h = H::new("claude");
        h.live(7);
        h.talk(7, "c1");
        h.at(0);
        assert!(h.opens().is_empty(), "活着就不动");
        h.die();
        h.at(5);
        assert!(h.opens().is_empty(), "刚发现停了，不立刻开");
        h.at(9);
        assert!(h.opens().is_empty());
        h.at(10);
        assert_eq!(
            h.opens(),
            vec![("tutor".to_string(), Some("c1".to_string()), Some(7))]
        );
    }

    #[test]
    fn quick_deaths_back_off_doubling_to_the_cap_and_reset_after_a_healthy_minute() {
        let mut h = H::new("shell");
        h.at(0); // 新开，t=0
        let mut opened = 0;
        let mut waits = Vec::new();
        for _ in 0..8 {
            // 开了之后马上死：下一轮就发现
            h.die();
            let died = opened + 1;
            opened = h.next_open_from(died);
            waits.push(opened - died);
        }
        assert_eq!(waits, vec![10, 20, 40, 80, 160, 300, 300, 300]);
        // 这次活够一分钟，清零
        h.live(101);
        h.at(opened + 1);
        h.at(opened + 61);
        h.die();
        let died = opened + 62;
        assert_eq!(h.next_open_from(died) - died, 5);
    }

    #[test]
    fn being_seen_alive_briefly_does_not_reset_the_backoff() {
        // 开起来活了 10 秒、被看见过再死，仍是秒退：不能一看见活着就清零
        let mut h = H::new("shell");
        h.at(0);
        let mut opened = 0;
        let mut waits = Vec::new();
        for _ in 0..3 {
            h.live(101);
            h.at(opened + 5);
            h.die();
            let died = opened + 10;
            opened = h.next_open_from(died);
            waits.push(opened - died);
        }
        assert_eq!(waits, vec![10, 20, 40]);
    }

    #[test]
    fn a_session_found_alive_and_then_lost_is_not_counted_as_a_quick_death() {
        // 不是我们开的（比如平稳重启接回的），死了按第一次算：5 秒
        let mut h = H::new("shell");
        h.live(3);
        h.at(0);
        h.die();
        assert_eq!(h.next_open_from(1) - 1, 5);
    }

    #[test]
    fn a_resumed_conversation_that_dies_within_20s_is_dropped_next_time() {
        let mut h = H::new("claude");
        assert!(Profile::builtin("claude").unwrap().resumes_by_id());
        h.live(7);
        h.talk(7, "c1");
        h.at(0);
        h.die();
        let t = h.next_open_from(1);
        assert_eq!(h.opens()[0].1.as_deref(), Some("c1"));
        // 带着对话 id 接回，没几秒就退了：对话文件多半没了
        h.next_open_from(t + 5);
        assert_eq!(h.opens()[1].1, None);
        assert_eq!(h.opens()[1].2, Some(7), "编号照旧");
    }

    #[test]
    fn a_resumed_conversation_that_lives_past_20s_is_kept() {
        let mut h = H::new("claude");
        h.live(7);
        h.talk(7, "c1");
        h.at(0);
        h.die();
        let t = h.next_open_from(1);
        h.live(7);
        h.at(t + 5);
        h.at(t + 25);
        h.die();
        h.next_open_from(t + 30);
        assert_eq!(h.opens()[1].1.as_deref(), Some("c1"));
    }

    #[test]
    fn a_quick_death_without_resume_by_id_keeps_the_conversation() {
        // shell 不按 id 接回，那它秒退就跟对话 id 无关，别扔
        let mut h = H::new("shell");
        assert!(!Profile::builtin("shell").unwrap().resumes_by_id());
        h.live(7);
        h.talk(7, "c1");
        h.at(0);
        h.die();
        let t = h.next_open_from(1);
        h.next_open_from(t + 5);
        assert_eq!(h.opens()[1].1.as_deref(), Some("c1"));
    }

    #[test]
    fn a_later_missing_conversation_id_keeps_the_remembered_one() {
        let mut h = H::new("claude");
        h.live(7);
        h.talk(7, "c1");
        h.at(0);
        h.r.conversations.borrow_mut().clear();
        h.at(5);
        h.die();
        h.next_open_from(10);
        assert_eq!(h.opens()[0].1.as_deref(), Some("c1"));
    }

    #[test]
    fn nothing_is_opened_while_paused() {
        let mut h = H::new("claude");
        h.r.paused.set(true);
        for s in 0..30 {
            h.at(s);
        }
        assert!(h.opens().is_empty(), "接回或准备重启时一个都不开");
        h.r.paused.set(false);
        h.at(30);
        assert_eq!(h.opens().len(), 1);
    }

    #[test]
    fn a_due_reopen_waits_out_a_pause() {
        let mut h = H::new("claude");
        h.live(7);
        h.at(0);
        h.die();
        h.at(5); // 排在 10 秒
        h.r.paused.set(true);
        for s in 6..40 {
            h.at(s);
        }
        assert!(h.opens().is_empty());
        h.r.paused.set(false);
        h.at(40);
        assert_eq!(h.opens().len(), 1);
    }

    #[test]
    fn a_removed_keep_is_forgotten_and_never_reopened() {
        let mut h = H::new("claude");
        h.live(7);
        h.talk(7, "c1");
        h.at(0);
        h.keeps.clear();
        h.die();
        for s in 1..400 {
            h.at(s);
        }
        assert!(h.opens().is_empty());
        // 再加回来就是一条新的：不带旧编号、旧对话
        h.keeps.push(Keep {
            name: "tutor".into(),
            profile: "claude".into(),
            dir: h.dir.path().to_path_buf(),
        });
        h.at(400);
        assert_eq!(h.opens(), vec![("tutor".to_string(), None, None)]);
    }

    #[test]
    fn a_missing_dir_warns_once_and_again_only_after_it_came_back() {
        let mut h = H::new("claude");
        let sub = h.dir.path().join("sub");
        h.keeps[0].dir = sub.clone();
        for s in 0..30 {
            h.at(s);
        }
        assert!(h.opens().is_empty());
        assert_eq!(h.warned(KeptSessionReason::DirGone), 1);
        std::fs::create_dir(&sub).unwrap();
        h.at(30);
        assert_eq!(h.opens().len(), 1);
        h.live(101);
        h.at(35);
        std::fs::remove_dir(&sub).unwrap();
        h.die();
        for s in 40..400 {
            h.at(s);
        }
        assert_eq!(h.opens().len(), 1);
        assert_eq!(h.warned(KeptSessionReason::DirGone), 2);
    }

    #[test]
    fn a_missing_profile_warns_once_and_again_only_after_it_came_back() {
        let mut h = H::new("claude");
        let all = std::mem::take(&mut h.profiles);
        for s in 0..30 {
            h.at(s);
        }
        assert!(h.opens().is_empty());
        assert_eq!(h.warned(KeptSessionReason::ProfileGone), 1);
        h.profiles = all;
        h.at(30);
        assert_eq!(h.opens().len(), 1);
        h.live(101);
        h.at(35);
        h.profiles.clear();
        h.die();
        for s in 40..400 {
            h.at(s);
        }
        assert_eq!(h.opens().len(), 1);
        assert_eq!(h.warned(KeptSessionReason::ProfileGone), 2);
    }

    #[test]
    fn a_failed_open_warns_once_and_backs_off() {
        let mut h = H::new("claude");
        h.r.open_fails.set(true);
        h.at(0);
        assert_eq!(h.opens().len(), 1);
        assert_eq!(h.warned(KeptSessionReason::StartFailed), 1);
        assert_eq!(h.next_open_from(1), 10);
        assert_eq!(h.next_open_from(11), 30);
        assert_eq!(h.warned(KeptSessionReason::StartFailed), 1);
        h.r.open_fails.set(false);
        assert_eq!(h.next_open_from(31), 70);
    }

    #[test]
    fn repeated_quick_deaths_warn_crash_looping_once_until_healthy_again() {
        let mut h = H::new("shell");
        h.at(0);
        let mut t = 0;
        for _ in 0..6 {
            h.die();
            t = h.next_open_from(t + 1);
        }
        assert_eq!(h.warned(KeptSessionReason::CrashLooping), 1);
        // 活够一分钟，之后又连着秒退，可以再报
        h.live(101);
        h.at(t + 1);
        h.at(t + 61);
        t += 61;
        for _ in 0..4 {
            h.die();
            t = h.next_open_from(t + 1);
        }
        assert_eq!(h.warned(KeptSessionReason::CrashLooping), 2);
    }

    #[test]
    fn a_name_clash_warns_once_and_still_opens() {
        let mut h = H::new("claude");
        h.r.clash.set(true);
        h.at(0);
        assert_eq!(h.opens().len(), 1);
        h.live(101);
        for s in 1..30 {
            h.at(s);
        }
        assert_eq!(h.warned(KeptSessionReason::NameClash), 1);
        // 那个同名会话走了又来：再报一次
        h.r.clash.set(false);
        h.at(30);
        h.r.clash.set(true);
        h.at(31);
        assert_eq!(h.warned(KeptSessionReason::NameClash), 2);
    }
}
