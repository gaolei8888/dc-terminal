//! `dct game scene [--game 名字] [--steps N] [--dry-run]`：寻物游戏的场景里自动玩。读屏幕（私人画面就停）→
//! 截图交给会看图的模型指一个位置 → 过安全检查 → 用 dco 按位置点 → 看有没有变化 → 记一条教学记录。
//! 循环本身在 `crates/dct-game/src/scene.rs`；这里接命令行、模型、记录文件和给用户看的话。
//! 设计：docs/superpowers/plans/2026-10-04-dct-game-scene.md。

const USAGE: &str = "用法：dct game scene [--game 名字] [--steps 10] [--dry-run] [--brain config,claude] [--goal 一句话目标]";

#[derive(Debug, PartialEq)]
pub struct Args {
    pub game: String,
    pub steps: usize,
    pub dry_run: bool,
    /// 大脑升级链：`config`（[llm] 里配的）、`claude`（本机登录的 Claude Code）。
    pub brain: Vec<String>,
    /// 可选的目标，一句话（比如「找到蜂蜜」）；不写就和以前一样。
    pub goal: Option<String>,
}

/// 用了 Claude 就得先说清楚画面会发给谁。
pub const CLAUDE_NOTICE: &str = "这次会把确认是游戏的画面发给 Claude（先用文字识别确认不是私人画面）。";

pub fn privacy_notice(brain: &[String]) -> Option<&'static str> {
    brain.iter().any(|b| b == "claude").then_some(CLAUDE_NOTICE)
}

fn parse_brain(v: &str) -> Result<Vec<String>, String> {
    let names: Vec<String> = v.split(',').map(|n| n.trim().to_string()).collect();
    match names.iter().find(|n| !matches!(n.as_str(), "config" | "claude")) {
        Some(bad) => Err(format!("不认识的大脑「{bad}」。只能写 config（设置里配的大模型）或 claude（本机登录的 Claude Code），用逗号隔开，比如 --brain config,claude。")),
        None => Ok(names),
    }
}

pub fn parse(args: &[String]) -> Result<Args, String> {
    let mut a = Args { game: "candy-crush".into(), steps: 10, dry_run: false, brain: vec!["config".into()], goal: None };
    let mut it = args.iter();
    while let Some(flag) = it.next() {
        match flag.as_str() {
            "--dry-run" => a.dry_run = true,
            "--game" => a.game = it.next().ok_or("--game 后面要写游戏名")?.clone(),
            "--brain" => a.brain = parse_brain(it.next().ok_or("--brain 后面要写大脑名，比如 config,claude")?)?,
            "--goal" => {
                let v = it.next().map(|v| v.trim()).filter(|v| !v.is_empty());
                a.goal = Some(v.ok_or("--goal 后面要写一句话目标，比如 --goal \"找到蜂蜜\"")?.to_string());
            }
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
pub use unix::{ChainVision, LlmVision, Recorder};

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
    // 先建大脑再连 dco：大脑不可用就什么都不碰。
    let mut brains: Vec<(String, Box<dyn dct_game::scene::Vision>)> = vec![];
    for name in &a.brain {
        let (backend, model) = if name == "claude" {
            match crate::cli::load_claude_backend() {
                Some(b) => (b, "claude".to_string()),
                None => {
                    println!("这台机器上没有可用的 Claude Code，没法看场景。");
                    return 0;
                }
            }
        } else {
            match crate::cli::load_llm_backend() {
                Ok(l) => (l.backend, l.model),
                Err(crate::cli::LoadLlmError::NotEnabled(_)) => {
                    println!("没开大模型，没法看场景。");
                    return 0;
                }
                Err(crate::cli::LoadLlmError::Problem { .. }) => {
                    println!("大模型连不上，没法看场景。可以先运行 dct llm check 看原因。");
                    return 0;
                }
            }
        };
        brains.push((name.clone(), Box::new(LlmVision::new(backend, model).with_goal(a.goal.clone()))));
    }
    let vision = unix::ChainVision::new(brains);
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
    use crate::llm::{complete_counted_with_timeout, Backend, LlmError, Prompt};
    use base64::Engine;
    use dct_game::play::{Clock, Dco};
    use dct_game::scene::{parse_pick, BrainChain, scene, SceneOptions, SceneStep, SceneStop, Vision, VisionAnswer, VisionFail};
    use std::io::Write;
    use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
    use std::path::{Path, PathBuf};
    use std::sync::Arc;
    use std::time::Duration;

    const SYSTEM: &str = "你在帮一个寻物冒险手机游戏的自动玩家看场景。只用 JSON 回答，不要别的话。";

    /// 跑缩图命令：`(输入 png, 输出 jpg)`，成功才算。测试里换成假的。
    pub type Sips = Arc<dyn Fn(&Path, &Path) -> bool + Send + Sync>;

    /// 真的 macOS `sips`：最宽 1000 像素、JPEG 质量 70。
    fn real_sips() -> Sips {
        Arc::new(|i, o| {
            std::process::Command::new("sips")
                .args(["-s", "format", "jpeg", "-s", "formatOptions", "70", "-Z", "1000"])
                .arg(i)
                .arg("--out")
                .arg(o)
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status()
                .map(|s| s.success())
                .unwrap_or(false)
        })
    }

    /// 0700 的临时目录，丢掉时连内容一起删。
    struct TempDir(PathBuf);
    impl TempDir {
        fn new() -> Option<TempDir> {
            let n = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
            let p = std::env::temp_dir().join(format!("dct-scene-{}-{n}", std::process::id()));
            std::fs::DirBuilder::new().mode(0o700).create(&p).ok()?;
            Some(TempDir(p))
        }
    }
    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// 发给模型的图：缩小成 JPEG（上传快、不超时）；缩不成就原样 PNG，并说明原因。
    /// 返回 (字节, mime, 备注)。临时文件放在 0700 的临时目录里，用完就删。
    pub fn shrink(png: &[u8], sips: &Sips) -> (Vec<u8>, &'static str, Option<String>) {
        let orig = |note: &str| (png.to_vec(), "image/png", Some(note.to_string()));
        let Some(dir) = TempDir::new() else { return orig("没能建临时文件夹，发的是原图") };
        let (i, o) = (dir.0.join("in.png"), dir.0.join("out.jpg"));
        if std::fs::write(&i, png).is_err() {
            return orig("没能写临时文件，发的是原图");
        }
        if !sips(&i, &o) {
            return orig("sips 缩图没成，发的是原图");
        }
        match std::fs::read(&o) {
            Ok(b) if !b.is_empty() => (b, "image/jpeg", None),
            _ => orig("sips 没产出图，发的是原图"),
        }
    }

    /// 按升级链挑大脑的 Vision：当前大脑回答，记下是谁答的；每步的结果通过 `observe` 喂回链。
    pub struct ChainVision {
        brains: Vec<(String, Box<dyn Vision>)>,
        chain: std::sync::Mutex<BrainChain>,
    }

    impl ChainVision {
        pub fn new(brains: Vec<(String, Box<dyn Vision>)>) -> ChainVision {
            let chain = std::sync::Mutex::new(BrainChain::new(brains.len()));
            ChainVision { brains, chain }
        }
    }

    impl Vision for ChainVision {
        fn pick(&self, png: &[u8], history: &[(u16, u16)]) -> Result<VisionAnswer, VisionFail> {
            let i = self.chain.lock().unwrap().current();
            let (name, v) = &self.brains[i];
            match v.pick(png, history) {
                Ok(mut a) => {
                    a.brain = name.clone();
                    Ok(a)
                }
                Err(f) => {
                    self.chain.lock().unwrap().observe(false);
                    Err(f)
                }
            }
        }
        fn observe(&self, effective: bool) {
            self.chain.lock().unwrap().observe(effective);
        }
    }

    pub struct LlmVision {
        backend: Arc<dyn Backend>,
        model: String,
        timeout: Duration,
        sips: Sips,
        goal: Option<String>,
    }

    impl LlmVision {
        pub fn new(backend: Arc<dyn Backend>, model: String) -> LlmVision {
            LlmVision { backend, model, timeout: Duration::from_secs(60), sips: real_sips(), goal: None }
        }

        pub fn with_goal(mut self, g: Option<String>) -> LlmVision {
            self.goal = g;
            self
        }

        #[cfg(test)]
        pub fn with_sips(mut self, s: Sips) -> LlmVision {
            self.sips = s;
            self
        }
    }

    pub fn question(history: &[(u16, u16)], goal: Option<&str>) -> String {
        let seen: Vec<String> = history.iter().map(|(x, y)| format!("({:.2}, {:.2})", *x as f64 / 10000.0, *y as f64 / 10000.0)).collect();
        let base = format!(
            "这是一个寻物冒险手机游戏的场景截图（横屏）。我要继续玩，请选出下一步最值得点的一个场景物件\
             （门、箱子、楼梯、工具、可拾取的东西等），不要选菜单、按钮、物品栏、提示、右上的金币。\
             已经点过的位置（不要重复）：[{}]。只用 JSON 回答：{{\"name\": 物件名, \"x\": 0到1的横向位置, \"y\": 0到1的纵向位置, \"why\": 一句话}}",
            seen.join(", ")
        );
        match goal {
            None => base,
            Some(g) => format!(
                "{base}\n这一轮的目标：{g}。如果从画面上已经能看出目标达成了，就在 JSON 里加上 \"done\": true（其他字段照常写）。"
            ),
        }
    }

    impl Vision for LlmVision {
        fn pick(&self, png: &[u8], history: &[(u16, u16)]) -> Result<VisionAnswer, VisionFail> {
            let (img, mime, image_note) = shrink(png, &self.sips);
            let p = Prompt {
                system: SYSTEM.into(),
                user: question(history, self.goal.as_deref()),
                max_tokens: 300,
                image_png_base64: Some(base64::engine::general_purpose::STANDARD.encode(&img)),
                image_mime: (mime != "image/png").then(|| mime.to_string()),
            };
            let (raw, usage) = complete_counted_with_timeout(self.backend.clone(), p, self.timeout).map_err(|e| match e {
                LlmError::Timeout => VisionFail::Timeout,
                _ => VisionFail::Error,
            })?;
            Ok(VisionAnswer { brain: String::new(), pick: parse_pick(&raw), raw, model: self.model.clone(), tokens: usage.map(|u| (u.input, u.output)), image_bytes: Some(img.len()), image_note })
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
            SceneStop::GoalReached => (format!("目标达成了，一共走了 {steps} 步。"), 0),
            SceneStop::DryRun => ("试走完成，没有真点。".into(), 0),
            SceneStop::PrivateScreen => ("这个画面看起来是私人内容，我没有读它，也没有操作。请切回游戏。".into(), 0),
            SceneStop::NoTapAt => ("这台章鱼还不会按位置点，等它更新后再试。".into(), 0),
            SceneStop::NoVision(VisionFail::Timeout) => ("大模型等太久没回应（图传不上去或模型太慢），先停下了。".into(), 0),
            SceneStop::NoVision(VisionFail::Error) => ("大模型那边出错了，先停下了。可以运行 dct llm check 看原因。".into(), 0),
            SceneStop::NoVision(VisionFail::Silent) => ("大模型没回应，先停下了。".into(), 0),
            SceneStop::Skipped3 => ("连着 3 次没点（落点不安全或没看清），先停下，请你看看画面。".into(), 0),
            SceneStop::Noop5 => ("连着 5 次点了画面都没变化，先停下。".into(), 0),
            SceneStop::Dco(e) => (super::super::text::dco_error(e), 1),
        }
    }

    /// 一句话：大模型一共花了多少秒、走了多少步。
    pub fn time_line(model_ms: u64, steps: usize) -> String {
        format!("大模型一共用了 {:.1} 秒，走了 {steps} 步。", model_ms as f64 / 1000.0)
    }

    /// 整条命令（除了装配）。`dco`、`vision`、`clock` 都是传进来的，测试里换成假的。
    pub fn run_core(a: &Args, loaded: &Loaded, dco: &mut dyn Dco, clock: &mut dyn Clock, vision: &dyn Vision, rec: &Recorder) -> i32 {
        let o = SceneOptions { max_steps: a.steps, dry_run: a.dry_run, no_tap: loaded.no_tap.clone(), game: a.game.clone(), goal: a.goal.clone() };
        if let Some(n) = super::privacy_notice(&a.brain) {
            println!("{n}");
        }
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
        if s.asks > 0 {
            println!("{}", time_line(s.model_ms, s.steps));
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
        assert_eq!(p(&[]).unwrap(), Args { game: "candy-crush".into(), steps: 10, dry_run: false, brain: vec!["config".into()], goal: None });
        assert_eq!(p(&["--game", "x-1", "--steps", "100", "--dry-run"]).unwrap(), Args { game: "x-1".into(), steps: 100, dry_run: true, brain: vec!["config".into()], goal: None });
    }

    #[test]
    fn parse_brain_lists_and_unknown_names() {
        assert_eq!(p(&["--brain", "config,claude"]).unwrap().brain, ["config", "claude"]);
        assert_eq!(p(&["--brain", "claude"]).unwrap().brain, ["claude"]);
        let e = p(&["--brain", "config,gpt"]).unwrap_err();
        assert!(e.contains("不认识的大脑「gpt」") && e.contains("config,claude"), "{e}");
        assert!(p(&["--brain", ""]).is_err() && p(&["--brain"]).is_err());
    }

    #[test]
    fn the_privacy_notice_appears_exactly_when_claude_is_in_the_chain() {
        let v = |s: &[&str]| s.iter().map(|x| x.to_string()).collect::<Vec<_>>();
        assert!(privacy_notice(&v(&["claude"])).unwrap().contains("发给 Claude"));
        assert!(privacy_notice(&v(&["config", "claude"])).is_some());
        assert!(privacy_notice(&v(&["config"])).is_none());
    }

    #[test]
    fn parse_goal_takes_text_and_refuses_empty_or_missing() {
        assert_eq!(p(&["--goal", " 找到蜂蜜 "]).unwrap().goal.as_deref(), Some("找到蜂蜜"));
        assert_eq!(p(&[]).unwrap().goal, None);
        for bad in [&["--goal"][..], &["--goal", ""], &["--goal", "  "]] {
            let e = p(bad).unwrap_err();
            assert!(e.contains("--goal 后面要写"), "{e}");
        }
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
    use dct_game::scene::{SceneStop, Vision, VisionAnswer, VisionFail};
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
        (LlmVision::new(b.clone(), "qwen-x".into()).with_sips(Arc::new(|_, _| false)), b)
    }

    /// 1x1 的真 PNG。
    const TINY_PNG: &[u8] = &[
        0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0, 0, 0, 0x0d, 0x49, 0x48, 0x44, 0x52, 0, 0, 0, 1, 0, 0, 0, 1, 8, 6, 0, 0, 0, 0x1f, 0x15, 0xc4, 0x89, 0, 0, 0, 0x0d, 0x49, 0x44, 0x41, 0x54, 0x78,
        0x9c, 0x63, 0xf8, 0xff, 0xff, 0x3f, 0, 5, 0xfe, 2, 0xfe, 0xa7, 0x35, 0x81, 0x84, 0, 0, 0, 0, 0x49, 0x45, 0x4e, 0x44, 0xae, 0x42, 0x60, 0x82,
    ];

    #[test]
    fn a_successful_shrink_sends_the_jpeg_not_the_original_and_cleans_up() {
        let ok = r#"{"name":"a","x":0.1,"y":0.1,"why":""}"#;
        let seen_dir = Arc::new(Mutex::new(None::<std::path::PathBuf>));
        let sd = seen_dir.clone();
        let sips: super::unix::Sips = Arc::new(move |i, o| {
            assert_eq!(std::fs::read(i).unwrap(), TINY_PNG, "sips 拿到的是原图");
            let dir = i.parent().unwrap();
            assert_eq!(std::fs::metadata(dir).unwrap().permissions().mode() & 0o777, 0o700);
            *sd.lock().unwrap() = Some(dir.to_path_buf());
            std::fs::write(o, b"JPG!").unwrap();
            true
        });
        let (v, b) = vision(Ok((ok.into(), None)));
        let a = v.with_sips(sips).pick(TINY_PNG, &[]).unwrap();
        let sent = b.1.lock().unwrap();
        assert_eq!(sent[0].image_mime.as_deref(), Some("image/jpeg"));
        assert_eq!(sent[0].image_png_base64.as_deref(), Some("SlBHIQ=="));
        assert_eq!((a.image_bytes, a.image_note), (Some(4), None));
        assert!(!seen_dir.lock().unwrap().as_ref().unwrap().exists(), "临时目录要删掉");
    }

    #[test]
    fn a_failed_shrink_sends_the_original_png_and_says_so() {
        let (v, b) = vision(Ok((r#"{"name":"a","x":0.1,"y":0.1,"why":""}"#.into(), None)));
        let a = v.pick(TINY_PNG, &[]).unwrap();
        assert!(b.1.lock().unwrap()[0].image_mime.is_none());
        assert_eq!(a.image_bytes, Some(TINY_PNG.len()));
        assert!(a.image_note.unwrap().contains("原图"));
    }

    #[test]
    fn a_sips_that_claims_success_but_writes_nothing_falls_back() {
        let (v, _) = vision(Ok(("x".into(), None)));
        let a = v.with_sips(Arc::new(|_, _| true)).pick(TINY_PNG, &[]).unwrap();
        assert_eq!(a.image_bytes, Some(TINY_PNG.len()));
        assert!(a.image_note.is_some());
    }

    #[test]
    fn model_failures_say_timeout_or_error() {
        let (v, _) = vision(Err(LlmError::Timeout));
        assert_eq!(v.pick(&[1], &[]).err(), Some(VisionFail::Timeout));
        let (v, _) = vision(Err(LlmError::Unavailable));
        assert_eq!(v.pick(&[1], &[]).err(), Some(VisionFail::Error));
        assert!(stop_line(&SceneStop::NoVision(VisionFail::Timeout), 0).0.contains("等太久"));
        assert!(stop_line(&SceneStop::NoVision(VisionFail::Error), 0).0.contains("出错"));
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
        assert!(v.pick(&[1], &[]).is_err());
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
        fn tap_at(&mut self, _: &Profile, _: u16, _: u16, _: &[Region], _: Option<&dct_game::play::TapSettleReq>) -> Result<TapAt, DcoError> {
            self.taps += 1;
            Ok(TapAt { kind: "no_text".into(), text: String::new(), settle: None })
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
        fn pick(&self, _: &[u8], _: &[(u16, u16)]) -> Result<VisionAnswer, VisionFail> {
            let pick = dct_game::scene::parse_pick(r#"{"name":"木箱","x":0.5,"y":0.5,"why":"w"}"#);
            Ok(VisionAnswer { brain: String::new(), pick, raw: String::new(), model: "m".into(), tokens: Some((5, 1)), image_bytes: None, image_note: None })
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
        let a = Args { game: "candy-crush".into(), steps: 5, dry_run: true, brain: vec!["config".into()], goal: None };
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
        let a = Args { game: "candy-crush".into(), steps: 5, dry_run: false, brain: vec!["config".into()], goal: None };
        assert_eq!(run_core(&a, &loaded, &mut d, &mut Clk, &One, &rec), 0);
        assert_eq!((d.captures, d.taps), (0, 0));
        assert!(std::fs::read_to_string(rec.dir().join("steps.jsonl")).unwrap().is_empty());
        assert!(!rec.dir().join("png/0001.png").exists());
    }

    struct Counting(Arc<Mutex<usize>>);
    impl Vision for Counting {
        fn pick(&self, _: &[u8], _: &[(u16, u16)]) -> Result<VisionAnswer, VisionFail> {
            *self.0.lock().unwrap() += 1;
            One.pick(&[], &[])
        }
    }

    #[test]
    fn the_chain_vision_is_never_called_when_reading_the_screen_fails() {
        let (_h, loaded, rec) = setup();
        let calls = Arc::new(Mutex::new(0));
        let chain = ChainVision::new(vec![("claude".into(), Box::new(Counting(calls.clone())))]);
        let mut d = Dc { private: true, captures: 0, taps: 0 };
        let a = Args { game: "candy-crush".into(), steps: 5, dry_run: false, brain: vec!["claude".into()], goal: None };
        assert_eq!(run_core(&a, &loaded, &mut d, &mut Clk, &chain, &rec), 0);
        assert_eq!((*calls.lock().unwrap(), d.captures), (0, 0));
    }

    #[test]
    fn the_chain_vision_escalates_after_two_dead_steps_and_names_the_brain_in_the_record() {
        let (_h, loaded, rec) = setup();
        let (c1, c2) = (Arc::new(Mutex::new(0)), Arc::new(Mutex::new(0)));
        let chain = ChainVision::new(vec![("config".into(), Box::new(Counting(c1.clone()))), ("claude".into(), Box::new(Counting(c2.clone())))]);
        // Dc 点了但文字永远没变 → 每步都是 noop → 第 3 步起该换 claude。
        let mut d = Dc { private: false, captures: 0, taps: 0 };
        let a = Args { game: "candy-crush".into(), steps: 4, dry_run: false, brain: vec!["config".into(), "claude".into()], goal: None };
        run_core(&a, &loaded, &mut d, &mut Clk, &chain, &rec);
        let brains: Vec<String> = std::fs::read_to_string(rec.dir().join("steps.jsonl")).unwrap().lines().map(|l| serde_json::from_str::<serde_json::Value>(l).unwrap()["brain"].as_str().unwrap().to_string()).collect();
        assert_eq!(&brains[..3], ["config", "config", "claude"], "{brains:?}");
        assert!(*c1.lock().unwrap() == 2 && *c2.lock().unwrap() >= 1);
    }

    #[test]
    fn the_prompt_names_the_goal_only_when_given_and_is_unchanged_without_it() {
        use super::unix::question;
        let plain = question(&[(1800, 6000)], None);
        assert!(!plain.contains("目标") && !plain.contains("done"), "{plain}");
        assert!(plain.ends_with("\"why\": 一句话}"), "{plain}");
        let with = question(&[(1800, 6000)], Some("找到蜂蜜"));
        assert!(with.starts_with(&plain) && with.contains("目标：找到蜂蜜") && with.contains("\"done\": true"), "{with}");
        let (v, b) = vision(Ok((r#"{"name":"a","x":0.1,"y":0.1}"#.into(), None)));
        v.with_goal(Some("找到蜂蜜".into())).pick(&[1], &[]).unwrap();
        assert!(b.1.lock().unwrap()[0].user.contains("找到蜂蜜"));
    }

    #[test]
    fn a_goal_run_writes_goal_in_every_record_and_the_time_line_says_seconds() {
        let (_h, loaded, rec) = setup();
        let mut d = Dc { private: false, captures: 0, taps: 0 };
        let a = Args { game: "candy-crush".into(), steps: 5, dry_run: true, brain: vec!["config".into()], goal: Some("找到蜂蜜".into()) };
        assert_eq!(run_core(&a, &loaded, &mut d, &mut Clk, &One, &rec), 0);
        let line = std::fs::read_to_string(rec.dir().join("steps.jsonl")).unwrap();
        let v: serde_json::Value = serde_json::from_str(line.lines().next().unwrap()).unwrap();
        assert_eq!(v["goal"], "找到蜂蜜");
        let t = super::unix::time_line(12_340, 3);
        assert!(t.contains("12.3 秒") && t.contains("3 步"), "{t}");
        assert_eq!(stop_line(&SceneStop::GoalReached, 4).1, 0);
        assert!(stop_line(&SceneStop::GoalReached, 4).0.contains("目标达成"));
    }

    #[test]
    fn stop_lines_are_plain_and_only_dco_errors_exit_nonzero() {
        for s in [SceneStop::StepsDone, SceneStop::GoalReached, SceneStop::DryRun, SceneStop::PrivateScreen, SceneStop::NoTapAt, SceneStop::NoVision(VisionFail::Silent), SceneStop::NoVision(VisionFail::Timeout), SceneStop::NoVision(VisionFail::Error), SceneStop::Skipped3, SceneStop::Noop5] {
            assert_eq!(stop_line(&s, 3).1, 0, "{s:?}");
        }
        assert!(stop_line(&SceneStop::NoTapAt, 0).0.contains("还不会按位置点"));
        assert_eq!(stop_line(&SceneStop::Dco(DcoError { code: "halted".into(), message: "x".into() }), 0).1, 1);
    }
}
