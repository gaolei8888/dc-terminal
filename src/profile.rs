use anyhow::{Context, Result};
use serde::Deserialize;
use std::collections::BTreeMap;

pub use crate::i18n::Lang;
use crate::proto::{IoReason, WarningCode};

/// 一段可翻译的文案。TOML 里写成子表：`[label]` 下面 `zh = "..."`。
///
/// profile 是**用户可编辑的数据文件**，进不了 i18n 的词条表——所以它的多语言
/// 走这条独立的路，而不是 `i18n::Key`。用户自己写的 profile 只写母语是常态。
#[derive(Debug, Clone, Default, Deserialize)]
pub struct LocalizedText {
    #[serde(default)]
    pub zh: Option<String>,
    #[serde(default)]
    pub en: Option<String>,
}

impl LocalizedText {
    /// 取不到就是 `None`，**不跨语言回落**。回落到另一种语言的话，英文界面里
    /// 会冒出一句中文，用户既看不懂也不知道那是哪来的；回落成什么由调用方
    /// 决定（`display_label` 回落到 profile 名，`display_note` 回落到空串），
    /// 那是它们各自知道、这里不知道的事。
    pub fn get(&self, lang: Lang) -> Option<&str> {
        match lang {
            Lang::Zh => self.zh.as_deref(),
            Lang::En => self.en.as_deref(),
        }
    }
}

/// 这个 profile 需要用户提供一份密钥。
#[derive(Debug, Clone, Deserialize)]
pub struct SecretSpec {
    /// 密钥注到哪个环境变量
    pub env: String,
    #[serde(default)]
    pub hint: LocalizedText,
    /// 申领页面，密钥界面上 Ctrl+O 打开
    #[serde(default)]
    pub url: Option<String>,
    #[serde(default)]
    pub verify: Option<VerifySpec>,
}

/// 存盘前拿这个端点探一下，确认密钥不是明显错的。
#[derive(Debug, Clone, Deserialize)]
pub struct VerifySpec {
    pub url: String,
}

/// 这个 agent 没装时怎么装。
#[derive(Debug, Clone, Deserialize)]
pub struct InstallSpec {
    pub command: Vec<String>,
    #[serde(default)]
    pub note: LocalizedText,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Profile {
    pub name: String,
    pub command: Vec<String>,
    #[serde(default)]
    pub is_agent: bool,
    #[serde(default)]
    pub idle_pattern: Option<String>,
    /// agent 干活时屏幕上一定有的串（比如 codex 的 `esc to interrupt`）。
    /// 比 `idle_pattern` 可靠：空闲时的输入框占位符用户一打字就没了。
    #[serde(default)]
    pub busy_pattern: Option<String>,
    #[serde(default)]
    pub env: BTreeMap<String, String>,
    #[serde(default)]
    pub secret: Option<SecretSpec>,
    #[serde(default)]
    pub install: Option<InstallSpec>,
    #[serde(default)]
    pub label: LocalizedText,
    #[serde(default)]
    pub note: LocalizedText,
}

const CLAUDE: &str = include_str!("../profiles/claude.toml");
const CODEX: &str = include_str!("../profiles/codex.toml");
const OPENCODE: &str = include_str!("../profiles/opencode.toml");
const QWEN: &str = include_str!("../profiles/qwen.toml");
const KIMI: &str = include_str!("../profiles/kimi.toml");
const GLM: &str = include_str!("../profiles/glm.toml");
const DEEPSEEK: &str = include_str!("../profiles/deepseek.toml");
const QWEN_API: &str = include_str!("../profiles/qwen-api.toml");
const SHELL: &str = include_str!("../profiles/shell.toml");

impl Profile {
    pub fn from_toml(s: &str) -> Result<Profile> {
        toml::from_str(s).context("profile TOML 解析失败")
    }

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
            "claude", "codex", "opencode", "qwen", "kimi", "glm", "deepseek", "qwen-api", "shell",
        ]
    }

    pub fn builtins() -> Vec<Profile> {
        Profile::builtin_names()
            .into_iter()
            .filter_map(Profile::builtin)
            .collect()
    }

    pub fn idle_regex(&self) -> Result<Option<regex::Regex>> {
        match &self.idle_pattern {
            None => Ok(None),
            Some(p) => {
                Ok(Some(regex::Regex::new(p).with_context(|| {
                    format!("idle_pattern 不是合法正则: {p}")
                })?))
            }
        }
    }

    pub fn busy_regex(&self) -> Result<Option<regex::Regex>> {
        match &self.busy_pattern {
            None => Ok(None),
            Some(p) => {
                Ok(Some(regex::Regex::new(p).with_context(|| {
                    format!("busy_pattern 不是合法正则: {p}")
                })?))
            }
        }
    }

    /// 菜单上显示的名字。没写 label 就回落到 profile 名——那至少是个能认的词。
    pub fn display_label(&self, lang: Lang) -> String {
        self.label.get(lang).unwrap_or(&self.name).to_string()
    }

    /// 菜单上的一行说明。没写就回落到**空串**，不回落到 name——
    /// 说明栏里再显示一遍命令名是噪音，不是信息。
    pub fn display_note(&self, lang: Lang) -> String {
        self.note.get(lang).unwrap_or("").to_string()
    }
}

use std::path::{Path, PathBuf};

/// 自定义 profile 目录，跟着 socket 走——测试把 socket 放临时目录就自动隔离，
/// 不会去读用户真实的 ~/.dct/profiles/（同 `projects::store_path_for_socket`）。
pub fn profiles_dir_for_socket(socket: &Path) -> PathBuf {
    match socket.parent() {
        Some(d) => d.join("profiles"),
        None => PathBuf::from("profiles"),
    }
}

/// `io::Error` 的 `Display` 是系统原话（英文，常年带 `os error N` 这种
/// 只有程序员看得懂的后缀），直接甩给零编程经验的用户就是一份变相栈追踪。
/// 这里按 `ErrorKind` 挑几种用户分得清、也做得了什么的说法；分不清的
/// 归到一句笼统的「读取失败」——原始详情不丢，调用方负责写到 stderr，
/// 不冒泡到界面上。
pub(crate) fn io_reason(e: &std::io::Error) -> IoReason {
    match e.kind() {
        std::io::ErrorKind::PermissionDenied => IoReason::PermissionDenied,
        std::io::ErrorKind::NotADirectory => IoReason::NotADirectory,
        _ => IoReason::Other,
    }
}

/// 读一个目录下所有 `*.toml`。第二个返回值是每个读不了的文件的人话错误——
/// **不能静默跳过**：用户自己写的 profile 没出现在菜单里，他需要知道为什么。
pub fn load_dir(dir: &Path) -> (Vec<Profile>, Vec<WarningCode>) {
    let mut found = Vec::new();
    let mut errs = Vec::new();

    // 目录不存在是绝大多数用户的正常状态（没建过自定义 profile），不该报错；
    // 但权限之类的其它读取失败不能悄悄吞掉——那和这个函数「不能静默跳过」的
    // 初衷正好相反，只是从「文件」这一层挪到了「目录」这一层。
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return (found, errs),
        Err(e) => {
            let name = dir.to_string_lossy();
            // 原始系统错误写到 stderr 留个诊断痕迹，界面上只给人话——
            // 见 describe_io_error 的注释。
            eprintln!("{name} 打不开：{e}");
            errs.push(WarningCode::ProfileDirUnreadable {
                name: name.to_string(),
                reason: io_reason(&e),
            });
            return (found, errs);
        }
    };

    // read_dir 的顺序由文件系统决定，不排序的话菜单每次启动都可能换序
    let mut paths: Vec<PathBuf> = entries
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "toml"))
        .collect();
    paths.sort();

    for path in paths {
        let name = path.file_name().unwrap_or_default().to_string_lossy();
        match std::fs::read_to_string(&path) {
            Err(e) => {
                eprintln!("{name} 读不了：{e}");
                errs.push(WarningCode::ProfileUnreadable {
                    name: name.to_string(),
                    reason: io_reason(&e),
                });
            }
            Ok(src) => match Profile::from_toml(&src) {
                // `Profile::from_toml` 用 `.context()` 包了一层，anyhow 的
                // Display 对 context 错误只吐 context 那句话，底层
                // toml::de::Error 的行号和具体原因（缺字段/写错类型……）都被吞了。
                // 把它挖出来才对用户有用；root_cause() 拿到的还是同一个
                // toml::de::Error，span/message 都在，只是不走它自带的多行
                // ASCII 图 Display（那是给等宽终端排版看的，不是人话）。
                Err(e) => {
                    let (line, reason) = e
                        .root_cause()
                        .downcast_ref::<toml::de::Error>()
                        .map(|te| describe_toml_error(te, &src))
                        .unwrap_or((None, e.to_string()));
                    errs.push(WarningCode::ProfileMalformed {
                        name: name.to_string(),
                        line,
                        reason,
                    });
                }
                Ok(p) => found.push(p),
            },
        }
    }
    (found, errs)
}

/// 把 `toml::de::Error` 拍成人话的一行：保留它的原因（缺字段、类型不对……）
/// 和大致的行号，丢掉它自带的 `TOML parse error at line X, column Y\n  |\n...`
/// 那套多行图形化 Display——那是给等宽终端排版看的，直接甩给用户就是一份栈追踪。
///
/// `err.message()` 本身看着像纯文字，但不保证不含换行：底层 winnow 在错误里同时
/// 带了「标签」和「期望是什么」两条上下文时，会用换行把两句拼在一起（比如
/// 写错转义符会得到 `"invalid escape sequence\nexpected \`b\`, \`f\`, ..."`）。
/// 这行菜单状态栏只能放一行字，两行糊在一起在等宽终端上会错位换行，看着又是
/// 一份变相的栈追踪。这里把内部换行拍平成中文顿号式的分隔符——两句话都留着，
/// 「expected ...」那半句是真正告诉用户该怎么改的部分，直接丢掉可惜。
pub(crate) fn describe_toml_error(err: &toml::de::Error, src: &str) -> (Option<usize>, String) {
    let reason = err
        .message()
        .lines()
        .filter(|line| !line.trim().is_empty())
        .collect::<Vec<_>>()
        .join("；");
    match err.span() {
        Some(span) => (
            Some(src[..span.start.min(src.len())].matches('\n').count() + 1),
            reason,
        ),
        None => (None, reason),
    }
}

/// 内置 + 磁盘。同名以磁盘为准（用户改了就是要改），新名字追加在后面。
pub fn all_profiles(dir: &Path) -> (Vec<Profile>, Vec<WarningCode>) {
    let (disk, errs) = load_dir(dir);
    let mut out = Profile::builtins();
    for p in disk {
        match out.iter_mut().find(|b| b.name == p.name) {
            Some(slot) => *slot = p,
            None => out.push(p),
        }
    }
    (out, errs)
}

/// 这个 profile 现在能不能用，不能的话卡在哪。
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum ProfileStatus {
    Ready,
    /// 声明了 secret 但密钥仓里没有
    NeedsSecret,
    /// 跑的是别的 profile 的命令，而那个命令没装。`label` 是那个 profile 的显示名。
    NeedsDependency {
        label: String,
    },
    /// `command[0]` 在 PATH 上找不到，而且这个命令就是它自己
    NotInstalled {
        command: String,
    },
}

/// `cmd` 能不能执行。带斜杠当路径查，否则遍历 PATH。
///
/// **这个判断必须和实际 spawn 用同一个环境**，所以只能在守护进程里调用——
/// 界面进程的 PATH 可能不一样，那会导致「菜单说能用，一开就失败」。
pub fn command_exists(cmd: &str) -> bool {
    fn is_exec(p: &Path) -> bool {
        use std::os::unix::fs::PermissionsExt;
        std::fs::metadata(p)
            .map(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
            .unwrap_or(false)
    }

    if cmd.contains('/') {
        return is_exec(Path::new(cmd));
    }
    let Ok(path) = std::env::var("PATH") else {
        return false;
    };
    path.split(':')
        .filter(|d| !d.is_empty())
        .any(|d| is_exec(&Path::new(d).join(cmd)))
}

/// `command[0]` 这个命令「归谁所有」——名字和命令名相同的那个 profile。
///
/// kimi/glm/deepseek/qwen-api 的 command[0] 都是 `claude`，归 `claude` 这个
/// profile 所有；`claude` 自己的名字就是 `claude`，所以它是自己的 owner。
/// 靠这个区分「我没装」和「我依赖的东西没装」。
fn dependency_owner<'a>(all: &'a [Profile], cmd: &str) -> Option<&'a Profile> {
    let base = Path::new(cmd)
        .file_name()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| cmd.to_string());
    all.iter().find(|p| p.name == base)
}

pub fn status_of(
    p: &Profile,
    all: &[Profile],
    has_secret: bool,
    installed: &dyn Fn(&str) -> bool,
    lang: Lang,
) -> ProfileStatus {
    let Some(cmd) = p.command.first() else {
        // 解析层允许空 command（TOML 里写了 `command = []`），这里兜住，
        // 免得 spawn 的时候 panic
        return ProfileStatus::NotInstalled {
            command: String::new(),
        };
    };

    // 顺序不能换：装没装排在密钥前面。见测试
    // `dependency_is_reported_before_secret` 的注释。
    if !installed(cmd) {
        return match dependency_owner(all, cmd) {
            Some(owner) if owner.name != p.name => ProfileStatus::NeedsDependency {
                label: owner.display_label(lang),
            },
            _ => ProfileStatus::NotInstalled {
                command: cmd.clone(),
            },
        };
    }

    if p.secret.is_some() && !has_secret {
        return ProfileStatus::NeedsSecret;
    }

    ProfileStatus::Ready
}

#[cfg(test)]
mod tests {
    use super::*;

    fn status_fixture() -> Vec<Profile> {
        Profile::builtins()
    }

    #[test]
    fn ready_when_installed_and_secret_present() {
        let all = status_fixture();
        let kimi = all.iter().find(|p| p.name == "kimi").unwrap();
        let st = status_of(kimi, &all, true, &|_| true, Lang::Zh);
        assert!(matches!(st, ProfileStatus::Ready));
    }

    #[test]
    fn needs_secret_when_installed_but_no_key() {
        let all = status_fixture();
        let kimi = all.iter().find(|p| p.name == "kimi").unwrap();
        let st = status_of(kimi, &all, false, &|_| true, Lang::Zh);
        assert!(matches!(st, ProfileStatus::NeedsSecret));
    }

    #[test]
    fn not_installed_when_the_command_owns_its_name() {
        let all = status_fixture();
        let codex = all.iter().find(|p| p.name == "codex").unwrap();
        let st = status_of(codex, &all, false, &|_| false, Lang::Zh);
        match st {
            ProfileStatus::NotInstalled { command } => assert_eq!(command, "codex"),
            other => panic!("codex 自己就是那个命令，应当报未安装，得到 {other:?}"),
        }
    }

    #[test]
    fn dependency_is_reported_before_secret() {
        // 这条顺序是整个判定里最要紧的：kimi 跑的是 claude。claude 没装时
        // 如果先报「未填密钥」，用户会去填 key，填完还是起不来，
        // 然后以为是 key 的问题——被送进死胡同。
        let all = status_fixture();
        let kimi = all.iter().find(|p| p.name == "kimi").unwrap();
        let st = status_of(kimi, &all, false, &|_| false, Lang::Zh);
        match st {
            ProfileStatus::NeedsDependency { label } => assert_eq!(label, "Claude"),
            other => panic!("claude 没装时 kimi 要报依赖，不是密钥，得到 {other:?}"),
        }
    }

    #[test]
    fn dependency_uses_the_owner_profiles_label_not_the_raw_command() {
        let all = status_fixture();
        let glm = all.iter().find(|p| p.name == "glm").unwrap();
        let st = status_of(glm, &all, true, &|c| c != "claude", Lang::Zh);
        match st {
            ProfileStatus::NeedsDependency { label } => {
                assert_eq!(label, "Claude", "给用户看 label，不是二进制名");
            }
            other => panic!("得到 {other:?}"),
        }
    }

    #[test]
    fn profile_without_secret_is_ready_when_installed() {
        let all = status_fixture();
        let shell = all.iter().find(|p| p.name == "shell").unwrap();
        assert!(matches!(
            status_of(shell, &all, false, &|_| true, Lang::Zh),
            ProfileStatus::Ready
        ));
    }

    #[test]
    fn command_exists_finds_sh_and_not_a_made_up_name() {
        assert!(command_exists("sh"), "PATH 上一定有 sh");
        assert!(!command_exists("dct-绝对没有这个命令-x9"));
    }

    #[test]
    fn command_exists_handles_absolute_paths() {
        assert!(command_exists("/bin/sh"));
        assert!(!command_exists("/bin/根本没有这个"));
    }

    #[test]
    fn parses_toml() {
        let p = Profile::from_toml(
            r#"
            name = "demo"
            command = ["echo", "hi"]
            is_agent = true
            idle_pattern = "\\$ $"
            "#,
        )
        .unwrap();
        assert_eq!(p.name, "demo");
        assert_eq!(p.command, vec!["echo", "hi"]);
        assert!(p.is_agent);
    }

    #[test]
    fn builtin_claude_uses_bypass_flag() {
        let p = Profile::builtin("claude").unwrap();
        assert!(p
            .command
            .contains(&"--dangerously-skip-permissions".to_string()));
        assert!(p.is_agent);
    }

    #[test]
    fn builtin_shell_is_not_agent() {
        let p = Profile::builtin("shell").unwrap();
        assert!(!p.is_agent);
        assert!(p.idle_pattern.is_none());
    }

    #[test]
    fn builtin_names_includes_claude_and_shell() {
        let names = Profile::builtin_names();
        assert!(names.contains(&"claude"));
        assert!(names.contains(&"shell"));
    }

    #[test]
    fn unknown_builtin_is_none() {
        assert!(Profile::builtin("nope").is_none());
    }

    #[test]
    fn idle_regex_compiles() {
        let p = Profile::builtin("claude").unwrap();
        let re = p.idle_regex().unwrap().unwrap();
        assert!(re.is_match("  ? for shortcuts"));
    }

    #[test]
    fn parses_env_and_secret() {
        let p = Profile::from_toml(
            r#"
            name = "kimi"
            command = ["claude"]
            is_agent = true

            [label]
            zh = "Kimi"

            [note]
            zh = "月之暗面"

            [env]
            ANTHROPIC_BASE_URL = "https://example.com/anthropic"

            [secret]
            env = "ANTHROPIC_AUTH_TOKEN"
            url = "https://example.com/keys"

            [secret.hint]
            zh = "去后台复制 API Key"

            [secret.verify]
            url = "https://example.com/anthropic/v1/messages"
            "#,
        )
        .unwrap();

        assert_eq!(
            p.env.get("ANTHROPIC_BASE_URL").map(String::as_str),
            Some("https://example.com/anthropic")
        );
        let s = p.secret.as_ref().unwrap();
        assert_eq!(s.env, "ANTHROPIC_AUTH_TOKEN");
        assert_eq!(s.hint.get(Lang::Zh), Some("去后台复制 API Key"));
        assert_eq!(s.url.as_deref(), Some("https://example.com/keys"));
        assert_eq!(
            s.verify.as_ref().unwrap().url,
            "https://example.com/anthropic/v1/messages"
        );
        assert_eq!(p.label.get(Lang::Zh), Some("Kimi"));
        assert_eq!(p.note.get(Lang::Zh), Some("月之暗面"));
    }

    /// 内置 profile 的每一条 `en` 里都不许出现汉字。
    ///
    /// 这条不是洁癖：补英文那次我用脚本批量加 `en =`，品牌名（`Claude`/`Kimi`）
    /// 中英同形所以直接抄了中文那行，结果 `shell` 的 label 被抄成
    /// `en = "命令行"`——英文界面上凭空冒出一句中文，而回落机制看它非空
    /// 就认了。人工补译很容易再犯同样的错，交给测试。
    #[test]
    fn no_builtin_profile_smuggles_chinese_into_its_english_text() {
        let cjk = |s: &str| s.chars().any(|c| ('\u{4e00}'..='\u{9fff}').contains(&c));
        for p in Profile::builtins() {
            for (what, t) in [
                ("label", &p.label),
                ("note", &p.note),
                (
                    "secret.hint",
                    &p.secret
                        .as_ref()
                        .map(|s| s.hint.clone())
                        .unwrap_or_default(),
                ),
                (
                    "install.note",
                    &p.install
                        .as_ref()
                        .map(|i| i.note.clone())
                        .unwrap_or_default(),
                ),
            ] {
                if let Some(en) = t.en.as_deref() {
                    assert!(!cjk(en), "{} 的 {what} 里 en 写着中文：{en}", p.name);
                }
            }
        }
    }

    /// profile 的 TOML 里 `[note]` 下同时写了 `zh` 和 `en`，两边都要取得到。
    /// 这是 i18n 那一期之前唯一说不通的地方：`Lang` 只有 `Zh` 一个变体，
    /// 用户写了 `en = "..."` 也永远没人读。
    #[test]
    fn localized_text_serves_both_languages() {
        let p = Profile::from_toml(
            r#"
            name = "kimi"
            command = ["kimi"]
            is_agent = true
            [label]
            zh = "Kimi"
            en = "Kimi"
            [note]
            zh = "月之暗面"
            en = "Moonshot AI"
            "#,
        )
        .unwrap();
        assert_eq!(p.display_note(Lang::Zh), "月之暗面");
        assert_eq!(p.display_note(Lang::En), "Moonshot AI");
    }

    /// 只写了中文的 profile，英文界面下回落到 profile 名而不是显示中文——
    /// 用户自己写的 profile 不会有人替他翻译，回落必须是个他认得的词。
    #[test]
    fn a_chinese_only_label_falls_back_to_the_profile_name_in_english() {
        let p = Profile::from_toml(
            r#"
            name = "myagent"
            command = ["x"]
            is_agent = true
            [label]
            zh = "我的助手"
            "#,
        )
        .unwrap();
        assert_eq!(p.display_label(Lang::Zh), "我的助手");
        assert_eq!(p.display_label(Lang::En), "myagent");
    }

    #[test]
    fn parses_busy_pattern_and_install() {
        let p = Profile::from_toml(
            r#"
            name = "codex"
            command = ["codex"]
            is_agent = true
            busy_pattern = "esc to interrupt"

            [install]
            command = ["npm", "i", "-g", "@openai/codex"]

            [install.note]
            zh = "需要先装 Node.js"
            "#,
        )
        .unwrap();

        assert_eq!(p.busy_pattern.as_deref(), Some("esc to interrupt"));
        let i = p.install.as_ref().unwrap();
        assert_eq!(i.command, vec!["npm", "i", "-g", "@openai/codex"]);
        assert_eq!(i.note.get(Lang::Zh), Some("需要先装 Node.js"));
    }

    #[test]
    fn new_fields_all_default_to_empty() {
        // 老 profile 文件（只有 name/command/is_agent）必须照样能解析
        let p = Profile::from_toml(
            r#"
            name = "shell"
            command = ["/bin/zsh"]
            "#,
        )
        .unwrap();
        assert!(p.env.is_empty());
        assert!(p.secret.is_none());
        assert!(p.install.is_none());
        assert!(p.busy_pattern.is_none());
        assert_eq!(p.label.get(Lang::Zh), None);
    }

    #[test]
    fn busy_regex_compiles() {
        let p = Profile::from_toml(
            r#"
            name = "x"
            command = ["x"]
            busy_pattern = "esc to interrupt"
            "#,
        )
        .unwrap();
        let re = p.busy_regex().unwrap().unwrap();
        assert!(re.is_match("  (12s • esc to interrupt)"));
        assert!(!re.is_match("? for shortcuts"));
    }

    #[test]
    fn bad_busy_pattern_is_an_error() {
        let p = Profile::from_toml(
            r#"
            name = "x"
            command = ["x"]
            busy_pattern = "["
            "#,
        )
        .unwrap();
        assert!(p.busy_regex().is_err(), "非法正则要报错，不能静默当没有");
    }

    #[test]
    fn every_builtin_parses_and_is_well_formed() {
        for name in Profile::builtin_names() {
            let p = Profile::builtin(name).unwrap_or_else(|| panic!("{name} 应当是内置 profile"));
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
                "claude", "codex", "opencode", "qwen", "kimi", "glm", "deepseek", "qwen-api",
                "shell",
            ]
        );
    }

    #[test]
    fn api_shaped_profiles_run_claude_and_need_a_secret() {
        for name in ["kimi", "glm", "deepseek", "qwen-api"] {
            let p = Profile::builtin(name).unwrap();
            assert_eq!(p.command[0], "claude", "{name}: API 形态跑的是 claude");
            assert!(
                p.env.contains_key("ANTHROPIC_BASE_URL"),
                "{name}: 要换 base_url"
            );
            let s = p
                .secret
                .as_ref()
                .unwrap_or_else(|| panic!("{name}: 要声明密钥"));
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
        assert!(p
            .busy_regex()
            .unwrap()
            .unwrap()
            .is_match("(12s • esc to interrupt)"));
    }

    #[test]
    fn unverified_profiles_have_no_pattern() {
        // opencode / qwen 的 TUI 没实测过。宁可状态显示「—」，不能瞎猜一个 pattern
        // 然后在看板上编状态。
        for name in ["opencode", "qwen"] {
            let p = Profile::builtin(name).unwrap();
            assert!(
                p.idle_pattern.is_none() && p.busy_pattern.is_none(),
                "{name}: 没实测就别填 pattern"
            );
        }
    }

    #[test]
    fn profiles_dir_sits_next_to_socket() {
        let p = profiles_dir_for_socket(std::path::Path::new("/home/x/.dct/daemon.sock"));
        assert_eq!(p, std::path::PathBuf::from("/home/x/.dct/profiles"));
    }

    #[test]
    fn disk_profile_overrides_builtin_of_same_name() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(
            tmp.path().join("claude.toml"),
            "name = \"claude\"\ncommand = [\"my-claude\"]\n",
        )
        .unwrap();

        let (all, errs) = all_profiles(tmp.path());
        assert!(errs.is_empty());
        let claude = all.iter().find(|p| p.name == "claude").unwrap();
        assert_eq!(
            claude.command,
            vec!["my-claude"],
            "磁盘的同名 profile 要覆盖内置"
        );
        assert_eq!(
            all.iter().filter(|p| p.name == "claude").count(),
            1,
            "覆盖不是追加"
        );
    }

    #[test]
    fn disk_profile_with_new_name_is_appended_after_builtins() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(
            tmp.path().join("mine.toml"),
            "name = \"mine\"\ncommand = [\"echo\"]\n",
        )
        .unwrap();

        let (all, _) = all_profiles(tmp.path());
        assert_eq!(all.last().unwrap().name, "mine", "新增的排在内置后面");
        assert_eq!(all[0].name, "claude", "内置顺序不受影响");
    }

    #[test]
    fn broken_disk_profile_reports_the_filename_and_keeps_the_rest() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("bad.toml"), "这不是 TOML {{{").unwrap();
        std::fs::write(
            tmp.path().join("good.toml"),
            "name = \"good\"\ncommand = [\"echo\"]\n",
        )
        .unwrap();

        let (all, errs) = all_profiles(tmp.path());
        assert!(
            all.iter().any(|p| p.name == "good"),
            "一个坏文件不能连累其它的"
        );
        assert_eq!(errs.len(), 1);
        let WarningCode::ProfileMalformed { name, line, reason } = &errs[0] else {
            panic!("应当是「写错了」这一类：{:?}", errs[0]);
        };
        assert_eq!(name, "bad.toml", "错误里要说是哪个文件");
        // anyhow 的 `.context()` 会把底层 toml::de::Error 的行号和原因吞掉，
        // 只剩一句「profile TOML 解析失败」——那等于把「坏了」重说一遍，
        // 用户本来就知道坏了。这里要证明详细原因确实透出来了。
        assert!(
            *line == Some(1) && reason.contains("invalid key"),
            "错误里要带解析细节（行号+原因），不能退化成一句空话：{:?}",
            errs[0]
        );
    }

    #[test]
    fn toml_error_with_embedded_newline_still_collapses_to_one_line() {
        // 非程序员很可能在字符串里写 Windows 路径这类带反斜杠的东西，
        // 比如 `name = "C:\Users\x"`——TOML 里反斜杠是转义符起始，
        // 这种写法不合法。toml::de::Error::message() 对「转义符不认识」
        // 这类错误会把「哪里错了」和「该写什么」拼成两行（中间一个 \n），
        // describe_toml_error 必须把这两行拍平，不能让换行漏到 errs 里——
        // 状态栏只有一行，漏了换行在等宽终端上就是错位的半份栈追踪。
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(
            tmp.path().join("windows_path.toml"),
            "name = \"C:\\x\"\ncommand = [\"echo\"]\n",
        )
        .unwrap();

        let (_, errs) = load_dir(tmp.path());
        assert_eq!(errs.len(), 1);
        let WarningCode::ProfileMalformed { reason, .. } = &errs[0] else {
            panic!("应当是「写错了」这一类：{:?}", errs[0]);
        };
        assert!(
            !reason.contains('\n'),
            "原因要是单行，不能带换行糊到状态栏上：{reason}"
        );
        assert!(
            reason.contains("invalid escape sequence"),
            "第一句原因不能丢：{reason}"
        );
        assert!(
            reason.contains("expected"),
            "该怎么改的那半句不能丢：{reason}"
        );
    }

    #[test]
    #[cfg(unix)]
    fn unreadable_dir_reports_an_error_instead_of_going_silent() {
        use std::os::unix::fs::PermissionsExt;

        let tmp = tempfile::tempdir().unwrap();
        let locked = tmp.path().join("locked");
        std::fs::create_dir(&locked).unwrap();
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o000)).unwrap();

        // 权限位落在 Drop 里恢复，而不是函数末尾的一条语句——下面几个 assert!
        // 失败会 panic 并直接展开出函数，末尾的语句根本执行不到。用 RAII 保证
        // 不管走正常路径还是 panic 都会把目录改回可读可写，否则 tempdir 自己
        // 的 Drop 删不掉这个目录，会在这条测试之外拖出一片脏临时文件。
        struct RestorePerms<'a>(&'a std::path::Path);
        impl Drop for RestorePerms<'_> {
            fn drop(&mut self) {
                let _ = std::fs::set_permissions(self.0, std::fs::Permissions::from_mode(0o700));
            }
        }
        let _restore = RestorePerms(&locked);

        // root（常见于容器化的 CI）不受目录权限位约束，读得穿。那种环境下这条
        // 测试验证的分支根本触发不了，硬跑只会得到一个和权限无关的 flaky 失败——
        // 与其那样，不如老实跳过。
        let root_ignores_permissions = std::fs::read_dir(&locked).is_ok();

        if !root_ignores_permissions {
            let (found, errs) = load_dir(&locked);
            assert!(found.is_empty(), "目录读不了，不该假装读到了空目录");
            assert!(
                !errs.is_empty(),
                "目录存在但读不了（比如权限不对）不能和「目录不存在」一样静默——\
                 用户既拿不到自定义 profile，也拿不到任何解释"
            );
            let WarningCode::ProfileDirUnreadable { name, reason } = &errs[0] else {
                panic!("应当是「目录打不开」这一类：{:?}", errs[0]);
            };
            assert!(name.contains("locked"), "错误里要指出是哪个目录：{name}");
            // io::Error 的 Display 是英文系统原话（"Permission denied (os
            // error 13)"），零编程经验的用户看不懂 errno。现在结构上就没有
            // 地方能塞进原文——码里只有一个分类枚举。
            assert_eq!(*reason, IoReason::PermissionDenied, "要点名是权限问题");
            let line = crate::i18n::msg::warning(crate::i18n::Lang::Zh, &errs[0]);
            assert!(
                !line.contains("os error") && !line.contains("Permission denied"),
                "组出来的话里不能有系统原话：{line}"
            );
            assert!(line.contains("权限"), "中文下要说「权限」：{line}");
        }
    }

    #[test]
    #[cfg(unix)]
    fn unreadable_file_reports_an_error_in_plain_chinese() {
        use std::os::unix::fs::PermissionsExt;

        let tmp = tempfile::tempdir().unwrap();
        let f = tmp.path().join("locked.toml");
        std::fs::write(&f, "name = \"x\"\ncommand = [\"echo\"]\n").unwrap();
        std::fs::set_permissions(&f, std::fs::Permissions::from_mode(0o000)).unwrap();

        struct RestorePerms<'a>(&'a std::path::Path);
        impl Drop for RestorePerms<'_> {
            fn drop(&mut self) {
                let _ = std::fs::set_permissions(self.0, std::fs::Permissions::from_mode(0o600));
            }
        }
        let _restore = RestorePerms(&f);

        // root（常见于容器化 CI）不受文件权限位约束，跳过而不是硬跑出一个
        // 和权限无关的 flaky 失败——同上面 unreadable_dir 那条测试的理由。
        if std::fs::read_to_string(&f).is_err() {
            let (found, errs) = load_dir(tmp.path());
            assert!(found.is_empty());
            assert_eq!(errs.len(), 1);
            let WarningCode::ProfileUnreadable { name, reason } = &errs[0] else {
                panic!("应当是「文件读不了」这一类：{:?}", errs[0]);
            };
            assert_eq!(name, "locked.toml");
            assert_eq!(*reason, IoReason::PermissionDenied);
            let line = crate::i18n::msg::warning(crate::i18n::Lang::Zh, &errs[0]);
            assert!(
                !line.contains("os error") && !line.contains("Permission denied"),
                "不能把系统原话漏给用户：{line}"
            );
        }
    }

    #[test]
    fn missing_dir_is_not_an_error() {
        let tmp = tempfile::tempdir().unwrap();
        let (all, errs) = all_profiles(&tmp.path().join("根本没这个目录"));
        assert!(errs.is_empty(), "没建过自定义目录是常态，不是错误");
        assert_eq!(all.len(), 9, "只有内置");
    }

    #[test]
    fn non_toml_files_are_ignored() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("README.md"), "随手放的笔记").unwrap();
        let (_, errs) = all_profiles(tmp.path());
        assert!(errs.is_empty(), "非 .toml 文件直接跳过，不该报错");
    }
}
