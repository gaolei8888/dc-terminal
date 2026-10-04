//! `dct game scene [--game 名字] [--steps N] [--dry-run]`：寻物游戏的场景里自动玩。读屏幕（私人画面就停）→
//! 截图交给会看图的模型指一个位置 → 过安全检查 → 用 dco 按位置点 → 看有没有变化 → 记一条教学记录。
//! 循环本身在 `crates/dct-game/src/scene.rs`；这里接命令行、模型、记录文件和给用户看的话。
//! 设计：docs/superpowers/plans/2026-10-04-dct-game-scene.md。

const USAGE: &str = "用法：dct game scene [--game 名字] [--steps 10] [--dry-run]";

#[derive(Debug, PartialEq)]
pub struct Args {
    pub game: String,
    pub steps: usize,
    pub dry_run: bool,
}

pub fn parse(args: &[String]) -> Result<Args, String> {
    let mut a = Args { game: "candy-crush".into(), steps: 10, dry_run: false };
    let mut it = args.iter();
    while let Some(flag) = it.next() {
        match flag.as_str() {
            "--dry-run" => a.dry_run = true,
            "--game" => a.game = it.next().ok_or("--game 后面要写游戏名")?.clone(),
            "--steps" => {
                let v = it.next().ok_or("--steps 后面要写步数")?;
                a.steps = v.parse().ok().filter(|n| (1..=100).contains(n)).ok_or("--steps 要在 1 到 100 之间")?;
            }
            other => return Err(format!("不认识 {other}。{USAGE}")),
        }
    }
    Ok(a)
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
    eprintln!("这一版只支持 Mac：看场景要用 dco，而 dco 目前只有 Mac 版。");
    1
}

#[cfg(unix)]
pub use unix::{LlmVision, Recorder};

#[cfg(unix)]
fn run_parsed(a: &Args) -> i32 {
    use super::{cli::SystemClock, profile, text};
    use dct_game::dco::DcoClient;
    use dct_game::play::Clock;

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
    // 先问模型再连 dco：没配大模型就什么都不碰。
    let llm = match crate::cli::load_llm_backend() {
        Ok(l) => l,
        Err(crate::cli::LoadLlmError::NotEnabled(_)) => {
            println!("没开大模型，没法看场景。");
            return 0;
        }
        Err(crate::cli::LoadLlmError::Problem { .. }) => {
            println!("大模型连不上，没法看场景。可以先运行 dct llm check 看原因。");
            return 0;
        }
    };
    let vision = LlmVision::new(llm.backend, llm.model);
    let clock = SystemClock;
    let rec = match Recorder::open(&home, &a.game, clock.now_ms() / 1000) {
        Ok(r) => r,
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
    unix::run_core(a, &loaded, &mut dco, &mut SystemClock, &vision, &rec)
}

#[cfg(unix)]
mod unix {
    use super::Args;
    use crate::game::profile::Loaded;
    use crate::llm::{complete_counted_with_timeout, Backend, Prompt};
    use base64::Engine;
    use dct_game::play::{Clock, Dco};
    use dct_game::scene::{parse_pick, scene, SceneOptions, SceneStep, SceneStop, Vision, VisionAnswer};
    use std::io::Write;
    use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
    use std::path::{Path, PathBuf};
    use std::sync::Arc;
    use std::time::Duration;

    const SYSTEM: &str = "你在帮一个寻物冒险手机游戏的自动玩家看场景。只用 JSON 回答，不要别的话。";

    pub struct LlmVision {
        backend: Arc<dyn Backend>,
        model: String,
        timeout: Duration,
    }

    impl LlmVision {
        pub fn new(backend: Arc<dyn Backend>, model: String) -> LlmVision {
            LlmVision { backend, model, timeout: Duration::from_secs(60) }
        }
    }

    fn question(history: &[(u16, u16)]) -> String {
        let seen: Vec<String> = history.iter().map(|(x, y)| format!("({:.2}, {:.2})", *x as f64 / 10000.0, *y as f64 / 10000.0)).collect();
        format!(
            "这是一个寻物冒险手机游戏的场景截图（横屏）。我要继续玩，请选出下一步最值得点的一个场景物件\
             （门、箱子、楼梯、工具、可拾取的东西等），不要选菜单、按钮、物品栏、提示、右上的金币。\
             已经点过的位置（不要重复）：[{}]。只用 JSON 回答：{{\"name\": 物件名, \"x\": 0到1的横向位置, \"y\": 0到1的纵向位置, \"why\": 一句话}}",
            seen.join(", ")
        )
    }

    impl Vision for LlmVision {
        fn pick(&self, png: &[u8], history: &[(u16, u16)]) -> Option<VisionAnswer> {
            let p = Prompt {
                system: SYSTEM.into(),
                user: question(history),
                max_tokens: 300,
                image_png_base64: Some(base64::engine::general_purpose::STANDARD.encode(png)),
            };
            let (raw, usage) = complete_counted_with_timeout(self.backend.clone(), p, self.timeout).ok()?;
            Some(VisionAnswer { pick: parse_pick(&raw), raw, model: self.model.clone(), tokens: usage.map(|u| (u.input, u.output)) })
        }
    }

    /// `~/.dct/learn/<游戏>/<日期>/steps.jsonl` 和 `png/NNNN.png`。目录 0700、文件 0600，只在本机。
    pub struct Recorder {
        dir: PathBuf,
        warned: std::cell::Cell<bool>,
    }

    impl Recorder {
        pub fn open(home: &Path, game: &str, secs_since_epoch: u64) -> Result<Recorder, String> {
            let (y, m, d) = crate::journal::civil_from_days((secs_since_epoch / 86_400) as i64);
            let dir = home.join(".dct").join("learn").join(game).join(format!("{y:04}-{m:02}-{d:02}"));
            let why = |e: std::io::Error| {
                let reason = match e.kind() {
                    std::io::ErrorKind::PermissionDenied => "没有写入的权限",
                    _ => "写不进去（可能是同名的文件挡着，或者磁盘满了）",
                };
                format!("教学记录的文件夹开不了：{}，{reason}。", dir.display())
            };
            std::fs::DirBuilder::new().recursive(true).mode(0o700).create(dir.join("png")).map_err(why)?;
            std::fs::OpenOptions::new().create(true).append(true).mode(0o600).open(dir.join("steps.jsonl")).map_err(why)?;
            Ok(Recorder { dir, warned: false.into() })
        }

        pub fn dir(&self) -> &Path {
            &self.dir
        }

        /// 写这一步：发给模型的那张图 + 一行记录。写不进去只说一次，不打断玩。
        pub fn write(&self, st: &SceneStep) {
            let r: std::io::Result<()> = (|| {
                if let (Some(png), Some(rel)) = (&st.png, st.record["screen"]["png"].as_str()) {
                    let mut f = std::fs::OpenOptions::new().create(true).write(true).truncate(true).mode(0o600).open(self.dir.join(rel))?;
                    f.write_all(png)?;
                }
                let mut f = std::fs::OpenOptions::new().create(true).append(true).mode(0o600).open(self.dir.join("steps.jsonl"))?;
                writeln!(f, "{}", st.record)
            })();
            if let Err(e) = r {
                if !self.warned.replace(true) {
                    eprintln!("教学记录写不进去了（{e}），后面的步不会被记下来。");
                }
            }
        }
    }

    /// 最后一句话和退出码：除了 dco 出错都是 0（停下是正常的）。
    pub fn stop_line(stop: &SceneStop, steps: usize) -> (String, i32) {
        match stop {
            SceneStop::StepsDone => (format!("走完了 {steps} 步。"), 0),
            SceneStop::DryRun => ("试走完成，没有真点。".into(), 0),
            SceneStop::PrivateScreen => ("这个画面看起来是私人内容，我没有读它，也没有操作。请切回游戏。".into(), 0),
            SceneStop::NoTapAt => ("这台章鱼还不会按位置点，等它更新后再试。".into(), 0),
            SceneStop::NoVision => ("大模型没回应，先停下了。".into(), 0),
            SceneStop::Skipped3 => ("连着 3 次没点（落点不安全或没看清），先停下，请你看看画面。".into(), 0),
            SceneStop::Noop5 => ("连着 5 次点了画面都没变化，先停下。".into(), 0),
            SceneStop::Dco(e) => (super::super::text::dco_error(e), 1),
        }
    }

    /// 整条命令（除了装配）。`dco`、`vision`、`clock` 都是传进来的，测试里换成假的。
    pub fn run_core(a: &Args, loaded: &Loaded, dco: &mut dyn Dco, clock: &mut dyn Clock, vision: &dyn Vision, rec: &Recorder) -> i32 {
        let o = SceneOptions { max_steps: a.steps, dry_run: a.dry_run, no_tap: loaded.no_tap.clone(), game: a.game.clone() };
        let s = scene(dco, clock, &loaded.profile, vision, &o, &mut |st| {
            rec.write(&st);
            println!("{}", st.say);
        });
        let (line, code) = stop_line(&s.stop, s.steps);
        if code == 0 {
            println!("{line}");
        } else {
            eprintln!("{line}");
        }
        if s.asks > 0 {
            if s.tokens_in + s.tokens_out > 0 {
                println!("这次问了大模型 {} 次，一共用了约 {} 个 token（输入 {}，输出 {}）。", s.asks, s.tokens_in + s.tokens_out, s.tokens_in, s.tokens_out);
            } else {
                println!("这次问了大模型 {} 次（没有读到用量）。", s.asks);
            }
        }
        println!("教学记录在 {}", rec.dir().display());
        code
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(v: &[&str]) -> Result<Args, String> {
        parse(&v.iter().map(|s| s.to_string()).collect::<Vec<_>>())
    }

    #[test]
    fn parse_defaults_and_flags() {
        assert_eq!(p(&[]).unwrap(), Args { game: "candy-crush".into(), steps: 10, dry_run: false });
        assert_eq!(p(&["--game", "x-1", "--steps", "100", "--dry-run"]).unwrap(), Args { game: "x-1".into(), steps: 100, dry_run: true });
    }

    #[test]
    fn parse_refuses_bad_input_with_a_reason() {
        for bad in [&["--steps", "0"][..], &["--steps", "101"], &["--steps", "x"], &["--steps"], &["--game"], &["--nope"]] {
            assert!(p(bad).is_err(), "{bad:?}");
        }
        assert!(p(&["--nope"]).unwrap_err().contains("不认识 --nope"));
    }
}

#[cfg(all(test, unix))]
mod unix_tests {
    use super::unix::{run_core, stop_line};
    use super::*;
    use crate::game::profile;
    use crate::llm::{Backend, LlmError, Prompt, Usage};
    use dct_game::play::{Clock, Dco, DcoError, Profile, Region, Seen, TapAt};
    use dct_game::scene::{SceneStop, Vision, VisionAnswer};
    use std::os::unix::fs::PermissionsExt;
    use std::sync::{Arc, Mutex};

    struct Fixed(Result<(String, Option<Usage>), LlmError>, Mutex<Vec<Prompt>>);
    impl Backend for Fixed {
        fn complete(&self, p: &Prompt) -> Result<String, LlmError> {
            self.complete_counted(p).map(|x| x.0)
        }
        fn complete_counted(&self, p: &Prompt) -> Result<(String, Option<Usage>), LlmError> {
            self.1.lock().unwrap().push(p.clone());
            self.0.clone()
        }
    }

    fn vision(r: Result<(String, Option<Usage>), LlmError>) -> (LlmVision, Arc<Fixed>) {
        let b = Arc::new(Fixed(r, Mutex::new(vec![])));
        (LlmVision::new(b.clone(), "qwen-x".into()), b)
    }

    #[test]
    fn llm_vision_parses_a_json_answer_and_sends_the_image_and_history() {
        let (v, b) = vision(Ok((r#"好的 {"name":"木箱","x":0.18,"y":0.6,"why":"近"}"#.into(), Some(Usage { input: 616, output: 66 }))));
        let a = v.pick(&[1, 2, 3], &[(1800, 6000)]).unwrap();
        let k = a.pick.unwrap();
        assert_eq!((k.name.as_str(), k.x, k.y), ("木箱", 0.18, 0.6));
        assert_eq!((a.model.as_str(), a.tokens), ("qwen-x", Some((616, 66))));
        let sent = b.1.lock().unwrap();
        assert_eq!(sent[0].image_png_base64.as_deref(), Some("AQID"));
        assert_eq!(sent[0].max_tokens, 300);
        assert!(sent[0].user.contains("(0.18, 0.60)"), "{}", sent[0].user);
    }

    #[test]
    fn llm_vision_garbage_is_an_answer_without_a_pick_and_a_failure_is_none() {
        let (v, _) = vision(Ok(("我看不懂".into(), None)));
        let a = v.pick(&[1], &[]).unwrap();
        assert!(a.pick.is_none() && a.raw == "我看不懂");
        let (v, _) = vision(Err(LlmError::Unavailable));
        assert!(v.pick(&[1], &[]).is_none());
    }

    #[test]
    fn the_recorder_makes_private_dirs_and_files() {
        let h = tempfile::tempdir().unwrap();
        // 2026-10-04 UTC
        let r = Recorder::open(h.path(), "g", 1_791_072_000).unwrap();
        assert!(r.dir().ends_with(".dct/learn/g/2026-10-04"), "{:?}", r.dir());
        let mode = |p: &std::path::Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(r.dir()), 0o700);
        assert_eq!(mode(&r.dir().join("png")), 0o700);
        assert_eq!(mode(&r.dir().join("steps.jsonl")), 0o600);
        assert_eq!(mode(&h.path().join(".dct/learn")), 0o700);
    }

    // ---- 整条命令：假 dco + 假模型 ----

    struct Dc {
        private: bool,
        captures: usize,
        taps: usize,
    }
    impl Dco for Dc {
        fn read_grid(&mut self, _: &Profile) -> Result<dct_game::GridRead, DcoError> {
            panic!()
        }
        fn swipe(&mut self, _: &Profile, _: (f64, f64), _: (f64, f64)) -> Result<(), DcoError> {
            panic!()
        }
        fn see_text(&mut self, _: &Profile) -> Result<Seen, DcoError> {
            if self.private {
                return Err(DcoError { code: "private_screen".into(), message: "私人".into() });
            }
            Ok(Seen { snapshot_id: "s".into(), observation_id: None, elements: vec![] })
        }
        fn capture(&mut self, _: &Profile) -> Result<Vec<u8>, DcoError> {
            self.captures += 1;
            Ok(vec![9, 9])
        }
        fn tap_at(&mut self, _: &Profile, _: u16, _: u16, _: &[Region]) -> Result<TapAt, DcoError> {
            self.taps += 1;
            Ok(TapAt { kind: "no_text".into(), text: String::new() })
        }
    }
    struct Clk;
    impl Clock for Clk {
        fn now_ms(&self) -> u64 {
            0
        }
        fn sleep_ms(&mut self, _: u64) {}
    }
    struct One;
    impl Vision for One {
        fn pick(&self, _: &[u8], _: &[(u16, u16)]) -> Option<VisionAnswer> {
            let pick = dct_game::scene::parse_pick(r#"{"name":"木箱","x":0.5,"y":0.5,"why":"w"}"#);
            Some(VisionAnswer { pick, raw: String::new(), model: "m".into(), tokens: Some((5, 1)) })
        }
    }

    fn setup() -> (tempfile::TempDir, profile::Loaded, Recorder) {
        let h = tempfile::tempdir().unwrap();
        let loaded = profile::load(h.path(), "candy-crush").unwrap();
        let rec = Recorder::open(h.path(), "candy-crush", 1_791_072_000).unwrap();
        (h, loaded, rec)
    }

    #[test]
    fn a_dry_run_writes_the_record_and_the_sent_image_and_taps_nothing() {
        let (_h, loaded, rec) = setup();
        let mut d = Dc { private: false, captures: 0, taps: 0 };
        let a = Args { game: "candy-crush".into(), steps: 5, dry_run: true };
        assert_eq!(run_core(&a, &loaded, &mut d, &mut Clk, &One, &rec), 0);
        assert_eq!(d.taps, 0);
        let line = std::fs::read_to_string(rec.dir().join("steps.jsonl")).unwrap();
        let v: serde_json::Value = serde_json::from_str(line.lines().next().unwrap()).unwrap();
        assert_eq!((v["label"].as_str(), v["game"].as_str(), v["screen"]["png"].as_str()), (Some("dry_run"), Some("candy-crush"), Some("png/0001.png")));
        assert_eq!(std::fs::read(rec.dir().join("png/0001.png")).unwrap(), vec![9, 9]);
    }

    #[test]
    fn a_private_screen_records_nothing_and_exits_zero_without_capturing() {
        let (_h, loaded, rec) = setup();
        let mut d = Dc { private: true, captures: 0, taps: 0 };
        let a = Args { game: "candy-crush".into(), steps: 5, dry_run: false };
        assert_eq!(run_core(&a, &loaded, &mut d, &mut Clk, &One, &rec), 0);
        assert_eq!((d.captures, d.taps), (0, 0));
        assert!(std::fs::read_to_string(rec.dir().join("steps.jsonl")).unwrap().is_empty());
        assert!(!rec.dir().join("png/0001.png").exists());
    }

    #[test]
    fn stop_lines_are_plain_and_only_dco_errors_exit_nonzero() {
        for s in [SceneStop::StepsDone, SceneStop::DryRun, SceneStop::PrivateScreen, SceneStop::NoTapAt, SceneStop::NoVision, SceneStop::Skipped3, SceneStop::Noop5] {
            assert_eq!(stop_line(&s, 3).1, 0, "{s:?}");
        }
        assert!(stop_line(&SceneStop::NoTapAt, 0).0.contains("还不会按位置点"));
        assert_eq!(stop_line(&SceneStop::Dco(DcoError { code: "halted".into(), message: "x".into() }), 0).1, 1);
    }
}
