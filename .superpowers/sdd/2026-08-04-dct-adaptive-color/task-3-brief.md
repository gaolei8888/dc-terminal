### Task 3: 带超时的 stdin 读取与四级探测链

把 Task 2 的解析函数按优先级串成 `detect()`。读 stdin 用 `poll(2)` 做超时，藏在一个 trait 后面，好让探测链本身能在测试里跑完整四级降级而不碰真终端。

**Files:**
- Modify: `src/theme.rs`

**Interfaces:**
- Consumes: Task 1 的 `Theme`；Task 2 的 `is_light` / `parse_osc11` / `parse_colorfgbg` / `theme_from_override`
- Produces:
  - `pub(crate) trait ReplyReader { fn read_reply(&mut self, deadline: Duration) -> Vec<u8>; }`
  - `pub(crate) fn detect_with<R: ReplyReader>(reader: &mut R, dct_theme: Option<&str>, colorfgbg: Option<&str>) -> Theme`
  - `pub fn detect() -> Theme` —— 读真环境变量 + 真 stdin，`ui.rs` 用这个

- [ ] **Step 1: 写失败的测试**

在 `src/theme.rs` 的 `mod tests` 里追加（仍在同一个 `mod tests` 内，右花括号之前）：

```rust
    /// 测试用的假读端：按剧本返回一段预设回复，或者返回空（= 终端一声不响，
    /// 真实世界里就是读到超时）。
    struct CannedReader {
        reply: Vec<u8>,
        calls: usize,
    }

    impl CannedReader {
        fn answering(reply: &[u8]) -> Self {
            CannedReader { reply: reply.to_vec(), calls: 0 }
        }
        /// 不答 OSC 11 的终端，读到超时拿到空字节
        fn silent() -> Self {
            CannedReader { reply: Vec::new(), calls: 0 }
        }
    }

    impl ReplyReader for CannedReader {
        fn read_reply(&mut self, _deadline: Duration) -> Vec<u8> {
            self.calls += 1;
            self.reply.clone()
        }
    }

    /// 第一级：环境变量指定了就用它，而且**不去查询终端**——用户已经明确
    /// 说了答案，再花 150ms 去问一遍是白等。
    #[test]
    fn override_wins_and_skips_the_query() {
        let mut r = CannedReader::answering(b"\x1b]11;rgb:ffff/ffff/ffff\x07");
        assert_eq!(detect_with(&mut r, Some("dark"), None), Theme::Dark);
        assert_eq!(r.calls, 0, "环境变量已经给出答案，不该再查询终端");
    }

    /// 环境变量还要压过 COLORFGBG。
    #[test]
    fn override_wins_over_colorfgbg() {
        let mut r = CannedReader::silent();
        assert_eq!(detect_with(&mut r, Some("light"), Some("15;0")), Theme::Light);
    }

    /// 第二级：OSC 11 答了就用它的结果。
    #[test]
    fn uses_osc11_reply_when_terminal_answers() {
        let mut dark = CannedReader::answering(b"\x1b]11;rgb:0000/2b2b/3636\x07");
        assert_eq!(detect_with(&mut dark, None, None), Theme::Dark);

        let mut light = CannedReader::answering(b"\x1b]11;rgb:fdfd/f6f6/e3e3\x07");
        assert_eq!(detect_with(&mut light, None, None), Theme::Light);
    }

    /// OSC 11 还要压过 COLORFGBG：问到终端本人的答案比环境变量里的陈旧线索可信
    /// （COLORFGBG 是登录时设的，用户中途换了配色它不会更新）。
    #[test]
    fn osc11_wins_over_colorfgbg() {
        let mut r = CannedReader::answering(b"\x1b]11;rgb:fdfd/f6f6/e3e3\x07");
        assert_eq!(detect_with(&mut r, None, Some("15;0")), Theme::Light);
    }

    /// 第三级：终端不答（超时读到空）就退回 COLORFGBG。
    #[test]
    fn falls_back_to_colorfgbg_when_terminal_is_silent() {
        let mut r = CannedReader::silent();
        assert_eq!(detect_with(&mut r, None, Some("15;0")), Theme::Dark);
        assert_eq!(detect_with(&mut CannedReader::silent(), None, Some("0;15")), Theme::Light);
    }

    /// 回复格式不对，也要能一路降到 COLORFGBG，而不是就地放弃。
    #[test]
    fn falls_back_to_colorfgbg_when_reply_is_garbage() {
        let mut r = CannedReader::answering(b"\x1b]11;rgb:zz/zz/zz\x07");
        assert_eq!(detect_with(&mut r, None, Some("0;15")), Theme::Light);
    }

    /// 第四级：什么线索都没有就是 Unknown。这必须是一个正常出口，
    /// 不是错误——`Unknown.dim()` 本身就是能用的样式。
    #[test]
    fn unknown_when_nothing_answers() {
        let mut r = CannedReader::silent();
        assert_eq!(detect_with(&mut r, None, None), Theme::Unknown);
    }

    /// 三级全是垃圾输入的组合拳：一样只能落到 Unknown，不许 panic。
    #[test]
    fn garbage_at_every_level_lands_on_unknown() {
        let mut r = CannedReader::answering(b"not an osc reply");
        assert_eq!(detect_with(&mut r, Some("mauve"), Some("not;numbers")), Theme::Unknown);
    }
```

- [ ] **Step 2: 跑测试确认失败**

Run: `export PATH="$HOME/.cargo/bin:$PATH" && cargo test --lib theme 2>&1 | head -20`
Expected: 编译错误，`cannot find trait ReplyReader` / `cannot find function detect_with`。

- [ ] **Step 3: 写最小实现**

在 `src/theme.rs` 顶部的 `use` 块里补两行：

```rust
use std::io::{Read, Write};
use std::time::{Duration, Instant};
```

然后在 `theme_from_override` 之后、`#[cfg(test)] mod tests` 之前插入：

```rust
/// OSC 11 查询的最长等待。不答这个查询的终端只付一次性的 150ms 启动代价，
/// 而不是挂在那里等。本地终端的往返是亚毫秒级，150ms 绰绰有余；对用户来说
/// 也还在「启动」这个心理窗口里面。
const QUERY_TIMEOUT: Duration = Duration::from_millis(150);

/// 把「发查询、在 deadline 内读回复」抽出来，只为了让 `detect_with` 能在
/// 测试里跑完整的四级降级——真实实现要一个 tty 和一个会答话的终端，
/// 两样都不该是单元测试的前提。
pub(crate) trait ReplyReader {
    /// 返回读到的字节；什么都没读到（超时、不是 tty、读失败）就返回空 Vec。
    /// **不返回 Result**：调用方对所有失败的处理都一样——降级，
    /// 用错误类型区分它们只会诱导出没人需要的分支。
    fn read_reply(&mut self, deadline: Duration) -> Vec<u8>;
}

/// 真实实现：往 stdout 写 OSC 11 查询，用 `poll(2)` 在 deadline 内读 stdin。
///
/// 必须在 `enable_raw_mode()` 之后用：非 raw 模式下这段回复会被行缓冲
/// （它不带换行，读不出来）并且被回显到屏幕上（用户会看见一串乱码）。
pub(crate) struct StdinReader;

impl ReplyReader for StdinReader {
    fn read_reply(&mut self, deadline: Duration) -> Vec<u8> {
        let mut out = std::io::stdout();
        // 写失败（stdout 被重定向/关闭）就没有查询可言，直接空手而归
        if out.write_all(b"\x1b]11;?\x07").is_err() || out.flush().is_err() {
            return Vec::new();
        }

        let start = Instant::now();
        let mut buf = Vec::new();
        loop {
            let Some(left) = deadline.checked_sub(start.elapsed()) else {
                // 超时。buf 里可能有半个回复，照样交出去——`parse_osc11`
                // 要求终止符必须在，残缺的会被它判成 None。
                return buf;
            };

            if !stdin_is_readable(left) {
                return buf;
            }

            let mut chunk = [0u8; 64];
            match std::io::stdin().read(&mut chunk) {
                Ok(0) | Err(_) => return buf,
                Ok(n) => {
                    buf.extend_from_slice(&chunk[..n]);
                    // 收到终止符就够了，不等满 deadline
                    if buf.contains(&0x07) || buf.windows(2).any(|w| w == b"\x1b\\") {
                        return buf;
                    }
                    // 封顶：用户在界面出来之前狂敲键盘的话，这里会一直有
                    // 字节可读。读满就走，不能让探测卡在一个喂不完的输入上。
                    if buf.len() >= 256 {
                        return buf;
                    }
                }
            }
        }
    }
}

/// stdin 在 `timeout` 内是否可读。`poll(2)` 而不是起线程去阻塞读：
/// 那个线程超时后仍卡在 `read` 上，之后会跟事件循环抢 stdin，把用户的
/// 按键吃掉——一个只在「终端不答 OSC 11」时才发作的偷键 bug。
fn stdin_is_readable(timeout: Duration) -> bool {
    let mut fd = libc::pollfd {
        fd: libc::STDIN_FILENO,
        events: libc::POLLIN,
        revents: 0,
    };
    // 上取整到毫秒：截断成 0 会让 poll 变成非阻塞轮询，在极短的剩余时间里
    // 空转。毫秒级的多等对 150ms 的总预算无关紧要。
    let ms = timeout.as_millis().min(i32::MAX as u128) as i32;
    let ms = if ms == 0 { 1 } else { ms };
    // 失败（含被信号打断的 EINTR）当成「没数据」：调用方会因此降级，
    // 而重试要另写一套超时记账，为一个 150ms 的尽力而为的查询不值得。
    unsafe { libc::poll(&mut fd, 1, ms) > 0 }
}

/// 按优先级探测背景深浅。四级降级的顺序和理由见设计文档。
///
/// 环境变量和读端都从参数进来，所以这个函数是可测的、也是纯粹的调度逻辑：
/// 不碰进程环境（`set_var` 是进程级的，并行测试之间会互相踩），不碰真 stdin。
pub(crate) fn detect_with<R: ReplyReader>(
    reader: &mut R,
    dct_theme: Option<&str>,
    colorfgbg: Option<&str>,
) -> Theme {
    // 1. 用户明说了就照办，而且不再去查询终端——他已经给了答案。
    if let Some(t) = theme_from_override(dct_theme) {
        return t;
    }

    // 2. 问终端本人。比 COLORFGBG 可信：那个变量是登录时设的，用户中途
    //    换了配色它不会更新。
    if let Some((r, g, b)) = parse_osc11(&reader.read_reply(QUERY_TIMEOUT)) {
        return if is_light(r, g, b) { Theme::Light } else { Theme::Dark };
    }

    // 3. 不答 OSC 11 的终端（rxvt/urxvt/konsole）留下的线索。
    if let Some(t) = colorfgbg.and_then(parse_colorfgbg) {
        return t;
    }

    // 4. 没有任何线索。不是错误——`Unknown.dim()` 是能用的样式。
    Theme::Unknown
}

/// `detect_with` 的生产入口：接真环境变量和真 stdin。
///
/// 必须在 `enable_raw_mode()` 之后、`EnterAlternateScreen` 之前调，
/// 两头都是硬约束，理由见 `ui.rs` 里调用点的注释。
pub fn detect() -> Theme {
    let dct = std::env::var("DCT_THEME").ok();
    let fgbg = std::env::var("COLORFGBG").ok();
    detect_with(&mut StdinReader, dct.as_deref(), fgbg.as_deref())
}
```

- [ ] **Step 4: 跑测试确认通过**

Run: `export PATH="$HOME/.cargo/bin:$PATH" && cargo test --lib theme`
Expected: 20 个测试全 PASS（前两个 task 的 12 个 + 这一轮的 8 个），0 failed。

Run: `export PATH="$HOME/.cargo/bin:$PATH" && cargo build 2>&1 | grep warning; echo "--- warnings above (none expected)"`
Expected: 没有 warning 行。

- [ ] **Step 5: 提交**

```bash
git add src/theme.rs
git commit -m "feat: detect terminal background via OSC 11 with poll(2) timeout

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>"
```

---

