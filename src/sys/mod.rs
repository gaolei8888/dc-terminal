//! 平台适配层。
//!
//! dct 的骨头是 Unix 的：守护进程和界面之间是一个 Unix domain socket，进程
//! 的生死是 `kill`/`setsid`，密钥文件的保护就是那个 0600 位，界面靠 `sigwait`
//! 收 SIGTERM 才能在被杀之前把终端还回去。Windows 一样都没有。
//!
//! 但「一样都没有」不等于「做不到」——每一件事 Windows 都有自己的说法，只是
//! 名字和语义都不一样。这一层就是那张对照表：**每个概念一个文件，两种实现
//! 并排放在同一个文件里**，而不是把 `#[cfg]` 撒到调用点上。理由是这些差异
//! 从来不是「换个函数名」那么简单（比如 Windows 没有 SIGTERM，「请你自己
//! 收拾干净再走」这句话根本说不出口），差异必须和它的代价写在一起，写在
//! 只有一份的地方。
//!
//! 调用方看到的是一套与平台无关的名字，语义以 Unix 那份为准；Windows 侧
//! 达不到同样强度的地方，在各自的文件里点名说清楚，不假装。
pub mod fs;
pub mod ipc;
pub mod proc;
pub mod shell;
pub mod signal;
pub mod term;
pub mod tls;

/// 测试夹具专用，见那个文件的头。产品代码不编译它。
#[cfg(test)]
pub mod testing;

/// Windows 的 ACL 拼装。放在这一层而不是 `fs` 里面，是因为 socket 目录
/// 也要用它（见 `ipc`）。
#[cfg(windows)]
pub mod acl;

/// 家目录。
///
/// `$HOME` 排第一，两个平台都是——Windows 上它通常没设，但**设了就得听**：
/// 集成测试正是靠改这个变量把整个 `~/.dct` 挪进临时目录的（见
/// `tests/daemon_upgrade.rs`），改不动它就等于在真实的家目录里跑测试。
///
/// Windows 上的正主是 `%USERPROFILE%`。最后那一档 `HOMEDRIVE`+`HOMEPATH`
/// 是给域账户兜底的：那种环境里家目录可能在网络盘上，两个变量拼起来才是
/// 完整路径。
///
/// 返回 `None` 表示这台机器上问不出家目录。调用方各自决定怎么办——有的
/// 退到临时目录，有的直接放弃，没有一个统一的兜底值能对所有场景都成立。
pub fn home() -> Option<std::path::PathBuf> {
    if let Some(h) = std::env::var_os("HOME").filter(|h| !h.is_empty()) {
        return Some(std::path::PathBuf::from(h));
    }
    #[cfg(windows)]
    {
        if let Some(h) = std::env::var_os("USERPROFILE").filter(|h| !h.is_empty()) {
            return Some(std::path::PathBuf::from(h));
        }
        let drive = std::env::var("HOMEDRIVE").ok()?;
        let path = std::env::var("HOMEPATH").ok()?;
        if !drive.is_empty() && !path.is_empty() {
            return Some(std::path::PathBuf::from(format!("{drive}{path}")));
        }
    }
    None
}
