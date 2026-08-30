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

    git_env(dir, &["add", "-A"], &[("GIT_INDEX_FILE", &index)])?;
    let tree = git_env(dir, &["write-tree"], &[("GIT_INDEX_FILE", &index)])?;

    let head = git(dir, &["rev-parse", "HEAD"]).ok();
    let mut args: Vec<&str> = vec!["commit-tree", &tree, "-m", "dct checkpoint"];
    if let Some(h) = head.as_deref() {
        args.push("-p");
        args.push(h);
    }
    let commit = git(dir, &args)?;

    let refname = format!("refs/dct/{session}/{seq}");
    git(dir, &["update-ref", &refname, &commit])?;
    Ok(commit)
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
        run(&["config", "user.email", "t@example.com"]);
        run(&["config", "user.name", "t"]);
        fs::write(p.join("a.txt"), "hello\n").unwrap();
        run(&["add", "-A"]);
        run(&["commit", "-q", "-m", "init"]);
        dir
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
