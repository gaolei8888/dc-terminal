//! `dct game play [--game 名字] [--steps N] [--dry-run] [--auto-next] [--tries N]`。

const USAGE: &str = "用法：dct game play [--game candy-crush] [--steps 20] [--dry-run] [--auto-next] [--tries 5] [--ask-model] [--goal \"清掉冰块\"]";

pub struct Args {
    pub game: String,
    pub steps: usize,
    pub dry_run: bool,
    /// 一局结束以后自己接着来（失败了重来、关安全的弹窗）。不带它就和以前一样：打完一关就停。
    pub auto_next: bool,
    /// 整个命令最多点几次“开始 / 再来一次”（每次都会用掉一条生命）。
    pub tries: usize,
    /// 每一步都问大模型选哪一步（没配大模型就只用规则）。
    pub ask_model: bool,
    /// 告诉大模型这一关要干什么。
    pub goal: Option<String>,
}

pub fn parse(args: &[String]) -> Result<Args, String> {
    if args.first().map(String::as_str) != Some("play") {
        return Err(USAGE.into());
    }
    let mut a = Args { game: "candy-crush".into(), steps: 20, dry_run: false, auto_next: false, tries: 5, ask_model: false, goal: None };
    let mut it = args[1..].iter();
    while let Some(flag) = it.next() {
        match flag.as_str() {
            "--dry-run" => a.dry_run = true,
            "--auto-next" => a.auto_next = true,
            "--tries" => {
                let v = it.next().ok_or("--tries 后面要写次数")?;
                a.tries = v.parse().ok().filter(|n| (1..=20).contains(n)).ok_or("--tries 要在 1 到 20 之间")?;
            }
            "--ask-model" => a.ask_model = true,
            "--goal" => a.goal = Some(it.next().ok_or("--goal 后面要写目标，比如 --goal \"清掉冰块\"")?.clone()),
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
    let advisor: Option<super::advisor::LlmAdvisor> = if a.ask_model {
        match crate::cli::load_llm_backend() {
            Ok(l) => Some(super::advisor::LlmAdvisor::new(l.backend, l.model)),
            Err(crate::cli::LoadLlmError::NotEnabled(_)) => {
                println!("没开大模型，这次只用规则玩。");
                None
            }
            Err(crate::cli::LoadLlmError::Problem { .. }) => {
                println!("大模型连不上，这次只用规则玩。可以先运行 dct llm check 看原因。");
                None
            }
        }
    } else {
        None
    };
    let goal = a.goal.clone().unwrap_or_else(|| "尽量多消".into());
    let adv_ref: Option<&dyn dct_game::ask::Advisor> = advisor.as_ref().map(|x| x as &dyn dct_game::ask::Advisor);
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
        auto_next(&mut dco, &mut SystemClock, &loaded.profile, &NavOptions { max_steps: a.steps, tries: a.tries, dry_run: a.dry_run, advisor: adv_ref, ask_always: a.ask_model, ask_budget: 30, goal: &goal }, &mut sink)
    } else {
        play(&mut dco, &mut SystemClock, &loaded.profile, &Options { max_steps: a.steps, dry_run: a.dry_run, advisor: adv_ref, ask_always: a.ask_model, ask_budget: 30, goal: &goal }, &mut sink)
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

    #[test]
    fn parse_ask_model_and_goal() {
        let a = parse(&["play".into(), "--ask-model".into(), "--goal".into(), "清掉冰块".into()]).unwrap();
        assert!(a.ask_model);
        assert_eq!(a.goal.as_deref(), Some("清掉冰块"));
        let d = parse(&["play".into()]).unwrap();
        assert!(!d.ask_model);
        assert_eq!(d.goal, None);
        assert!(parse(&["play".into(), "--goal".into()]).is_err());
    }

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
