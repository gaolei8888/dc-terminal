//! 一盘游戏怎么玩：读盘 → 选步 → 划 → 等画面停下 → 下一步，以及什么时候停。
//! 跟 dco 说话（`Dco`）和计时（`Clock`）都是传进来的，所以测试里换成假的，不碰真机也不真睡觉。
use crate::ask::{board_text, describe, describe_move, Advisor, AskInput};
use crate::board::{fixed_ids, looks_like_board, Board, GridRead};
use crate::choose::{choose, Candidate, Weights};
use crate::goal::GoalFinder;
use crate::mood::{self, Mood};
use crate::screen::Element;
use serde_json::{json, Value};

#[derive(Clone, Debug)]
pub struct Profile {
    pub window: Value,
    pub region: [f64; 4], // x, y, w, h
    pub rows: usize,
    pub cols: usize,
    pub extra: Value, // inset / class_de / top_div / odd_share：原样带给 read_grid
    /// 「不是糖」的那几类（洞、蜂蜜块、糖果机……）的颜色；读到的类别颜色离其中之一不超过 match_de 就当它是。
    /// 空 = 没有这种格子（第一轮的行为）。这份数据来自用户每关的配置文件，代码里不写死任何游戏的颜色。
    pub fixed_rgb: Vec<[u8; 3]>,
    pub match_de: f64,
    /// 打分权重，来自配置文件的 `[weights]`；不写就是通用的中性默认（特殊糖全 0）。
    pub weights: Weights,
    /// 关卡号在画面上的写法（一个带恰好一个捕获组的正则），来自配置文件的 `level_pattern`。
    /// 有它、画面上对得上、又读得出合格棋盘，就直接当棋盘，不过 OCR 分类。没写 = 不启用。
    pub level_pattern: Option<String>,
    /// 章鱼的主题（配置文件的可选键 `theme`），开局发一次；没写就不发。
    pub theme: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct DcoError {
    pub code: String,
    pub message: String,
}

/// 一次 `see`（OCR）：画面上读到的字和它们的编号。`snapshot_id` 要原样交给 `tap`。
#[derive(Clone, Debug)]
pub struct Seen {
    pub snapshot_id: String,
    pub observation_id: Option<String>,
    pub elements: Vec<Element>,
}

/// 划完之后让 dco 等屏幕稳定：连续安静这么久算稳定、最多等这么久（毫秒）。
pub const SETTLE_QUIET_MS: u32 = 300;
pub const SETTLE_TIMEOUT_MS: u32 = 8000;

/// dco 替我们等屏幕稳定之后的报告。
#[derive(Clone, Debug, PartialEq)]
pub struct SwipeSettle {
    pub changed: bool,
    pub change: f64,
    pub settled: bool,
    pub timed_out: bool,
    pub settled_ms: Option<u64>,
}

/// `swipe_settle` 的结果。要分清「划动没发生」和「划了但没拿到稳定报告」：后一种绝不能再划一次。
#[derive(Clone, Debug, PartialEq)]
pub enum SwipeOutcome {
    NotSwiped(DcoError),
    SwipedNoSettle(DcoError),
    Settled(SwipeSettle),
}

/// 窗口里的一块（万分比），不点的区域。dct-game 自己的，不依赖 dct-brain。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Region {
    pub x_bp: u16,
    pub y_bp: u16,
    pub w_bp: u16,
    pub h_bp: u16,
}

/// 一次按位置点（`tap_at`）的结果：落点下 dco 看到的是 `no_text` 还是 `text`，以及那条字。
#[derive(Clone, Debug, PartialEq)]
pub struct TapAt {
    pub kind: String,
    pub text: String,
    /// dco 点完等屏幕稳定后的报告；旧 dco 不认 `settle` 参数就没有。
    pub settle: Option<TapSettle>,
}

/// 点完以后的稳定报告，和划动的一样。
pub type TapSettle = SwipeSettle;

/// 要 dco 点完替我们等屏幕稳定：安静多久算稳、最多等多久、只看窗口里的哪一块（比例 `[x, y, w, h]`）。
#[derive(Clone, Debug, PartialEq)]
pub struct TapSettleReq {
    pub quiet_ms: u32,
    pub timeout_ms: u32,
    pub region: [f64; 4],
}

/// `tap_at` 一次最多带几块「不点」的区域；多了截断。
pub const TAP_AT_MAX_AVOID: usize = 16;

fn unsupported() -> DcoError {
    DcoError { code: "unsupported".into(), message: "这个 dco 不会认画面上的字".into() }
}

/// 画面上整条文字就是一个数字的元素（顶部的目标数、步数……），按出现顺序。
/// 「1716/♥5」这种夹着别的字的不要。哪个数是目标，这里不猜；只记下来。
pub(crate) fn numbers(seen: &Seen) -> Vec<u64> {
    seen.elements
        .iter()
        .filter_map(|e| {
            let t = e.text.trim();
            if t.is_empty() || !t.bytes().all(|b| b.is_ascii_digit()) {
                return None;
            }
            t.parse::<u64>().ok()
        })
        .collect()
}

/// 停滞计数的一次更新；`gi` 是当前生效的目标下标（用户给的优先，否则识别器认出的），没有就不动。
fn update_stall(no_progress: &mut usize, gi: Option<usize>, before: &Option<Vec<u64>>, after: &Option<Vec<u64>>) {
    if let (Some(gi), Some(b), Some(a)) = (gi, before, after) {
        match goal_dropped(b, a, gi) {
            Some(true) => *no_progress = 0,
            Some(false) => *no_progress += 1,
            None => {}
        }
    }
}

/// 连着这么多步目标数字都没下降，就算停滞。
pub const STALL_STEPS: usize = 6;

/// 目标那个数字下降了才算「下降」。两张清单个数不同（OCR 漏读了一个）、或者序号越界，都算「不知道」，返回 `None`。
pub(crate) fn goal_dropped(before: &[u64], after: &[u64], goal: usize) -> Option<bool> {
    if before.len() != after.len() || goal >= before.len() {
        return None;
    }
    Some(after[goal] < before[goal])
}

/// 读屏幕上的数字。`Err(())` 只有一种：dco 说这是私人画面（不读、不操作）；别的错误都当「读不到」（`Ok(None)`）。
fn read_numbers(dco: &mut dyn Dco, p: &Profile) -> Result<Option<Vec<u64>>, ()> {
    match dco.see_text(p) {
        Ok(s) => Ok(Some(numbers(&s))),
        Err(e) if e.code == "private_screen" => Err(()),
        Err(_) => Ok(None),
    }
}

pub trait Dco {
    fn read_grid(&mut self, p: &Profile) -> Result<GridRead, DcoError>;
    /// 坐标是窗口比例（0～1）。
    fn swipe(&mut self, p: &Profile, from: (f64, f64), to: (f64, f64)) -> Result<(), DcoError>;
    /// 划一下，并让 dco 等屏幕稳定后报告有没有变。默认实现只会划（划动确实发生了），
    /// 然后说「没有稳定报告」，调用方据此回退轮询、不会再划。
    fn swipe_settle(&mut self, p: &Profile, from: (f64, f64), to: (f64, f64)) -> SwipeOutcome {
        match self.swipe(p, from, to) {
            Err(e) => SwipeOutcome::NotSwiped(e),
            Ok(()) => SwipeOutcome::SwipedNoSettle(unsupported()),
        }
    }
    /// 读画面上的字。只有“自动开始下一局”要用；默认不支持，所以只玩一关的假 dco 不用实现。
    fn see_text(&mut self, _p: &Profile) -> Result<Seen, DcoError> {
        Err(unsupported())
    }
    /// 点 `see_text` 读到的某个元素。dco 自己按那个元素上的字定档，带价格的会拒绝。
    fn tap(&mut self, _snapshot_id: &str, _element_id: &str) -> Result<(), DcoError> {
        Err(unsupported())
    }
    /// 取当前窗口的截图（PNG 字节）。调用方必须先用 `see_text` 确认这是游戏画面：私人画面 dco 本来就不给图。
    fn capture(&mut self, _p: &Profile) -> Result<Vec<u8>, DcoError> {
        Err(unsupported())
    }
    /// 按窗口里的位置（万分比）点一下；`avoid` 里的区域 dco 会拒点。默认不支持（旧 dco 没有 `tap_at`）。
    fn tap_at(&mut self, _p: &Profile, _x_bp: u16, _y_bp: u16, _avoid: &[Region], _settle: Option<&TapSettleReq>) -> Result<TapAt, DcoError> {
        Err(unsupported())
    }
    /// 告诉 dco 屏幕上的小章鱼现在在“想”还是“看”。只改它的样子，所以故意不返回错误：
    /// 这里出什么事都不许影响玩。默认什么都不做，只玩一关的假 dco 不用实现。
    fn show_status(&mut self, _state: &str) {}
    /// 同 `show_status`，另带一条不超过 16 个字的说明和一个主题。默认转调 `show_status`（忽略 text/theme），
    /// 所以不关心这些的假 dco 不用改。
    fn show_status_with(&mut self, state: &str, _text: Option<&str>, _theme: Option<&str>) {
        self.show_status(state);
    }
}

pub trait Clock {
    fn now_ms(&self) -> u64;
    fn sleep_ms(&mut self, ms: u64);
}

/// 最多发给模型几个候选。
pub const ASK_SHOWN: usize = 8;
/// 连着最多几次「划了没反应」才停。被拒绝的格子在有一步成功前不再被选（除非别的步都被它们挡住了），所以多试几次不会重复同一处。
pub const MAX_REFUSED_IN_A_ROW: usize = 5;

pub struct Options<'a> {
    pub max_steps: usize,
    pub dry_run: bool,
    /// 规则没把握时问的那个。`None` = 不问（和以前一样）。
    pub advisor: Option<&'a dyn Advisor>,
    /// 每一步都问（`--ask-every-step`）；否则只在这盘棋上已有失败的步时才问。
    pub ask_always: bool,
    /// 这一次 `play()` 最多问几次。
    pub ask_budget: usize,
    pub goal: &'a str,
    /// 目标数字是屏幕上第几个数字（从 0 数）；`None` = 不知道，不判断停滞。
    pub goal_index: Option<usize>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Stop {
    /// 读不出棋盘（含：类别太多，多半是弹了窗）。
    NoGrid(String),
    /// 类别比开始时多出两个以上：多半这关结束了、弹了窗。
    ClassesChanged { was: usize, now: usize },
    NoMoves,
    /// 连着两次划了没反应。
    Stuck,
    /// 8 秒了画面还在变。
    StillMoving,
    StepsDone,
    DryRun,
    /// dco 急停 / 暂停 / 锁屏 / 别的错误。
    Dco(DcoError),
    /// dco 认出这是私人画面（微信、相册……）不给读：一步都不划，也不碰。
    PrivateScreen,
    // ---- 下面几个只有 `--auto-next` 才会出现（navigate.rs）----
    /// 通关了：下一关版面不一样，先停。
    Won,
    /// 一局结束了，但看不出是通关还是没过，也没有明确的“再来一次”：先停，不乱点。
    LevelEnded,
    LivesOut,
    /// 出现了要花钱的画面，又没有安全的关闭按钮。
    Money,
    Ad,
    /// 不认识的画面；带着画面上读到的字。
    UnknownScreen(Vec<String>),
    /// 点了按钮，画面连着两次没变化。
    NoEffect,
    /// 点“开始 / 再来一次”的次数到上限。
    TriesDone,
    /// 总共点了太多次还没回到棋盘。
    TapLimit,
}

pub struct Summary {
    pub steps: usize,
    pub stop: Stop,
}

enum Settle {
    Settled(GridRead),
    NoChange(GridRead),
    StillMoving,
    /// 划了以后一张读得出的棋盘都没见到（多半是结果页）。
    NoGrid(String),
    Failed(DcoError),
}

/// 按行优先第一次出现的顺序重新编号。dco 的类别号只在一次返回里有意义，
/// 同一张没动的画面两次读可能分组相同、号码互换，所以跨读比较只能比分组。
pub(crate) fn canonical(cells: &[Vec<u16>]) -> Vec<Vec<u16>> {
    let mut map: std::collections::HashMap<u16, u16> = std::collections::HashMap::new();
    cells
        .iter()
        .map(|r| {
            r.iter()
                .map(|c| {
                    let n = map.len() as u16;
                    *map.entry(*c).or_insert(n)
                })
                .collect()
        })
        .collect()
}

/// 只比较分组。`odd`（特殊糖标记）会跟着游戏的提示光晕和掉落动画闪，
/// 不能算成「棋盘变了」，否则没效果的一步会被当成有效，停不下来。
fn same(a: &GridRead, b: &GridRead) -> bool {
    canonical(&a.cells) == canonical(&b.cells)
}

/// 同一格划前划后的颜色相差不到这个就当没变（dco 的类别号每次读都会变，所以比颜色，不比编号）。
const COLOUR_SAME_DE: f64 = 12.0;

fn rgb_of(g: &GridRead, id: u16) -> Option<[u8; 3]> {
    g.classes.iter().find(|c| c.id == id).and_then(|c| c.rgb)
}

/// 划前划后有几格颜色变了。行列数不同或任何一格拿不到颜色就是 `None`（不能当 0）。
pub(crate) fn changed_cells(a: &GridRead, b: &GridRead) -> Option<usize> {
    if a.rows != b.rows || a.cols != b.cols {
        return None;
    }
    let mut n = 0;
    for (ra, rb) in a.cells.iter().zip(&b.cells) {
        for (ca, cb) in ra.iter().zip(rb) {
            let (x, y) = (rgb_of(a, *ca)?, rgb_of(b, *cb)?);
            if crate::lab::delta_e(x, y) > COLOUR_SAME_DE {
                n += 1;
            }
        }
    }
    Some(n)
}

const FIRST_WAIT_MS: u64 = 150;
const POLL_MS: u64 = 100;
const NO_CHANGE_MS: u64 = 3_000;
const GIVE_UP_MS: u64 = 8_000;
/// “没步可走”“画面变了”下结论前的停顿：出口还在一个个往下补糖，特效也会让颜色数暂时跳高，
/// 马上读到的往往是中间状态。
const CONFIRM_PAUSE_MS: u64 = 1_500;
/// 连着最多确认几次“没步可走”：盘面一直在变却始终没有步，也不能无限等。
const MAX_NO_MOVE_CONFIRMS: usize = 2;

fn big_classes(g: &GridRead) -> usize {
    g.classes.iter().filter(|c| c.count >= 2).count()
}

fn has_move(p: &Profile, g: &GridRead) -> bool {
    Board::from_read_fixed(g, &fixed_ids(g, &p.fixed_rgb, p.match_de), !p.fixed_rgb.is_empty()).is_ok_and(|b| !choose(&b, &p.weights).is_empty())
}

fn settle(dco: &mut dyn Dco, clock: &mut dyn Clock, p: &Profile, before: &GridRead) -> Settle {
    let t0 = clock.now_ms();
    clock.sleep_ms(FIRST_WAIT_MS);
    let mut prev: Option<GridRead> = None;
    let mut changed = false;
    let mut last_unreadable: Option<String> = None;
    let mut readable = false;
    loop {
        let waited = clock.now_ms().saturating_sub(t0);
        match dco.read_grid(p) {
            Ok(g) => {
                readable = true;
                if !same(&g, before) {
                    changed = true;
                }
                if changed {
                    if prev.as_ref().is_some_and(|x| same(x, &g)) {
                        return Settle::Settled(g);
                    }
                } else if waited >= NO_CHANGE_MS {
                    return Settle::NoChange(g);
                }
                prev = Some(g);
            }
            // 动画中间读不出来是常事：接着等，等到时间到。别的错误（急停、锁屏）马上停。
            Err(e) if e.code == "not_a_grid" => {
                last_unreadable = Some(e.message);
                prev = None;
            }
            Err(e) => return Settle::Failed(e),
        }
        if waited >= GIVE_UP_MS {
            return match last_unreadable {
                Some(m) if !readable => Settle::NoGrid(m),
                _ => Settle::StillMoving,
            };
        }
        clock.sleep_ms(POLL_MS);
    }
}

fn centre(p: &Profile, (r, c): (usize, usize)) -> (f64, f64) {
    let [x, y, w, h] = p.region;
    (x + (c as f64 + 0.5) / p.cols as f64 * w, y + (r as f64 + 0.5) / p.rows as f64 * h)
}

/// 章鱼只在表情变化时才发：同一个连着不重发；没有合适的表情就回到「看」（已经是「看」就不发）。
fn send_mood(dco: &mut dyn Dco, last: &mut &'static str, m: Option<Mood>) {
    match m {
        Some(m) if m.state != *last => {
            dco.show_status_with(m.state, m.text, None);
            *last = m.state;
        }
        Some(_) => {}
        None if *last != "look" => {
            dco.show_status_with("look", None, None);
            *last = "look";
        }
        None => {}
    }
}

pub fn play(dco: &mut dyn Dco, clock: &mut dyn Clock, p: &Profile, o: &Options<'_>, sink: &mut dyn FnMut(Value)) -> Summary {
    let s = play_inner(dco, clock, p, o, sink);
    if let Some(m) = mood::for_stop(&s.stop) {
        dco.show_status_with(m.state, m.text, None);
    }
    s
}

/// 同 `play`，但停下时不发章鱼表情：`auto_next` 里一局结束只是中途，由它自己在真正停下时发。
pub(crate) fn play_inner(dco: &mut dyn Dco, clock: &mut dyn Clock, p: &Profile, o: &Options<'_>, sink: &mut dyn FnMut(Value)) -> Summary {
    let mut steps = 0;
    // 章鱼现在的样子（开局算「看」）；`hold_calm`：刚说了「大模型没回应」，别马上被「看」盖掉。
    let mut last_mood: &'static str = "look";
    let mut hold_calm = false;
    let mut told_advisor_down = false;
    if !o.dry_run {
        if let Some(t) = p.theme.as_deref() {
            dco.show_status_with("look", None, Some(t));
        }
    }
    let mut baseline: Option<usize> = None;
    let mut failed: Vec<(crate::sim::Move, Vec<Vec<u16>>)> = Vec::new();
    let mut asked = 0usize;
    let mut streak = 0;
    let mut locked: Vec<(usize, usize)> = Vec::new();
    let mut current: Option<GridRead> = None;
    let mut no_move_confirms = 0;
    let mut no_progress = 0usize;
    let mut finder = GoalFinder::new();
    let mut moved_seen = 0usize;
    let (mut progress_prev, private_at_start) = if o.dry_run {
        (None, false)
    } else {
        match read_numbers(dco, p) {
            Ok(v) => (v, false),
            Err(()) => (None, true),
        }
    };
    let stop = loop {
        if private_at_start {
            break Stop::PrivateScreen;
        }
        if steps >= o.max_steps {
            break Stop::StepsDone;
        }
        let t_read = clock.now_ms();
        let g = match current.take() {
            Some(g) => g,
            None => match dco.read_grid(p) {
                Ok(g) => g,
                Err(e) if e.code == "not_a_grid" => break Stop::NoGrid(e.message),
                Err(e) => break Stop::Dco(e),
            },
        };
        let read_ms = clock.now_ms().saturating_sub(t_read);
        // 这一轮的第一张盘：大半是同一种平色的是弹窗不是棋盘，一步都不划。后面的读数不查（后期的盘可以很偏）。
        if baseline.is_none() && !looks_like_board(&g) {
            break Stop::NoGrid("这块区域看着不像棋盘".into());
        }
        // 只数至少两格的类别：一格的（彩色炸弹、条纹糖被读成自己的颜色）是“不认识”，忽略
        let big = big_classes(&g);
        let base = *baseline.get_or_insert(big);
        if big > base + 1 {
            if o.dry_run {
                break Stop::ClassesChanged { was: base, now: big };
            }
            // 特效清完一大片，颜色数会暂时跳高：停一下再读一遍，还高才算画面真的变了。
            clock.sleep_ms(CONFIRM_PAUSE_MS);
            match dco.read_grid(p) {
                Ok(re) => {
                    let re_big = big_classes(&re);
                    if re_big > base + 1 {
                        break Stop::ClassesChanged { was: base, now: re_big };
                    }
                    current = Some(re);
                    continue;
                }
                Err(e) if e.code == "not_a_grid" => break Stop::NoGrid(e.message),
                Err(e) => break Stop::Dco(e),
            }
        }
        let Ok(board) = Board::from_read_fixed(&g, &fixed_ids(&g, &p.fixed_rgb, p.match_de), !p.fixed_rgb.is_empty()) else {
            break Stop::NoGrid("棋盘的行列数对不上".into());
        };
        // 规则选步约 1 ms，不发 think/look：章鱼没法“想”这么短，停住的状态反而拖慢动画。
        // 只在问模型（慢）的那段时间发 think/look。
        let t_choose = clock.now_ms();
        let cands: Vec<Candidate> = choose(&board, &p.weights);
        let choose_ms = clock.now_ms().saturating_sub(t_choose);
        // 同一盘棋上划了没反应的步，不再重复选。
        let board_key = canonical(&g.cells);
        let blocked = |c: &Candidate| {
            failed.iter().any(|(m, cells)| *m == c.mv && *cells == board_key) || locked.contains(&c.mv.a) || locked.contains(&c.mv.b)
        };
        let pick = cands.iter().position(|c| !blocked(c)).or_else(|| {
            // 只是被「锁」挡住的（别的步碰到了划过没反应的格子），不能因此停下：
            // 退一步，选第一个在这张盘上没被原样拒绝过的步，不看锁。次数仍由 MAX_REFUSED_IN_A_ROW 管着。
            cands.iter().position(|c| !failed.iter().any(|(m, cells)| *m == c.mv && *cells == board_key))
        });
        if cands.is_empty() && !o.dry_run && no_move_confirms < MAX_NO_MOVE_CONFIRMS {
            // 补糖还没补完时会有一瞬间没步可走：停一下再读，盘面变了或有步了就接着玩。
            no_move_confirms += 1;
            clock.sleep_ms(CONFIRM_PAUSE_MS);
            match dco.read_grid(p) {
                Ok(re) => {
                    if same(&re, &g) && !has_move(p, &re) {
                        break Stop::NoMoves;
                    }
                    current = Some(re);
                    continue;
                }
                Err(e) if e.code == "not_a_grid" => break Stop::NoGrid(e.message),
                Err(e) => break Stop::Dco(e),
            }
        }
        let Some(pick) = pick else {
            break if cands.is_empty() { Stop::NoMoves } else { Stop::Stuck };
        };
        let mut pick = pick;
        let mut ask_rec: Option<Value> = None;
        let mut decider = "rules";
        let stalled_now = no_progress >= STALL_STEPS;
        if let Some(adv) = o.advisor {
            // 发给模型的候选：没失败过的，最多 ASK_SHOWN 个；下标指回 cands
            let offered: Vec<usize> = {
                (0..cands.len()).filter(|&i| !blocked(&cands[i])).take(ASK_SHOWN).collect()
            };
            let had_failed_here = failed.iter().any(|(_, cells)| *cells == board_key);
            if !o.dry_run && adv.available() && asked < o.ask_budget && offered.len() >= 2 && (o.ask_always || had_failed_here || stalled_now) {
                asked += 1;
                // 只因为卡住才问的：问过一次就重新数，别每一步都问。
                if !o.ask_always && !had_failed_here {
                    no_progress = 0;
                }
                let fixed = fixed_ids(&g, &p.fixed_rgb, p.match_de);
                let failed_now: Vec<String> = failed.iter().filter(|(_, cells)| *cells == board_key).map(|(m, _)| describe_move(m)).collect();
                let input = AskInput {
                    board: board_text(&g, &fixed),
                    goal: o.goal.to_string(),
                    candidates: offered.iter().map(|&i| describe(&cands[i])).collect(),
                    failed: failed_now.clone(),
                };
                dco.show_status_with("think", Some("问大模型中"), None);
                let advice = adv.pick(&input);
                if advice.is_none() && !told_advisor_down {
                    told_advisor_down = true;
                    hold_calm = true;
                    dco.show_status_with("stall", Some("大模型没回应"), None);
                    last_mood = "stall";
                } else {
                    dco.show_status_with("look", None, None);
                    last_mood = "look";
                }
                if let Some(a) = advice {
                    let chosen_idx = a.choice.filter(|&k| k < offered.len()).map(|k| offered[k]);
                    ask_rec = Some(json!({
                        "model": a.model, "raw": a.raw, "reason": a.reason,
                        "tokens_in": a.tokens.map(|t| t.0), "tokens_out": a.tokens.map(|t| t.1),
                        "asked": offered, "choice": chosen_idx,
                        "failed": failed_now, "goal": o.goal,
                    }));
                    if let Some(i) = chosen_idx {
                        pick = i;
                        decider = "model";
                    }
                }
            }
        }
        let chosen = &cands[pick];
        let mut rec = json!({
            "schema": 1, "time_ms": clock.now_ms(),
            "observation_id": g.observation_id, "observed_at_ms": g.observed_at_ms, "frame_age_ms": g.frame_age_ms,
            "rows": g.rows, "cols": g.cols, "cells": g.cells, "odd": g.odd, "classes": g.classes,
            "candidates": cands.iter().map(|c| json!({
                "a": [c.mv.a.0, c.mv.a.1], "b": [c.mv.b.0, c.mv.b.1], "score": c.score,
                "features": {
                    "cleared": c.features.cleared, "cascade": c.features.cascade, "striped": c.features.striped,
                    "wrapped": c.features.wrapped, "bomb": c.features.bomb, "triggered": c.features.triggered,
                    "special_swap": c.features.special_swap, "lowest_row": c.features.lowest_row } })).collect::<Vec<_>>(),
            "chosen": pick, "decider": decider, "dry_run": o.dry_run, "swiped": false,
            "predicted_cleared": chosen.features.cleared + chosen.features.cascade,
        });
        if stalled_now {
            rec["stalled"] = json!(true);
        }
        if let Some(a) = ask_rec {
            rec["ask"] = a;
        }
        if o.dry_run {
            rec["timing_ms"] = json!({ "read": read_ms, "choose": choose_ms, "swipe": 0, "settle": 0 });
            rec["outcome"] = json!("dry_run");
            sink(rec);
            break Stop::DryRun;
        }
        // 端点和起始时刻原样记进记录，章鱼叠加层的事件日志靠它跟 dct 真做的对账（窗口 0–1 比例）。
        let (from, to) = (centre(p, chosen.mv.a), centre(p, chosen.mv.b));
        let r4 = |v: f64| (v * 10000.0).round() / 10000.0;
        let t_swipe = clock.now_ms();
        let outcome = dco.swipe_settle(p, from, to);
        let call_ms = clock.now_ms().saturating_sub(t_swipe);
        let swipe_rec = |ms: u64, includes_settle: bool| {
            let mut v = json!({ "from": [r4(from.0), r4(from.1)], "to": [r4(to.0), r4(to.1)], "started_ms": t_swipe, "duration_ms": ms });
            if includes_settle {
                v["duration_ms_includes_settle"] = json!(true);
            }
            v
        };
        if let SwipeOutcome::NotSwiped(e) = outcome {
            rec["swipe"] = swipe_rec(call_ms, false);
            rec["timing_ms"] = json!({ "read": read_ms, "choose": choose_ms, "swipe": call_ms, "settle": 0 });
            rec["outcome"] = json!("stopped");
            sink(rec);
            break Stop::Dco(e);
        }
        // 划成功了才算走了一步。
        steps += 1;
        no_move_confirms = 0;
        rec["swiped"] = json!(true);
        // dco 带稳定报告：等待发生在 swipe_settle 里面，所以整段调用时长要拆成「划」和「等」；
        // 拆不开时整段记在 swipe 上并打标记。回退到轮询绝不重划。
        let (result, swipe_ms, settle_ms) = match outcome {
            SwipeOutcome::Settled(s) if s.settled && !s.timed_out => {
                rec["settle"] = json!({ "source": "dco", "changed": s.changed, "settled_ms": s.settled_ms });
                let t_read = clock.now_ms();
                let r = if !s.changed {
                    Settle::NoChange(g.clone())
                } else {
                    match dco.read_grid(p) {
                        Ok(next) => Settle::Settled(next),
                        Err(e) if e.code == "not_a_grid" => Settle::NoGrid(e.message),
                        Err(e) => Settle::Failed(e),
                    }
                };
                let read_after = clock.now_ms().saturating_sub(t_read);
                match s.settled_ms {
                    Some(m) => {
                        rec["swipe"] = swipe_rec(call_ms.saturating_sub(m), false);
                        (r, call_ms.saturating_sub(m), m + read_after)
                    }
                    None => {
                        rec["swipe"] = swipe_rec(call_ms, true);
                        (r, call_ms, read_after)
                    }
                }
            }
            other => {
                rec["settle"] = json!({ "source": "poll", "changed": null, "settled_ms": null });
                // 老 dco 的默认实现只划了一下（unsupported），时长是纯划动；其余回退时调用里可能已经含了等待。
                let includes = !matches!(&other, SwipeOutcome::SwipedNoSettle(e) if e.code == "unsupported");
                rec["swipe"] = swipe_rec(call_ms, includes);
                let t = clock.now_ms();
                let r = settle(dco, clock, p, &g);
                (r, call_ms, clock.now_ms().saturating_sub(t))
            }
        };
        rec["timing_ms"] = json!({ "read": read_ms, "choose": choose_ms, "swipe": swipe_ms, "settle": settle_ms });
        match result {
            // 游戏动了一下又弹回原样（被笼子、锁住的糖），棋盘和划之前一样：算没反应：同一盘棋上不再重复选这一步。
            Settle::Settled(next) if same(&next, &g) => {
                rec["outcome"] = json!("no_change");
                rec["observed_changed"] = json!(0);
                rec["after_observation_id"] = json!(next.observation_id);
                let progress_after = match read_numbers(dco, p) {
                    Ok(v) => v,
                    Err(()) => {
                        rec["outcome"] = json!("stopped");
                        sink(rec);
                        break Stop::PrivateScreen;
                    }
                };
                let gi = o.goal_index.or(finder.index());
                update_stall(&mut no_progress, gi, &progress_prev, &progress_after);
                rec["progress"] = json!({ "before": progress_prev, "after": progress_after, "goal_index": gi });
                progress_prev = progress_after;
                sink(rec);
                streak += 1;
                failed.push((chosen.mv, canonical(&g.cells)));
                locked.push(chosen.mv.a);
                locked.push(chosen.mv.b);
                if streak >= MAX_REFUSED_IN_A_ROW {
                    break Stop::Stuck;
                }
                current = Some(next);
            }
            Settle::Settled(next) => {
                rec["outcome"] = json!("moved");
                rec["observed_changed"] = json!(changed_cells(&g, &next));
                rec["after_observation_id"] = json!(next.observation_id);
                let progress_after = match read_numbers(dco, p) {
                    Ok(v) => v,
                    Err(()) => {
                        rec["outcome"] = json!("stopped");
                        sink(rec);
                        break Stop::PrivateScreen;
                    }
                };
                // 只有成功走了一步才让识别器看：被拒绝的步数字不降，会把「每步刚好少 1」的步数误排除。
                // 第一步也不看：开局前读到的数字可能带着关卡开始的弹窗，不是这一步造成的变化。
                moved_seen += 1;
                if o.goal_index.is_none() {
                    let had = finder.index();
                    if let (true, Some(b), Some(a)) = (moved_seen > 1, &progress_prev, &progress_after) {
                        finder.observe(b, a);
                    }
                    if had.is_none() {
                        if let Some(i) = finder.index() {
                            rec["goal_found"] = json!(i + 1);
                        }
                    }
                }
                let gi = o.goal_index.or(finder.index());
                update_stall(&mut no_progress, gi, &progress_prev, &progress_after);
                rec["progress"] = json!({ "before": progress_prev, "after": progress_after, "goal_index": gi });
                progress_prev = progress_after;
                sink(rec);
                streak = 0;
                failed.clear();
                locked.clear();
                current = Some(next);
            }
            Settle::NoChange(same_board) => {
                rec["outcome"] = json!("no_change");
                rec["observed_changed"] = json!(0);
                rec["after_observation_id"] = json!(same_board.observation_id);
                let progress_after = match read_numbers(dco, p) {
                    Ok(v) => v,
                    Err(()) => {
                        rec["outcome"] = json!("stopped");
                        sink(rec);
                        break Stop::PrivateScreen;
                    }
                };
                let gi = o.goal_index.or(finder.index());
                update_stall(&mut no_progress, gi, &progress_prev, &progress_after);
                rec["progress"] = json!({ "before": progress_prev, "after": progress_after, "goal_index": gi });
                progress_prev = progress_after;
                sink(rec);
                streak += 1;
                failed.push((chosen.mv, canonical(&g.cells)));
                locked.push(chosen.mv.a);
                locked.push(chosen.mv.b);
                if streak >= MAX_REFUSED_IN_A_ROW {
                    break Stop::Stuck;
                }
                current = Some(same_board);
            }
            Settle::StillMoving => {
                rec["outcome"] = json!("stopped");
                sink(rec);
                break Stop::StillMoving;
            }
            Settle::NoGrid(why) => {
                rec["outcome"] = json!("stopped");
                sink(rec);
                break Stop::NoGrid(why);
            }
            Settle::Failed(e) => {
                rec["outcome"] = json!("stopped");
                sink(rec);
                break Stop::Dco(e);
            }
        }
        let gi = o.goal_index.or(finder.index());
        let at = |i: Option<usize>| i.and_then(|i| progress_prev.as_ref()?.get(i).copied());
        let m = mood::for_step(&chosen.features, stalled_now, at(finder.step_index()), at(gi), finder.goal_start());
        if m.is_some() {
            hold_calm = false;
        }
        if !(m.is_none() && hold_calm) {
            send_mood(dco, &mut last_mood, m);
        }
    };
    Summary { steps, stop }
}
