use serde::{Deserialize, Serialize};
use std::path::PathBuf;

use crate::git::FileStat;
use crate::profile::ProfileStatus;
use crate::pty::ScreenSpan;
use crate::session::{SessionInfo, SessionState};

/// 界面和守护进程之间的线上契约版本。**改了协议就要加一。**
///
/// 这两个东西是分开升级的：守护进程一活就是好几天（它活得久正是这个产品
/// 存在的理由），用户装了新版本 dct 之后，跟他说话的还是几天前那个进程。
/// 协议一改，新界面发的请求旧守护进程就解不出来——2026-08-05 的现场是
/// `Profiles` 加了 `lang` 字段之后，按 n 只弹一句「拿不到 agent 列表」，
/// 没有任何线索指向真正的原因。
///
/// `the_request_shape_is_pinned_to_the_protocol_version` 会在形状变了而
/// 这个数字没变时变红。
pub const PROTOCOL_VERSION: u32 = 1;

/// 对面那个守护进程能不能用。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DaemonStatus {
    /// 同一份协议，正常用。
    Same,
    /// 号对不上，或者老到连 `Hello` 都不认识。
    Stale,
}

/// `None` = 连 `Hello` 都答不上来，那是比握手本身还老的守护进程。
pub fn daemon_status(protocol: Option<u32>) -> DaemonStatus {
    match protocol {
        Some(v) if v == PROTOCOL_VERSION => DaemonStatus::Same,
        _ => DaemonStatus::Stale,
    }
}

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
    /// 「你是几号协议？」界面连上之后问的第一句。放在最前面，且
    /// **永远不许改形状**——它是唯一一条必须让任何年纪的守护进程都能
    /// 解出来的请求（老到答不上来也没关系，答不上来本身就是答案）。
    Hello,
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
            Request::Hello => write!(f, "Hello"),
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
    /// 对 [`Request::Hello`] 的回答。同样不许改形状。
    Hello {
        protocol: u32,
    },
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
        /// 报码不报句子——理由同 `ErrorCode`。
        warnings: Vec<WarningCode>,
    },
    Projects(Vec<String>),
    LastProfile(Option<String>),
    Verify(crate::verify::VerifyOutcome),
    Ok,
    Error(ErrorCode),
}

/// 守护进程报「哪一类错 + 参数」，**不组句**。
///
/// 组句一律发生在界面进程：daemon 是常驻的、可能同时服务多个界面的进程，
/// 它不知道也不该知道谁在用什么语言。报码的另一个好处是切语言立刻生效，
/// 不用重启 daemon——它存下来的东西里没有任何一句是某种语言的。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum ErrorCode {
    NoSuchProfile(String),
    DirNotFound(String),
    NotAGitRepo(String),
    NoSuchSession(u32),
    NoCheckpoint,
    /// 「这个会话没有检查点」和「没有改动记录」成因其实是同一个：不是 agent
    /// 会话。措辞交给界面——它知道用户刚按的是 `u` 还是 `d`，比守护进程更有
    /// 资格决定这句话怎么说。
    NotAnAgentSession,
    /// 请求解析失败，带上原始错误供排查
    BadRequest(String),
    /// git 自己的 stderr。**刻意留的兜底**：那是 git 按它自己的 `LANG` 输出的，
    /// dct 翻不动也不该翻。界面显示成「操作失败：<原文>」——外面那半句是
    /// 翻译过的，里面照抄。
    Git(String),
    /// agent 的命令跑不起来。带上命令名——用户至少知道该去修哪个。
    CannotStart(String),
    /// 守护进程那边没反应了（连不上、超时、连接被关）。三种情况用户能做的
    /// 是同一件事，所以不分。
    DaemonNotResponding,
    /// 密钥文件坏了，所以拒绝写入——当空覆盖的话，用户手改坏的文件
    /// （也许只是少个引号，完全能救回来）会被内存里那份残缺数据彻底盖掉。
    SecretsFileBroken {
        path: String,
    },
    /// 某个具体操作失败了。这些都是罕见的文件系统 / git 故障，用户能做的
    /// 只有「再试一次」或者「知道自己的工作区可能停在半路」——所以按**操作**
    /// 分类就够，不必把底层 io 错误也搬上界面。
    OperationFailed(Operation),
    /// 还没归类的内部错误，同样照抄原文。有它才能一步步迁移，而不是等到
    /// 每一条都归好类才敢合并。
    Internal(String),
}

/// 把一个 `ErrorCode` 塞进 `anyhow::Error` 里带出去。
///
/// 不把各模块的错误类型整个换成 `ErrorCode`：它们内部到处在 `?` io/git
/// 错误，全换要动的地方远多于收益；而 daemon 边界那一处 `downcast` 就能把
/// 码取回来（见 `daemon.rs` 的 `to_code`）。取不回来的就是还没归类的内部
/// 错误，照抄原文。
pub fn coded(c: ErrorCode) -> anyhow::Error {
    anyhow::Error::new(CodedError(c))
}

/// `anyhow` 要求负载实现 `std::error::Error`，包一层。
///
/// **刻意不给 `ErrorCode` 实现 `Display`**：实现了就会有人顺手拿它当界面
/// 文案，而那正是这套机制要根除的事——句子只能由 `i18n::msg::error` 组。
#[derive(Debug)]
pub struct CodedError(pub ErrorCode);

impl std::fmt::Display for CodedError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // 这句只会进 stderr 日志，不上界面——界面拿的是码。
        write!(f, "{:?}", self.0)
    }
}

impl std::error::Error for CodedError {}

/// 哪个操作失败了。措辞的分寸各不相同——「撤销失败」必须提醒用户工作区
/// 可能停在改到一半的状态（这是他必须知道的后果），而「算不出改了哪些文件」
/// 只要说再试一次就够。
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum Operation {
    /// 建会话时拍第一个检查点
    FirstCheckpoint,
    /// 每一步之后拍检查点
    Checkpoint,
    /// 撤销（git restore）
    Undo,
    /// 算改动（git diff）
    Diff,
    /// 写密钥文件
    SaveSecret,
    /// 写设置文件
    SaveSettings,
    /// 起 PTY 子进程
    SpawnPty,
    /// 读剪贴板
    ReadClipboard,
}

/// 读文件失败的类别。只留用户**分得清、也做得了什么**的那几种；
/// 其余归到笼统的一条——`io::Error` 的 `Display` 是系统原话，
/// 常年带 `os error N` 这种只有程序员看得懂的后缀。
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum IoReason {
    PermissionDenied,
    NotADirectory,
    Other,
}

/// 顶到界面上的**警告**（不是错误：dct 照常能用，只是有东西没读进来）。
/// 跟 `ErrorCode` 同样的道理——守护进程报码，界面组句。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum WarningCode {
    /// 自定义 profile 目录打不开
    ProfileDirUnreadable { name: String, reason: IoReason },
    /// 某个 profile 文件读不了
    ProfileUnreadable { name: String, reason: IoReason },
    /// 某个 profile 文件写错了。`line` 是行号（有的话），`reason` 是 toml 库
    /// 自己的说法——那半句可能是英文，但用户本来就在手改这份 TOML，
    /// 「expected ...」比吞掉更有用（同 `ErrorCode::Git` 的道理）。
    ProfileMalformed {
        name: String,
        line: Option<usize>,
        reason: String,
    },
    /// 密钥文件读不了
    SecretsUnreadable { path: String, reason: IoReason },
    /// 密钥文件坏了。**不给行号也不给 toml 的原文**：README 明说密钥不该手改，
    /// 而且这时候所有写入都被拒，照着行号去抠语法是把用户往错路上支。
    /// 唯一有效的下一步是删掉它重新粘贴一遍。
    SecretsCorrupt { path: String },
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

    /// 答不上「你是几号协议」的守护进程，就是老到连这个问题都不认识的那种。
    /// 现场那个 daemon 正是如此：它比 `Hello` 还老。
    #[test]
    fn a_daemon_that_cannot_answer_is_stale() {
        assert_eq!(daemon_status(None), DaemonStatus::Stale);
    }

    /// 答得上但号对不上，一样不能用——两边编译自不同的源码。
    #[test]
    fn a_daemon_on_another_protocol_is_stale() {
        assert_eq!(
            daemon_status(Some(PROTOCOL_VERSION + 1)),
            DaemonStatus::Stale
        );
        assert_eq!(
            daemon_status(Some(PROTOCOL_VERSION.wrapping_sub(1))),
            DaemonStatus::Stale
        );
    }

    #[test]
    fn a_daemon_on_the_same_protocol_is_usable() {
        assert_eq!(daemon_status(Some(PROTOCOL_VERSION)), DaemonStatus::Same);
    }

    /// 协议的线上形状，钉死在 `PROTOCOL_VERSION` 上。
    ///
    /// 改了任何一个变体的名字或字段，这条会红。红了就**必须**把
    /// `PROTOCOL_VERSION` 一起加一——否则新界面又会去跟一个解不出它请求的
    /// 旧守护进程说话，而用户看到的只有一句「拿不到 agent 列表」，
    /// 没有任何线索指向真正的原因。这正是 2026-08-05 那次现场事故。
    ///
    /// 只钉请求：请求是**新界面发给旧 daemon** 的那一半，也就是会炸的那一半。
    #[test]
    fn the_request_shape_is_pinned_to_the_protocol_version() {
        // 每个变体都要在这里出现一次。漏一个不会被这条测试发现，但会被
        // `impl Debug for Request` 那个穷举 match 拦下来——加变体必须
        // 同时改那里。
        let all = vec![
            Request::Hello,
            Request::List,
            Request::Create {
                dir: "d".into(),
                profile: "p".into(),
                remember: true,
            },
            Request::Input {
                id: 1,
                text: "t".into(),
            },
            Request::Screen { id: 1 },
            Request::Screens { ids: vec![1] },
            Request::Resize {
                id: 1,
                rows: 2,
                cols: 3,
            },
            Request::Stop { id: 1 },
            Request::Undo { id: 1 },
            Request::Diff { id: 1 },
            Request::Profiles {
                lang: crate::i18n::Lang::Zh,
            },
            Request::Projects,
            Request::SetSecret {
                profile: "p".into(),
                value: "v".into(),
            },
            Request::DeleteSecret {
                profile: "p".into(),
            },
            Request::LastProfile,
            Request::VerifySecret {
                profile: "p".into(),
                value: "v".into(),
            },
        ];

        let shape = serde_json::to_string(&all).unwrap();
        assert_eq!(
            (PROTOCOL_VERSION, shape.as_str()),
            (
                1,
                r#"["Hello","List",{"Create":{"dir":"d","profile":"p","remember":true}},{"Input":{"id":1,"text":"t"}},{"Screen":{"id":1}},{"Screens":{"ids":[1]}},{"Resize":{"id":1,"rows":2,"cols":3}},{"Stop":{"id":1}},{"Undo":{"id":1}},{"Diff":{"id":1}},{"Profiles":{"lang":"Zh"}},"Projects",{"SetSecret":{"profile":"p","value":"v"}},{"DeleteSecret":{"profile":"p"}},"LastProfile",{"VerifySecret":{"profile":"p","value":"v"}}]"#
            ),
            "协议的线上形状变了。把 PROTOCOL_VERSION 加一，再把这里的期望值更新成新的形状。"
        );
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
