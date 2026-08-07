//! `dct ps` / `dct stop`：不开界面，直接从普通终端看会话、停会话。
//!
//! 为什么需要它：守护进程活得比界面久（那正是它存在的理由），所以「有东西
//! 在后台跑着」和「我现在能看见它」是两回事。在这之前，想知道有哪些 agent
//! 还活着、想把它们停掉，只能靠 `ps` 加 `pkill` 去认进程——而那条路认的是
//! **进程**不是**会话**，`pkill -f "dct daemon"` 会把所有会话一起带走，用户
//! 分不出自己杀掉的是哪一个。
//!
//! 两条命令都**不会**顺手拉起守护进程。`dct` 开界面时拉是对的（用户就是要
//! 用它），但「问问有没有东西在跑」不该把「没有」变成「现在有了」。
//!
//! 渲染和参数解析都抽成纯函数，跟 socket 无关，所以能直接测——`dct stop`
//! 是不可撤销的，「哪些参数意味着停掉全部」这件事必须在没有守护进程的
//! 情况下也测得到。

use std::path::Path;

use anyhow::Result;

use crate::client::Client;
use crate::i18n::{text, Key, Lang};
use crate::proto::{Request, Response};
use crate::session::{SessionInfo, SessionState};

/// `dct stop` / `dct kill` 的参数解析结果。两条命令的参数形状一模一样，
/// 规矩也一模一样——**要哪个必须明写**——所以共用一份解析。
#[derive(Debug, PartialEq, Eq)]
pub enum Target {
    /// 这几个会话
    Ids(Vec<u32>),
    /// 全部
    All,
    /// 参数不对，把该说的话说清楚
    Usage(String),
}

/// 解析 `dct stop` / `dct kill` 后面的参数。
///
/// **不给参数不等于「全部」。** 这两条命令都不可撤销，而它们又都是最容易被
/// 手滑敲出来的形式——默认成全部的话，用户想停一个却停光了所有 agent，
/// 正在跑的活全断。要全部必须明写 `--all`。
///
/// `cmd` 只影响用法提示里印的是 `dct stop 3` 还是 `dct kill 3`：用户敲的是
/// 哪条命令，回话里就得是哪条。印错了等于让他去解一个他没问的问题。
pub fn parse_target_args(args: &[String], lang: Lang, cmd: &str) -> Target {
    if args.is_empty() {
        return Target::Usage(crate::i18n::msg::needs_a_target(lang, cmd));
    }
    if args.iter().any(|a| a == "--all") {
        // `--all` 跟具体 id 混着给，说明用户自己也没想清楚要停什么。
        // 与其猜一个，不如让他重敲一遍——这条命令撤不回来。
        if args.len() > 1 {
            return Target::Usage(crate::i18n::msg::all_takes_no_ids(lang, cmd));
        }
        return Target::All;
    }
    let mut ids = Vec::new();
    for a in args {
        match a.parse::<u32>() {
            Ok(n) => ids.push(n),
            Err(_) => return Target::Usage(crate::i18n::msg::not_a_session_id(lang, a)),
        }
    }
    Target::Ids(ids)
}

fn status_word(s: SessionState, lang: Lang) -> &'static str {
    text(
        match s {
            SessionState::Working => Key::StatusWorking,
            SessionState::Asking => Key::StatusAsking,
            SessionState::Idle => Key::StatusIdle,
            SessionState::Stopped => Key::StatusStopped,
            SessionState::Failed => Key::StatusFailed,
            SessionState::Unknown => Key::StatusUnknown,
        },
        lang,
    )
}

/// 一行一个会话。空列表给一句话，不是一张空表。
///
/// 不对齐成表格：状态词是中文（双宽），项目路径长短差得远，按字符数补空格
/// 在终端里反而歪得更厉害。这是给人一眼扫的，不是给 `awk` 切的——真要切的
/// 话字段之间的双空格也够用。
pub fn render_ps(sessions: &[SessionInfo], lang: Lang) -> String {
    if sessions.is_empty() {
        return text(Key::NoSessionsRunning, lang).into();
    }
    sessions
        .iter()
        .map(|s| {
            let line = format!(
                "{}  {}  {}  {}",
                s.id,
                s.profile,
                status_word(s.state, lang),
                s.dir
            );
            // 「在干什么」可能是空的（比如刚起来、或者已经停了），空的就不拖
            // 一条尾巴出来
            if s.activity.trim().is_empty() {
                line
            } else {
                format!("{line}  {}", s.activity.trim())
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// 连上已经在跑的守护进程。**连不上就是连不上**，不拉起新的——见模块注释。
fn connect(sock: &Path) -> Option<Client> {
    Client::connect(sock).ok()
}

pub fn run_ps(sock: &Path, lang: Lang) -> Result<()> {
    let Some(mut c) = connect(sock) else {
        println!("{}", text(Key::NoDaemonRunning, lang));
        return Ok(());
    };
    match c.call(Request::List)? {
        Response::Sessions(v) => {
            println!("{}", render_ps(&v, lang));
            Ok(())
        }
        other => anyhow::bail!("守护进程答非所问：{other:?}"),
    }
}

/// 返回值是进程退出码：有任何一个会话没停成就是非零。
///
/// 逐个停而不是遇错就停手：用户敲 `dct stop 3 4 5` 的意思是这三个都别跑了，
/// 3 号已经没了不该连累 4、5 还留着。
pub fn run_stop(sock: &Path, lang: Lang, target: Target) -> Result<i32> {
    run_on_targets(sock, lang, target, Force::No)
}

/// 跟 `run_stop` 走同一条路，只把请求换成 `Kill`。
///
/// 共用而不是抄一份：两条命令唯一的差别是发给守护进程的那一个请求，
/// 而**周围那一圈**（`--all` 要先问一遍谁还活着、逐个发不中途放弃、
/// 有一个失败就退非零）全都一样。抄一份的话，将来改退出码只会改对一半。
pub fn run_kill(sock: &Path, lang: Lang, target: Target) -> Result<i32> {
    run_on_targets(sock, lang, target, Force::Yes)
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Force {
    No,
    Yes,
}

fn run_on_targets(sock: &Path, lang: Lang, target: Target, force: Force) -> Result<i32> {
    let target = match target {
        Target::Usage(msg) => {
            eprintln!("{msg}");
            return Ok(2);
        }
        t => t,
    };

    let Some(mut c) = connect(sock) else {
        println!("{}", text(Key::NoDaemonRunning, lang));
        return Ok(0);
    };

    let ids = match target {
        // 「全部」= 所有还没停的。已经停了的不必再停一次，也不该因为
        // 它们而让退出码变成非零。
        Target::All => match c.call(Request::List)? {
            Response::Sessions(v) => v
                .iter()
                .filter(|s| s.state != SessionState::Stopped)
                .map(|s| s.id)
                .collect(),
            other => anyhow::bail!("守护进程答非所问：{other:?}"),
        },
        Target::Ids(v) => v,
        Target::Usage(_) => unreachable!("上面已经处理过了"),
    };

    if ids.is_empty() {
        println!("{}", text(Key::NoSessionsRunning, lang));
        return Ok(0);
    }

    let mut bad = 0;
    for id in ids {
        let req = match force {
            Force::No => Request::Stop { id },
            Force::Yes => Request::Kill { id },
        };
        match c.call(req) {
            Ok(Response::Ok) => println!(
                "{}",
                match force {
                    Force::No => crate::i18n::msg::stopped_session(lang, id),
                    Force::Yes => crate::i18n::msg::killed_session(lang, id),
                }
            ),
            Ok(Response::Error(ref e)) => {
                bad += 1;
                eprintln!("{}", crate::i18n::msg::error(lang, e));
            }
            Ok(other) => {
                bad += 1;
                eprintln!("守护进程答非所问：{other:?}");
            }
            Err(e) => {
                bad += 1;
                eprintln!("{e}");
            }
        }
    }
    Ok(if bad > 0 { 1 } else { 0 })
}

/// `dct prune`：把已经停掉的会话从名册上抹掉。
///
/// 不接参数、也没有 `--all`：这条命令本来就只对「已经停了的」下手，
/// 而那批东西不可能被误伤——它们已经不跑了。
pub fn run_prune(sock: &Path, lang: Lang) -> Result<()> {
    let Some(mut c) = connect(sock) else {
        println!("{}", text(Key::NoDaemonRunning, lang));
        return Ok(());
    };
    match c.call(Request::Prune)? {
        // 一个都没清跟清了几个是两句话：印「清掉 0 个」会让人以为
        // 命令没生效，而事实是本来就没有可清的。
        Response::Pruned(0) => println!("{}", text(Key::NothingToPrune, lang)),
        Response::Pruned(n) => println!("{}", crate::i18n::msg::pruned(lang, n)),
        other => anyhow::bail!("守护进程答非所问：{other:?}"),
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    /// **不给参数不能等于全停。** `dct stop` 是最容易手滑敲出来的形式，
    /// 而停会话撤不回来——默认全停的话，用户想停一个会把所有 agent 停光。
    #[test]
    fn a_bare_stop_asks_what_to_stop_instead_of_stopping_everything() {
        match parse_target_args(&args(&[]), Lang::Zh, "stop") {
            Target::Usage(m) => assert!(!m.trim().is_empty(), "得说清该怎么用"),
            other => panic!("空参数必须是用法提示，实际 {other:?}"),
        }
    }

    /// `kill` 更凶（不给收尾时间），同一条规矩更要成立。
    #[test]
    fn a_bare_kill_asks_what_to_kill_instead_of_killing_everything() {
        match parse_target_args(&args(&[]), Lang::Zh, "kill") {
            Target::Usage(m) => assert!(!m.trim().is_empty(), "得说清该怎么用"),
            other => panic!("空参数必须是用法提示，实际 {other:?}"),
        }
    }

    /// 用户敲的是哪条命令，用法提示里就得印哪条。印着 `dct stop 3` 去回答
    /// 一个敲了 `dct kill` 的人，等于把他推去解一个他没问的问题。
    #[test]
    fn the_usage_hint_names_the_command_you_actually_typed() {
        for (cmd, other) in [("kill", "stop"), ("stop", "kill")] {
            for bad in [args(&[]), args(&["--all", "3"])] {
                match parse_target_args(&bad, Lang::Zh, cmd) {
                    Target::Usage(m) => {
                        assert!(m.contains(&format!("dct {cmd}")), "提示里该印 {cmd}：{m}");
                        assert!(!m.contains(&format!("dct {other}")), "不该印 {other}：{m}");
                    }
                    other => panic!("该是用法提示，实际 {other:?}"),
                }
            }
        }
    }

    #[test]
    fn ids_are_parsed_in_the_order_given() {
        assert_eq!(
            parse_target_args(&args(&["3", "4", "5"]), Lang::Zh, "stop"),
            Target::Ids(vec![3, 4, 5])
        );
    }

    #[test]
    fn all_means_all() {
        assert_eq!(
            parse_target_args(&args(&["--all"]), Lang::Zh, "stop"),
            Target::All
        );
    }

    /// `--all` 和具体 id 混着给，说明用户自己也没想清楚。猜一个的代价是
    /// 停掉了他没打算停的东西。
    #[test]
    fn all_mixed_with_ids_is_refused_rather_than_guessed() {
        match parse_target_args(&args(&["--all", "3"]), Lang::Zh, "stop") {
            Target::Usage(_) => {}
            other => panic!("混着给必须拒绝，实际 {other:?}"),
        }
    }

    #[test]
    fn kill_all_takes_no_ids() {
        match parse_target_args(&args(&["--all", "3"]), Lang::Zh, "kill") {
            Target::Usage(m) => assert!(m.contains("kill"), "{m}"),
            other => panic!("混着给必须拒绝，实际 {other:?}"),
        }
    }

    #[test]
    fn a_non_number_says_so_instead_of_being_skipped() {
        match parse_target_args(&args(&["claude"]), Lang::Zh, "stop") {
            Target::Usage(m) => assert!(m.contains("claude"), "得点名是哪个参数不对：{m}"),
            other => panic!("非数字必须报错，实际 {other:?}"),
        }
    }

    fn s(id: u32, state: SessionState, activity: &str) -> SessionInfo {
        SessionInfo {
            id,
            profile: "claude".into(),
            dir: "/w/dc-terminal".into(),
            state,
            activity: activity.into(),
            is_agent: true,
        }
    }

    #[test]
    fn ps_lists_one_session_per_line_with_its_state() {
        let out = render_ps(
            &[
                s(1, SessionState::Working, "正在读 src/main.rs"),
                s(2, SessionState::Stopped, ""),
            ],
            Lang::Zh,
        );
        let lines: Vec<&str> = out.lines().collect();
        assert_eq!(lines.len(), 2, "一行一个会话：{out}");
        assert!(lines[0].contains('1') && lines[0].contains("干活中"));
        assert!(lines[0].contains("正在读 src/main.rs"));
        assert!(lines[1].contains("已停止"));
    }

    /// 没有会话时给一句话。打印一张只有表头的空表，用户会以为命令坏了。
    #[test]
    fn ps_with_nothing_running_says_so_in_words() {
        let out = render_ps(&[], Lang::Zh);
        assert!(!out.trim().is_empty());
        assert!(!out.contains('\n'), "一句话就够：{out}");
    }

    /// 「在干什么」是空的时候不要拖一条空尾巴——行尾多出来的空格在终端里
    /// 看不见，复制粘贴时才发现。
    #[test]
    fn an_empty_activity_does_not_leave_trailing_space() {
        let out = render_ps(&[s(1, SessionState::Stopped, "")], Lang::Zh);
        assert_eq!(out, out.trim_end(), "行尾不该有空格：{out:?}");
    }
}
