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

fn git(dir: &Path, args: &[&str]) -> Result<String> {
    let out = Command::new("git")
        .args(args)
        .current_dir(dir)
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
    let mut cmd = Command::new("git");
    cmd.args(args).current_dir(dir);
    for (k, v) in env {
        cmd.env(k, v);
    }
    let out = cmd
        .output()
        .with_context(|| format!("执行 git {args:?} 失败"))?;
    if !out.status.success() {
        bail!("{}", String::from_utf8_lossy(&out.stderr).trim());
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

pub fn is_repo(dir: &Path) -> bool {
    Command::new("git")
        .args(["rev-parse", "--is-inside-work-tree"])
        .current_dir(dir)
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
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
