## Task 2: 七个新的内置 profile

**Files:**
- Create: `profiles/codex.toml`、`profiles/opencode.toml`、`profiles/qwen.toml`、`profiles/kimi.toml`、`profiles/glm.toml`、`profiles/deepseek.toml`、`profiles/qwen-api.toml`
- Modify: `profiles/claude.toml`、`profiles/shell.toml`（补 label / note）
- Modify: `src/profile.rs` 的 `builtin()` 与 `builtin_names()`
- Test: `src/profile.rs` 的 `mod tests`

**Interfaces:**
- Consumes: Task 1 的 `Profile`、`Lang`
- Produces: `Profile::builtin(name) -> Option<Profile>`（已有，扩容到 9 条）、`Profile::builtin_names() -> Vec<&'static str>`（已有，返回顺序即菜单顺序）、`Profile::builtins() -> Vec<Profile>`（新）

⚠️ **本任务的 base_url、命令行参数、安装包名，除 codex 外都未经实测。** 见设计文档的「未实测项」表。实施时**必须逐条实跑验证**，与本文档不符的以实跑为准，并回头更新设计文档那张表。

- [ ] **Step 1: 写失败的测试**

```rust
#[test]
fn every_builtin_parses_and_is_well_formed() {
    for name in Profile::builtin_names() {
        let p = Profile::builtin(name)
            .unwrap_or_else(|| panic!("{name} 应当是内置 profile"));
        assert_eq!(p.name, name, "{name}: 文件里的 name 必须和清单一致");
        assert!(!p.command.is_empty(), "{name}: command 不能为空");
        assert!(
            p.label.get(Lang::Zh).is_some(),
            "{name}: 必须有中文 label，九个选项摆在非程序员面前没说明等于没得选"
        );
        // 正则必须能编译，否则一到 tick 就报错
        p.idle_regex().unwrap();
        p.busy_regex().unwrap();
    }
}

#[test]
fn builtin_names_are_in_menu_order() {
    assert_eq!(
        Profile::builtin_names(),
        vec![
            "claude", "codex", "opencode", "qwen",
            "kimi", "glm", "deepseek", "qwen-api",
            "shell",
        ]
    );
}

#[test]
fn api_shaped_profiles_run_claude_and_need_a_secret() {
    for name in ["kimi", "glm", "deepseek", "qwen-api"] {
        let p = Profile::builtin(name).unwrap();
        assert_eq!(p.command[0], "claude", "{name}: API 形态跑的是 claude");
        assert!(p.env.contains_key("ANTHROPIC_BASE_URL"), "{name}: 要换 base_url");
        let s = p.secret.as_ref().unwrap_or_else(|| panic!("{name}: 要声明密钥"));
        assert_eq!(s.env, "ANTHROPIC_AUTH_TOKEN");
        assert!(s.verify.is_some(), "{name}: 要能验证密钥");
    }
}

#[test]
fn codex_detects_busy_not_idle() {
    // codex 空闲时屏幕上没有稳定的固定串，干活时一定有 esc to interrupt。
    // 实测自 codex v0.146.0。
    let p = Profile::builtin("codex").unwrap();
    assert!(p.busy_pattern.is_some());
    assert!(p.idle_pattern.is_none());
    assert!(p.busy_regex().unwrap().unwrap().is_match("(12s • esc to interrupt)"));
}

#[test]
fn unverified_profiles_have_no_pattern() {
    // opencode / qwen 的 TUI 没实测过。宁可状态显示「—」，不能瞎猜一个 pattern
    // 然后在看板上编状态。
    for name in ["opencode", "qwen"] {
        let p = Profile::builtin(name).unwrap();
        assert!(p.idle_pattern.is_none() && p.busy_pattern.is_none(),
                "{name}: 没实测就别填 pattern");
    }
}
```

- [ ] **Step 2: 跑测试确认它失败**

Run: `~/.cargo/bin/cargo test --lib profile`
Expected: FAIL，`codex 应当是内置 profile`

- [ ] **Step 3: 写九个 profile 文件**

`profiles/claude.toml`（改）：

```toml
name = "claude"
command = ["claude", "--dangerously-skip-permissions"]
is_agent = true
idle_pattern = "\\? for shortcuts"

[label]
zh = "Claude"

[note]
zh = "Anthropic 官方"
```

`profiles/codex.toml`：

```toml
name = "codex"
command = ["codex", "--dangerously-bypass-approvals-and-sandbox"]
is_agent = true
# 实测自 codex v0.146.0：干活时底部一定有「(12s • esc to interrupt)」。
# 空闲时只有输入框占位符，用户一打字就没了，不能拿来当 idle_pattern。
busy_pattern = "esc to interrupt"

[label]
zh = "Codex"

[note]
zh = "OpenAI 官方"

[install]
command = ["npm", "i", "-g", "@openai/codex"]

[install.note]
zh = "需要先装 Node.js"
```

`profiles/opencode.toml`：

```toml
name = "opencode"
command = ["opencode"]
is_agent = true
# TUI 没实测过，不填 pattern —— 状态显示「—」比编一个假状态好

[label]
zh = "OpenCode"

[note]
zh = "开源，可接多种模型"

[install]
command = ["npm", "i", "-g", "opencode-ai"]

[install.note]
zh = "需要先装 Node.js"
```

`profiles/qwen.toml`：

```toml
name = "qwen"
command = ["qwen"]
is_agent = true
# 同 opencode，未实测

[label]
zh = "Qwen Code"

[note]
zh = "阿里通义，独立命令行"

[install]
command = ["npm", "i", "-g", "@qwen-code/qwen-code"]

[install.note]
zh = "需要先装 Node.js"
```

`profiles/kimi.toml`：

```toml
name = "kimi"
command = ["claude", "--dangerously-skip-permissions"]
is_agent = true
idle_pattern = "\\? for shortcuts"

[label]
zh = "Kimi"

[note]
zh = "月之暗面，套用 Claude 界面"

[env]
ANTHROPIC_BASE_URL = "https://api.moonshot.cn/anthropic"

[secret]
env = "ANTHROPIC_AUTH_TOKEN"
url = "https://platform.moonshot.cn/console/api-keys"

[secret.hint]
zh = "在 platform.moonshot.cn 开通后复制 API Key"

[secret.verify]
url = "https://api.moonshot.cn/anthropic/v1/messages"
```

`profiles/glm.toml`：

```toml
name = "glm"
command = ["claude", "--dangerously-skip-permissions"]
is_agent = true
idle_pattern = "\\? for shortcuts"

[label]
zh = "GLM"

[note]
zh = "智谱，套用 Claude 界面"

[env]
ANTHROPIC_BASE_URL = "https://open.bigmodel.cn/api/anthropic"

[secret]
env = "ANTHROPIC_AUTH_TOKEN"
url = "https://open.bigmodel.cn/usercenter/apikeys"

[secret.hint]
zh = "在 open.bigmodel.cn 的用户中心复制 API Key"

[secret.verify]
url = "https://open.bigmodel.cn/api/anthropic/v1/messages"
```

`profiles/deepseek.toml`：

```toml
name = "deepseek"
command = ["claude", "--dangerously-skip-permissions"]
is_agent = true
idle_pattern = "\\? for shortcuts"

[label]
zh = "DeepSeek"

[note]
zh = "深度求索，套用 Claude 界面"

[env]
ANTHROPIC_BASE_URL = "https://api.deepseek.com/anthropic"

[secret]
env = "ANTHROPIC_AUTH_TOKEN"
url = "https://platform.deepseek.com/api_keys"

[secret.hint]
zh = "在 platform.deepseek.com 开通后复制 API Key"

[secret.verify]
url = "https://api.deepseek.com/anthropic/v1/messages"
```

`profiles/qwen-api.toml`：

```toml
name = "qwen-api"
command = ["claude", "--dangerously-skip-permissions"]
is_agent = true
idle_pattern = "\\? for shortcuts"

[label]
zh = "Qwen API"

[note]
zh = "阿里通义，套用 Claude 界面"

[env]
ANTHROPIC_BASE_URL = "https://dashscope.aliyuncs.com/api/v2/apps/claude-code-proxy"

[secret]
env = "ANTHROPIC_AUTH_TOKEN"
url = "https://bailian.console.aliyun.com/?tab=model#/api-key"

[secret.hint]
zh = "在阿里云百炼控制台创建 API Key"

[secret.verify]
url = "https://dashscope.aliyuncs.com/api/v2/apps/claude-code-proxy/v1/messages"
```

`profiles/shell.toml`（改）：

```toml
name = "shell"
command = ["/bin/zsh"]
is_agent = false

[label]
zh = "命令行"

[note]
zh = "普通终端，不带 AI"
```

- [ ] **Step 4: 接上 builtin()**

替换 `src/profile.rs` 里的 `CLAUDE` / `SHELL` 常量与两个方法：

```rust
const CLAUDE: &str = include_str!("../profiles/claude.toml");
const CODEX: &str = include_str!("../profiles/codex.toml");
const OPENCODE: &str = include_str!("../profiles/opencode.toml");
const QWEN: &str = include_str!("../profiles/qwen.toml");
const KIMI: &str = include_str!("../profiles/kimi.toml");
const GLM: &str = include_str!("../profiles/glm.toml");
const DEEPSEEK: &str = include_str!("../profiles/deepseek.toml");
const QWEN_API: &str = include_str!("../profiles/qwen-api.toml");
const SHELL: &str = include_str!("../profiles/shell.toml");
```

```rust
    pub fn builtin(name: &str) -> Option<Profile> {
        let src = match name {
            "claude" => CLAUDE,
            "codex" => CODEX,
            "opencode" => OPENCODE,
            "qwen" => QWEN,
            "kimi" => KIMI,
            "glm" => GLM,
            "deepseek" => DEEPSEEK,
            "qwen-api" => QWEN_API,
            "shell" => SHELL,
            _ => return None,
        };
        Some(Profile::from_toml(src).expect("内置 profile 必须能解析"))
    }

    /// 返回顺序就是菜单顺序：先独立 CLI，再 API 形态，命令行垫底。
    /// 命令行放最后是因为它对目标用户价值最低——非程序员不需要裸终端。
    pub fn builtin_names() -> Vec<&'static str> {
        vec![
            "claude", "codex", "opencode", "qwen",
            "kimi", "glm", "deepseek", "qwen-api",
            "shell",
        ]
    }

    pub fn builtins() -> Vec<Profile> {
        Profile::builtin_names()
            .into_iter()
            .filter_map(Profile::builtin)
            .collect()
    }
```

原有的 `builtin_names_lists_both` 测试改名成 `builtin_names_includes_claude_and_shell`，断言改成 `contains`（`builtin_names_are_in_menu_order` 已经覆盖完整顺序）。

- [ ] **Step 5: 跑测试确认通过**

Run: `~/.cargo/bin/cargo test --lib profile`
Expected: PASS

- [ ] **Step 6: 实跑验证未实测项**

这一步是人工的，不能跳。逐条确认，与本文档不符就以实跑为准，同时更新设计文档的「未实测项」表：

```bash
# 独立 CLI：命令能不能起、TUI 干活时屏幕上有什么固定串
which opencode qwen
# 装了的话，各起一次，观察空闲屏和干活屏，补上 idle_pattern 或 busy_pattern

# 四个 base_url：拿一份真 key 探一下，401/403 以外都算通
curl -s -o /dev/null -w '%{http_code}\n' \
  -X POST 'https://api.moonshot.cn/anthropic/v1/messages' \
  -H 'content-type: application/json' -H "x-api-key: $KEY" \
  -d '{"model":"moonshot-v1-8k","max_tokens":1,"messages":[{"role":"user","content":"hi"}]}'
```

- [ ] **Step 7: 提交**

```bash
~/.cargo/bin/cargo fmt
git add profiles src/profile.rs docs/superpowers/specs/2026-08-03-dct-multi-agent-design.md
git commit -m "feat: 内置 codex/opencode/qwen 与 kimi/glm/deepseek/qwen-api

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>"
```

---

