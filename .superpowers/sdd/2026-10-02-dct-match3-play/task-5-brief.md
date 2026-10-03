### Task 5: 说给用户听的话、说明卡、`dct game play` 命令

**Files:**
- Create: `src/game/text.rs`、`src/game/skill.rs`、`src/game/skill.md`、`src/game/cli.rs`、`tests/game_cli.rs`
- Modify: `src/game/mod.rs`、`src/main.rs`、`src/daemon.rs`

**Interfaces:**
- Consumes: 任务 2、3 的 `play`、`DcoClient`；任务 4 的 `profile::load`、`LogFile`。
- Produces: `dct game play [--game 名字] [--steps N] [--dry-run]`，退出码：走满步数 / 没有能走的步 / 试走 = 0，其余 = 1，参数错 = 2；`skill::install_all(home) -> Vec<(&str, io::Result<Installed>)>`。


- [ ] **Step 1: 把 `src/game/mod.rs` 改成完整的**

```rust
//! `dct game`：让 dct 自己玩三消游戏。选步是纯算法（`crates/dct-game`），这里只管棋盘配置、
//! 记录、给用户看的话、装给 agent 的说明卡，和命令本身（设计：
//! docs/superpowers/specs/2026-10-02-dct-match3-play-design.md）。
pub mod cli;
pub mod log;
pub mod profile;
pub mod skill;
pub mod text;
```

- [ ] **创建 `src/game/text.rs`**（下面的代码已在临时副本里编译、跑过测试）

```rust
//! 说给用户（和转述给用户的 agent）听的话。不出现类别编号、分数公式；行列从 1 数、从上往下。
//! 第一轮只有中文；换成 i18n 是后面的事（设计「不在这一轮」之外的收尾项）。
use dct_game::play::{DcoError, Stop};
use serde_json::Value;
use std::path::Path;

fn n(v: &Value) -> u64 {
    v.as_u64().unwrap_or(0)
}

pub fn step_line(step: usize, rec: &Value) -> String {
    let c = &rec["candidates"][rec["chosen"].as_u64().unwrap_or(0) as usize];
    let pos = |p: &Value| format!("第 {} 行第 {} 列", n(&p[0]) + 1, n(&p[1]) + 1);
    let mut s = format!("第 {step} 步：{} ↔ {}，消 {} 颗", pos(&c["a"]), pos(&c["b"]), n(&c["cleared"]));
    for (key, what) in [("striped", "条纹糖"), ("wrapped", "包装糖"), ("bomb", "彩色炸弹")] {
        if n(&c[key]) > 0 {
            s += &format!("，做出{what}");
        }
    }
    if n(&c["triggered"]) > 0 {
        s += &format!("，引爆 {} 颗特殊糖", n(&c["triggered"]));
    }
    if c["special_swap"].as_bool().unwrap_or(false) {
        s += "，两颗特殊糖互换";
    }
    match rec["outcome"].as_str().unwrap_or("") {
        "dry_run" => s += "（试走，没有真划）",
        outcome => {
            let t = &rec["timing_ms"];
            if t.is_object() {
                s += &format!("（读 {} ms，选 {} ms，划 {} ms）", n(&t["read"]), n(&t["choose"]), n(&t["swipe"]));
            }
            if outcome == "no_change" {
                s += "；这一步划了没反应";
            }
        }
    }
    s
}

/// 写进记录文件的停下原因（给程序看的，不是给人看的）。
pub fn stop_code(stop: &Stop) -> &'static str {
    match stop {
        Stop::NoGrid(_) => "no_grid",
        Stop::ClassesChanged { .. } => "classes_changed",
        Stop::NoMoves => "no_moves",
        Stop::Stuck => "stuck",
        Stop::StillMoving => "still_moving",
        Stop::StepsDone => "steps_done",
        Stop::DryRun => "dry_run",
        Stop::Dco(_) => "dco",
    }
}

pub fn dco_error(e: &DcoError) -> String {
    match e.code.as_str() {
        "dco_down" | "dco_refused" => e.message.clone(),
        "halted" => "dco 急停了，这次停下。要接着玩，先让 dco 恢复。".into(),
        "paused" => "dco 暂停了，这次停下。".into(),
        "screen_locked" => "屏幕锁着，解锁以后再试。".into(),
        "not_found" => "没找到 iPhone 镜像的窗口。先打开它，进到一关里再试。".into(),
        _ => format!("dco 说：{}", e.message),
    }
}

/// 最后一句话，和退出码：正常结束（走满步数、没有能走的步、试走）是 0，其余都是 1。
pub fn stop_line(stop: &Stop, steps: usize, log: &Path) -> (String, i32) {
    let tail = format!("这次走了 {steps} 步，记录在 {}", log.display());
    match stop {
        Stop::StepsDone => (format!("到设定的步数了。{tail}。要接着玩，再运行一次。"), 0),
        Stop::NoMoves => (format!("停了：没有能走的步了。{tail}"), 0),
        Stop::DryRun => ("试走结束，没有真划。".into(), 0),
        Stop::NoGrid(why) => (format!("停了：读不出棋盘（{why}）。多半是这一关结束了，或者弹出了别的画面。{tail}"), 1),
        Stop::ClassesChanged { was, now } => (
            format!("停了：棋盘上的颜色种类一下子变多了（原来 {was} 种，现在 {now} 种），多半是这一关结束了，或者弹出了窗口。{tail}"),
            1,
        ),
        Stop::Stuck => (format!("停了：连着两次划了都没反应。请看一眼屏幕上是不是弹出了什么。{tail}"), 1),
        Stop::StillMoving => (format!("停了：等了 8 秒画面还在动。{tail}"), 1),
        Stop::Dco(e) => (format!("停了：{} {tail}", dco_error(e)), 1),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn rec(extra: Value) -> Value {
        let mut r = json!({
            "chosen": 1,
            "candidates": [
                {"a": [0, 0], "b": [0, 1], "cleared": 3, "striped": 0, "wrapped": 0, "bomb": 0, "triggered": 0, "special_swap": false},
                {"a": [6, 1], "b": [6, 2], "cleared": 4, "striped": 1, "wrapped": 0, "bomb": 0, "triggered": 0, "special_swap": false}
            ],
            "outcome": "moved", "timing_ms": {"read": 2, "choose": 0, "swipe": 262, "settle": 900}
        });
        for (k, v) in extra.as_object().unwrap() {
            r[k] = v.clone();
        }
        r
    }

    #[test]
    fn a_step_reads_as_plain_chinese_counting_from_one() {
        assert_eq!(
            step_line(3, &rec(json!({}))),
            "第 3 步：第 7 行第 2 列 ↔ 第 7 行第 3 列，消 4 颗，做出条纹糖（读 2 ms，选 0 ms，划 262 ms）"
        );
    }

    #[test]
    fn an_unmoved_step_and_a_dry_run_say_so() {
        assert!(step_line(1, &rec(json!({"outcome": "no_change"}))).ends_with("；这一步划了没反应"));
        let d = step_line(1, &rec(json!({"outcome": "dry_run", "timing_ms": null})));
        assert!(d.ends_with("（试走，没有真划）") && !d.contains("ms"), "{d}");
    }

    #[test]
    fn no_internal_words_leak_into_a_step_line() {
        let s = step_line(1, &rec(json!({})));
        for w in ["class", "score", "类别", "分数", "odd", "cells"] {
            assert!(!s.contains(w), "{s}");
        }
    }

    #[test]
    fn exit_codes_separate_normal_ends_from_trouble() {
        let p = Path::new("/x/log.jsonl");
        let e = |code: &str| Stop::Dco(DcoError { code: code.into(), message: "m".into() });
        for (s, want) in [
            (Stop::StepsDone, 0),
            (Stop::NoMoves, 0),
            (Stop::DryRun, 0),
            (Stop::Stuck, 1),
            (Stop::StillMoving, 1),
            (Stop::NoGrid("x".into()), 1),
            (Stop::ClassesChanged { was: 5, now: 8 }, 1),
            (e("halted"), 1),
        ] {
            assert_eq!(stop_line(&s, 4, p).1, want, "{s:?}");
        }
        assert!(stop_line(&Stop::NoMoves, 4, p).0.contains("走了 4 步") && stop_line(&Stop::NoMoves, 4, p).0.contains("/x/log.jsonl"));
    }

    #[test]
    fn dco_errors_are_translated() {
        let e = |c: &str| DcoError { code: c.into(), message: "原话".into() };
        assert!(dco_error(&e("halted")).contains("急停"));
        assert!(dco_error(&e("paused")).contains("暂停"));
        assert!(dco_error(&e("screen_locked")).contains("锁"));
        assert!(dco_error(&e("not_found")).contains("iPhone 镜像"));
        assert_eq!(dco_error(&e("dco_down")), "原话");
        assert_eq!(dco_error(&e("weird")), "dco 说：原话");
    }

    #[test]
    fn every_stop_has_a_distinct_code() {
        let all = [
            Stop::NoGrid("".into()),
            Stop::ClassesChanged { was: 1, now: 3 },
            Stop::NoMoves,
            Stop::Stuck,
            Stop::StillMoving,
            Stop::StepsDone,
            Stop::DryRun,
            Stop::Dco(DcoError { code: "x".into(), message: "".into() }),
        ];
        let mut codes: Vec<&str> = all.iter().map(stop_code).collect();
        codes.sort();
        codes.dedup();
        assert_eq!(codes.len(), all.len());
    }
}
```

- [ ] **创建 `src/game/skill.md`**（下面的代码已在临时副本里编译、跑过测试）

```markdown
---
name: dct-game
description: 用户想让 AI 玩三消游戏（比如 Candy Crush）时使用。运行 dct game play，按规则一步一步玩，每步不到一秒，不用截图问大模型。
---
<!-- dct-managed: dct-game-skill v1 -->

# 玩三消游戏

用户说「帮我玩一关 Candy Crush」「帮我消几步」「玩一会儿三消」之类的话，就用这个办法。

## 开始之前

- 用户要先在 iPhone 镜像里把游戏打开，进到一关里。这一步你不要代劳：关卡按钮、道具、付款都不要自己去点。
- 如果用户还没开游戏，告诉他先打开，再回来说一声。

## 怎么玩

运行：

    dct game play

它会一步一步玩，最多 20 步，每走一步就印一行，例如：

    第 3 步：第 7 行第 2 列 ↔ 第 7 行第 3 列，消 4 颗，做出条纹糖（读 2 ms，选 0 ms，划 262 ms）

- 把每一步的话转述给用户，用大白话，不用解释内部细节。
- 命令结束以后，告诉用户这次走了几步、为什么停；步数到了的话问他要不要接着玩，要就再运行一次。
- 想先看看它会怎么走、但不真的划：`dct game play --dry-run`。
- 想一次多走几步：`dct game play --steps 50`（最多 200）。

## 停

- 用户说「停」，马上中断这条命令。
- 命令自己停下时（没有能走的步、读不出棋盘、画面一直在动、连着两次没反应、dco 急停），把它印的最后一句话原样告诉用户。
- 不要自己去点屏幕补救，也不要换别的办法去操作游戏；让用户决定下一步。

## 不要做

- 不要自己调用 dco 去点关卡按钮、道具、广告、付款。
- 不要同时开两条 `dct game play`。
```

- [ ] **创建 `src/game/skill.rs`**（下面的代码已在临时副本里编译、跑过测试）

```rust
//! 把「怎么让 AI 玩三消」的说明卡装进 Claude Code、Codex、千问各自的 skills 目录。三家都认
//! `<家目录>/.<agent>/skills/<名字>/SKILL.md` 这个格式。卡片里带一行 dct 的标记：有标记的我们可以更新，
//! 没有标记的同名文件是用户自己写的，绝不覆盖。
use std::io;
use std::path::Path;

pub const SKILL_MD: &str = include_str!("skill.md");
pub const MARKER: &str = "<!-- dct-managed: dct-game-skill v1 -->";
const AGENT_DIRS: [&str; 3] = [".claude", ".codex", ".qwen"];

#[derive(Debug, PartialEq, Eq)]
pub enum Installed {
    Wrote,
    Same,
    /// 同名文件不是我们写的，没动。
    UserOwned,
    /// 这个 agent 的目录不存在（没装过），不替它建。
    NoAgent,
}

fn install_one(home: &Path, agent_dir: &str) -> io::Result<Installed> {
    let root = home.join(agent_dir);
    if !root.is_dir() {
        return Ok(Installed::NoAgent);
    }
    let dir = root.join("skills").join("dct-game");
    let file = dir.join("SKILL.md");
    match std::fs::read_to_string(&file) {
        Ok(old) if !old.contains(MARKER) => return Ok(Installed::UserOwned),
        Ok(old) if old == SKILL_MD => return Ok(Installed::Same),
        Ok(_) => {}
        Err(e) if e.kind() == io::ErrorKind::NotFound => {}
        Err(e) => return Err(e),
    }
    std::fs::create_dir_all(&dir)?;
    std::fs::write(&file, SKILL_MD)?;
    Ok(Installed::Wrote)
}

/// 尽力而为：装不上不影响守护进程，所以逐个返回结果，由调用方决定要不要理。
pub fn install_all(home: &Path) -> Vec<(&'static str, io::Result<Installed>)> {
    AGENT_DIRS.iter().map(|d| (*d, install_one(home, d))).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn skill_path(home: &Path, d: &str) -> std::path::PathBuf {
        home.join(d).join("skills").join("dct-game").join("SKILL.md")
    }

    #[test]
    fn the_card_has_front_matter_on_line_one_and_the_marker() {
        assert!(SKILL_MD.starts_with("---\nname: dct-game\ndescription: "));
        assert!(SKILL_MD.contains(MARKER));
    }

    #[test]
    fn installs_into_every_agent_that_exists_and_skips_the_rest() {
        let h = tempfile::tempdir().unwrap();
        std::fs::create_dir(h.path().join(".claude")).unwrap();
        std::fs::create_dir(h.path().join(".qwen")).unwrap();
        let r = install_all(h.path());
        let got: Vec<_> = r.iter().map(|(d, x)| (*d, x.as_ref().unwrap())).collect();
        assert_eq!(got, vec![(".claude", &Installed::Wrote), (".codex", &Installed::NoAgent), (".qwen", &Installed::Wrote)]);
        assert_eq!(std::fs::read_to_string(skill_path(h.path(), ".claude")).unwrap(), SKILL_MD);
        assert!(!h.path().join(".codex").exists(), "不替没装的 agent 建目录");
    }

    #[test]
    fn a_second_install_writes_nothing() {
        let h = tempfile::tempdir().unwrap();
        std::fs::create_dir(h.path().join(".codex")).unwrap();
        install_all(h.path());
        let before = std::fs::metadata(skill_path(h.path(), ".codex")).unwrap().modified().unwrap();
        std::thread::sleep(std::time::Duration::from_millis(20));
        assert_eq!(install_all(h.path())[1].1.as_ref().unwrap(), &Installed::Same);
        assert_eq!(std::fs::metadata(skill_path(h.path(), ".codex")).unwrap().modified().unwrap(), before);
    }

    #[test]
    fn an_old_version_of_our_card_is_updated() {
        let h = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(skill_path(h.path(), ".claude").parent().unwrap()).unwrap();
        std::fs::write(skill_path(h.path(), ".claude"), format!("旧的\n{MARKER}\n")).unwrap();
        assert_eq!(install_all(h.path())[0].1.as_ref().unwrap(), &Installed::Wrote);
        assert_eq!(std::fs::read_to_string(skill_path(h.path(), ".claude")).unwrap(), SKILL_MD);
    }

    #[test]
    fn a_users_own_file_with_the_same_name_is_never_overwritten() {
        let h = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(skill_path(h.path(), ".claude").parent().unwrap()).unwrap();
        std::fs::write(skill_path(h.path(), ".claude"), "我自己写的").unwrap();
        assert_eq!(install_all(h.path())[0].1.as_ref().unwrap(), &Installed::UserOwned);
        assert_eq!(std::fs::read_to_string(skill_path(h.path(), ".claude")).unwrap(), "我自己写的");
    }
}
```

- [ ] **创建 `src/game/cli.rs`**（下面的代码已在临时副本里编译、跑过测试）

```rust
//! `dct game play [--game 名字] [--steps N] [--dry-run]`。
use super::{log::LogFile, profile, text};
use dct_game::play::{play, Clock, Options};

const USAGE: &str = "用法：dct game play [--game candy-crush] [--steps 20] [--dry-run]";

pub struct Args {
    pub game: String,
    pub steps: usize,
    pub dry_run: bool,
}

pub fn parse(args: &[String]) -> Result<Args, String> {
    if args.first().map(String::as_str) != Some("play") {
        return Err(USAGE.into());
    }
    let mut a = Args { game: "candy-crush".into(), steps: 20, dry_run: false };
    let mut it = args[1..].iter();
    while let Some(flag) = it.next() {
        match flag.as_str() {
            "--dry-run" => a.dry_run = true,
            "--game" => a.game = it.next().ok_or("--game 后面要写游戏名")?.clone(),
            "--steps" => {
                let v = it.next().ok_or("--steps 后面要写步数")?;
                a.steps = v.parse().ok().filter(|n| (1..=200).contains(n)).ok_or("--steps 要在 1 到 200 之间")?;
            }
            other => return Err(format!("不认识 {other}。{USAGE}")),
        }
    }
    Ok(a)
}

struct SystemClock;

impl Clock for SystemClock {
    fn now_ms(&self) -> u64 {
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
    }
    fn sleep_ms(&mut self, ms: u64) {
        std::thread::sleep(std::time::Duration::from_millis(ms));
    }
}

pub fn run(args: &[String]) -> i32 {
    let a = match parse(args) {
        Ok(a) => a,
        Err(m) => {
            eprintln!("{m}");
            return 2;
        }
    };
    run_parsed(&a)
}

#[cfg(not(unix))]
fn run_parsed(_: &Args) -> i32 {
    eprintln!("这一版只支持 Mac：玩游戏要用 dco，而 dco 目前只有 Mac 版。");
    1
}

#[cfg(unix)]
fn run_parsed(a: &Args) -> i32 {
    use dct_game::dco::DcoClient;
    use serde_json::json;

    let Some(home) = crate::sys::home() else {
        eprintln!("找不到家目录。");
        return 1;
    };
    let loaded = match profile::load(&home, &a.game) {
        Ok(l) => l,
        Err(m) => {
            eprintln!("{m}");
            return 1;
        }
    };
    let mut dco = match DcoClient::connect(&home.join(".dco")) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("{}", text::dco_error(&e));
            return 1;
        }
    };
    let clock = SystemClock;
    let log = match LogFile::open(&home, clock.now_ms() / 1000) {
        Ok(l) => l,
        Err(e) => {
            // 记录是这件事的要点（以后拿去比），写不了就不开始。
            eprintln!("记录文件开不了（{}）：{e}", profile::games_dir(&home).join("log").display());
            return 1;
        }
    };
    let (mut n, mut warned) = (0, false);
    let mut sink = |mut rec: serde_json::Value| {
        n += 1;
        rec["game"] = json!(a.game);
        rec["profile_sha256"] = json!(loaded.sha256);
        if let Err(e) = log.append(&rec) {
            if !warned {
                eprintln!("记录文件写不进去了（{e}），后面的步不会被记下来。");
                warned = true;
            }
        }
        println!("{}", text::step_line(n, &rec));
    };
    let summary = play(&mut dco, &mut SystemClock, &loaded.profile, &Options { max_steps: a.steps, dry_run: a.dry_run }, &mut sink);
    let (line, code) = text::stop_line(&summary.stop, summary.steps, log.path());
    let _ = log.append(&json!({ "schema": 1, "game": a.game, "stop": text::stop_code(&summary.stop), "steps": summary.steps }));
    if code == 0 {
        println!("{line}");
    } else {
        eprintln!("{line}");
    }
    code
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(v: &[&str]) -> Result<Args, String> {
        parse(&v.iter().map(|s| s.to_string()).collect::<Vec<_>>())
    }

    #[test]
    fn defaults() {
        let a = p(&["play"]).unwrap();
        assert_eq!((a.game.as_str(), a.steps, a.dry_run), ("candy-crush", 20, false));
    }

    #[test]
    fn flags() {
        let a = p(&["play", "--steps", "50", "--dry-run", "--game", "x-1"]).unwrap();
        assert_eq!((a.game.as_str(), a.steps, a.dry_run), ("x-1", 50, true));
    }

    #[test]
    fn bad_input_is_refused_with_a_reason() {
        for bad in [&["play", "--steps", "0"][..], &["play", "--steps", "201"], &["play", "--steps", "x"], &["play", "--steps"], &["play", "--game"], &["play", "--nope"], &[], &["stop"]] {
            assert!(p(bad).is_err(), "{bad:?}");
        }
    }
}
```

- [ ] **Step 2: 在 `src/main.rs` 里接线**

在 `Some("keys") => ...` 这一行**上面**加：

```rust
        // 玩三消游戏：连的是本机的 dco，不经守护进程。
        Some("game") => std::process::exit(dct::game::cli::run(&args[1..])),
```

在 `HELP` 里 `  dct peers        看组里有哪些电脑` 这一行**上面**加：

```text
  dct game play [--game candy-crush] [--steps 20] [--dry-run]
                   让 dct 自己玩三消游戏（要先在 iPhone 镜像里打开一关；只支持 Mac）
```

- [ ] **Step 3: 在 `src/daemon.rs` 的 `pub fn run(socket: &Path)` 开头接上说明卡的安装线程**

把 `pub fn run` 的开头（`let mgr = SessionManager::new();` 之前）改成：

```rust
pub fn run(socket: &Path) -> Result<()> {
    // 「怎么让 AI 玩三消」的说明卡：真正的守护进程里才装。单元测试和集成测试把 socket 放进临时目录，
    // 它们绝不能去写用户真实的 `~/.claude` 等。每 30 秒看一遍，是因为 agent 第一次运行才会建出自己的
    // 目录——用户装了 agent、开了会话，不用重启守护进程说明卡也会出现。
    if socket == crate::proto::socket_path().as_path() {
        let _ = std::thread::Builder::new().name("game-skill".into()).spawn(|| loop {
            if let Some(home) = crate::sys::home() {
                let _ = crate::game::skill::install_all(&home);
            }
            std::thread::sleep(Duration::from_secs(30));
        });
    }
    let mgr = SessionManager::new();
```

（`Duration` 在 `src/daemon.rs` 第 7 行已经 `use std::time::Duration;`。）

- [ ] **Step 4: 写集成测试**


- [ ] **创建 `tests/game_cli.rs`**（下面的代码已在临时副本里编译、跑过测试）

```rust
//! `dct game play` 对着一个假 dco 跑一遍：握手、读棋盘、选步、记录、退出码。假 dco 按 dco 真实的
//! 握手和回复形状说话（见 dc-octo `src/client.rs`、`src/mcp.rs`）。
#![cfg(unix)]
use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixListener;
use std::process::Command;

const TOKEN: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

/// 9 行 5 列，每个类别不止一格；(0,2)↔(1,2) 能换出第 0 行的四连。
fn board() -> Value {
    let mut cells: Vec<Vec<u64>> = (0..9).map(|r| (0..5).map(|c| ((r + c) % 3) as u64).collect()).collect();
    cells[0] = vec![0, 0, 1, 0, 2];
    cells[1] = vec![1, 2, 0, 1, 0];
    let mut counts = [0usize; 3];
    cells.iter().flatten().for_each(|&c| counts[c as usize] += 1);
    json!({
        "rows": 9, "cols": 5, "cells": cells, "odd": vec![vec![false; 5]; 9],
        "classes": (0..3).map(|i| json!({"id": i, "rgb": [i, i, i], "count": counts[i as usize]})).collect::<Vec<_>>(),
        "observation_id": "obs-0000000000000001", "observed_at_ms": 1790942400000u64, "frame_age_ms": 5, "elapsed_ms": 1
    })
}

fn fake_dco(home: &std::path::Path, swipe_error: Option<&'static str>) -> std::thread::JoinHandle<Vec<String>> {
    let dir = home.join(".dco");
    std::fs::create_dir_all(&dir).unwrap();
    let sock = dir.join("dco.sock");
    std::fs::write(dir.join("endpoint.json"), json!({"socket": sock}).to_string()).unwrap();
    std::fs::write(dir.join("token"), TOKEN).unwrap();
    let l = UnixListener::bind(&sock).unwrap();
    std::thread::spawn(move || {
        let (s, _) = l.accept().unwrap();
        let mut w = s.try_clone().unwrap();
        let mut r = BufReader::new(s);
        let mut tools = vec![];
        let mut line = String::new();
        r.read_line(&mut line).unwrap();
        assert_eq!(serde_json::from_str::<Value>(line.trim()).unwrap()["dco_token"], TOKEN);
        writeln!(w, r#"{{"ok":true}}"#).unwrap();
        loop {
            line.clear();
            if r.read_line(&mut line).unwrap() == 0 {
                return tools;
            }
            let req: Value = serde_json::from_str(line.trim()).unwrap();
            let Some(id) = req.get("id") else { continue };
            let result = match req["method"].as_str().unwrap() {
                "tools/call" => {
                    let name = req["params"]["name"].as_str().unwrap().to_string();
                    tools.push(name.clone());
                    let (body, is_error) = match (name.as_str(), swipe_error) {
                        ("read_grid", _) => (board(), false),
                        ("swipe", Some(code)) => (json!({"error": {"code": code, "message": "x"}}), true),
                        ("swipe", None) => (json!({"swiped": true}), false),
                        (other, _) => panic!("没想到会调 {other}"),
                    };
                    json!({"content": [{"type": "text", "text": body.to_string()}], "isError": is_error})
                }
                _ => json!({"protocolVersion": "2025-06-18"}),
            };
            writeln!(w, "{}", json!({"jsonrpc": "2.0", "id": id, "result": result})).unwrap();
        }
    })
}

fn dct(home: &std::path::Path, args: &[&str]) -> (String, String, i32) {
    let o = Command::new(env!("CARGO_BIN_EXE_dct")).args(args).env("HOME", home).stdin(std::process::Stdio::null()).output().unwrap();
    (String::from_utf8_lossy(&o.stdout).into(), String::from_utf8_lossy(&o.stderr).into(), o.status.code().unwrap_or(-1))
}

fn log_lines(home: &std::path::Path) -> Vec<Value> {
    let dir = home.join(".dct/games/log");
    let f = std::fs::read_dir(&dir).unwrap().next().expect("没有记录文件").unwrap().path();
    std::fs::read_to_string(f).unwrap().lines().map(|l| serde_json::from_str(l).unwrap()).collect()
}

#[test]
fn a_dry_run_prints_the_move_never_swipes_and_logs_it() {
    let home = tempfile::tempdir().unwrap();
    let h = fake_dco(home.path(), None);
    let (out, err, code) = dct(home.path(), &["game", "play", "--dry-run"]);
    assert_eq!(code, 0, "{out}{err}");
    assert!(out.contains("第 1 步：") && out.contains("试走，没有真划"), "{out}");
    assert!(out.contains("试走结束"), "{out}");
    assert_eq!(h.join().unwrap(), vec!["read_grid"], "试走不该划");
    let lines = log_lines(home.path());
    assert_eq!(lines.len(), 2);
    assert_eq!(lines[0]["game"], "candy-crush");
    assert_eq!(lines[0]["profile_sha256"].as_str().unwrap().len(), 64);
    assert_eq!(lines[0]["observation_id"], "obs-0000000000000001");
    assert_eq!(lines[0]["outcome"], "dry_run");
    assert_eq!((lines[1]["stop"].as_str(), lines[1]["steps"].as_u64()), (Some("dry_run"), Some(1)));
}

#[test]
fn a_halted_dco_stops_the_game_with_a_plain_sentence_and_a_failing_exit_code() {
    let home = tempfile::tempdir().unwrap();
    let h = fake_dco(home.path(), Some("halted"));
    let (out, err, code) = dct(home.path(), &["game", "play", "--steps", "3"]);
    assert_eq!(code, 1, "{out}{err}");
    assert!(err.contains("急停"), "{err}");
    assert_eq!(h.join().unwrap(), vec!["read_grid", "swipe"]);
    assert_eq!(log_lines(home.path()).last().unwrap()["stop"], "dco");
}

#[test]
fn without_dco_it_says_so_and_does_not_create_a_log() {
    let home = tempfile::tempdir().unwrap();
    let (out, err, code) = dct(home.path(), &["game", "play"]);
    assert_eq!(code, 1, "{out}{err}");
    assert!(err.contains("dco 没在运行"), "{err}");
    assert!(!home.path().join(".dct/games/log").exists());
}

#[test]
fn bad_arguments_exit_2_and_show_usage() {
    let home = tempfile::tempdir().unwrap();
    for args in [&["game", "play", "--steps", "0"][..], &["game"], &["game", "play", "--nope"]] {
        let (_, err, code) = dct(home.path(), args);
        assert_eq!(code, 2, "{args:?} {err}");
        assert!(!err.is_empty());
    }
}

#[test]
fn a_broken_profile_file_names_itself() {
    let home = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(home.path().join(".dct/games")).unwrap();
    std::fs::write(home.path().join(".dct/games/candy-crush.toml"), "rows = \"x\"").unwrap();
    let (_, err, code) = dct(home.path(), &["game", "play", "--dry-run"]);
    assert_eq!(code, 1);
    assert!(err.contains("candy-crush.toml"), "{err}");
}

#[test]
fn help_lists_the_game_command() {
    let home = tempfile::tempdir().unwrap();
    let (out, _, _) = dct(home.path(), &["--help"]);
    assert!(out.contains("dct game play"), "{out}");
}
```

- [ ] **Step 5: 跑测试**

Run: `cargo test --lib game:: && cargo test --test game_cli`
Expected: 库里 `21 passed`（profile 5、log 2、text 6、skill 5、cli 3），集成 `6 passed`。

再跑一遍全仓库，确认没碰坏别的（已在临时副本里跑过：库 1646 个单元测试全过）：

Run: `cargo test --workspace`
Expected: 全绿。

- [ ] **Step 6: 提交**

```bash
cargo clippy --all-targets
git add src tests/game_cli.rs
git commit -m "feat(game): dct game play, plain-Chinese output, and the skill card for Claude Code, Codex and Qwen"
```


---

