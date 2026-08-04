# Task 2 报告：七个新的内置 profile

## 实现内容

- 新增 `profiles/codex.toml`、`profiles/opencode.toml`、`profiles/qwen.toml`、
  `profiles/kimi.toml`、`profiles/glm.toml`、`profiles/deepseek.toml`、
  `profiles/qwen-api.toml`，内容与任务简报逐字一致。
- 修改 `profiles/claude.toml`、`profiles/shell.toml`，补上 `[label]` / `[note]`。
- 修改 `src/profile.rs`：
  - 九个 `include_str!` 常量（`CLAUDE` / `CODEX` / `OPENCODE` / `QWEN` / `KIMI` /
    `GLM` / `DEEPSEEK` / `QWEN_API` / `SHELL`）。
  - `builtin()` 九路匹配。
  - `builtin_names()` 返回九个名字，顺序即菜单顺序（独立 CLI 在前，API 形态居中，
    命令行垫底）。
  - 新增 `builtins() -> Vec<Profile>`，基于 `builtin_names()` 过滤映射。
  - 原测试 `builtin_names_lists_both` 改名为
    `builtin_names_includes_claude_and_shell`，断言维持 `contains`（完整顺序由
    新测试 `builtin_names_are_in_menu_order` 覆盖）。
  - 新增五个测试：`every_builtin_parses_and_is_well_formed`、
    `builtin_names_are_in_menu_order`、
    `api_shaped_profiles_run_claude_and_need_a_secret`、
    `codex_detects_busy_not_idle`、`unverified_profiles_have_no_pattern`。

所有 TOML 文件内容与 `task-2-brief.md` 中给出的文本逐字一致，没有做任何修改或“顺手改进”。

## 测试

### TDD 证据

**RED** —— 先把 Step 1 的五个新测试和改名后的旧测试加进 `src/profile.rs`（此时
`builtin()`/`builtin_names()` 还只认识 `claude`/`shell`），跑：

```
~/.cargo/bin/cargo test --lib profile
```

输出（节选）：

```
test profile::tests::codex_detects_busy_not_idle ... FAILED
test profile::tests::builtin_names_are_in_menu_order ... FAILED
test profile::tests::api_shaped_profiles_run_claude_and_need_a_secret ... FAILED
test profile::tests::unverified_profiles_have_no_pattern ... FAILED
test profile::tests::every_builtin_parses_and_is_well_formed ... FAILED

---- profile::tests::builtin_names_are_in_menu_order stdout ----
thread 'profile::tests::builtin_names_are_in_menu_order' panicked at src/profile.rs:327:9:
assertion `left == right` failed
  left: ["claude", "shell"]
 right: ["claude", "codex", "opencode", "qwen", "kimi", "glm", "deepseek", "qwen-api", "shell"]

---- profile::tests::every_builtin_parses_and_is_well_formed stdout ----
thread 'profile::tests::every_builtin_parses_and_is_well_formed' panicked at src/profile.rs:315:13:
claude: 必须有中文 label，九个选项摆在非程序员面前没说明等于没得选

test result: FAILED. 11 passed; 5 failed; 0 ignored; 0 measured; 61 filtered out
```

失败符合预期：`builtin_names()` 还只有两个名字，`claude.toml` 当时还没补
`[label]`，其余四个测试因为 `Profile::builtin("codex"/"kimi"/...)` 返回 `None` 而
`.unwrap()` panic。

**GREEN** —— 写完九个 TOML 文件并接上 `builtin()`/`builtin_names()`/`builtins()`
之后：

```
~/.cargo/bin/cargo fmt && ~/.cargo/bin/cargo test --lib profile
```

```
running 16 tests
test profile::tests::builtin_names_includes_claude_and_shell ... ok
test profile::tests::builtin_names_are_in_menu_order ... ok
test profile::tests::builtin_shell_is_not_agent ... ok
test profile::tests::new_fields_all_default_to_empty ... ok
test profile::tests::parses_busy_pattern_and_install ... ok
test profile::tests::builtin_claude_uses_bypass_flag ... ok
test profile::tests::parses_toml ... ok
test profile::tests::unknown_builtin_is_none ... ok
test profile::tests::parses_env_and_secret ... ok
test profile::tests::unverified_profiles_have_no_pattern ... ok
test profile::tests::bad_busy_pattern_is_an_error ... ok
test profile::tests::api_shaped_profiles_run_claude_and_need_a_secret ... ok
test profile::tests::busy_regex_compiles ... ok
test profile::tests::idle_regex_compiles ... ok
test profile::tests::codex_detects_busy_not_idle ... ok
test profile::tests::every_builtin_parses_and_is_well_formed ... ok

test result: ok. 16 passed; 0 failed; 0 ignored; 0 measured; 61 filtered out
```

### 全量测试

```
env GOCACHE=/tmp/dcwb-go-cache ~/.cargo/bin/cargo test
```

lib 测试全部通过：`test result: ok. 77 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out`。
所有集成测试文件（`concurrency`、`daemon_detach`、`daemon_roundtrip`、
`projects_flow`、`signal_restore`、`slow_input`、`socket_perms`）以及 doc-tests
全部 `ok`，无失败无告警。

`git diff --check profiles/ src/profile.rs` 无输出，无空白错误。

## 实跑验证结果

- **`which opencode qwen codex claude`**：
  - `opencode`：未安装（`command -v` 退出码 1）。
  - `qwen`：未安装（`command -v` 退出码 1）。
  - `codex`：已安装，`/Users/lei/.nvm/versions/node/v22.17.0/bin/codex`，
    `codex --version` → `codex-cli 0.146.0`，**与简报注释里声明的 v0.146.0 完全一致**，
    佐证 `busy_pattern = "esc to interrupt"` 这条已实测结论可信。
  - `claude`：已安装，`/Users/lei/.local/bin/claude`，`claude --version` →
    `2.1.221 (Claude Code)`。
- **opencode / qwen 的 TUI 观察**：**做不到**——两者都未安装在这台机器上，没有
  npm/其他包管理器权限或时间预算去安装并验证。按 Step 6 指示，没有实测就不填
  `idle_pattern`/`busy_pattern`，两个文件保持简报原样（空 pattern），
  由 `unverified_profiles_have_no_pattern` 测试强制这一点。**这是留给人工的
  后续验证项**，不是本任务遗漏。
- **四个 base_url 的 curl 探测**（kimi/glm/deepseek/qwen-api）：按指示**未执行**——
  需要四家真实 API Key，环境里没有，且不应该对四个厂商端点做投机性请求。
  **未验证，需要人工**。

## 变更文件

- `profiles/claude.toml`（改，补 label/note）
- `profiles/shell.toml`（改，补 label/note）
- `profiles/codex.toml`（新增）
- `profiles/opencode.toml`（新增）
- `profiles/qwen.toml`（新增）
- `profiles/kimi.toml`（新增）
- `profiles/glm.toml`（新增）
- `profiles/deepseek.toml`（新增）
- `profiles/qwen-api.toml`（新增）
- `src/profile.rs`（改：九路 `builtin()`、`builtin_names()`、新增 `builtins()`、
  测试改名 + 五个新测试）

未改动 `docs/superpowers/specs/2026-08-03-dct-multi-agent-design.md`（按指示由
调用方根据本报告手工更新「未实测项」表）。

## 自查结果

- 九个 TOML 文件内容逐字核对过与简报一致，没有引入未声明的字段或改写措辞。
- 五个新测试 + 一个改名测试全部到位；`builtin_names_includes_claude_and_shell`
  的断言保持 `contains`，没有被误强化成 `assert_eq`。
- TOML 文件里的注释（codex 的 busy_pattern 说明、opencode/qwen 的“未实测”说明）
  照抄简报，说明的是“为什么这么写/为什么留空”，符合本仓库高密度、讲道理由的注释
  习惯。
- 没有在 `builtin()`/`builtin_names()` 之外新增函数或重构既有结构；`builtins()`
  是简报要求的产物，实现为最简单的 `filter_map`。
- 没有去猜 opencode/qwen 的 pattern，也没有跑 curl 探测——两者都按指示明确报告
  为「未验证，需要人工」。
- `src/profile.rs` 没有超出简报意图变大：只新增了 7 个 `include_str!` 常量、
  `builtin()` 的 7 个新分支、`builtin_names()` 扩容、`builtins()` 一个新函数、
  测试模块里 5 个新测试 + 1 个改名测试。

## 疑虑

无阻塞性疑虑。唯一需要人工跟进的两项已在「实跑验证结果」里列出：
1. 安装 opencode / qwen 后补测 TUI 空闲/干活屏幕的固定串。
2. 用真实 API Key 探测 kimi/glm/deepseek/qwen-api 四个 base_url。
