# Task 8 报告：协议与守护进程

## 实现内容

### `src/proto.rs`
- 新增 `SecretPrompt { hint, url }`、`InstallPrompt { command, note }`、
  `ProfileEntry { name, label, note, status, secret, install }`——全部字段是
  已经按语言选定好的 `String`，不把 `LocalizedText` 送过线。
- `Request` 新增 `SetSecret { profile, value }`、`DeleteSecret { profile }`、
  `LastProfile`；`Create` 加 `remember: bool` 字段。
- `Response::Profiles` 从 `Vec<String>` 改成 `{ entries: Vec<ProfileEntry>,
  warning: Option<String> }`；新增 `Response::LastProfile(Option<String>)`。

### `src/projects.rs`
- `Disk` / `Store` 都加了 `last_profile: Option<String>` 字段，`#[serde(default)]`
  保证老文件（没有这个字段）照样能读。
- 新增 `Store::last_profile() -> Option<&str>` 和
  `Store::set_last_profile(&mut self, name: &str)`（写完立即 `save()`，
  跟 `touch()` 同一套落盘纪律——守护进程没有干净退出的钩子）。

### `src/session.rs`
- `resolve_profile` 加一个 `profiles: &[Profile]` 参数，查找顺序：先查这个
  切片（daemon 传进来的「内置 + 磁盘」全集）、再查 `extra_profiles`（测试
  注册入口）、最后落到编译进二进制的内置表。
- `SessionManager::create` 签名相应加上 `profiles: &[Profile]` 这个新参数
  （在 Task 5 定的 `create(dir, profile_name, secret)` 基础上追加，不是重排）。
- 本文件 `mod tests` 里全部 16 处 `.create(...)` 调用改成传 `&[]`（这些测试
  都是通过 `register_profile` 走 `extra_profiles`，不关心磁盘 profile）。

### `src/daemon.rs`
- `serve()` / `handle()` 都加了 `profiles_dir: &Path`（`serve` 拿的是拥有的
  `PathBuf`，跟着每个连接线程 clone 一份）。
- `Request::Profiles`：调用 `all_profiles(profiles_dir)` 拿到「内置 + 磁盘」
  全集和磁盘文件的错误列表；再借一下 `secrets` 锁看 `load_error()` 和
  逐条 `get(&p.name).is_some()`；组出 `ProfileEntry` 列表和合并后的
  `warning`（密钥文件错误插在最前面）。
- `Request::Create`：新增 `remember` 字段的处理——建会话成功且 `remember`
  为真才 `set_last_profile`；同时补上 `all_profiles(profiles_dir)` 传给
  `mgr.create()`，让 `resolve_profile` 认得磁盘 profile。**密钥锁依旧只借
  一下就放**（`recover(secrets.lock()).get(&profile).map(str::to_string)`
  是一条独立语句，guard 是临时值，语句结束就释放），慢操作（PTY 起进程/
  git checkpoint）全程不持有它。
- 新增 `Request::SetSecret` / `DeleteSecret` / `LastProfile` 分支。

### `src/ui.rs`（只改了两处，见下方专门小节）

## 测试与结果

### TDD Evidence

**RED**（写完 `src/projects.rs` 的两个新测试、`tests/profiles_flow.rs`、
`tests/common/mod.rs`，尚未改动 `Store`/`proto`/`daemon`/`session` 之前）：

```
$ ~/.cargo/bin/cargo test
   Compiling dct v0.1.0 (...)
error[E0599]: no method named `set_last_profile` found for struct `projects::Store` in the current scope
error[E0599]: no method named `last_profile` found for struct `projects::Store` in the current scope
error[E0599]: no method named `last_profile` found for struct `projects::Store` in the current scope
error: could not compile `dct` (lib test) due to 3 previous errors
```

这是预期的失败：新测试引用了还不存在的 `Store::last_profile` /
`Store::set_last_profile`。lib 编译不过，整个 workspace（含
`tests/profiles_flow.rs` 用到的 `Response::Profiles { .. }` 等新协议形状）
也一并卡在编译阶段，没有跑到执行层面——这正是本任务改动横跨多个文件、
互相依赖的体现。

**GREEN**（实现完 `proto.rs` / `projects.rs` / `session.rs` / `daemon.rs` /
`ui.rs`，并更新了受影响的既有测试之后）：

```
$ ~/.cargo/bin/cargo test
...
test result: ok. 113 passed; 0 failed; 0 ignored; ... (src/lib.rs 单元测试，含
  projects::tests::last_profile_survives_reload、
  projects::tests::old_file_without_last_profile_still_loads、
  daemon::tests::create_does_not_hold_the_secrets_lock_across_the_slow_work)
...
     Running tests/profiles_flow.rs
running 5 tests
test profiles_returns_entries_with_labels_and_status ... ok
test delete_secret_puts_it_back ... ok
test set_secret_flips_kimi_off_needs_secret ... ok
test create_without_remember_does_not_record ... ok
test create_with_remember_records_the_profile ... ok
test result: ok. 5 passed; 0 failed; ...
     Running tests/projects_flow.rs
running 3 tests
test result: ok. 3 passed; 0 failed; ...
（daemon_roundtrip / concurrency / slow_input / socket_perms / signal_restore /
  daemon_detach / client_timeout / cli 全部 ok，见下方完整跑一次的汇总）
```

完整跑一次（`cargo fmt` 之后，`cargo test` 全量，用 `tee` + `grep` 摘出每个
test binary 的汇总行，确认没有任何一行 `FAILED`）：

```
$ ~/.cargo/bin/cargo test 2>&1 | tee /tmp/full_test_run.log | grep -E "FAILED|error\[|test result"
test result: ok. 113 passed; 0 failed; ...   (unittests src/lib.rs)
test result: ok. 0 passed; 0 failed; ...     (unittests src/main.rs)
test result: ok. 2 passed; 0 failed; ...     (tests/cli.rs)
test result: ok. 1 passed; 0 failed; ...     (tests/client_timeout.rs)
test result: ok. 1 passed; 0 failed; ...     (tests/concurrency.rs)
test result: ok. 1 passed; 0 failed; ...     (tests/daemon_detach.rs)
test result: ok. 2 passed; 0 failed; ...     (tests/daemon_roundtrip.rs)
test result: ok. 5 passed; 0 failed; ...     (tests/profiles_flow.rs)
test result: ok. 3 passed; 0 failed; ...     (tests/projects_flow.rs)
test result: ok. 2 passed; 0 failed; ...     (tests/signal_restore.rs)
test result: ok. 1 passed; 0 failed; ...     (tests/slow_input.rs)
test result: ok. 1 passed; 0 failed; ...     (tests/socket_perms.rs)
test result: ok. 0 passed; 0 failed; ...     (doctests)
```

（中途第一次全量跑时 `tests/client_timeout.rs` 撞过一次瞬时超时失败——单独
重跑、以及后续两次全量重跑都稳定通过；这是一条依赖真实时钟窗口的既有测试，
本任务没有碰过这个文件的逻辑，判断是并行编译/运行时的系统抖动，不是本次
改动引入的问题。）

`~/.cargo/bin/cargo clippy --all-targets` 干净，无警告无错误。
`git diff --check` 无空白问题。

## common 模块抽取的决定

`tests/common/mod.rs` 新增 `DaemonHandle`（`start_daemon()` 返回），提供
`.client()`（每次开一条新连接）和 `.git_repo(name)`（在这个 handle 的临时
`home` 下建一个初始化好的 git 仓库，生命周期跟着 handle 走，不需要额外
`Mutex<Vec<TempDir>>` 之类的内部可变性）。

- **移过去的**：`tests/projects_flow.rs`（brief 明确要求，行为不变，三个
  测试原样通过）、`tests/daemon_roundtrip.rs`（起法和 `projects_flow.rs`
  完全一样——纯 `dct::daemon::run(&s)` + 等 socket 出现，是一个干净的win；
  它本来就要因为 `Create` 加字段而改动，顺手抽掉重复代码）。
- **没有移的，及原因**：
  - `tests/concurrency.rs`：起 daemon 之前要往一个自定义 `SessionManager`
    里 `register_profile` 一个测试专用的慢 profile，`start_daemon()` 这个
    「零参数、内部自己 new manager」的形状塞不下这个需求；硬塞一个
    可选参数会让这份本该单纯的脚手架为了一个调用方多分支。
  - `tests/daemon.rs` 里的 `create_does_not_hold_the_secrets_lock_across_the_slow_work`
    同理，且它直接调用内部 `handle()` 而不是走 socket，跟 `common` 的
    「起一个真进程」定位不同。
  - `tests/socket_perms.rs`：故意把 socket 放在 `dir.path().join("sub").join(...)`
    这种**还不存在的子目录**里，用来验证 daemon 会 `create_dir_all` + 收紧到
    `0700`。`common::start_daemon()` 的 `home` 本身是 tempdir，默认权限已经
    收紧，socket 直接建在 `home` 根下——套用 common 会让这条测试测的东西
    从「daemon 有没有正确创建并收紧父目录权限」退化成「tempdir 本来就是
    0700」，看着通过但没测到原来想测的事，所以没动。
  - `tests/client_timeout.rs`：起的是一个自定义的、故意拖慢应答的
    `UnixListener` 服务端，不经过 `dct::daemon::run`，没有可抽的公共部分。
  - `tests/signal_restore.rs` / `tests/daemon_detach.rs`：拉起的是编译好的
    `dct` 二进制本身（真实子进程 + pty），不是进程内调用
    `dct::daemon::run`，跟 `common` 的形状是两回事。
  - `tests/cli.rs`：根本不起 daemon。

## `src/ui.rs` 的改动与「为什么是最小改动」

只改了两处，都只求编译通过，不碰 `View::PickProfile` 的形状（那是 Task 10
的事）：

1. `n` 键那里：`Response::Profiles(p)` 解构改成
   `Response::Profiles { entries, .. }`，然后
   `entries.into_iter().map(|e| e.name).collect()` 塞回原来的
   `View::PickProfile(Vec<String>)`。只取 `name`，丢弃了新协议里的
   `label`/`status`/`secret`/`install`——因为 Task 10 之前，UI 侧完全没有
   消费这些字段的代码（渲染、按键处理都还是老样子），提前塞进去也用不上，
   只会让这次「只求编译过」的改动看起来在动 UI 逻辑。
2. `Create` 请求字面量里加了 `remember: true`——用户在这个选择器里按数字
   选的就是「我要用这个 agent」，是唯一会经过这条代码路径的场景（「帮你
   装 CLI」那条 `remember: false` 的路径是 Task 9 才加的新分支，不在这里）。

没有改 `View::PickProfile` 的定义、没有改 `draw()` 里渲染 profile 列表的
代码、没有让菜单去读 `label`/`status`——这些都是 Task 10 的范围。

## 文件改动清单

- `src/proto.rs`：新增线上类型、Request/Response 变体
- `src/projects.rs`：`last_profile` 字段 + 两个方法 + 两个新测试
- `src/session.rs`：`resolve_profile`/`create` 签名加 `profiles` 参数，16 处内部测试调用点跟上
- `src/daemon.rs`：`serve`/`handle` 接线 `profiles_dir`；`Profiles`/`Create`/`SetSecret`/`DeleteSecret`/`LastProfile` 分支；既有回归测试跟上新签名
- `src/ui.rs`：两处调用点跟上新协议（如上）
- `tests/common/mod.rs`（新建）：`start_daemon()` / `DaemonHandle::client()` / `DaemonHandle::git_repo()`
- `tests/profiles_flow.rs`（新建）：5 个端到端测试，抄自 brief
- `tests/projects_flow.rs`：改用 `common`，行为不变
- `tests/daemon_roundtrip.rs`：改用 `common`，`Create` 加 `remember: true`
- `tests/concurrency.rs`：`Create` 加 `remember: true`
- `tests/slow_input.rs`：`SessionManager::create` 调用加 `&[]`

## 自我审查发现

- 密钥锁：确认 `Request::Create` 里查密钥的那一句是独立语句，`MutexGuard`
  是临时值，语句结束（不是整个 match 分支结束）就释放；`store` 锁和
  `secrets` 锁全程没有嵌套。既有回归测试
  `daemon::tests::create_does_not_hold_the_secrets_lock_across_the_slow_work`
  改了签名（加 `profiles_dir` 参数、`Create` 加 `remember` 字段）后依然通过。
- 密钥不外泄：`ProfileEntry`/`SecretPrompt` 只带 `hint`（profile 文件里写的
  提示文案）和 `url`（申领页面），从不带 `sec.get()` 取到的密钥值本身——
  `sec.get(&p.name).is_some()` 只取了布尔结果参与 `status_of` 判定。
  `SecretStore` 本身也没有 `#[derive(Debug)]`，误写 `{:?}` 会直接编译不过。
- 向后兼容：`old_file_without_last_profile_still_loads` 验证了没有
  `last_profile` 字段的老 `projects.json` 照样能读（`#[serde(default)]`）。
- 顺手把 `daemon.rs` 里一条已经过时的注释（原来说「`SetSecret` 请求还不
  存在，Task 8 才加」）改成了跟当前代码状态一致的描述，没有留一句自相
  矛盾的话在代码里。

## 问题或顾虑

- 无阻塞性问题。`tests/client_timeout.rs` 在一次全量并行跑测中出现过一次
  超时类失败，重跑即过，判断为既有测试对时钟窗口的敏感度，与本任务改动
  无关（本任务没有碰这个文件）；后续两次全量重跑均稳定绿。
