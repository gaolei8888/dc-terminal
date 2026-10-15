//! `~/.dct/config.toml`。目前只有 `[llm]` 一段。
//!
//! **配置坏了绝不能让 dct 起不来。** LLM 是增强，不是地基——解析失败
//! 一律退回默认值，只往 stderr 留一行痕迹。
//!
//! **`llm` 是 `Option`，不是默认开着的 `LlmConfig`。这不是随手选的类型，
//! 是隐私边界。** 出错解释功能（`session::explain_prompt`）会把一个失败
//! 会话屏幕上最后 2000 个字符原样送给配置里指定的模型——而那正是
//! `Invalid API key: sk-ant-...`、`Authorization: Bearer ...`、`.env` 内容、
//! 带 token 的 git 地址最容易出现的地方。**把这功能打开必须是用户的一次
//! 主动动作**，不能因为「用户什么都没配」就替他默认打开、把他终端里的东西
//! 发给第三方。文件里没有 `[llm]` 这一段（不存在、内容为空、这一段没写、
//! 甚至整份文件解析坏了）一律落在 `None` 上——功能整个关着，`daemon.rs`
//! 连 `resolve()` 都不会调用。用户显式写下 `[llm]`（哪怕后面什么都不填）
//! 才算「我要开」。**但「我要开」不等于「我选好了哪一家」**：`provider`
//! 没有默认值（以前默认 `claude`——那是替用户挑了一家厂商）。`[llm]` 里没写
//! `provider` 就是还没配好：解析照常成功，连接那一层报 `NoProvider`，功能关着，
//! 界面警告栏 / `dct llm check` / `dct answer` / `dct do` 各给一句同样的提示，
//! 教用户写 `provider = "<profile 名>"`。`transport` 默认仍是 `Cli`——它不是
//! 厂商相关的（cli 走的是被点名那个 profile 自己的 `[headless]` 命令）。
//!
//! 一个不点任何厂商名的 OpenAI 兼容端点这样写（地址只是例子）：
//! ```toml
//! [llm]
//! provider  = "gateway"             # ~/.dct/profiles/gateway.toml 的 name，里面有 [api] 段
//! transport = "http"                # [api] 里写 base_url（例：http://127.0.0.1:11435）和 wire = "openai"
//! model     = "<模型名>"            # http 必填
//! ```
//! 完整例子见 README；`llm::resolve` 的测试钉着这套写法能走通。
//!
//! **2026-09-02 起多了第二条打开它的路**：配对屏上那个勾选框
//! （`ui::pair_view` 画它、`ui::view::LlmOptIn` 存它、`llm_optin::enable`
//! 落它的盘）。它**默认勾着**，这一点不违反上面那条边界，因为它同时满足
//! 边界真正要求的东西：那一行字就印在学生正盯着的配对屏上，说的是代价
//! （「会把终端上的报错原文发给训练营网关」）而不是好处，按 `l` 就能取消
//! 勾选，而且要等他在浏览器里点了确认，这一勾才会变成磁盘上的 `[llm]`。
//! 边界从来不是「默认值必须是关」，是「不能在没有人看见代价的情况下打开」。
//!
//! **配对绝不覆盖一份已经有 `[llm]` 的文件。** `llm_optin::enable` 见到那
//! 一段就原样退出，配对屏那一侧也不摆勾选框、改成说一句「已经有了，这次
//! 不动它」（`LlmOptIn::AlreadyConfigured`）——用户当初为什么那么写，
//! 配对流程无从知道，也就没有资格替他改。

use serde::Deserialize;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Transport {
    /// 把 provider 的 CLI 用无界面模式拉起来。认证是那个 CLI 自己的事。
    Cli,
    /// 直接打 HTTP 端点。需要凭据。
    Http,
}

fn default_transport() -> Transport {
    Transport::Cli
}

#[derive(Debug, Clone, Deserialize)]
pub struct LlmConfig {
    /// 用哪个 profile 的名字（内置的或 `~/.dct/profiles/` 里自己写的）。
    /// **没有默认值**：不写 = 没配，`resolve` 报 `ResolveError::NoProvider`，
    /// 功能保持关着，dct 其余部分不受影响。dct 不替用户选一家厂商。
    #[serde(default)]
    pub provider: Option<String>,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub base_url: Option<String>,
    #[serde(default = "default_transport")]
    pub transport: Transport,
}

impl Default for LlmConfig {
    fn default() -> Self {
        LlmConfig {
            provider: None,
            model: None,
            base_url: None,
            transport: default_transport(),
        }
    }
}

fn default_judge_backend() -> String {
    "jev".to_string()
}
fn default_judge_model() -> String {
    "jeff-latest".to_string()
}
fn default_threshold() -> f64 {
    0.8
}
fn default_judge_timeout_ms() -> u64 {
    5000
}

/// `[judge]`：给 dct 自己做决定用的「选哪个」引擎。没写 = 关。
#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct JudgeConfig {
    /// 现在只有 "jev"：POST {base_url}/v1/systemone。
    #[serde(default = "default_judge_backend")]
    pub backend: String,
    /// 例：http://127.0.0.1:8765
    pub base_url: String,
    /// jeff-serve 认 "jeff-latest"。
    #[serde(default = "default_judge_model")]
    pub model: String,
    /// 有就作为 Bearer 发（只发给 base_url 那台主机）。值从这个环境变量读，
    /// 不写进配置文件。
    #[serde(default)]
    pub api_key_env: Option<String>,
    /// Jeff 答案的门槛，起始 0.8。
    #[serde(default = "default_threshold")]
    pub threshold: f64,
    #[serde(default = "default_judge_timeout_ms")]
    pub timeout_ms: u64,
    /// `dct answer` 的危险闸拦哪几档：content / critical / money（不分大小写），没写 = 三档全拦。
    /// critical 和 money 必须在，只有 content 可以不写；检查在 `answer::gate_tiers_from_config`。
    /// 这里只收字符串：写错不会让别的读 `[judge]` 的命令跟着读不了配置。
    #[serde(default)]
    pub gate_tiers: Option<Vec<String>>,
    /// `[judge.serve]`：怎么在本机把 judge 服务拉起来。没写 = dct 什么都不启动、不停止。
    #[serde(default)]
    pub serve: Option<ServeConfig>,
}

fn default_ready_timeout_secs() -> u64 {
    60
}
fn default_min_free_percent() -> u8 {
    15
}

/// `[judge.serve]`：dct 替你启停本机 judge 服务。全部可选；`command` 不写就什么都不做。
/// **这里不要放密钥**：argv 和 env 其他用户用 `ps` 就能看见。
/// `idle_stop_minutes` 还没做：写了会被忽略。
#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct ServeConfig {
    /// argv 数组，不经 shell：`["<可执行文件>", "参数"...]`。
    #[serde(default)]
    pub command: Vec<String>,
    /// 额外的环境变量（字符串）。
    #[serde(default)]
    pub env: std::collections::BTreeMap<String, String>,
    /// 等服务答 `GET {base_url}/health` 的上限（秒）。
    #[serde(default = "default_ready_timeout_secs")]
    pub ready_timeout_secs: u64,
    /// true = `dct answer` 发现 judge 连不上时自动启动一次。默认 false。
    #[serde(default)]
    pub autostart: bool,
    /// 空闲内存低于这个百分比就不启动（服务要占几 GB）。
    #[serde(default = "default_min_free_percent")]
    pub min_free_percent: u8,
}

/// 菜单上只留哪几项。给一个班、一个团队统一发机器的人用的。
///
/// 空列表（没写这一段、写了空的、名字全填错）= 不裁，全都显示。**裁到
/// 一个不剩时也是全都显示**——理由见 `profile::trim_menu`。
#[derive(Debug, Clone, Default, Deserialize)]
pub struct MenuConfig {
    /// profile 名，**按写下的顺序**显示。写一个不存在的名字不报错也不显示，
    /// 因为这份文件是发机器的人自己写的，而收到机器的学生对着一条
    /// 「profile 名写错了」的警告什么也做不了。
    #[serde(default)]
    pub agents: Vec<String>,
}

/// `[mesh]`：多电脑那一路。
#[derive(Debug, Clone, Default, Deserialize)]
pub struct MeshConfig {
    /// 中转地址；没写就是 `mesh::DEFAULT_RELAY`。
    #[serde(default)]
    pub relay: Option<String>,
}

impl MeshConfig {
    pub fn relay(&self) -> String {
        self.relay
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .unwrap_or(crate::mesh::DEFAULT_RELAY)
            .to_string()
    }
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct Config {
    /// `None` = 用户没写 `[llm]` 这一段，出错解释功能整个关着。**默认值
    /// 就是 `None`**（`Option` 自己的 `Default`，这里不额外提供
    /// `#[serde(default = "...")]` 之类的东西去猜一个「开」的初值）——见
    /// 模块头注释，这是隐私边界，不是随手选的类型。
    #[serde(default)]
    pub llm: Option<LlmConfig>,
    /// `[menu]` 跟 `[llm]` 不一样，**不是 `Option`**：它不碰任何隐私边界，
    /// 「没写」和「写了但空着」要的是同一件事——菜单原样显示。
    #[serde(default)]
    pub menu: MenuConfig,
    #[serde(default)]
    pub mesh: MeshConfig,
    /// `None` = 没写 `[judge]`，judge 关着。
    #[serde(default)]
    pub judge: Option<JudgeConfig>,
}

impl Config {
    pub fn from_toml(s: &str) -> anyhow::Result<Config> {
        Ok(toml::from_str(s)?)
    }

    /// 读不到、解析不了，一律是默认值——也就是 `llm: None`，功能关着。
    /// 见模块头注释：配置坏了不该让 dct 起不来，更不该坏一下就把「发终端
    /// 内容给第三方」这种功能悄悄打开。
    pub fn load(path: &Path) -> Config {
        let src = match std::fs::read_to_string(path) {
            Ok(s) => s,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Config::default(),
            Err(e) => {
                eprintln!("配置读取失败（{}）：{e}", path.display());
                return Config::default();
            }
        };
        match Config::from_toml(&src) {
            Ok(c) => c,
            Err(e) => {
                eprintln!("配置解析失败（{}）：{e}", path.display());
                Config::default()
            }
        }
    }
}

/// 跟着 socket 走，测试自动隔离（同 `secrets_path_for_socket`）。
pub fn config_path_for_socket(socket: &Path) -> PathBuf {
    match socket.parent() {
        Some(d) => d.join("config.toml"),
        None => PathBuf::from("config.toml"),
    }
}

/// 同一份文件，从 profiles 目录反推。
///
/// `daemon::handle` 拿不到 socket——它只拿得到 profiles 目录——而「菜单裁不裁」
/// 是每次开选择器都要问一遍的事（发机器的人改完配置，学生不该为了让它生效
/// 去重启守护进程）。两个函数指向的必须是同一个文件：`profiles` 是 socket
/// 同级目录下的子目录，所以它的父目录就是 socket 的父目录。改任何一边都要
/// 回来看另一边，下面那条测试钉着这件事。
pub fn config_path_for_profiles_dir(profiles_dir: &Path) -> PathBuf {
    match profiles_dir.parent() {
        Some(d) => d.join("config.toml"),
        None => PathBuf::from("config.toml"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn judge_is_off_unless_written_and_has_defaults() {
        assert!(Config::from_toml("").unwrap().judge.is_none());
        let c = Config::from_toml("[judge]\nbase_url = \"http://127.0.0.1:8765\"\n").unwrap();
        let j = c.judge.unwrap();
        assert_eq!(
            (
                j.backend.as_str(),
                j.model.as_str(),
                j.threshold,
                j.timeout_ms
            ),
            ("jev", "jeff-latest", 0.8, 5000)
        );
    }

    #[test]
    fn the_relay_defaults_and_can_be_overridden() {
        assert_eq!(Config::default().mesh.relay(), crate::mesh::DEFAULT_RELAY);
        let c = Config::from_toml("[mesh]\nrelay = \"http://127.0.0.1:8787\"\n").unwrap();
        assert_eq!(c.mesh.relay(), "http://127.0.0.1:8787");
        let blank = Config::from_toml("[mesh]\nrelay = \"  \"\n").unwrap();
        assert_eq!(blank.mesh.relay(), crate::mesh::DEFAULT_RELAY);
        assert!(c.llm.is_none(), "写了 [mesh] 不该顺手打开 [llm]");
    }

    #[test]
    fn parses_a_full_llm_section() {
        let c = Config::from_toml(
            r#"
            [llm]
            provider = "kimi"
            model = "kimi-k2"
            base_url = "https://example.test/v1"
            transport = "http"
            "#,
        )
        .unwrap();
        let llm = c.llm.expect("写了 [llm] 就该是 Some");
        assert_eq!(llm.provider.as_deref(), Some("kimi"));
        assert_eq!(llm.model.as_deref(), Some("kimi-k2"));
        assert_eq!(llm.base_url.as_deref(), Some("https://example.test/v1"));
        assert_eq!(llm.transport, Transport::Http);
    }

    /// 隐私边界的核心断言：完全没有 `[llm]` 这一段（哪怕文件里有别的内容）
    /// 就是关着，不能悄悄补出一份「默认」配置来。**这不是「凑合能跑」的
    /// 退路，是「没让开就不能开」的正确行为**——见模块头注释。
    #[test]
    fn a_file_without_an_llm_section_means_the_feature_is_off() {
        let c = Config::from_toml("# 这份文件目前没有 [llm]，就是这么回事\n").unwrap();
        assert!(
            c.llm.is_none(),
            "没写 [llm] 就该是关着的，不能给它编一份默认配置"
        );
    }

    /// 空文件是「没写 [llm]」的极端情形，同一条规矩。
    #[test]
    fn an_empty_file_means_the_feature_is_off() {
        let c = Config::from_toml("").unwrap();
        assert!(c.llm.is_none(), "空文件等于什么都没配，功能该是关着的");
    }

    /// 写了 `[llm]` 却没写 `provider`：解析照常成功（不能因此让整份配置读不了），
    /// 但**不替用户挑一家厂商**——provider 是 `None`，连接那一层据此报「没写」。
    /// `transport` 不是厂商相关的（cli 走的是被点名那个 profile 自己的 `[headless]`
    /// 命令），默认值照旧。
    #[test]
    fn a_bare_llm_section_does_not_pick_a_vendor() {
        let c = Config::from_toml("[llm]\n").unwrap();
        let llm = c.llm.expect("写了 [llm]，哪怕是空的，也该是 Some");
        assert!(llm.provider.is_none(), "没写 provider 就不能替他选一个");
        assert_eq!(llm.transport, Transport::Cli);
        assert!(llm.model.is_none());
        assert!(llm.base_url.is_none());
    }

    /// 只写了别的字段（这里是一个 OpenAI 兼容端点的整套）也一样：没有 provider
    /// 就是没有，不会因为别的字段齐了就补上一个。
    #[test]
    fn other_fields_do_not_imply_a_provider() {
        let c = Config::from_toml(
            "[llm]\ntransport = \"http\"\nbase_url = \"http://127.0.0.1:11435\"\nmodel = \"m\"\n",
        )
        .unwrap();
        assert!(c.llm.unwrap().provider.is_none());
    }

    /// 显式写下 provider（包括 claude）：跟以前一模一样。
    #[test]
    fn an_explicit_provider_is_kept_verbatim() {
        let c = Config::from_toml("[llm]\nprovider = \"claude\"\ntransport = \"cli\"\n").unwrap();
        let llm = c.llm.unwrap();
        assert_eq!(llm.provider.as_deref(), Some("claude"));
        assert_eq!(llm.transport, Transport::Cli);
    }

    #[test]
    fn a_partial_llm_section_keeps_the_other_defaults() {
        let c = Config::from_toml("[llm]\nprovider = \"codex\"\n").unwrap();
        let llm = c.llm.unwrap();
        assert_eq!(llm.provider.as_deref(), Some("codex"));
        assert_eq!(llm.transport, Transport::Cli);
    }

    /// 没有配置文件的用户是绝大多数用户——这条测试原来断言「默认值」，
    /// 现在断言的是「关着」：没配就是没开，不是替他挑了一套默认厂商。
    #[test]
    fn a_missing_file_means_the_feature_is_off() {
        let dir = tempfile::tempdir().unwrap();
        let c = Config::load(&dir.path().join("nope.toml"));
        assert!(c.llm.is_none(), "没有配置文件，功能该是关着的");
    }

    #[test]
    fn a_broken_file_falls_back_to_off_and_does_not_panic() {
        // 配置坏了不该让 dct 起不来——LLM 是增强，不是地基。也不该因为
        // 文件坏了就把「发终端内容给第三方」这种功能悄悄打开。
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("config.toml");
        std::fs::write(&p, "[llm\nprovider =").unwrap();
        let c = Config::load(&p);
        assert!(
            c.llm.is_none(),
            "配置解析失败就该退回关着，不是猜一份默认值"
        );
    }

    #[test]
    fn config_path_sits_next_to_the_socket() {
        let p = config_path_for_socket(std::path::Path::new("/home/x/.dct/daemon.sock"));
        assert_eq!(p, std::path::PathBuf::from("/home/x/.dct/config.toml"));
    }

    /// 两条推导必须落在同一个文件上——落不到一起的话，发机器的人写下的
    /// `[menu]` 会静静地不生效，而他手里没有任何线索能查出为什么。
    #[test]
    fn both_paths_point_at_the_same_file() {
        let sock = std::path::Path::new("/home/x/.dct/daemon.sock");
        let profiles = crate::profile::profiles_dir_for_socket(sock);
        assert_eq!(
            config_path_for_profiles_dir(&profiles),
            config_path_for_socket(sock)
        );
    }

    #[test]
    fn parses_a_menu_section() {
        let c = Config::from_toml("[menu]\nagents = [\"dc\", \"shell\"]\n").unwrap();
        assert_eq!(c.menu.agents, vec!["dc".to_string(), "shell".to_string()]);
    }

    /// 绝大多数用户没有这一段，那必须等于「菜单原样显示」。
    #[test]
    fn no_menu_section_means_no_trimming() {
        assert!(Config::default().menu.agents.is_empty());
        assert!(Config::from_toml("[llm]\n").unwrap().menu.agents.is_empty());
    }
}
