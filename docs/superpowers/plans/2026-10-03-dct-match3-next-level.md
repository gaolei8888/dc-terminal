# dct 玩三消（第二轮 · 第一步）Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** `dct game play --auto-next`：一局结束后自己接着来——失败了点「Try again」「Play」重来同一关；弹窗里只点白名单里的字；认出生命用完、价格、广告、通关、不认识的画面就停，什么都不点；每个不认识的画面把画面上的字记进记录。

**Architecture:** `crates/dct-game` 加三块：`screen.rs`（纯函数：OCR 元素 → 画面分类）、`navigate.rs`（`auto_next` 循环，外面套着第一轮的 `play`）、`Dco` trait 加 `see_text` / `tap`（带默认实现，第一轮的假 dco 不用改）。主 crate 的 `src/game/text.rs`、`cli.rs` 加 `--auto-next`、`--tries` 和大白话；说明卡 `skill.md` 加这一段。

**Tech Stack:** Rust、serde_json（都已有）。没有新依赖。

**Spec:** `docs/superpowers/specs/2026-10-03-dct-match3-next-level-design.md`（读它再读本计划）；第一轮：`docs/superpowers/specs/2026-10-02-dct-match3-play-design.md`；dco 接口见 dc-octo 的 `src/tier.rs`、`src/tools/act.rs`。

## Global Constraints

- 一律用中文给用户说话；不出现 snapshot / element / OCR / tap 这类词（项目 CLAUDE.md：用户不是程序员）。
- 没有 C 依赖，不新增依赖 crate。Windows 也要能编：`dco.rs`、`dco_tests.rs` 带 `#[cfg(unix)]`；`screen.rs`、`navigate.rs` 是纯逻辑，不加 cfg。
- 提交信息用英文，**不加任何 AI 署名行**（用户的规矩）。
- 测试**绝不能写用户真实的 `~/.dct`、`~/.claude`、`~/.codex`、`~/.qwen`、`~/.dco`**，一律临时目录；不要连真 dco。
- 判断放在 dct、不放 dco：dco 这一步一行都不改；dco 自己按按钮上的字定档，是第二道保险。
- 白名单只放整条文字**正好是**这些词的按钮：Close / No thanks / Not now / Later / Maybe later / Skip / Cancel / Got it / Tap to continue / Next / 关闭 / 以后再说 / 暂不 / 跳过 / 取消 / 知道了 / 下一步；Play / Start / 开始 / 开始游戏；Try again / Retry / 再试一次 / 重试。**不放** Continue / OK / Collect / Claim / Save My Progress。
- 判断顺序（前面的优先）：生命用完 → 通关 → 安全的关闭 → 价格 → 广告 → 再来一次 → 开始（恰好一个）→ 不认识。
- 刚玩完一局后，除非看到明确的「再来一次」，**不点 Play**（那可能是通关后的下一关，版面不同，会乱走烧生命）；这条保险在关掉弹窗之后仍然有效。
- 常量：点完等 400 ms 再看，之后每 300 ms 看一次，8 秒不变记没反应，连着两次没反应停；整个命令最多点 40 次；`--tries` 默认 5、范围 1～20；`--steps` 在 `--auto-next` 下是整个命令一共最多走的步数。
- 用户 2026-10-03 定：允许用掉开局框里默认选上的道具；生命用完就停下来告诉他。

## Review Focus

每一条都落在下面某个任务的测试里：

1. 刚玩完一局后看到 Play 不能点；中间关掉了几个弹窗，这条保险还在（任务 3）。
2. 画面上有价格时：有整条正好是 No thanks 的按钮就点它，没有就停；价格画面里的 Play 绝不点（任务 1、3）。
3. 两个都正好是 Play 的按钮（开局框里粉色和紫色「看广告」）→ 停，不猜（任务 1）。
4. 一个点了没反应的按钮、两个互相弹来弹去的弹窗，都必须停下，不能无限点（任务 3）。
5. 钱、广告、生命用完、通关、不认识的画面：一次 tap 都不发（任务 3、5）。
6. 不带 `--auto-next` 时一次 see、tap 都不发，行为和第一轮一样（任务 5）。
7. dco 拒绝了一次点击（比如「需要执行票」）→ 把原因交出来，不重试（任务 3）。
8. 给用户的话：不认识的画面最多列 8 段字；没点任何东西的停下要明说「没有点任何东西」（任务 4）。

---

## File Structure

| 文件 | 职责 |
|---|---|
| `crates/dct-game/src/screen.rs` | `Element`、`Screen`、`classify`：OCR 元素 → 画面分类（纯函数） |
| `crates/dct-game/src/play.rs` | 改：`Seen`、`Dco::see_text/tap`（默认不支持）、`Stop` 加 9 个新原因 |
| `crates/dct-game/src/dco.rs`、`dco_tests.rs` | 改：真 dco 的 `see_text`（`see` + `source: ocr`）和 `tap` |
| `crates/dct-game/src/navigate.rs` | `auto_next` 循环 + 假世界测试 |
| `src/game/text.rs` | 改（任务 2）：新停下原因的话和退出码、`nav_line` |
| `src/game/cli.rs` | 改：`--auto-next`、`--tries`，nav 记录的输出 |
| `src/game/skill.md` | 改：说明卡加 `--auto-next` 一段 |
| `tests/game_cli.rs` | 改：假 dco 加 see / tap，5 个新集成测试 |
| `README.md`、`README.zh-CN.md` | 加一小段说明 |

---

### Task 1: 认画面（纯函数）

**Files:**
- Create: `crates/dct-game/src/screen.rs`
- Modify: `crates/dct-game/src/lib.rs`

**Interfaces:**
- Produces: `screen::Element { id: String, text: String }`；`screen::Screen::{LivesOut, Won, Dismiss(Element), Money, Ad, Retry(Element), PlayButton(Element), Unknown}`；`screen::classify(&[Element]) -> Screen`。

- [ ] **Step 1: 在 `crates/dct-game/src/lib.rs` 里 `pub mod play;` 下面加一行 `pub mod screen;`**


- [ ] **创建 `crates/dct-game/src/screen.rs`**（下面的代码已在临时副本里编译、跑过测试）

```rust
//! 认画面：一次 `see`（OCR）读到的元素 → 这是什么画面、该点哪个。纯函数，不碰屏幕。
//! 分类的顺序和白名单见设计 docs/superpowers/specs/2026-10-03-dct-match3-next-level-design.md。

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Element {
    pub id: String,
    pub text: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Screen {
    /// 生命用完了。
    LivesOut,
    /// 通关结算。
    Won,
    /// 有个整条文字正好是“关闭”这一类词的按钮：点它。
    Dismiss(Element),
    /// 画面上有价格，又没有安全的关闭按钮。
    Money,
    /// 广告（右上角的叉叉没有字，dco 不让点）。
    Ad,
    /// 有个整条文字正好是“再来一次”的按钮。
    Retry(Element),
    /// 恰好一个整条文字正好是“开始”的按钮。
    PlayButton(Element),
    Unknown,
}

const DISMISS: &[&str] = &[
    "close", "no thanks", "not now", "later", "maybe later", "skip", "cancel", "got it", "tap to continue", "next", "关闭", "以后再说", "暂不", "跳过",
    "取消", "知道了", "下一步",
];
const RETRY: &[&str] = &["try again", "retry", "再试一次", "重试"];
const PLAY: &[&str] = &["play", "start", "开始", "开始游戏"];
const MONEY_MARKS: &[&str] = &["¥", "￥", "$", "€", "£", "💎", "金币", "钻石", "购买", "支付", "充值"];
const MONEY_WORDS: &[&str] = &["buy", "purchase", "pay", "checkout", "gold", "gem", "gems", "bar", "bars"];
const WON_PHRASES: &[&str] = &["level complete", "level completed", "level cleared", "通关"];

/// 小写、合并空白、去掉两头的标点和符号（OCR 常把叉叉、箭头读成一个字符粘在旁边）。
fn norm(s: &str) -> String {
    let lower = s.to_lowercase();
    let joined = lower.split_whitespace().collect::<Vec<_>>().join(" ");
    joined.trim_matches(|c: char| !c.is_alphanumeric() && !is_cjk(c)).to_string()
}

fn is_cjk(c: char) -> bool {
    ('\u{4e00}'..='\u{9fff}').contains(&c)
}

fn words(s: &str) -> Vec<String> {
    s.to_lowercase().split(|c: char| !c.is_ascii_alphanumeric()).filter(|w| !w.is_empty()).map(str::to_string).collect()
}

fn exact<'a>(els: &'a [Element], list: &[&str]) -> Vec<&'a Element> {
    els.iter().filter(|e| list.contains(&norm(&e.text).as_str())).collect()
}

fn has_money(els: &[Element]) -> bool {
    els.iter().any(|e| {
        let t = e.text.to_lowercase();
        MONEY_MARKS.iter().any(|m| t.contains(m)) || words(&t).iter().any(|w| MONEY_WORDS.contains(&w.as_str()))
    })
}

fn has_ad(els: &[Element]) -> bool {
    els.iter().any(|e| words(&e.text).iter().any(|w| w == "ad" || w == "ads" || w == "advert" || w == "advertisement"))
}

fn lives_out(all: &str) -> bool {
    let w = words(all);
    let lives = w.iter().any(|x| x == "lives" || x == "life");
    let out = ["no more", "out of", "ask friends", "get more", "0 lives"].iter().any(|p| all.contains(p));
    (lives && out) || (all.contains("生命") && ["用完", "不足", "没有了"].iter().any(|p| all.contains(p)))
}

/// 顺序就是优先级：生命用完 → 通关 → 安全的关闭 → 价格 → 广告 → 再来一次 → 开始 → 不认识。
/// 关闭排在价格前面：一个带价格的失败弹窗里有正好是 “No thanks” 的按钮，点它是安全的。
pub fn classify(els: &[Element]) -> Screen {
    let all = els.iter().map(|e| e.text.to_lowercase()).collect::<Vec<_>>().join(" \n ");
    if lives_out(&all) {
        return Screen::LivesOut;
    }
    if WON_PHRASES.iter().any(|p| all.contains(p)) {
        return Screen::Won;
    }
    if let Some(e) = exact(els, DISMISS).first() {
        return Screen::Dismiss((*e).clone());
    }
    if has_money(els) {
        return Screen::Money;
    }
    if has_ad(els) {
        return Screen::Ad;
    }
    if let Some(e) = exact(els, RETRY).first() {
        return Screen::Retry((*e).clone());
    }
    let plays = exact(els, PLAY);
    if plays.len() == 1 {
        return Screen::PlayButton(plays[0].clone());
    }
    Screen::Unknown
}

#[cfg(test)]
mod tests {
    use super::*;

    fn els(texts: &[&str]) -> Vec<Element> {
        texts.iter().enumerate().map(|(i, t)| Element { id: format!("e{}", i + 1), text: t.to_string() }).collect()
    }

    fn id(s: &Screen) -> &str {
        match s {
            Screen::Dismiss(e) | Screen::Retry(e) | Screen::PlayButton(e) => &e.id,
            _ => "",
        }
    }

    /// 2026-10-03 真机上通关后的画面，OCR 原样。
    #[test]
    fn the_real_daily_stamps_screen_is_a_play_button() {
        let s = classify(&els(&["Daily Stamps", "Complete tasks to get stamps ana", "unlock reward chests", "Play"]));
        assert_eq!((id(&s), matches!(s, Screen::PlayButton(_))), ("e4", true));
    }

    #[test]
    fn a_price_without_a_safe_close_is_money() {
        assert_eq!(classify(&els(&["Out of moves", "Continue for 💎 5"])), Screen::Money);
        assert_eq!(classify(&els(&["Buy now", "Play"])), Screen::Money);
        assert_eq!(classify(&els(&["Play now for 💎 5"])), Screen::Money);
    }

    #[test]
    fn every_money_word_counts() {
        for t in ["Buy now", "Purchase", "Pay", "Checkout", "Gold", "Gem", "Gems", "Bar", "Bars", "购买道具", "5 金币", "充值", "支付"] {
            assert_eq!(classify(&els(&[t])), Screen::Money, "{t}");
        }
        // 整词才算：Payment 不是 pay 这个词，但 Pay 在句子里就是
        assert_eq!(classify(&els(&["Barbara"])), Screen::Unknown);
    }

    #[test]
    fn a_price_with_an_exact_no_thanks_is_dismissed_not_stopped() {
        let s = classify(&els(&["Out of moves", "Continue for 💎 5", "No thanks"]));
        assert_eq!((id(&s), matches!(s, Screen::Dismiss(_))), ("e3", true));
    }

    #[test]
    fn an_ad_is_an_ad_unless_there_is_a_safe_close() {
        assert_eq!(classify(&els(&["Watch an ad for a sweet treat", "Watch ad"])), Screen::Ad);
        assert!(matches!(classify(&els(&["Watch ad", "Not now"])), Screen::Dismiss(_)));
    }

    #[test]
    fn two_exact_play_buttons_are_unknown_but_a_garbled_one_is_ignored() {
        assert_eq!(classify(&els(&["Level 1712", "Select boosters:", "Play", "Play"])), Screen::Unknown);
        let s = classify(&els(&["Level 1712", "Select boosters:", "Play", "B Play"]));
        assert_eq!((id(&s), matches!(s, Screen::PlayButton(_))), ("e3", true));
    }

    #[test]
    fn lives_out_beats_everything_below_it() {
        assert_eq!(classify(&els(&["No more lives", "Ask friends", "Play"])), Screen::LivesOut);
        assert_eq!(classify(&els(&["Out of lives", "Close"])), Screen::LivesOut);
        assert_eq!(classify(&els(&["生命用完了", "关闭"])), Screen::LivesOut);
    }

    #[test]
    fn a_won_screen_stops_even_when_it_offers_next() {
        assert_eq!(classify(&els(&["Level Complete!", "Next"])), Screen::Won);
    }

    #[test]
    fn retry_is_found_by_its_whole_text() {
        let s = classify(&els(&["Out of moves", "Try again"]));
        assert_eq!((id(&s), matches!(s, Screen::Retry(_))), ("e2", true));
        assert_eq!(classify(&els(&["Try again later maybe"])), Screen::Unknown);
    }

    #[test]
    fn matching_ignores_case_spaces_and_stray_symbols() {
        assert!(matches!(classify(&els(&["  NOT   NOW "])), Screen::Dismiss(_)));
        assert!(matches!(classify(&els(&["Close ×"])), Screen::Dismiss(_)));
        assert!(matches!(classify(&els(&["关闭"])), Screen::Dismiss(_)));
    }

    #[test]
    fn close_beats_play_and_save_my_progress_is_never_chosen() {
        assert!(matches!(classify(&els(&["Play", "Close"])), Screen::Dismiss(_)));
        let s = classify(&els(&["Play", "Save My Progress"]));
        assert_eq!((id(&s), matches!(s, Screen::PlayButton(_))), ("e1", true));
        assert_eq!(classify(&els(&["Save My Progress"])), Screen::Unknown);
    }

    #[test]
    fn claim_and_collect_are_not_on_the_list() {
        for t in ["Claim", "Collect", "Continue", "OK", "Collect your daily treat!"] {
            assert_eq!(classify(&els(&[t])), Screen::Unknown, "{t}");
        }
    }

    #[test]
    fn nothing_to_read_is_unknown() {
        assert_eq!(classify(&[]), Screen::Unknown);
        assert_eq!(classify(&els(&["38", "90", "4"])), Screen::Unknown);
    }
}
```

- [ ] **Step 2: 跑测试**

Run: `cargo test -p dct-game screen`
Expected: `13 passed`。

- [ ] **Step 3: 变异检查（改坏，必须有测试变红，改完立刻还原；注意只改 `classify` 周围这一处）**

| 把 | 改成 | 该红的测试 |
|---|---|---|
| `if plays.len() == 1 {` | `if !plays.is_empty() {` | `two_exact_play_buttons_…` |
| `MONEY_WORDS` 里去掉任意一个词（`gold`、`gem`、`bar`、`buy`、`checkout`、`purchase` 各试一次） | — | `every_money_word_counts` |
| `MONEY_MARKS` 里去掉 `"💎",` | — | `a_price_without_a_safe_close_is_money` |
| 删掉 `if lives_out(&all) { return Screen::LivesOut; }` | — | `lives_out_beats_everything_below_it` |
| 把「安全的关闭」那段和「价格」那段对调 | — | `a_price_with_an_exact_no_thanks_…` |
| `list.contains(&norm(&e.text).as_str())` | `norm(&e.text).split(" ").any(\|w\| list.contains(&w))` | 多个 |

- [ ] **Step 4: 提交**

```bash
cargo clippy --workspace --all-targets --locked -- -D warnings
git add crates/dct-game/src/screen.rs crates/dct-game/src/lib.rs
git commit -m "feat(game): classify a screen from its OCR text: lives out, price, ad, safe close, retry, play"
```

---

### Task 2: `Dco` 会读字、会点；`Stop` 多九种原因

**Files:**
- Modify: `crates/dct-game/src/play.rs`、`crates/dct-game/src/dco.rs`、`crates/dct-game/src/dco_tests.rs`、`src/game/text.rs`

**Interfaces:**
- Consumes: 任务 1 的 `screen::Element`。
- Produces: `play::Seen { snapshot_id: String, observation_id: Option<String>, elements: Vec<Element> }`；`Dco::see_text(&mut self, &Profile) -> Result<Seen, DcoError>`、`Dco::tap(&mut self, snapshot_id: &str, element_id: &str) -> Result<(), DcoError>`（**默认实现返回 `unsupported` 错误**，第一轮的假 dco 不用改）；`Stop::{Won, LevelEnded, LivesOut, Money, Ad, UnknownScreen(Vec<String>), NoEffect, TriesDone, TapLimit}`；`DcoClient` 实现这两个方法：`see_text` 调 dco 的 `see`（`source: "ocr"`，只留 `id`、`text`、`snapshot_id`、`observation_id`），`tap` 调 dco 的 `tap {snapshot_id, element_id}`，dco 拒绝时保留它的错误码（比如 `needs_ticket`）；`text::nav_line(&Value) -> Option<String>`；`stop_code` 的新代码 `won`、`level_ended`、`lives_out`、`money`、`ad`、`unknown_screen`、`no_effect`、`tries_done`、`tap_limit`；退出码：`won`、`level_ended`、`lives_out`、`tries_done` 是 0，`money`、`ad`、`unknown_screen`、`no_effect`、`tap_limit` 是 1。


- [ ] **改 `crates/dct-game/src/play.rs`**（顶部的 use）：把

```rust
use crate::choose::{choose, Candidate};
```

换成

```rust
use crate::choose::{choose, Candidate};
use crate::screen::Element;
```

- [ ] **改 `crates/dct-game/src/play.rs`**（`Dco` trait）：把

```rust
pub trait Dco {
    fn read_grid(&mut self, p: &Profile) -> Result<GridRead, DcoError>;
    /// 坐标是窗口比例（0～1）。
    fn swipe(&mut self, p: &Profile, from: (f64, f64), to: (f64, f64)) -> Result<(), DcoError>;
}
```

换成

```rust
/// 一次 `see`（OCR）：画面上读到的字和它们的编号。`snapshot_id` 要原样交给 `tap`。
#[derive(Clone, Debug)]
pub struct Seen {
    pub snapshot_id: String,
    pub observation_id: Option<String>,
    pub elements: Vec<Element>,
}

fn unsupported() -> DcoError {
    DcoError { code: "unsupported".into(), message: "这个 dco 不会认画面上的字".into() }
}

pub trait Dco {
    fn read_grid(&mut self, p: &Profile) -> Result<GridRead, DcoError>;
    /// 坐标是窗口比例（0～1）。
    fn swipe(&mut self, p: &Profile, from: (f64, f64), to: (f64, f64)) -> Result<(), DcoError>;
    /// 读画面上的字。只有“自动开始下一局”要用；默认不支持，所以只玩一关的假 dco 不用实现。
    fn see_text(&mut self, _p: &Profile) -> Result<Seen, DcoError> {
        Err(unsupported())
    }
    /// 点 `see_text` 读到的某个元素。dco 自己按那个元素上的字定档，带价格的会拒绝。
    fn tap(&mut self, _snapshot_id: &str, _element_id: &str) -> Result<(), DcoError> {
        Err(unsupported())
    }
}
```

- [ ] **改 `crates/dct-game/src/play.rs`**（`Stop` 枚举末尾）：把

```rust
    /// dco 急停 / 暂停 / 锁屏 / 别的错误。
    Dco(DcoError),
}
```

换成

```rust
    /// dco 急停 / 暂停 / 锁屏 / 别的错误。
    Dco(DcoError),
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
```

- [ ] **改 `crates/dct-game/src/dco.rs`**（顶部的 use）：把

```rust
use crate::play::{Dco, DcoError, Profile};
```

换成

```rust
use crate::play::{Dco, DcoError, Profile, Seen};
use crate::screen::Element;
```

- [ ] **改 `crates/dct-game/src/dco.rs`**（`impl Dco for DcoClient` 末尾）：把

```rust
        self.call("swipe", json!({
            "window": p.window,
            "from": { "x": from.0, "y": from.1 }, "to": { "x": to.0, "y": to.1 },
        }))
        .map(|_| ())
    }
}
```

换成

```rust
        self.call("swipe", json!({
            "window": p.window,
            "from": { "x": from.0, "y": from.1 }, "to": { "x": to.0, "y": to.1 },
        }))
        .map(|_| ())
    }

    fn see_text(&mut self, p: &Profile) -> Result<Seen, DcoError> {
        let body = self.call("see", json!({ "window": p.window, "source": "ocr" }))?;
        let elements = body["elements"]
            .as_array()
            .map(|a| {
                a.iter()
                    .filter_map(|e| Some(Element { id: e["id"].as_str()?.to_string(), text: e["text"].as_str().unwrap_or("").to_string() }))
                    .collect()
            })
            .unwrap_or_default();
        Ok(Seen {
            snapshot_id: body["snapshot_id"].as_str().unwrap_or("").to_string(),
            observation_id: body["observation_id"].as_str().map(String::from),
            elements,
        })
    }

    fn tap(&mut self, snapshot_id: &str, element_id: &str) -> Result<(), DcoError> {
        self.call("tap", json!({ "snapshot_id": snapshot_id, "element_id": element_id })).map(|_| ())
    }
}
```

- [ ] **追加到 `crates/dct-game/src/dco_tests.rs` 末尾**

```rust
/// 2026-10-03 真机通关后那一屏的 `see`（OCR）回复，只留 dct 用到的字段。
#[test]
fn see_text_reads_the_ocr_elements_and_tap_sends_the_snapshot_and_element_ids() {
    let dir = tempfile::tempdir().unwrap();
    let h = fake(dir.path(), TOKEN, |tool, args| match tool {
        "see" => {
            assert_eq!(args["source"], "ocr");
            assert_eq!(args["window"]["app"], "iPhone Mirroring");
            (json!({
                "snapshot_id": "s6", "observation_id": "obs-4adac3d30f33736a", "sources": ["ocr"],
                "elements": [
                    {"id": "e1", "role": "text", "source": "ocr", "text": "Daily Stamps", "bounds": {"x": 1.0, "y": 2.0, "w": 3.0, "h": 4.0}},
                    {"id": "e4", "role": "text", "source": "ocr", "text": "Play", "confidence": 1.0}
                ]
            }), false)
        }
        "tap" => {
            assert_eq!((args["snapshot_id"].as_str(), args["element_id"].as_str()), (Some("s6"), Some("e4")));
            (json!({"tapped": {"id": "e4", "text": "Play"}}), false)
        }
        other => panic!("没想到会调 {other}"),
    });
    let mut c = DcoClient::connect(dir.path()).unwrap();
    let seen = c.see_text(&profile()).unwrap();
    assert_eq!((seen.snapshot_id.as_str(), seen.observation_id.as_deref()), ("s6", Some("obs-4adac3d30f33736a")));
    assert_eq!(seen.elements.iter().map(|e| (e.id.as_str(), e.text.as_str())).collect::<Vec<_>>(), [("e1", "Daily Stamps"), ("e4", "Play")]);
    c.tap(&seen.snapshot_id, "e4").unwrap();
    drop(c);
    assert_eq!(h.join().unwrap().iter().filter(|m| *m == "tools/call").count(), 2);
}

#[test]
fn a_refused_tap_keeps_dcos_reason() {
    let dir = tempfile::tempdir().unwrap();
    let _h = fake(dir.path(), TOKEN, |_, _| (json!({"error": {"code": "needs_ticket", "message": "这个动作属于「付款」档"}}), true));
    let mut c = DcoClient::connect(dir.path()).unwrap();
    let e = c.tap("s1", "e2").unwrap_err();
    assert_eq!(e.code, "needs_ticket");
}
```

- [ ] **整个替换 `src/game/text.rs`**（`Stop` 多了变体，这里的 `match` 要穷尽才编得过；`nav_line` 在任务 4 的命令里才会用到）（下面的代码已在临时副本里编译、跑过测试）

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
    let outcome = rec["outcome"].as_str().unwrap_or("");
    // 划被拒绝：没有走成这一步，不能说消了几颗，也不能叫它第几步。
    if outcome == "stopped" && rec["swiped"].as_bool() == Some(false) {
        return format!("想走 {} ↔ {}，这一步没划成", pos(&c["a"]), pos(&c["b"]));
    }
    // 划了，但画面一直没停下来：消了几颗是我们的预测，没有看到结果，不报。
    if outcome == "stopped" {
        return format!("第 {step} 步：{} ↔ {}，划了以后没等到画面停下", pos(&c["a"]), pos(&c["b"]));
    }
    let f = &c["features"];
    let mut s = format!("第 {step} 步：{} ↔ {}，消 {} 颗", pos(&c["a"]), pos(&c["b"]), n(&f["cleared"]));
    for (key, what) in [("striped", "条纹糖"), ("wrapped", "包装糖"), ("bomb", "彩色炸弹")] {
        if n(&f[key]) > 0 {
            s += &format!("，做出{what}");
        }
    }
    if n(&f["triggered"]) > 0 {
        s += &format!("，引爆 {} 颗特殊糖", n(&f["triggered"]));
    }
    if f["special_swap"].as_bool().unwrap_or(false) {
        s += "，两颗特殊糖互换";
    }
    match outcome {
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
        Stop::Won => "won",
        Stop::LevelEnded => "level_ended",
        Stop::LivesOut => "lives_out",
        Stop::Money => "money",
        Stop::Ad => "ad",
        Stop::UnknownScreen(_) => "unknown_screen",
        Stop::NoEffect => "no_effect",
        Stop::TriesDone => "tries_done",
        Stop::TapLimit => "tap_limit",
    }
}

/// `--auto-next` 点按钮时印的一句话；停下的几种（没有点任何东西）由最后一句话说，这里返回 `None`。
pub fn nav_line(rec: &Value) -> Option<String> {
    let tapped = rec["tapped"].as_str()?;
    let what = match rec["screen"].as_str().unwrap_or("") {
        "play" => format!("点了「{tapped}」，开始新的一局"),
        "retry" => format!("这一局没过，点了「{tapped}」重来"),
        "dismiss" => format!("关掉了一个弹窗（{tapped}）"),
        _ => return None,
    };
    Some(match rec["outcome"].as_str().unwrap_or("") {
        "dry_run" => format!("画面上有「{tapped}」，会点它（试走，没有真点）"),
        "no_effect" => format!("{what}；可是画面没有变化"),
        "stopped" => format!("{what}；没点成"),
        _ => what,
    })
}

pub fn dco_error(e: &DcoError) -> String {
    match e.code.as_str() {
        "dco_down" | "dco_timeout" => e.message.clone(),
        "dco_refused" => "dco 不认这把钥匙（~/.dco/token），先重启 dco。".into(),
        "dco_too_old" => "dco 版本太旧，先更新 dco 再试。".into(),
        "dco_blocked" => "这里不让连 dco（可能是这个 AI 的沙盒限制）。换一个能访问本机的地方再试。".into(),
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
        Stop::Stuck => (format!("停了：划了几次画面都没有变化。请看一眼屏幕上是不是弹出了什么。{tail}"), 1),
        Stop::StillMoving => (format!("停了：等了 8 秒画面还在动。{tail}"), 1),
        Stop::Dco(e) => (format!("停了：{} {tail}", dco_error(e)), 1),
        Stop::Won => (format!("通关了。下一关的棋盘位置不一样，要先重新认棋盘，所以先停在这里。{tail}"), 0),
        Stop::LevelEnded => (
            format!("这一局结束了，我看不出是通关还是没过，为了不乱点先停在这里。要接着玩，请你自己点 Play。{tail}"),
            0,
        ),
        Stop::LivesOut => (format!("生命用完了，先停下。等生命恢复了再让我继续。{tail}"), 0),
        Stop::Money => (format!("出现了要花钱的画面，请你自己处理。我没有点任何东西。{tail}"), 1),
        Stop::Ad => (format!("出现了广告，请你自己关掉。我没有点任何东西。{tail}"), 1),
        Stop::UnknownScreen(texts) => {
            let shown: Vec<&str> = texts.iter().map(String::as_str).filter(|t| !t.trim().is_empty()).take(8).collect();
            (format!("出现了我不认识的画面，先停下，没有点任何东西。画面上的字：{}。{tail}", shown.join(" / ")), 1)
        }
        Stop::NoEffect => (format!("点了按钮，画面却一直没变化，先停下。请看一眼屏幕。{tail}"), 1),
        Stop::TriesDone => (format!("开始和重来的次数到上限了，先停下。要接着来，再运行一次。{tail}"), 0),
        Stop::TapLimit => (format!("点了很多次还没回到棋盘，先停下。请看一眼屏幕上是不是有弹窗在反复出现。{tail}"), 1),
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
                {"a": [0, 0], "b": [0, 1], "score": 3.0, "features": {"cleared": 3, "striped": 0, "wrapped": 0, "bomb": 0, "triggered": 0, "special_swap": false}},
                {"a": [6, 1], "b": [6, 2], "score": 9.0, "features": {"cleared": 4, "striped": 1, "wrapped": 0, "bomb": 0, "triggered": 0, "special_swap": false}}
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
    fn a_refused_swipe_is_not_a_step_and_claims_no_cleared_count() {
        let s = step_line(1, &rec(json!({"outcome": "stopped", "swiped": false})));
        assert!(s.contains("这一步没划成") && !s.contains("消 ") && !s.contains("第 1 步"), "{s}");
    }

    #[test]
    fn a_swipe_whose_settle_never_finished_says_so_and_claims_no_cleared_count() {
        let s = step_line(2, &rec(json!({"outcome": "stopped", "swiped": true})));
        assert!(s.contains("划了以后没等到画面停下") && !s.contains("消 ") && !s.contains("没划成"), "{s}");
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
            (Stop::Won, 0),
            (Stop::LevelEnded, 0),
            (Stop::LivesOut, 0),
            (Stop::TriesDone, 0),
            (Stop::Money, 1),
            (Stop::Ad, 1),
            (Stop::UnknownScreen(vec!["Claim".into()]), 1),
            (Stop::NoEffect, 1),
            (Stop::TapLimit, 1),
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
        assert_eq!(dco_error(&e("dco_timeout")), "原话");
        assert!(dco_error(&e("dco_too_old")).contains("版本太旧"));
        assert!(dco_error(&e("dco_blocked")).contains("不让连 dco") && !dco_error(&e("dco_blocked")).contains("原话"));
        let refused = DcoError { code: "dco_refused".into(), message: r#"{"ok":false}"#.into() };
        assert_eq!(dco_error(&refused), "dco 不认这把钥匙（~/.dco/token），先重启 dco。");
        assert_eq!(dco_error(&e("weird")), "dco 说：原话");
    }

    #[test]
    fn the_stuck_sentence_does_not_claim_two_tries() {
        let l = stop_line(&Stop::Stuck, 1, Path::new("/x")).0;
        assert!(l.contains("划了几次画面都没有变化") && !l.contains("两次"), "{l}");
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
            Stop::Won,
            Stop::LevelEnded,
            Stop::LivesOut,
            Stop::Money,
            Stop::Ad,
            Stop::UnknownScreen(vec![]),
            Stop::NoEffect,
            Stop::TriesDone,
            Stop::TapLimit,
        ];
        let mut codes: Vec<&str> = all.iter().map(stop_code).collect();
        codes.sort();
        codes.dedup();
        assert_eq!(codes.len(), all.len());
    }

    #[test]
    fn nav_lines_say_what_was_pressed_and_never_use_jargon() {
        let r = |screen: &str, outcome: &str| json!({"kind": "nav", "screen": screen, "tapped": "Play", "outcome": outcome});
        assert_eq!(nav_line(&r("play", "changed")).unwrap(), "点了「Play」，开始新的一局");
        assert_eq!(nav_line(&r("retry", "changed")).unwrap(), "这一局没过，点了「Play」重来");
        assert_eq!(nav_line(&r("dismiss", "changed")).unwrap(), "关掉了一个弹窗（Play）");
        assert!(nav_line(&r("play", "no_effect")).unwrap().ends_with("可是画面没有变化"));
        assert!(nav_line(&r("play", "dry_run")).unwrap().contains("试走"));
        // 没点任何东西的停下，不在这里说
        assert_eq!(nav_line(&json!({"kind": "nav", "screen": "money", "tapped": null, "outcome": "stopped"})), None);
        for s in ["play", "retry", "dismiss"] {
            let line = nav_line(&r(s, "changed")).unwrap();
            for w in ["snapshot", "element", "tap", "OCR", "daemon"] {
                assert!(!line.to_lowercase().contains(&w.to_lowercase()), "{line}");
            }
        }
    }

    #[test]
    fn an_unknown_screen_shows_what_was_on_it_but_at_most_eight_pieces() {
        let texts: Vec<String> = (1..=12).map(|i| format!("t{i}")).collect();
        let (line, code) = stop_line(&Stop::UnknownScreen(texts), 0, Path::new("/x"));
        assert_eq!(code, 1);
        assert!(line.contains("t1 / t2") && line.contains("t8") && !line.contains("t9"), "{line}");
        assert!(line.contains("没有点任何东西"));
    }

    #[test]
    fn stop_sentences_for_money_and_ads_promise_nothing_was_pressed() {
        for s in [Stop::Money, Stop::Ad] {
            assert!(stop_line(&s, 3, Path::new("/x")).0.contains("没有点任何东西"));
        }
    }
}
```

- [ ] **Step 2: 跑测试，主 crate 也要能编**

Run: `cargo test -p dct-game`
Expected: 全绿（`dco_tests` 多了 2 个：`see_text_reads_…`、`a_refused_tap_…`）。

Run: `cargo test --lib game::`
Expected: `30 passed`（第一轮的 27 个 + `text` 新的 3 个）。

Run: `cargo build`
Expected: 通过。

- [ ] **Step 3: 提交**

```bash
cargo clippy -p dct-game --all-targets -- -D warnings
git add crates/dct-game src/game/text.rs
git commit -m "feat(game): the dco client can read the screen's text and tap an element; Stop gains the auto-next reasons, with plain-Chinese lines for each"
```

---

### Task 3: `auto_next` 循环

**Files:**
- Create: `crates/dct-game/src/navigate.rs`
- Modify: `crates/dct-game/src/lib.rs`

**Interfaces:**
- Consumes: 任务 1 的 `classify`、`Screen`、`Element`；任务 2 的 `Seen`、`Dco::see_text/tap`、新的 `Stop`；第一轮的 `play`、`Options`、`Summary`、`Clock`、`Profile`。
- Produces: `navigate::NavOptions { max_steps: usize, tries: usize, dry_run: bool }`；`navigate::auto_next(&mut dyn Dco, &mut dyn Clock, &Profile, &NavOptions, &mut dyn FnMut(Value)) -> Summary`。每次要点的、要停的，都通过回调交出一条 `{"schema":1,"kind":"nav","time_ms","observation_id","screen":"play|retry|dismiss|won|lives_out|money|ad|unknown|level_ended","texts":[…],"tapped":文字|null,"outcome":"changed|no_effect|stopped|dry_run"}`；棋盘上的每一步照旧由 `play` 交出第一轮那种记录。

- [ ] **Step 1: 在 `crates/dct-game/src/lib.rs` 里 `pub mod dco;` 下面（`#[cfg(unix)]` 之后）、`pub mod play;` 上面加 `pub mod navigate;`**


- [ ] **创建 `crates/dct-game/src/navigate.rs`**（下面的代码已在临时副本里编译、跑过测试）

```rust
//! `--auto-next`：一局结束以后自己接着来——失败了点“再来一次”，弹窗里只点白名单里的字，
//! 认出生命用完、价格、广告、不认识的画面就停。打赢也停：下一关的版面不一样，现在的棋盘位置读不对。
//! 跟 dco 说话和计时都是传进来的（同 `play`），所以测试里换成假的。
use crate::play::{play, Clock, Dco, DcoError, Options, Profile, Seen, Stop, Summary};
use crate::screen::{classify, Element, Screen};
use serde_json::{json, Value};

pub struct NavOptions {
    /// 整个命令一共最多走几步棋（不是每一局）。
    pub max_steps: usize,
    /// 整个命令最多点几次“开始 / 再来一次”；每一次都会用掉一条生命。
    pub tries: usize,
    pub dry_run: bool,
}

const AFTER_TAP_FIRST_MS: u64 = 400;
const POLL_MS: u64 = 300;
const CHANGE_GIVE_UP_MS: u64 = 8_000;
/// 整个命令里点按钮的总次数上限：防止两个弹窗互相弹来弹去停不下来。
const MAX_TAPS: usize = 40;

fn texts(seen: &Seen) -> Vec<String> {
    seen.elements.iter().map(|e| e.text.clone()).collect()
}

fn sorted_texts(seen: &Seen) -> Vec<String> {
    let mut t = texts(seen);
    t.sort();
    t
}

fn nav_record(clock: &dyn Clock, seen: &Seen, screen: &str, tapped: Option<&str>, outcome: &str) -> Value {
    json!({
        "schema": 1, "kind": "nav", "time_ms": clock.now_ms(), "observation_id": seen.observation_id,
        "screen": screen, "texts": texts(seen), "tapped": tapped, "outcome": outcome,
    })
}

/// 点一个元素，再等画面变。变了返回 `Ok(true)`，8 秒不变返回 `Ok(false)`；dco 拒绝或出错就把错误交出去。
fn tap_and_wait(dco: &mut dyn Dco, clock: &mut dyn Clock, p: &Profile, seen: &Seen, el: &Element) -> Result<bool, DcoError> {
    dco.tap(&seen.snapshot_id, &el.id)?;
    let before = sorted_texts(seen);
    let t0 = clock.now_ms();
    clock.sleep_ms(AFTER_TAP_FIRST_MS);
    loop {
        let now = dco.see_text(p)?;
        if sorted_texts(&now) != before {
            return Ok(true);
        }
        if clock.now_ms().saturating_sub(t0) >= CHANGE_GIVE_UP_MS {
            return Ok(false);
        }
        clock.sleep_ms(POLL_MS);
    }
}

pub fn auto_next(dco: &mut dyn Dco, clock: &mut dyn Clock, p: &Profile, o: &NavOptions, sink: &mut dyn FnMut(Value)) -> Summary {
    let (mut steps, mut tries, mut taps, mut no_effect) = (0usize, 0usize, 0usize, 0usize);
    // after_board：刚玩完一局，还没有被明确告知“再来一次”。这时看到的 Play 可能是通关后的下一关，不点。
    let (mut after_board, mut retrying) = (false, false);
    let stop = loop {
        let seen = match dco.see_text(p) {
            Ok(s) => s,
            Err(e) => break Stop::Dco(e),
        };
        let screen = classify(&seen.elements);
        // 要点哪个元素、它是什么类型；不点的情况直接在这里停。
        let (kind, el) = match screen {
            Screen::LivesOut => {
                sink(nav_record(clock, &seen, "lives_out", None, "stopped"));
                break Stop::LivesOut;
            }
            Screen::Won => {
                sink(nav_record(clock, &seen, "won", None, "stopped"));
                break Stop::Won;
            }
            Screen::Money => {
                sink(nav_record(clock, &seen, "money", None, "stopped"));
                break Stop::Money;
            }
            Screen::Ad => {
                sink(nav_record(clock, &seen, "ad", None, "stopped"));
                break Stop::Ad;
            }
            Screen::Dismiss(el) => ("dismiss", el),
            Screen::Retry(el) => {
                if tries >= o.tries {
                    break Stop::TriesDone;
                }
                ("retry", el)
            }
            Screen::PlayButton(el) => {
                if after_board && !retrying {
                    sink(nav_record(clock, &seen, "level_ended", None, "stopped"));
                    break Stop::LevelEnded;
                }
                if tries >= o.tries {
                    break Stop::TriesDone;
                }
                ("play", el)
            }
            Screen::Unknown => {
                // 不是任何已知的按钮画面：看看是不是棋盘。
                match dco.read_grid(p) {
                    Ok(g) if g.classes.iter().filter(|c| c.count >= 2).count() >= 3 => {
                        let remaining = o.max_steps.saturating_sub(steps);
                        if remaining == 0 {
                            break Stop::StepsDone;
                        }
                        let s = play(dco, clock, p, &Options { max_steps: remaining, dry_run: o.dry_run }, sink);
                        steps += s.steps;
                        match s.stop {
                            // 画面变了（结算页、弹窗）：回到上面重新看是什么。
                            Stop::NoGrid(_) | Stop::ClassesChanged { .. } => {
                                after_board = true;
                                retrying = false;
                                no_effect = 0;
                                continue;
                            }
                            other => break other,
                        }
                    }
                    Ok(_) => {
                        sink(nav_record(clock, &seen, "unknown", None, "stopped"));
                        break Stop::UnknownScreen(texts(&seen));
                    }
                    Err(e) if e.code == "not_a_grid" => {
                        sink(nav_record(clock, &seen, "unknown", None, "stopped"));
                        break Stop::UnknownScreen(texts(&seen));
                    }
                    Err(e) => break Stop::Dco(e),
                }
            }
        };
        if o.dry_run {
            sink(nav_record(clock, &seen, kind, Some(&el.text), "dry_run"));
            break Stop::DryRun;
        }
        if taps >= MAX_TAPS {
            break Stop::TapLimit;
        }
        taps += 1;
        if kind != "dismiss" {
            tries += 1;
        }
        match tap_and_wait(dco, clock, p, &seen, &el) {
            Ok(true) => {
                sink(nav_record(clock, &seen, kind, Some(&el.text), "changed"));
                no_effect = 0;
                if kind == "retry" {
                    after_board = false;
                    retrying = true;
                }
            }
            Ok(false) => {
                sink(nav_record(clock, &seen, kind, Some(&el.text), "no_effect"));
                no_effect += 1;
                if no_effect >= 2 {
                    break Stop::NoEffect;
                }
            }
            Err(e) => {
                sink(nav_record(clock, &seen, kind, Some(&el.text), "stopped"));
                break Stop::Dco(e);
            }
        }
    };
    Summary { steps, stop }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::board::GridRead;

    fn grid(cells: &[&[u16]]) -> GridRead {
        let (rows, cols) = (cells.len(), cells[0].len());
        let mut ids: Vec<u16> = cells.iter().flat_map(|r| r.iter().copied()).collect();
        ids.sort();
        ids.dedup();
        let classes: Vec<Value> = ids.iter().map(|&i| json!({"id": i, "count": cells.iter().flat_map(|r| r.iter()).filter(|&&c| c == i).count()})).collect();
        serde_json::from_value(json!({
            "rows": rows, "cols": cols, "cells": cells, "odd": vec![vec![false; cols]; rows], "classes": classes
        }))
        .unwrap()
    }

    /// 能走的棋盘（把 (0,2) 和 (1,2) 对换，第 0 行四连），三个大类别。
    const A: &[&[u16]] = &[&[1, 1, 2, 1], &[2, 3, 1, 3], &[3, 2, 3, 2]];
    /// 没有任何能走的步，也是三个大类别。
    const DEAD: &[&[u16]] = &[&[1, 2, 3, 1], &[3, 1, 2, 3], &[2, 3, 1, 2]];

    #[derive(Clone)]
    enum Frame {
        Text(Vec<&'static str>),
        Board(GridRead),
    }

    /// 一个按帧往前走的假世界：点了按钮、划了一下，就换到下一帧；最后一帧停在那里。
    struct World {
        frames: Vec<Frame>,
        at: usize,
        taps: Vec<String>,
        swipes: usize,
        tap_error: Option<DcoError>,
        /// 为真时，点按钮不换帧（模拟点了没反应）。
        stuck: bool,
        /// 逐次点按钮的脚本：`true` = 这一次点了没反应。用完以后看 `stuck`。
        stuck_script: Vec<bool>,
    }

    impl World {
        fn new(frames: Vec<Frame>) -> World {
            World { frames, at: 0, taps: vec![], swipes: 0, tap_error: None, stuck: false, stuck_script: vec![] }
        }
        fn advance(&mut self) {
            if self.at + 1 < self.frames.len() {
                self.at += 1;
            }
        }
        /// 两个文字帧交替，让每次点完画面都“变”，但永远停在弹窗上。
        fn cycling(a: Vec<&'static str>, b: Vec<&'static str>) -> World {
            let mut frames = vec![];
            for i in 0..200 {
                frames.push(Frame::Text(if i % 2 == 0 { a.clone() } else { b.clone() }));
            }
            World::new(frames)
        }
    }

    impl Dco for World {
        fn read_grid(&mut self, _: &Profile) -> Result<GridRead, DcoError> {
            match &self.frames[self.at] {
                Frame::Board(g) => Ok(g.clone()),
                Frame::Text(_) => Err(DcoError { code: "not_a_grid".into(), message: "这块区域分不清类别".into() }),
            }
        }
        fn swipe(&mut self, _: &Profile, _: (f64, f64), _: (f64, f64)) -> Result<(), DcoError> {
            self.swipes += 1;
            self.advance();
            Ok(())
        }
        fn see_text(&mut self, _: &Profile) -> Result<Seen, DcoError> {
            let texts: Vec<&str> = match &self.frames[self.at] {
                Frame::Text(t) => t.clone(),
                Frame::Board(_) => vec!["38", "90"],
            };
            Ok(Seen {
                snapshot_id: format!("s{}", self.at),
                observation_id: Some(format!("obs-{}", self.at)),
                elements: texts.iter().enumerate().map(|(i, t)| Element { id: format!("e{}", i + 1), text: t.to_string() }).collect(),
            })
        }
        fn tap(&mut self, snapshot_id: &str, element_id: &str) -> Result<(), DcoError> {
            if let Some(e) = self.tap_error.clone() {
                return Err(e);
            }
            // 点的必须是眼前这一帧里的元素（snapshot 对得上）。
            assert_eq!(snapshot_id, format!("s{}", self.at), "点的不是刚读到的那一屏");
            let text = match &self.frames[self.at] {
                Frame::Text(t) => t[element_id.trim_start_matches('e').parse::<usize>().unwrap() - 1].to_string(),
                Frame::Board(_) => panic!("棋盘上不该点按钮"),
            };
            self.taps.push(text);
            let stuck = if self.stuck_script.is_empty() { self.stuck } else { self.stuck_script.remove(0) };
            if !stuck {
                self.advance();
            }
            Ok(())
        }
    }

    struct Clk(u64);
    impl Clock for Clk {
        fn now_ms(&self) -> u64 {
            self.0
        }
        fn sleep_ms(&mut self, ms: u64) {
            self.0 += ms;
        }
    }

    fn profile() -> Profile {
        Profile { window: json!({"app": "x"}), region: [0.2, 0.3, 0.5, 0.4], rows: 3, cols: 4, extra: json!({}) }
    }

    fn run(w: &mut World, tries: usize, dry: bool) -> (Summary, Vec<Value>) {
        let mut log = vec![];
        let s = auto_next(w, &mut Clk(0), &profile(), &NavOptions { max_steps: 50, tries, dry_run: dry }, &mut |v| log.push(v));
        (s, log)
    }

    fn text(t: &[&'static str]) -> Frame {
        Frame::Text(t.to_vec())
    }
    fn board(c: &[&[u16]]) -> Frame {
        Frame::Board(grid(c))
    }

    #[test]
    fn a_failed_level_is_retried_through_the_start_box_and_played_again() {
        let mut w = World::new(vec![
            board(A),
            text(&["Out of moves", "Try again"]),
            text(&["Level 1712", "Select boosters:", "Play", "B Play"]),
            board(DEAD),
        ]);
        let (s, log) = run(&mut w, 5, false);
        assert_eq!(s.stop, Stop::NoMoves);
        assert_eq!(w.taps, ["Try again", "Play"]);
        assert_eq!(w.swipes, 1);
        let screens: Vec<&str> = log.iter().filter(|r| r["kind"] == "nav").map(|r| r["screen"].as_str().unwrap()).collect();
        assert_eq!(screens, ["retry", "play"]);
        // 每条 nav 记录都带着画面上的字（以后补白名单的证据）
        assert!(log.iter().filter(|r| r["kind"] == "nav").all(|r| r["texts"].as_array().unwrap().len() >= 2));
    }

    #[test]
    fn after_a_level_a_play_button_is_not_pressed_because_it_may_be_the_next_level() {
        let mut w = World::new(vec![board(A), text(&["Daily Stamps", "Play"])]);
        let (s, log) = run(&mut w, 5, false);
        assert_eq!(s.stop, Stop::LevelEnded);
        assert!(w.taps.is_empty());
        assert_eq!(log.last().unwrap()["screen"], "level_ended");
    }

    #[test]
    fn the_guard_survives_dismissing_popups_after_a_level() {
        let mut w = World::new(vec![board(A), text(&["Not now"]), text(&["Daily Stamps", "Play"])]);
        let (s, _) = run(&mut w, 5, false);
        assert_eq!(s.stop, Stop::LevelEnded);
        assert_eq!(w.taps, ["Not now"]);
    }

    #[test]
    fn a_fresh_start_presses_play_then_plays_the_board() {
        let mut w = World::new(vec![text(&["Daily Stamps", "Play"]), board(DEAD)]);
        let (s, _) = run(&mut w, 5, false);
        assert_eq!(s.stop, Stop::NoMoves);
        assert_eq!(w.taps, ["Play"]);
    }

    #[test]
    fn popups_are_dismissed_one_after_another() {
        let mut w = World::new(vec![text(&["Not now"]), text(&["Got it"]), text(&["Close"]), board(DEAD)]);
        let (s, _) = run(&mut w, 5, false);
        assert_eq!(s.stop, Stop::NoMoves);
        assert_eq!(w.taps, ["Not now", "Got it", "Close"]);
    }

    #[test]
    fn money_ads_lives_won_and_unknown_screens_stop_without_a_single_tap() {
        for (frame, want) in [
            (text(&["Buy 💎 5"]), Stop::Money),
            (text(&["Watch an ad for a sweet treat", "Watch ad"]), Stop::Ad),
            (text(&["No more lives", "Ask friends"]), Stop::LivesOut),
            (text(&["Level Complete!"]), Stop::Won),
            (text(&["Collect your daily treat!", "Claim"]), Stop::UnknownScreen(vec!["Collect your daily treat!".into(), "Claim".into()])),
        ] {
            let mut w = World::new(vec![frame]);
            let (s, log) = run(&mut w, 5, false);
            assert_eq!(s.stop, want);
            assert!(w.taps.is_empty(), "{want:?} 不该点任何东西");
            assert_eq!(log.last().unwrap()["outcome"], "stopped");
        }
    }

    #[test]
    fn a_priced_failure_popup_with_no_thanks_is_declined_and_play_goes_on() {
        let mut w = World::new(vec![
            board(A),
            text(&["Out of moves", "Continue for 💎 5", "No thanks"]),
            text(&["Out of moves", "Try again"]),
            text(&["Play"]),
            board(DEAD),
        ]);
        let (s, _) = run(&mut w, 5, false);
        assert_eq!(s.stop, Stop::NoMoves);
        assert_eq!(w.taps, ["No thanks", "Try again", "Play"]);
    }

    #[test]
    fn a_button_that_does_nothing_stops_after_two_tries() {
        let mut w = World::new(vec![text(&["Not now"])]);
        w.stuck = true;
        let (s, log) = run(&mut w, 5, false);
        assert_eq!(s.stop, Stop::NoEffect);
        assert_eq!(w.taps.len(), 2);
        assert_eq!(log.iter().filter(|r| r["outcome"] == "no_effect").count(), 2);
    }

    #[test]
    fn the_retry_limit_counts_play_and_try_again_but_not_dismissals() {
        let mut w = World::cycling(vec!["Play", "a"], vec!["Play", "b"]);
        let (s, _) = run(&mut w, 2, false);
        assert_eq!(s.stop, Stop::TriesDone);
        assert_eq!(w.taps.len(), 2);
    }

    #[test]
    fn popups_that_keep_coming_back_hit_the_tap_limit() {
        let mut w = World::cycling(vec!["Close", "a"], vec!["Close", "b"]);
        let (s, _) = run(&mut w, 5, false);
        assert_eq!(s.stop, Stop::TapLimit);
        assert_eq!(w.taps.len(), MAX_TAPS);
    }

    #[test]
    fn dry_run_taps_nothing_and_says_what_it_would_press() {
        let mut w = World::new(vec![text(&["Daily Stamps", "Play"]), board(DEAD)]);
        let (s, log) = run(&mut w, 5, true);
        assert_eq!(s.stop, Stop::DryRun);
        assert!(w.taps.is_empty());
        assert_eq!((log[0]["outcome"].as_str(), log[0]["tapped"].as_str()), (Some("dry_run"), Some("Play")));
    }

    #[test]
    fn a_dco_refusal_to_tap_is_passed_on() {
        let mut w = World::new(vec![text(&["Play"]), board(DEAD)]);
        w.tap_error = Some(DcoError { code: "needs_ticket".into(), message: "x".into() });
        let (s, _) = run(&mut w, 5, false);
        assert_eq!(s.stop, Stop::Dco(DcoError { code: "needs_ticket".into(), message: "x".into() }));
    }

    #[test]
    fn the_step_budget_is_shared_across_levels() {
        let mut w = World::new(vec![board(A), board(A)]);
        let mut log = vec![];
        let s = auto_next(&mut w, &mut Clk(0), &profile(), &NavOptions { max_steps: 1, tries: 5, dry_run: false }, &mut |v| log.push(v));
        assert_eq!(s.steps, 1);
        assert_eq!(s.stop, Stop::StepsDone);
    }

    #[test]
    fn dismissals_do_not_use_up_the_retry_budget() {
        let mut w = World::new(vec![text(&["Not now"]), text(&["Got it"]), text(&["Daily Stamps", "Play"]), board(DEAD)]);
        let (s, _) = run(&mut w, 1, false);
        assert_eq!(s.stop, Stop::NoMoves);
        assert_eq!(w.taps, ["Not now", "Got it", "Play"]);
    }

    #[test]
    fn a_button_that_works_resets_the_no_effect_count() {
        // 没反应、成功、没反应、成功：每次都只是“一次没反应”，不该停
        let mut w = World::new(vec![text(&["Not now", "a"]), text(&["Not now", "b"]), text(&["Not now", "c"]), board(DEAD)]);
        w.stuck_script = vec![true, false, true, false, false];
        let (s, _) = run(&mut w, 5, false);
        assert_eq!(s.stop, Stop::NoMoves);
    }
}
```

- [ ] **Step 2: 跑测试**

Run: `cargo test -p dct-game`
Expected: `79 passed`（lib）+ 1（`tests/real_board.rs`）。其中 `navigate` 15 个。

- [ ] **Step 3: 变异检查（逐条做，改完还原）**

| 把 | 改成 | 该红的测试 |
|---|---|---|
| `if after_board && !retrying {` | `if false {` | `after_a_level_a_play_button_is_not_pressed…`、`the_guard_survives…` |
| `if kind != "dismiss" {\n            tries += 1;\n        }` | `tries += 1;` | `dismissals_do_not_use_up_the_retry_budget`、`popups_that_keep_coming_back_hit_the_tap_limit` |
| `if no_effect >= 2 {` | `if no_effect >= 3 {` | `a_button_that_does_nothing_stops_after_two_tries` |
| `sink(…"changed"));\n                no_effect = 0;` 里的 `no_effect = 0;` | （删掉） | `a_button_that_works_resets_the_no_effect_count` |
| `if kind == "retry" { after_board = false; retrying = true; }` 整段 | （删掉） | `a_failed_level_is_retried_…` |
| `if taps >= MAX_TAPS { break Stop::TapLimit; }` 整段 | （删掉） | `popups_that_keep_coming_back_hit_the_tap_limit` |
| `Stop::NoGrid(_) \| Stop::ClassesChanged { .. } => {\n                                after_board = true;` 里的 `after_board = true;` | （删掉） | `after_a_level_…`、`the_guard_survives…` |

- [ ] **Step 4: 提交**

```bash
cargo clippy -p dct-game --all-targets -- -D warnings
git add crates/dct-game
git commit -m "feat(game): --auto-next loop: retry a failed level, press only whitelisted buttons, stop on money, ads, lives, wins and unknown screens"
```

---

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

### Task 5: 命令层的集成测试、说明文档

**Files:**
- Modify: `tests/game_cli.rs`、`README.md`、`README.zh-CN.md`

- [ ] **Step 1: 把下面的内容追加到 `tests/game_cli.rs` 末尾**（它给假 dco 加了 `see`、`tap`，按「帧」往前走）

```rust
// ---- `--auto-next`：假 dco 按“帧”往前走，点一下换下一帧 ----

enum Frame {
    Text(Vec<&'static str>),
    Board,
}

/// 假 dco：除了 read_grid / swipe，还会 see（OCR）和 tap。点或划一下就换到下一帧，最后一帧停在那里。
/// 返回它收到的工具调用名和 tap 的元素文字（按先后）。
fn fake_dco_world(home: &std::path::Path, frames: Vec<Frame>) -> std::thread::JoinHandle<(Vec<String>, Vec<String>)> {
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
        let (mut tools, mut taps, mut at) = (vec![], vec![], 0usize);
        let mut line = String::new();
        r.read_line(&mut line).unwrap();
        writeln!(w, r#"{{"ok":true}}"#).unwrap();
        loop {
            line.clear();
            if r.read_line(&mut line).unwrap() == 0 {
                return (tools, taps);
            }
            let req: Value = serde_json::from_str(line.trim()).unwrap();
            let Some(id) = req.get("id") else { continue };
            let result = match req["method"].as_str().unwrap() {
                "tools/call" => {
                    let name = req["params"]["name"].as_str().unwrap().to_string();
                    tools.push(name.clone());
                    let args = &req["params"]["arguments"];
                    let (body, is_error) = match name.as_str() {
                        "see" => {
                            let texts: Vec<&str> = match &frames[at] {
                                Frame::Text(t) => t.clone(),
                                Frame::Board => vec!["38", "90"],
                            };
                            let elements: Vec<Value> = texts.iter().enumerate().map(|(i, t)| json!({"id": format!("e{}", i + 1), "text": t})).collect();
                            (json!({"snapshot_id": format!("s{at}"), "observation_id": format!("obs-{at}"), "elements": elements}), false)
                        }
                        "tap" => {
                            assert_eq!(args["snapshot_id"], format!("s{at}"));
                            let Frame::Text(t) = &frames[at] else { panic!("棋盘上不该点按钮") };
                            let n: usize = args["element_id"].as_str().unwrap().trim_start_matches('e').parse().unwrap();
                            taps.push(t[n - 1].to_string());
                            if at + 1 < frames.len() {
                                at += 1;
                            }
                            (json!({"tapped": true}), false)
                        }
                        "read_grid" => match &frames[at] {
                            Frame::Board => (board(), false),
                            Frame::Text(_) => (json!({"error": {"code": "not_a_grid", "message": "这块区域分不清类别"}}), true),
                        },
                        other => panic!("没想到会调 {other}"),
                    };
                    json!({"content": [{"type": "text", "text": body.to_string()}], "isError": is_error})
                }
                _ => json!({"protocolVersion": "2025-06-18"}),
            };
            writeln!(w, "{}", json!({"jsonrpc": "2.0", "id": id, "result": result})).unwrap();
        }
    })
}

fn nav_records(home: &std::path::Path) -> Vec<Value> {
    log_lines(home).into_iter().filter(|r| r["kind"] == "nav").collect()
}

#[test]
fn auto_next_dry_run_says_what_it_would_press_and_presses_nothing() {
    let home = tempfile::tempdir().unwrap();
    let h = fake_dco_world(home.path(), vec![Frame::Text(vec!["Daily Stamps", "Play"]), Frame::Board]);
    let (out, err, code) = dct(home.path(), &["game", "play", "--auto-next", "--dry-run"]);
    assert_eq!(code, 0, "{out}{err}");
    assert!(out.contains("画面上有「Play」，会点它"), "{out}");
    let (tools, taps) = h.join().unwrap();
    assert_eq!(tools, vec!["see"]);
    assert!(taps.is_empty());
    let nav = nav_records(home.path());
    assert_eq!((nav[0]["screen"].as_str(), nav[0]["outcome"].as_str()), (Some("play"), Some("dry_run")));
    assert_eq!(nav[0]["texts"], json!(["Daily Stamps", "Play"]));
}

#[test]
fn auto_next_stops_on_a_price_without_pressing_anything() {
    let home = tempfile::tempdir().unwrap();
    let h = fake_dco_world(home.path(), vec![Frame::Text(vec!["Out of moves", "Continue for 💎 5"])]);
    let (out, err, code) = dct(home.path(), &["game", "play", "--auto-next"]);
    assert_eq!(code, 1, "{out}{err}");
    assert!(err.contains("要花钱") && err.contains("没有点任何东西"), "{err}");
    assert!(h.join().unwrap().1.is_empty());
    let lines = log_lines(home.path());
    assert_eq!(lines.last().unwrap()["stop"], "money");
    assert!(lines.iter().all(|l| l["run_id"].is_string()));
}

#[test]
fn auto_next_presses_a_safe_button_then_stops_on_a_screen_it_does_not_know_and_records_its_words() {
    let home = tempfile::tempdir().unwrap();
    let h = fake_dco_world(home.path(), vec![Frame::Text(vec!["Music Season", "Not now"]), Frame::Text(vec!["Collect your daily treat!", "Claim"])]);
    let (out, err, code) = dct(home.path(), &["game", "play", "--auto-next"]);
    assert_eq!(code, 1, "{out}{err}");
    assert!(out.contains("关掉了一个弹窗（Not now）"), "{out}");
    assert!(err.contains("不认识的画面") && err.contains("Collect your daily treat! / Claim"), "{err}");
    assert_eq!(h.join().unwrap().1, vec!["Not now"]);
    let nav = nav_records(home.path());
    assert_eq!(nav.len(), 2);
    assert_eq!((nav[0]["screen"].as_str(), nav[0]["tapped"].as_str(), nav[0]["outcome"].as_str()), (Some("dismiss"), Some("Not now"), Some("changed")));
    assert_eq!((nav[1]["screen"].as_str(), nav[1]["outcome"].as_str()), (Some("unknown"), Some("stopped")));
    assert_eq!(nav[1]["texts"], json!(["Collect your daily treat!", "Claim"]));
    assert_eq!(log_lines(home.path()).last().unwrap()["stop"], "unknown_screen");
}

#[test]
fn without_auto_next_the_screen_is_never_read_as_text() {
    let home = tempfile::tempdir().unwrap();
    let h = fake_dco_world(home.path(), vec![Frame::Board]);
    let (out, err, code) = dct(home.path(), &["game", "play", "--dry-run"]);
    assert_eq!(code, 0, "{out}{err}");
    assert_eq!(h.join().unwrap().0, vec!["read_grid"]);
}

#[test]
fn auto_next_arguments_are_checked() {
    let home = tempfile::tempdir().unwrap();
    for args in [&["game", "play", "--tries", "0"][..], &["game", "play", "--tries", "21"], &["game", "play", "--auto-next", "--tries"]] {
        let (_, err, code) = dct(home.path(), args);
        assert_eq!(code, 2, "{args:?} {err}");
    }
}
```

- [ ] **Step 2: 跑测试**

Run: `cargo test --test game_cli`
Expected: `12 passed`（第一轮 7 个 + 这里 5 个）。

- [ ] **Step 3: 再跑一遍全仓库**

Run: `cargo test --workspace`
Expected: 全绿。

Run: `cargo clippy --workspace --all-targets --locked -- -D warnings`
Expected: 通过。

- [ ] **Step 4: 文档**

在 `README.zh-CN.md` 的命令清单附近（跟 `dct send` 同一块）加：

```markdown
### 让 AI 玩三消游戏（Mac）

在 iPhone 镜像里打开 Candy Crush 的一关，然后在 dct 的会话里对 AI 说「帮我玩一关 Candy Crush」。
dct 读棋盘、按规则选步、让 dco 划，每步不到一秒；每一步的棋盘、所有能走的步和选择都记在 `~/.dct/games/log/`。
也可以自己运行：`dct game play [--steps 20] [--dry-run]`。棋盘的位置写在 `~/.dct/games/candy-crush.toml`（没有这个文件就用内置的）。

加上 `--auto-next`，一局没过时它会自己点「Try again」「Play」重来；弹窗只关「Close」「Not now」「No thanks」这类安全的；
遇到要花钱、广告、生命用完、通关、不认识的画面就停下，什么都不点（`--tries` 限制重来的次数，默认 5，每次都会用掉一条生命）。
打赢以后不会自动进下一关：下一关的棋盘位置不一样，要等「自动认新棋盘」做好。
```

`README.md`（英文）加同样内容的英文版，位置相同。

- [ ] **Step 5: 提交**

```bash
git add tests/game_cli.rs README.md README.zh-CN.md
git commit -m "test(game): auto-next against a fake dco that can see text and tap; docs for --auto-next"
```

---

### Task 6: 真机验收（需要用户在场，控制者自己做，不派给子 agent）

前 5 个任务都在假 dco 上测，这是第一次对着真的游戏。能自己看屏幕的就自己看（`DCO_IMAGE_OUT=文件 dco call see '{"window":{"app":"iPhone Mirroring"},"include_image":true}'`，再用 Read 看图），不要转给用户去核对。

- [ ] **Step 1: 试走，不点任何东西**

Run: `dct game play --auto-next --dry-run`（用新编译的 `target/debug/dct`，不是装好的旧 dct）
Expected: 屏幕上是什么，它就说什么：比如在「Daily Stamps」上说「画面上有「Play」，会点它（试走，没有真点）」，退出码 0。截图核对它认的画面对不对。在棋盘上则是第一轮的试走。

- [ ] **Step 2: 真点一次，只开始一局**

只在用户明确说可以开一局之后做：`dct game play --auto-next --steps 5 --tries 1`。
Expected: 点 Play、进入新的一关；因为下一关的版面和 1712 不一样，读出来的棋盘多半不对——这时**期望的结果是它自己停下并说清楚**，不是乱走。截图核对它停在哪里、说了什么。如果新一关恰好读得出来，如实记录它走了什么。

- [ ] **Step 3: 把真机上遇到的每个画面的字补进证据**

打开 `~/.dct/games/log/<今天>.jsonl`，找 `"kind":"nav"` 的行，看 `screen` 是 `unknown` 或 `level_ended` 的，把 `texts` 抄下来：这是补白名单的依据。**只在有真实证据时才改词表**（`screen.rs` 的三个列表），每改一个词配一个测试和一个提交，不凭想象加。

- [ ] **Step 4: 失败路线**

找一个会失败的局面（让步数走完），看 `Try again` 是不是真的这个字、点了以后是不是进到「开局框」、两个 Play 的 OCR 怎么读。把真实字样记下来；和假设对不上就先报告，别硬改。

- [ ] **Step 5: 急停**

玩的时候让 dco 急停（`dco call halt`），确认它在下一步之前停下、说「dco 急停了」，退出码 1；之后 `dco call resume`。

- [ ] **Step 6: 三个 AI 都会用**

确认说明卡已更新（守护进程在跑、三个目录都存在时 30 秒内会更新；否则手动把 `src/game/skill.md` 放到 `~/.claude/skills/dct-game/SKILL.md`、`~/.codex/skills/dct-game/SKILL.md`、`~/.qwen/skills/dct-game/SKILL.md`，内容要完全一致）。在 Claude Code、Codex、千问里各说一句「帮我多打几局，失败了自己重来」，看它们是不是都运行 `dct game play --auto-next`，并转述每一步；哪一家没反应就如实报告。

- [ ] **Step 7: 告诉 dc-octo 和 dc-vault 会话**

用 SendMessage：第二轮第一步做完了、真机上看到了哪些画面（`texts`）、哪些词要补；dc-octo 那边关心开局框和广告的真实字样。
