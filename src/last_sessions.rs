//! 守护进程重启前还活着的会话，记一份小清单在 socket 旁边（`last-sessions.toml`），
//! 好让重启之后能把它们接回来。
//!
//! **这不是一个通用的持久化层。** 它只回答一个问题：「上次关掉之前，
//! 哪些目录 + 哪个 profile 还在跑」，多做一步都是画蛇添足——具体到底
//! 该不该真的把某一条接回去（目录还在不在、profile 还认不认识、同一个
//! 目录+profile 下该让哪一条继续），全部交给 [`group_for_resume`] 这个
//! 纯函数和调用方（`daemon.rs`）判断，这个模块自己不做任何决定，只管
//! 读写磁盘。
//!
//! # 什么时候落盘
//!
//! **只在会话集合真的变了的时候**——建、（显式）停/杀、清理，一律由
//! `SessionManager` 在那几个方法末尾调用。**绝不能从 `tick()` 里写**：
//! `tick()` 是 200ms 跑一轮、驱动整个守护进程的主循环，磁盘 IO 会让它
//! 卡住所有会话的状态刷新（`list()`/`screen()` 全靠它及时判定 Working/
//! Idle/Failed）。这也意味着「进程自己崩了」（`tick()` 里那条 `vanished`
//! 分支）不会立刻把这一条从清单上抹掉——它会留到下一次 `create`/`stop`/
//! `kill`/`prune` 才被冲掉。这是故意的：一条稍微过期的记录好过为了
//! 保持它绝对精确而让主循环背上磁盘 IO。
//!
//! # 路径怎么定
//!
//! 完全照抄 `secrets::secrets_path_for_socket` 的做法：挂在 socket 的
//! 同一个目录下。测试把 socket 放进临时目录，这个文件也就自动跟着隔离，
//! 不会碰到用户真实的 `~/.dct/last-sessions.toml`。

use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::io::Write;
use std::path::{Path, PathBuf};

/// 一条记下来的、当时还活着的会话。
///
/// **不记 agent 自己的对话 id**——那条路（`claude --resume <id>`）在设计
/// 阶段就被明确划出了范围：能记的只有「哪个目录、哪个 profile、起了个
/// 什么名字、什么时候还活着」，接不接得回**同一个**对话，交给
/// `--continue` 自己按目录去猜。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecordedSession {
    pub dir: PathBuf,
    pub profile: String,
    #[serde(default)]
    pub tag: String,
    /// UNIX 纪元毫秒（不是秒）——精度按秒算的话，两个会话在同一秒内先后
    /// 被用过（比如一次交互测试里连着敲两下），落盘时会看着一样新，
    /// `group_for_resume` 的 `max_by_key` 就退化成「打平取最后一个」，
    /// 而那不是真的「谁更晚被用过」。只在同一份清单内部比较「谁更新」，
    /// 跨进程重启之后
    /// 的绝对值没有意义——见 `group_for_resume` 的文档。
    pub last_active: u64,
}

/// 磁盘格式。跟 `secrets.rs` 一样包一层表，留出将来加别的段的余地。
#[derive(Default, Serialize, Deserialize)]
struct Disk {
    #[serde(default)]
    sessions: Vec<RecordedSession>,
}

/// 跟着 socket 走，测试自动隔离（同 `secrets::secrets_path_for_socket`）。
pub fn last_sessions_path_for_socket(socket: &Path) -> PathBuf {
    match socket.parent() {
        Some(d) => d.join("last-sessions.toml"),
        None => PathBuf::from("last-sessions.toml"),
    }
}

/// 读清单。**任何读不了/解析不了的情况都当成「没有记录」**，不是错误——
/// 跟 `secrets.rs` 不一样，这份数据丢了的代价只是「这次不问要不要恢复」，
/// 不是丢用户的密钥，没必要为了保护一份可以随时重建的缓存而拒绝之后的
/// 写入。原始错误写 stderr 留个诊断痕迹。
pub fn load(path: &Path) -> Vec<RecordedSession> {
    match std::fs::read_to_string(path) {
        Ok(src) => match toml::from_str::<Disk>(&src) {
            Ok(d) => d.sessions,
            Err(e) => {
                eprintln!("上次会话清单解析失败（{}）：{e}", path.display());
                Vec::new()
            }
        },
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Vec::new(),
        Err(e) => {
            eprintln!("上次会话清单读取失败（{}）：{e}", path.display());
            Vec::new()
        }
    }
}

/// 原子写：先写同目录的临时文件再 rename，理由同 `secrets.rs::save`——
/// 直接覆写的话写到一半断电会留下半截 TOML。
///
/// 0600：清单里的目录名多少也是用户项目的信息，没必要让同机器的其它
/// 账号读到。
pub fn save(path: &Path, sessions: &[RecordedSession]) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let text = toml::to_string(&Disk {
        sessions: sessions.to_vec(),
    })?;

    let tmp = path.with_extension("toml.tmp");
    let result = (|| -> Result<()> {
        let mut f = crate::sys::fs::create_private(&tmp)?;
        f.write_all(text.as_bytes())?;
        f.sync_all()?;
        // 同 `secrets.rs`：先关句柄再改名，不依赖 Windows 的共享位。
        drop(f);
        std::fs::rename(&tmp, path)?;
        Ok(())
    })();

    // 跟 secrets.rs 同一条纪律：写到一半失败不能把临时文件留在目录里。
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    result
}

/// 用户在开局提示里按了 Enter（拒绝恢复）：清单要清空，不是留着等下次。
/// **不是删文件**——`save` 一份空清单，跟「文件从没存在过」在 `load()`
/// 眼里是同一件事，行为更容易预测（不用在「文件不存在」和「文件是空
/// 清单」两种状态之间再分叉一次）。
pub fn clear(path: &Path) -> Result<()> {
    save(path, &[])
}

/// **这是整个功能里风险最大的一步，所以拆成一个纯函数、单独往死里测。**
///
/// `claude --continue` 恢复的是**这个目录下最近一次对话**，不认「是哪个
/// 会话喊我恢复的」。如果同一个 `(dir, profile)` 下当时活着两个会话，
/// 两个都带上 `--continue` 的话，它们会一起接到同一份对话上——把这条
/// 命令拍在错的槽位上，比这个槽位老老实实开一个新对话糟糕得多：用户在
/// 看板上看到的是两个格子，以为它们各自独立，实际上一个格子里打的字会
/// 出现在另一个格子的对话历史里。
///
/// 规则：按 `(dir, profile)` 分组，每组里 `last_active` 最大的那一条判
/// `true`（该接上 `resume_args`），组里其余的判 `false`（老老实实开一个
/// 新的）。落单的组（绝大多数情况）里唯一的那条自然判 `true`。
///
/// 返回值跟输入**逐位对应**（`out[i]` 对应 `sessions[i]`），不是重新排序
/// 或者去重——调用方（`daemon.rs` 的恢复逻辑、`main.rs` 的开局提示）都要
/// 按原始顺序把这份判定跟对应的会话配对着用。
pub fn group_for_resume(sessions: &[RecordedSession]) -> Vec<bool> {
    let mut out = vec![false; sessions.len()];

    let mut groups: std::collections::HashMap<(&Path, &str), Vec<usize>> =
        std::collections::HashMap::new();
    for (i, s) in sessions.iter().enumerate() {
        groups
            .entry((s.dir.as_path(), s.profile.as_str()))
            .or_default()
            .push(i);
    }

    for idxs in groups.values() {
        // `max_by_key` 在打平的情况下取**最后**一个最大值——两条
        // `last_active` 完全相等时，这里没有「哪个更该继续」这种依据，
        // 谁被选中都不会比另一种选法更对，选最后一个只是为了让结果
        // 确定、可重复，不依赖 HashMap 的迭代顺序（`idxs` 本身是按
        // 输入顺序 push 进去的，跟 HashMap 的桶序无关）。
        if let Some(&best) = idxs.iter().max_by_key(|&&i| sessions[i].last_active) {
            out[best] = true;
        }
    }

    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rec(dir: &str, profile: &str, last_active: u64) -> RecordedSession {
        RecordedSession {
            dir: PathBuf::from(dir),
            profile: profile.to_string(),
            tag: String::new(),
            last_active,
        }
    }

    #[test]
    fn last_sessions_path_sits_next_to_socket() {
        let p = last_sessions_path_for_socket(Path::new("/home/x/.dct/daemon.sock"));
        assert_eq!(p, PathBuf::from("/home/x/.dct/last-sessions.toml"));
    }

    #[test]
    fn missing_file_is_empty_and_not_an_error() {
        let tmp = tempfile::tempdir().unwrap();
        let recs = load(&tmp.path().join("从没建过.toml"));
        assert!(recs.is_empty());
    }

    #[test]
    fn round_trips_through_a_temp_dir() {
        let tmp = tempfile::tempdir().unwrap();
        let f = tmp.path().join("last-sessions.toml");
        let sessions = vec![
            rec("/w/proj-a", "claude", 100),
            rec("/w/proj-b", "shell", 200),
        ];
        save(&f, &sessions).unwrap();

        let loaded = load(&f);
        assert_eq!(loaded, sessions);
    }

    #[test]
    fn declining_the_prompt_clears_the_record() {
        let tmp = tempfile::tempdir().unwrap();
        let f = tmp.path().join("last-sessions.toml");
        save(&f, &[rec("/w/proj-a", "claude", 1)]).unwrap();
        assert!(!load(&f).is_empty());

        clear(&f).unwrap();
        assert!(load(&f).is_empty(), "拒绝恢复之后清单要清空");
    }

    #[test]
    fn corrupt_file_is_treated_as_empty_not_fatal() {
        let tmp = tempfile::tempdir().unwrap();
        let f = tmp.path().join("last-sessions.toml");
        std::fs::write(&f, "这不是 TOML {{{").unwrap();
        assert!(
            load(&f).is_empty(),
            "读坏了不该让整个功能崩掉，当空清单处理"
        );
    }

    /// 只在 Unix 上验位。Windows 的对应保证是一条只有当前用户的 ACL，
    /// 形状完全不同，验它要另一套调用（见 `sys::acl`），不在这条测试里凑。
    #[test]
    #[cfg(unix)]
    fn file_is_owner_only() {
        use std::os::unix::fs::PermissionsExt;
        let tmp = tempfile::tempdir().unwrap();
        let f = tmp.path().join("last-sessions.toml");
        save(&f, &[rec("/w/proj-a", "claude", 1)]).unwrap();
        let mode = std::fs::metadata(&f).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
    }

    #[test]
    fn no_temp_file_is_left_behind() {
        let tmp = tempfile::tempdir().unwrap();
        let f = tmp.path().join("last-sessions.toml");
        save(&f, &[rec("/w/proj-a", "claude", 1)]).unwrap();
        let leftovers: Vec<_> = std::fs::read_dir(tmp.path())
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().to_string())
            .filter(|n| n != "last-sessions.toml")
            .collect();
        assert!(leftovers.is_empty(), "临时文件要收干净：{leftovers:?}");
    }

    /// **这是整个功能里最要命的一条测试。** 同一个目录下两个 claude
    /// 会话，只有活得最晚的那个能带 `--continue`；早的那个必须老老实实
    /// 判 `false`，否则两个格子会一起接到同一份对话上。
    #[test]
    fn claude_x2_collision_only_the_most_recently_active_one_continues() {
        let sessions = vec![
            rec("/w/dc-terminal", "claude", 100), // 早
            rec("/w/dc-terminal", "claude", 200), // 晚——该是这个继续
        ];
        let out = group_for_resume(&sessions);
        assert_eq!(out, vec![false, true]);
    }

    #[test]
    fn a_lone_session_in_its_group_always_continues() {
        let sessions = vec![rec("/w/dc-terminal", "claude", 1)];
        assert_eq!(group_for_resume(&sessions), vec![true]);
    }

    /// 不同目录、不同 profile 的会话互不影响——分组键是 `(dir, profile)`
    /// 两者都要匹配才算同一组。
    #[test]
    fn different_dirs_or_profiles_are_independent_groups() {
        let sessions = vec![
            rec("/w/a", "claude", 1),
            rec("/w/b", "claude", 2),
            rec("/w/a", "shell", 3),
        ];
        assert_eq!(group_for_resume(&sessions), vec![true, true, true]);
    }

    #[test]
    fn three_way_collision_still_picks_exactly_one() {
        let sessions = vec![
            rec("/w/x", "claude", 5),
            rec("/w/x", "claude", 9),
            rec("/w/x", "claude", 1),
        ];
        let out = group_for_resume(&sessions);
        assert_eq!(out, vec![false, true, false]);
        assert_eq!(out.iter().filter(|&&b| b).count(), 1);
    }
}
