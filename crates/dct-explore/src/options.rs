//! 每一轮把画面变成一道选择题：能点的元素各一项 + 固定动作；每个选项对应一串封闭词表步骤。
//! 题面里没有参数值（调用方在 look 进来前已把值换成 `{名字}`）。
use crate::app_profile::AppProfile;
use crate::gate::{fold, tappable};
use crate::hands::{El, Look, StepReq};
use crate::JevChoice;

#[derive(Debug, Clone, PartialEq)]
pub enum Act {
    /// 有字：intent = 元素上的字
    Tap {
        el: El,
        intent: String,
    },
    /// 没字：name 由大模型或用户给
    TapIcon {
        el: El,
        name: Option<String>,
    },
    /// tap_by_intent field -> type_param param
    Fill {
        param: String,
        field: El,
        intent: String,
    },
    /// pick_file param（选择框要已经打开）
    Attach {
        param: String,
    },
    Scroll {
        down: bool,
    },
    Wait,
    Done,
    Stuck,
}

#[derive(Debug, Clone)]
pub struct Opt {
    pub key: String,
    pub text: String,
    pub act: Act,
    pub publish: bool,
}

pub struct Goal<'a> {
    /// 用户的目标（「把这段文字发到 X」），已脱敏
    pub text: &'a str,
    pub platform: &'a AppProfile,
    /// post_text、title…
    pub text_params: &'a [String],
    /// video、image…
    pub file_params: &'a [String],
}

pub struct Progress {
    pub filled: Vec<String>,
    pub attached: Vec<String>,
    pub published: bool,
    pub done_lines: Vec<String>,
}

/// 给 Jeff 的选项上限（judge 设计 2–20）。
const MAX_OPTIONS: usize = 20;
const MAX_STATE_LINES: usize = 200;

fn is_input_role(role: &str) -> bool {
    matches!(role, "textfield" | "textarea" | "searchfield")
}

/// dco 报不出灰字：看起来是问句、以「…」「?」结尾的字当输入框的占位字。
fn looks_like_placeholder(text: &str) -> bool {
    let t = text.trim_end();
    ["?", "？", "…", "..."].iter().any(|s| t.ends_with(s))
}

fn is_input(e: &El) -> bool {
    is_input_role(&e.role) || looks_like_placeholder(&e.text)
}

fn center(e: &El) -> (f64, f64) {
    (e.frac.x + e.frac.w / 2.0, e.frac.y + e.frac.h / 2.0)
}

/// 没字的框描述成「离它最近的有字元素」的某一边。
pub(crate) fn neighbour_hint(e: &El, all: &[El]) -> String {
    let (cx, cy) = center(e);
    let near = all
        .iter()
        .filter(|o| o.id != e.id && !o.text.trim().is_empty())
        .map(|o| {
            let (ox, oy) = center(o);
            (((ox - cx).powi(2) + (oy - cy).powi(2)), o, ox - cx, oy - cy)
        })
        .min_by(|a, b| a.0.total_cmp(&b.0));
    match near {
        Some((_, o, dx, dy)) => {
            // 对方在我的哪边 = 我在对方的反方向
            let side = if dx.abs() >= dy.abs() {
                if dx > 0.0 {
                    "左"
                } else {
                    "右"
                }
            } else if dy > 0.0 {
                "上"
            } else {
                "下"
            };
            format!("没有字的图标，在「{}」{}边", o.text.trim(), side)
        }
        None => "没有字的图标，画面上没有其他字可参照".to_string(),
    }
}

fn element_key(id: &str) -> String {
    if id.starts_with('e') {
        id.to_string()
    } else {
        format!("e{id}")
    }
}

fn fixed(key: &str, text: &str, act: Act) -> Opt {
    Opt {
        key: key.into(),
        text: text.into(),
        act,
        publish: false,
    }
}

/// 全部选项（不截断，给大模型和用户看）。
///
/// 规则：
/// - 有字 -> `Tap`；没字的检测框 -> `TapIcon`；`gate::tappable` 为假的不进。
/// - 发布词（整句相等，fold 后比）-> `publish: true`，只有所有文字参数都已在画面上、
///   所有文件参数都已附上，才进选项（发布前画面上必须看得到要发的字）；已经点过一次发布
///   （`prog.published`，两段式发布的确认框上）不再要求。
/// - `Fill`：每个还没填的文字参数 × 每个像输入框的元素；有文件参数没附上时一项都不出（文件先于文字）。
/// - `Attach`：每个还没附上的文件参数一项。
/// - 固定动作 scroll_down / scroll_up / wait / stuck；done 只在已发布后出。
pub fn all_options(
    look: &Look,
    goal: &Goal,
    prog: &Progress,
    values_on_screen: &dyn Fn(&str) -> bool,
) -> Vec<Opt> {
    let mut out: Vec<Opt> = Vec::new();
    let publish_words: Vec<String> = goal
        .platform
        .publish_words
        .iter()
        .map(|w| fold(w))
        .collect();
    // 已经点过一次发布（两段式发布的确认框上）：内容在第一下之前已经核对过，确认框上看不到帖子也照样给发布词。
    let ready = prog.published
        || (goal.text_params.iter().all(|p| values_on_screen(p))
            && goal.file_params.iter().all(|p| prog.attached.contains(p)));
    let files_pending = goal.file_params.iter().any(|p| !prog.attached.contains(p));

    for e in &look.elements {
        let label = e.text.trim();
        if !tappable(label, goal.platform) {
            continue;
        }
        let key = element_key(&e.id);
        if out.iter().any(|o| o.key == key) {
            continue; // key 必须唯一
        }
        if label.is_empty() {
            out.push(Opt {
                key,
                text: neighbour_hint(e, &look.elements),
                act: Act::TapIcon {
                    el: e.clone(),
                    name: None,
                },
                publish: false,
            });
            continue;
        }
        let publish = {
            let f = fold(label);
            publish_words.iter().any(|w| !w.is_empty() && *w == f)
        };
        if publish && !ready {
            continue;
        }
        let kind = if e.role == "button" {
            "按钮"
        } else {
            "文字"
        };
        out.push(Opt {
            key,
            text: format!("{kind} {label}"),
            act: Act::Tap {
                el: e.clone(),
                intent: label.to_string(),
            },
            publish,
        });
    }

    if !files_pending {
        for param in goal.text_params.iter().filter(|p| !prog.filled.contains(p)) {
            for e in look.elements.iter().filter(|e| is_input(e)) {
                let label = e.text.trim();
                // 没字的输入框没法按字去点，交给 TapIcon
                if label.is_empty() || !tappable(label, goal.platform) {
                    continue;
                }
                out.push(Opt {
                    key: format!("fill_{param}_{}", e.id),
                    text: format!("在「{label}」里填 {{{param}}}"),
                    act: Act::Fill {
                        param: param.clone(),
                        field: e.clone(),
                        intent: label.to_string(),
                    },
                    publish: false,
                });
            }
        }
    }
    for param in goal
        .file_params
        .iter()
        .filter(|p| !prog.attached.contains(p))
    {
        out.push(Opt {
            key: format!("attach_{param}"),
            text: format!("在打开的选择框里选文件 {{{param}}}"),
            act: Act::Attach {
                param: param.clone(),
            },
            publish: false,
        });
    }

    out.push(fixed("scroll_down", "向下滚动", Act::Scroll { down: true }));
    out.push(fixed("scroll_up", "向上滚动", Act::Scroll { down: false }));
    out.push(fixed("wait", "等一会儿", Act::Wait));
    if prog.published {
        out.push(fixed("done", "已经发完了", Act::Done));
    }
    out.push(fixed("stuck", "看不懂或者卡住了", Act::Stuck));
    out
}

fn tokens(s: &str) -> Vec<String> {
    fold(s)
        .split(|c: char| !c.is_alphanumeric())
        .filter(|t| t.chars().count() >= 2 || !t.is_ascii())
        .map(str::to_owned)
        .collect()
}

fn score(o: &Opt, goal: &Goal, vocab: &[String]) -> i32 {
    let el = match &o.act {
        Act::Tap { el, .. } | Act::TapIcon { el, .. } => el,
        _ => return 0,
    };
    let mut s = 0;
    if o.publish {
        s += 3;
    }
    if tokens(&el.text).iter().any(|t| vocab.contains(t)) {
        s += 2;
    }
    if el.role == "button" {
        s += 1;
    }
    let (cx, cy) = center(el);
    if (0.2..=0.8).contains(&cx) && (0.2..=0.8).contains(&cy) {
        s += 1;
    }
    let _ = goal;
    s
}

/// 给 Jeff 的：最多 20 个；固定动作永远在内（`done` 如果在也算固定动作）。
/// 固定动作、`Fill`、`Attach` 先占位；元素按分数排（发布词 +3、和目标 / 平台发布词 / 参数名的词重合 +2、
/// button +1、在窗口中间 60% +1），同分从上到下、从左到右。保持 `all_options` 里的原顺序输出。
pub fn shortlist(opts: &[Opt], goal: &Goal) -> Vec<Opt> {
    let is_fixed = |o: &Opt| {
        matches!(
            o.act,
            Act::Scroll { .. } | Act::Wait | Act::Done | Act::Stuck
        )
    };
    let is_slot = |o: &Opt| matches!(o.act, Act::Fill { .. } | Act::Attach { .. });
    let mut keep = vec![false; opts.len()];
    let mut n = 0;
    for (i, o) in opts.iter().enumerate() {
        if is_fixed(o) {
            keep[i] = true;
            n += 1;
        }
    }
    for (i, o) in opts.iter().enumerate() {
        if is_slot(o) && n < MAX_OPTIONS {
            keep[i] = true;
            n += 1;
        }
    }
    let mut vocab = tokens(goal.text);
    for w in &goal.platform.publish_words {
        vocab.extend(tokens(w));
    }
    for p in goal.text_params.iter().chain(goal.file_params) {
        vocab.extend(tokens(p));
    }
    let mut rest: Vec<(usize, i32, f64, f64)> = opts
        .iter()
        .enumerate()
        .filter(|(_, o)| !is_fixed(o) && !is_slot(o))
        .map(|(i, o)| {
            let (y, x) = match &o.act {
                Act::Tap { el, .. } | Act::TapIcon { el, .. } => (el.frac.y, el.frac.x),
                _ => (0.0, 0.0),
            };
            (i, score(o, goal, &vocab), y, x)
        })
        .collect();
    rest.sort_by(|a, b| {
        b.1.cmp(&a.1)
            .then(a.2.total_cmp(&b.2))
            .then(a.3.total_cmp(&b.3))
    });
    for (i, ..) in rest {
        if n >= MAX_OPTIONS {
            break;
        }
        keep[i] = true;
        n += 1;
    }
    opts.iter()
        .zip(keep)
        .filter(|(_, k)| *k)
        .map(|(o, _)| o.clone())
        .collect()
}

/// 题面：目标 + 已走的步骤 + 页面上的字（最多 200 行）。
pub fn state_text(goal: &Goal, trail: &[String], look: &Look) -> String {
    let mut s = format!(
        "目标：{}\n平台：{}\n已经做了：\n",
        goal.text, goal.platform.name
    );
    if trail.is_empty() {
        s.push_str("- （还没有）\n");
    }
    for t in trail {
        s.push_str(&format!("- {t}\n"));
    }
    s.push_str("页面上的字：\n");
    for e in look.elements.iter().take(MAX_STATE_LINES) {
        let t = e.text.trim();
        let t = if t.is_empty() { "（没有字）" } else { t };
        s.push_str(&format!("[{}] {}\n", e.id, t));
    }
    s
}

/// 题目固定一句话；criteria 按选项顺序 key -> text（Jeff 按位置读，key 重复时补后缀保证唯一）。
pub fn question(opts: &[Opt]) -> JevChoice {
    let mut criteria: Vec<(String, Option<String>)> = Vec::with_capacity(opts.len());
    for o in opts {
        let mut key = o.key.clone();
        let mut n = 2;
        while criteria.iter().any(|(k, _)| *k == key) {
            key = format!("{}_{n}", o.key);
            n += 1;
        }
        criteria.push((key, Some(o.text.clone())));
    }
    JevChoice {
        instructions: "Pick the single next action that best moves toward the goal.".into(),
        criteria,
    }
}

fn step(action: &str, arg: &str) -> StepReq {
    StepReq {
        action: action.into(),
        arg: arg.into(),
        done_when: None,
    }
}

/// 一个选项变成要交给 dco 的步骤（Fill 是两步）。`TapIcon` 还没有名字、`Done`、`Stuck`
/// 不能执行，返回空（调用方要先拿到名字）。
pub fn steps_for(o: &Opt) -> Vec<StepReq> {
    match &o.act {
        Act::Tap { intent, .. } => vec![step("tap_by_intent", intent)],
        Act::TapIcon { name: Some(n), .. } => vec![step("tap_by_intent", n)],
        Act::TapIcon { name: None, .. } => vec![],
        Act::Fill { param, intent, .. } => {
            vec![step("tap_by_intent", intent), step("type_param", param)]
        }
        Act::Attach { param } => vec![step("pick_file", param)],
        Act::Scroll { down } => vec![step("scroll", if *down { "down" } else { "up" })],
        Act::Wait => vec![step("wait", "2")],
        Act::Done | Act::Stuck => vec![],
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app_profile::Table;
    use crate::hands::{El, Frac, Look};

    fn el(id: &str, t: &str, role: &str, x: f64, y: f64) -> El {
        El {
            id: id.into(),
            text: t.into(),
            role: role.into(),
            frac: Frac {
                x,
                y,
                w: 0.06,
                h: 0.03,
            },
            source: "ocr".into(),
        }
    }
    fn look(els: Vec<El>) -> Look {
        Look {
            snapshot_id: "s".into(),
            observation_id: None,
            scene_hash: Some("h".into()),
            elements: els,
            marks_png: None,
        }
    }
    fn x_composer() -> Look {
        look(vec![
            el("e1", "Post", "button", 0.08, 0.5),
            el("e2", "What's happening?", "textarea", 0.45, 0.25),
            el("e3", "Post", "button", 0.8, 0.35),
            el("e4", "Follow", "button", 0.9, 0.7),
            el("e5", "", "image", 0.3, 0.35),
        ])
    }

    fn goal<'a>(t: &'a Table, text: &'a [String], files: &'a [String]) -> Goal<'a> {
        Goal {
            text: "把这段文字发到 X",
            platform: t.get("x").unwrap(),
            text_params: text,
            file_params: files,
        }
    }
    fn empty() -> Progress {
        Progress {
            filled: vec![],
            attached: vec![],
            published: false,
            done_lines: vec![],
        }
    }

    #[test]
    fn publish_is_hidden_until_every_text_param_shows() {
        let t = Table::seed();
        let tp = vec!["post_text".to_string()];
        let before = all_options(&x_composer(), &goal(&t, &tp, &[]), &empty(), &|_| false);
        assert!(
            before.iter().all(|o| !o.publish),
            "字还没在画面上，两个 Post 都不能选"
        );
        let after = all_options(&x_composer(), &goal(&t, &tp, &[]), &empty(), &|p| {
            p == "post_text"
        });
        assert_eq!(
            after.iter().filter(|o| o.publish).count(),
            2,
            "字在了，两个 Post 都是候选，选哪个交给 Jeff"
        );
    }

    #[test]
    fn publish_also_waits_for_files() {
        let t = Table::seed();
        let tp = vec!["post_text".to_string()];
        let fp = vec!["image".to_string()];
        let o = all_options(&x_composer(), &goal(&t, &tp, &fp), &empty(), &|_| true);
        assert!(o.iter().all(|o| !o.publish), "文件没附上不能发");
        let mut p = empty();
        p.attached.push("image".into());
        let o = all_options(&x_composer(), &goal(&t, &tp, &fp), &p, &|_| true);
        assert_eq!(o.iter().filter(|o| o.publish).count(), 2);
    }

    #[test]
    fn files_come_before_text() {
        let t = Table::seed();
        let tp = vec!["post_text".to_string()];
        let fp = vec!["image".to_string()];
        let o = all_options(&x_composer(), &goal(&t, &tp, &fp), &empty(), &|_| false);
        assert!(o.iter().any(|o| matches!(o.act, Act::Attach { .. })));
        assert!(
            o.iter().all(|o| !matches!(o.act, Act::Fill { .. })),
            "文件没附上之前不出填字选项"
        );
        let mut p = empty();
        p.attached.push("image".into());
        let o = all_options(&x_composer(), &goal(&t, &tp, &fp), &p, &|_| false);
        assert!(o
            .iter()
            .any(|o| matches!(&o.act, Act::Fill { param, .. } if param == "post_text")));
        assert!(o.iter().all(|o| !matches!(o.act, Act::Attach { .. })));
    }

    #[test]
    fn never_tap_is_not_an_option_and_done_only_after_publish() {
        let t = Table::seed();
        let tp = vec!["post_text".to_string()];
        let o = all_options(&x_composer(), &goal(&t, &tp, &[]), &empty(), &|_| true);
        assert!(o.iter().all(|o| o.key != "e4"), "Follow 不进选项");
        assert!(o.iter().all(|o| !matches!(o.act, Act::Done)));
        let mut p = empty();
        p.published = true;
        assert!(
            all_options(&x_composer(), &goal(&t, &tp, &[]), &p, &|_| true)
                .iter()
                .any(|o| matches!(o.act, Act::Done))
        );
    }

    #[test]
    fn a_textless_box_is_an_icon_option_described_by_its_neighbour() {
        let t = Table::seed();
        let o = all_options(&x_composer(), &goal(&t, &[], &[]), &empty(), &|_| false);
        let icon = o.iter().find(|o| o.key == "e5").unwrap();
        assert!(matches!(icon.act, Act::TapIcon { name: None, .. }));
        assert!(icon.text.contains("没有字的图标"), "{}", icon.text);
        assert!(icon.text.contains('「'), "{}", icon.text);
    }

    #[test]
    fn shortlist_keeps_at_most_twenty_and_always_the_fixed_actions() {
        let t = Table::seed();
        let many: Vec<El> = (0..40)
            .map(|i| {
                el(
                    &format!("e{i}"),
                    &format!("Link {i}"),
                    "link",
                    0.5,
                    i as f64 / 40.0,
                )
            })
            .collect();
        let all = all_options(&look(many), &goal(&t, &[], &[]), &empty(), &|_| false);
        let s = shortlist(&all, &goal(&t, &[], &[]));
        assert!(s.len() <= 20);
        for k in ["scroll_down", "scroll_up", "wait", "stuck"] {
            assert!(s.iter().any(|o| o.key == k), "{k}");
        }
    }

    #[test]
    fn shortlist_prefers_publish_and_keeps_fill() {
        let t = Table::seed();
        let tp = vec!["post_text".to_string()];
        let mut els: Vec<El> = (0..40)
            .map(|i| {
                el(
                    &format!("e{}", i + 10),
                    &format!("Link {i}"),
                    "link",
                    0.5,
                    i as f64 / 40.0,
                )
            })
            .collect();
        els.push(el("e99", "Post", "button", 0.9, 0.95));
        els.push(el("e98", "What's happening?", "textarea", 0.1, 0.99));
        let g = goal(&t, &tp, &[]);
        let all = all_options(&look(els), &g, &empty(), &|_| true);
        let s = shortlist(&all, &g);
        assert!(s.len() <= 20);
        assert!(s.iter().any(|o| o.key == "e99"), "发布词在最底下也要入选");
        assert!(s.iter().any(|o| matches!(o.act, Act::Fill { .. })));
    }

    #[test]
    fn state_text_never_contains_param_values() {
        let t = Table::seed();
        let l = look(vec![el("e2", "hello world from lei", "textarea", 0.4, 0.3)]);
        let s = state_text(
            &goal(&t, &[], &[]),
            &[
                "tap_by_intent: What's happening?".into(),
                "type_param: post_text".into(),
            ],
            &l,
        );
        // 调用方在 look 进来前已把参数值换成 {post_text}（见 Task D9 的 redact）；这里只验证题面结构
        assert!(s.starts_with("目标："));
        assert!(s.contains("已经做了："));
        assert!(s.contains("页面上的字："));
        assert!(s.contains("[e2] "));
    }

    #[test]
    fn question_keeps_order_and_unique_keys() {
        let o = |k: &str| Opt {
            key: k.into(),
            text: format!("t-{k}"),
            act: Act::Wait,
            publish: false,
        };
        let q = question(&[o("b"), o("a"), o("b")]);
        let keys: Vec<&str> = q.criteria.iter().map(|(k, _)| k.as_str()).collect();
        assert_eq!(keys, ["b", "a", "b_2"]);
        assert_eq!(q.criteria[1].1.as_deref(), Some("t-a"));
        assert!(!q.instructions.is_empty());
    }

    #[test]
    fn fill_becomes_tap_then_type_param() {
        let o = Opt {
            key: "fill_post_text_e2".into(),
            text: "".into(),
            publish: false,
            act: Act::Fill {
                param: "post_text".into(),
                field: el("e2", "What's happening?", "textarea", 0.4, 0.3),
                intent: "What's happening?".into(),
            },
        };
        let s = steps_for(&o);
        assert_eq!(
            (s[0].action.as_str(), s[0].arg.as_str()),
            ("tap_by_intent", "What's happening?")
        );
        assert_eq!(
            (s[1].action.as_str(), s[1].arg.as_str()),
            ("type_param", "post_text")
        );
    }

    #[test]
    fn other_acts_map_to_closed_vocabulary_steps() {
        let e = el("e5", "", "image", 0.3, 0.3);
        let mk = |act| Opt {
            key: "k".into(),
            text: "".into(),
            act,
            publish: false,
        };
        let pair = |o: &Opt| {
            let s = steps_for(o);
            assert_eq!(s.len(), 1);
            (s[0].action.clone(), s[0].arg.clone())
        };
        assert_eq!(
            pair(&mk(Act::TapIcon {
                el: e.clone(),
                name: Some("Add photo".into())
            })),
            ("tap_by_intent".into(), "Add photo".into())
        );
        assert!(steps_for(&mk(Act::TapIcon { el: e, name: None })).is_empty());
        assert_eq!(
            pair(&mk(Act::Attach {
                param: "video".into()
            })),
            ("pick_file".into(), "video".into())
        );
        assert_eq!(
            pair(&mk(Act::Scroll { down: true })),
            ("scroll".into(), "down".into())
        );
        assert_eq!(
            pair(&mk(Act::Scroll { down: false })),
            ("scroll".into(), "up".into())
        );
        assert_eq!(pair(&mk(Act::Wait)), ("wait".into(), "2".into()));
        assert!(steps_for(&mk(Act::Done)).is_empty());
        assert!(steps_for(&mk(Act::Stuck)).is_empty());
    }
}
