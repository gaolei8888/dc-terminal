# Task 9 报告：鼠标事件编码

## 改了什么，在哪

- `src/pty.rs`
  - 新增纯函数 `encode_mouse(mode, enc, ev) -> Option<Vec<u8>>`，紧跟在
    `impl Drop for PtySession` 之前、`PtySession` impl 块之后。
  - 把 `PtySession::write_mouse` 的空壳占位实现（`Ok(())`，标了
    `// Task 9 实现：空壳先让 ... 编译得过、跑得起来`）替换成真实实现：
    在持锁的块里读 `screen.mouse_protocol_mode()` /
    `screen.mouse_protocol_encoding()`、调用 `encode_mouse`，块结束后锁
    释放，再在锁外调用 `self.write(&b)`。占位注释已删除。
  - 在 `#[cfg(test)] mod tests` 末尾，`an_app_that_asks_for_the_mouse_owns_the_scrolling`
    测试之后，逐字加入 brief Step 1 给出的 8 个测试（`sgr_encodes_a_wheel_scroll`、
    `sgr_wheel_down_uses_a_different_button_code`、
    `sgr_marks_release_with_a_lowercase_m`、`modifiers_are_added_to_the_button_code`、
    `default_encoding_uses_the_single_byte_form`、
    `default_encoding_refuses_coordinates_it_cannot_express`、
    `nothing_is_sent_when_the_agent_does_not_want_the_mouse`、
    `x10_mode_drops_release_events`），含 `use crate::proto::{MouseForward, MouseForwardKind};`
    和 `fn ev(...)` 辅助函数。
- `src/session.rs`
  - `SessionManager::forward_mouse` 上方的占位说明（"占位实现：... 留给
    Task 9 ... 让 `Request::Mouse` 编译得过、跑得起来"）改写成描述真实行为
    的注释，因为占位已经被替换，旧注释会误导读者。函数体本身（
    `self.with_session(id, |s| s.pty.write_mouse(ev))`）没变，之前已经接对了线。

## TDD 步骤与结果

1. **红**：只加测试（不加实现）跑
   `export PATH="$HOME/.cargo/bin:$PATH" && cargo test --lib pty:: -- --test-threads=1`，
   得到 8 处 `error[E0425]: cannot find function 'encode_mouse' in this scope`，
   符合预期的编译失败。
2. 实现 `encode_mouse` 和 `write_mouse`。
3. **绿**：
   ```
   export PATH="$HOME/.cargo/bin:$PATH"
   cargo test -- --test-threads=1
   ```
   汇总：`548` 个 `... ok`（`grep -c "^test .* \.\.\. ok"`），`0` 个
   `FAILED`。基线是 540、0 failed，548 = 540 + 8，新测试全部在里面且全绿
   （逐条确认：`pty::tests::sgr_encodes_a_wheel_scroll`、
   `sgr_wheel_down_uses_a_different_button_code`、
   `sgr_marks_release_with_a_lowercase_m`、`modifiers_are_added_to_the_button_code`、
   `default_encoding_uses_the_single_byte_form`、
   `default_encoding_refuses_coordinates_it_cannot_express`、
   `nothing_is_sent_when_the_agent_does_not_want_the_mouse`、
   `x10_mode_drops_release_events` 都是 `... ok`）。
4. `cargo fmt --check`：退出码 0，无差异。
5. `cargo clippy --all-targets -- -D warnings`：`Finished`，无警告。
6. `git diff --check -- src/pty.rs src/session.rs`：退出码 0，无空白问题。

## vt100 0.16 的鼠标 API 实况 vs brief 的假设

去 `~/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/vt100-0.16.2/src/screen.rs`
核对：

- `pub enum MouseProtocolMode { None, Press, PressRelease, ButtonMotion, AnyMotion }`，
  `#[derive(Copy, Clone, Debug, Eq, PartialEq, Default)]`，`#[default] None`。
- `pub enum MouseProtocolEncoding { Default, Utf8, Sgr }`，同样
  derive 了 `PartialEq`，`#[default] Default`。
- `Screen::mouse_protocol_mode(&self) -> MouseProtocolMode` 和
  `Screen::mouse_protocol_encoding(&self) -> MouseProtocolEncoding` 都是
  公开的只读访问器，签名与 brief 假设的完全一致。
- 两个枚举都在 `vt100::lib.rs` 里 `pub use screen::{MouseProtocolEncoding,
  MouseProtocolMode, Screen};`，也就是 `vt100::MouseProtocolMode` /
  `vt100::MouseProtocolEncoding` 这两条路径本身也对得上。

**结论：0.16.2 的鼠标相关 API 与 brief 引用的完全一致**，没有需要改名或改
签名的地方——0.15→0.16 那次跳版本影响的是 scrollback 越界 panic，不影响
这一块。brief 里给的实现代码（`encode_mouse`、`write_mouse`）逐字量了一遍，
和真实类型系统对得上，没有编译期分歧，直接照抄即可。

## brief 与现实不一致之处，以及怎么处理的

- **占位注释清理**：brief 没提「把旧占位注释删掉」这一步，但任务说明明确
  要求「移除占位标记」，我在 `pty.rs` 删除了
  `// Task 9 实现：空壳先让 'Request::Mouse' 这条线路编译得过、跑得起来。`，
  并顺手把 `session.rs` 里指向"留给 Task 9"的过时说明也改写了，避免文档
  和代码脱节（这属于任务说明里"WHAT EXISTS NOW"段落交代的收尾工作，不是
  brief 正文，但逻辑上必须做）。
- **测试数量**：brief Step 4 写"跑测试确认通过，Expected: 205 个，全绿"，
  这是过时的绝对数字（brief 写于更早阶段）。按任务说明的指示，验证标准
  改成"基线 540 全绿 + 新增 8 个全绿"，不是任何绝对总数。
- **其余部分**：SGR 编码规则、单字节编码上界（223，来自 `32+223=255`）、
  三种返回 `None` 的场景、坐标 0 起 vs 1 起的转换，brief 正文的描述、示例
  测试的期望字节、参考实现三者互相一致，也跟 vt100 0.16.2 的真实类型对得上，
  没有发现需要偏离 brief 字面意思的地方。**没有发现 brief 期望字节或参考
  实现有错**——不同于本计划里前面几个任务遇到过的"排序 bug / 永久挂起测试"
  问题，这次 brief 的代码可以直接用。

## 严谨性核查

- 用 mutation 的方式验证了 `sgr_marks_release_with_a_lowercase_m` 这条测试
  确实会抓到错误实现：把 `let end = if is_release { 'm' } else { 'M' };`
  改成恒为 `'M'`（模拟"漏判 release，永远发大写 M"这个说得通的错误实现），
  单独跑该测试得到
  ```
  left:  [.., 77]   // 'M' = 77，来自坏实现
  right: [.., 109]  // 'm' = 109，测试期望
  ```
  确认失败，然后用 `diff` 核对文件已完整还原成正确版本，无残留改动。
- 逐条检查了其余 7 个测试的断言逻辑，均对应 brief 描述的行为分支（SGR 滚轮、
  修饰键叠加、默认编码单字节、边界值拒绝、`None` 模式吞掉一切、X10 模式
  吞掉 release），每条都在改变对应分支逻辑时会给出不同字节序列或
  `Some`/`None`翻转，不存在"测了但测不出东西"的假阳性测试。
- `git diff --check` 和 `cargo fmt --check` 都是空差异，没有尾随空白或
  格式漂移。

## 顾虑

无实质性顾虑。唯一值得记录但不影响本任务判定的一点：
`encode_mouse` 里 `Utf8` 编码目前和 `Default` 走同一分支（单字节表示），
brief 注释里也承认了这一点（"Utf8 跟 Default 的差别只在坐标怎么编码...
也就不用分开写"）——这意味着如果未来真的接到一个只认 UTF-8 鼠标编码
的 agent，发出去的坐标编码方式其实是 Default 的，不是真正的 UTF-8
变长编码。这不是本任务范围内的 bug（brief 明确把这个简化写进了注释和
接口设计），只是留个记号，万一以后有 agent 真的要求 UTF-8 鼠标编码，
这里需要单独实现。

---

## 复审修复报告（同一 task-9，第二轮）

复审提了 4 个问题，两个 CRITICAL/IMPORTANT 是 brief 参考代码本身的锅
（照抄没错，但真实 xterm 协议压过 brief），两个 MINOR 是测试覆盖缺口。
四个都在 `src/pty.rs` 里改完了，提交 `f59a98f`。

### CRITICAL：legacy（非 SGR）release 必须用哨兵值 3，不是按钮号

**问题**：原实现 `K::Press(b) | K::Release(b) => u32::from(b)` 对 SGR 和
legacy 编码共用同一个按钮码，但 xterm 的 `ctlseqs.txt` 里 normal tracking
mode（`?1000` 不带 `?1006`）明确规定：Cb 的低两位里，0/1/2 是按钮号，
**3 是「有按钮释放」的哨兵值，跟释放的是哪个按钮无关**。SGR 协议因为
Cb 和 M/m 结尾分开传，不需要这个哨兵，release 时照样发真实按钮号——
这也是 SGR 相对 legacy 协议的优势之一。原实现把 SGR 那套「release 也发
真实按钮号」的逻辑错误地套到了 legacy 编码上，导致复审说的现象：
`encode_mouse(PressRelease, Default, Release(0)@(0,0))` 和
`encode_mouse(PressRelease, Default, Press(0)@(0,0))` 输出完全一样的
字节串，agent 没法区分「按下」和「松开」。

**修法**：把「按钮号+修饰键」拆成两个变量——

```rust
let sgr_button = raw_button + modifiers;
let legacy_button = (if is_release { 3 } else { raw_button }) + modifiers;
```

SGR 分支用 `sgr_button`（保留真实按钮号，release 靠结尾的 `m` 区分），
`Default` 和 `Utf8` 分支共用 `legacy_button`（release 时按钮号被替换成
哨兵值 3）。代码里加了注释解释这个不对称是故意的、为什么 SGR 不能这么改。

**覆盖测试**（新增）：
- `default_encoding_uses_the_release_sentinel_not_the_button_number` ——
  钉住 `Release(0)@(0,0)` 在 Default 编码下输出 `[..., 35, 33, 33]`
  （35 = 32+3，不是 32+0=32）。
- `default_encoding_release_differs_from_a_press_at_the_same_spot` ——
  直接复现复审给出的证据反面：同一坐标的 press 和 release 字节串必须
  不同（`assert_ne!`）。
- `utf8_encoding_also_uses_the_release_sentinel` —— 因为
  `legacy_button` 是 Default/Utf8 共用变量，顺手钉住 Utf8 分支没有被
  漏掉这个修复。

**没碰 SGR 路径**：`sgr_marks_release_with_a_lowercase_m` 那条老测试
（Release(0) 期望 `\x1b[<0;5;6m`，按钮号是 0 不是 3）原样保留且仍然通过，
证明 SGR 那边确实没被这次修复动到。

### IMPORTANT：Utf8 编码不能走单字节路径

**问题**：原实现 `_` 分支把 `Utf8` 和 `Default` 合并处理，直接输出
`32+值` 的原始字节。但 `?1005`（UTF-8 mouse mode）真正的定义是把
`32+值` 当 Unicode 码点，按 UTF-8 变长编码发出去，不是发单字节。一旦
`32+值 >= 128`（也就是坐标/按钮值 >= 96），单字节路径会吐出一个独立
的 `>=128` 字节，在 UTF-8 里是非法的续字节起始，会污染 agent 后续所有
输入的解析，不只是这一次事件坐标读错——复审说得对，这比"坐标编不下"
严重得多。

**修法**：给 `Utf8` 单开一个分支，`char::from_u32(32 + 值)` 转成
`char` 再 `push` 进 `String`，最后取 `.into_bytes()`，让标准库按
UTF-8 规则编码（0-127 单字节，128-2047 两字节，……）。上界另算：两字节
UTF-8 能表达的最大码点是 `0x7FF`（2047），减掉固定加的 32，单个值最大
能到 **2015**，超过就用 `checked` 式的提前返回 `None`（`> 2015` 才拒绝，
不是凑一个随意数字——2015 正好是 xterm 文档里对 `?1005` 行列上限给出
的数字，两边算法互相印证）。

**覆盖测试**（新增）：
- `utf8_encoding_uses_multiple_bytes_once_the_column_passes_127` ——
  列号 96（跨过 128 门槛）时断言完整字节序列
  `[0x1b, b'[', b'M', 32, 0xC2, 0x81, 33]`（0xC2 0x81 是 U+0081 的
  合法两字节 UTF-8），钉住不会退化成单字节。用 mutation 测试验证过：
  把 Utf8 分支临时改回单字节形式，这条测试会失败并报出
  `left: [.., 129, ..]` vs `right: [.., 194, 129, ..]`（详见下方
  mutation 记录）。
- `utf8_encoding_refuses_coordinates_past_the_two_byte_ceiling` ——
  列坐标使 wire 值超过 2015 时必须返回 `None`。

### MINOR：Default 编码的边界只测了远超边界的值

**问题**：原来唯一的拒绝测试用的是列 300，`32+301=333`，无论上界写
`>255` 还是错误地写成 `>256`，300 都能触发拒绝——测试测不出边界具体
在哪。复审用 mutation（`>255`→`>256`）验证过全套测试照样绿。

**修法**：加了一对钉住边界两侧的测试，而不是只加一个更精确的拒绝用例。

**223 和 255 的关系（复审要求核实）**：这两个数字描述的是同一条边界的
两个视角，不是两个互相矛盾的说法——都对：
- **223** 是 brief 原始设计说明里给的「值本身」的上限：wire 坐标
  （1 起算）或按钮码最大能是 223。
- **255** 是代码里检查的「编码后的字节」上限：`32 + 值`，因为要塞进一
  个 `u8`，最大只能是 255。
- 二者的换算关系就是固定的 `+32` 偏移：`223 + 32 = 255`。值 ≤ 223
  当且仅当编码字节 ≤ 255。代码检查 `b > 255`（`b` 已经是 `32+值`）
  和"值不能超过 223"是完全等价的两种写法，没有谁更对，只是站在
  加法两端说话。

**覆盖测试**（新增）：
- `default_encoding_accepts_the_largest_column_it_can_express` —— 内部
  0 起算列号 222（wire 值 223，编码字节 255，u8 能装的最大值）必须
  成功，断言完整字节 `[0x1b, b'[', b'M', 32, 255, 33]`。
- `default_encoding_rejects_the_column_one_past_the_boundary` —— 列号
  223（wire 值 224，编码字节 256，超出 u8）必须返回 `None`。

用 mutation 验证过这对测试确实能抓到复审提到的那种"`>255`偷偷改成
`>256`"的错误：把检查改成 `> 256` 后重跑，`accepts_the_largest_column`
仍然通过（255 本来就没超过 256），但 `rejects_the_column_one_past_the_boundary`
失败——256 不再 `>256`，函数会返回 `Some(..)` 而不是期望的 `None`。

### MINOR：补 Default 编码的 release 测试

这条本身就是让 CRITICAL bug 得以蒙混过关的覆盖缺口。上面 CRITICAL 部分
新增的 `default_encoding_uses_the_release_sentinel_not_the_button_number`
和 `default_encoding_release_differs_from_a_press_at_the_same_spot`
两条就是这个缺口的补齐，不重复列。

### 跑测试的命令和结果

```
export PATH="$HOME/.cargo/bin:$PATH"
cargo test --lib pty:: -- --test-threads=1
```
→ `test result: ok. 31 passed; 0 failed; 0 ignored; 0 measured; 489 filtered out`
（原来 8 个鼠标测试 + 这轮新增的 8 个 = 16 个鼠标相关测试全绿，其余 15
个是同文件里已有的 PTY/scrollback 测试）。

全量：
```
cargo test -- --test-threads=1
```
→ 汇总 `grep -c "^test .* \.\.\. ok"` = **555**，`grep -c "FAILED"` = **0**。
上一轮报告里的基线是 548（540 原始基线 + 8 个第一版鼠标测试），
555 = 548 + 7（这轮净增 7 条新测试：release 哨兵 2 条 + Utf8 共用哨兵
1 条 + 边界两侧 2 条 + Utf8 多字节 1 条 + Utf8 上界拒绝 1 条）。

```
cargo fmt --check
```
→ 退出码 0，无差异。

```
cargo clippy --all-targets -- -D warnings
```
→ `Finished`，无警告。

```
git diff --check -- src/pty.rs
```
→ 退出码 0，无空白问题。

### 严谨性核查（这轮新增的 mutation 测试）

对三处改动分别做了"改错再跑测试"的验证，改完立刻用 `diff` 核对文件已
完整还原，无残留：

1. **release 哨兵**：把 `legacy_button` 改回 `raw_button + modifiers`
   （也就是复现修复前的 bug），`default_encoding_uses_the_release_sentinel_not_the_button_number`、
   `default_encoding_release_differs_from_a_press_at_the_same_spot`、
   `utf8_encoding_also_uses_the_release_sentinel` 三条全部失败——三条
   测试都真的在守这个行为，不是摆设。
2. **边界**：把 `> 255` 改成 `> 256`（复审给出的确切 mutation），
   `default_encoding_rejects_the_column_one_past_the_boundary` 失败，
   `default_encoding_accepts_the_largest_column_it_can_express` 保持
   通过（符合预期——255 不受这个 mutation 影响，只有边界那一侧的测试
   才应该翻）。
3. **Utf8 多字节**：把 Utf8 分支临时替换成跟 Default 一样的单字节实现，
   `utf8_encoding_uses_multiple_bytes_once_the_column_passes_127` 失败，
   报出 `left: [.., 129, 33]` vs `right: [.., 194, 129, 33]`，证明这条
   测试确实在防止"退化成单字节"这个具体错误，不是碰巧断言了点别的
   什么。

### 顾虑

无新增顾虑。上一轮报告里记的"Utf8 只是坐标范围变窄"的说法已经被这轮
修复替换成正确实现，不再是遗留问题。
