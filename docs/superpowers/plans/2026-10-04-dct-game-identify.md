# dct 快速鉴定「这是哪一类游戏」 Design + Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans. Steps use checkbox (`- [ ]`) syntax.

**Goal:** `dct game identify` 在几十毫秒内、只靠本机，说出当前画面是「方格消除类」「寻物类」还是「看不出来」，并用一句大白话告诉用户 dct 会不会玩。

**Architecture:** 新增 `crates/dct-game/src/genre.rs`（纯函数，不碰 dco）：`classify(texts, board_ok) -> Genre`。线索表是**类别的通用线索**（不是某个游戏的），放在同一个文件顶部的常量表里，方便以后搬去 dcv。`src/game/` 加 `identify` 子命令：用 dco 的 `see`（文字识别）和 `read_grid`（棋盘检查）各调一次，把结果交给 `classify`，打印一句话。第三层「把截图发给模型问」不在本轮（需要给 llm 层加图片通道，另一个功能）。

**Tech Stack:** Rust（workspace `dct` + `crates/dct-game`）。

## 为什么这样分层（2026-10-04 与用户讨论的结论）

dco 自己只会认字、框图标、读颜色格子，不懂内容，不能直接说「这是寻物游戏」。能快速判断的办法是：
1. 读得出合格棋盘 → 方格消除类（dct 已有的 `looks_like_board`）；
2. 读屏幕上的字，命中类别的特征词 → 寻物类；
3. 都不确定 → 看不出（以后可以问会看图的模型，本轮不做）。

## Global Constraints

- 提交信息用英文，**不加任何 AI 署名或 Co-Authored-By 行**（用户的长期规则）。不推送，用户说「推吧」才推；用户说过便利优先，所以合并和推送前不再逐项问。
- 面向用户的文字用大白话中文。三句输出固定为：
  - 方格消除：「这是方格消除类游戏，我会玩。」
  - 寻物：「这看起来是寻物类游戏，我现在还不会玩。」
  - 看不出：「我看不出这是哪一类游戏。」
- dct-game 保持通用：线索是**游戏类别**的通用词，不含任何具体游戏名；线索表是数据（const 表），不散落在逻辑里。
- 不发图、不联网：整个鉴定只用 dco 的文字识别和棋盘读取。
- 读不到文字（旧 dco 不支持 see）：只看棋盘；棋盘也读不出 → 「看不出」，不报错、退出码 0。
- 推送前跑：`cargo +1.99.0 clippy --workspace --all-targets --locked -- -D warnings` 和 `cargo +1.99.0 test --workspace --locked`（`PATH` 含 `$HOME/.cargo/bin`，`CARGO_TARGET_DIR=/private/tmp/claude-502/-Users-lei-work-dc-dc-terminal/d52f1f4f-cb49-40cc-ba88-c37403102743/scratchpad/target-199`）。已知偶发失败：`daemon::web_tests::enabling_starts_a_listener_and_disabling_stops_it`、`session::tests::recovering_from_a_failure_after_real_input_still_does_not_count`、`zombie_reaping`，重跑。

## Review Focus

1. 方格消除游戏的主菜单也会出现 PLAY / OPTIONS / EXTRAS 这类字：不能因为弱线索就判成寻物。→ Task 1。
2. 画面上同时有棋盘和寻物线索词（比如消除游戏里弹出「Hint」）：棋盘优先，判方格消除。→ Task 1。
3. 文字全空 + 棋盘读不出（黑屏、加载中）：「看不出」，不是寻物。→ Task 1/2。
4. 线索词大小写、全角半角、前后空白：不影响命中。→ Task 1。

---

### Task 1: `genre.rs`

**Files:**
- Create: `crates/dct-game/src/genre.rs`
- Modify: `crates/dct-game/src/lib.rs`（`pub mod genre;`）
- Test: `genre.rs` 内 `tests` 模块

**Interfaces:**
- Produces: `#[derive(Clone, Copy, Debug, PartialEq, Eq)] pub enum Genre { Match3, HiddenObject, Unknown }`；`pub fn classify(texts: &[String], board_ok: Option<bool>) -> Genre`（`board_ok`：`Some(true)` 读出了合格棋盘，`Some(false)` 没读出，`None` 没试）；`pub fn sentence(g: Genre) -> &'static str`（上面三句固定文字）。

规则：
1. `board_ok == Some(true)` → `Match3`（最优先）。
2. 否则把每条文字规整（去前后空白、转小写、全角字母数字转半角）。命中任一**强线索**，或命中至少两个**不同的弱线索** → `HiddenObject`。
3. 否则 `Unknown`。

线索表（数据，放在文件顶部）：
- 强：`collector's edition`、`hidden object`、`inventory`、`journal`、`物品栏`、`寻物`、`收藏版`
- 弱：`hint`、`map`、`extras`、`find`、`skip`、`提示`、`地图`、`跳过`
命中按「规整后的文字**包含**线索词」判断；同一个线索词出现多次只算一个。

- [ ] **Step 1: 写失败的测试**

```rust
#[cfg(test)]
mod tests {
    use super::*;

    fn t(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn a_readable_board_is_match3_even_with_hidden_object_words_on_screen() {
        assert_eq!(classify(&t(&["Hint", "Inventory", "Journal"]), Some(true)), Genre::Match3);
    }

    #[test]
    fn one_strong_cue_is_enough() {
        assert_eq!(classify(&t(&["Enemy in Reflection", "COLLECTOR'S EDITION", "PLAY"]), Some(false)), Genre::HiddenObject);
        assert_eq!(classify(&t(&["物品栏"]), None), Genre::HiddenObject);
    }

    #[test]
    fn one_weak_cue_alone_is_not_enough_but_two_different_ones_are() {
        // 方格消除游戏的主菜单也有 PLAY / OPTIONS / EXTRAS 这类字
        assert_eq!(classify(&t(&["PLAY", "OPTIONS", "EXTRAS"]), Some(false)), Genre::Unknown);
        assert_eq!(classify(&t(&["Hint"]), Some(false)), Genre::Unknown);
        assert_eq!(classify(&t(&["Hint", "Map"]), Some(false)), Genre::HiddenObject);
        // 同一个词出现多次只算一个
        assert_eq!(classify(&t(&["Hint", "hint", "HINT"]), Some(false)), Genre::Unknown);
    }

    #[test]
    fn matching_ignores_case_whitespace_and_full_width_letters() {
        assert_eq!(classify(&t(&["  Ｉｎｖｅｎｔｏｒｙ "]), None), Genre::HiddenObject);
        assert_eq!(classify(&t(&["Hidden   Object"]), None), Genre::Unknown, "中间多空格不是线索词，不乱猜");
        assert_eq!(classify(&t(&["HIDDEN OBJECT"]), None), Genre::HiddenObject);
    }

    #[test]
    fn nothing_readable_and_no_board_is_unknown_not_hidden_object() {
        assert_eq!(classify(&[], Some(false)), Genre::Unknown);
        assert_eq!(classify(&[], None), Genre::Unknown);
        assert_eq!(classify(&t(&["", "  "]), Some(false)), Genre::Unknown);
    }

    #[test]
    fn the_three_sentences_are_fixed_plain_chinese() {
        assert_eq!(sentence(Genre::Match3), "这是方格消除类游戏，我会玩。");
        assert_eq!(sentence(Genre::HiddenObject), "这看起来是寻物类游戏，我现在还不会玩。");
        assert_eq!(sentence(Genre::Unknown), "我看不出这是哪一类游戏。");
    }
}
```

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo +1.99.0 test -p dct-game genre::`
Expected: 编译失败（`classify` 未定义）。

- [ ] **Step 3: 写实现**

```rust
//! 快速鉴定「这是哪一类游戏」：只用本机能得到的东西（有没有合格棋盘、屏幕上的字）。
//! 线索是游戏**类别**的通用词，不含任何具体游戏；放在下面的表里，以后可以搬去 dcv。
//! 不确定就说看不出，绝不乱猜。

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Genre {
    Match3,
    HiddenObject,
    Unknown,
}

/// 命中任一个就判寻物。
const HIDDEN_STRONG: &[&str] = &["collector's edition", "hidden object", "inventory", "journal", "物品栏", "寻物", "收藏版"];
/// 至少命中两个**不同**的才判寻物（消除游戏的主菜单也有 play / options / extras）。
const HIDDEN_WEAK: &[&str] = &["hint", "map", "extras", "find", "skip", "提示", "地图", "跳过"];

/// 去前后空白、转小写、把全角字母数字转成半角。中间的空白不动（「hidden   object」不是线索词）。
fn normalise(s: &str) -> String {
    s.trim()
        .chars()
        .map(|c| match c {
            '\u{FF01}'..='\u{FF5E}' => char::from_u32(c as u32 - 0xFEE0).unwrap_or(c),
            _ => c,
        })
        .collect::<String>()
        .to_lowercase()
}

pub fn classify(texts: &[String], board_ok: Option<bool>) -> Genre {
    if board_ok == Some(true) {
        return Genre::Match3;
    }
    let norm: Vec<String> = texts.iter().map(|t| normalise(t)).filter(|t| !t.is_empty()).collect();
    let hits = |table: &[&str]| table.iter().filter(|cue| norm.iter().any(|t| t.contains(**cue))).count();
    if hits(HIDDEN_STRONG) >= 1 || hits(HIDDEN_WEAK) >= 2 {
        Genre::HiddenObject
    } else {
        Genre::Unknown
    }
}

pub fn sentence(g: Genre) -> &'static str {
    match g {
        Genre::Match3 => "这是方格消除类游戏，我会玩。",
        Genre::HiddenObject => "这看起来是寻物类游戏，我现在还不会玩。",
        Genre::Unknown => "我看不出这是哪一类游戏。",
    }
}
```

- [ ] **Step 4: 跑测试**

Run: `cargo +1.99.0 test -p dct-game genre::` 与 clippy。
Expected: 全绿，无警告。

- [ ] **Step 5: 变异检查，然后提交**

把 `hits(HIDDEN_WEAK) >= 2` 改成 `>= 1` → `one_weak_cue_alone…` 变红；把 `board_ok == Some(true)` 的提前返回删掉 → `a_readable_board…` 变红；去掉 `to_lowercase` → 大小写测试变红。改回。

```bash
git add crates/dct-game/src/genre.rs crates/dct-game/src/lib.rs
git commit -m "feat(game): classify the screen as match-3, hidden-object or unknown from a readable board and a small table of genre cue words"
```

---

### Task 2: `dct game identify`

**Files:**
- Modify: `src/game/cli.rs`（子命令 `identify`，和 `ask-bench` 一样在 `parse` 之前分流）、`src/game/mod.rs`（若需要新文件 `identify.rs` 则登记）、`src/main.rs`（`dct --help` 加一行）、`src/game/skill.md`（说明卡加一句）
- Test: `src/game/cli.rs` 或新文件 `src/game/identify.rs` 的测试；若有 `tests/game_cli.rs` 的假 dco 集成测试风格，沿用

**Interfaces:**
- Consumes: Task 1 的 `Genre`、`classify`、`sentence`；dct-game 的 `Dco` trait（`read_grid`、`see_text`）、`Profile`、`looks_like_board`、`DcoClient::connect`；`profile::load(&home, game)`。
- Produces: `pub fn identify(dco: &mut dyn Dco, p: &Profile) -> Genre`（纯函数风格，便于用假 dco 测）：
  1. `see_text` → 取所有元素的 `text`，失败就空列表；
  2. `read_grid` → `Ok(g)` 且 `looks_like_board(&g)` 为 `Some(true)`，`Err` 或不像棋盘为 `Some(false)`；
  3. `classify`。
  命令 `dct game identify [--game 名字]`：加载该游戏的配置（默认 `candy-crush`，用它的区域和行列去试读棋盘）、连 dco、调 `identify`、打印 `sentence` 一句话，退出码 0（dco 没运行、窗口没找到：打印和 `play` 一样的 `text::dco_error` 并退出 1）。

- [ ] **Step 1: 写失败的测试**

用假 dco（参照 `crates/dct-game` 里 `Fake`/`play_tests.rs` 的写法；`identify` 在主 crate，所以在 `src/game/identify.rs` 里写一个最小假 `Dco` 实现）：

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use dct_game::board::GridRead;
    use dct_game::genre::Genre;
    use dct_game::play::{Dco, DcoError, Profile, Seen};
    use dct_game::screen::Element;
    use serde_json::json;

    struct Fake {
        texts: Option<Vec<&'static str>>,
        grid: Result<GridRead, DcoError>,
    }
    impl Dco for Fake {
        fn read_grid(&mut self, _: &Profile) -> Result<GridRead, DcoError> {
            self.grid.clone()
        }
        fn swipe(&mut self, _: &Profile, _: (f64, f64), _: (f64, f64)) -> Result<(), DcoError> {
            panic!("identify must never swipe");
        }
        fn see_text(&mut self, _: &Profile) -> Result<Seen, DcoError> {
            match &self.texts {
                Some(t) => Ok(Seen {
                    snapshot_id: "s".into(),
                    observation_id: None,
                    elements: t.iter().enumerate().map(|(i, s)| Element { id: format!("e{i}"), text: (*s).into() }).collect(),
                }),
                None => Err(DcoError { code: "unsupported".into(), message: "x".into() }),
            }
        }
    }

    fn profile() -> Profile {
        Profile { window: json!({"app": "x"}), region: [0.0, 0.0, 1.0, 1.0], rows: 3, cols: 4, extra: json!({}), fixed_rgb: vec![], match_de: 24.0, weights: Default::default(), level_pattern: None }
    }
    fn no_grid() -> Result<GridRead, DcoError> {
        Err(DcoError { code: "not_a_grid".into(), message: "m".into() })
    }
    fn good_grid() -> Result<GridRead, DcoError> {
        // 3 行 4 列，三种颜色，每种至少两格，没有一种超过 70%
        Ok(serde_json::from_value(json!({
            "rows": 3, "cols": 4,
            "cells": [[1,1,2,3],[2,3,1,1],[3,2,3,2]],
            "odd": [[false;4],[false;4],[false;4]],
            "classes": [{"id":1,"count":4},{"id":2,"count":4},{"id":3,"count":4}]
        }))
        .unwrap())
    }

    #[test]
    fn a_readable_board_is_match3() {
        let mut d = Fake { texts: Some(vec!["Hint"]), grid: good_grid() };
        assert_eq!(identify(&mut d, &profile()), Genre::Match3);
    }

    #[test]
    fn a_strong_cue_and_no_board_is_hidden_object() {
        let mut d = Fake { texts: Some(vec!["COLLECTOR'S EDITION", "PLAY"]), grid: no_grid() };
        assert_eq!(identify(&mut d, &profile()), Genre::HiddenObject);
    }

    #[test]
    fn text_unsupported_falls_back_to_the_board_only() {
        let mut d = Fake { texts: None, grid: good_grid() };
        assert_eq!(identify(&mut d, &profile()), Genre::Match3);
        let mut d = Fake { texts: None, grid: no_grid() };
        assert_eq!(identify(&mut d, &profile()), Genre::Unknown);
    }

    #[test]
    fn nothing_readable_is_unknown() {
        let mut d = Fake { texts: Some(vec![]), grid: no_grid() };
        assert_eq!(identify(&mut d, &profile()), Genre::Unknown);
    }
}
```

（`looks_like_board` 的真实签名和「合格棋盘」的判断以 `crates/dct-game/src/board.rs` 为准；若上面 `good_grid` 的数据过不了它，调整数据直到过，断言不变。`Weights` 若没有 `Default` 就用 `Weights::default()` 的实际写法，参照 `play_tests.rs` 的 `profile()`。）

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo +1.99.0 test --lib game::identify`
Expected: 编译失败。

- [ ] **Step 3: 写实现**

`identify`：

```rust
pub fn identify(dco: &mut dyn Dco, p: &Profile) -> Genre {
    let texts: Vec<String> = dco.see_text(p).map(|s| s.elements.into_iter().map(|e| e.text).collect()).unwrap_or_default();
    let board_ok = Some(dco.read_grid(p).map(|g| looks_like_board(&g)).unwrap_or(false));
    classify(&texts, board_ok)
}
```

`dct game identify [--game 名字]`：在 `run()` 里和 `ask-bench` 一样先分流；加载配置、连 dco（失败时用 `text::dco_error` 打印、退出 1）、调 `identify`、`println!("{}", sentence(g))`、退出 0。`dct --help` 加一行：`dct game identify    看看现在屏幕上是哪一类游戏（只在本机识别，不上传）`。`skill.md` 加一句大白话：不知道打开的是什么游戏时先运行 `dct game identify`。

- [ ] **Step 4: 跑全部测试和 clippy**

Run: `cargo +1.99.0 test --workspace --locked` 与 clippy。
Expected: 全绿，无警告。

- [ ] **Step 5: 变异检查，然后提交**

把 `identify` 里 `looks_like_board` 的结果取反 → 前两个测试变红；把 `see_text` 的失败当成 panic（`unwrap()`）→ `text_unsupported_falls_back…` 变红。改回。

```bash
git add src crates
git commit -m "feat(game): dct game identify says which kind of game is on screen, using only local text recognition and the board check"
```

---

## Self-Review

1. **覆盖：** 三层里的前两层（棋盘、文字线索）→ Task 1+2；第三层（问模型）明确不在本轮，已写在 Architecture。
2. **占位：** `looks_like_board`/`Weights::default` 的确切写法让实现者以现有代码为准，已写明断言不变。
3. **类型一致：** `Genre` / `classify` / `sentence`（Task 1）在 Task 2 里原样使用；`identify` 在 Task 2 内定义并测试。
4. **Review Focus：** 四条分别落在 Task 1（弱线索、棋盘优先、规整化）和 Task 1/2（全空不是寻物）。
