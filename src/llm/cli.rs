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
    /// 只在带图时才追加的参数（比如 claude 的 `--allowedTools Read`，让它能读图片文件）。
    image_args: Vec<String>,
    runner: Arc<Runner>,
}

impl CliBackend {
    /// `env` 不进结构体——它只被真实 runner 用到，闭包捕获走就够了。
    /// 存一份在字段里没有任何读者，是纯粹的死重量。
    pub fn new(command: Vec<String>, env: BTreeMap<String, String>) -> CliBackend {
        CliBackend {
            command,
            image_args: vec![],
            runner: Arc::new(move |cmd, input| run_real(cmd, input, &env)),
        }
    }

    pub fn with_runner(command: Vec<String>, runner: Arc<Runner>) -> CliBackend {
        CliBackend { command, image_args: vec![], runner }
    }

    pub fn with_image_args(mut self, args: Vec<String>) -> CliBackend {
        self.image_args = args;
        self
    }
}

/// 0700 的临时目录，丢掉时连内容一起删。
struct TempDir(std::path::PathBuf);
impl TempDir {
    fn new() -> Option<TempDir> {
        let n = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
        let p = std::env::temp_dir().join(format!("dct-cli-img-{}-{n}", std::process::id()));
        let mut b = std::fs::DirBuilder::new();
        #[cfg(unix)]
        std::os::unix::fs::DirBuilderExt::mode(&mut b, 0o700);
        b.create(&p).ok()?;
        Some(TempDir(p))
    }
}
impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

impl Backend for CliBackend {
    fn complete(&self, p: &Prompt) -> Result<String, LlmError> {
        // 带图：写进 0700 临时目录，提示词里让模型去读那个文件，命令里放开 Read；
        // 不带图时命令和提示词跟以前一字不差。`_dir` 活到调用结束，丢掉时删目录。
        let (_dir, user, command);
        let (user, command): (&str, &[String]) = match &p.image_png_base64 {
            None => (&p.user, &self.command),
            Some(b64) => {
                use base64::Engine;
                let bytes = base64::engine::general_purpose::STANDARD.decode(b64).map_err(|_| LlmError::Malformed)?;
                let d = TempDir::new().ok_or(LlmError::Unavailable)?;
                let ext = if p.image_mime.as_deref() == Some("image/jpeg") { "jpg" } else { "png" };
                let file = d.0.join(format!("image.{ext}"));
                std::fs::write(&file, bytes).map_err(|_| LlmError::Unavailable)?;
                user = format!("{}\n\nRead the image file at {} (use the Read tool) and answer about that image.", p.user, file.display());
                command = [self.command.clone(), self.image_args.clone()].concat();
                _dir = d;
                (&user, &command)
            }
        };
        let input = format!("{}\n\n{}", p.system, user);
        let out = (self.runner)(command, &input).map_err(|e| {
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

/// 真实子进程。**跟具体 agent CLI（`claude` 之类）的集成没有单元测试覆盖**，
/// 那部分在实测那一步验；但收发管道本身的正确性（不跟真 CLI 绑定）有一条
/// 用 `cat` 做的回归测试，见下面 `run_real_does_not_deadlock_...`。
///
/// 提示词走 stdin 不走参数：参数会进 `ps` 输出、可能超长度上限，
/// 还要处理引号转义。
fn run_real(cmd: &[String], input: &str, env: &BTreeMap<String, String>) -> Result<String, String> {
    // 跟 pty 那条路同一次翻译：Windows 上 `claude` 是 `claude.cmd`，而
    // `.cmd` 不是可执行映像，得由 cmd.exe 解释（见 `sys::shell::launch_argv`）。
    // 少了这一句，会话自动起名这类走 CLI 的功能在 Windows 上永远静默失败。
    let cmd = crate::sys::shell::launch_argv(cmd);
    let (head, rest) = cmd.split_first().ok_or_else(|| "空命令".to_string())?;
    let mut c = std::process::Command::new(head);
    // 这条路也是守护进程走的（自动起名之类），同样不能弹窗口——
    // 理由见 `sys::proc::no_console`。
    crate::sys::proc::no_console(&mut c);
    let mut child = c
        .args(rest)
        .envs(env)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| format!("{head} 起不来：{e}"))?;

    // 写 stdin 得放到单独的线程上，跟 wait_with_output 读 stdout/stderr
    // 并发进行：如果提示词超过管道缓冲区（macOS 16KB / Linux 64KB），而
    // 子进程这时候正往 stdout 写东西没人读，父进程堵在 write_all、子进程
    // 堵在写 stdout，就是经典的双向管道死锁。两条管道得同时有人伺候。
    let mut stdin = child
        .stdin
        .take()
        .ok_or_else(|| "拿不到 stdin".to_string())?;
    let input = input.to_string();
    let writer = std::thread::spawn(move || -> Result<(), String> {
        let result = stdin.write_all(input.as_bytes());
        // `stdin` 在这里出作用域被 drop，子进程收到 EOF——这个行为必须保留：
        // 父进程不主动关，子进程读 stdin 会永远等下去。
        match result {
            Ok(()) => Ok(()),
            // 子进程提前退出（参数错、没登录）会自己关掉 stdin，父进程这时候
            // 写入会拿到 BrokenPipe——这不是真的错误，退出码和 stderr 才是。
            Err(e) if e.kind() == std::io::ErrorKind::BrokenPipe => Ok(()),
            Err(e) => Err(format!("写 stdin 失败：{e}")),
        }
    });

    let out = child
        .wait_with_output()
        .map_err(|e| format!("等待失败：{e}"))?;
    // 线程 panic 不能 unwrap 带崩——转成错误字符串正常传回去。
    let write_result = writer
        .join()
        .unwrap_or_else(|_| Err("写 stdin 的线程 panic 了".to_string()));

    if !out.status.success() {
        return Err(format!(
            "{head} 退出码非零：{}",
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    write_result?;
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
            image_png_base64: None, image_mime: None,
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

    fn img_prompt() -> Prompt {
        Prompt { image_png_base64: Some("AQID".into()), image_mime: Some("image/jpeg".into()), ..p() }
    }

    type Seen = Arc<Mutex<Option<(Vec<String>, String, Option<std::path::PathBuf>)>>>;

    fn backend_seeing(seen: &Seen) -> CliBackend {
        let sink = seen.clone();
        CliBackend::with_runner(
            vec!["claude".into(), "-p".into()],
            Arc::new(move |cmd: &[String], input: &str| {
                // 路径是提示词里「at <path> (」之间的那段；趁文件还在读一下内容和权限。
                let path = input.split("Read the image file at ").nth(1).and_then(|r| r.split(" (use").next()).map(std::path::PathBuf::from);
                if let Some(pa) = &path {
                    assert_eq!(std::fs::read(pa).unwrap(), vec![1, 2, 3]);
                    #[cfg(unix)]
                    {
                        use std::os::unix::fs::PermissionsExt;
                        assert_eq!(std::fs::metadata(pa.parent().unwrap()).unwrap().permissions().mode() & 0o777, 0o700);
                    }
                }
                *sink.lock().unwrap() = Some((cmd.to_vec(), input.to_string(), path));
                Ok("ok".into())
            }),
        )
        .with_image_args(vec!["--allowedTools".into(), "Read".into()])
    }

    #[test]
    fn an_image_gets_a_temp_file_the_read_permission_and_is_cleaned_up() {
        let seen: Seen = Default::default();
        backend_seeing(&seen).complete(&img_prompt()).unwrap();
        let (cmd, input, path) = seen.lock().unwrap().clone().unwrap();
        assert_eq!(cmd, ["claude", "-p", "--allowedTools", "Read"]);
        let path = path.expect("提示词里要有图片路径");
        assert!(path.to_string_lossy().ends_with(".jpg"));
        assert!(input.contains("出了什么事？"), "原来的问题要保留");
        assert!(!path.parent().unwrap().exists(), "临时目录要删掉");
    }

    #[test]
    fn without_an_image_the_command_and_prompt_are_unchanged() {
        let seen: Seen = Default::default();
        backend_seeing(&seen).complete(&p()).unwrap();
        let (cmd, input, path) = seen.lock().unwrap().clone().unwrap();
        assert_eq!(cmd, ["claude", "-p"]);
        assert_eq!(input, "你是个助手\n\n出了什么事？");
        assert!(path.is_none());
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

    // 这条测的是 run_real 本身（不经 with_runner），因为要复现的问题只在真
    // 管道上才存在：写 stdin 和读 stdout 谁先谁后，是操作系统管道缓冲区和
    // 真实并发调度的事，字符串层面的注入测试完全绕不过它。用 `cat` 而不是
    // 某个 agent CLI，是因为这里要验的是我们自己收发管道的正确性，跟具体
    // 厂商命令、登录态都无关——`cat` 到处都有，天然满足「一边吐 stdout
    // 一边等 stdin 读完」的条件：它会原样把收到的每个字节写回去。
    #[cfg(unix)]
    #[test]
    fn run_real_does_not_deadlock_when_prompt_exceeds_the_pipe_buffer() {
        // 数百 KB，稳稳超过 macOS 16KB / Linux 64KB 的管道缓冲区上限。
        let big = "喵".repeat(200_000);
        let (tx, rx) = std::sync::mpsc::channel();
        let payload = big.clone();
        std::thread::spawn(move || {
            let result = run_real(
                &[crate::sys::testing::tool("cat")],
                &payload,
                &BTreeMap::new(),
            );
            let _ = tx.send(result);
        });
        // 用超时兜底：如果死锁又出现了，测试要能报「卡死」而不是把
        // 整个测试进程挂在这里等到天荒地老。
        let result = rx
            .recv_timeout(std::time::Duration::from_secs(10))
            .expect("run_real 卡死了——这正是要防的双向管道死锁");
        assert_eq!(result, Ok(big));
    }
}
