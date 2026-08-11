# 跟进 2 —— 报告

## 结论

修好了。两个测试不再起真登录 shell，改起测试自己注册的 `/bin/sh --noediting`。
详细验证见下。

## 简报核对结果（一处不准确）

简报（followup-2-brief.md）第 46-49 行说：

> `src/session.rs` 的测试里已经有几个现成的样子可以参考……但它们走的是进程内的
> `register_profile`，而这两个测试走 socket，**拿不到那个入口**，得靠磁盘上的
> profile 文件。

**这处不准确。** 读了 `src/daemon.rs::run_with_manager`、`src/session.rs::create`/
`resolve_profile`、以及三个已经在用这个手法的现成测试之后确认：`register_profile`
注册的 profile **完全够得到 socket**——

- `daemon::run_with_manager(socket, mgr)` 接受一个预先配置好的 `Arc<SessionManager>`，
  跟 `daemon::run(socket)`（内部自己 `new()` 一个空 manager）平行存在，专门就是
  给测试用的（`run()` 自己的文档注释也这么写）。
- `SessionManager::resolve_profile`（`src/session.rs:564`）查找顺序是：调用方传入的
  `profiles`（磁盘+内置）→ `extra_profiles`（`register_profile` 注册的那张表）→
  编译进二进制的内置表。`extra_profiles` 这条路完全在 `create()` 内部，跟请求是
  不是从 socket 来的无关。
- `tests/concurrency.rs::list_is_not_blocked_by_slow_create` 和
  `tests/profiles_flow.rs::two_projects_each_keep_their_own_agent_over_the_wire`
  两个测试都已经是「`register_profile` 之后用 `run_with_manager` 起一个真
  socket，再用真 `Client` 连上去发请求」——跟 `grid_reply.rs`、
  `entering_a_session...` 这两条要走的路径一模一样。

`register_profile` 拿不到的入口，是 `tests/common::start_daemon()` 这个**具体的
测试脚手架函数**——它是零参数、内部自己 `new()` manager 的形状（这一点
`common/mod.rs` 头上的注释也解释了，是给 `concurrency.rs` 的同类需求特意没抽过去
的），不是 `register_profile` 机制本身够不着 socket。

**处理方式：没有先报告再动手**——发现的时候已经在写代码路上，但这处发现直接
改变了实现选择：没有照简报建议去磁盘上写 profile TOML，而是用了这个已经在库里
出现三次的现成手法（`register_profile` + `run_with_manager`），不新造机制、
也不碰产品代码。如果这个判断不对，请指出，我可以改回磁盘 TOML 那条路。

其余核对过、没发现问题的地方：
- `profiles/shell.toml` 确实是 `command = ["/bin/zsh"]`，`is_agent = false`。
- `profiles_dir_for_socket`（`src/profile.rs:218`）确实是 `socket.parent()/profiles`，
  `all_profiles`（`src/profile.rs:334`）确实每次调用都重新 `load_dir`。
- 两个测试原来的样子和简报描述的一致：`grid_reply.rs` 两条测试都靠
  `wait_for_prompt` 认「屏幕非空白」；`ui/mod.rs` 那条起daemon 用的也是内置
  `"shell"` profile，且发送 200 行循环命令时**完全没有**等提示符（连
  「非空白」这种弱等待都没有），比 `grid_reply.rs` 更激进地依赖了「kernel
  canonical 模式下输入会被缓冲，不管子进程有没有准备好读」这条隐含假设——
  这也是我额外给它加一次提示符等待的原因（见下）。

## 换了什么程序，怎么保住「回显 ≠ 执行」的区分

两处都换成同一个测试专用 profile：`command = ["/bin/sh", "--noediting"]`，
`env.ENV = "/dev/null"`，`env.PS1` 钉死成一个测试专用固定串（`grid_reply.rs`
里是 `"dct-test$ "`）。选它的原因：

- **零可观测延迟、不读任何 rc**：`--noediting` 关掉 GNU Readline，
  `sh` 以 `sh` 这个名字启动、又是交互式时会去找 `$ENV` 指向的文件当启动脚本
  （posix 模式下 sh 版本的 rc 文件），显式把它摁死成 `/dev/null`，不管跑测试
  的机器上这个变量有没有被意外设过。
- **不引入新的竞态窗口**：`--noediting` 关掉的不只是行编辑功能，还包括
  Readline 会在某个不确定时刻把终端从 canonical 模式切成 raw 模式这件事——
  这正是原始问题里 zsh 的 ZLE 会做、而这份简报没点名但同样致命的一个次要
  竞态源。用 `python3` 的 `pty` 模块单独探测过（脚本已清理）：`/bin/sh
  --noediting` 在「daemon 刚 fork 出子进程、还没来得及等提示符」的 0 延迟场景
  下，`echo` 命令依然能正确执行、行数不多不少——因为 canonical 回显和行缓冲
  都是内核 tty 驱动做的，跟子进程有没有开始 `read()` 无关，只要没人把模式切
  成 raw。

它依然**是一个真正的 POSIX shell**，`echo` 依然是它自己执行的内建命令，不是
拿 `cat` 之类的东西去模拟「随便啥都往回吐」。

**抓住「只回显、没执行回车」的断言**，两条测试分别是：

- `text_then_an_empty_input_submits_the_line`：`wait_for_count(&mut c, id,
  "dct-reply-landed", 2)`（`tests/grid_reply.rs:139` 起的 `wait_for_count`
  函数，调用点在文件尾部）。发文字时 `dct-reply-landed` 只会因为终端回显出现
  一次（这行命令本身），只有 sh 真的收到 `\r`、执行了 `echo`，才会在**单独
  一行**上再打印一次同样的文字，凑够 2 次。回车没送到就永远卡在 1 次，
  10 秒之后 `panic!`。
- `an_empty_input_on_its_own_is_a_bare_enter`：先用 `wait_for` 断言「回车前
  只有 1 次」（防止这条测试本身在没证明什么的情况下就通过），再用
  `wait_for_count(..., 2)` 断言回车后变成 2 次，逻辑对称。

这个断言结构完全照抄自改动前的版本，一行没动——改的只是「谁在跑这个 shell」。

## 两个必测的变异（mutation）——都确认变红

按简报要求，亲手做了退化，确认测试为**正确的原因**失败：

1. **`grid_reply.rs`：去掉两次 `Input` 里的空串**（模拟「回车没送」）。
   分别对两条测试做了这个变异（各自单独跑，跑完立刻 `git checkout` 复原）：
   - `text_then_an_empty_input_submits_the_line`：去掉第二次 `Input{text:
     String::new()}` 调用 → **FAILED**，10.11s 后 panic：
     `「dct-reply-landed」没出现到 2 次，屏幕上是：dct-test$ echo
     dct-reply-landed`（只出现了回显那一次）。
   - `an_empty_input_on_its_own_is_a_bare_enter`：去掉那次「只发空串」的
     `Input` 调用 → **FAILED**，10.12s 后 panic：
     `「bare-enter-works」没出现到 2 次，屏幕上是：dct-test$ echo
     bare-enter-works`。
2. **`entering_a_session_always_lands_at_the_bottom_even_without_a_resize`：
   去掉 `enter_session` 里那次显式 `Request::Scroll { by: Bottom }`**
   （`src/ui/mod.rs` 里 `let _ = app.client().and_then(|c| { c.call(...) })`
   那一段，注释掉整段只留空函数体）→ **FAILED**，5.14s 后 panic：
   `重新进入会话必须落在底部，不能停在离开前翻到的地方：offset=20`
   （`src/ui/mod.rs:2680`）。

三次变异全部按预期变红，改完立刻 `git checkout` 复原，没有把变异过的版本
带进提交。

## 满载并行下重复跑的证据

- `cargo test`（默认并行，不加 `--test-threads=1`）**连续跑了 5 次完整套件**
  （外加一次因为工具超时被杀掉、但已经跑到一半、其中两个目标测试都已经跑过
  且是 `ok` 的第 6 次）。5 次完整跑下来：
  - 全部 `exit=0`，日志里 `grep -c FAILED` 是 **0**。
  - `entering_a_session_always_lands_at_the_bottom_even_without_a_resize ...
    ok` 出现 6 次（5 次完整 + 1 次半截但跑过的）。
  - `text_then_an_empty_input_submits_the_line ... ok` 出现 5 次。
  - `an_empty_input_on_its_own_is_a_bare_enter ... ok` 出现 5 次。
  - 每次完整跑耗时 7.5～10 秒左右起，总运行时间约 37～40 秒/次（含编译后的
    增量），没有任何一次卡在旧的固定期限上——修之前这两个测试单独跑都要
    么很快、要么在 10 秒/5 秒期限上打转，现在稳定在几百毫秒量级完成
    （`0.12s`～`0.17s`）。
- 另外单独对这两个测试所在的两个 target（`grid_reply` + `ui::tests::
  entering_a_session...`）跑了 20 次独立的 `cargo test` 调用，全部
  `exit=0`、0 failed。

## 收尾检查

- `cargo test -- --test-threads=1`：全绿。
- `cargo fmt --check`：第一次跑出一处格式差异（`tests/grid_reply.rs` 里一个
  `assert!` 该拆多行），跑了 `cargo fmt` 改正，单独提交
  （`style: run cargo fmt on grid_reply.rs`）。
- `cargo clippy --all-targets`：无警告。
- `git diff --check`：无输出（没有空白问题）。

## 产品代码

**没有动。** 两处改动都在测试文件里：`tests/grid_reply.rs`、
`src/ui/mod.rs`（改的是 `#[cfg(test)] mod tests` 里那一条测试函数，不是
上面的生产代码部分）。`daemon::run_with_manager`、
`SessionManager::register_profile` 都是改动前就存在、且已经被至少两个
其它测试文件使用的公开测试入口。

## 提交

1. `test: stop racing the developer's real login shell in two flaky
   integration tests` —— 核心修复：两处都换成 `/bin/sh --noediting` +
   `ENV=/dev/null` + 固定 `PS1`；`grid_reply.rs` 走
   `register_profile`/`run_with_manager`（不再依赖 `tests/common::
   start_daemon()`，理由见上面「简报核对」一节）；`ui/mod.rs` 那条额外加了
   一次提示符等待（原来完全没等，属于同一根因下的一个更激进的竞态点）。
2. `style: run cargo fmt on grid_reply.rs` —— `cargo fmt --check` 揪出来的
   格式修正。

## 没做但值得注意的

- `SessionInfo { profile: ... }` 那个字段在 `ui/mod.rs` 的测试里从
  `"shell"` 改成了新 profile 名，纯粹是为了跟 `Request::Create` 传的名字对
  得上、少一处误导；确认过这个字段在 `enter_session` 的生产代码里没被读，
  改不改都不影响断言结果。
- 没有触碰 `.superpowers/sdd/.gitignore` 和 `progress.md`。
