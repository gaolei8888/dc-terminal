# Task 9 报告：密钥验证

## 实现内容

- 新增 `src/verify.rs`：
  - `VerifyOutcome { Ok, BadKey, Unreachable }`（`Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize`）
  - `verify_with(url, key, send)` —— 纯判定逻辑，传输层由调用方注入的 `&dyn Fn(&str, &str) -> Result<u16, String>` 提供。401/403 → `BadKey`；`Err` → `Unreachable`；其余状态码一律 `Ok`。
  - `send_probe(url, key)` —— 真实 `ureq` 调用，`PROBE_TIMEOUT = 4s`，POST 一个最小 Anthropic 风格 body（`model: "probe"`），同时带 `x-api-key` 和 `authorization: Bearer` 两个头，把 `ureq::Error::Status(code, _)` 解出来还原成 `Ok(code)`，其它错误归为 `Err`。
- `Cargo.toml` 加 `ureq = { version = "2", default-features = false, features = ["tls", "json"] }`。
- `src/lib.rs` 加 `pub mod verify;`。
- `src/proto.rs`：`Request::VerifySecret { profile, value }`、`Response::Verify(crate::verify::VerifyOutcome)`。
- `src/daemon.rs::handle`：新增 `Request::VerifySecret` 分支——按 `profile` 名字查 `all_profiles(profiles_dir)`，找到该 profile 的 `secret.verify`；没声明 `verify` 的直接放行 `Response::Verify(VerifyOutcome::Ok)`（不是错误），声明了的调用 `verify_with(&v.url, &value, &send_probe)`。

代码与任务简报中的代码逐字一致，未做改写。

## 测试与结果

TDD 四个测试全部按简报原文写入 `src/verify.rs` 的 `mod tests`：
- `unauthorized_means_bad_key`
- `network_failure_is_reported_as_unreachable`
- `anything_else_passes`
- `the_key_reaches_the_transport`

### RED

命令：
```
~/.cargo/bin/cargo test --lib verify
```
（先只写了测试模块 + `pub mod verify;`，还没写实现）

输出（节选）：
```
error[E0425]: cannot find function `verify_with` in this scope
 --> src/verify.rs:9:17
error[E0433]: cannot find type `VerifyOutcome` in this scope
  --> src/verify.rs:10:17
...
error: could not compile `dct` (lib test) due to 7 previous errors
```
符合预期：`verify_with` / `VerifyOutcome` 尚未实现，编译失败。

### GREEN

命令：
```
~/.cargo/bin/cargo test --lib verify
```
输出：
```
running 4 tests
test verify::tests::anything_else_passes ... ok
test verify::tests::unauthorized_means_bad_key ... ok
test verify::tests::the_key_reaches_the_transport ... ok
test verify::tests::network_failure_is_reported_as_unreachable ... ok

test result: ok. 4 passed; 0 failed; 0 ignored; 0 measured; 113 filtered out; finished in 0.00s
```

全量套件：
```
~/.cargo/bin/cargo test
```
结果：`test result: ok. 117 passed; 0 failed` （src/lib.rs 单元测试）+ 全部集成测试文件（`cli.rs`、`client_timeout.rs`、`concurrency.rs`、`daemon_detach.rs`、`daemon_roundtrip.rs`、`profiles_flow.rs`、`projects_flow.rs`、`signal_restore.rs`、`slow_input.rs`、`socket_perms.rs`）均 `ok`，无失败无忽略。

`~/.cargo/bin/cargo fmt` 已跑，`git diff --check` 无输出（无尾随空白/无换行结尾问题）。
`~/.cargo/bin/cargo clippy --lib` 无警告。

无任何测试触发真实网络请求——四个测试全部通过注入闭包驱动 `verify_with`，`send_probe` 本身（唯一会打网络的函数）没有被任何测试直接调用。

## READ_TIMEOUT 核实

`src/client.rs` 第 11 行：
```rust
const READ_TIMEOUT: Duration = Duration::from_secs(5);
```
`PROBE_TIMEOUT`（`src/verify.rs`）设为 4 秒，严格小于 5 秒，留了 1 秒余量。守护进程处理 `VerifySecret` 的耗时上限（约 4 秒 + 少量开销）不会触发客户端侧的读超时/丢弃重连。

## 两个自查问题

**1. 密钥值是否会流到不该去的地方（Debug / 错误信息 / 日志行）？**

没有。`grep -rn 'Request'` 检查了 `src/ui.rs`、`src/daemon.rs`、`src/client.rs` 里所有 `Request` 相关代码：
- `client.rs::try_call` 只用 `serde_json::to_string(req)` 序列化上线，不是 `Debug` 格式化。
- `daemon.rs::serve` 里 `Response::Error(format!("请求解析失败: {e}"))` 只在**解析失败**（JSON 语法错）时触发，格式化的是解析错误 `e`，不是 `req` 本身——这时候连 `Request` 都还没成功反序列化，不存在把整个结构体连密钥一起打进日志的路径。
- 全仓库搜索 `{:?}` 未命中任何 `req`/`Request` 相关格式化调用。
- `eprintln!("连接处理失败: {e}")`（`run_with_manager`）打印的是 `serve()` 的 `Result<()>` 错误（I/O 层面），不涉及 `Request` 内容。

`Request` 保留 `#[derive(Debug, ...)]` 是延续既有协议枚举的统一 derive 集合（`SetSecret` 早就带着明文密钥这么做了），没有新增泄露面。

**2. `ureq` 让依赖树膨胀了多少？**

`git diff Cargo.lock` 显示新增 **43 个** transitive 包（含 TLS 栈 `rustls`/`ring`/`rustls-webpki`/`webpki-roots`、URL/IDNA/ICU 的一整套 `idna`/`icu_*`/`url`、以及 `base64`/`getrandom`/`zeroize` 等支撑库）。这是 `ureq` 走 `tls` feature 引入 `rustls` 全套依赖导致的，量不小，但都是 `rustls` 生态里标准且广泛复用的包，没有出现意料之外的依赖（比如没有拉进异步运行时 tokio 之类的东西）。值得在此报告一下，供后续任务或依赖审计参考。

## 变更文件

- `Cargo.toml`（新增 ureq 依赖）
- `Cargo.lock`（更新）
- `src/verify.rs`（新建）
- `src/lib.rs`（注册 `pub mod verify;`）
- `src/proto.rs`（`Request::VerifySecret`、`Response::Verify`）
- `src/daemon.rs`（`handle` 新增 `VerifySecret` 分支，import `verify` 模块三个符号）

提交：`4d8b1de feat: 密钥存盘前先探一下端点，401/403 当场拦住`

## 自查发现

无发现需要修复的问题。范围严格贴合简报：没有加重试逻辑、没有加缓存、没有碰 UI（Task 11 的范围）。`verify_with`/`send_probe` 的拆分按要求保留，daemon 侧对「没声明 verify」的处理按要求走 `Ok` 分支而非错误分支。

## 问题或顾虑

无阻塞项。唯一值得注意的是上面提到的 43 个新增 transitive 依赖（TLS 栈体积），如果后续有依赖体积/审计的关注点，这是一个数据点。

---

## 修复报告：PROBE_TIMEOUT 没兜住建连阶段（Important）

### 问题

`send_probe` 只调了 `.timeout(PROBE_TIMEOUT)`，没调 `.timeout_connect(...)`。核对 vendored `ureq-2.12.1` 源码（`~/.cargo/registry/src/index.crates.io-*/ureq-2.12.1/src/`）：

- `AgentBuilder::new()` 默认 `timeout_connect: Some(Duration::from_secs(30))`（`agent.rs:256`）。
- `connect_host` 里，`connect_deadline` 只要 `unit.agent.config.timeout_connect` 是 `Some` 就直接用它，完全不看 `.timeout()` 设的 `unit.deadline`（`stream.rs:352-357`）。
- 也就是说没显式设 `.timeout_connect()`，建连阶段实际预算是默认的 30 秒，不是 4 秒的 `PROBE_TIMEOUT`。一个在 TCP 层就慢或者悄无声息不可达的主机——恰好是这个功能要判成 `Unreachable` 的那类情况——能把 `send_probe` 卡住接近 30 秒，远超 `client::READ_TIMEOUT`（5 秒），界面侧会把连接当作错位丢掉重连，用户看到「连不上守护进程」而不是验证结果，正是 `src/verify.rs:4-6` 那条注释宣称被挡住的失败模式。

### 选择的预算：`.timeout_connect(PROBE_TIMEOUT)`，同一个 4 秒，不是更小的子预算

核对 `stream.rs` 和 `unit.rs`/`request.rs` 后确认：ureq 对一次请求只算一个起点相同的截止时间。`.timeout()` 对应的 `unit.deadline` 是在 `request.rs:122`（`Instant::now() + timeout`）构造 `Unit` 时算好的一个固定 `Instant`；`connect_host` 里 `timeout_connect` 的 `connect_deadline` 是在紧接着的建连阶段用 `Instant::now() + timeout_connect` 现算的——两者起点几乎是同一时刻（中间只隔了 DNS 解析，通常极快）。建连之后，读写超时用的是 `time_until_deadline(unit.deadline)`（`stream.rs:433-443`），也就是同一个截止时间剩下的余量，不是重新给的一整份预算。

结论：把 `timeout_connect` 设成和 `timeout` 相同的 `PROBE_TIMEOUT`，并不会把总预算翻倍成 8 秒——建连吃掉多少时间，就会从后面读写阶段的剩余预算里扣掉，整条请求的实际上限仍然近似 4 秒，仍然小于 `client::READ_TIMEOUT` 的 5 秒。因此选了同一个 `PROBE_TIMEOUT`，而不是为建连单独切一个更小的子预算——没有必要，也不会更安全，只会更复杂。

代码：把 agent 构造抽成 `build_probe_agent()`，同时设 `.timeout(PROBE_TIMEOUT)` 和 `.timeout_connect(PROBE_TIMEOUT)`。

### 其它 ureq 超时旋钮排查

逐个核对了 `AgentConfig` 的四个超时字段在 vendored 源码里的实际走向：

- `timeout_read` / `timeout_write`：`connect_host` 里，只要 `unit.deadline`（即 `.timeout()` 设的那个）是 `Some`，就直接用 `time_until_deadline(unit.deadline)` 设 socket 的读/写超时，完全不看 `timeout_read`/`timeout_write` 字段（`stream.rs:433-443`）。也就是说这两个字段只在没设 `.timeout()` 时才会生效，本任务里 `.timeout()` 一直是 `Some`，所以它们不构成问题——和 ureq 文档注释「`.timeout()` 覆盖 `timeout_read`/`timeout_write`」的说法一致，这条没有被误导。
- `timeout_connect`：如上，唯一一个「`Some` 就优先、不看 `.timeout()`」的字段，本次已修复。

另外发现一个不是「旋钮」但同样会无界阻塞的点，记录在这里但**没有动**：`connect_host` 里的 DNS 解析（`unit.resolver().resolve(&netloc)`，`stream.rs:365`）完全没有超时保护，源码里就有一行 `// TODO: Find a way to apply deadline to DNS lookup.`（`stream.rs:364`）。这不是某个可以调的 `AgentBuilder` 字段——ureq 2.12.1 本身就没给 DNS 阶段留超时旋钮，要绕开得自己实现 `Resolver` trait 并在里面加超时，这已经超出「补一个遗漏的 timeout 调用」的范围，属于 ureq 自身的已知限制，不在这次修复范围内，供后续如果要收紧 `Unreachable` 判定的上限时参考。

### 注释更新

`PROBE_TIMEOUT` 上方的注释扩展为说明：这个预算必须同时喂给 `.timeout()` 和 `.timeout_connect()`，否则建连阶段会退回 ureq 默认的 30 秒；并说明两个字段共享同一条截止时间、不会把预算翻倍。`build_probe_agent` 单独加了函数级注释说明拆分的原因——让测试能在不发真实请求的前提下核实配置。

### 非网络的配置断言

`send_probe`（唯一会打网络的函数）依旧不能被任何测试直接调用。但把 agent 构造拆成 `build_probe_agent() -> ureq::Agent` 之后，发现 `ureq::Agent` 本身 `#[derive(Debug, Clone)]`（`agent.rs:111`），内部的 `AgentConfig`（`agent.rs:70`，字段是 `pub(crate)`）也 `#[derive(Debug)]`——`pub(crate)` 只限制字段的直接访问，不影响派生的 `Debug::fmt` 把字段值格式化进输出，而 `Debug::fmt` 是公开 trait 方法，外部 crate 能调。用一个独立的 scratch crate 验证过：

```
Agent { config: AgentConfig { ..., timeout_connect: Some(4s), ..., timeout: Some(4s), ... }, ... }
```

所以加了 `probe_agent_bounds_the_connect_phase_too` 测试：只调用 `build_probe_agent()`（不 `.get()`/`.post()`，不发任何字节），断言 `format!("{:?}", ...)` 里含 `"timeout_connect: Some(4s)"` 和 `"timeout: Some(4s)"`。这条测试不建 socket、不解析 DNS、不等待，纯粹是对象构造 + 字符串断言，能在 CI 里稳定跑，也能在未来有人不小心删掉 `.timeout_connect()` 时立刻炸掉，不必只靠注释和这份评审记录兜底。

### 覆盖测试

`src/verify.rs::tests` 现在 5 个测试（新增 `probe_agent_bounds_the_connect_phase_too`），原有 4 个不变。

### 命令与输出

```
~/.cargo/bin/cargo test --lib verify
```
```
running 5 tests
test verify::tests::anything_else_passes ... ok
test verify::tests::unauthorized_means_bad_key ... ok
test verify::tests::network_failure_is_reported_as_unreachable ... ok
test verify::tests::the_key_reaches_the_transport ... ok
test verify::tests::probe_agent_bounds_the_connect_phase_too ... ok

test result: ok. 5 passed; 0 failed; 0 ignored; 0 measured; 113 filtered out; finished in 0.00s
```

```
~/.cargo/bin/cargo test
```
lib: `test result: ok. 118 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out`（比修复前多 1，即新增的配置断言测试）。全部集成测试文件（`cli.rs`、`client_timeout.rs`、`concurrency.rs`、`daemon_detach.rs`、`daemon_roundtrip.rs`、`profiles_flow.rs`、`projects_flow.rs`、`signal_restore.rs`、`slow_input.rs`、`socket_perms.rs`）均 `ok`，无失败无忽略。

`~/.cargo/bin/cargo fmt` 已跑，无改动（新代码格式本身已符合）。`git diff --check` 无输出。`~/.cargo/bin/cargo clippy --lib` 无警告。

只改了 `src/verify.rs` 一个文件，`git status` 确认没有牵动其它文件。

---

## Round 2 修复报告：澄清 PROBE_TIMEOUT 注释中 DNS 超时的限制

### 问题

第一轮修复后的代码是正确的，但注释仍然承诺了代码做不到的事：「必须小于 `client::READ_TIMEOUT`（5 秒）」这个保证对 DNS 解析不成立。ureq-2.12.1 的 `stream.rs:364` 明确有一行 TODO 注释：`// TODO: Find a way to apply deadline to DNS lookup.`——DNS 查询完全不被任何 ureq 级超时保护，某个响应缓慢或 UDP 丢包的 resolver 仍然可以让 `send_probe` 卡超过 5 秒。

注释原文没有说明这一点，用户读代码时会误认为 4 秒的预算是绝对的，实际上有个隐藏的缺口。

### 新注释内容

替换了 `PROBE_TIMEOUT` 上方的 doc 注释，明确表述：

1. **覆盖的部分**：TCP 建连 + 请求/响应 = ~4 秒，确实小于 5 秒的 `client::READ_TIMEOUT`。
2. **不覆盖的部分**：DNS 查询无超时保护。引用 `ureq-2.12.1/src/stream.rs:364` 的 TODO 作为佐证，下一个读者可以直接去源码验证，不必只信任注释。
3. **为什么接受这个限制**：
   - 完全挡住 DNS 超时需要实现自定义 `Resolver` trait，已超出「补遗漏 timeout 调用」的范围。
   - 现实中 resolver 自己有超时，不会无限卡。
   - 关键是**影响有限**：验证在后台线程进行（Task 11 设计），即使一条后台连接卡住也不会冻结主界面，最多用户看到「连不上守护进程」然后刷新重试。

### 新注释文本（中文，与仓库风格一致）

```
/// 探测请求的超时（TCP 建连 + 请求/响应）。**TCP 阶段被限在 4 秒以下**
/// `client::READ_TIMEOUT`（5 秒）之内。守护进程在这里等多久，界面那条连接就等多久，
/// 超过 5 秒界面会判定连接错位并丢弃重连，用户看到的是「连不上守护进程」。
///
/// 这个预算必须同时喂给 `.timeout()` 和 `.timeout_connect()`（见 `build_probe_agent`）：
/// ureq 的 `AgentBuilder` 默认把 `timeout_connect` 设成 30 秒，且建连阶段优先认它而不是
/// `.timeout()` 的整体截止时间（`ureq-2.12.1/src/stream.rs` `connect_host`）。两个字段
/// 共用同一个 `PROBE_TIMEOUT` 不会把预算翻倍——ureq 内部对整条请求只算一个起点相同的
/// `Instant` 截止时间，建连阶段跑掉的时间会从后续读写阶段的剩余预算里扣。
///
/// **DNS 查询不被这个超时保护**。ureq 2.12.1 的 `stream.rs:364` 无法为 DNS 设置截止时间
/// （代码注释里有 TODO），所以如果 resolver 响应缓慢或 UDP 丢包，发送可能会卡超过 5 秒。
/// 为了完全挡住这个风险需要实现自定义 `Resolver`，但实际好处有限：(1) resolver 本身有超时，
/// 不会无限卡；(2) UI 在独立后台线程验证（见 Task 11 设计），不会冻结主界面，最坏情况是
/// 这条后台连接超时、用户刷新再试。
```

### 测试确认

所有测试通过，无新增失败：

```
~/.cargo/bin/cargo test --lib verify
```
```
running 5 tests
test verify::tests::anything_else_passes ... ok
test verify::tests::unauthorized_means_bad_key ... ok
test verify::tests::network_failure_is_reported_as_unreachable ... ok
test verify::tests::the_key_reaches_the_transport ... ok
test verify::tests::probe_agent_bounds_the_connect_phase_too ... ok

test result: ok. 5 passed; 0 failed; 0 ignored; 0 measured; 113 filtered out; finished in 0.00s
```

```
~/.cargo/bin/cargo test
```
```
test result: ok. 122 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 14.56s
```

全部集成测试（`cli.rs`、`client_timeout.rs`、`concurrency.rs`、`daemon_detach.rs`、`daemon_roundtrip.rs`、`profiles_flow.rs`、`projects_flow.rs`、`signal_restore.rs`、`slow_input.rs`、`socket_perms.rs`）均通过，无失败。

`~/.cargo/bin/cargo fmt` 无改动；`git diff --check` 无输出；`~/.cargo/bin/cargo clippy --lib` 无警告。

### 提交

```
3f6aa78 fix: 更新 PROBE_TIMEOUT 注释以准确说明 DNS 查询不被超时保护
```

仅修改 `src/verify.rs` 中的 doc 注释，无代码逻辑改动，无其它文件牵动。

---

## Round 3 修复报告：修正 PROBE_TIMEOUT 注释开句连贯性

### 问题

第 2 轮修复后的注释开句仍然阅读体验不佳：「探测请求的超时（TCP 建连 + 请求/响应）。**TCP 阶段被限在 4 秒以下** `client::READ_TIMEOUT`（5 秒）之内。」两个子句之间缺少连接词，读者会在"被限在 4 秒以下"和"之内"之间卡住。

### 修复

改为：「探测请求的超时（TCP 建连 + 请求/响应）为 4 秒，在 `client::READ_TIMEOUT`（5 秒）之内。」

- 明确表述"为 4 秒"而不是"被限在...以下"。
- 用"在...之内"自然地表达约束关系。
- 整句流畅，后文「守护进程在这里等多久，界面那条连接就等多久」自然承接。

### 测试确认

```
~/.cargo/bin/cargo test 2>&1 | tail -5
```
```
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s

   Doc-tests dct

running 0 tests

test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
```

全量测试 `ok. 122 passed; 0 failed`（无增删，仅注释文本改动）。

### 提交

```
9a3c0f5 fix: 修正 PROBE_TIMEOUT 注释的开句连贯性
```

仅修改 `src/verify.rs` 第 4-5 行的 doc 注释文本。
