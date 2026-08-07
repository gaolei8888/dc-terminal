### Task 5: `CliBackend`——把 agent CLI 当模型用

**Files:**
- Create: `src/llm/cli.rs`
- Modify: `src/llm/mod.rs`（加 `pub mod cli;`）

**Interfaces:**
- Consumes: `llm::{Backend, Prompt, LlmError}`
- Produces:
  - `llm::cli::Runner`：类型别名 `dyn Fn(&[String], &str) -> Result<String, String> + Send + Sync`
  - `llm::cli::CliBackend { command: Vec<String>, env: BTreeMap<String, String>, runner: Arc<Runner> }`
  - `CliBackend::new(command: Vec<String>, env: BTreeMap<String, String>) -> CliBackend`（用真实子进程 runner）
  - `CliBackend::with_runner(command: Vec<String>, runner: Arc<Runner>) -> CliBackend`（测试用）

**说明：** 用户要的 SSO 在这条路上是**零代码**的——`claude -p` 自己就会去读它自己的登录态，dct 一个 token 都不用碰。

runner 注入是为了能不拉真子进程地测：真实 runner 单独一个函数，**不被单元测试覆盖**，在 Task 9 的实测里验。

提示词的送法：`system` 与 `user` 拼成一段文本，**从 stdin 送**，不作为命令行参数——参数会进 `ps` 输出、可能超长度上限，而且要处理引号转义。

- [ ] **Step 1: 写失败的测试**

`src/llm/cli.rs` 末尾：

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    fn p() -> Prompt {
        Prompt { system: "你是个助手".into(), user: "出了什么事？".into(), max_tokens: 64 }
    }

    #[test]
    fn the_stdout_of_the_cli_is_the_answer() {
        let b = CliBackend::with_runner(
            vec!["claude".into(), "-p".into()],
            Arc::new(|_cmd: &[String], _input: &str| Ok("  磁盘满了。\n".to_string())),
        );
        // 首尾空白要修掉：CLI 普遍带一个尾随换行，原样传下去会污染界面。
        assert_eq!(b.complete(&p()), Ok("磁盘满了。".to_string()));
    }

    #[test]
    fn the_prompt_reaches_the_cli_on_stdin() {
        let seen = Arc::new(Mutex::new((Vec::new(), String::new())));
        let sink = seen.clone();
        let b = CliBackend::with_runner(
            vec!["claude".into(), "-p".into()],
            Arc::new(move |cmd: &[String], input: &str| {
                *sink.lock().unwrap() = (cmd.to_vec(), input.to_string());
                Ok("ok".into())
            }),
        );
        b.complete(&p()).unwrap();
        let (cmd, input) = seen.lock().unwrap().clone();
        assert_eq!(cmd, vec!["claude".to_string(), "-p".to_string()]);
        assert!(input.contains("你是个助手"), "system 没送到");
        assert!(input.contains("出了什么事？"), "user 没送到");
    }

    #[test]
    fn a_failing_cli_is_unavailable_not_a_crash() {
        let b = CliBackend::with_runner(
            vec!["nope".into()],
            Arc::new(|_: &[String], _: &str| Err("command not found".into())),
        );
        assert_eq!(b.complete(&p()), Err(LlmError::Unavailable));
    }

    #[test]
    fn empty_output_is_malformed_not_an_empty_answer() {
        // 空回答比没回答更糟：界面会显示一片空白，用户以为功能坏了。
        // 当成 Malformed，让调用方走退路。
        let b = CliBackend::with_runner(
            vec!["claude".into()],
            Arc::new(|_: &[String], _: &str| Ok("   \n  ".into())),
        );
        assert_eq!(b.complete(&p()), Err(LlmError::Malformed));
    }
}
```

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test --lib llm::cli`
Expected: 编译失败，`unresolved module 'cli'`

- [ ] **Step 3: 写实现**

`src/llm/cli.rs`：

```rust
//! 把一个 agent CLI 的无界面模式当成模型来用。
//!
//! **用户要的 SSO 在这条路上是零代码的**：`claude -p` 自己会去读它自己的
//! 登录态，dct 一个 token 都不碰，也就没有任何厂商格式可以变坏。

use super::{Backend, LlmError, Prompt};
use std::collections::BTreeMap;
use std::io::Write;
use std::sync::Arc;

/// 命令 + stdin → stdout。注入是为了能不拉真子进程地测。
pub type Runner = dyn Fn(&[String], &str) -> Result<String, String> + Send + Sync;

pub struct CliBackend {
    command: Vec<String>,
    runner: Arc<Runner>,
}

impl CliBackend {
    /// `env` 不进结构体——它只被真实 runner 用到，闭包捕获走就够了。
    /// 存一份在字段里没有任何读者，是纯粹的死重量。
    pub fn new(command: Vec<String>, env: BTreeMap<String, String>) -> CliBackend {
        CliBackend {
            command,
            runner: Arc::new(move |cmd, input| run_real(cmd, input, &env)),
        }
    }

    pub fn with_runner(command: Vec<String>, runner: Arc<Runner>) -> CliBackend {
        CliBackend { command, runner }
    }
}

impl Backend for CliBackend {
    fn complete(&self, p: &Prompt) -> Result<String, LlmError> {
        let input = format!("{}\n\n{}", p.system, p.user);
        let out = (self.runner)(&self.command, &input).map_err(|e| {
            eprintln!("LLM CLI 调用失败：{e}");
            LlmError::Unavailable
        })?;
        let trimmed = out.trim();
        if trimmed.is_empty() {
            // 空回答比没回答更糟：界面会显示一片空白，用户以为功能坏了。
            return Err(LlmError::Malformed);
        }
        Ok(trimmed.to_string())
    }
}

/// 真实子进程。**没有单元测试覆盖**（会拉起真 CLI），在实测那一步验。
///
/// 提示词走 stdin 不走参数：参数会进 `ps` 输出、可能超长度上限，
/// 还要处理引号转义。
fn run_real(cmd: &[String], input: &str, env: &BTreeMap<String, String>) -> Result<String, String> {
    let (head, rest) = cmd.split_first().ok_or_else(|| "空命令".to_string())?;
    let mut child = std::process::Command::new(head)
        .args(rest)
        .envs(env)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| format!("{head} 起不来：{e}"))?;
    child
        .stdin
        .take()
        .ok_or_else(|| "拿不到 stdin".to_string())?
        .write_all(input.as_bytes())
        .map_err(|e| format!("写 stdin 失败：{e}"))?;
    let out = child.wait_with_output().map_err(|e| format!("等待失败：{e}"))?;
    if !out.status.success() {
        return Err(format!(
            "{head} 退出码非零：{}",
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    String::from_utf8(out.stdout).map_err(|e| format!("输出不是 UTF-8：{e}"))
}
```

`src/llm/mod.rs` 的 `pub mod creds;` 下面加 `pub mod cli;`。

- [ ] **Step 4: 跑测试确认通过**

Run: `cargo test --lib llm::cli`
Expected: 4 passed

- [ ] **Step 5: 提交**

```bash
cargo fmt && cargo test --lib && git add src/llm/cli.rs src/llm/mod.rs
git commit -m "feat(llm): run an agent CLI headlessly as a model backend

This is the path where the user's SSO works with zero code: claude -p reads
its own login, so dct never handles a token and no vendor format can rot.

The prompt goes over stdin rather than argv — arguments show up in ps output,
can exceed length limits, and would need quote escaping."
```

---

