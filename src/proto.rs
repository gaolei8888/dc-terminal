use serde::{Deserialize, Serialize};
use std::path::PathBuf;

use crate::git::FileStat;
use crate::profile::ProfileStatus;
use crate::pty::ScreenSpan;
use crate::session::{SessionInfo, SessionState};

/// 需要密钥时，UI 画输入界面要用的东西。
///
/// 只带**已经取好语言**的字符串，不把 `LocalizedText` 送过线：组句发生在哪
/// 一侧必须一致——如果协议里同时存在「原始多语言表」和「已选定语言的字符串」，
/// 迟早会有一条路径读错表，界面上冒出一句英文夹在中文里。daemon 端已经知道
/// 用户语言（目前只有 `Lang::Zh`），选定的动作只在那一侧发生一次。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SecretPrompt {
    pub hint: String,
    pub url: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InstallPrompt {
    pub command: Vec<String>,
    pub note: String,
}

/// 菜单上一行的完整信息：UI 拿到它就能画出「名字 + 说明 + 能不能用 +
/// 需要的话怎么补」，不用再回头去问 daemon 第二遍。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProfileEntry {
    pub name: String,
    pub label: String,
    pub note: String,
    pub status: ProfileStatus,
    pub secret: Option<SecretPrompt>,
    pub install: Option<InstallPrompt>,
    /// 密钥仓里现在是不是真有这个 profile 的密钥。跟 `status` 分开存是因为
    /// `status_of` 里「装没装排在密钥前面」（见 profile.rs），一个 CLI 没装的
    /// profile 不管密钥填没填都会报 `NeedsDependency`/`NotInstalled`——从
    /// `status` 反推不出真实的密钥状态。密钥设置页要的是这个事实本身，
    /// 不是「现在能不能开会话」，两者在这种情况下会给出不同答案。
    pub has_secret: bool,
}

/// 九宫格一格的内容。跟 `Response::Screen` 不同，不带光标——
/// 只读的格子画光标只会误导人去打字。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScreenEntry {
    pub id: u32,
    pub lines: Vec<Vec<ScreenSpan>>,
}

#[derive(Serialize, Deserialize)]
pub enum Request {
    List,
    Create {
        dir: String,
        profile: String,
        /// 这次开的会话算不算「用户选的 agent」。「帮你装 CLI」那条路径开的是一个
        /// 跑安装命令的 shell 会话，绝不能被记成上次用的 agent——否则用户下次
        /// 按 `n` 会直接掉进一个命令行，而不是回到他真正想用的那个 agent。
        remember: bool,
    },
    Input {
        id: u32,
        text: String,
    },
    Screen {
        id: u32,
    },
    /// 一次取多个会话的屏幕。九宫格九个格子要是一个个问，
    /// 一问一答的串行连接上就是九个来回。
    Screens {
        ids: Vec<u32>,
    },
    Resize {
        id: u32,
        rows: u16,
        cols: u16,
    },
    Stop {
        id: u32,
    },
    Undo {
        id: u32,
    },
    Diff {
        id: u32,
    },
    /// 带上界面当前的语言。守护进程不知道、也不该知道谁在用什么语言——
    /// 它是常驻的、可能同时服务多个界面的进程，把「当前语言」存成它的状态
    /// 就等于假设只有一个客户端。取哪一份文案由请求方说了算。
    Profiles {
        lang: crate::i18n::Lang,
    },
    Projects,
    SetSecret {
        profile: String,
        value: String,
    },
    DeleteSecret {
        profile: String,
    },
    LastProfile,
    VerifySecret {
        profile: String,
        value: String,
    },
}

/// 手写 `Debug`，不能靠 `derive`——`SetSecret`/`VerifySecret` 两个变体的
/// `value` 是用户的明文密钥。今天没有任何地方真的 `{req:?}` 打印一个
/// `Request`（已核实），但 `serve()` 解析失败那条分支离
/// `eprintln!("bad request: {req:?}")` 只有一行距离——一旦有人图省事加了
/// 这行调试日志，密钥就会写进守护进程的 stderr（很可能重定向进一个存活
/// 比进程本身久得多的日志文件）。这里把 `value` 换成占位符，`profile`
/// 这种不敏感的字段照常打印，排查问题时还能看出是哪个 profile 出的请求。
impl std::fmt::Debug for Request {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Request::List => write!(f, "List"),
            Request::Create {
                dir,
                profile,
                remember,
            } => f
                .debug_struct("Create")
                .field("dir", dir)
                .field("profile", profile)
                .field("remember", remember)
                .finish(),
            Request::Input { id, text } => f
                .debug_struct("Input")
                .field("id", id)
                .field("text", text)
                .finish(),
            Request::Screen { id } => f.debug_struct("Screen").field("id", id).finish(),
            Request::Screens { ids } => f.debug_struct("Screens").field("ids", ids).finish(),
            Request::Resize { id, rows, cols } => f
                .debug_struct("Resize")
                .field("id", id)
                .field("rows", rows)
                .field("cols", cols)
                .finish(),
            Request::Stop { id } => f.debug_struct("Stop").field("id", id).finish(),
            Request::Undo { id } => f.debug_struct("Undo").field("id", id).finish(),
            Request::Diff { id } => f.debug_struct("Diff").field("id", id).finish(),
            Request::Profiles { lang } => f.debug_struct("Profiles").field("lang", lang).finish(),
            Request::Projects => write!(f, "Projects"),
            Request::SetSecret { profile, .. } => f
                .debug_struct("SetSecret")
                .field("profile", profile)
                .field("value", &"<redacted>")
                .finish(),
            Request::DeleteSecret { profile } => f
                .debug_struct("DeleteSecret")
                .field("profile", profile)
                .finish(),
            Request::LastProfile => write!(f, "LastProfile"),
            Request::VerifySecret { profile, .. } => f
                .debug_struct("VerifySecret")
                .field("profile", profile)
                .field("value", &"<redacted>")
                .finish(),
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub enum Response {
    Sessions(Vec<SessionInfo>),
    Created {
        id: u32,
    },
    Screen {
        lines: Vec<Vec<ScreenSpan>>,
        cursor: (u16, u16),
        /// 贴在会话里时界面只调 `Screen`（`List` 太贵，见 `ui::run` 里的注释），
        /// 所以进程死了它只能从这里知道。少了它界面会永远画那张空缓冲——
        /// agent 退出时恢复主屏，主屏从来没被写过，所以「屏是空的」是正常的，
        /// 判断死活只能靠状态。
        state: SessionState,
    },
    Screens {
        screens: Vec<ScreenEntry>,
    },
    Diff(Vec<FileStat>),
    Profiles {
        entries: Vec<ProfileEntry>,
        /// 密钥文件读不了、自定义 profile 写错了之类。UI 顶部红字。
        warning: Option<String>,
    },
    Projects(Vec<String>),
    LastProfile(Option<String>),
    Verify(crate::verify::VerifyOutcome),
    Ok,
    Error(String),
}

pub fn socket_path() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".into());
    PathBuf::from(home).join(".dct").join("daemon.sock")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn debug_redacts_the_secret_on_set_secret() {
        let req = Request::SetSecret {
            profile: "kimi".into(),
            value: "sk-super-secret-value".into(),
        };
        let s = format!("{req:?}");
        assert!(
            !s.contains("sk-super-secret-value"),
            "密钥不能出现在 Debug 输出里：{s}"
        );
        assert!(s.contains("kimi"), "profile 名字留着帮排查：{s}");
    }

    #[test]
    fn debug_redacts_the_secret_on_verify_secret() {
        let req = Request::VerifySecret {
            profile: "glm".into(),
            value: "sk-another-secret-value".into(),
        };
        let s = format!("{req:?}");
        assert!(
            !s.contains("sk-another-secret-value"),
            "密钥不能出现在 Debug 输出里：{s}"
        );
        assert!(s.contains("glm"), "profile 名字留着帮排查：{s}");
    }

    #[test]
    fn screens_request_round_trips() {
        let req = Request::Screens { ids: vec![1, 3, 7] };
        let s = serde_json::to_string(&req).unwrap();
        let back: Request = serde_json::from_str(&s).unwrap();
        match back {
            Request::Screens { ids } => assert_eq!(ids, vec![1, 3, 7]),
            other => panic!("解回来不是 Screens：{other:?}"),
        }
    }

    #[test]
    fn screens_response_round_trips() {
        use crate::pty::{ScreenSpan, ScreenStyle};
        let resp = Response::Screens {
            screens: vec![ScreenEntry {
                id: 4,
                lines: vec![vec![ScreenSpan {
                    text: "干活中".into(),
                    style: ScreenStyle::default(),
                }]],
            }],
        };
        let s = serde_json::to_string(&resp).unwrap();
        let back: Response = serde_json::from_str(&s).unwrap();
        match back {
            Response::Screens { screens } => {
                assert_eq!(screens.len(), 1);
                assert_eq!(screens[0].id, 4);
                assert_eq!(screens[0].lines[0][0].text, "干活中");
            }
            other => panic!("解回来不是 Screens：{other:?}"),
        }
    }
}
