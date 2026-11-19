//! 常驻会话：`~/.dct/config.toml` 里的 `[[keep]]`。
//!
//! 这里**只**读 `keep` 这一个键，不碰 `[llm]` 等其它段，也不走 `Config` 的
//! 整体反序列化：一条写坏的 `[[keep]]` 只该丢掉它自己，不能拖垮别的条目，
//! 更不能让 `[llm]` 读不出来（反过来也一样：`Config::load` 不认识 `keep`
//! 这个键，照样读 `[llm]`）。坏条目跳过并回报 `WarningCode::KeepEntryInvalid`。

use crate::proto::{KeepEntryProblem, WarningCode};
use std::path::{Path, PathBuf};

/// 一条常驻会话的声明。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Keep {
    pub name: String,
    pub profile: String,
    /// 已展开 `~`。
    pub dir: PathBuf,
}

fn expand(dir: &str, home: &Path) -> PathBuf {
    if dir == "~" {
        home.to_path_buf()
    } else if let Some(rest) = dir.strip_prefix("~/") {
        home.join(rest)
    } else {
        PathBuf::from(dir)
    }
}

/// 取一个非空字符串字段；缺了、类型不对、全空白都算缺。
fn text_field(t: &toml::Table, key: &str) -> Option<String> {
    let s = t.get(key)?.as_str()?.trim();
    (!s.is_empty()).then(|| s.to_string())
}

/// 只解析 config.toml 源文本里的 `keep` 数组。坏条目跳过并回报，
/// 重名保留先写的那条。整份文本解析不了时返回空（`Config::load` 已经报过）。
pub fn parse(src: &str, home: &Path) -> (Vec<Keep>, Vec<WarningCode>) {
    let Ok(root) = src.parse::<toml::Table>() else {
        return (Vec::new(), Vec::new());
    };
    let Some(keep) = root.get("keep") else {
        return (Vec::new(), Vec::new());
    };
    let invalid = |index: usize, reason: KeepEntryProblem| WarningCode::KeepEntryInvalid {
        index,
        reason,
    };
    let Some(entries) = keep.as_array() else {
        return (Vec::new(), vec![invalid(0, KeepEntryProblem::NotATable)]);
    };
    let mut out: Vec<Keep> = Vec::new();
    let mut warnings = Vec::new();
    for (i, entry) in entries.iter().enumerate() {
        let index = i + 1;
        let Some(t) = entry.as_table() else {
            warnings.push(invalid(index, KeepEntryProblem::NotATable));
            continue;
        };
        // 名字写了但全是空白，要说「名字是空的」，不是「没写名字」
        if t.get("name")
            .and_then(|v| v.as_str())
            .is_some_and(|s| s.trim().is_empty())
        {
            warnings.push(invalid(index, KeepEntryProblem::EmptyName));
            continue;
        }
        let mut fields = Vec::with_capacity(3);
        let mut missing = None;
        for key in ["name", "profile", "dir"] {
            match text_field(t, key) {
                Some(v) => fields.push(v),
                None => {
                    missing = Some(key);
                    break;
                }
            }
        }
        if let Some(key) = missing {
            warnings.push(invalid(index, KeepEntryProblem::MissingField(key.into())));
            continue;
        }
        let [name, profile, dir]: [String; 3] = fields.try_into().expect("三个字段");
        if out.iter().any(|k| k.name == name) {
            warnings.push(invalid(index, KeepEntryProblem::DuplicateName(name)));
            continue;
        }
        out.push(Keep {
            name,
            profile,
            dir: expand(&dir, home),
        });
    }
    (out, warnings)
}

/// 读文件。不存在、读不了、解析不了都是「没有常驻会话」且不报警：
/// 文件坏了这件事 `Config::load` 已经说过一次。
pub fn load(config_path: &Path, home: &Path) -> (Vec<Keep>, Vec<WarningCode>) {
    match std::fs::read_to_string(config_path) {
        Ok(src) => parse(&src, home),
        Err(_) => (Vec::new(), Vec::new()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn home() -> PathBuf {
        PathBuf::from("/home/u")
    }

    fn problem(w: &WarningCode) -> (usize, &KeepEntryProblem) {
        match w {
            WarningCode::KeepEntryInvalid { index, reason } => (*index, reason),
            other => panic!("应当是 KeepEntryInvalid：{other:?}"),
        }
    }

    #[test]
    fn two_good_entries_are_read_in_order() {
        let src = r#"
[[keep]]
name = "a"
profile = "claude"
dir = "/w/a"
[[keep]]
name = "b"
profile = "codex"
dir = "/w/b"
"#;
        let (k, w) = parse(src, &home());
        assert!(w.is_empty());
        assert_eq!(
            k,
            vec![
                Keep {
                    name: "a".into(),
                    profile: "claude".into(),
                    dir: "/w/a".into()
                },
                Keep {
                    name: "b".into(),
                    profile: "codex".into(),
                    dir: "/w/b".into()
                },
            ]
        );
    }

    #[test]
    fn tilde_is_expanded_against_the_given_home() {
        let src = r#"
[[keep]]
name = "a"
profile = "p"
dir = "~/work/x"
[[keep]]
name = "b"
profile = "p"
dir = "~"
"#;
        let (k, _) = parse(src, &home());
        assert_eq!(k[0].dir, PathBuf::from("/home/u/work/x"));
        assert_eq!(k[1].dir, PathBuf::from("/home/u"));
    }

    #[test]
    fn an_entry_missing_a_field_is_skipped_and_the_rest_survive() {
        let src = r#"
[[keep]]
name = "a"
dir = "/w/a"
[[keep]]
name = "b"
profile = "p"
dir = "/w/b"
"#;
        let (k, w) = parse(src, &home());
        assert_eq!(k.len(), 1);
        assert_eq!(k[0].name, "b");
        assert_eq!(w.len(), 1);
        assert_eq!(
            problem(&w[0]),
            (1, &KeepEntryProblem::MissingField("profile".into()))
        );
    }

    #[test]
    fn a_blank_name_is_reported_as_empty() {
        let src = "[[keep]]\nname = \"  \"\nprofile = \"p\"\ndir = \"/w\"\n";
        let (k, w) = parse(src, &home());
        assert!(k.is_empty());
        assert_eq!(problem(&w[0]), (1, &KeepEntryProblem::EmptyName));
    }

    #[test]
    fn a_repeated_name_keeps_the_first() {
        let src = r#"
[[keep]]
name = "a"
profile = "p1"
dir = "/w/1"
[[keep]]
name = "a"
profile = "p2"
dir = "/w/2"
"#;
        let (k, w) = parse(src, &home());
        assert_eq!(k.len(), 1);
        assert_eq!(k[0].profile, "p1");
        assert_eq!(
            problem(&w[0]),
            (2, &KeepEntryProblem::DuplicateName("a".into()))
        );
    }

    #[test]
    fn no_keep_key_is_empty_and_silent() {
        let (k, w) = parse("[llm]\nprovider = \"claude\"\n", &home());
        assert!(k.is_empty() && w.is_empty());
    }

    #[test]
    fn a_keep_of_the_wrong_type_warns_once_and_does_not_panic() {
        let (k, w) = parse("keep = 3\n", &home());
        assert!(k.is_empty());
        assert_eq!(w.len(), 1);
        let (k, w) = parse("keep = [1, 2]\n", &home());
        assert!(k.is_empty());
        assert_eq!(w.len(), 2);
    }

    #[test]
    fn other_sections_are_ignored() {
        let src = r#"
[llm]
provider = "claude"
[menu]
hide = ["x"]
[[keep]]
name = "a"
profile = "p"
dir = "/w"
"#;
        let (k, w) = parse(src, &home());
        assert_eq!(k.len(), 1);
        assert!(w.is_empty());
    }

    #[test]
    fn load_treats_missing_and_unparseable_files_as_empty() {
        let d = tempfile::tempdir().unwrap();
        assert_eq!(load(&d.path().join("nope.toml"), &home()), (vec![], vec![]));
        let p = d.path().join("c.toml");
        std::fs::write(&p, "[[keep\nname =").unwrap();
        assert_eq!(load(&p, &home()), (vec![], vec![]));
        std::fs::write(&p, "[[keep]]\nname=\"a\"\nprofile=\"p\"\ndir=\"~/x\"\n").unwrap();
        let (k, _) = load(&p, &home());
        assert_eq!(k[0].dir, PathBuf::from("/home/u/x"));
    }
}
