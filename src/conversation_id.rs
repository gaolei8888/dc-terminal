//! 给 agent 的对话起一个 id：dct 开会话时自己定，重启后按它接回。
//! 格式是 UUID v4——claude 的 `--session-id` 只收合法 UUID。
//!
//! 对话 id 在 agent 里会变（claude 里 `/clear`、`/resume`），或者 dct 起会话时
//! 根本没给（走的是 `--continue`）。所以还能从 agent 自己为每个进程写的小文件
//! 里读回它此刻的 id，见 `read_from_file`。

use std::path::{Path, PathBuf};

/// 新起一个对话 id：UUID v4，小写、带连字符、36 个字符。
pub fn new() -> String {
    let mut b = [0u8; 16];
    // 拿不到系统随机数是极端情况；退回时间戳拼一个，宁可接不回也不让开会话失败。
    if getrandom::getrandom(&mut b).is_err() {
        let n = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        b.copy_from_slice(&n.to_le_bytes());
    }
    b[6] = (b[6] & 0x0f) | 0x40;
    b[8] = (b[8] & 0x3f) | 0x80;
    let h: String = b.iter().map(|x| format!("{x:02x}")).collect();
    format!(
        "{}-{}-{}-{}-{}",
        &h[0..8],
        &h[8..12],
        &h[12..16],
        &h[16..20],
        &h[20..32]
    )
}

/// 能不能当对话 id 用：1 到 128 个字符，只许 ASCII 字母、数字和 `-`。
/// 从别人写的文件里读来的 id 会被拼进命令行（`--resume {id}`），宽进不得。
pub fn looks_like_an_id(id: &str) -> bool {
    (1..=128).contains(&id.len()) && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
}

/// 把 profile 的 `conversation_file` 模板换成真路径：开头的 `~/` 换成家目录，
/// `{pid}` 换成 agent 的进程号。模板是空的、或者要家目录却问不出来，返回 `None`。
pub fn file_path(template: &str, pid: u32, home: Option<&Path>) -> Option<PathBuf> {
    if template.is_empty() {
        return None;
    }
    let filled = template.replace("{pid}", &pid.to_string());
    match filled.strip_prefix("~/") {
        Some(rest) => Some(home?.join(rest)),
        None => Some(PathBuf::from(filled)),
    }
}

/// 从 agent 自己为这个进程写的小文件里读出它此刻的对话 id。
///
/// 这份文件不是公开约定（claude 的 `~/.claude/sessions/<pid>.json` 就没写进
/// 文档），所以任何一处对不上都当没读到，调用方照旧用手上的 id：
/// - 读不到、不是 JSON 对象、`key` 不是像样的 id（见 `looks_like_an_id`）：不要；
/// - 文件里写了 `cwd`，解开链接后跟会话目录不是同一个：不要（别人的窗口）；
/// - 文件里写了 `pid`，跟会话的进程号不一样：不要（旧进程留下的）。
///
/// 没写 `cwd`/`pid` 的就不比那一项。
pub fn read_from_file(path: &Path, key: &str, dir: &Path, pid: u32) -> Option<String> {
    let text = std::fs::read_to_string(path).ok()?;
    let v: serde_json::Value = serde_json::from_str(&text).ok()?;
    let obj = v.as_object()?;
    let id = obj.get(key)?.as_str()?;
    if !looks_like_an_id(id) {
        return None;
    }
    if let Some(cwd) = obj.get("cwd") {
        let cwd = Path::new(cwd.as_str()?);
        if !same_dir(cwd, dir) {
            return None;
        }
    }
    if let Some(p) = obj.get("pid") {
        if p.as_u64()? != u64::from(pid) {
            return None;
        }
    }
    Some(id.to_string())
}

/// 解开链接后比；有一边解不开（目录没了）就按原样比。
fn same_dir(a: &Path, b: &Path) -> bool {
    let canon = |p: &Path| p.canonicalize().unwrap_or_else(|_| p.to_path_buf());
    canon(a) == canon(b)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(dir: &Path, body: &str) -> std::path::PathBuf {
        let f = dir.join("42.json");
        std::fs::write(&f, body).unwrap();
        f
    }

    #[test]
    fn learns_the_id_from_a_good_file() {
        let tmp = tempfile::tempdir().unwrap();
        let cwd = tmp.path().display().to_string();
        let f = write(
            tmp.path(),
            &format!(r#"{{"pid":42,"sessionId":"ab-12","cwd":{cwd:?},"kind":"interactive"}}"#),
        );
        assert_eq!(
            read_from_file(&f, "sessionId", tmp.path(), 42).as_deref(),
            Some("ab-12")
        );
    }

    #[test]
    fn accepts_a_file_without_cwd_or_pid() {
        let tmp = tempfile::tempdir().unwrap();
        let f = write(tmp.path(), r#"{"sessionId":"ab-12"}"#);
        assert_eq!(
            read_from_file(&f, "sessionId", tmp.path(), 42).as_deref(),
            Some("ab-12")
        );
    }

    #[test]
    fn cwd_is_compared_after_resolving_links() {
        // macOS 的临时目录 /var/... 其实是 /private/var/...：两边都要先解开再比。
        let tmp = tempfile::tempdir().unwrap();
        let real = tmp.path().join("real");
        std::fs::create_dir(&real).unwrap();
        let link = tmp.path().join("link");
        #[cfg(unix)]
        std::os::unix::fs::symlink(&real, &link).unwrap();
        #[cfg(not(unix))]
        return;
        let cwd = real.canonicalize().unwrap().display().to_string();
        let f = write(tmp.path(), &format!(r#"{{"sessionId":"x1","cwd":{cwd:?}}}"#));
        assert_eq!(read_from_file(&f, "sessionId", &link, 42).as_deref(), Some("x1"));
    }

    #[test]
    fn rejects_a_file_for_another_directory() {
        let tmp = tempfile::tempdir().unwrap();
        let other = tempfile::tempdir().unwrap();
        let cwd = other.path().display().to_string();
        let f = write(tmp.path(), &format!(r#"{{"sessionId":"x1","cwd":{cwd:?}}}"#));
        assert_eq!(read_from_file(&f, "sessionId", tmp.path(), 42), None);
    }

    #[test]
    fn rejects_a_file_for_another_process() {
        let tmp = tempfile::tempdir().unwrap();
        let f = write(tmp.path(), r#"{"pid":43,"sessionId":"x1"}"#);
        assert_eq!(read_from_file(&f, "sessionId", tmp.path(), 42), None);
    }

    #[test]
    fn rejects_a_missing_empty_or_odd_key() {
        let tmp = tempfile::tempdir().unwrap();
        for body in [
            r#"{"pid":42}"#,
            r#"{"sessionId":""}"#,
            r#"{"sessionId":7}"#,
            r#"{"sessionId":"a b"}"#,
            r#"{"sessionId":"../../etc"}"#,
            r#"[1,2]"#,
        ] {
            let f = write(tmp.path(), body);
            assert_eq!(read_from_file(&f, "sessionId", tmp.path(), 42), None, "{body}");
        }
        let long = "a".repeat(200);
        let f = write(tmp.path(), &format!(r#"{{"sessionId":"{long}"}}"#));
        assert_eq!(read_from_file(&f, "sessionId", tmp.path(), 42), None);
    }

    #[test]
    fn rejects_junk_and_a_missing_file() {
        let tmp = tempfile::tempdir().unwrap();
        let f = write(tmp.path(), "not json {");
        assert_eq!(read_from_file(&f, "sessionId", tmp.path(), 42), None);
        let gone = tmp.path().join("nope.json");
        assert_eq!(read_from_file(&gone, "sessionId", tmp.path(), 42), None);
    }

    #[test]
    fn file_path_fills_pid_and_home() {
        let home = Path::new("/h");
        assert_eq!(
            file_path("~/.claude/sessions/{pid}.json", 97, Some(home)),
            Some(Path::new("/h/.claude/sessions/97.json").to_path_buf())
        );
        assert_eq!(
            file_path("/abs/{pid}.json", 5, None),
            Some(Path::new("/abs/5.json").to_path_buf())
        );
        // 要家目录却问不出来：不读。
        assert_eq!(file_path("~/x/{pid}.json", 5, None), None);
        assert_eq!(file_path("", 5, Some(home)), None);
    }

    #[test]
    fn looks_like_an_id_accepts_uuids_only_shapes() {
        assert!(looks_like_an_id(&new()));
        assert!(!looks_like_an_id(""));
        assert!(!looks_like_an_id("a/b"));
        assert!(!looks_like_an_id(&"a".repeat(129)));
    }

    #[test]
    fn looks_like_uuid_v4_and_differs() {
        let a = new();
        let b = new();
        assert_eq!(a.len(), 36);
        assert_eq!(&a[14..15], "4");
        assert!(matches!(&a[19..20], "8" | "9" | "a" | "b"));
        assert!(a
            .chars()
            .all(|c| c == '-' || c.is_ascii_hexdigit() && !c.is_ascii_uppercase()));
        assert_eq!(
            a.char_indices()
                .filter(|&(_, c)| c == '-')
                .map(|(i, _)| i)
                .collect::<Vec<_>>(),
            vec![8, 13, 18, 23]
        );
        assert_ne!(a, b);
    }
}
