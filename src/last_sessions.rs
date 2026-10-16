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
/// **2026-10-14 起也记 agent 自己的对话 id**（profile 声明了
/// `session_id_args` / `resume_id_args` 才有）。以前只记「哪个目录、哪个
/// profile、起了个什么名字、什么时候还活着」，接回哪段对话交给
/// `--continue` 按目录去猜——同一个目录下开着好几个会话时，只有一个能
/// 接回原来的对话，其余的只能从头开。有了 id 就各接各的，见
/// `docs/superpowers/specs/2026-10-14-graceful-restart-design.md` 第 1 节。
/// 没有 id 的旧记录照旧走 `--continue` 分组（[`continue_flags`]）。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
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
    /// agent 自己的对话 id（profile 声明了 `session_id_args` 才有）。
    #[serde(default)]
    pub conversation_id: Option<String>,
    /// 平稳重启时这个会话是被打断的（没干完就被叫停或硬停）。
    #[serde(default)]
    pub interrupted: bool,
    /// 接不回对话的会话，重启前写的交接文件。
    #[serde(default)]
    pub handoff: Option<PathBuf>,
    /// 会话编号（看板上的「3 号」）。重启后尽量还用这个号——开着的界面、
    /// 用户脑子里的「3 号」都还指着它。旧文件没有，或者号已经被占了，就
    /// 发一个新号。
    #[serde(default)]
    pub id: Option<u32>,
    /// 这个会话就是敲 `dct restart` 的那个。重启后它收到的是「重启已经
    /// 完成了，不用再重启」，不是「继续」——它没做完的事正是那条命令。
    #[serde(default)]
    pub restart_initiator: bool,
    /// 准备重启期间别的电脑发来、还没送进去的留言（已经加好来源标记的
    /// 整句）。重启后等会话空下来照常送。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub held_messages: Vec<String>,
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

/// 这一条重启后会不会按对话 id 接回：记录里有 id，**而且**它的 profile
/// 此刻还声明着按 id 接回的参数（`Profile::resumes_by_id`）。profile 后来
/// 去掉了那两组参数的话，id 就没处用，这一条照旧走 `--continue` 分组。
pub fn will_resume_by_id(r: &RecordedSession, profiles: &[crate::profile::Profile]) -> bool {
    r.conversation_id.is_some()
        && profiles
            .iter()
            .find(|p| p.name == r.profile)
            .is_some_and(|p| p.resumes_by_id())
}

/// 按 id 接回的记录各接各的，不进 `--continue` 的分组；其余的照旧分组。
/// 返回值跟输入逐位对应，语义同 [`group_for_resume`]：`true` = 这一条该带
/// `resume_args`。按 id 接回的那些一律是 `false`——用不着猜。判定「按不按
/// id 接回」见 [`will_resume_by_id`]。
///
/// **同一个 (目录, profile) 里已经有按 id 接回的记录时，没有 id 的那几条
/// 一律不带 `--continue`，从空白开始。** `--continue` 接的是这个目录下最近
/// 写过的那段对话；兄弟会话有自己的 id、又常常是最近用过的那个，这时
/// `--continue` 接到的就是兄弟的对话——两个格子写同一份对话，自己那段
/// 反而丢了。没有 id 又用过的会话在准备重启时会被请去写交接（见
/// `SessionManager::drain_resumable`），所以从空白开始也能接着干。
pub fn continue_flags(
    sessions: &[RecordedSession],
    profiles: &[crate::profile::Profile],
) -> Vec<bool> {
    let by_id: Vec<bool> = sessions
        .iter()
        .map(|r| will_resume_by_id(r, profiles))
        .collect();
    let sibling_has_id = |i: usize| {
        (0..sessions.len()).any(|j| {
            by_id[j] && sessions[j].dir == sessions[i].dir && sessions[j].profile == sessions[i].profile
        })
    };
    let idx: Vec<usize> = (0..sessions.len())
        .filter(|&i| !by_id[i] && !sibling_has_id(i))
        .collect();
    let sub: Vec<RecordedSession> = idx.iter().map(|&i| sessions[i].clone()).collect();
    let flags = group_for_resume(&sub);
    let mut out = vec![false; sessions.len()];
    for (k, &i) in idx.iter().enumerate() {
        out[i] = flags[k];
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
            last_active,
            ..Default::default()
        }
    }

    #[test]
    fn old_file_without_new_fields_still_loads() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("last-sessions.toml");
        std::fs::write(
            &p,
            "[[sessions]]\ndir = \"/tmp\"\nprofile = \"claude\"\ntag = \"\"\nlast_active = 5\n",
        )
        .unwrap();
        let v = load(&p);
        assert_eq!(v.len(), 1);
        assert_eq!(v[0].conversation_id, None);
        assert!(!v[0].interrupted);
        assert_eq!(v[0].handoff, None);
    }

    #[test]
    fn new_fields_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("last-sessions.toml");
        let r = RecordedSession {
            dir: "/tmp".into(),
            profile: "claude".into(),
            tag: "t".into(),
            last_active: 1,
            conversation_id: Some("11111111-1111-4111-8111-111111111111".into()),
            interrupted: true,
            handoff: Some("/tmp/h.md".into()),
            id: Some(7),
            restart_initiator: true,
            held_messages: vec!["[来自 pc] 你好".into()],
        };
        save(&p, std::slice::from_ref(&r)).unwrap();
        assert_eq!(load(&p), vec![r]);
    }

    /// 有对话 id 的那条各接各的，不占 `--continue` 的位子；没 id 的照旧按
    /// `last_active` 挑一条继续——**只在同组没有按 id 接回的兄弟时**。
    #[test]
    fn continue_flags_skips_records_with_ids() {
        let mut with_id = rec("/w/x", "claude", 9);
        with_id.conversation_id = Some("abc".into());
        let sessions = vec![
            rec("/w/x", "claude", 5),
            with_id,
            rec("/w/x", "claude", 1),
            rec("/w/y", "claude", 3),
            rec("/w/y", "claude", 4),
        ];
        let profiles = vec![crate::profile::Profile::builtin("claude").unwrap()];
        assert_eq!(
            continue_flags(&sessions, &profiles),
            vec![false, false, false, false, true]
        );
    }

    /// I2：没 id 的会话（上次是 `--continue` 接回来的）跟有 id 的兄弟在同一个
    /// 目录里时，`--continue` 会接到兄弟最近写的那段对话上。这种一律从空白开始。
    #[test]
    fn no_continue_when_a_sibling_resumes_by_id() {
        let mut b = rec("/w/x", "claude", 9);
        b.conversation_id = Some("b".into());
        let a = rec("/w/x", "claude", 5);
        let profiles = vec![crate::profile::Profile::builtin("claude").unwrap()];
        assert_eq!(continue_flags(&[a.clone(), b], &profiles), vec![false, false]);
        // 没有兄弟时照旧接回
        assert_eq!(continue_flags(&[a], &profiles), vec![true]);
    }

    /// 记录里有 id，但 profile 已经不声明按 id 接回的参数了：id 没处用，
    /// 这一条要回到 `--continue` 分组里去抢位，不能被晾在外面。
    #[test]
    fn an_id_whose_profile_no_longer_resumes_by_id_joins_continue_grouping() {
        let mut with_id = rec("/w/x", "claude", 9);
        with_id.conversation_id = Some("abc".into());
        let sessions = vec![rec("/w/x", "claude", 5), with_id.clone()];

        let mut no_ids = crate::profile::Profile::builtin("claude").unwrap();
        no_ids.session_id_args.clear();
        no_ids.resume_id_args.clear();
        let profiles = vec![no_ids];

        assert!(!will_resume_by_id(&with_id, &profiles));
        assert_eq!(continue_flags(&sessions, &profiles), vec![false, true]);
        // profile 都不认识了也一样：不算按 id 接回。
        assert!(!will_resume_by_id(&with_id, &[]));
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
