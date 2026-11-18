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
