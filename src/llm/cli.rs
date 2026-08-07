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
    let out = child
        .wait_with_output()
        .map_err(|e| format!("等待失败：{e}"))?;
    if !out.status.success() {
        return Err(format!(
            "{head} 退出码非零：{}",
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    String::from_utf8(out.stdout).map_err(|e| format!("输出不是 UTF-8：{e}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    fn p() -> Prompt {
        Prompt {
            system: "你是个助手".into(),
            user: "出了什么事？".into(),
            max_tokens: 64,
        }
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
