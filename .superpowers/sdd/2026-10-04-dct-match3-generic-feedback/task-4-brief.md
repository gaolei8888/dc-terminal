### Task 4: 读进度数字

**Files:**
- Modify: `crates/dct-game/src/play.rs`
- Test: `crates/dct-game/src/play_tests.rs`

**Interfaces:**
- Consumes: `Dco::see_text(&mut self, &Profile) -> Result<Seen, DcoError>`（默认返回 unsupported）；`Seen.elements: Vec<Element{id,text}>`。
- Produces: `pub(crate) fn numbers(seen: &Seen) -> Vec<u64>`（只取**整条文字全是数字**的元素，按出现顺序）；记录字段 `progress`：`{"before": [..]|null, "after": [..]|null}`；循环状态 `progress_prev: Option<Vec<u64>>`。

- [ ] **Step 1: 写失败的测试**

`Fake` 要支持 `see_text`：给 `Fake` 加字段 `texts: VecDeque<Vec<&'static str>>`（默认空），并实现

```rust
    fn see_text(&mut self, _: &Profile) -> Result<Seen, DcoError> {
        match self.texts.pop_front() {
            Some(t) => Ok(Seen {
                snapshot_id: "s".into(),
                observation_id: None,
                elements: t.iter().enumerate().map(|(i, s)| crate::screen::Element { id: format!("e{}", i + 1), text: (*s).into() }).collect(),
            }),
            None => Err(DcoError { code: "unsupported".into(), message: "x".into() }),
        }
    }
```

（`Fake::new` 里 `texts: VecDeque::new()`；已有测试不受影响。）测试：

```rust
#[test]
fn numbers_keeps_only_whole_number_elements_in_order() {
    let seen = Seen {
        snapshot_id: "s".into(),
        observation_id: None,
        elements: ["1716/♥5", "38", "x", "122", " 7 "].iter().enumerate().map(|(i, s)| crate::screen::Element { id: format!("e{i}"), text: (*s).into() }).collect(),
    };
    assert_eq!(numbers(&seen), vec![38, 122, 7]);
}

#[test]
fn progress_is_recorded_before_and_after_each_step() {
    let mut d = Fake::new(settled_script());
    // 一次循环前读一回（起始），每步划完读一回
    d.texts = vec![vec!["50", "30"], vec!["49", "30"]].into();
    let (_, log) = run(&mut d, 1, false);
    assert_eq!(log[0]["progress"]["before"], json!([50, 30]));
    assert_eq!(log[0]["progress"]["after"], json!([49, 30]));
}

#[test]
fn progress_is_null_when_the_dco_cannot_read_text() {
    let mut d = Fake::new(settled_script()); // texts 为空 → unsupported
    let (s, log) = run(&mut d, 1, false);
    assert!(log[0]["progress"]["before"].is_null());
    assert!(log[0]["progress"]["after"].is_null());
    assert_eq!(s.stop, Stop::StepsDone);
}
```

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo +1.99.0 test -p dct-game play_tests::numbers play_tests::progress`
Expected: 编译失败（`numbers` 未定义）。

- [ ] **Step 3: 写实现**

```rust
/// 画面上整条文字就是一个数字的元素（顶部的目标数、步数……），按出现顺序。
/// 「1716/♥5」这种夹着别的字的不要。哪个数是目标，这里不猜；只记下来。
pub(crate) fn numbers(seen: &Seen) -> Vec<u64> {
    seen.elements.iter().filter_map(|e| e.text.trim().parse::<u64>().ok()).collect()
}
```

`play()`：循环前（`let stop = loop {` 之前）`let mut progress_prev: Option<Vec<u64>> = if o.dry_run { None } else { dco.see_text(p).ok().map(|s| numbers(&s)) };`。

两种结果分支（成功 / 没反应）写记录前各读一次：

```rust
let progress_after = dco.see_text(p).ok().map(|s| numbers(&s));
rec["progress"] = json!({ "before": progress_prev, "after": progress_after });
progress_prev = progress_after.clone();
```

（放进一个小闭包或内联；`stopped` 类的分支不读。`dry_run` 分支直接 `break`，不读。）

- [ ] **Step 4: 跑测试**

Run: `cargo +1.99.0 test --workspace --locked`
Expected: 全绿（其它假 dco 不支持 `see_text`，字段是 `null`，行为不变）。

- [ ] **Step 5: 变异检查，然后提交**

把 `numbers` 的 `trim()` 去掉 → `numbers_keeps_only…` 变红（`" 7 "`）；把 `progress_prev = progress_after.clone()` 删掉 → `progress_is_recorded…` 在多步时的 before 会错（若当前测试只有一步没抓到，加一个两步的断言）。改回。

```bash
git add crates/dct-game/src
git commit -m "feat(game): record the whole-number texts on screen before and after each step as progress"
```

---

