# Task 7 报告：可用性判定

## 实现内容

在 `src/profile.rs` 里加了第三块模块级函数（磁盘加载函数之后）：

- `pub enum ProfileStatus { Ready, NeedsSecret, NeedsDependency { label: String }, NotInstalled { command: String } }`
  —— `#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]`
- `pub fn command_exists(cmd: &str) -> bool` —— 带斜杠当路径查（`Path::new(cmd)` 直接 stat），
  否则按 `:` 切 `PATH`，逐目录拼接后 stat，要求是普通文件且带任一执行位（`mode & 0o111 != 0`）。
- `fn dependency_owner<'a>(all: &'a [Profile], cmd: &str) -> Option<&'a Profile>` —— 私有辅助函数，
  取 `cmd` 的 basename，在 `all` 里找 `name` 与之相等的 profile，即“这个命令归谁所有”。
- `pub fn status_of(p, all, has_secret, installed: &dyn Fn(&str) -> bool, lang) -> ProfileStatus`
  —— 先看 `command.first()`（空则 `NotInstalled { command: "" }` 兜底）；再 `installed(cmd)`，
  没装的话按 `dependency_owner` 区分「自己没装」还是「依赖没装」；装了才轮到 `secret` 判定。

代码与任务简报里的代码逐字一致，只是 `cargo fmt` 把两个多字段 enum variant 的花括号换行成了标准 rustfmt 风格（纯格式，无逻辑改动）。

## 测试与结果

按简报 Step 1 原样加了 `status_fixture()` 和全部 8 个测试到 `mod tests`。

### TDD 证据

**RED**（写完测试、实现代码之前）：

```
$ ~/.cargo/bin/cargo test --lib profile
```

```
error[E0425]: cannot find function `command_exists` in this scope
   --> src/profile.rs:346:17
    |
346 |         assert!(command_exists("/bin/sh"));
    |                 ^^^^^^^^^^^^^^ not found in this scope
...
error[E0433]: cannot find type `ProfileStatus` in this scope
   --> src/profile.rs:279:30
    |
279 |         assert!(matches!(st, ProfileStatus::Ready));
    |                              ^^^^^^^^^^^^^ use of undeclared type `ProfileStatus`
...
error: could not compile `dct` (lib test) due to 16 previous errors
```

符合预期——`status_of` / `command_exists` / `ProfileStatus` 都还不存在，编译不过（比简报预告的“cannot find function `status_of`”更早一步，因为 `ProfileStatus` 类型本身也没定义，两者一起报错，性质相同）。

**GREEN**（实现完之后）：

```
$ ~/.cargo/bin/cargo test --lib profile
```

```
running 32 tests
test profile::tests::builtin_names_includes_claude_and_shell ... ok
...
test profile::tests::dependency_uses_the_owner_profiles_label_not_the_raw_command ... ok
test profile::tests::dependency_is_reported_before_secret ... ok
test profile::tests::not_installed_when_the_command_owns_its_name ... ok
test profile::tests::profile_without_secret_is_ready_when_installed ... ok
test profile::tests::ready_when_installed_and_secret_present ... ok
test profile::tests::needs_secret_when_installed_but_no_key ... ok
test profile::tests::command_exists_finds_sh_and_not_a_made_up_name ... ok
test profile::tests::command_exists_handles_absolute_paths ... ok
...
test result: ok. 32 passed; 0 failed; 0 ignored; 0 measured; 79 filtered out; finished in 0.00s
```

全部 32 个 profile 模块测试通过（含之前存量的 24 个 + 新增 8 个）。

### 提交前全量验证

```
$ ~/.cargo/bin/cargo fmt
$ git diff --check          # 无输出，无尾随空白问题
$ ~/.cargo/bin/cargo test
```

全部单元测试 + 每个集成测试文件（`cli.rs`、`client_timeout.rs`、`concurrency.rs`、`daemon_detach.rs`、
`daemon_roundtrip.rs`、`projects_flow.rs`、`signal_restore.rs`、`slow_input.rs`、`socket_perms.rs`）
全绿，doc-tests 0 个（无 doc example）。`cargo fmt` 只重排了两个 enum variant 的花括号换行，没有别的改动。

## 自查两问的回答

**问 1：`command_exists` 对空 `PATH` 段、`PATH` 未设置、不可读目录、名字实际是目录的情况怎么处理？**

- **空 `PATH` 段**（比如 `PATH=/bin::/usr/bin`）：`.filter(|d| !d.is_empty())` 显式过滤掉了空段，
  不会落到“空字符串当路径”从而变相变成“当前目录”的传统 shell 语义——这正是我们要的：dct 不该把
  cwd 当 PATH 的一部分来搜命令。
- **`PATH` 未设置**：`std::env::var("PATH")` 返回 `Err`，直接 `return false`。保守处理，符合“找不到就是找不到”的语义，不会 panic。
- **不可读目录**（比如目录本身没有可执行/搜索位）：`std::fs::metadata(p)` 对目录内文件做 stat 需要
  该目录的执行（搜索）位，没有的话 stat 会失败，`Err` 走 `.unwrap_or(false)`，结果是 `false`——
  同一命令即使真的躺在那个目录里，也会被判定为“不存在”。这和 shell 实际尝试 exec 时会遇到的
  失败是同构的（没有搜索权限，exec 也会失败），所以不算判定错误，只是稍微悲观。
- **名字是目录而非文件**：`m.is_file()` 对目录返回 `false`，被过滤掉，正确不算“可执行”。

四种情况都不会 panic，也不会误报“可用”，全部合理地退化为 `false`。判断为：**符合预期，无需改动**。

**问 2：`status_of` 对空 `command` 返回 `NotInstalled { command: String::new() }`——空 `command` 真的可达吗？该显示成什么？**

可达。`Profile.command: Vec<String>` 字段没有 `#[serde(default)]`，TOML 必须显式给出，但
`command = []` 是合法 TOML，`toml` crate 会正常反序列化成空 `Vec`——**只对内置 profile**有
`every_builtin_parses_and_is_well_formed` 测试兜底“`command` 不能为空”，这个约束不对用户自己
放在 `~/.dct/profiles/*.toml` 里的自定义 profile 生效。也就是说，一个手写错的自定义 profile
文件（漏填 `command` 的值、只写了 `command = []`）会一路顺利通过 `load_dir` / `all_profiles`，
落到 `status_of` 时才第一次被拦下，得到 `NotInstalled { command: "" }`。

我的判断：**保留简报里的行为不改**（简报明确写了这段兜底就是防 spawn 时 panic，是一处防御性代码，
不是这个任务要解决的“正常路径”），但这确实是留给下一棒的一个真实缺口——Task 8（渲染状态）如果
直接把 `command` 塞进「未安装（{command}）」这样的模板，空 `command` 会渲染成「未安装（）」，
一个非程序员看到会不知道括号里本该有什么。建议 Task 8 渲染这条分支时，对空 `command` 单独兜底成
类似「未安装（配置有问题）」或类似人话，而不是原样嵌入空字符串。这里不擅自改 `status_of` 的返回值
（该返回什么是 Task 8 的消费者决定，产生空字符串本身没问题，问题在展示层），只把这个观察点写进报告，
按任务说明的要求交给评审判断要不要在 Task 8 里处理。

## 文件改动

- `/Users/lei/work/dc/dc-terminal/.claude/worktrees/multi-agent/src/profile.rs`
  （+167 行：`ProfileStatus`、`command_exists`、`dependency_owner`、`status_of`，加 8 个测试与 `status_fixture` 辅助函数）

文件现在 842 行（原 675 行）。任务说明要求“检查文件大小并汇报是否变得笨重，但不要自行拆分”——
842 行对一个既管解析、又管磁盘加载、现在又加了可用性判定的模块来说已经不小了，三块职责（TOML 结构与
`impl Profile`、磁盘 I/O、可用性判定）边界还算清楚，但如果后续任务还要往这个文件里加东西，可能是拆分
的信号。本任务未做任何拆分。

## 自查发现

- 逐字核对了简报里的类型签名、枚举变体、注释、测试内容，无偏离（只有 rustfmt 的换行风格差异）。
- `command_exists` 和 `status_of` 都严格按“先装没装、后密钥”的顺序实现，
  `dependency_is_reported_before_secret` 测试单独验证了这一点且通过。
- 没有引入任何新的 UI 侧调用——`command_exists` 目前只在 `src/profile.rs` 内定义并由测试和
  `status_of`（经由 `installed` 参数注入，测试用假函数替换）调用，本任务没有在别处接线，
  符合“只能在守护进程里调用”的要求（接线是后续任务的事）。
- 未修改 `.superpowers/sdd/.gitignore`，未 `git add -A`，只 `git add src/profile.rs`。

## 疑虑

无阻塞性疑虑。唯一值得后续任务注意的是上面“自查问 2”里提到的空 `command` 渲染问题，留给 Task 8 处理。
