//! 寻物游戏的一步循环：读屏幕（认字，私人画面就停）→ 取截图给会看图的模型 → 模型指一个位置 →
//! 过安全检查 → 按位置点 → 看有没有变化 → 记一条教学记录。跟 dco、模型、时钟说话都是传进来的，测试里用假的。
//! 设计：docs/superpowers/plans/2026-10-04-dct-game-scene.md。
use crate::mood::Mood;
use crate::play::{numbers, Clock, Dco, DcoError, Profile, Region, Seen};
use crate::screen::Element;
use serde_json::{json, Value};

/// 落点附近不许有的字（小写比较；`+` 只算整条文字就是 `+` 的）。
/// 以后会进 dcv 的规矩；现在是通用的「花钱、提示、重来」词表，不是哪个游戏专属的。
pub const FORBID: &[&str] = &["hint", "buy", "purchase", "start over", "shop", "store", "购买", "商店", "提示", "重新开始", "+"];
/// 落点附近的范围：窗口宽、高的万分比（半边）。
const NEAR_W_BP: i32 = 700;
const NEAR_H_BP: i32 = 900;
/// 去重的格子大小：2%。
const QUANT_BP: u16 = 200;
/// 点完等多久再读（毫秒）。
pub const AFTER_TAP_MS: u64 = 1500;
const MAX_SKIPPED_IN_A_ROW: usize = 3;
const MAX_NOOP_IN_A_ROW: usize = 5;

#[derive(Clone, Debug, PartialEq)]
pub struct Pick {
    pub name: String,
    pub x: f64,
    pub y: f64,
    pub why: String,
}

/// 从模型回答里找出 `{"name","x","y","why"}`。去掉思考块；`x`、`y` 必须是 0～1 的数；读不懂就 `None`。
pub fn parse_pick(text: &str) -> Option<Pick> {
    let mut t = text.to_string();
    while let (Some(a), Some(b)) = (t.find("<think>"), t.find("</think>")) {
        if b < a {
            break;
        }
        t.replace_range(a..b + "</think>".len(), "");
    }
    let (a, b) = (t.find('{')?, t.rfind('}')?);
    if b < a {
        return None;
    }
    let v: Value = serde_json::from_str(&t[a..=b]).ok()?;
    let (x, y) = (v["x"].as_f64()?, v["y"].as_f64()?);
    if !(0.0..=1.0).contains(&x) || !(0.0..=1.0).contains(&y) {
        return None;
    }
    Some(Pick { name: v["name"].as_str()?.to_string(), x, y, why: v["why"].as_str().unwrap_or("").to_string() })
}

/// 0～1 的位置变成整数万分比：四舍五入，夹在 0..=10000。
pub fn to_bp(x: f64) -> u16 {
    (x * 10000.0).round().clamp(0.0, 10000.0) as u16
}

/// 落点在不在任何一块里（含边界）。
pub fn in_regions(x_bp: u16, y_bp: u16, regions: &[Region]) -> bool {
    let (x, y) = (x_bp as u32, y_bp as u32);
    regions.iter().any(|r| x >= r.x_bp as u32 && x <= r.x_bp as u32 + r.w_bp as u32 && y >= r.y_bp as u32 && y <= r.y_bp as u32 + r.h_bp as u32)
}

/// 落点附近（窗口宽 7%、高 9% 以内）有没有带危险字的文字元素；有就返回那条文字。
pub fn near_forbidden(elements: &[Element], x_bp: u16, y_bp: u16) -> Option<String> {
    elements.iter().find_map(|e| {
        let [fx, fy, fw, fh] = e.frac?;
        let (cx, cy) = (fx as i32 + fw as i32 / 2, fy as i32 + fh as i32 / 2);
        if (cx - x_bp as i32).abs() >= NEAR_W_BP || (cy - y_bp as i32).abs() >= NEAR_H_BP {
            return None;
        }
        let low = e.text.to_lowercase();
        let bad = FORBID.iter().any(|w| if *w == "+" { low.trim() == "+" } else { low.contains(w) });
        bad.then(|| e.text.clone())
    })
}

/// 同一格（2%）就算同一个落点。
pub fn quantise(x_bp: u16, y_bp: u16) -> (u16, u16) {
    (x_bp / QUANT_BP, y_bp / QUANT_BP)
}

pub struct VisionAnswer {
    /// 解析出的位置；模型回了但读不懂就是 `None`。
    pub pick: Option<Pick>,
    pub raw: String,
    pub model: String,
    pub tokens: Option<(u64, u64)>,
    /// 实际发给模型的图有多大（字节），和缩图时要说的话（没缩成就写原因）。
    pub image_bytes: Option<usize>,
    pub image_note: Option<String>,
}

/// 模型这一问为什么没拿到回答。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VisionFail {
    /// 没配 / 没说原因。
    Silent,
    Timeout,
    Error,
}

/// 给一张图，指出下一个最值得点的位置。`history` 是已经点过的位置（万分比）。没配模型 / 没回应 = `Err`。
pub trait Vision {
    fn pick(&self, png: &[u8], history: &[(u16, u16)]) -> Result<VisionAnswer, VisionFail>;
}

pub struct SceneOptions {
    pub max_steps: usize,
    pub dry_run: bool,
    pub no_tap: Vec<Region>,
    /// 写进记录的游戏名（只是标签）。
    pub game: String,
}

#[derive(Clone, Debug, PartialEq)]
pub enum SceneStop {
    StepsDone,
    /// 读不了屏幕：私人画面。
    PrivateScreen,
    /// 旧 dco 没有按位置点。
    NoTapAt,
    /// 模型没回应 / 没配。
    NoVision(VisionFail),
    Skipped3,
    Noop5,
    Dco(DcoError),
    DryRun,
}

/// 每走一步交给调用方的东西：一条记录（`screen.png` 已填好相对路径）、发给模型的那张图、一句大白话。
pub struct SceneStep {
    pub record: Value,
    pub png: Option<Vec<u8>>,
    pub say: String,
}

pub struct SceneSummary {
    pub steps: usize,
    pub taps: usize,
    pub asks: usize,
    pub tokens_in: u64,
    pub tokens_out: u64,
    pub stop: SceneStop,
}

/// 停下时章鱼的表情。
pub fn mood_for_stop(stop: &SceneStop) -> Option<Mood> {
    let text = match stop {
        SceneStop::PrivateScreen => "私人画面，没操作",
        SceneStop::NoTapAt => "要更新章鱼",
        SceneStop::NoVision(_) => "大模型没回应",
        SceneStop::Skipped3 | SceneStop::Noop5 => "没把握，停了",
        _ => return None,
    };
    Some(Mood { state: "wait", text: Some(text) })
}

fn texts_of(s: &Seen) -> Vec<String> {
    s.elements.iter().map(|e| e.text.clone()).collect()
}

pub fn scene(dco: &mut dyn Dco, clock: &mut dyn Clock, p: &Profile, vision: &dyn Vision, o: &SceneOptions, sink: &mut dyn FnMut(SceneStep)) -> SceneSummary {
    let mut sum = SceneSummary { steps: 0, taps: 0, asks: 0, tokens_in: 0, tokens_out: 0, stop: SceneStop::StepsDone };
    let mut history: Vec<(u16, u16)> = vec![];
    let mut skipped = 0;
    let mut noop = 0;
    let stop = 'run: loop {
        if sum.steps >= o.max_steps {
            break SceneStop::StepsDone;
        }
        // 先认字：失败（含私人画面）就停，**不取截图、不问模型**。
        let before = match dco.see_text(p) {
            Ok(s) => s,
            Err(e) if e.code == "private_screen" => break SceneStop::PrivateScreen,
            Err(e) => break SceneStop::Dco(e),
        };
        let png = match dco.capture(p) {
            Ok(b) => b,
            Err(e) if e.code == "private_screen" => break SceneStop::PrivateScreen,
            Err(e) => break SceneStop::Dco(e),
        };
        let ans = match vision.pick(&png, &history) {
            Ok(a) => a,
            Err(f) => break SceneStop::NoVision(f),
        };
        sum.asks += 1;
        if let Some((i, out)) = ans.tokens {
            sum.tokens_in += i;
            sum.tokens_out += out;
        }
        sum.steps += 1;
        let n = sum.steps;
        let mut rec = json!({
            "schema": 1, "game": o.game, "time_ms": clock.now_ms(),
            "screen": { "png": format!("png/{n:04}.png"), "texts": texts_of(&before), "numbers": numbers(&before) },
            "teacher": "qwen", "rationale": ans.pick.as_ref().map(|k| k.why.clone()).unwrap_or_else(|| ans.raw.chars().take(200).collect()),
            "model": ans.model, "image_bytes": ans.image_bytes, "image_note": ans.image_note, "tokens_in": ans.tokens.map(|t| t.0), "tokens_out": ans.tokens.map(|t| t.1),
        });
        let mut emit = |rec: Value, say: String| sink(SceneStep { record: rec, png: Some(png.clone()), say });
        // 过检查：任何一项不满足就不点。
        let skip = match &ans.pick {
            None => Some("大模型没指出能用的位置".to_string()),
            Some(k) => {
                let (x, y) = (to_bp(k.x), to_bp(k.y));
                rec["action"] = json!({ "kind": "tap_at", "x_bp": x, "y_bp": y, "name": k.name });
                if in_regions(x, y, &o.no_tap) {
                    Some(format!("落点在不点的区域里（{}）", k.name))
                } else if let Some(t) = near_forbidden(&before.elements, x, y) {
                    Some(format!("落点附近有「{t}」"))
                } else if history.last().is_some_and(|h| quantise(h.0, h.1) == quantise(x, y)) {
                    Some("和上一次点的是同一个位置".to_string())
                } else {
                    None
                }
            }
        };
        if let Some(reason) = skip {
            rec["label"] = json!("skipped");
            rec["skip_reason"] = json!(reason);
            emit(rec, format!("第 {n} 步：这一步没点：{reason}"));
            skipped += 1;
            if skipped >= MAX_SKIPPED_IN_A_ROW {
                break SceneStop::Skipped3;
            }
            continue;
        }
        let k = ans.pick.as_ref().expect("过了检查就一定有位置");
        let (x, y) = (to_bp(k.x), to_bp(k.y));
        if o.dry_run {
            rec["label"] = json!("dry_run");
            emit(rec, format!("第 {n} 步：试走，会点『{}』，没有真点", k.name));
            break SceneStop::DryRun;
        }
        dco.show_status_with("think", Some("看场景"), None);
        let tapped = dco.tap_at(p, x, y, &o.no_tap);
        dco.show_status("look");
        match tapped {
            Err(e) if e.code == "unsupported" => break 'run SceneStop::NoTapAt,
            Err(e) if e.code == "private_screen" => break 'run SceneStop::PrivateScreen,
            Err(e) if e.code == "not_allowed" => {
                rec["label"] = json!("skipped");
                rec["skip_reason"] = json!(format!("dco 不让点：{}", e.message));
                emit(rec, format!("第 {n} 步：这一步没点：dco 不让点这里"));
                skipped += 1;
                if skipped >= MAX_SKIPPED_IN_A_ROW {
                    break SceneStop::Skipped3;
                }
                continue;
            }
            Err(e) => {
                rec["label"] = json!("error");
                rec["error"] = json!(e.message);
                emit(rec, format!("第 {n} 步：点的时候出错了"));
                break SceneStop::Dco(e);
            }
            Ok(_) => {}
        }
        skipped = 0;
        sum.taps += 1;
        history.push((x, y));
        clock.sleep_ms(AFTER_TAP_MS);
        let after = match dco.see_text(p) {
            Ok(s) => s,
            Err(e) => {
                rec["label"] = json!("error");
                rec["error"] = json!(e.message);
                emit(rec, format!("第 {n} 步：点了『{}』，但之后读不了屏幕", k.name));
                break if e.code == "private_screen" { SceneStop::PrivateScreen } else { SceneStop::Dco(e) };
            }
        };
        let changed = texts_of(&before) != texts_of(&after) || numbers(&before) != numbers(&after);
        rec["outcome"] = json!({ "changed": changed, "texts_after": texts_of(&after) });
        rec["label"] = json!(if changed { "effective" } else { "noop" });
        emit(rec, format!("第 {n} 步：点了『{}』，画面{}", k.name, if changed { "变了" } else { "没变" }));
        noop = if changed { 0 } else { noop + 1 };
        if noop >= MAX_NOOP_IN_A_ROW {
            break SceneStop::Noop5;
        }
    };
    if let Some(m) = mood_for_stop(&stop) {
        dco.show_status_with(m.state, m.text, None);
    }
    sum.stop = stop;
    sum
}
