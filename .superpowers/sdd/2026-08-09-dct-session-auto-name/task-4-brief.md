### Task 4: 洗模型返回值 + 起名 prompt

两个纯函数，先独立做出来 —— 它们是这个功能里唯一有「答案对不对」可言的部分。

**Files:**
- Modify: `src/session.rs`（`explain_prompt` 旁边）
- Test: `src/session.rs` 的 `mod tests`

**Interfaces:**
- Produces:
  - `const NAME_MAX_CHARS: usize = 24;`
  - `pub(crate) fn clean_name(raw: &str) -> String`
  - `pub fn name_prompt(first_input: &str, screen: &str) -> crate::llm::Prompt`

- [ ] **Step 1: 写失败测试**

```rust
    /// 模型多半会回一句带标点、带引号的话，不会老老实实只给名字。
    /// 洗不干净的话，格子标题上会出现「「修登录白屏」。」这种东西。
    #[test]
    fn clean_name_strips_quotes_punctuation_and_extra_lines() {
        assert_eq!(clean_name("「修登录白屏」。"), "修登录白屏");
        assert_eq!(clean_name("\"fix login blank\""), "fix login blank");
        assert_eq!(clean_name("修登录白屏\n（这个会话在修登录）"), "修登录白屏");
        assert_eq!(clean_name("  修登录白屏  "), "修登录白屏");
    }

    /// 洗完是空的就当模型没答上来，调用方走兜底。
    #[test]
    fn clean_name_returns_empty_when_there_is_nothing_left() {
        assert_eq!(clean_name(""), "");
        assert_eq!(clean_name("   \n  "), "");
        assert_eq!(clean_name("。。。"), "");
    }

    /// 模型不听话给了一长串：按字符数封顶，别让它撑爆标题。
    #[test]
    fn clean_name_caps_a_runaway_answer() {
        let long = "修".repeat(100);
        assert_eq!(clean_name(&long).chars().count(), NAME_MAX_CHARS);
    }

    /// prompt 必须带上第一句输入和屏幕末尾两样，缺一样模型就只能猜。
    #[test]
    fn name_prompt_carries_both_the_first_line_and_the_screen() {
        let p = name_prompt("修一下登录白屏", "…… 正在改 auth.ts ……");
        assert!(p.user.contains("修一下登录白屏"));
        assert!(p.user.contains("auth.ts"));
        assert!(p.max_tokens <= 64, "起个名字不需要长回答");
    }
```

- [ ] **Step 2: 跑它，确认它红**

```bash
cargo test --lib session::tests::clean_name_strips_quotes_punctuation_and_extra_lines
```

预期：FAIL，编译错误 `cannot find function 'clean_name'`。

- [ ] **Step 3: 最小实现**

```rust
/// 名字最多留这么多字符。**按字符数、不按显示宽度**：守护进程存的是
/// 一段文字，画多宽是界面那一侧按各自的位置算的（见 `widgets::truncate`）。
/// 24 是 12 个汉字，跟 prompt 里要的「不超过 12 个字」对得上。
const NAME_MAX_CHARS: usize = 24;

/// 把模型回的东西洗成一个能直接画在标题上的名字。
///
/// 模型很少老老实实只给名字：会加引号、会加句号、会多写一句解释。
/// 洗不干净的话屏幕上就会出现「「修登录白屏」。」。洗完是空串表示
/// 这次没拿到可用的答案，调用方走兜底。
pub(crate) fn clean_name(raw: &str) -> String {
    const QUOTES: [char; 12] = ['"', '\'', '「', '」', '『', '』', '“', '”', '‘', '’', '《', '》'];
    const TAIL: [char; 12] = ['。', '．', '.', '，', ',', '！', '!', '？', '?', '；', ';', '、'];

    let line = raw.lines().map(str::trim).find(|l| !l.is_empty()).unwrap_or("");
    let line = line.trim_matches(|c: char| QUOTES.contains(&c) || c.is_whitespace());
    let line = line.trim_end_matches(|c: char| TAIL.contains(&c));
    let line = line.trim();
    line.chars().take(NAME_MAX_CHARS).collect()
}

/// 让模型给这个会话起个名字。
///
/// **只送屏幕末尾**，理由同 `explain_prompt`：整屏几千字，又慢又贵，
/// 还容易让模型抓错重点。
///
/// **语言写进 prompt，不做参数**：名字由守护进程生成并钉死，而界面语言
/// 用户随时能切（`l` 键，不重启 daemon）。跟着用户输入的语言走，切界面
/// 语言之后也不会留下一堆对不上的名字。
pub fn name_prompt(first_input: &str, screen: &str) -> crate::llm::Prompt {
    const TAIL: usize = 2000;
    let tail: String = {
        let chars: Vec<char> = screen.chars().collect();
        let start = chars.len().saturating_sub(TAIL);
        chars[start..].iter().collect()
    };
    crate::llm::Prompt {
        system: "给下面这个编程会话起一个名字，好让人在一屏几个会话里认出它。\
                 只回名字本身，不超过 12 个字。说的是这个会话在做的**任务**，\
                 不是它此刻的动作。不要引号、不要标点、不要「任务」「会话」\
                 这类没有信息的词。**用与用户那句话相同的语言。**"
            .into(),
        user: format!("用户说的第一句话：\n{first_input}\n\n屏幕上的最后一段内容：\n\n{tail}"),
        max_tokens: 64,
    }
}
```

- [ ] **Step 4: 跑测试**

```bash
cargo test --lib session::tests::clean_name
cargo test --lib session::tests::name_prompt
```

预期：四个都 PASS。

- [ ] **Step 5: 全量 + 提交**

```bash
cargo fmt && cargo clippy --all-targets -- -D warnings && cargo test
git add src/session.rs
git commit -m "feat: ask a model for a session name, and scrub what comes back

Models answer with quotes, a full stop, and often a second sentence of
explanation. Unscrubbed that lands in a tile title verbatim."
```

---

