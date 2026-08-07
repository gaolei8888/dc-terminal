### Task 2: profile 的 `[headless]` 与 `[api]` 块

**Files:**
- Modify: `src/profile.rs`（`Profile` 结构 + 新增两个 spec 结构）
- Modify: `profiles/claude.toml`, `profiles/codex.toml`, `profiles/kimi.toml`, `profiles/glm.toml`, `profiles/deepseek.toml`, `profiles/qwen-api.toml`

**Interfaces:**
- Consumes: 无
- Produces:
  - `profile::HeadlessSpec { pub command: Vec<String> }`
  - `profile::Wire { Openai, Anthropic }`（`Deserialize`，rename 小写）
  - `profile::ApiSpec { pub base_url: String, pub wire: Wire }`
  - `Profile` 新增两个字段：`pub headless: Option<HeadlessSpec>`、`pub api: Option<ApiSpec>`

**说明：** 这是「登记表长在 profile 上，不另起平行清单」那条决定的落地。**只给实测过无界面模式的 profile 写 `[headless]`**——`opencode` 和 `qwen` 本机没装、没验过，留空，与既有的「没验过就不填 pattern」是同一条纪律。

`[api]` 的 `base_url` 与 profile 里已有的 `[env] ANTHROPIC_BASE_URL` 是同一个值，但**不能合并**：`env` 是给子进程用的，`api` 是给 dct 自己用的，两者将来会分叉（比如 dc_llm 只有 `[api]` 没有 `[env]`）。

- [ ] **Step 1: 写失败的测试**

在 `src/profile.rs` 的 `mod tests` 里追加：

```rust
#[test]
fn claude_and_codex_declare_a_headless_command() {
    // 这两个是本机实测过的：`claude -p` 和 `codex exec`。
    for name in ["claude", "codex"] {
        let p = Profile::builtin(name).unwrap();
        let h = p.headless.as_ref().unwrap_or_else(|| panic!("{name}: 要有 [headless]"));
        assert!(!h.command.is_empty(), "{name}: headless 命令不能为空");
    }
    assert_eq!(
        Profile::builtin("claude").unwrap().headless.unwrap().command,
        vec!["claude".to_string(), "-p".to_string()]
    );
    assert_eq!(
        Profile::builtin("codex").unwrap().headless.unwrap().command,
        vec!["codex".to_string(), "exec".to_string()]
    );
}

#[test]
fn unverified_clis_declare_no_headless_command() {
    // opencode / qwen 本机没装，无界面模式没验过。编一个出来 = 造一条
    // 用户按了就报错的路，和「没验过就不填 pattern」是同一条纪律。
    for name in ["opencode", "qwen"] {
        let p = Profile::builtin(name).unwrap();
        assert!(p.headless.is_none(), "{name}: 没实测过就别填 [headless]");
    }
}

#[test]
fn api_shaped_profiles_declare_an_api_block() {
    for name in ["kimi", "glm", "deepseek", "qwen-api"] {
        let p = Profile::builtin(name).unwrap();
        let api = p.api.as_ref().unwrap_or_else(|| panic!("{name}: 要有 [api]"));
        assert!(api.base_url.starts_with("https://"), "{name}: base_url 要是 https");
        assert_eq!(api.wire, Wire::Anthropic, "{name}: 这四个都是 Anthropic 兼容形态");
    }
}

#[test]
fn the_api_base_url_matches_the_env_base_url() {
    // 两个字段现在值相同但用途不同（env 给子进程，api 给 dct 自己）。
    // 不合并，但要一致——不一致意味着有人只改了一边。
    for name in ["kimi", "glm", "deepseek", "qwen-api"] {
        let p = Profile::builtin(name).unwrap();
        let env = p.env.get("ANTHROPIC_BASE_URL").unwrap();
        assert_eq!(&p.api.as_ref().unwrap().base_url, env, "{name}: 两处 base_url 不一致");
    }
}

#[test]
fn a_profile_without_the_new_blocks_still_parses() {
    // 用户手写的老 profile 不能因为加了新字段就读不了。
    let p = Profile::from_toml("name = \"x\"\ncommand = [\"x\"]\n").unwrap();
    assert!(p.headless.is_none());
    assert!(p.api.is_none());
}
```

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test --lib profile::`
Expected: 编译失败，`no field 'headless' on type 'Profile'`

- [ ] **Step 3: 写实现**

`src/profile.rs`，在 `InstallSpec` 定义之后插入：

```rust
/// 怎么把这个 profile 用**无界面**方式跑一次（dct 自己要用模型时走这条）。
///
/// 命令后面会追加提示词，stdout 就是回答。
/// **只给实测过的 profile 写**——编一个出来等于造一条用户按了就报错的路。
#[derive(Debug, Clone, Deserialize)]
pub struct HeadlessSpec {
    pub command: Vec<String>,
}

/// HTTP 端点说的是哪种话。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Wire {
    Openai,
    Anthropic,
}

/// 这个 profile 背后的 HTTP 端点，给 dct 自己直连用。
///
/// 和 `[env] ANTHROPIC_BASE_URL` 值相同但**不合并**：那个是给子进程的，
/// 这个是给 dct 的，将来会分叉（dc_llm 只有 `[api]`，没有 `[env]`）。
#[derive(Debug, Clone, Deserialize)]
pub struct ApiSpec {
    pub base_url: String,
    pub wire: Wire,
}
```

`Profile` 结构体在 `pub install: Option<InstallSpec>,` 之后加：

```rust
    #[serde(default)]
    pub headless: Option<HeadlessSpec>,
    #[serde(default)]
    pub api: Option<ApiSpec>,
```

`profiles/claude.toml` 末尾追加：

```toml
[headless]
# 实测：`claude -p/--print` 是官方的非交互模式，提示词作为参数追加。
command = ["claude", "-p"]
```

`profiles/codex.toml` 末尾追加：

```toml
[headless]
# 实测 codex v0.146.0：`codex exec` 是官方的非交互模式。
command = ["codex", "exec"]
```

`profiles/kimi.toml` 末尾追加：

```toml
[headless]
command = ["claude", "-p"]

[api]
base_url = "https://api.moonshot.cn/anthropic"
wire = "anthropic"
```

`profiles/glm.toml` 末尾追加：

```toml
[headless]
command = ["claude", "-p"]

[api]
base_url = "https://open.bigmodel.cn/api/anthropic"
wire = "anthropic"
```

`profiles/deepseek.toml` 与 `profiles/qwen-api.toml` 同样追加 `[headless]`（`["claude", "-p"]`）与 `[api]`，`base_url` **必须逐字照抄该文件 `[env] ANTHROPIC_BASE_URL` 的值**，`wire = "anthropic"`。

- [ ] **Step 4: 跑测试确认通过**

Run: `cargo test --lib profile::`
Expected: 全绿，含新增 5 个

- [ ] **Step 5: 提交**

```bash
cargo fmt && cargo test --lib && git add src/profile.rs profiles/
git commit -m "feat(profile): declare headless commands and API endpoints on profiles

The provider registry lives on profiles/*.toml rather than beside it, so
there is never a second vendor list to keep in sync.

Only claude and codex get a [headless] block; opencode and qwen have never
been run non-interactively here, and inventing a command would create a path
that errors the moment a user takes it."
```

---

