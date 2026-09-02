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
//! 才算「我要开」，那一刻开始，段内每个字段该有什么默认值（`provider`
//! 默认 `claude`、`transport` 默认 `Cli`）还是照旧——那些默认值只回答
//! 「开了之后怎么配」，不回答「要不要开」。
//!
//! **2026-09-02 起多了第二条打开它的路**：配对屏上那个默认勾上的勾选框
//! （`llm_optin::enable`）。边界没变——仍然要一个人当面看着「会把报错原文
//! 发给训练营网关」这句话点头，只是那个人现在可能是在配对流程里点的。

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

fn default_provider() -> String {
    "claude".to_string()
}

fn default_transport() -> Transport {
    Transport::Cli
}

#[derive(Debug, Clone, Deserialize)]
pub struct LlmConfig {
    #[serde(default = "default_provider")]
    pub provider: String,
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
            provider: default_provider(),
            model: None,
            base_url: None,
            transport: default_transport(),
        }
    }
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
        assert_eq!(llm.provider, "kimi");
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

    /// 用户显式写下 `[llm]`（哪怕一个字段都不填）就是「我要开」——这一刻
    /// 开始，段内默认值（provider claude、transport Cli）照旧生效。
    /// 「要不要开」和「开了怎么配」是两件事，这条测试钉住第二件事没有
    /// 因为第一件事变成 Option 而跟着跑偏。
    #[test]
    fn a_bare_llm_section_opts_in_with_the_usual_defaults() {
        let c = Config::from_toml("[llm]\n").unwrap();
        let llm = c.llm.expect("写了 [llm]，哪怕是空的，也该是 Some");
        assert_eq!(llm.provider, "claude", "默认用最可能已登录的 CLI");
        assert_eq!(llm.transport, Transport::Cli, "默认走不需要凭据的那条路");
        assert!(llm.model.is_none());
        assert!(llm.base_url.is_none());
    }

    #[test]
    fn a_partial_llm_section_keeps_the_other_defaults() {
        let c = Config::from_toml("[llm]\nprovider = \"codex\"\n").unwrap();
        let llm = c.llm.unwrap();
        assert_eq!(llm.provider, "codex");
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
