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
    /// agent 失败时屏幕上一定会出现的串（比如 Claude Code 的 `API Error`）。
    ///
    /// **只给见过真实错误文案的 profile 写。** 凭想象编正则会造出误报，
    /// 而误报比不报更糟：一个好端端的会话被标成失败，用户跑去看一个根本
    /// 没出事的东西，然后就不再相信这个标记了。没写 = 这个功能对它关着。
    #[serde(default)]
    pub error_pattern: Option<String>,
    #[serde(default)]
    pub env: BTreeMap<String, String>,
    #[serde(default)]
    pub secret: Option<SecretSpec>,
    /// 这个 profile 只做 dct 自己的 LLM 后端，**不是一个能开会话的 agent**。
    ///
    /// 起因是 dc_llm 这类东西：它是一个 HTTP 端点，压根没有命令行。可
    /// `Profile` 又要求写 `command`，于是硬塞一个不存在的命令，选择器上
    /// 就多出一条永远灰着的「未安装」——而它永远装不上，因为根本没有那个
    /// 东西可装。这正是仓库那条「屏幕上不写按不动的键」要挡的情况。
    ///
    /// 打了这个标记的 profile：
    /// - **不进** agent 选择器（见 `view::agent_rows`）
    /// - **仍然进**密钥页——那儿正是用户要填这个端点令牌的地方
    /// - 仍然能被 `[llm].provider` 指名（`resolve` 走的是另一条查找路径）
    #[serde(default)]
    pub backend_only: bool,
    #[serde(default)]
    pub install: Option<InstallSpec>,
    #[serde(default)]
    pub headless: Option<HeadlessSpec>,
    #[serde(default)]
    pub api: Option<ApiSpec>,
    #[serde(default)]
    pub label: LocalizedText,
    #[serde(default)]
    pub note: LocalizedText,
    /// 恢复会话时追加到 `command` 后面的参数（比如 `claude` 的
    /// `--continue`）。**只给实测过「这个参数确实是恢复上一次对话」的
    /// profile 写**——凭空编一个会造出「看着在恢复、其实开了个新对话」
    /// 的假象，比老老实实开一个新会话更糟：用户会带着错的上下文继续干活。
    ///
    /// 恢复时该不该真的把这些参数接上去，不是这个字段自己决定的——见
    /// `last_sessions::group_for_resume` 的文档：同一个目录下的多个
    /// 会话如果都在跑同一个 profile，`claude --continue` 只会捡回**最新**
    /// 那一份对话，所以一组里只有最近活跃的那一个才会真的带上这些参数。
    #[serde(default)]
    pub resume_args: Vec<String>,
}

const DC: &str = include_str!("../profiles/dc.toml");
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
            "dc" => DC,
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
        let mut p = Profile::from_toml(src).expect("内置 profile 必须能解析");
        // `shell` 要跑哪个 shell 编译期定不下来：TOML 里写死任何一个都会在
        // 某类机器上落空（macOS 有 /bin/zsh，Ubuntu 默认没有），而落空的后果
        // 不是「换一个 shell 跑」，是「命令行」整行被标成没安装、按下去只剩
        // 一句找不到。它偏偏是唯一 `is_agent = false` 的内置 profile——在不是
        // git 仓库的目录里，别的九项全被 `NotAGitRepo` 挡住，它是仅剩的那一项。
        if p.name == "shell" {
            p.command = vec![login_shell()];
        }
        Some(p)
    }

    /// 返回顺序就是菜单顺序：训练营那一项排头，然后独立 CLI，再 API 形态，
    /// 命令行垫底。命令行放最后是因为它对目标用户价值最低——非程序员不需要
    /// 裸终端。
    ///
    /// **`dc` 排第一不是偏心，是那一项是唯一一个新学生按下去就能跑的。**
    /// 第一位在这里是有实际后果的：一个从没开过会话的项目，`n` 弹出选择器
    /// 且光标落在第一项上（`quick_start_target` 只在「上次那个仍然 Ready」
    /// 时才直开），而学生第一次选的那个会被记成这个项目的 last_profile，
    /// 从此 `n` 一直开它。第一位放 `claude`，中国学生的第一下就落在一个
    /// 要外币订阅才起得来的东西上。
    pub fn builtin_names() -> Vec<&'static str> {
        vec![
            "dc", "claude", "codex", "opencode", "qwen", "kimi", "glm", "deepseek", "qwen-api",
            "shell",
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

    pub fn error_regex(&self) -> Result<Option<regex::Regex>> {
        match &self.error_pattern {
            None => Ok(None),
            Some(p) => {
                Ok(Some(regex::Regex::new(p).with_context(|| {
                    format!("error_pattern 不是合法正则: {p}")
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

/// 按 `[menu] agents` 把菜单裁短，顺序照那份清单写的来。
///
/// 谁需要这个：给一个班、一个团队统一发机器的人。十项摆在零基础的人面前，
/// 其中八项要么他这辈子不会用，要么要他自己去某个网站注册充值——而那正是
/// 发机器的人替他免掉的那一步。
///
/// **空清单 = 不裁。** 绝大多数用户没写这一段，他们要看到全部。
///
/// **裁到一个不剩时也不裁。** 清单里的名字全写错了（改了自定义 profile 的
/// 名字、删了一个 toml），照裁的话用户会打开一个空菜单——一个什么都开不了、
/// 也不解释为什么的界面，比多几项没用的东西糟得多。宁可多显示，不可无路可走。
///
/// **必须在 `status_of` 之后调用，不能先裁 profile 再算状态。** 状态里的
/// `NeedsDependency` 要回头在全量清单里找「这条命令归谁」（`dc` 跑的是
/// `claude` 那个二进制），先裁掉 `claude` 的话，这一步就找不到那个用来
/// 显示的名字了。
pub fn trim_menu(
    entries: Vec<crate::proto::ProfileEntry>,
    only: &[String],
) -> Vec<crate::proto::ProfileEntry> {
    if only.is_empty() {
        return entries;
    }
    let kept: Vec<_> = only
        .iter()
        .filter_map(|name| entries.iter().find(|e| &e.name == name).cloned())
        .collect();
    if kept.is_empty() {
        return entries;
    }
    kept
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

/// 内置 `shell` profile 真正要跑的那个 shell。按平台分（`sys::shell`）：
/// Unix 上是 `$SHELL` 或 bash/zsh/sh，Windows 上是 PowerShell 或 cmd.exe。
fn login_shell() -> String {
    crate::sys::shell::login_shell()
}

/// `cmd` 能不能执行。带斜杠当路径查，否则遍历 PATH。
///
/// **这个判断必须和实际 spawn 用同一个环境**，所以只能在守护进程里调用——
/// 界面进程的 PATH 可能不一样，那会导致「菜单说能用，一开就失败」。
/// 真正的实现按平台分在 `sys::fs`——「能执行」这件事两个系统的说法差得远：
/// Unix 看权限位，Windows 看扩展名，而且用户敲的 `claude` 和磁盘上的
/// `claude.cmd` 根本不是同一个字符串。这个名字留在这里不动，是因为
/// `status_of` 和一串测试都按它的形状写的。
pub fn command_exists(cmd: &str) -> bool {
    crate::sys::fs::command_exists(cmd)
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
    fn command_exists_finds_a_real_command_and_not_a_made_up_name() {
        // 不写死 `sh`：Windows 上没有它。当前测试二进制一定在 PATH 之外，
        // 所以这里问的是「本机的登录 shell」——那是两个平台上都一定存在、
        // 而且一定叫得出名字的东西。
        let shell = crate::sys::shell::login_shell();
        assert!(command_exists(&shell), "本机的登录 shell 该找得到：{shell}");
        assert!(!command_exists("dct-绝对没有这个命令-x9"));
    }

    #[test]
    fn command_exists_handles_absolute_paths() {
        // 当前测试二进制：绝对路径、一定存在、一定可执行，两个平台通用。
        let me = std::env::current_exe().unwrap();
        assert!(command_exists(&me.to_string_lossy()));
        assert!(!command_exists(
            &me.with_file_name("根本没有这个").to_string_lossy()
        ));
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

    /// 「命令行」这一项必须指着本机真的能起来的那个 shell。写死 `/bin/zsh`
    /// 的那一版在 Ubuntu 上整行是灰的（Windows 走 WSL 之后这就是默认发行版），
    /// 而它是唯一 `is_agent = false` 的内置项——它一灰，在不是 git 仓库的
    /// 目录里九项就一项都开不了，dct 整个看上去是坏的。
    #[test]
    fn the_shell_profile_points_at_a_shell_that_exists_here() {
        let p = Profile::builtin("shell").unwrap();
        let cmd = &p.command[0];
        assert!(command_exists(cmd), "命令行指向 {cmd}，本机起不来");
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

    /// 不挑某个内置 profile 来测：内置的用 idle 还是 busy 特征是它自己的事
    /// （claude 系就从 `idle_pattern` 换到了 `busy_pattern`），这条只问
    /// 「声明了 idle_pattern 的 profile，那条正则编得过、匹得上」。
    #[test]
    fn idle_regex_compiles() {
        let p = Profile::from_toml("name = \"x\"\ncommand = [\"x\"]\nidle_pattern = \"READY$\"\n")
            .unwrap();
        let re = p.idle_regex().unwrap().unwrap();
        assert!(re.is_match("  READY"));
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

    /// 声明了 `error_pattern` 的内置 profile，那条正则必须编得过——
    /// 编不过的话，会话建起来直接失败（`create()` 会 `?` 掉它），
    /// 而用户什么都没做错。
    #[test]
    fn every_builtin_error_pattern_compiles() {
        for p in Profile::builtins() {
            assert!(
                p.error_regex().is_ok(),
                "{} 的 error_pattern 不是合法正则",
                p.name
            );
        }
    }

    /// claude 系那几个的 `command` 就是 `claude`，界面完全一样，所以
    /// 错误文案也一样。漏掉任何一个，那个 agent 就会静默地坏着。
    #[test]
    fn every_claude_based_profile_detects_the_same_errors() {
        for name in ["dc", "claude", "kimi", "glm", "deepseek", "qwen-api"] {
            let p = Profile::builtin(name).unwrap();
            let re = p
                .error_regex()
                .unwrap()
                .unwrap_or_else(|| panic!("{name} 应当声明 error_pattern"));
            assert!(
                re.is_match("API Error: Connection closed mid-response."),
                "{name} 认不出用户实际撞到的那句话"
            );
        }
    }

    /// **没见过错误文案的 agent 一条都不许编。** 凭想象写正则会造出误报，
    /// 而误报比不报更糟：好端端的会话被标成失败，用户跑去看一个没出事的
    /// 东西，然后就不再相信这个标记了。
    #[test]
    fn profiles_with_unknown_error_text_declare_nothing() {
        for name in ["codex", "opencode", "qwen", "shell"] {
            assert!(
                Profile::builtin(name).unwrap().error_pattern.is_none(),
                "{name} 的错误文案还没见过实物，不该凭空编一条"
            );
        }
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
    /// agent 失败时屏幕上那句话。没写 `error_pattern` 的 profile
    /// 这个功能就是关着的，行为跟改之前完全一样。
    #[test]
    fn parses_and_compiles_the_error_pattern() {
        let p =
            Profile::from_toml("name = \"x\"\ncommand = [\"x\"]\nerror_pattern = \"API Error\"\n")
                .unwrap();
        assert_eq!(p.error_pattern.as_deref(), Some("API Error"));
        let re = p.error_regex().unwrap().unwrap();
        assert!(re.is_match("API Error: Connection closed mid-response"));
        assert!(!re.is_match("一切正常"));
    }

    #[test]
    fn a_profile_without_an_error_pattern_has_no_error_regex() {
        let p = Profile::from_toml("name = \"x\"\ncommand = [\"x\"]\n").unwrap();
        assert!(p.error_pattern.is_none());
        assert!(p.error_regex().unwrap().is_none());
    }

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
                "{name}: 必须有中文 label，十个选项摆在非程序员面前没说明等于没得选"
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
                "dc", "claude", "codex", "opencode", "qwen", "kimi", "glm", "deepseek", "qwen-api",
                "shell",
            ],
            "第一项是新学生按下去就能跑的那个——理由写在 builtin_names 上"
        );
    }

    #[test]
    fn api_shaped_profiles_run_claude_and_need_a_secret() {
        for name in ["dc", "kimi", "glm", "deepseek", "qwen-api"] {
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

    /// **每个我们知道绕过参数的 agent，profile 里必须真的带着它。**
    ///
    /// 这不是风格问题。dct 敢让 agent 关掉所有权限确认，靠的是每轮之前那张
    /// 隐藏快照——「它从来不问这样可以吗」是产品承诺的一半，另一半是「反悔
    /// 只要一个键」。漏掉这个参数，agent 会在动第一个文件时停下来等人点头，
    /// 而看板上只会显示成「在干活」，用户要进到会话里才发现它其实在等他。
    ///
    /// `qwen` 就这么漏了很久（`command = ["qwen"]`），因为它一次都没真跑过。
    /// 这条测试是为了让下一次「顺手编辑一下 profile」不会又把它删掉。
    ///
    /// **只列我们实测过的。** `opencode` 不在这儿——它的绕过参数叫什么还没人
    /// 验过，凭想象填一个进去只会让这条测试变成谎话。
    #[test]
    fn every_agent_we_know_the_flag_for_actually_carries_it() {
        for (name, flag) in [
            ("dc", "--dangerously-skip-permissions"),
            ("claude", "--dangerously-skip-permissions"),
            ("codex", "--dangerously-bypass-approvals-and-sandbox"),
            ("qwen", "--approval-mode=yolo"),
        ] {
            let p = Profile::builtin(name).unwrap();
            assert!(
                p.command.iter().any(|a| a == flag),
                "{name} 的 command 里必须有 {flag}，否则它会停下来问用户：{:?}",
                p.command
            );
        }
    }

    /// opencode 是同一条规矩的另一个形状：它的权限**不在命令行参数上**，
    /// 而在 `OPENCODE_PERMISSION` 这个环境变量里，值是一段 JSON。
    ///
    /// 所以这条测试真的把那段 JSON parse 一遍，而不是看它非空就算数。
    /// 理由在 opencode 自己的代码里：JSON 解析失败时它 **catch 住、记一条
    /// 只有 `--print-logs` 才看得见的警告、然后继续**。少一个引号的后果
    /// 因此不是「起不来」，而是 agent 安安静静地又开始逐条问用户——
    /// 而看板上它显示成「在干活」，没有任何东西会提示你。
    ///
    /// 顺带钉住「没有一项是 ask」：只改一半（比如把 bash 留成 ask）同样
    /// 会让会话卡住，而那种半吊子编辑正是最可能发生的。
    #[test]
    fn opencodes_permissions_are_real_json_and_all_of_them_are_allow() {
        let p = Profile::builtin("opencode").unwrap();
        let raw = p
            .env
            .get("OPENCODE_PERMISSION")
            .expect("opencode 没有命令行开关，权限全靠这个环境变量");

        let parsed: serde_json::Value = serde_json::from_str(raw)
            .expect("这段 JSON 必须是合法的——写错了 opencode 会静默忽略它");
        let obj = parsed.as_object().expect("得是一个对象");

        // 键名来自 opencode 1.18.23 二进制里那份清单，不是猜的。漏掉一个
        // 不会报错，只会在某一次真的用到它时停下来。
        for action in [
            "bash",
            "read",
            "edit",
            "glob",
            "grep",
            "webfetch",
            "task",
            "todowrite",
            "websearch",
            "lsp",
            "skill",
        ] {
            assert_eq!(
                obj.get(action).and_then(|v| v.as_str()),
                Some("allow"),
                "{action} 没设成 allow，agent 用到它的时候会停下来等人点头"
            );
        }
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
        assert_eq!(all[0].name, "dc", "内置顺序不受影响");
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
        // 跟着 `builtin_names()` 走，不写死数字：加一个内置 profile 是
        // 正常的事，为它改一个跟这条测试要断言的东西无关的数字不是。
        assert_eq!(all.len(), Profile::builtin_names().len(), "只有内置");
    }

    #[test]
    fn non_toml_files_are_ignored() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("README.md"), "随手放的笔记").unwrap();
        let (_, errs) = all_profiles(tmp.path());
        assert!(errs.is_empty(), "非 .toml 文件直接跳过，不该报错");
    }

    #[test]
    fn claude_and_codex_declare_a_headless_command() {
        // 这两个是本机实测过的：`claude -p` 和 `codex exec`。
        for name in ["claude", "codex"] {
            let p = Profile::builtin(name).unwrap();
            let h = p
                .headless
                .as_ref()
                .unwrap_or_else(|| panic!("{name}: 要有 [headless]"));
            assert!(!h.command.is_empty(), "{name}: headless 命令不能为空");
        }
        assert_eq!(
            Profile::builtin("claude")
                .unwrap()
                .headless
                .unwrap()
                .command,
            vec!["claude".to_string(), "-p".to_string()]
        );
        assert_eq!(
            Profile::builtin("codex").unwrap().headless.unwrap().command,
            vec!["codex".to_string(), "exec".to_string()]
        );
    }

    /// **只有非交互模式被真的跑过的 CLI 才许写 `[headless]`。**
    ///
    /// - opencode / qwen 本机没装，无界面模式没验过。
    /// - kimi / glm / deepseek / qwen-api 一样没验过，而且它们比前两个更危险：
    ///   它们的 `[headless]` 曾经写着 `claude -p`，配上自己的
    ///   `[env] ANTHROPIC_BASE_URL`（moonshot / bigmodel / deepseek /
    ///   dashscope），等于起一个 `claude` 去打第三方端点，而那条路上没有
    ///   任何地方注入过厂商密钥——`claude` 只好拿用户 Keychain 里的
    ///   Anthropic 登录态去认证，**把 A 家的凭据发给 B 家的服务器**。
    ///   这四个是 API 密钥形态的厂商，它们的正路是 `[api]` + HTTP 直连。
    ///
    /// 编一个没验过的 `[headless]` 出来 = 造一条用户一走就坏的路，
    /// 和「没验过就不填 pattern」是同一条纪律。不要把这些块加回去。
    #[test]
    fn unverified_clis_declare_no_headless_command() {
        for name in ["opencode", "qwen", "kimi", "glm", "deepseek", "qwen-api"] {
            let p = Profile::builtin(name).unwrap();
            assert!(
                p.headless.is_none(),
                "{name}: 没有实测过非交互模式就不许写 [headless]——\
                 编出来的那条路一走就坏，而且这几个 API 形态的 profile 还会\
                 让子进程去借另一家的登录态。它们的正路是 [api] + 直连。"
            );
        }
    }

    #[test]
    fn api_shaped_profiles_declare_an_api_block() {
        for name in ["dc", "kimi", "glm", "deepseek", "qwen-api"] {
            let p = Profile::builtin(name).unwrap();
            let api = p
                .api
                .as_ref()
                .unwrap_or_else(|| panic!("{name}: 要有 [api]"));
            assert!(
                api.base_url.starts_with("https://"),
                "{name}: base_url 要是 https"
            );
            assert_eq!(
                api.wire,
                Wire::Anthropic,
                "{name}: 这四个都是 Anthropic 兼容形态"
            );
        }
    }

    #[test]
    fn the_api_base_url_matches_the_env_base_url() {
        // 两个字段现在值相同但用途不同（env 给子进程，api 给 dct 自己）。
        // 不合并，但要一致——不一致意味着有人只改了一边。
        for name in ["dc", "kimi", "glm", "deepseek", "qwen-api"] {
            let p = Profile::builtin(name).unwrap();
            let env = p.env.get("ANTHROPIC_BASE_URL").unwrap();
            assert_eq!(
                &p.api.as_ref().unwrap().base_url,
                env,
                "{name}: 两处 base_url 不一致"
            );
        }
    }

    #[test]
    fn a_profile_without_the_new_blocks_still_parses() {
        // 用户手写的老 profile 不能因为加了新字段就读不了。
        let p = Profile::from_toml("name = \"x\"\ncommand = [\"x\"]\n").unwrap();
        assert!(p.headless.is_none());
        assert!(p.api.is_none());
    }

    /// 跑 claude 二进制的五个 profile 都该带 `--continue`——它们重启后要
    /// 靠这个接回原来的对话。
    #[test]
    fn claude_based_profiles_declare_continue_as_resume_args() {
        for name in ["dc", "claude", "deepseek", "glm", "kimi", "qwen-api"] {
            let p = Profile::builtin(name).unwrap();
            assert_eq!(
                p.resume_args,
                vec!["--continue".to_string()],
                "{name}: 应当声明 resume_args = [\"--continue\"]"
            );
        }
    }

    /// **没实测过恢复方式的四个 profile 一条参数都不许编。** 编一个出来，
    /// 用户会以为自己接回了原来的对话，实际却是一个安安静静的新会话——
    /// 这比老实告诉他「这个重开了」更糟。
    #[test]
    fn unverified_profiles_declare_no_resume_args() {
        for name in ["codex", "opencode", "qwen", "shell"] {
            let p = Profile::builtin(name).unwrap();
            assert!(
                p.resume_args.is_empty(),
                "{name}: 恢复方式没实测过，不该编 resume_args"
            );
        }
    }

    fn entry(name: &str) -> crate::proto::ProfileEntry {
        crate::proto::ProfileEntry {
            name: name.to_string(),
            label: name.to_string(),
            note: String::new(),
            status: ProfileStatus::Ready,
            secret: None,
            install: None,
            has_secret: false,
            backend_only: false,
        }
    }

    fn names(v: &[crate::proto::ProfileEntry]) -> Vec<&str> {
        v.iter().map(|e| e.name.as_str()).collect()
    }

    /// 绝大多数用户没写 `[menu]`，他们要看到全部十项。
    #[test]
    fn an_empty_menu_list_keeps_everything() {
        let all = vec![entry("dc"), entry("claude"), entry("shell")];
        assert_eq!(names(&trim_menu(all, &[])), vec!["dc", "claude", "shell"]);
    }

    #[test]
    fn the_menu_list_decides_both_which_and_in_what_order() {
        let all = vec![entry("dc"), entry("claude"), entry("shell")];
        let only = vec!["shell".to_string(), "dc".to_string()];
        assert_eq!(
            names(&trim_menu(all, &only)),
            vec!["shell", "dc"],
            "顺序照清单写的来，不是照内置顺序"
        );
    }

    /// 名字写错了就当没写这一项——发这份配置的人自己会发现少了一项，
    /// 而收到机器的学生对着一条「名字拼错了」的警告什么也做不了。
    #[test]
    fn a_name_that_matches_nothing_is_skipped() {
        let all = vec![entry("dc"), entry("shell")];
        let only = vec!["dc".to_string(), "typo".to_string()];
        assert_eq!(names(&trim_menu(all, &only)), vec!["dc"]);
    }

    /// **裁到一个不剩就不裁。** 空菜单是一个什么都开不了、也不说为什么的
    /// 界面，比多几项用不上的东西糟得多。宁可多显示，不可无路可走。
    #[test]
    fn a_list_that_matches_nothing_falls_back_to_the_whole_menu() {
        let all = vec![entry("dc"), entry("shell")];
        let only = vec!["nope".to_string()];
        assert_eq!(names(&trim_menu(all, &only)), vec!["dc", "shell"]);
    }
}
