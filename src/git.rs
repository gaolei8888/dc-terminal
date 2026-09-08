use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::process::Command;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct FileStat {
    pub path: String,
    pub added: usize,
    pub removed: usize,
}

/// 所有 git 调用的唯一出口。**`no_console` 必须走这里**：检查点是守护进程
/// 干的，而守护进程没有控制台，少了那一句每敲一次回车 Windows 就闪一排黑
/// 窗口（理由写在 `sys::proc::no_console`）。
fn cmd(dir: &Path) -> Command {
    let mut c = Command::new("git");
    c.current_dir(dir);
    crate::sys::proc::no_console(&mut c);
    c
}

fn git(dir: &Path, args: &[&str]) -> Result<String> {
    let out = cmd(dir)
        .args(args)
        .output()
        .with_context(|| format!("执行 git {args:?} 失败"))?;
    if !out.status.success() {
        // 不要把命令数组和 git 的英文原文原样甩到界面上——用户看不懂，
        // 也不知道该做什么。调用方负责给出中文的上下文。
        bail!("{}", String::from_utf8_lossy(&out.stderr).trim());
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

fn git_env(dir: &Path, args: &[&str], env: &[(&str, &str)]) -> Result<String> {
    let mut c = cmd(dir);
    c.args(args);
    for (k, v) in env {
        c.env(k, v);
    }
    let out = c
        .output()
        .with_context(|| format!("执行 git {args:?} 失败"))?;
    if !out.status.success() {
        bail!("{}", String::from_utf8_lossy(&out.stderr).trim());
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

pub fn is_repo(dir: &Path) -> bool {
    cmd(dir)
        .args(["rev-parse", "--is-inside-work-tree"])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// 这台机器上有没有一个**跑得起来的** git。
///
/// **必须真的跑一次 `git --version`，不能只问 PATH 上有没有这个名字。**
/// macOS 上 `/usr/bin/git` 是个占位的壳：Xcode 命令行工具没装时它照样在
/// PATH 上、`command_exists` 照样说有，真跑起来才会弹一个安装窗口出来。
/// 只查名字的话，这条检查在最需要它的那台机器上恰好是失灵的。
/// （`scripts/install.sh` 的 `check_git` 早就是这么写的，理由同一条。）
///
/// 为什么单独有这个函数：`is_repo` 分不出「这儿不是仓库」和「这台机器上
/// 没有 git」——两种情况它都返回 `false`，因为后者 `output()` 直接是
/// `Err`。而这两句话对用户来说是完全不同的两件事，给出的下一步也不一样
/// （前者按 `g` 建仓库，后者按 `g` 只会再失败一次）。
///
/// 目录用当前目录即可：问的是「git 这个程序在不在」，跟在哪儿问无关。
pub fn available() -> bool {
    let mut c = Command::new("git");
    crate::sys::proc::no_console(&mut c);
    c.arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// 在这个目录上建一个 git 仓库。
///
/// **只该在 `is_repo()` 说"不是"的时候调。** 那个判断走的是
/// `rev-parse --is-inside-work-tree`，父目录是仓库时它也为真——所以
/// "`is_repo` 为假"同时意味着"往上一级也没有仓库"，这里不可能建出一个
/// 嵌套在别人工作区里的仓库来。这一层保证是调用方的（`ui::pick` 的 `g`
/// 只在那种情况下才写得出来），不是这个函数自己的。
///
/// 不 `git commit`：空仓库就够 agent 干活了（检查点是游离 commit，不依赖
/// HEAD 上已经有东西）。替用户造一个他没要过的首次提交是另一回事。
pub fn init(dir: &Path) -> Result<()> {
    git(dir, &["init"])?;
    Ok(())
}

/// 检查点用 **dct 自己的身份**，不借用户配的那个。
///
/// **这不是图省事，是因为借不到。** `commit-tree` 一定要一个身份；拿不到
/// `user.email` 时 git 会去猜 `<用户名>@<主机名>`，而主机名没有域的机器上
/// 那一猜是失败的，git 直接 fatal：
///
/// ```text
/// fatal: unable to auto-detect email address (got 'dc@4c2120397151.(none)')
/// ```
///
/// 容器里这是**必然**的（2026-09-07 实测），刚装完 git、还没跑过
/// `git config --global user.email` 的机器上同样必然——**而那正是这个产品
/// 服务的那批人**。
///
/// 后果不是「少拍一张快照」那么轻：`Session::create` 里第一张检查点拍不上
/// 就直接拒绝开会话（`Operation::FirstCheckpoint`），于是那台机器上**一个
/// agent 会话都开不出来**，界面只说一句「拍不了检查点」。
///
/// 用 dct 自己的名字也更诚实：这个 commit 不是用户写的，它挂在 `refs/dct/`
/// 底下，用户的 `git log` 里根本看不见。**它不该顶着用户的名字。**
///
/// 这里只管 dct 自己造的 commit。用户（或者 agent）自己敲的 `git commit`
/// 仍然要用户自己的身份——那是他真实的提交历史，替他编一个署名比报错更坏。
const CHECKPOINT_IDENTITY: &[(&str, &str)] = &[
    ("GIT_AUTHOR_NAME", "dct"),
    ("GIT_AUTHOR_EMAIL", "dct@localhost"),
    ("GIT_COMMITTER_NAME", "dct"),
    ("GIT_COMMITTER_EMAIL", "dct@localhost"),
];

/// 给项目当前状态拍一张隐藏快照，返回快照的 commit sha。
///
/// **不动用户的分支、提交历史和暂存区**——agent 在你的真项目里干活，
/// 检查点不能顺手往你的历史里塞一堆 commit。做法是用一个临时索引把当前
/// 全部内容（含未跟踪文件，尊重 .gitignore）写成一个 tree，再造一个游离的
/// commit，最后用 refs/dct/... 挂住防止被 gc 回收。用户 `git log` 里看不到它。
pub fn checkpoint(dir: &Path, session: u32, seq: usize) -> Result<String> {
    let index = git(dir, &["rev-parse", "--git-dir"])
        .map(|d| dir.join(d).join(format!("dct-index-{session}")))?;
    let index = index
        .to_str()
        .context("索引路径不是合法 UTF-8")?
        .to_string();

    // 上一次 git 被硬杀在 `add` 中途（守护进程被任务管理器结束、机器休眠、
    // 崩溃）会留下一个 `dct-index-N.lock`，而 git 见到锁就一直失败——**那把
    // 锁不会自己消失**，于是这个会话从此每一轮都拍不上快照。撞上就清掉重来
    // 一次。
    //
    // 清它是安全的：`dct-index-N` 是 dct 自己的临时索引，只有本会话的检查点
    // 写它，用户真正的 `.git/index` 从头到尾没被碰过（全程 `GIT_INDEX_FILE`
    // 指开了）。最坏情况也只是一张快照算错，而快照本来就是可以重拍的东西。
    if let Err(e) = git_env(dir, &["add", "-A"], &[("GIT_INDEX_FILE", &index)]) {
        if !clear_index_lock(&index) {
            return Err(e);
        }
        git_env(dir, &["add", "-A"], &[("GIT_INDEX_FILE", &index)])?;
    }
    let tree = git_env(dir, &["write-tree"], &[("GIT_INDEX_FILE", &index)])?;

    let head = git(dir, &["rev-parse", "HEAD"]).ok();
    let mut args: Vec<&str> = vec!["commit-tree", &tree, "-m", "dct checkpoint"];
    if let Some(h) = head.as_deref() {
        args.push("-p");
        args.push(h);
    }
    let commit = git_env(dir, &args, CHECKPOINT_IDENTITY)?;

    let refname = format!("refs/dct/{session}/{seq}");
    git(dir, &["update-ref", &refname, &commit])?;
    Ok(commit)
}

/// 把临时索引旁边那把锁删掉。真删掉了才返回 `true`——**没有锁不算删掉**，
/// 不然 `checkpoint` 会拿「其实是别的原因失败」当成「清完了可以重来」，
/// 白跑一遍再报同一个错。
fn clear_index_lock(index: &str) -> bool {
    let lock = std::path::PathBuf::from(format!("{index}.lock"));
    lock.exists() && std::fs::remove_file(&lock).is_ok()
}

/// 恢复到某张快照：工作区内容和暂存区都回到拍照那一刻，
/// 快照之后新建的文件被清掉。分支和提交历史不受影响。
pub fn restore(dir: &Path, commit: &str) -> Result<()> {
    let tree = format!("{commit}^{{tree}}");
    git(dir, &["read-tree", "-u", "--reset", &tree])?;
    git(dir, &["clean", "-fdq"])?;
    Ok(())
}

pub fn diff_stat(dir: &Path, base: &str) -> Result<Vec<FileStat>> {
    // 标记新文件意图（仅登记，不真正暂存），这样未跟踪的新文件也会出现在 diff 里
    let _ = git(dir, &["add", "-N", "."]);
    let out = git(dir, &["diff", "--numstat", base])?;
    let mut stats = Vec::new();
    for line in out.lines() {
        let cols: Vec<&str> = line.split('\t').collect();
        if cols.len() != 3 {
            continue;
        }
        stats.push(FileStat {
            added: cols[0].parse().unwrap_or(0),
            removed: cols[1].parse().unwrap_or(0),
            path: cols[2].to_string(),
        });
    }
    Ok(stats)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::process::Command;

    fn init_repo() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path();
        let run = |args: &[&str]| {
            Command::new("git")
                .args(args)
                .current_dir(p)
                .output()
                .unwrap();
        };
        run(&["init", "-q"]);
        // **这两行是一个真 bug 藏了三个月的地方。** 夹具替被测代码把唯一
        // 会出事的条件抹平了：现场大量机器上 `user.email` 根本没配，而
        // `commit-tree` 没身份就 fatal。见 `init_repo_without_identity`。
        run(&["config", "user.email", "t@example.com"]);
        run(&["config", "user.name", "t"]);
        fs::write(p.join("a.txt"), "hello\n").unwrap();
        run(&["add", "-A"]);
        run(&["commit", "-q", "-m", "init"]);
        dir
    }

    /// 一个**没配 git 身份**的仓库，跟 `init_repo` 只差那两行 config
    /// （也因此没有初始 commit——建 commit 本身就要身份）。
    ///
    /// 现场什么时候长这样：容器里（2026-09-07 实测），以及任何刚装完 git、
    /// 还没跑过 `git config --global user.email` 的机器——**正是这个产品
    /// 服务的那批人**。
    fn init_repo_without_identity() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path();
        Command::new("git")
            .args(["init", "-q"])
            .current_dir(p)
            .output()
            .unwrap();
        fs::write(p.join("a.txt"), "hello\n").unwrap();
        dir
    }

    /// **这一条才是真正钉住修复的那个。**
    ///
    /// 下面那条「没身份也拍得上」在一台配了全局 `user.email` 的开发机上，
    /// 就算把修复撤掉也照样绿（git 会去用全局那个）。而这一条不会：环境
    /// 变量压过一切配置，署名不是 `dct` 就说明 `CHECKPOINT_IDENTITY` 没生效；
    /// 而在一台没有任何身份的机器上，撤掉修复连 `checkpoint` 都会直接失败。
    /// 两种机器上它都红。
    #[test]
    fn a_checkpoint_is_signed_by_dct_not_by_whoever_owns_the_machine() {
        let dir = init_repo();
        let sha = checkpoint(dir.path(), 1, 0).unwrap();
        let who = git(dir.path(), &["show", "-s", "--format=%an <%ae>", &sha]).unwrap();
        assert_eq!(
            who, "dct <dct@localhost>",
            "检查点是 dct 自己造的对象，不该顶着用户的名字"
        );
    }

    /// 没有 git 身份的机器上，检查点和撤销都必须照常。
    ///
    /// 拍不上第一张检查点的后果不是「少一张快照」：`Session::create` 会直接
    /// 拒绝开会话（`Operation::FirstCheckpoint`），于是那台机器上**一个
    /// agent 会话都开不出来**。
    #[test]
    fn checkpoints_and_undo_work_with_no_git_identity_configured() {
        let dir = init_repo_without_identity();
        let sha = checkpoint(dir.path(), 1, 0).expect("没配 git 身份也必须拍得上检查点");
        fs::write(dir.path().join("a.txt"), "changed\n").unwrap();
        fs::write(dir.path().join("new.txt"), "junk\n").unwrap();
        restore(dir.path(), &sha).unwrap();
        assert_eq!(
            fs::read_to_string(dir.path().join("a.txt")).unwrap(),
            "hello\n",
            "撤销没把内容还原回去"
        );
        assert!(
            !dir.path().join("new.txt").exists(),
            "撤销没清掉快照之后新建的文件"
        );
    }

    fn git_out(dir: &Path, args: &[&str]) -> String {
        let o = Command::new("git")
            .args(args)
            .current_dir(dir)
            .output()
            .unwrap();
        String::from_utf8_lossy(&o.stdout).trim().to_string()
    }

    #[test]
    fn detects_repo() {
        let repo = init_repo();
        assert!(is_repo(repo.path()));
        let plain = tempfile::tempdir().unwrap();
        assert!(!is_repo(plain.path()));
    }

    #[test]
    fn restore_undoes_changes_including_new_files() {
        let repo = init_repo();
        let base = checkpoint(repo.path(), 1, 0).unwrap();

        fs::write(repo.path().join("a.txt"), "agent 改的\n").unwrap();
        fs::write(repo.path().join("new.txt"), "agent 新建的\n").unwrap();

        restore(repo.path(), &base).unwrap();

        assert_eq!(
            fs::read_to_string(repo.path().join("a.txt")).unwrap(),
            "hello\n"
        );
        assert!(
            !repo.path().join("new.txt").exists(),
            "新建的文件必须被清掉"
        );
    }

    #[test]
    fn checkpoint_does_not_touch_branch_or_history() {
        // 这是"在真项目里干活"的前提：检查点不能往用户的历史里塞东西
        let repo = init_repo();
        let before_head = git_out(repo.path(), &["rev-parse", "HEAD"]);
        let before_count = git_out(repo.path(), &["rev-list", "--count", "HEAD"]);
        let before_branch = git_out(repo.path(), &["rev-parse", "--abbrev-ref", "HEAD"]);

        fs::write(repo.path().join("a.txt"), "改了\n").unwrap();
        checkpoint(repo.path(), 7, 0).unwrap();
        fs::write(repo.path().join("b.txt"), "又加了一个\n").unwrap();
        checkpoint(repo.path(), 7, 1).unwrap();

        assert_eq!(git_out(repo.path(), &["rev-parse", "HEAD"]), before_head);
        assert_eq!(
            git_out(repo.path(), &["rev-list", "--count", "HEAD"]),
            before_count,
            "用户的提交历史里不能多出任何东西"
        );
        assert_eq!(
            git_out(repo.path(), &["rev-parse", "--abbrev-ref", "HEAD"]),
            before_branch
        );
        // 工作区内容也不能被检查点动过
        assert_eq!(
            fs::read_to_string(repo.path().join("a.txt")).unwrap(),
            "改了\n"
        );
        assert!(repo.path().join("b.txt").exists());
    }

    /// 上一次 git 被硬杀留下的那把锁，不能让这个会话的快照从此全废。
    ///
    /// 现场是「回车按了没反应」那条 bug 的另一半来源：锁不会自己消失，
    /// 于是这个会话每一轮 `git add -A` 都失败，而快照失败在修好之前
    /// 会把回车一起吃掉（见 `session::send_input`）。
    #[test]
    fn a_leftover_index_lock_does_not_kill_this_sessions_checkpoints() {
        let repo = init_repo();
        // 先拍一张，把 dct-index-9 建出来——现场那把锁就是躺在它旁边的。
        checkpoint(repo.path(), 9, 0).unwrap();
        let lock = repo.path().join(".git").join("dct-index-9.lock");
        fs::write(&lock, "上一次 git 被杀在这儿了\n").unwrap();

        fs::write(repo.path().join("a.txt"), "锁着也得拍上\n").unwrap();
        let snap = checkpoint(repo.path(), 9, 1).expect("撞上残留的锁要能自己缓过来");

        assert!(!lock.exists(), "锁清掉了才算数，不然下一轮还是同一个死结");
        // 拍到的得是真内容，不是「装作成功」。
        fs::write(repo.path().join("a.txt"), "又改了\n").unwrap();
        restore(repo.path(), &snap).unwrap();
        assert_eq!(
            fs::read_to_string(repo.path().join("a.txt")).unwrap(),
            "锁着也得拍上\n"
        );
    }

    #[test]
    fn checkpoint_snapshots_are_kept_alive_after_gc() {
        // 快照是游离 commit，必须靠 refs/dct 挂住，否则 gc 一跑撤销就失效
        let repo = init_repo();
        fs::write(repo.path().join("a.txt"), "v1\n").unwrap();
        let snap = checkpoint(repo.path(), 3, 0).unwrap();

        Command::new("git")
            .args(["gc", "--prune=now", "--aggressive", "-q"])
            .current_dir(repo.path())
            .output()
            .unwrap();

        fs::write(repo.path().join("a.txt"), "v2\n").unwrap();
        restore(repo.path(), &snap).expect("gc 之后快照必须还在");
        assert_eq!(
            fs::read_to_string(repo.path().join("a.txt")).unwrap(),
            "v1\n"
        );
    }

    #[test]
    fn diff_stat_reports_changes() {
        let repo = init_repo();
        let base = checkpoint(repo.path(), 4, 0).unwrap();
        fs::write(repo.path().join("a.txt"), "hello\nworld\n").unwrap();

        let stats = diff_stat(repo.path(), &base).unwrap();
        assert_eq!(stats.len(), 1);
        assert_eq!(stats[0].path, "a.txt");
        assert_eq!(stats[0].added, 1);
        assert_eq!(stats[0].removed, 0);
    }

    #[test]
    fn diff_stat_includes_untracked_new_files() {
        let repo = init_repo();
        let base = checkpoint(repo.path(), 5, 0).unwrap();
        fs::write(repo.path().join("brand-new.txt"), "one\ntwo\n").unwrap();

        let stats = diff_stat(repo.path(), &base).unwrap();
        assert_eq!(
            stats.len(),
            1,
            "新建文件必须出现在 diff 里，实际: {stats:?}"
        );
        assert_eq!(stats[0].path, "brand-new.txt");
        assert_eq!(stats[0].added, 2);
    }
}
