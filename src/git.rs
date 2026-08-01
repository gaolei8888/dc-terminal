use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::process::Command;

#[derive(Debug, Clone)]
pub struct Worktree {
    pub path: PathBuf,
    pub branch: String,
    pub repo: PathBuf,
}

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
        bail!(
            "git {:?} 失败: {}",
            args,
            String::from_utf8_lossy(&out.stderr).trim()
        );
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

pub fn create_worktree(repo: &Path, name: &str) -> Result<Worktree> {
    if !is_repo(repo) {
        bail!("{} 不是 git 仓库，无法开 agent 会话", repo.display());
    }
    let root = PathBuf::from(git(repo, &["rev-parse", "--show-toplevel"])?);
    let branch = format!("dct/{name}");
    // 放在 .git 里面：主工作树的 git status 看不见它，git clean -fd 也不会误删
    let path = root.join(".git").join("dct-worktrees").join(name);

    git(
        &root,
        &[
            "worktree",
            "add",
            "-q",
            "-b",
            &branch,
            path.to_str().context("worktree 路径不是合法 UTF-8")?,
        ],
    )?;

    Ok(Worktree {
        path,
        branch,
        repo: root,
    })
}

pub fn remove_worktree(wt: &Worktree) -> Result<()> {
    git(
        &wt.repo,
        &[
            "worktree",
            "remove",
            "--force",
            wt.path.to_str().context("worktree 路径不是合法 UTF-8")?,
        ],
    )?;
    let _ = git(&wt.repo, &["branch", "-D", &wt.branch]);
    Ok(())
}

pub fn checkpoint(wt: &Worktree, label: &str) -> Result<String> {
    git(&wt.path, &["add", "-A"])?;
    let dirty = !git(&wt.path, &["status", "--porcelain"])?.is_empty();
    if dirty {
        git(
            &wt.path,
            &["commit", "-q", "-m", &format!("checkpoint: {label}")],
        )?;
    }
    git(&wt.path, &["rev-parse", "HEAD"])
}

pub fn reset_to(wt: &Worktree, sha: &str) -> Result<()> {
    git(&wt.path, &["reset", "--hard", "-q", sha])?;
    git(&wt.path, &["clean", "-fdq"])?;
    Ok(())
}

pub fn diff_stat(wt: &Worktree, base: &str) -> Result<Vec<FileStat>> {
    let out = git(&wt.path, &["diff", "--numstat", base])?;
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

    #[test]
    fn detects_repo() {
        let repo = init_repo();
        assert!(is_repo(repo.path()));
        let plain = tempfile::tempdir().unwrap();
        assert!(!is_repo(plain.path()));
    }

    #[test]
    fn creates_and_removes_worktree() {
        let repo = init_repo();
        let wt = create_worktree(repo.path(), "s1").unwrap();
        assert!(wt.path.join("a.txt").exists());
        assert_eq!(wt.branch, "dct/s1");
        assert!(wt.path.to_string_lossy().contains("dct-worktrees"));
        remove_worktree(&wt).unwrap();
        assert!(!wt.path.exists());
    }

    #[test]
    fn checkpoint_then_reset_discards_changes() {
        let repo = init_repo();
        let wt = create_worktree(repo.path(), "s2").unwrap();

        let base = checkpoint(&wt, "before").unwrap();

        fs::write(wt.path.join("a.txt"), "changed\n").unwrap();
        fs::write(wt.path.join("new.txt"), "new\n").unwrap();

        reset_to(&wt, &base).unwrap();

        assert_eq!(
            fs::read_to_string(wt.path.join("a.txt")).unwrap(),
            "hello\n"
        );
        assert!(!wt.path.join("new.txt").exists());
    }

    #[test]
    fn checkpoint_commits_pending_changes() {
        let repo = init_repo();
        let wt = create_worktree(repo.path(), "s3").unwrap();
        let first = checkpoint(&wt, "c0").unwrap();

        fs::write(wt.path.join("a.txt"), "v2\n").unwrap();
        let second = checkpoint(&wt, "c1").unwrap();

        assert_ne!(first, second);
        // 提交之后工作区干净，再 checkpoint 应当返回同一个 sha
        let third = checkpoint(&wt, "c2").unwrap();
        assert_eq!(second, third);
    }

    #[test]
    fn diff_stat_reports_changes() {
        let repo = init_repo();
        let wt = create_worktree(repo.path(), "s4").unwrap();
        let base = checkpoint(&wt, "c0").unwrap();
        fs::write(wt.path.join("a.txt"), "hello\nworld\n").unwrap();

        let stats = diff_stat(&wt, &base).unwrap();
        assert_eq!(stats.len(), 1);
        assert_eq!(stats[0].path, "a.txt");
        assert_eq!(stats[0].added, 1);
        assert_eq!(stats[0].removed, 0);
    }
}
