use serde::{Deserialize, Serialize};
use std::path::PathBuf;

use crate::git::FileStat;
use crate::profile::ProfileStatus;
use crate::pty::ScreenSpan;
use crate::session::SessionInfo;

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

#[derive(Debug, Serialize, Deserialize)]
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
    Profiles,
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

#[derive(Debug, Serialize, Deserialize)]
pub enum Response {
    Sessions(Vec<SessionInfo>),
    Created {
        id: u32,
    },
    Screen {
        lines: Vec<Vec<ScreenSpan>>,
        cursor: (u16, u16),
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
