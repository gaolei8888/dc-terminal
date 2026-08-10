### Task 3: 采集第一句用户输入

**Files:**
- Modify: `src/session.rs`（`Session` 加两个字段；`send_input` 开头调一次；新增自由函数 `collect_first_input`）
- Test: `src/session.rs` 的 `mod tests`

**Interfaces:**
- Produces:
  - `const FIRST_INPUT_MAX: usize = 200;`
  - `pub(crate) fn collect_first_input(buf: &mut String, sealed: &mut bool, text: &str)`
  - `Session.first_input: String` / `Session.first_input_sealed: bool`

两个客户端送输入的形状不同，都要接住：**会话视图**逐键转发，回车到达时 `text` 是 `"\r"`；
**九宫格 `i` 回一句**先发整段 body，再发一次**空 `Input`**（空 = 按回车，见 `src/ui/grid.rs:600-612`）。

- [ ] **Step 1: 写失败测试**

```rust
    /// 逐键送和整段送必须封存出同一句话 —— 会话视图是一个键一次
    /// `Input`，九宫格 `i` 是整段 + 一次空 `Input`。
    #[test]
    fn both_input_paths_seal_the_same_first_line() {
        let mut a = (String::new(), false);
        for k in ["h", "i", "\r"] {
            collect_first_input(&mut a.0, &mut a.1, k);
        }

        let mut b = (String::new(), false);
        collect_first_input(&mut b.0, &mut b.1, "hi");
        collect_first_input(&mut b.0, &mut b.1, "");

        assert_eq!(a.0, "hi");
        assert_eq!(b.0, "hi");
        assert!(a.1 && b.1, "两条路都要封存");
    }

    /// 封存之后再送字，第一句不再变 —— 它是「第一句」，不是「最近一句」。
    #[test]
    fn sealed_first_input_never_changes_again() {
        let mut buf = String::new();
        let mut sealed = false;
        collect_first_input(&mut buf, &mut sealed, "hi");
        collect_first_input(&mut buf, &mut sealed, "");
        collect_first_input(&mut buf, &mut sealed, "and more");
        assert_eq!(buf, "hi");
    }

    /// 粘一大段需求进来：只留前 200 个字符，剩下的不进内存。
    #[test]
    fn a_pasted_wall_of_text_is_capped() {
        let mut buf = String::new();
        let mut sealed = false;
        collect_first_input(&mut buf, &mut sealed, &"x".repeat(300));
        assert_eq!(buf.chars().count(), FIRST_INPUT_MAX);
        assert!(!sealed, "没按回车就不算封存");
    }

    /// 一次送进来的字里就带着回车（粘贴多行）：回车之前的算第一句，
    /// 回车本身封存。
    #[test]
    fn a_newline_inside_one_chunk_seals_at_the_newline() {
        let mut buf = String::new();
        let mut sealed = false;
        collect_first_input(&mut buf, &mut sealed, "fix login\nand also");
        assert_eq!(buf, "fix login");
        assert!(sealed);
    }
```

- [ ] **Step 2: 跑它，确认它红**

```bash
cargo test --lib session::tests::both_input_paths_seal_the_same_first_line
```

预期：FAIL，编译错误 `cannot find function 'collect_first_input'`。

- [ ] **Step 3: 最小实现**

`src/session.rs`，放在 `explain_prompt` 旁边（都是「喂模型的原料」这一类）：

```rust
/// 第一句输入最多留这么多字符。粘一大段需求时前 200 字足够喂模型，
/// 把几千字留在内存里没有意义。
const FIRST_INPUT_MAX: usize = 200;

/// 攒「用户对这个会话说的第一句话」。
///
/// 抽成自由函数是因为两个客户端送输入的形状完全不同（会话视图逐键、
/// 九宫格整段 + 一次空 `Input`），而这条规则必须对两条路给出同一个答案 ——
/// 那是能测的，`send_input` 里那一圈锁和 PTY 写入不是。
///
/// `text` 为空 = 按回车（见 `send_input` 的文档）。
pub(crate) fn collect_first_input(buf: &mut String, sealed: &mut bool, text: &str) {
    if *sealed {
        return;
    }
    if text.is_empty() {
        *sealed = true;
        return;
    }
    // `find` 给的是字节下标，而 `\r`/`\n` 都是 ASCII，切在这里一定是
    // 合法的字符边界。
    match text.find(['\r', '\n']) {
        Some(i) => {
            append_capped(buf, &text[..i]);
            *sealed = true;
        }
        None => append_capped(buf, text),
    }
}

/// 按**字符数**封顶追加。这里不按显示宽度算：这段字是喂给模型的原料，
/// 不是画在屏幕上的东西，宽度是界面那一侧的事。
fn append_capped(buf: &mut String, text: &str) {
    for ch in text.chars() {
        if buf.chars().count() >= FIRST_INPUT_MAX {
            return;
        }
        buf.push(ch);
    }
}
```

`Session` 结构体加两个字段（挨着 `name_slot`）：

```rust
    /// 用户对这个会话说的第一句话，起名用。只在 agent 会话上攒。
    first_input: String,
    /// 第一句攒完了没有。见 `collect_first_input`。
    first_input_sealed: bool,
```

构造处加：

```rust
            first_input: String::new(),
            first_input_sealed: false,
```

`send_input`（`src/session.rs:441`）开头，取到 `arc` 之后、别的都还没做之前：

```rust
        {
            // 攒第一句。**在所有分支之前**——下面空串那一支会提早 return，
            // 挂在它后面就永远收不到回车。
            let mut s = recover(arc.lock());
            if s.is_agent {
                let (buf, sealed) = (&mut s.first_input, &mut s.first_input_sealed);
                collect_first_input(buf, sealed, text);
            }
        }
```

> 借用两个字段要写成上面那样先解构，直接 `collect_first_input(&mut s.first_input, &mut s.first_input_sealed, text)` 也可以 —— 两者都能过借用检查（不相交字段），取编译器不报错的那个。

- [ ] **Step 4: 跑测试**

```bash
cargo test --lib session
```

预期：四个新测试 PASS。

- [ ] **Step 5: 全量 + 提交**

```bash
cargo fmt && cargo clippy --all-targets -- -D warnings && cargo test
git add src/session.rs
git commit -m "feat: remember the first thing a user says to an agent session

The attached view sends one Input per keystroke and the grid's reply box
sends the whole body plus an empty Input for Enter. One rule has to seal the
same sentence from both, so it lives in a free function that can be tested
without a PTY."
```

---

