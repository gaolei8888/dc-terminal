//! 寻物游戏的一步循环：读屏幕（认字，私人画面就停）→ 取截图给会看图的模型 → 模型指一个位置 →
//! 过安全检查 → 按位置点 → 看有没有变化 → 记一条教学记录。跟 dco、模型、时钟说话都是传进来的，测试里用假的。
//! 设计：docs/superpowers/plans/2026-10-04-dct-game-scene.md。
use crate::mood::Mood;
use crate::play::{numbers, Clock, Dco, DcoError, Profile, Region, Seen, TapSettle, TapSettleReq};
use crate::screen::Element;
use serde_json::{json, Value};

/// 落点附近不许有的字（小写比较；`+` 只算整条文字就是 `+` 的）。
/// 以后会进 dcv 的规矩；现在是通用的「花钱、提示、重来」词表，不是哪个游戏专属的。
pub const FORBID: &[&str] = &["hint", "buy", "purchase", "start over", "shop", "store", "购买", "商店", "提示", "重新开始", "+"];
/// 落点附近的范围：窗口宽、高的万分比（半边）。
const NEAR_W_BP: i32 = 700;
const NEAR_H_BP: i32 = 900;
/// 拖动的起点和落点至少隔这么远（窗口宽的 2%，万分比）。
const MIN_DRAG_BP: f64 = 200.0;
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
    /// 拖的起点（物品栏里的格子，0～1）；有它就是「从这里拖到 (x, y)」，没有就是点一下。
    pub from: Option<(f64, f64)>,
    /// 模型说目标已经达成（只在给了目标时才算数）。
    pub done: bool,
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
    // `from` 写了就必须写对：写坏了不能悄悄变成「点一下」。
    let from = match v.get("from") {
        None | Some(Value::Null) => None,
        Some(f) => {
            let (fx, fy) = (f["x"].as_f64()?, f["y"].as_f64()?);
            if !(0.0..=1.0).contains(&fx) || !(0.0..=1.0).contains(&fy) {
                return None;
            }
            Some((fx, fy))
        }
    };
    Some(Pick { name: v["name"].as_str()?.to_string(), x, y, why: v["why"].as_str().unwrap_or("").to_string(), from, done: v["done"].as_bool().unwrap_or(false) })
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
    /// 回答的是哪个大脑（`config` / `claude`）；由链式 Vision 填，单个 Vision 留空。
    pub brain: String,
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
    /// 每步走完告诉它这一步有没有效（没点、没变化、没回答都算没效）。链式 Vision 靠它决定要不要换下一个大脑。
    fn observe(&self, _effective: bool) {}
}

/// 升级链：连着这么多步没效，后面所有步都换下一个大脑。
pub const ESCALATE_AFTER: usize = 2;

/// 大脑升级链的纯逻辑：只往后走，不回头；有效的一步清零连续计数。
pub struct BrainChain {
    len: usize,
    idx: usize,
    bad: usize,
}

impl BrainChain {
    pub fn new(len: usize) -> BrainChain {
        BrainChain { len, idx: 0, bad: 0 }
    }
    pub fn current(&self) -> usize {
        self.idx
    }
    pub fn observe(&mut self, effective: bool) {
        if effective {
            self.bad = 0;
            return;
        }
        self.bad += 1;
        if self.bad >= ESCALATE_AFTER && self.idx + 1 < self.len {
            self.idx += 1;
            self.bad = 0;
        }
    }
}

pub struct SceneOptions {
    pub max_steps: usize,
    pub dry_run: bool,
    pub no_tap: Vec<Region>,
    /// 写进记录的游戏名（只是标签）。
    pub game: String,
    /// 可选的目标（一句话）；只写进记录，并让「done」生效。
    pub goal: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum SceneStop {
    StepsDone,
    /// 模型说目标已经达成。
    GoalReached,
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
    /// 所有模型调用花的时间合计（毫秒）。
    pub model_ms: u64,
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

/// 点完让 dco 等屏幕稳定：安静 0.5 秒算稳，最多等 6 秒，只看窗口上面 80%（底栏不算）。
pub const TAP_SETTLE: TapSettleReq = TapSettleReq { quiet_ms: 500, timeout_ms: 6000, region: [0.0, 0.0, 1.0, 0.8] };

/// 文字里所有连续的数字（「4/6」是 4 和 6，「5+」是 5），按出现顺序。认字抖一下（`5` 变 `5+`）不会改变它。
fn digit_runs(s: &Seen) -> Vec<u64> {
    let mut out = vec![];
    for e in &s.elements {
        let mut cur = String::new();
        for c in e.text.chars().chain(std::iter::once(' ')) {
            if c.is_ascii_digit() {
                cur.push(c);
            } else if !cur.is_empty() {
                if let Ok(n) = cur.parse() {
                    out.push(n);
                }
                cur.clear();
            }
        }
    }
    out
}

/// 「词」：至少 4 个字母数字、小写。更短的（NIE、HIN）多半是认字抖出来的碎片。
fn words(s: &Seen) -> Vec<String> {
    let mut out = vec![];
    for e in &s.elements {
        for w in e.text.split(|c: char| !c.is_alphanumeric()) {
            if w.chars().count() >= 4 {
                out.push(w.to_lowercase());
            }
        }
    }
    out
}

fn edit_distance_le1(a: &str, b: &str) -> bool {
    let (a, b): (Vec<char>, Vec<char>) = (a.chars().collect(), b.chars().collect());
    if a.len().abs_diff(b.len()) > 1 {
        return false;
    }
    let (long, short) = if a.len() >= b.len() { (&a, &b) } else { (&b, &a) };
    let Some(i) = (0..short.len()).find(|&i| long[i] != short[i]) else { return true };
    if long.len() == short.len() { long[i + 1..] == short[i + 1..] } else { long[i + 1..] == short[i..] }
}

/// 这次点有没有让画面变：dco 的稳定报告（稳了才信）优先；否则比文字，且不被认字抖动骗到：
/// 数字变了，或者出现一个以前没有、也和以前任何一个词差不到一个字母的新词。
pub fn effective(before: &Seen, after: &Seen, settle: Option<&TapSettle>) -> bool {
    if let Some(s) = settle {
        if s.settled && !s.timed_out {
            return s.changed;
        }
    }
    if digit_runs(before) != digit_runs(after) {
        return true;
    }
    let old = words(before);
    words(after).iter().any(|w| !old.iter().any(|o| edit_distance_le1(w, o)))
}

fn texts_of(s: &Seen) -> Vec<String> {
    s.elements.iter().map(|e| e.text.clone()).collect()
}

pub fn scene(dco: &mut dyn Dco, clock: &mut dyn Clock, p: &Profile, vision: &dyn Vision, o: &SceneOptions, sink: &mut dyn FnMut(SceneStep)) -> SceneSummary {
    let mut sum = SceneSummary { steps: 0, taps: 0, asks: 0, tokens_in: 0, tokens_out: 0, model_ms: 0, stop: SceneStop::StepsDone };
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
        let asked_at = clock.now_ms();
        let ans = match vision.pick(&png, &history) {
            Ok(a) => a,
            Err(f) => break SceneStop::NoVision(f),
        };
        let model_ms = clock.now_ms().saturating_sub(asked_at);
        sum.asks += 1;
        sum.model_ms += model_ms;
        if let Some((i, out)) = ans.tokens {
            sum.tokens_in += i;
            sum.tokens_out += out;
        }
        sum.steps += 1;
        let n = sum.steps;
        let mut rec = json!({
            "schema": 1, "game": o.game, "goal": o.goal, "time_ms": clock.now_ms(),
            "screen": { "png": format!("png/{n:04}.png"), "texts": texts_of(&before), "numbers": numbers(&before) },
            "teacher": "qwen", "rationale": ans.pick.as_ref().map(|k| k.why.clone()).unwrap_or_else(|| ans.raw.chars().take(200).collect()),
            "model": ans.model, "brain": ans.brain, "assisted": "none", "model_ms": model_ms, "image_bytes": ans.image_bytes, "image_note": ans.image_note, "tokens_in": ans.tokens.map(|t| t.0), "tokens_out": ans.tokens.map(|t| t.1),
        });
        let mut emit = |rec: Value, say: String| sink(SceneStep { record: rec, png: Some(png.clone()), say });
        // 给了目标、模型说已经达成：记一条，不点，结束。
        if o.goal.is_some() && ans.pick.as_ref().is_some_and(|k| k.done) {
            rec["label"] = json!("goal_reached");
            emit(rec, format!("第 {n} 步：大模型说目标已经达成"));
            break SceneStop::GoalReached;
        }
        // 过检查：任何一项不满足就不点。
        let skip = match &ans.pick {
            None => Some("大模型没指出能用的位置".to_string()),
            Some(k) => {
                let (x, y) = (to_bp(k.x), to_bp(k.y));
                rec["action"] = match k.from {
                    None => json!({ "kind": "tap_at", "x_bp": x, "y_bp": y, "name": k.name }),
                    Some(f) => json!({ "kind": "drag", "from_x_bp": to_bp(f.0), "from_y_bp": to_bp(f.1), "x_bp": x, "y_bp": y, "name": k.name }),
                };
                let too_short = k.from.is_some_and(|f| ((to_bp(f.0) as f64 - x as f64).powi(2) + (to_bp(f.1) as f64 - y as f64).powi(2)).sqrt() < MIN_DRAG_BP);
                if too_short {
                    Some(format!("拖的距离太短（{}）", k.name))
                } else if in_regions(x, y, &o.no_tap) {
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
            vision.observe(false);
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
            emit(rec, format!("第 {n} 步：试走，会{}『{}』，没有真{}", if k.from.is_some() { "拖到" } else { "点" }, k.name, if k.from.is_some() { "拖" } else { "点" }));
            break SceneStop::DryRun;
        }
        dco.show_status_with("think", Some("看场景"), None);
        let tapped = match k.from {
            None => dco.tap_at(p, x, y, &o.no_tap, Some(&TAP_SETTLE)).map(|t| t.settle),
            // 拖：没有稳定报告，之后只比文字。
            Some(f) => dco.swipe(p, f, (k.x, k.y)).map(|()| None),
        };
        dco.show_status("look");
        let settle: Option<TapSettle> = match tapped {
            Err(e) if e.code == "unsupported" => break 'run SceneStop::NoTapAt,
            Err(e) if e.code == "private_screen" => break 'run SceneStop::PrivateScreen,
            Err(e) if e.code == "not_allowed" => {
                rec["label"] = json!("skipped");
                rec["skip_reason"] = json!(format!("dco 不让点：{}", e.message));
                emit(rec, format!("第 {n} 步：这一步没点：dco 不让点这里"));
                vision.observe(false);
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
            Ok(t) => t,
        };
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
        let changed = effective(&before, &after, settle.as_ref());
        rec["outcome"] = json!({ "changed": changed, "texts_after": texts_of(&after), "by": if settle.as_ref().is_some_and(|s| s.settled && !s.timed_out) { "dco_settle" } else { "texts" } });
        rec["label"] = json!(if changed { "effective" } else { "noop" });
        emit(rec, format!("第 {n} 步：点了『{}』，画面{}", k.name, if changed { "变了" } else { "没变" }));
        vision.observe(changed);
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
