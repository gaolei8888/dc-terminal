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

