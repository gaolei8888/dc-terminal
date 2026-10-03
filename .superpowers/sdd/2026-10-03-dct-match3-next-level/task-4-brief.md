### Task 4: 大白话、命令行开关、说明卡

**Files:**
- Modify: `src/game/cli.rs`、`src/game/skill.md`

**Interfaces:**
- Consumes: 任务 2、3 的 `Stop` 新变体、`auto_next`、`NavOptions`。
- Produces: `dct game play --auto-next [--tries N]`（`--tries` 1～20，默认 5；不带 `--auto-next` 行为和第一轮完全一样）；记录里 `kind:"nav"` 的行也写进日志，屏幕上用 `text::nav_line` 印一句话。

下面两个文件**整个换成**给出的内容（它们是第一轮文件加上这一步的改动，已在临时副本里编译、测过）。


- [ ] **整个替换 `src/game/cli.rs`**（下面的代码已在临时副本里编译、跑过测试）

```rust
//! `dct game play [--game 名字] [--steps N] [--dry-run] [--auto-next] [--tries N]`。

const USAGE: &str = "用法：dct game play [--game candy-crush] [--steps 20] [--dry-run] [--auto-next] [--tries 5]";

pub struct Args {
    pub game: String,
    pub steps: usize,
    pub dry_run: bool,
    /// 一局结束以后自己接着来（失败了重来、关安全的弹窗）。不带它就和以前一样：打完一关就停。
    pub auto_next: bool,
    /// 整个命令最多点几次“开始 / 再来一次”（每次都会用掉一条生命）。
    pub tries: usize,
}

pub fn parse(args: &[String]) -> Result<Args, String> {
    if args.first().map(String::as_str) != Some("play") {
        return Err(USAGE.into());
    }
    let mut a = Args { game: "candy-crush".into(), steps: 20, dry_run: false, auto_next: false, tries: 5 };
    let mut it = args[1..].iter();
    while let Some(flag) = it.next() {
        match flag.as_str() {
            "--dry-run" => a.dry_run = true,
            "--auto-next" => a.auto_next = true,
            "--tries" => {
                let v = it.next().ok_or("--tries 后面要写次数")?;
                a.tries = v.parse().ok().filter(|n| (1..=20).contains(n)).ok_or("--tries 要在 1 到 20 之间")?;
            }
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

#[cfg(unix)]
struct SystemClock;

#[cfg(unix)]
impl dct_game::play::Clock for SystemClock {
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
    use super::{log::LogFile, profile, text};
    use dct_game::dco::DcoClient;
    use dct_game::navigate::{auto_next, NavOptions};
    use dct_game::play::{play, Clock, Options};
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
        Err(m) => {
            // 记录是这件事的要点（以后拿去比），写不了就不开始。
            eprintln!("{m}");
            return 1;
        }
    };
    // 这一次运行的编号：同一次里的每条记录都带它，以后才分得清哪些步属于同一盘。
    let run_id = format!("{:08x}", (clock.now_ms() ^ u64::from(std::process::id())) as u32);
    let (mut n, mut warned) = (0, false);
    let mut sink = |mut rec: serde_json::Value| {
        rec["game"] = json!(a.game);
        rec["run_id"] = json!(run_id);
        rec["profile_sha256"] = json!(loaded.sha256);
        if let Err(e) = log.append(&rec) {
            if !warned {
                eprintln!("记录文件写不进去了（{e}），后面的步不会被记下来。");
                warned = true;
            }
        }
        if rec["kind"] == "nav" {
            if let Some(line) = text::nav_line(&rec) {
                println!("{line}");
            }
        } else {
            n += 1;
            println!("{}", text::step_line(n, &rec));
        }
    };
    let summary = if a.auto_next {
        auto_next(&mut dco, &mut SystemClock, &loaded.profile, &NavOptions { max_steps: a.steps, tries: a.tries, dry_run: a.dry_run }, &mut sink)
    } else {
        play(&mut dco, &mut SystemClock, &loaded.profile, &Options { max_steps: a.steps, dry_run: a.dry_run }, &mut sink)
    };
    let (line, code) = text::stop_line(&summary.stop, summary.steps, log.path());
    let _ = log.append(&json!({ "schema": 1, "run_id": run_id, "time_ms": SystemClock.now_ms(), "game": a.game, "stop": text::stop_code(&summary.stop), "steps": summary.steps }));
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
        assert_eq!((a.auto_next, a.tries), (false, 5));
    }

    #[test]
    fn flags() {
        let a = p(&["play", "--steps", "50", "--dry-run", "--game", "x-1"]).unwrap();
        assert_eq!((a.game.as_str(), a.steps, a.dry_run), ("x-1", 50, true));
    }

    #[test]
    fn auto_next_flags() {
        let a = p(&["play", "--auto-next", "--tries", "3"]).unwrap();
        assert_eq!((a.auto_next, a.tries), (true, 3));
    }

    #[test]
    fn bad_input_is_refused_with_a_reason() {
        for bad in [&["play", "--steps", "0"][..], &["play", "--steps", "201"], &["play", "--steps", "x"], &["play", "--steps"], &["play", "--game"], &["play", "--nope"], &["play", "--tries", "0"], &["play", "--tries", "21"], &["play", "--tries"], &[], &["stop"]] {
            assert!(p(bad).is_err(), "{bad:?}");
        }
    }
}
```

- [ ] **整个替换 `src/game/skill.md`**（下面的代码已在临时副本里编译、跑过测试）

```markdown
---
name: dct-game
description: 用户想让 AI 玩三消游戏（比如 Candy Crush）时使用。运行 dct game play，按规则一步一步玩，每步很快，不用截图问大模型。
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

## 想让它失败了自己重来、一直玩

用户说「失败了自己重来」「一直玩」「帮我多打几局」，就运行：

    dct game play --auto-next

可以加 `--steps 50`（一共最多走几步棋）和 `--tries 3`（最多点几次「开始」「重来」，默认 5，每一次都会用掉一条生命）。它会：

- 一局没过时，自己点「Try again」「Play」重来同一关；
- 弹窗只关「Close」「Not now」「No thanks」这一类安全的，其它一律不点；
- 遇到要花钱的画面、广告、生命用完、通关了、不认识的画面，就停下，什么都不点；
- 也会一边做一边印一行，例如「点了「Play」，开始新的一局」「这一局没过，点了「Try again」重来」。

把这些话转述给用户。它停下以后，把最后一句话原样告诉用户（比如「生命用完了」「出现了广告，请你自己关掉」），由用户决定下一步。

## 停

- 用户说「停」，马上中断这条命令。
- 命令自己停下时（没有能走的步、读不出棋盘、画面一直在动、连着两次没反应、dco 急停），把它印的最后一句话原样告诉用户。
- 不要自己去点屏幕补救，也不要换别的办法去操作游戏；让用户决定下一步。

## 不要做

- 不要自己调用 dco 去点关卡按钮、道具、广告、付款。`--auto-next` 停下以后，也不要替它去点屏幕上的按钮、关广告或买东西。
- 不要同时开两条 `dct game play`。
```

- [ ] **Step 2: 跑测试，并确认主 crate 又能编**

Run: `cargo test --lib game::`
Expected: `31 passed`（任务 2 之后的 30 个 + `cli` 新的 1 个）。

Run: `cargo build`
Expected: 通过。

- [ ] **Step 3: 提交**

```bash
cargo clippy --workspace --all-targets --locked -- -D warnings
git add src/game
git commit -m "feat(game): dct game play --auto-next and --tries, and the skill card learns the flag"
```

---

