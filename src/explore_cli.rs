//! `dct do "<目标>" --platform x --campaign <名字> [--window <id>] [--preview] [--json]`
//!
//! 有正式流程 → 重放；没有 → 摸索 → 写草稿 → 确认卡片 → 批准。
mod io;
mod real;

pub use io::{Say, Talk};

use crate::judge::{ChoiceQuestion, JudgeBackend};
use crate::learn::{apply_confirmation, render_card, write_drafts, Card, Fixer, Verdict};
use crate::procedures::Dcv;
use dct_explore::draft::{draft, DraftError};
use dct_explore::explore::{explore, explore_from, Clock, Confirm, ExploreInput, Outcome};
use dct_explore::gate::{Budget, Halt};
use dct_explore::hands::{Hands, Params, WindowRow};
use dct_explore::ladder::{Jeff, JeffAnswer, Ladder, Model, User, UserPick};
use dct_explore::options::Opt;
use dct_explore::platform::Platform;
use dct_explore::replay::{replay, ReplayInput, ReplayOutcome};
use dct_explore::trace::{redact, Trace};
use serde_json::{json, Value};
use std::cell::RefCell;
use std::path::PathBuf;
use std::sync::Arc;

const USAGE: &str = "用法：dct do \"<目标>\" --platform <平台> --campaign <活动名> [--window <窗口号>] [--preview] [--json]";

/// 参数名是这些的，一律当文件参数（值是不是存在的文件都一样）。
const FILE_PARAM_NAMES: [&str; 2] = ["video", "image"];

#[derive(Debug, Clone, PartialEq)]
pub struct DoArgs {
    pub goal: String,
    pub platform: String,
    pub campaign: String,
    pub window: Option<u64>,
    pub preview: bool,
    pub json: bool,
}

pub fn parse(args: &[String]) -> Result<DoArgs, String> {
    let (mut goal, mut platform, mut campaign, mut window) = (None, None, None, None);
    let (mut preview, mut json) = (false, false);
    let mut it = args.iter();
    while let Some(a) = it.next() {
        let mut val = |flag: &str| {
            it.next()
                .cloned()
                .ok_or_else(|| format!("{flag} 后面缺值\n{USAGE}"))
        };
        match a.as_str() {
            "--platform" => platform = Some(val("--platform")?),
            "--campaign" => campaign = Some(val("--campaign")?),
            "--window" => {
                let v = val("--window")?;
                window = Some(
                    v.parse::<u64>()
                        .map_err(|_| format!("--window 要写窗口号（数字），这里是 `{v}`"))?,
                );
            }
            "--preview" => preview = true,
            "--json" => json = true,
            f if f.starts_with("--") => return Err(format!("不认识的选项 {f}\n{USAGE}")),
            g if goal.is_none() => goal = Some(g.to_string()),
            _ => return Err(format!("目标只能写一个（要带空格就加引号）\n{USAGE}")),
        }
    }
    let goal = goal
        .filter(|g| !g.trim().is_empty())
        .ok_or_else(|| format!("缺目标\n{USAGE}"))?;
    let platform = platform.ok_or_else(|| format!("缺 --platform\n{USAGE}"))?;
    let campaign = campaign.ok_or_else(|| format!("缺 --campaign\n{USAGE}"))?;
    Ok(DoArgs {
        goal,
        platform,
        campaign,
        window,
        preview,
        json,
    })
}

#[derive(Debug)]
pub enum WindowPick {
    One(u64),
    Choose(Vec<WindowRow>),
    None,
}

/// 只按 `window_id` 认窗口，不猜：给了 `explicit` 就必须在清单里、而且是平台的浏览器；
/// 没给时浏览器窗口正好一个才用它，多个 → `Choose`，零个 → `None`。
pub fn pick_window(rows: &[WindowRow], browser: &str, explicit: Option<u64>) -> WindowPick {
    let browsers: Vec<&WindowRow> = rows.iter().filter(|r| r.app == browser).collect();
    match explicit {
        Some(id) if browsers.iter().any(|r| r.window_id == id) => WindowPick::One(id),
        Some(_) => WindowPick::None,
        None => match browsers.as_slice() {
            [one] => WindowPick::One(one.window_id),
            [] => WindowPick::None,
            many => WindowPick::Choose(many.iter().map(|r| (*r).clone()).collect()),
        },
    }
}

/// (文字参数, 文件参数)：值是存在的文件、或参数名是 video / image 的 → 文件参数。
pub fn split_params(params: &Params) -> (Vec<String>, Vec<String>) {
    let (mut text, mut files) = (Vec::new(), Vec::new());
    for (k, v) in params {
        if FILE_PARAM_NAMES.contains(&k.as_str()) || std::path::Path::new(v).is_file() {
            files.push(k.clone());
        } else {
            text.push(k.clone());
        }
    }
    (text, files)
}

impl From<dct_explore::JevChoice> for ChoiceQuestion {
    fn from(q: dct_explore::JevChoice) -> Self {
        ChoiceQuestion {
            instructions: q.instructions,
            criteria: q.criteria,
        }
    }
}

/// `[judge]` 配了：Jeff 走 judge 后端；任何 `JudgeError` 一律当「Jeff 没答」。
pub struct JudgeJeff(pub Arc<dyn JudgeBackend>);
impl Jeff for JudgeJeff {
    fn ask(&self, state: &str, q: &dct_explore::JevChoice) -> Option<JeffAnswer> {
        let a = self.0.choice(state, &q.clone().into()).ok()?;
        Some(JeffAnswer {
            key: a.choice,
            probabilities: a.probabilities,
            answer_confidence: a.answer_confidence,
        })
    }
}

/// 没配 `[judge]`：跳过 Jeff。
pub struct NoJeff;
impl Jeff for NoJeff {
    fn ask(&self, _: &str, _: &dct_explore::JevChoice) -> Option<JeffAnswer> {
        None
    }
}

/// 没配 `[llm]`：dcv 拒收草稿时不改，第 1 轮就交给用户看。
pub struct NoFix;
impl Fixer for NoFix {
    fn fix(&self, _: &str, _: &str) -> Option<String> {
        None
    }
}

/// `dct do` 里接起来的一切：测试换成假的。
pub struct Env<'a> {
    pub hands: &'a mut dyn Hands,
    pub dcv: &'a dyn Dcv,
    pub jeff: &'a dyn Jeff,
    pub model: &'a dyn Model,
    pub fixer: &'a dyn Fixer,
    pub threshold: f64,
    pub approve_proc: &'a mut dyn FnMut(&str) -> anyhow::Result<()>,
    pub clock: &'a dyn Clock,
    /// ~/.dct/explore
    pub data_dir: PathBuf,
    pub run_id: String,
    pub say: &'a RefCell<dyn Say + 'a>,
}

/// 活动里给这个平台的那份文字。
pub fn params_for(c: &crate::campaign::Campaign, platform: &str) -> Result<Params, String> {
    c.per_platform
        .get(platform)
        .cloned()
        .ok_or_else(|| format!("活动 {} 里没有给 {platform} 的文字", c.name))
}

/// 摸索循环问用户 → `Say::pick`。
struct AsUser<'s, 'a>(&'s RefCell<dyn Say + 'a>);
impl User for AsUser<'_, '_> {
    fn pick(&mut self, _: &str, all: &[Opt], pic: Option<&str>, why: &str) -> Option<UserPick> {
        self.0.borrow_mut().pick(all, pic, why)
    }
}

/// 最终发布前问用户 → `Say::yes`。
struct AsConfirm<'s, 'a>(&'s RefCell<dyn Say + 'a>);
impl Confirm for AsConfirm<'_, '_> {
    fn publish_ok(&mut self, pic: Option<&str>, summary: &str) -> bool {
        self.0
            .borrow_mut()
            .yes("confirm_publish", summary, json!({}), pic)
    }
}

/// 最后一句话（JSON 带上退出码），返回退出码。
fn fin(say: &RefCell<dyn Say + '_>, code: i32, event: &str, text: &str, extra: Value) -> i32 {
    let mut extra = if extra.is_object() { extra } else { json!({}) };
    extra["exit"] = json!(code);
    say.borrow_mut().tell(event, text, extra);
    code
}

fn publish_words(p: &Platform) -> String {
    p.publish_words.join("」或「")
}

fn by_user_note(p: &Platform) -> String {
    format!(
        "{} 的最后发布一下要你自己点：dct 只做到发布前，请在窗口里点「{}」。",
        p.name,
        publish_words(p)
    )
}

/// 发布要用户自己点的平台：卡片上多说一句（render_card 看不到平台）。
pub fn with_platform_notes(card: Card, p: &Platform) -> Card {
    if !p.publish_by_user_only {
        return card;
    }
    let note = format!("⚠ {}以后重放也只停在发布前。", by_user_note(p));
    let text = match card.text.rsplit_once('\n') {
        Some((head, ask)) => format!("{head}\n{note}\n{ask}"),
        None => format!("{note}\n{}", card.text),
    };
    let mut json = card.json;
    json["publish_by_user_only"] = json!(true);
    json["publish_note"] = json!(note);
    Card { text, json }
}

/// 窗口清单里的标题给用户看：参数值换回 `{名字}`。
fn window_list(rows: &[WindowRow], params: &Params) -> (String, Value) {
    let mut t = String::new();
    let mut v = Vec::new();
    for r in rows {
        let title = redact(&r.title, params);
        t += &format!("  {}  {}\n", r.window_id, title);
        v.push(json!({"window_id": r.window_id, "title": title}));
    }
    (t, json!({ "windows": v }))
}

/// 选窗口之后的一切。返回退出码。
pub fn drive(a: &DoArgs, p: &Platform, params: Params, env: Env) -> i32 {
    let Env {
        hands,
        dcv,
        jeff,
        model,
        fixer,
        threshold,
        approve_proc,
        clock,
        data_dir,
        run_id,
        say,
    } = env;
    // 1. 窗口：只按 window_id，不猜。
    let rows = match hands.list_windows() {
        Ok(r) => r,
        Err(e) => return fin(say, 1, "error", &dco_text(&e), json!({})),
    };
    let window_id = match pick_window(&rows, &p.browser, a.window) {
        WindowPick::One(id) => id,
        WindowPick::Choose(v) => {
            let (list, extra) = window_list(&v, &params);
            let text = format!(
                "开着好几个 {} 窗口，用 --window <id> 指定是哪一个：\n{}",
                p.browser,
                list.trim_end()
            );
            return fin(say, 2, "choose_window", &text, extra);
        }
        WindowPick::None => {
            let text = match a.window {
                Some(id) => format!(
                    "窗口 {id} 不是开着的 {} 窗口（dct do 只认 dco list_windows 里的窗口号）",
                    p.browser
                ),
                None => format!("先打开 {}，再跑一次", p.browser),
            };
            return fin(say, 2, "error", &text, json!({}));
        }
    };
    // 2. 参数分类、dcv 里的弹窗规则（dcv 不在就别开始动屏幕）。
    let (text_params, file_params) = split_params(&params);
    let popups = match dcv.popups(&p.name) {
        Ok(v) => v,
        Err(e) => return fin(say, 1, "error", &format!("dcv 出错：{e}"), json!({})),
    };
    let kind = if file_params.is_empty() {
        "post"
    } else {
        "video"
    };
    let full = format!("{}:promote-{}-{kind}", p.vault, p.name);
    let approved = dcv
        .show(&full)
        .ok()
        .filter(|v| v["executable"].as_bool() == Some(true));

    let mut user = AsUser(say);
    let mut confirm = AsConfirm(say);
    let mut lad = Ladder {
        jeff,
        model,
        user: &mut user,
        threshold,
    };
    let input = ExploreInput {
        platform: p,
        goal_text: &a.goal,
        params: params.clone(),
        text_params,
        file_params: file_params.clone(),
        window_id,
        popups: popups.clone(),
        budget: Budget::default(),
        run_id,
        data_dir: data_dir.clone(),
    };
    // 3. 有正式流程 → 重放；走不通 → 从卡住那步接着摸索。没有 → 摸索。
    let outcome = match &approved {
        Some(procedure) => {
            let r = replay(
                hands,
                &mut lad,
                ReplayInput {
                    procedure,
                    params: params.clone(),
                    popups,
                    window_id,
                    preview: a.preview,
                    platform: p,
                },
            );
            match r {
                ReplayOutcome::NeedsRepair { at_step, prefix } => {
                    say.borrow_mut().tell(
                        "note",
                        &format!("{full} 第 {at_step} 步走不通了，从这里接着摸索，补出来的写成新草稿（原流程不动）"),
                        json!({"at_step": at_step}),
                    );
                    explore_from(hands, &mut lad, &mut confirm, clock, input, prefix)
                }
                other => return replayed(say, p, &full, other),
            }
        }
        None => explore(hands, &mut lad, &mut confirm, clock, input),
    };
    let trace = match &outcome {
        Outcome::Published(t)
        | Outcome::DeclinedAtPublish(t)
        | Outcome::Halted { trace: t, .. }
        | Outcome::Stopped { trace: t, .. } => t,
    };
    if let Err(e) = dct_explore::trace::save(&data_dir.join("runs"), trace) {
        say.borrow_mut()
            .tell("note", &format!("轨迹没存下：{e}"), json!({}));
    }
    match outcome {
        Outcome::Published(trace) => {
            let mut learn = Learn {
                dcv,
                fixer,
                approve_proc,
                say,
            };
            learn.run(hands, p, &trace, &file_params)
        }
        Outcome::DeclinedAtPublish(_) => fin(
            say,
            3,
            "declined",
            "你没让发，停在了发布前；这次不写草稿",
            json!({}),
        ),
        Outcome::Halted { why, .. } => halted(say, p, &why),
        Outcome::Stopped { why, .. } => fin(say, 3, "stopped", &why, json!({})),
    }
}

/// 停在登录 / 验证码 / 警告画面。画面上的字不发给任何模型，这里也不回显。
fn halted(say: &RefCell<dyn Say + '_>, p: &Platform, why: &Halt) -> i32 {
    let (reason, text) = match why {
        Halt::Login(_) if p.login_qr => (
            "login",
            format!(
                "{} 要先登录：请用手机扫窗口里的二维码登录，登录好再跑一次",
                p.name
            ),
        ),
        Halt::Login(_) => (
            "login",
            format!("{} 要先登录：请你在窗口里登录好，再跑一次", p.name),
        ),
        Halt::Captcha(_) => (
            "captcha",
            "画面要验证码 / 人机验证，交给你：在窗口里过了验证再跑一次".to_string(),
        ),
        Halt::Warning(_) => (
            "warning",
            "画面上出了账号警告，停下交给你；先去看看是怎么回事".to_string(),
        ),
    };
    fin(
        say,
        3,
        "stopped",
        &text,
        json!({"reason": reason, "login_qr": p.login_qr}),
    )
}

/// 重放的结果（走不通的已经交给摸索了）。
fn replayed(say: &RefCell<dyn Say + '_>, p: &Platform, full: &str, r: ReplayOutcome) -> i32 {
    match r {
        ReplayOutcome::Done { learned } => {
            let mut text = format!("照 {full} 发好了");
            if !learned.is_empty() {
                text += &format!(
                    "；dco 这次新认了 {} 个按钮，还没确认（不确认就不会用来重放）",
                    learned.len()
                );
            }
            fin(say, 0, "done", &text, json!({"learned": learned.len()}))
        }
        ReplayOutcome::Previewed { at_step } if p.publish_by_user_only => fin(
            say,
            0,
            "previewed",
            &format!(
                "照 {full} 填好了，停在第 {at_step} 步发布前。{}",
                by_user_note(p)
            ),
            json!({"at_step": at_step, "publish_by_user_only": true}),
        ),
        ReplayOutcome::Previewed { at_step } => fin(
            say,
            0,
            "previewed",
            &format!("试跑停在第 {at_step} 步发布前，没有发"),
            json!({"at_step": at_step}),
        ),
        ReplayOutcome::Blocked { note } => fin(
            say,
            3,
            "stopped",
            &format!("dco 拦下了：{note}；这一步交给你"),
            json!({}),
        ),
        ReplayOutcome::Stopped(why) if why.starts_with("dco 出错") => {
            fin(say, 1, "error", &why, json!({}))
        }
        ReplayOutcome::Stopped(why) => fin(say, 3, "stopped", &why, json!({})),
        // drive 里已经接住，到不了这里。
        ReplayOutcome::NeedsRepair { at_step, .. } => fin(
            say,
            1,
            "error",
            &format!("第 {at_step} 步走不通"),
            json!({}),
        ),
    }
}

/// 发出去以后：草稿 → dcv → 确认卡片 → 批准。
struct Learn<'e, 'a> {
    dcv: &'e dyn Dcv,
    fixer: &'e dyn Fixer,
    approve_proc: &'e mut dyn FnMut(&str) -> anyhow::Result<()>,
    say: &'e RefCell<dyn Say + 'a>,
}

fn draft_error_text(e: &DraftError) -> String {
    match e {
        DraftError::LoginGateNotSeen(g) if g.is_empty() => {
            "平台表里还没有登录闸文字（login_gate），先在 ~/.dct/platforms.toml 补上".into()
        }
        DraftError::LoginGateNotSeen(g) => format!("第一屏上没看到登录闸文字「{g}」"),
        DraftError::NoPublish => "轨迹里没有最终发布那一下".into(),
        DraftError::Empty => "轨迹里没有成功的步骤".into(),
        DraftError::BadStepArg(m) => format!("有一步写不成流程：{m}"),
    }
}

impl Learn<'_, '_> {
    fn run(&mut self, hands: &mut dyn Hands, p: &Platform, trace: &Trace, files: &[String]) -> i32 {
        let say = self.say;
        let dismiss: Vec<String> = self
            .dcv
            .vocab()
            .ok()
            .and_then(|v| v["popup_dismiss"].as_array().cloned())
            .unwrap_or_default()
            .iter()
            .filter_map(|w| w.as_str().map(str::to_string))
            .collect();
        let (pd, pops) = match draft(trace, p, &dismiss, files) {
            Ok(x) => x,
            Err(e) => {
                let text = format!("发出去了，但这次没能写成流程草稿：{}", draft_error_text(&e));
                return fin(say, 0, "done", &text, json!({"draft": false}));
            }
        };
        let w = match write_drafts(self.dcv, self.fixer, &pd, &pops) {
            Ok(w) => w,
            Err((_, reason)) => {
                let text = format!("发出去了，但 dcv 没收下草稿：{reason}");
                return fin(say, 1, "error", &text, json!({}));
            }
        };
        let mut learned: Vec<String> = Vec::new();
        for k in trace
            .steps
            .iter()
            .filter(|s| s.ok)
            .flat_map(|s| &s.learned_keys)
        {
            if !learned.contains(k) {
                learned.push(k.clone());
            }
        }
        let card = with_platform_notes(render_card(&w, &pd, &pops, &learned), p);
        let ok = say.borrow_mut().yes("card", &card.text, card.json, None);
        let v = if ok { Verdict::Yes } else { Verdict::No };
        match apply_confirmation(v, self.dcv, &mut *self.approve_proc, hands, &w, &learned) {
            Ok(()) if ok => fin(
                say,
                0,
                "done",
                &format!("已确认 {}，以后照这个发", w.procedure),
                json!({"procedure": w.procedure}),
            ),
            Ok(()) => fin(
                say,
                3,
                "declined",
                &format!("没确认；草稿 {} 留在 dcv 里，没有生效", w.procedure),
                json!({"procedure": w.procedure}),
            ),
            Err(e) if touch_id_cancelled(&e) => fin(
                say,
                3,
                "declined",
                &format!("没有按指纹确认；草稿 {} 留在 dcv 里，没有生效", w.procedure),
                json!({"procedure": w.procedure}),
            ),
            Err(e) => fin(
                say,
                1,
                "error",
                &format!(
                    "确认没成功：{e}；草稿 {} 留在 dcv 里，没有生效",
                    w.procedure
                ),
                json!({"procedure": w.procedure}),
            ),
        }
    }
}

/// `~/.dct/config.toml`：没有这个文件 = 默认（什么都没配）；读不了、解析不了 = 错（退出码 2）。
pub fn load_config(path: &std::path::Path) -> Result<crate::config::Config, String> {
    match std::fs::read_to_string(path) {
        Ok(t) => {
            toml::from_str::<crate::config::Config>(&t).map_err(|e| config_error_text(path, &t, &e))
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Default::default()),
        Err(e) => Err(format!("读不了 {}：{e}", path.display())),
    }
}

/// 配置错的提示：只给路径、行列和说明，不带原文（toml 自己的 Display 会把出错那一行抄出来，
/// 那一行可能就是一把密钥）。说明里引号括起来的值（serde 的「invalid type: string "…"」）也换掉。
fn config_error_text(path: &std::path::Path, text: &str, e: &toml::de::Error) -> String {
    let at = e
        .span()
        .map(|sp| {
            let before = text.get(..sp.start).unwrap_or(text);
            let line = before.matches('\n').count() + 1;
            let col = before.rsplit('\n').next().map_or(0, |l| l.chars().count()) + 1;
            format!("第 {line} 行第 {col} 列：")
        })
        .unwrap_or_default();
    format!(
        "{} 解析不了（{at}{}）",
        path.display(),
        scrub_toml_message(e.message())
    )
}

/// toml / serde 的说明里，引号里的一律是值（换成 `"…"`）；反引号里的是键名、词表里的词或值：
/// 只留短的小写词（`cli`、`base_url`）和单个符号，其余换成 `` `…` ``。换行压成「；」。
fn scrub_toml_message(m: &str) -> String {
    let safe = |w: &str| {
        (w.chars().count() == 1 && !w.chars().all(char::is_alphanumeric))
            || (!w.is_empty()
                && w.len() <= 24
                && w.bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_'))
    };
    let mut out = String::new();
    let mut it = m.trim().chars();
    while let Some(c) = it.next() {
        match c {
            '"' | '`' => {
                let inner: String = it.by_ref().take_while(|x| *x != c).collect();
                if c == '`' && safe(&inner) {
                    out.push_str(&format!("`{inner}`"));
                } else {
                    out.push_str(&format!("{c}…{c}"));
                }
            }
            '\n' => out.push('；'),
            c if c.is_control() => {}
            c => out.push(c),
        }
    }
    out
}

/// 用户在 Touch ID 那一步取消了。`procedures::approve` 把 `SignError` 变成了同样的一句话
/// （`anyhow!("{e}")`），所以按那句话认；认不出的一律当出错（退出码 1）。
fn touch_id_cancelled(e: &anyhow::Error) -> bool {
    let cancelled = dct_brain::sign::SignError::Cancelled.to_string();
    e.chain().any(|c| c.to_string() == cancelled)
}

fn dco_text(e: &dct_dco::DcoError) -> String {
    crate::game::text::dco_error(e)
}

pub fn run(args: &[String]) -> i32 {
    let json_mode = args.iter().any(|a| a == "--json");
    // 还没连上任何东西时的错：--json 照样是一行事件（stdout），终端模式写 stderr。
    let say_early = |code: i32, text: &str| -> i32 {
        let w: Box<dyn std::io::Write> = if json_mode {
            Box::new(std::io::stdout())
        } else {
            Box::new(std::io::stderr())
        };
        Talk::new(std::io::empty(), w, json_mode, None).tell("error", text, json!({"exit": code}));
        code
    };
    let a = match parse(args) {
        Ok(a) => a,
        Err(m) => return say_early(2, &m),
    };
    real::run(&a, &say_early)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::judge::{ChoiceAnswer, JudgeError};
    use crate::procedures::AddReq;
    use dct_explore::hands::{El, Frac, Look, RunReply};
    use dct_explore::ladder::NoModel;
    use dct_explore::platform::Table;
    use dct_explore::testing::FakeHands;
    use serde_json::{json, Value};
    use std::io::Cursor;

    fn row(id: u64, app: &str, title: &str) -> WindowRow {
        WindowRow {
            window_id: id,
            app: app.into(),
            title: title.into(),
        }
    }

    #[test]
    fn parse_requires_platform_and_campaign() {
        assert!(parse(&["把这段发到 X".into()]).is_err());
        let a = parse(&[
            "把这段发到 X".into(),
            "--platform".into(),
            "x".into(),
            "--campaign".into(),
            "launch".into(),
            "--window".into(),
            "4100".into(),
            "--json".into(),
        ])
        .unwrap();
        assert_eq!(
            (a.platform.as_str(), a.window, a.json, a.preview),
            ("x", Some(4100), true, false)
        );
    }

    fn s(v: &[&str]) -> Vec<String> {
        v.iter().map(|x| x.to_string()).collect()
    }

    #[test]
    fn parse_refuses_unknown_flags_a_bad_window_and_a_missing_goal() {
        let ok = ["g", "--platform", "x", "--campaign", "c"];
        let a = parse(&s(&[&ok[..], &["--preview"]].concat())).unwrap();
        assert_eq!(
            (a.goal.as_str(), a.preview, a.json, a.window),
            ("g", true, false, None)
        );
        assert!(parse(&s(&[&ok[..], &["--force"]].concat())).is_err());
        assert!(parse(&s(&[&ok[..], &["--window", "abc"]].concat())).is_err());
        assert!(parse(&s(&[&ok[..], &["--window"]].concat())).is_err());
        assert!(parse(&s(&[&ok[..], &["second goal"]].concat())).is_err());
        assert!(parse(&s(&["--platform", "x", "--campaign", "c"])).is_err());
        assert!(parse(&s(&["  ", "--platform", "x", "--campaign", "c"])).is_err());
    }

    #[test]
    fn several_browser_windows_need_an_explicit_window() {
        let rows = vec![
            row(1, "Google Chrome", "X"),
            row(2, "Google Chrome", "YouTube Studio"),
            row(3, "Finder", ""),
        ];
        assert!(
            matches!(pick_window(&rows, "Google Chrome", None), WindowPick::Choose(v) if v.len() == 2)
        );
        assert!(matches!(
            pick_window(&rows, "Google Chrome", Some(2)),
            WindowPick::One(2)
        ));
        assert!(
            matches!(
                pick_window(&rows, "Google Chrome", Some(3)),
                WindowPick::None
            ),
            "指定的窗口不是浏览器就不用"
        );
        assert!(matches!(
            pick_window(&rows[..1], "Google Chrome", None),
            WindowPick::One(1)
        ));
        assert!(
            matches!(
                pick_window(&rows, "Google Chrome", Some(9)),
                WindowPick::None
            ),
            "不在清单里"
        );
        assert!(matches!(
            pick_window(&rows[2..], "Google Chrome", None),
            WindowPick::None
        ));
    }

    #[test]
    fn file_params_are_told_apart_from_text() {
        let d = tempfile::tempdir().unwrap();
        let f = d.path().join("a.mp4");
        std::fs::write(&f, b"x").unwrap();
        let (text, files) = split_params(
            &[
                ("post_text".into(), "hi".into()),
                ("video".into(), f.display().to_string()),
            ]
            .into_iter()
            .collect(),
        );
        assert_eq!(
            (text, files),
            (vec!["post_text".to_string()], vec!["video".to_string()])
        );
        let (text, files) = split_params(
            &[
                ("image".to_string(), "missing.png".to_string()),
                ("clip".to_string(), f.display().to_string()),
            ]
            .into_iter()
            .collect(),
        );
        assert!(text.is_empty());
        assert_eq!(files, ["clip", "image"]);
    }

    #[test]
    fn campaign_without_this_platform_is_refused() {
        let c = crate::campaign::Campaign {
            name: "launch".into(),
            approved_at: 1,
            per_platform: [(
                "x".to_string(),
                [("post_text".to_string(), "hi".to_string())]
                    .into_iter()
                    .collect(),
            )]
            .into_iter()
            .collect(),
        };
        assert_eq!(params_for(&c, "x").unwrap()["post_text"], "hi");
        let e = params_for(&c, "tiktok").unwrap_err();
        assert!(e.contains("launch") && e.contains("tiktok"), "{e}");
    }

    struct FakeJudge(
        Result<ChoiceAnswer, JudgeError>,
        std::sync::Mutex<Vec<ChoiceQuestion>>,
    );
    impl JudgeBackend for FakeJudge {
        fn choice(&self, _: &str, q: &ChoiceQuestion) -> Result<ChoiceAnswer, JudgeError> {
            self.1.lock().unwrap().push(q.clone());
            self.0.clone()
        }
    }

    #[test]
    fn jeff_goes_through_the_judge_in_order_and_any_judge_error_means_no_answer() {
        let q = dct_explore::JevChoice {
            instructions: "pick".into(),
            criteria: vec![
                ("e3".into(), Some("按钮 Post".into())),
                ("stuck".into(), None),
            ],
        };
        let cq: ChoiceQuestion = q.clone().into();
        assert_eq!(cq.instructions, "pick");
        assert_eq!(cq.criteria, q.criteria);
        let ok = Arc::new(FakeJudge(
            Ok(ChoiceAnswer {
                choice: "e3".into(),
                probabilities: vec![("e3".into(), 0.9), ("stuck".into(), 0.1)],
                answer_confidence: 0.9,
            }),
            Default::default(),
        ));
        let a = JudgeJeff(ok.clone()).ask("state", &q).unwrap();
        assert_eq!((a.key.as_str(), a.answer_confidence), ("e3", 0.9));
        assert_eq!(a.probabilities.len(), 2);
        assert_eq!(ok.1.lock().unwrap()[0].criteria[0].0, "e3");
        for e in [
            JudgeError::Busy,
            JudgeError::Timeout,
            JudgeError::Unavailable("x".into()),
            JudgeError::Malformed("x".into()),
            JudgeError::BadRequest("x".into()),
        ] {
            assert!(JudgeJeff(Arc::new(FakeJudge(Err(e), Default::default())))
                .ask("s", &q)
                .is_none());
        }
    }

    // ---- 整条命令，对着假 dco / 假 dcv ----

    #[derive(Default)]
    struct FakeDcv {
        shows: Vec<(String, Value)>,
        adds: RefCell<Vec<Vec<String>>>,
        approved: RefCell<Vec<String>>,
        no_popups: bool,
    }
    impl Dcv for FakeDcv {
        fn show(&self, n: &str) -> anyhow::Result<Value> {
            if let Some((_, v)) = self.shows.iter().find(|(k, _)| k == n) {
                return Ok(v.clone());
            }
            let added = self.adds.borrow().iter().any(|a| {
                a.windows(2)
                    .any(|w| w[0] == "--name" && n.ends_with(&format!(":{}", w[1])))
            });
            if added {
                Ok(json!({"body_sha256": "sha-of-popup"}))
            } else {
                anyhow::bail!("没有 {n}")
            }
        }
        fn approve(&self, n: &str, sha: &str) -> anyhow::Result<String> {
            self.approved.borrow_mut().push(n.into());
            Ok(sha.into())
        }
        fn add(&self, r: &AddReq) -> anyhow::Result<Value> {
            self.adds.borrow_mut().push(crate::procedures::add_args(r));
            Ok(json!({"ok": true}))
        }
        fn vocab(&self) -> anyhow::Result<Value> {
            Ok(json!({"popup_dismiss": ["Cancel", "Not now"]}))
        }
        fn popups(&self, _: &str) -> anyhow::Result<Vec<Value>> {
            if self.no_popups {
                anyhow::bail!("找不到 dcv 命令")
            }
            Ok(vec![])
        }
    }

    use super::{NoFix, NoJeff};

    struct SureJeff(Vec<&'static str>, RefCell<usize>);
    impl Jeff for SureJeff {
        fn ask(&self, _: &str, _: &dct_explore::JevChoice) -> Option<JeffAnswer> {
            let i = *self.1.borrow();
            *self.1.borrow_mut() += 1;
            self.0.get(i).map(|k| JeffAnswer {
                key: k.to_string(),
                probabilities: vec![(k.to_string(), 0.95)],
                answer_confidence: 0.95,
            })
        }
    }
    struct T0;
    impl Clock for T0 {
        fn now_ms(&self) -> u64 {
            0
        }
    }

    const TEXT: &str = "hello from dct test";

    fn el(id: &str, t: &str, role: &str, x: f64, y: f64) -> El {
        El {
            id: id.into(),
            text: t.into(),
            role: role.into(),
            frac: Frac {
                x,
                y,
                w: 0.05,
                h: 0.03,
            },
            source: "ocr".into(),
        }
    }
    fn look(hash: &str, els: Vec<El>) -> Look {
        Look {
            snapshot_id: format!("s-{hash}"),
            observation_id: Some(format!("obs-{hash}")),
            scene_hash: Some(hash.into()),
            elements: els,
            marks_png: None,
        }
    }
    fn reply(id: &str, status: &str, at: u32) -> RunReply {
        RunReply {
            run_id: id.into(),
            status: status.into(),
            at_step: at,
            note: None,
            help: None,
            learned_keys: vec![],
            raw: json!({}),
        }
    }
    fn composer(hash: &str) -> Look {
        look(
            hash,
            vec![
                el("e2", "What's happening?", "textarea", 0.45, 0.25),
                el("e5", "Drafts", "button", 0.6, 0.1),
            ],
        )
    }
    fn filled(hash: &str) -> Look {
        look(
            hash,
            vec![
                el("e1", "Post", "button", 0.08, 0.5),
                el("e2", TEXT, "textarea", 0.45, 0.25),
                el("e3", "Post", "button", 0.8, 0.35),
            ],
        )
    }
    fn sent(hash: &str) -> Look {
        look(hash, vec![el("e1", "Your post was sent", "text", 0.4, 0.9)])
    }
    fn chrome(h: &mut FakeHands) {
        h.windows = vec![row(4100, "Google Chrome", "X"), row(7, "Finder", "")];
    }
    /// 摸索 X 发一条：open_url → 填字 → 发布（先问用户）→ 做完。
    fn script_x_explore(h: &mut FakeHands) {
        chrome(h);
        h.push_step(reply("r0", "done", 1)); // open_url
        h.push_look(composer("a"));
        h.push_step(reply("r1", "done", 1)); // tap_by_intent What's happening?
        h.push_step(reply("r2", "done", 1)); // type_param post_text
        h.push_look(filled("b"));
        h.push_look(filled("b")); // 编号图（问发不发）
        h.push_step(reply("r3", "done", 1)); // tap Post
        h.push_look(sent("c"));
    }
    fn x_jeff() -> SureJeff {
        SureJeff(vec!["fill_post_text_e2", "e3", "done"], RefCell::new(0))
    }
    fn args(json: bool) -> DoArgs {
        DoArgs {
            goal: "把这段发到 X".into(),
            platform: "x".into(),
            campaign: "launch".into(),
            window: None,
            preview: false,
            json,
        }
    }
    fn params() -> Params {
        [("post_text".to_string(), TEXT.to_string())]
            .into_iter()
            .collect()
    }

    struct Ran {
        code: i32,
        out: String,
        approved: Vec<String>,
    }

    #[allow(clippy::too_many_arguments)]
    fn go(
        a: &DoArgs,
        platform: &str,
        h: &mut FakeHands,
        dcv: &FakeDcv,
        jeff: &dyn Jeff,
        input: &str,
        params: Params,
        d: &std::path::Path,
    ) -> Ran {
        go_with(a, platform, h, dcv, jeff, input, params, d, &mut |_| Ok(()))
    }

    #[allow(clippy::too_many_arguments)]
    fn go_with(
        a: &DoArgs,
        platform: &str,
        h: &mut FakeHands,
        dcv: &FakeDcv,
        jeff: &dyn Jeff,
        input: &str,
        params: Params,
        d: &std::path::Path,
        approve_result: &mut dyn FnMut(&str) -> anyhow::Result<()>,
    ) -> Ran {
        let t = Table::seed();
        let p = t.get(platform).unwrap().clone();
        let talk = RefCell::new(Talk::new(
            Cursor::new(input.as_bytes().to_vec()),
            Vec::<u8>::new(),
            a.json,
            Some(d.join("runs").join("x-20261011-0001")),
        ));
        let mut approved = Vec::new();
        let mut approve = |n: &str| {
            approved.push(n.to_string());
            approve_result(n)
        };
        let code = drive(
            a,
            &p,
            params,
            Env {
                hands: h,
                dcv,
                jeff,
                model: &NoModel,
                fixer: &NoFix,
                threshold: 0.8,
                approve_proc: &mut approve,
                clock: &T0,
                data_dir: d.to_path_buf(),
                run_id: "x-20261011-0001".into(),
                say: &talk,
            },
        );
        let out = String::from_utf8(talk.into_inner().w).unwrap();
        Ran {
            code,
            out,
            approved,
        }
    }

    #[test]
    fn several_chrome_windows_stop_before_touching_anything() {
        let d = tempfile::tempdir().unwrap();
        let mut h = FakeHands::new();
        h.windows = vec![
            row(1, "Google Chrome", "X"),
            row(2, "Google Chrome", "YouTube Studio"),
        ];
        let r = go(
            &args(false),
            "x",
            &mut h,
            &FakeDcv::default(),
            &NoJeff,
            "",
            params(),
            d.path(),
        );
        assert_eq!(r.code, 2);
        assert!(
            r.out.contains("--window") && r.out.contains("YouTube Studio"),
            "{}",
            r.out
        );
        assert_eq!(h.calls, ["list_windows"], "没选好窗口就什么都不碰");
        let mut h = FakeHands::new();
        let r = go(
            &args(false),
            "x",
            &mut h,
            &FakeDcv::default(),
            &NoJeff,
            "",
            params(),
            d.path(),
        );
        assert_eq!(r.code, 2);
        assert!(r.out.contains("先打开 Google Chrome"), "{}", r.out);
    }

    #[test]
    fn an_approved_procedure_is_replayed_not_explored() {
        let d = tempfile::tempdir().unwrap();
        let mut h = FakeHands::new();
        chrome(&mut h);
        h.push_procedure(reply("p1", "done", 4));
        let dcv = FakeDcv {
            shows: vec![(
                "social-global:promote-x-post".into(),
                json!({"name": "promote-x-post", "executable": true, "steps": [
                    {"n": 1, "action": "open_url", "arg": "https://x.com/compose/post"},
                    {"n": 2, "action": "tap_by_intent", "arg": "Post"}]}),
            )],
            ..Default::default()
        };
        let r = go(
            &args(false),
            "x",
            &mut h,
            &dcv,
            &NoJeff,
            "",
            params(),
            d.path(),
        );
        assert_eq!(r.code, 0, "{}", r.out);
        assert_eq!(h.calls, ["list_windows", "run_procedure"]);
        assert_eq!(h.procedure_windows, [4100]);
        assert_eq!(h.sent_procedures[0].1["post_text"], TEXT);
        assert!(dcv.adds.borrow().is_empty());
    }

    #[test]
    fn preview_is_passed_to_replay() {
        let d = tempfile::tempdir().unwrap();
        let mut h = FakeHands::new();
        chrome(&mut h);
        h.push_procedure(reply("p1", "stopped", 2));
        let dcv = FakeDcv {
            shows: vec![(
                "social-global:promote-x-post".into(),
                json!({"name": "promote-x-post", "executable": true, "steps": [
                    {"n": 1, "action": "open_url", "arg": "https://x.com/compose/post"},
                    {"n": 2, "action": "tap_by_intent", "arg": "Post"}]}),
            )],
            ..Default::default()
        };
        let a = DoArgs {
            preview: true,
            ..args(true)
        };
        let r = go(&a, "x", &mut h, &dcv, &NoJeff, "", params(), d.path());
        assert_eq!(r.code, 0, "{}", r.out);
        assert_eq!(h.calls[1], "run_procedure stop_before=2");
        let last: Value = serde_json::from_str(r.out.lines().last().unwrap()).unwrap();
        assert_eq!(last["event"], "previewed");
    }

    #[test]
    fn publish_by_user_only_replay_tells_the_user_to_tap_publish() {
        let d = tempfile::tempdir().unwrap();
        let mut h = FakeHands::new();
        h.windows = vec![row(4100, "Google Chrome", "公众号")];
        h.push_procedure(reply("p1", "stopped", 2));
        let dcv = FakeDcv {
            shows: vec![(
                "social-global:promote-wechat_mp-post".into(),
                json!({"name": "promote-wechat_mp-post", "executable": true, "steps": [
                    {"n": 1, "action": "open_url", "arg": "https://mp.weixin.qq.com/"},
                    {"n": 2, "action": "tap_by_intent", "arg": "发布"}]}),
            )],
            ..Default::default()
        };
        let a = DoArgs {
            platform: "wechat_mp".into(),
            ..args(false)
        };
        let r = go(
            &a,
            "wechat_mp",
            &mut h,
            &dcv,
            &NoJeff,
            "",
            params(),
            d.path(),
        );
        assert_eq!(r.code, 0, "{}", r.out);
        assert_eq!(
            h.calls[1], "run_procedure stop_before=2",
            "这种平台永远停在发布前"
        );
        assert!(r.out.contains("自己点"), "{}", r.out);
    }

    #[test]
    fn no_procedure_explores_writes_a_draft_shows_the_card_and_approves_on_yes() {
        let d = tempfile::tempdir().unwrap();
        let mut h = FakeHands::new();
        script_x_explore(&mut h);
        let dcv = FakeDcv::default();
        // 发布前问一次（y），确认卡片一次（y）
        let r = go(
            &args(false),
            "x",
            &mut h,
            &dcv,
            &x_jeff(),
            "y\ny\n",
            params(),
            d.path(),
        );
        assert_eq!(r.code, 0, "{}", r.out);
        assert_eq!(h.step_calls[0].step.action, "open_url");
        assert_eq!(h.step_calls[0].window_id, 4100);
        let adds = dcv.adds.borrow();
        assert_eq!(adds.len(), 1);
        assert!(adds[0]
            .windows(2)
            .any(|w| w == ["--name", "promote-x-post"]));
        assert!(adds[0]
            .windows(2)
            .any(|w| w == ["--source", "import:agent"]));
        assert!(adds[0]
            .windows(2)
            .any(|w| w == ["--learned-from", "run:x-20261011-0001"]));
        assert!(r.out.contains("学会了"), "确认卡片：{}", r.out);
        assert_eq!(r.approved, ["social-global:promote-x-post"]);
        assert!(!r.out.contains(TEXT), "要发的文字不回显：{}", r.out);
        assert!(
            d.path().join("runs/x-20261011-0001/trace.json").exists(),
            "轨迹存盘"
        );
    }

    #[test]
    fn saying_no_to_the_card_approves_nothing() {
        let d = tempfile::tempdir().unwrap();
        let mut h = FakeHands::new();
        script_x_explore(&mut h);
        let dcv = FakeDcv::default();
        let r = go(
            &args(false),
            "x",
            &mut h,
            &dcv,
            &x_jeff(),
            "y\nn\n",
            params(),
            d.path(),
        );
        assert_eq!(r.code, 3, "{}", r.out);
        assert!(r.approved.is_empty());
        assert!(dcv.approved.borrow().is_empty());
    }

    #[test]
    fn declining_the_publish_tap_exits_3_and_writes_no_draft() {
        let d = tempfile::tempdir().unwrap();
        let mut h = FakeHands::new();
        chrome(&mut h);
        h.push_step(reply("r0", "done", 1));
        h.push_look(filled("b"));
        h.push_look(filled("b")); // 编号图
        let dcv = FakeDcv::default();
        let jeff = SureJeff(vec!["e3"], RefCell::new(0));
        let r = go(
            &args(false),
            "x",
            &mut h,
            &dcv,
            &jeff,
            "n\n",
            params(),
            d.path(),
        );
        assert_eq!(r.code, 3, "{}", r.out);
        assert!(dcv.adds.borrow().is_empty());
        assert!(!h.calls.iter().any(|c| c == "run_step tap_by_intent: Post"));
    }

    #[test]
    fn json_mode_is_one_event_per_line_with_ask_confirm_card_and_done() {
        let d = tempfile::tempdir().unwrap();
        let mut h = FakeHands::new();
        script_x_explore(&mut h);
        let dcv = FakeDcv::default();
        let input = "{\"verdict\":\"yes\"}\n{\"verdict\":\"yes\"}\n";
        let r = go(
            &args(true),
            "x",
            &mut h,
            &dcv,
            &x_jeff(),
            input,
            params(),
            d.path(),
        );
        assert_eq!(r.code, 0, "{}", r.out);
        let ev: Vec<Value> = r
            .out
            .lines()
            .map(|l| serde_json::from_str(l).unwrap_or_else(|e| panic!("{l}: {e}")))
            .collect();
        let names: Vec<&str> = ev.iter().filter_map(|e| e["event"].as_str()).collect();
        assert!(names.contains(&"confirm_publish"), "{names:?}");
        assert!(names.contains(&"card"), "{names:?}");
        assert_eq!(names.last(), Some(&"done"));
        assert_eq!(ev.last().unwrap()["exit"], 0);
        assert!(!r.out.contains(TEXT), "{}", r.out);
    }

    #[test]
    fn json_mode_asks_the_user_when_no_one_else_can() {
        let d = tempfile::tempdir().unwrap();
        let mut h = FakeHands::new();
        chrome(&mut h);
        h.push_step(reply("r0", "done", 1));
        h.push_look(composer("a"));
        h.push_look(composer("a")); // 编号图（问用户）
        h.push_step(reply("r1", "done", 1));
        h.push_step(reply("r2", "done", 1));
        h.push_look(filled("b"));
        h.push_look(filled("b")); // 编号图（第二题）
        let dcv = FakeDcv::default();
        // 用户第一题选填字，第二题说停。
        let input = "{\"key\":\"fill_post_text_e2\"}\n{\"stop\":true}\n";
        let r = go(
            &args(true),
            "x",
            &mut h,
            &dcv,
            &NoJeff,
            input,
            params(),
            d.path(),
        );
        assert_eq!(r.code, 3, "{}", r.out);
        let ev: Vec<Value> = r
            .out
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect();
        assert_eq!(ev.iter().filter(|e| e["event"] == "ask").count(), 2);
        assert_eq!(ev.last().unwrap()["event"], "stopped");
        assert!(!r.out.contains(TEXT));
    }

    #[test]
    fn a_qr_login_screen_says_scan_with_the_phone_and_rerun() {
        let d = tempfile::tempdir().unwrap();
        let mut h = FakeHands::new();
        h.windows = vec![row(4100, "Google Chrome", "小红书")];
        h.push_step(reply("r0", "done", 1));
        h.push_look(look("a", vec![el("e1", "扫码登录", "text", 0.5, 0.5)]));
        let a = DoArgs {
            platform: "xiaohongshu".into(),
            ..args(false)
        };
        let r = go(
            &a,
            "xiaohongshu",
            &mut h,
            &FakeDcv::default(),
            &NoJeff,
            "",
            params(),
            d.path(),
        );
        assert_eq!(r.code, 3, "{}", r.out);
        assert!(r.out.contains("手机") && r.out.contains("扫"), "{}", r.out);
        assert!(r.out.contains("再跑"), "{}", r.out);
    }

    #[test]
    fn a_missing_dcv_is_an_error_before_any_screen_action() {
        let d = tempfile::tempdir().unwrap();
        let mut h = FakeHands::new();
        chrome(&mut h);
        let dcv = FakeDcv {
            no_popups: true,
            ..Default::default()
        };
        let r = go(
            &args(false),
            "x",
            &mut h,
            &dcv,
            &NoJeff,
            "",
            params(),
            d.path(),
        );
        assert_eq!(r.code, 1, "{}", r.out);
        assert_eq!(h.calls, ["list_windows"]);
    }

    #[test]
    fn a_broken_procedure_is_repaired_from_where_it_stuck_as_a_draft_of_the_original() {
        let d = tempfile::tempdir().unwrap();
        let mut h = FakeHands::new();
        chrome(&mut h);
        let mut failed = reply("p1", "failed", 2);
        failed.note = Some("找不到 Compose".into());
        h.push_procedure(failed);
        // 从当前画面接着摸索（不再 open_url）
        h.push_look(composer("a"));
        h.push_step(reply("r1", "done", 1));
        h.push_step(reply("r2", "done", 1));
        h.push_look(filled("b"));
        h.push_look(filled("b")); // 编号图
        h.push_step(reply("r3", "done", 1));
        h.push_look(sent("c"));
        let dcv = FakeDcv {
            shows: vec![(
                "social-global:promote-x-post".into(),
                json!({"name": "promote-x-post", "executable": true, "steps": [
                    {"n": 1, "action": "open_url", "arg": "https://x.com/compose/post"},
                    {"n": 2, "action": "tap_by_intent", "arg": "Compose"},
                    {"n": 3, "action": "tap_by_intent", "arg": "Post"}]}),
            )],
            ..Default::default()
        };
        let r = go(
            &args(false),
            "x",
            &mut h,
            &dcv,
            &x_jeff(),
            "y\nn\n",
            params(),
            d.path(),
        );
        assert!(
            !h.calls.iter().any(|c| c.starts_with("run_step open_url")),
            "{:?}",
            h.calls
        );
        let adds = dcv.adds.borrow();
        assert_eq!(adds.len(), 1, "{}", r.out);
        assert!(
            adds[0]
                .windows(2)
                .any(|w| w == ["--draft-of", "promote-x-post"]),
            "{:?}",
            adds[0]
        );
        assert_eq!(r.code, 3, "卡片说不：{}", r.out);
    }

    #[test]
    fn the_card_says_who_taps_publish_on_publish_by_user_only_platforms() {
        let card = crate::learn::Card {
            text: "学会了：x\n  1. 点「发布」\n确认后以后就照这个自动发（需要按一下指纹）。确认吗？[y/N]".into(),
            json: json!({"procedure": "v:p"}),
        };
        let t = Table::seed();
        let plain = with_platform_notes(
            crate::learn::Card {
                text: card.text.clone(),
                json: card.json.clone(),
            },
            t.get("x").unwrap(),
        );
        assert_eq!(plain.text, card.text);
        let c = with_platform_notes(card, t.get("wechat_mp").unwrap());
        let lines: Vec<&str> = c.text.lines().collect();
        assert!(lines[lines.len() - 2].contains("自己点"), "{}", c.text);
        assert!(lines.last().unwrap().ends_with("[y/N]"));
        assert_eq!(c.json["publish_by_user_only"], true);
        assert!(c.json["publish_note"].as_str().unwrap().contains("自己点"));
    }

    /// 修补：已有正式流程走不通 → 接着摸索 → 新草稿（draft_of 原名）。
    fn script_repair(h: &mut FakeHands) -> FakeDcv {
        chrome(h);
        let mut failed = reply("p1", "failed", 2);
        failed.note = Some("找不到 Compose".into());
        h.push_procedure(failed);
        h.push_look(composer("a"));
        h.push_step(reply("r1", "done", 1));
        h.push_step(reply("r2", "done", 1));
        h.push_look(filled("b"));
        h.push_look(filled("b")); // 编号图
        h.push_step(reply("r3", "done", 1));
        h.push_look(sent("c"));
        FakeDcv {
            shows: vec![(
                "social-global:promote-x-post".into(),
                json!({"name": "promote-x-post", "executable": true, "steps": [
                    {"n": 1, "action": "open_url", "arg": "https://x.com/compose/post"},
                    {"n": 2, "action": "tap_by_intent", "arg": "Compose"},
                    {"n": 3, "action": "tap_by_intent", "arg": "Post"}]}),
            )],
            ..Default::default()
        }
    }

    #[test]
    fn a_repaired_draft_is_approved_under_its_new_name_never_the_original() {
        let d = tempfile::tempdir().unwrap();
        let mut h = FakeHands::new();
        let dcv = script_repair(&mut h);
        let r = go(
            &args(false),
            "x",
            &mut h,
            &dcv,
            &x_jeff(),
            "y\ny\n",
            params(),
            d.path(),
        );
        assert_eq!(r.code, 0, "{}", r.out);
        let adds = dcv.adds.borrow();
        assert!(
            adds[0]
                .windows(2)
                .any(|w| w == ["--name", "promote-x-post-0001"]),
            "{:?}",
            adds[0]
        );
        assert!(adds[0]
            .windows(2)
            .any(|w| w == ["--draft-of", "promote-x-post"]));
        assert_eq!(r.approved, ["social-global:promote-x-post-0001"]);
    }

    #[test]
    fn a_cancelled_touch_id_is_the_user_saying_no() {
        let d = tempfile::tempdir().unwrap();
        let mut h = FakeHands::new();
        script_x_explore(&mut h);
        // procedures::approve 把 SignError 变成同样的一句话（anyhow!("{e}")）。
        let mut cancelled = |_: &str| -> anyhow::Result<()> {
            Err(anyhow::anyhow!("{}", dct_brain::sign::SignError::Cancelled))
        };
        let r = go_with(
            &args(true),
            "x",
            &mut h,
            &FakeDcv::default(),
            &x_jeff(),
            "{\"verdict\":\"yes\"}\n{\"verdict\":\"yes\"}\n",
            params(),
            d.path(),
            &mut cancelled,
        );
        assert_eq!(r.code, 3, "{}", r.out);
        let last: Value = serde_json::from_str(r.out.lines().last().unwrap()).unwrap();
        assert_eq!(last["event"], "declined");

        let mut h = FakeHands::new();
        script_x_explore(&mut h);
        let mut broken = |_: &str| -> anyhow::Result<()> {
            Err(anyhow::anyhow!("确认流程失败：dcv 挂了"))
        };
        let r = go_with(
            &args(false),
            "x",
            &mut h,
            &FakeDcv::default(),
            &x_jeff(),
            "y\ny\n",
            params(),
            d.path(),
            &mut broken,
        );
        assert_eq!(r.code, 1, "别的错还是 1：{}", r.out);
    }

    #[test]
    fn a_config_error_names_the_line_but_never_echoes_the_file() {
        let d = tempfile::tempdir().unwrap();
        let f = d.path().join("config.toml");
        // 语法错：toml 自己的 Display 会把这一行原样带出来
        std::fs::write(
            &f,
            "[llm]\nprovider = \"claude\"\napi_key = sk-SECRET123 oops\n",
        )
        .unwrap();
        let e = load_config(&f).unwrap_err();
        assert!(e.contains(&f.display().to_string()), "{e}");
        assert!(e.contains("第 3 行"), "{e}");
        assert!(!e.contains("SECRET"), "{e}");
        // 类型错：serde 的说明里会带上值
        std::fs::write(
            &f,
            "[judge]\nbase_url = \"http://127.0.0.1:8765\"\nthreshold = \"sk-SECRET456\"\n",
        )
        .unwrap();
        let e = load_config(&f).unwrap_err();
        assert!(e.contains("第 3 行"), "{e}");
        assert!(!e.contains("SECRET"), "{e}");
        // 枚举值写错：serde 把值放在反引号里
        std::fs::write(&f, "[llm]\ntransport = \"sk-SECRET789\"\n").unwrap();
        let e = load_config(&f).unwrap_err();
        assert!(e.contains("第 2 行"), "{e}");
        assert!(!e.contains("SECRET"), "{e}");
        assert!(e.contains("`cli`"), "词表里的短词照留：{e}");
        // 说明一行写完，不吞掉后半句
        std::fs::write(&f, "a = sk-SECRET000\n").unwrap();
        let e = load_config(&f).unwrap_err();
        assert!(
            e.contains("expected") && !e.contains('\n') && !e.contains("SECRET"),
            "{e}"
        );
    }

    #[test]
    fn config_missing_is_defaults_and_unparsable_is_an_error() {
        let d = tempfile::tempdir().unwrap();
        let c = load_config(&d.path().join("config.toml")).unwrap();
        assert!(c.judge.is_none() && c.llm.is_none());
        let f = d.path().join("bad.toml");
        std::fs::write(&f, "[judge\nbase_url = ").unwrap();
        let e = load_config(&f).unwrap_err();
        assert!(e.contains("bad.toml"), "{e}");
        std::fs::write(&f, "[judge]\nbase_url = \"http://127.0.0.1:8765\"\n").unwrap();
        assert!(load_config(&f).unwrap().judge.is_some());
    }

    #[test]
    fn dct_do_writes_to_stdout_only_through_say() {
        // --json 时 stdout 只能有事件：这几个文件里不许直接 print / println。
        let main = include_str!("explore_cli.rs");
        let main = &main[..main.find("#[cfg(test)]\nmod tests").unwrap()];
        let io = include_str!("explore_cli/io.rs");
        let io = &io[..io.find("#[cfg(test)]\nmod tests").unwrap()];
        for (name, src) in [
            ("explore_cli.rs", main),
            ("io.rs", io),
            ("real.rs", include_str!("explore_cli/real.rs")),
        ] {
            for bad in ["println!(", "print!(", "dbg!("] {
                assert!(!src.contains(bad), "{name} 里有 {bad}");
            }
        }
    }
}
