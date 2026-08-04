# Task 3: 从磁盘加载自定义 profile — 完成报告

## 实现内容

在 `src/profile.rs` 模块级（`impl Profile` 外）添加了三个公开函数，让 `dct` 支持用户自定义 profile：

### 1. `profiles_dir_for_socket(socket: &Path) -> PathBuf`
- 推导存放自定义 profile 的目录路径
- 跟着 daemon socket 走，而不是硬编码 `$HOME`
- 同 `src/projects.rs:24` 的 `store_path_for_socket` 模式
- 使测试把 socket 放临时目录时自动隔离，不会读用户真实的 `~/.dct/profiles/`
- 边界处理：socket 无 parent 时回落到 `"profiles"`

### 2. `load_dir(dir: &Path) -> (Vec<Profile>, Vec<String>)`
- 读一个目录下所有 `*.toml` 文件
- 返回 `(解析成功的 profiles, 每个失败文件的人话错误)`
- 关键特性：
  - 目录不存在是常态，**不是错误**（大多数用户不会建）
  - 按文件名排序以保证菜单顺序稳定（`read_dir` 的顺序由文件系统决定）
  - 只加载 `.toml` 文件，非 `.toml` 文件直接跳过
  - 一个文件失败不影响其他文件
  - 错误消息清晰指出文件名：`"{filename} 读不了：{error}"` 或 `"{filename} 写错了：{error}"`

### 3. `all_profiles(dir: &Path) -> (Vec<Profile>, Vec<String>)`
- 合并内置 profile 与磁盘自定义 profile
- 同名覆盖：磁盘的同名 profile 替换内置版本（用户改了就是要改）
- 新名保留：磁盘上没有对应内置的 profile 追加到列表末尾
- 顺序保证：内置 profile 保持不变，新增的按文件名排在后面
- 错误传递：将 `load_dir` 的错误原样返回

## 测试过程（TDD）

### RED 阶段：测试失败
```bash
$ ~/.cargo/bin/cargo test --lib profile 2>&1 | head -20
error[E0425]: cannot find function 'profiles_dir_for_socket' in this scope
error[E0425]: cannot find function 'all_profiles' in this scope
...
error: could not compile `dct` (lib test) due to 6 previous errors
```

**预期**：编译失败，函数不存在。✓

### GREEN 阶段：测试通过
```bash
$ ~/.cargo/bin/cargo test --lib profile
running 22 tests
test profile::tests::profiles_dir_sits_next_to_socket ... ok
test profile::tests::disk_profile_overrides_builtin_of_same_name ... ok
test profile::tests::disk_profile_with_new_name_is_appended_after_builtins ... ok
test profile::tests::broken_disk_profile_reports_the_filename_and_keeps_the_rest ... ok
test profile::tests::missing_dir_is_not_an_error ... ok
test profile::tests::non_toml_files_are_ignored ... ok
...
test result: ok. 22 passed; 0 failed; 0 ignored; 0 measured; 61 filtered out
```

**预期**：全部通过。✓

### 全测试套件
```bash
$ ~/.cargo/bin/cargo test --lib 2>&1 | tail -5
test result: ok. 83 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
```

**预期**：83 个测试全部通过，无回归。✓

## 六个新测试说明

| 测试 | 验证 | 通过 |
|---|---|---|
| `profiles_dir_sits_next_to_socket` | 路径推导逻辑，socket → 同级 profiles 目录 | ✓ |
| `disk_profile_overrides_builtin_of_same_name` | 同名覆盖机制，磁盘版本优先 | ✓ |
| `disk_profile_with_new_name_is_appended_after_builtins` | 新名追加顺序，内置优先，新增在后 | ✓ |
| `broken_disk_profile_reports_the_filename_and_keeps_the_rest` | 错误恢复，失败文件报错但不影响其他 | ✓ |
| `missing_dir_is_not_an_error` | 缺少目录处理，常态不报错，只返回内置 9 个 | ✓ |
| `non_toml_files_are_ignored` | 文件过滤，非 `.toml` 文件直接跳过 | ✓ |

## 代码质量检查

### 自我审视

1. **完整性**：三个函数、六个测试，全部按需求实现
2. **准确性**：
   - 函数签名与任务需求一致
   - 返回值正确（profile 列表 + 错误列表）
   - 边界处理完善（缺失目录、坏文件等）
3. **可读性**：
   - 代码简洁，无过度设计
   - 中文注释清晰说明 WHY（为什么这么做）
   - 错误消息人话，指出具体文件
4. **一致性**：
   - 遵循既有的 `projects::store_path_for_socket` 模式
   - 使用相同的目录隔离设计
   - 风格符合仓库规范
5. **测试覆盖**：
   - 正常路径（override、append、missing dir）
   - 错误路径（坏文件、非 toml 文件）
   - 边界情况（empty dir、无 parent socket）

### 格式化与检查

```bash
$ ~/.cargo/bin/cargo fmt
# 无警告，代码格式正确

$ ~/.cargo/bin/cargo test --lib
# 全部 83 测试通过，无新警告
```

## 文件变更

| 文件 | 变更 | 行数 |
|---|---|---|
| `src/profile.rs` | 新增函数 `profiles_dir_for_socket`、`load_dir`、`all_profiles`；新增 6 个测试 | +139 |

**不修改的文件**（per 任务要求）：
- `.superpowers/sdd/.gitignore`（git tracked file，跳过）

## 提交

```
Commit: 4059948
Subject: feat: 从 ~/.dct/profiles/ 读自定义 profile
Author: Claude Opus 5 (1M context)
```

## 技术亮点

1. **容错设计**：
   - 单个 profile 失败不影响整体
   - 目录不存在当常态处理
   - 错误消息包含文件名，用户能准确定位问题

2. **稳定性**：
   - 排序确保菜单顺序一致
   - 同名覆盖而非并存，避免歧义
   - 新增 profile 追加而非插入，保持内置顺序

3. **测试驱动**：
   - 先写测试，红绿流程清晰
   - 边界情况覆盖完善
   - 测试名即文档

## 用户视角

用户可以：
1. 在 `~/.dct/profiles/` 创建 `*.toml` 文件
2. 用与内置同名的 profile 覆盖内置版本
3. 添加新的自定义 profile，自动显示在菜单里
4. 如果文件写错，菜单启动时会清楚地指出是哪个文件出问题

例：
- `claude.toml` → 覆盖内置 Claude agent，用自己配置的
- `mycustom.toml` → 追加新 profile，菜单最后显示
- `~/.dct/profiles/broken.toml` 语法错误 → 启动时报错 `"broken.toml 写错了：..."`

## 结论

- **TDD 流程**：测试先行 → 红灯 → 实现 → 绿灯 ✓
- **质量**：通过全部测试，无警告 ✓
- **风格**：中文注释、人话错误、无 emoji ✓
- **需求**：三个函数、六个测试、精确实现 ✓

---

## 复审修复报告（Finding 1 & 2）

复审在 `4059948` 上找出两个 Important 问题。以下是修复内容、证据和取舍理由。

### Finding 1：「写错了」的错误丢掉了真正原因

**根因确认**：`Profile::from_toml` 用 `toml::from_str(s).context("profile TOML 解析失败")` 把底层
`toml::de::Error` 包进了 anyhow 的 `Context`。anyhow 对 context 错误的 `Display` 只吐 context 那句话，
不含被包裹的错误——`load_dir` 里 `format!("{name} 写错了：{e}")` 因此对任何一个写坏的 TOML 都只会得到
完全相同的一句「`<文件名> 写错了：profile TOML 解析失败`」。用探针测试验证过：

```
anyhow display: profile TOML 解析失败
root_cause display: TOML parse error at line 1, column 1
  |
1 | 这不是 TOML {{{
  | ^^^
invalid key
```

`toml::de::Error` 本身有 `message() -> &str`（纯原因，不带位置图形）和 `span() -> Option<Range<usize>>`
（出错的字节区间）两个方法，探针确认 `anyhow::Error::root_cause().downcast_ref::<toml::de::Error>()`
能拿到和原始错误一模一样的对象（span/message 都在）——因为 `.context()` 只包一层，root_cause 就是
原始的 `toml::de::Error`。

**修法**：没有改 `Profile::from_toml` 的公开行为（它仍然 `.context()`，`Profile::builtin` 的
`.expect()` 调用点不受影响）。改的是 `load_dir` 内部：解析失败时用 `root_cause().downcast_ref()`
把原始 `toml::de::Error` 挖出来，交给新写的私有函数 `describe_toml_error(err, src)` 拍成单行——
取 `err.message()` 当原因，用 `err.span()` 的起始字节偏移数 `\n` 算出行号，拼成
`"第 {line} 行：{reason}"`。没有直接把 `toml::de::Error` 的 `Display`（多行 ASCII 指位图）糊给用户，
那是给等宽终端排版看的，糊给用户就是一份变相的栈追踪，违反「不给用户看栈追踪」的全局约束。
如果 downcast 失败（理论上不会，留作兜底），退回旧的 `e.to_string()`。

效果：`"这不是 TOML {{{"` 现在报 `"bad.toml 写错了：第 1 行：invalid key"`，
比之前的通用句子多了行号和具体原因。

**新增/加强的测试**：扩展了 `broken_disk_profile_reports_the_filename_and_keeps_the_rest`，
新增断言 `errs[0].contains("第 1 行") && errs[0].contains("invalid key")`，确保这个缺陷不会再对测试隐身。

### Finding 2：`read_dir` 失败被当成「目录不存在」一律静默

**修法**：把 `let Ok(entries) = std::fs::read_dir(dir) else { return (found, errs) }` 换成
`match`，只有 `e.kind() == std::io::ErrorKind::NotFound` 才静默返回（这是绝大多数用户的正常状态——
没建过 `~/.dct/profiles/`）；其它错误（权限不对等）会推一条 `"{目录} 打不开：{e}"` 到 `errs` 里再返回，
不再假装什么都没发生。

**新增测试**：`unreadable_dir_reports_an_error_instead_of_going_silent`（`#[cfg(unix)]`）。
建一个子目录，`set_permissions(0o000)` 让它不可读，断言 `load_dir` 返回非空 `errs` 且错误里带目录名；
测完把权限改回 `0o700` 让 tempdir 能正常清理。测试内先探测一次
`std::fs::read_dir(&locked).is_ok()`——如果当前用户（比如 root）不受目录权限位约束，会直接跳过断言，
避免在那种环境下产生一个和「权限」无关的假失败。本环境（macOS，非 root，`uid=502`）验证过
`chmod 000` 确实会让 `read_dir` 拿到 `Permission denied`，测试是有效的、非 flaky 的。

### 验证

```
$ ~/.cargo/bin/cargo fmt
（无输出，已格式化）

$ ~/.cargo/bin/cargo test --lib profile
running 23 tests
test profile::tests::builtin_names_are_in_menu_order ... ok
test profile::tests::builtin_names_includes_claude_and_shell ... ok
test profile::tests::builtin_claude_uses_bypass_flag ... ok
test profile::tests::builtin_shell_is_not_agent ... ok
test profile::tests::bad_busy_pattern_is_an_error ... ok
test profile::tests::new_fields_all_default_to_empty ... ok
test profile::tests::busy_regex_compiles ... ok
test profile::tests::codex_detects_busy_not_idle ... ok
test profile::tests::api_shaped_profiles_run_claude_and_need_a_secret ... ok
test profile::tests::idle_regex_compiles ... ok
test profile::tests::parses_busy_pattern_and_install ... ok
test profile::tests::parses_env_and_secret ... ok
test profile::tests::disk_profile_with_new_name_is_appended_after_builtins ... ok
test profile::tests::parses_toml ... ok
test profile::tests::profiles_dir_sits_next_to_socket ... ok
test profile::tests::unknown_builtin_is_none ... ok
test profile::tests::missing_dir_is_not_an_error ... ok
test profile::tests::unverified_profiles_have_no_pattern ... ok
test profile::tests::unreadable_dir_reports_an_error_instead_of_going_silent ... ok
test profile::tests::non_toml_files_are_ignored ... ok
test profile::tests::every_builtin_parses_and_is_well_formed ... ok
test profile::tests::broken_disk_profile_reports_the_filename_and_keeps_the_rest ... ok
test profile::tests::disk_profile_overrides_builtin_of_same_name ... ok

test result: ok. 23 passed; 0 failed; 0 ignored; 0 measured; 61 filtered out; finished in 0.00s

$ ~/.cargo/bin/cargo test --lib
test result: ok. 84 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.44s

$ ~/.cargo/bin/cargo fmt -- --check
（无输出，格式已是最终状态）

$ git diff --check
（无输出，无空白/冲突标记问题）
```

83 → 84：净增一个测试（`unreadable_dir_...` 是新测试；`broken_disk_profile_...` 是在原测试上加断言，不是新增测试函数）。全测试套件无回归。

### 未改动

两条 Minor（`.filter_map(|e| e.ok())` 丢单个 `DirEntry` 失败；`use std::path::...` 插在文件中部）按复审意见不在本轮范围内，未动。

### 提交

```
c673212 fix: profile TOML 错误信息补上具体原因和行号
```

---

## 复审修复报告第二轮（Finding：`describe_toml_error` 未真正保证单行）

复审在 `c673212` 上确认前两个 Important 已解决，但发现修复本身引入了一个新的 Important 问题：
`describe_toml_error` 只剥掉了 `toml::de::Error` 自带的多行 ASCII 指位图（`TOML parse error at
line X...`），却假设 `err.message()` 本身一定是单行——不成立。

### 根因

`toml` 0.8.23 底层用的 winnow 0.7.15，`ContextError<StrContext>::Display`（`winnow-0.7.15/src/
error.rs:834-880`）在错误同时带有 `Label` 和 `Expected` 两种上下文时，会用内部的 `writeln!` 把两句
拼接成两行，这个拼接结果原样成为 `toml::de::Error::message()` 的返回值。用探针复现（本环境
toml 0.8.23）：

```
name = "C:\x"
command = ["echo"]
```

反斜杠后面接一个既不是 `b/f/n/r/t/u/U/\/"` 的字符（这里是 `x`）——非程序员写 Windows 路径时
极容易踩到——`err.message()` 拿到的是：

```
"invalid escape sequence\nexpected `b`, `f`, `n`, `r`, `t`, `u`, `U`, `\\`, `\"`"
```

`describe_toml_error` 原样把这句塞进 `第 {line} 行：{reason}`，`errs` 里就真出现了一个两行条目，
糊在状态栏上是错位的半份栈追踪，违反「不给用户看栈追踪」的全局约束。

**复审给的原始例子**是 `name = "C:\Users\x"`（同一处非法转义），我用探针实测发现第一个反斜杠后面
跟的是 `U`——那是合法转义的起始字符（8 位十六进制 Unicode 转义），会先在别的地方报错
（`invalid unicode 8-digit hex code`，单行）。把路径简化成 `name = "C:\x"`（去掉中间的
`Users`，让反斜杠直接跟上一个不合法的转义字符）复现出了复审贴的那句
`message() = "invalid escape sequence\nexpected ..."`，逐字符匹配。用这个更短的输入做测试，
场景没变（用户在字符串里写反斜杠、TOML 把它当转义符起始），只是命中路径更直接。

### 修法

在 `describe_toml_error` 里，从 `err.span()` 算行号之前，先把 `err.message()` 按行拆开、
丢掉空行、用 `；` 重新拼成一行：

```rust
let reason = err
    .message()
    .lines()
    .filter(|line| !line.trim().is_empty())
    .collect::<Vec<_>>()
    .join("；");
```

选的是**拼接**而不是只留第一行：`expected \`b\`, \`f\`, ...` 那半句是唯一具体告诉用户
「该写成什么样」的部分（第一句「invalid escape sequence」只说明「你写错了」，用户已经知道，
因为菜单上出现了这条错误）。只留第一行等于把最有用的信息删掉。选 `；` 而不是空格或逗号，是因为
两句本来是两件独立的事（「哪里错了」+「该写什么」），顿号式的中文标点比强行拼成一句读起来更
自然，也不会和 `expected` 后面本来就用逗号分隔的列表混在一起。

`.filter(|line| !line.trim().is_empty())` 是防御性的：如果某天上游的 message 变成三行、或者
中间/结尾带一个空行，`join` 不会因为空字符串元素产出多余的前导/尾随 `；`。

### 新增测试

`toml_error_with_embedded_newline_still_collapses_to_one_line`：写入
`name = "C:\x"\ncommand = ["echo"]\n`，断言 `load_dir` 返回的 `errs[0]`
- 不含 `'\n'`（核心断言，直接命中这次复审的问题）
- 包含 `"invalid escape sequence"`（第一句原因没丢）
- 包含 `"expected"`（「该怎么改」那半句没丢）

### 顺带处理的 Minor（可选项，做了）

复审里的可选 Minor：`unreadable_dir_reports_an_error_instead_of_going_silent` 只在断言全部通过的
happy path 上恢复 `0o700`，`assert!` 一旦在 if 块里 panic 就会直接展开出函数，跳过末尾那句
`set_permissions`。复审自己判断"大概率无害"（清空目录靠父目录的写权限，不靠这个目录自己的
mode），但既然改动很小就顺手修了：把权限恢复挪进一个 `RestorePerms` 的 `Drop` 实现里，
不管是正常返回还是 assert panic 展开，Drop 都会跑。选它而不是 `std::panic::catch_unwind`
之类更重的方案，是因为 RAII 是这个仓库里 `checkpoint` 之类代码已经在用的惯用法，不引入新概念。

### 验证

```
$ ~/.cargo/bin/cargo fmt
（无输出）

$ ~/.cargo/bin/cargo fmt -- --check
（无输出，已是最终格式）

$ ~/.cargo/bin/cargo test --lib profile
running 24 tests
test profile::tests::builtin_names_are_in_menu_order ... ok
test profile::tests::builtin_names_includes_claude_and_shell ... ok
test profile::tests::builtin_claude_uses_bypass_flag ... ok
test profile::tests::builtin_shell_is_not_agent ... ok
test profile::tests::api_shaped_profiles_run_claude_and_need_a_secret ... ok
test profile::tests::new_fields_all_default_to_empty ... ok
test profile::tests::bad_busy_pattern_is_an_error ... ok
test profile::tests::parses_busy_pattern_and_install ... ok
test profile::tests::busy_regex_compiles ... ok
test profile::tests::idle_regex_compiles ... ok
test profile::tests::codex_detects_busy_not_idle ... ok
test profile::tests::parses_env_and_secret ... ok
test profile::tests::parses_toml ... ok
test profile::tests::unknown_builtin_is_none ... ok
test profile::tests::profiles_dir_sits_next_to_socket ... ok
test profile::tests::unverified_profiles_have_no_pattern ... ok
test profile::tests::disk_profile_with_new_name_is_appended_after_builtins ... ok
test profile::tests::disk_profile_overrides_builtin_of_same_name ... ok
test profile::tests::missing_dir_is_not_an_error ... ok
test profile::tests::toml_error_with_embedded_newline_still_collapses_to_one_line ... ok
test profile::tests::unreadable_dir_reports_an_error_instead_of_going_silent ... ok
test profile::tests::every_builtin_parses_and_is_well_formed ... ok
test profile::tests::broken_disk_profile_reports_the_filename_and_keeps_the_rest ... ok
test profile::tests::non_toml_files_are_ignored ... ok

test result: ok. 24 passed; 0 failed; 0 ignored; 0 measured; 61 filtered out; finished in 0.01s

$ ~/.cargo/bin/cargo test --lib
test result: ok. 85 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.44s

$ git diff --check -- src/profile.rs
（无输出，无空白/冲突标记问题）
```

24 → 24 是 profile 模块内测试数（本轮新增 1 个，之前一轮也新增 1 个，counts 都在模块内）；
全仓 84 → 85，净增本轮新增的 1 个测试，无回归。

### 未改动

两条 Minor（`.filter_map(|e| e.ok())` 丢单个 `DirEntry` 失败；`use std::path::...` 插在文件中部）
仍按两轮复审的一致意见排除在范围外，未动。

### 提交

```
<待填：见下方 commit>
```
