# Task 3 报告：PTY 与屏幕缓冲

## 状态

DONE

## 做了什么

按 brief 的步骤顺序实现 `src/pty.rs`，暴露 `PtySession::spawn/write/screen_text/is_alive/kill`，并在 `src/main.rs` 加 `mod pty;`。

### Step 1：先写失败的测试

只把 brief 里的 `#[cfg(test)] mod tests { ... }` 写入全新的 `src/pty.rs`（此时文件里没有任何非测试代码），并在 `src/main.rs` 加 `mod pty;`。

### Step 2：跑测试确认失败

命令：

```
export PATH="$HOME/.cargo/bin:$PATH"
cargo test pty
```

输出（节选）：

```
error[E0425]: cannot find type `PtySession` in this scope
 --> src/pty.rs:7:21
error[E0433]: cannot find type `PtySession` in this scope
  --> src/pty.rs:21:17
...
error: could not compile `dct` (bin "dct" test) due to 4 previous errors; 1 warning emitted
```

符合预期：`PtySession` 未定义，编译失败。

### Step 3：实现 PtySession

先用 `find` 定位到本机实际安装的依赖源码（`~/.cargo/registry/src/.../portable-pty-0.8.1`、`vt100-0.15.2`），核对了以下 API 与 brief 代码逐一比对：

- `portable_pty::NativePtySystem`（`unix` 上是 `UnixPtySystem`，`#[derive(Default)]`，因此 `NativePtySystem::default()` 可用）
- `PtySystem::openpty(&self, size: PtySize) -> anyhow::Result<PtyPair>`
- `PtyPair { pub slave: Box<dyn SlavePty + Send>, pub master: Box<dyn MasterPty + Send> }`
- `SlavePty::spawn_command(&self, cmd: CommandBuilder) -> Result<Box<dyn Child + Send + Sync>, Error>`（`Error = anyhow::Error`）
- `MasterPty::try_clone_reader(&self) -> Result<Box<dyn Read + Send>, Error>`
- `MasterPty::take_writer(&self) -> Result<Box<dyn Write + Send>, Error>`
- `vt100::Parser::new(rows: u16, cols: u16, scrollback_len: usize) -> Self`
- `vt100::Parser::process(&mut self, bytes: &[u8])`
- `vt100::Parser::screen(&self) -> &Screen`，`Screen::contents(&self) -> String`

结论：brief 里给的实现代码与本机实际安装的 0.8.1 / 0.15.2 API **完全一致**，未做任何调整，逐字采用。

随后把实现代码插入到测试模块之前。

### Step 4：跑测试确认通过

命令：

```
cargo build
```

输出：编译通过，仅有既有风格的 `dead_code` 警告（`PtySession` 及其方法尚未被 `main.rs` 使用，与 `git.rs`/`profile.rs` 现状一致，不是本次引入的新问题）。

命令：

```
cargo test pty -- --test-threads=1
```

输出：

```
running 3 tests
test pty::tests::captures_command_output ... ok
test pty::tests::reports_death ... ok
test pty::tests::writes_input_to_process ... ok

test result: ok. 3 passed; 0 failed; 0 ignored; 0 measured; 12 filtered out; finished in 0.18s
```

为排查 brief 提到的"这个任务的测试是有时序的"风险，把 `cargo test pty -- --test-threads=1` 连续跑了 5 次，全部 3/3 通过，用时均在 0.16–0.18s 区间，没有观察到 flaky 现象。

又跑了全量测试套件（含 `git`、`profile` 模块）：

```
cargo test -- --test-threads=1
```

```
running 15 tests
... (git:: 6 个, profile:: 6 个, pty:: 3 个)
test result: ok. 15 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.87s
```

全部通过。

### Step 5：格式化与提交

```
cargo fmt
```

只改动了 `src/main.rs`（在 `mod profile;` 和空行之间插入 `mod pty;` 后的标准换行，diff 为 1 行新增，无实质格式问题；`src/pty.rs` 本身未被 `fmt` 改动，说明 brief 给的代码已符合 rustfmt 默认风格）。

```
cargo fmt --check
```

通过，无输出。

```
git diff --check
```

无输出，无空白问题。

```
git add src/main.rs src/pty.rs
git commit -m "feat: PTY 会话与 vt100 屏幕缓冲"
```

提交成功：`d2c7430 feat: PTY 会话与 vt100 屏幕缓冲`（2 files changed, 147 insertions(+)）。

## 与 brief 的偏差

无代码偏差。实现代码与 brief 提供的代码逐字一致，未新增依赖，未调整任何 API 调用方式。

流程上的偏差：brief 的 Step 1 展示的代码块是"测试模块"本身，我先把它单独写入 `pty.rs`（此时文件中不存在任何 struct/impl），确认编译失败（`PtySession` 未定义）之后，再把 Step 3 的实现代码插入到测试模块之前，这样更贴合"先写失败测试、再实现"的 TDD 顺序，而不是一次性把 brief 里两段代码拼好再跑测试。

## 自查发现的问题

- `cargo build`（不带 `--tests`）会给出 `PtySession`、`spawn`、`write`、`screen_text`、`is_alive`、`kill` 均为 `never used`（dead_code）的警告，因为 `main.rs` 目前只有 `println!("dct")`，还没有代码路径调用 `pty` 模块。这与仓库里 `git.rs`、`profile.rs` 当前的状态完全一样（它们的 pub 接口现在也全部是 dead_code 警告来源），是历史任务留下的既有模式，不是本任务引入的新问题，预计后续任务把 `main.rs` 接起来后会自然消失。
- 未发现 flaky 测试；`reports_death` 用例（spawn `true` 后轮询 `is_alive()` 直到超时或进程退出）在 5 次重复运行中均在极短时间内收敛为 `false`，没有卡到 5 秒超时的情况。

## 第二轮：代码审查修复（子进程回收）

审查发现一个 Critical + 三个 Important，都围绕"子进程回收"这条主线：

1. **[Critical]** `PtySession` 没有 `Drop`，丢弃对象后子进程变僵尸。
2. **[Important]** 读线程理论上可能永久悬挂在 `reader.read()`（`try_clone_reader()` 拿到的是独立 dup fd，PTY slave 不产生 EOF/HUP 就不会返回）。
3. **[Important]** `kill()` 只发信号不 `wait()`，portable-pty 在 unix 上 kill 内部有约 200ms 宽限期，超时会退化为 SIGKILL 且这条路径不再自动 `wait()`，导致 `kill()` 返回后子进程仍可能是僵尸。
4. **[Important]** `is_alive()` 把 `try_wait()` 的 `Err`（例如 ECHILD，子进程已被回收过一次）当作"存活"，`match ... { Ok(Some(_)) => false, _ => true }` 的 `_` 分支吞掉了 `Err`。

### 先加测试，确认改代码前失败

在 `tests` 模块末尾按审查者给的思路加了 `drop_reaps_child_process`，并给 `PtySession` 加了只读转发方法 `pub fn process_id(&self) -> Option<u32>`（转发 `portable_pty::Child::process_id`），仅用于测试拿到 pid 供 `ps` 检查。

命令（改 `Drop`/`kill`/`is_alive` 之前）：

```
export PATH="$HOME/.cargo/bin:$PATH"
cargo test pty::tests::drop_reaps_child_process -- --test-threads=1 --nocapture
```

输出：

```
running 1 test
test pty::tests::drop_reaps_child_process ...
thread 'pty::tests::drop_reaps_child_process' (37520941) panicked at src/pty.rs:173:13:
drop 之后子进程是僵尸: Z
note: run with `RUST_BACKTRACE=1` environment variable to display a backtrace
FAILED

failures:
    pty::tests::drop_reaps_child_process

test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 15 filtered out; finished in 0.07s
```

确认复现：drop 之后子进程确实变成僵尸（`ps` 的 `STAT` 列是 `Z`），与审查者的实测结果一致。

### 修复实现

- `kill(&mut self)`：kill 之后显式 `wait()` 一次回收，两步都用 `let _ = ...` 吞掉错误（僵尸已被回收后再 `wait()` 会报错，属预期，不应该让 `kill()` 失败）。
- `is_alive(&self)`：把 `match` 拆成三支——`Ok(Some(_))` 和 `Err(_)` 都判定为已死并把 `alive` 置 `false`，只有 `Ok(None)` 才算存活。
- 新增 `impl Drop for PtySession`：`drop()` 里调用 `self.kill()` 并丢弃返回值，不 panic。
- 新增 `pub fn process_id(&self) -> Option<u32>`，仅用于测试验证 pid 是否被回收（不是多余接口，测试必需）。

关于第 2 点（读线程悬挂）：没有单独改代码，也没有单独加测试，是靠 1、3 的修复自然解决的——`Drop`/`kill()` 现在会先 kill 子进程再 `wait()` 回收；子进程退出会导致 PTY slave 侧不再有任何进程持有该 tty，master 端阻塞的 `read()` 会返回 `Ok(0)`（EOF）或 `Err`（如 EIO），而 `reader.read()` 的匹配分支 `Ok(0) | Err(_) => break` 本来就会让读线程退出循环。`drop_reaps_child_process` 测试能在 5 秒截止时间内稳定通过（多次运行都在 100ms 级别完成，见下），间接印证了子进程被回收后 pty 侧确实很快产生了让读线程退出所需的信号——如果读线程真的永久悬挂，理论上不会影响这个基于 `ps` 的测试本身（该测试不等待线程），但也没有观察到任何异常（比如残留线程导致进程无法退出、CPU 占用异常等）。这一条严格来说是"逻辑推理 + 间接观察"而非"直接测试验证"，如需要更硬的证据（比如显式记录并 `join` 读线程、在 Drop 里加超时等待），需要额外改动结构体持有 `JoinHandle`，超出本轮审查列出的必改项范围，先如实说明。

### 改代码后：确认新测试通过，且不是偶发

命令：

```
cargo test pty -- --test-threads=1
```

连续跑 3 次，输出（每次都是）：

```
running 4 tests
test pty::tests::captures_command_output ... ok
test pty::tests::drop_reaps_child_process ... ok
test pty::tests::reports_death ... ok
test pty::tests::writes_input_to_process ... ok

test result: ok. 4 passed; 0 failed; 0 ignored; 0 measured; 12 filtered out; finished in 0.34s
```

（三次分别用时 0.34s / 0.33s / 0.34s，稳定。）

### 全量测试 + fmt

```
cargo test -- --test-threads=1
```

```
running 16 tests
test git::tests::checkpoint_commits_pending_changes ... ok
test git::tests::checkpoint_then_reset_discards_changes ... ok
test git::tests::creates_and_removes_worktree ... ok
test git::tests::detects_repo ... ok
test git::tests::diff_stat_includes_untracked_new_files ... ok
test git::tests::diff_stat_reports_changes ... ok
test profile::tests::builtin_claude_uses_bypass_flag ... ok
test profile::tests::builtin_names_lists_both ... ok
test profile::tests::builtin_shell_is_not_agent ... ok
test profile::tests::idle_regex_compiles ... ok
test profile::tests::parses_toml ... ok
test profile::tests::unknown_builtin_is_none ... ok
test pty::tests::captures_command_output ... ok
test pty::tests::drop_reaps_child_process ... ok
test pty::tests::reports_death ... ok
test pty::tests::writes_input_to_process ... ok

test result: ok. 16 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 1.42s
```

全部通过，没有把 `git`/`profile` 模块跑挂。

```
cargo fmt
```

只改动了 `src/pty.rs`（`diff --stat`: `1 file changed, 58 insertions(+), 2 deletions(-)`），主要是把新增代码格式化为 rustfmt 风格（例如把 `drop_reaps_child_process` 里一条较长的 `assert!` 拆成多行）。

```
cargo fmt --check
```

通过，无输出。

```
git diff --check
```

无输出，无空白问题。

`cargo build` 仍然只有既有风格的 `dead_code` 警告（`PtySession` 及其方法目前只被测试代码使用，`main.rs` 还没接线），新增的 `process_id` 也在同一警告列表里，属于同一既有模式，不是新问题。

### 提交

```
git add src/pty.rs
git commit -m "fix: PtySession 增加 Drop 回收子进程，修复僵尸进程与 is_alive 误判"
```

## 最终结论

- Critical（无 Drop 导致僵尸）：已修复，`drop_reaps_child_process` 测试改代码前失败（观测到 `Z` 僵尸态）、改代码后连续 3 次通过。
- Important（读线程可能悬挂）：通过修复 1、3 间接解决，逻辑上已闭环，但没有独立的直接测试覆盖“读线程一定会退出”这件事——如需要更强证据需额外结构改动（持有并 `join` `JoinHandle`），本轮未做。
- Important（`kill()` 后仍可能是僵尸）：已修复，`kill()` 内部现在会显式 `wait()`。
- Important（`is_alive()` 把 `Err` 当存活）：已修复，`Err` 分支现在判定为已死并回写 `alive` 标志。
