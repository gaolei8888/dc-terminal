//! 把「怎么让 AI 玩三消」的说明卡装进 Claude Code、Codex、千问各自的 skills 目录。三家都认
//! `<家目录>/.<agent>/skills/<名字>/SKILL.md` 这个格式。卡片里带一行 dct 的标记：有标记的我们可以更新，
//! 没有标记的同名文件是用户自己写的，绝不覆盖。
use std::io;
use std::path::Path;

pub const SKILL_MD: &str = include_str!("skill.md");
pub const MARKER: &str = "<!-- dct-managed: dct-game-skill v1 -->";
const AGENT_DIRS: [&str; 3] = [".claude", ".codex", ".qwen"];

#[derive(Debug, PartialEq, Eq)]
pub enum Installed {
    Wrote,
    Same,
    /// 同名文件不是我们写的，没动。
    UserOwned,
    /// 这个 agent 的目录不存在（没装过），不替它建。
    NoAgent,
}

fn install_one(home: &Path, agent_dir: &str) -> io::Result<Installed> {
    let root = home.join(agent_dir);
    if !root.is_dir() {
        return Ok(Installed::NoAgent);
    }
    let dir = root.join("skills").join("dct-game");
    let file = dir.join("SKILL.md");
    match std::fs::read_to_string(&file) {
        Ok(old) if !old.contains(MARKER) => return Ok(Installed::UserOwned),
        Ok(old) if old == SKILL_MD => return Ok(Installed::Same),
        Ok(_) => {}
        Err(e) if e.kind() == io::ErrorKind::NotFound => {}
        Err(e) => return Err(e),
    }
    std::fs::create_dir_all(&dir)?;
    // 先写同目录下的临时文件再改名：别的进程（agent）读到的要么是旧卡片，要么是完整的新卡片。
    let tmp = dir.join(format!(".SKILL.md.tmp-{}", std::process::id()));
    if let Err(e) = std::fs::write(&tmp, SKILL_MD).and_then(|_| std::fs::rename(&tmp, &file)) {
        let _ = std::fs::remove_file(&tmp);
        return Err(e);
    }
    Ok(Installed::Wrote)
}

/// 尽力而为：装不上不影响守护进程，所以逐个返回结果，由调用方决定要不要理。
pub fn install_all(home: &Path) -> Vec<(&'static str, io::Result<Installed>)> {
    AGENT_DIRS.iter().map(|d| (*d, install_one(home, d))).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn skill_path(home: &Path, d: &str) -> std::path::PathBuf {
        home.join(d).join("skills").join("dct-game").join("SKILL.md")
    }

    #[test]
    fn the_card_has_front_matter_on_line_one_and_the_marker() {
        assert!(SKILL_MD.starts_with("---\nname: dct-game\ndescription: "));
        assert!(SKILL_MD.contains(MARKER));
    }

    #[test]
    fn installs_into_every_agent_that_exists_and_skips_the_rest() {
        let h = tempfile::tempdir().unwrap();
        std::fs::create_dir(h.path().join(".claude")).unwrap();
        std::fs::create_dir(h.path().join(".qwen")).unwrap();
        let r = install_all(h.path());
        let got: Vec<_> = r.iter().map(|(d, x)| (*d, x.as_ref().unwrap())).collect();
        assert_eq!(got, vec![(".claude", &Installed::Wrote), (".codex", &Installed::NoAgent), (".qwen", &Installed::Wrote)]);
        assert_eq!(std::fs::read_to_string(skill_path(h.path(), ".claude")).unwrap(), SKILL_MD);
        assert!(!h.path().join(".codex").exists(), "不替没装的 agent 建目录");
    }

    #[test]
    fn installing_leaves_no_temp_file_behind() {
        let h = tempfile::tempdir().unwrap();
        std::fs::create_dir(h.path().join(".claude")).unwrap();
        install_all(h.path());
        let dir = skill_path(h.path(), ".claude").parent().unwrap().to_path_buf();
        let names: Vec<_> = std::fs::read_dir(&dir).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect();
        assert_eq!(names, vec!["SKILL.md"]);
    }

    #[test]
    fn the_card_does_not_promise_a_speed_we_have_not_measured() {
        assert!(!SKILL_MD.contains("不到一秒") && SKILL_MD.contains("每步很快"));
    }

    #[test]
    fn a_second_install_writes_nothing() {
        let h = tempfile::tempdir().unwrap();
        std::fs::create_dir(h.path().join(".codex")).unwrap();
        install_all(h.path());
        let before = std::fs::metadata(skill_path(h.path(), ".codex")).unwrap().modified().unwrap();
        std::thread::sleep(std::time::Duration::from_millis(20));
        assert_eq!(install_all(h.path())[1].1.as_ref().unwrap(), &Installed::Same);
        assert_eq!(std::fs::metadata(skill_path(h.path(), ".codex")).unwrap().modified().unwrap(), before);
    }

    #[test]
    fn an_old_version_of_our_card_is_updated() {
        let h = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(skill_path(h.path(), ".claude").parent().unwrap()).unwrap();
        std::fs::write(skill_path(h.path(), ".claude"), format!("旧的\n{MARKER}\n")).unwrap();
        assert_eq!(install_all(h.path())[0].1.as_ref().unwrap(), &Installed::Wrote);
        assert_eq!(std::fs::read_to_string(skill_path(h.path(), ".claude")).unwrap(), SKILL_MD);
    }

    #[test]
    fn a_users_own_file_with_the_same_name_is_never_overwritten() {
        let h = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(skill_path(h.path(), ".claude").parent().unwrap()).unwrap();
        std::fs::write(skill_path(h.path(), ".claude"), "我自己写的").unwrap();
        assert_eq!(install_all(h.path())[0].1.as_ref().unwrap(), &Installed::UserOwned);
        assert_eq!(std::fs::read_to_string(skill_path(h.path(), ".claude")).unwrap(), "我自己写的");
    }
}
