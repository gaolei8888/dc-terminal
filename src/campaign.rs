//! 活动文字一次批准：同一个活动里各平台要发的文字，用户在终端里看过、点头才存。
//! `dct do --campaign <名字>` 只用这里批过的文字，不收别人临时塞进来的内容参数。
use std::collections::BTreeMap;
use std::io::{IsTerminal, Write};
use std::path::{Path, PathBuf};

/// 平台 → 参数 → 值。
pub type PerPlatform = BTreeMap<String, BTreeMap<String, String>>;

#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, PartialEq)]
pub struct Campaign {
    pub name: String,
    pub approved_at: u64,
    pub per_platform: PerPlatform,
}

/// 同平台表的名字规则：小写英文开头，只含小写、数字、`-`、`_`，最长 32。
fn valid_ident(v: &str) -> bool {
    let b = v.as_bytes();
    !b.is_empty()
        && b.len() <= 32
        && b[0].is_ascii_lowercase()
        && b.iter()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || *c == b'-' || *c == b'_')
}

/// 活动名会变成文件名：只许字母数字、`-`、`_`、`.`（不能以 `.` 开头），最长 64。
fn check_campaign_name(name: &str) -> Result<(), String> {
    let ok = !name.is_empty()
        && name.len() <= 64
        && !name.starts_with('.')
        && !name.contains("..")
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.');
    if ok {
        Ok(())
    } else {
        Err(format!(
            "活动名只能用英文字母、数字、-、_、.（最长 64，不能以 . 开头），这里是 `{name}`"
        ))
    }
}

/// `--param <平台>.<参数>=<值>`，可重复。文件参数只存路径（这里不读文件）。
pub fn parse_params(args: &[String]) -> Result<PerPlatform, String> {
    let mut out = PerPlatform::new();
    let mut it = args.iter();
    while let Some(flag) = it.next() {
        if flag != "--param" {
            return Err(format!(
                "不认识的参数 `{flag}`（只有 --param <平台>.<参数>=<值>）"
            ));
        }
        let kv = it.next().ok_or("--param 后面缺值")?;
        let (key, value) = kv
            .split_once('=')
            .ok_or_else(|| format!("`{kv}` 要写成 <平台>.<参数>=<值>"))?;
        let (platform, param) = key
            .split_once('.')
            .ok_or_else(|| format!("`{key}` 缺平台：要写成 <平台>.<参数>=<值>"))?;
        if !valid_ident(platform) {
            return Err(format!(
                "平台名要写小写英文短名（如 x、tiktok），这里是 `{platform}`"
            ));
        }
        if !valid_ident(param) {
            return Err(format!("参数名要写小写英文短名，这里是 `{param}`"));
        }
        check_value(key, value)?;
        out.entry(platform.to_string())
            .or_default()
            .insert(param.to_string(), value.to_string());
    }
    Ok(out)
}

/// 看不见或会改变显示顺序的字符：控制字符（换行、制表符除外）、格式字符（Cf）、
/// 行/段分隔符（Zl、Zp）。std 没有字符类别接口，这里按范围列。
/// 放行 U+200D（ZWJ），不然组合 emoji 会坏；变体选择符不是 Cf，本来就放行。
/// 标签字符 U+E0000–E007F 也拒（能藏字，只是旗帜 emoji 的子区域旗用它）。
fn is_hidden_or_control(c: char) -> bool {
    if c == '\n' || c == '\t' || c == '\u{200D}' {
        return false;
    }
    let u = c as u32;
    c.is_control()
        || matches!(u,
            0xAD | 0x600..=0x605 | 0x61C | 0x6DD | 0x70F | 0x180E
            | 0x200B..=0x200F | 0x2028..=0x202E | 0x2060..=0x206F
            | 0xFEFF | 0xFFF9..=0xFFFB | 0x1BCA0..=0x1BCA3
            | 0x1D173..=0x1D17A | 0xE0000..=0xE007F)
}

fn check_value(key: &str, value: &str) -> Result<(), String> {
    match value.chars().find(|c| is_hidden_or_control(*c)) {
        Some(c) => Err(format!(
            "`{key}` 的值里有看不见的或控制用的字符（U+{:04X}）：只许普通文字、换行和制表符",
            c as u32
        )),
        None => Ok(()),
    }
}

/// 开头句：到第一个句末标点或换行为止（`.` 只在后面是空白或结尾时才算，
/// `v1.2`、`e.g.`、网址不会被截断），内部空白合成一个空格、统一小写。
fn opening_sentence(text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    let mut end = chars.len();
    for (i, c) in chars.iter().enumerate() {
        let stop = match c {
            '!' | '?' | '。' | '！' | '？' | '\n' => true,
            '.' => chars.get(i + 1).is_none_or(|n| n.is_whitespace()),
            _ => false,
        };
        if stop {
            end = i;
            break;
        }
    }
    let head: String = chars[..end].iter().collect();
    head.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

/// 同一活动里各平台的 `post_text` 不能相同，开头句也不能相同。
pub fn check_distinct(per: &PerPlatform) -> Result<(), String> {
    let texts: Vec<(&String, &String)> = per
        .iter()
        .filter_map(|(p, m)| m.get("post_text").map(|t| (p, t)))
        .collect();
    for (i, (pa, ta)) in texts.iter().enumerate() {
        for (pb, tb) in &texts[i + 1..] {
            if ta.trim().to_lowercase() == tb.trim().to_lowercase() {
                return Err(format!("{pa} 和 {pb} 的文字完全一样：各平台要写不同的文字"));
            }
            let (oa, ob) = (opening_sentence(ta), opening_sentence(tb));
            if !oa.is_empty() && oa == ob {
                return Err(format!(
                    "{pa} 和 {pb} 的开头句一样（`{oa}`）：各平台的开头句要不同"
                ));
            }
        }
    }
    Ok(())
}

fn render(name: &str, per: &PerPlatform) -> String {
    let mut s = format!("活动 `{name}` 要发的内容：\n");
    for (platform, params) in per {
        s.push_str(&format!("\n[{platform}]\n"));
        for (k, v) in params {
            // 多行的值：续行统一缩进并加 `| `，值里的 `[x]` 不会像小节标题。
            let mut lines = v.split('\n');
            s.push_str(&format!("  {k}: {}\n", lines.next().unwrap_or("")));
            for l in lines {
                s.push_str(&format!("      | {l}\n"));
            }
        }
    }
    s
}

fn file_for(dir: &Path, name: &str) -> PathBuf {
    dir.join(format!("{name}.json"))
}

/// `ask` 收到给用户看的全文，返回用户点没点头。没点头什么都不存。
pub fn approve(
    dir: &Path,
    name: &str,
    per: PerPlatform,
    ask: &mut dyn FnMut(&str) -> bool,
    now: u64,
) -> Result<Campaign, String> {
    check_campaign_name(name)?;
    if per.is_empty() || per.values().any(|m| m.is_empty()) {
        return Err("没有要批准的内容：至少一个 --param <平台>.<参数>=<值>".into());
    }
    check_distinct(&per)?;
    if !ask(&render(name, &per)) {
        return Err("没有批准，什么都没存".into());
    }
    let c = Campaign {
        name: name.to_string(),
        approved_at: now,
        per_platform: per,
    };
    std::fs::create_dir_all(dir).map_err(|e| format!("建不了 {}：{e}", dir.display()))?;
    let path = file_for(dir, name);
    let json = serde_json::to_vec_pretty(&c).map_err(|e| e.to_string())?;
    write_private(&path, &json).map_err(|e| format!("写不了 {}：{e}", path.display()))?;
    Ok(c)
}

fn write_private(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let mut opts = std::fs::OpenOptions::new();
    opts.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    let mut f = opts.open(path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        f.set_permissions(std::fs::Permissions::from_mode(0o600))?;
    }
    f.write_all(bytes)
}

pub fn load(dir: &Path, name: &str) -> Result<Campaign, String> {
    check_campaign_name(name)?;
    let raw = std::fs::read(file_for(dir, name))
        .map_err(|_| format!("活动 `{name}` 还没批准过（先跑 dct campaign approve {name} ...）"))?;
    let c: Campaign =
        serde_json::from_slice(&raw).map_err(|e| format!("活动 `{name}` 的记录坏了：{e}"))?;
    // 文件可能被手改过：和 parse_params 同一套检查，不让不可见字符溜进 dct do。
    for (platform, params) in &c.per_platform {
        if !valid_ident(platform) {
            return Err(format!("活动 `{name}` 的记录里平台名 `{platform}` 不合规"));
        }
        for (k, v) in params {
            if !valid_ident(k) {
                return Err(format!("活动 `{name}` 的记录里参数名 `{k}` 不合规"));
            }
            check_value(&format!("{platform}.{k}"), v)
                .map_err(|e| format!("活动 `{name}` 的记录被改过：{e}"))?;
        }
    }
    Ok(c)
}

pub fn default_dir() -> PathBuf {
    crate::proto::socket_path()
        .parent()
        .map(|p| p.join("campaigns"))
        .unwrap_or_else(|| PathBuf::from("campaigns"))
}

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

pub fn run_cli(args: &[String]) -> i32 {
    match (args.first().map(|s| s.as_str()), args.get(1)) {
        (Some("approve"), Some(name)) => {
            if !std::io::stdin().is_terminal() {
                eprintln!("批准文字要你本人在终端里点头：这里的标准输入不是终端");
                return 1;
            }
            let per = match parse_params(&args[2..]) {
                Ok(p) => p,
                Err(e) => {
                    eprintln!("{e}");
                    return 2;
                }
            };
            let mut ask = |text: &str| {
                println!("{text}");
                print!("批准这些文字？[y/N] ");
                let _ = std::io::stdout().flush();
                let mut line = String::new();
                std::io::stdin().read_line(&mut line).is_ok()
                    && matches!(line.trim(), "y" | "Y" | "yes")
            };
            match approve(&default_dir(), name, per, &mut ask, now_secs()) {
                Ok(c) => {
                    println!("已批准活动 `{}`", c.name);
                    0
                }
                Err(e) => {
                    eprintln!("{e}");
                    1
                }
            }
        }
        _ => {
            eprintln!("用法：dct campaign approve <名字> --param <平台>.<参数>=<值> ...");
            2
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(v: &str) -> String {
        v.to_string()
    }

    #[test]
    fn params_are_split_by_platform() {
        let p = parse_params(&[
            "--param".into(),
            "x.post_text=hi there".into(),
            "--param".into(),
            "tiktok.video=/v/a.mp4".into(),
        ])
        .unwrap();
        assert_eq!(p["x"]["post_text"], "hi there");
        assert_eq!(p["tiktok"]["video"], "/v/a.mp4");
        assert!(parse_params(&["--param".into(), "post_text=no platform".into()]).is_err());
    }

    #[test]
    fn same_text_on_two_platforms_is_refused() {
        let p = parse_params(&[
            "--param".into(),
            "x.post_text=New video out now! link".into(),
            "--param".into(),
            "facebook.post_text=New video out now! link".into(),
        ])
        .unwrap();
        assert!(check_distinct(&p).is_err());
    }

    #[test]
    fn same_opening_sentence_is_refused_but_different_openers_pass() {
        let same = parse_params(&[
            "--param".into(),
            "x.post_text=New video out now! Watch on X".into(),
            "--param".into(),
            "tiktok.post_text=new video out now! Watch here".into(),
        ])
        .unwrap();
        assert!(check_distinct(&same).unwrap_err().contains("开头句"));
        let ok = parse_params(&[
            "--param".into(),
            "x.post_text=New video out now! a".into(),
            "--param".into(),
            "tiktok.post_text=We made a thing! b".into(),
        ])
        .unwrap();
        assert!(check_distinct(&ok).is_ok());
    }

    #[test]
    fn approve_needs_a_yes_and_stores_privately() {
        let d = tempfile::tempdir().unwrap();
        let p = parse_params(&["--param".into(), "x.post_text=hello".into()]).unwrap();
        assert!(approve(d.path(), "launch", p.clone(), &mut |_| false, 1).is_err());
        assert!(load(d.path(), "launch").is_err(), "没点头就什么都不存");
        let mut shown = String::new();
        approve(
            d.path(),
            "launch",
            p,
            &mut |t| {
                shown = t.into();
                true
            },
            1,
        )
        .unwrap();
        assert!(shown.contains("hello"), "给用户看的是全文");
        assert_eq!(
            load(d.path(), "launch").unwrap().per_platform["x"]["post_text"],
            "hello"
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let m = std::fs::metadata(d.path().join("launch.json"))
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(m & 0o777, 0o600);
        }
    }

    #[test]
    fn distinct_check_runs_before_asking() {
        let d = tempfile::tempdir().unwrap();
        let p = parse_params(&[
            "--param".into(),
            "x.post_text=Same".into(),
            "--param".into(),
            "fb.post_text=Same".into(),
        ])
        .unwrap();
        let mut asked = false;
        assert!(approve(
            d.path(),
            "c",
            p,
            &mut |_| {
                asked = true;
                true
            },
            1
        )
        .is_err());
        assert!(!asked);
    }

    #[test]
    fn bad_campaign_names_and_platforms_are_refused() {
        let d = tempfile::tempdir().unwrap();
        let p = parse_params(&["--param".into(), "x.post_text=hello".into()]).unwrap();
        for bad in [
            "",
            "../evil",
            "a/b",
            "a\\b",
            "..",
            ".hidden",
            "a..b",
            "with space",
            &"n".repeat(65),
        ] {
            assert!(
                approve(d.path(), bad, p.clone(), &mut |_| true, 1).is_err(),
                "{bad:?}"
            );
            assert!(load(d.path(), bad).is_err(), "{bad:?}");
        }
        assert!(std::fs::read_dir(d.path()).unwrap().next().is_none());
        for bad in [
            "X.post_text=a",
            "1x.post_text=a",
            "a/b.post_text=a",
            ".post_text=a",
            "x..y=a",
        ] {
            assert!(parse_params(&[s("--param"), s(bad)]).is_err(), "{bad}");
        }
    }

    #[test]
    fn file_params_store_only_the_path() {
        let d = tempfile::tempdir().unwrap();
        let p = parse_params(&[
            "--param".into(),
            "tiktok.video=~/v/a.mp4".into(),
            "--param".into(),
            "tiktok.post_text=hi".into(),
        ])
        .unwrap();
        let c = approve(d.path(), "v", p, &mut |_| true, 5).unwrap();
        assert_eq!(c.per_platform["tiktok"]["video"], "~/v/a.mp4");
        assert_eq!(c.approved_at, 5);
    }

    #[test]
    fn control_characters_in_values_are_refused() {
        for bad in [
            "x.post_text=a\x1b[2Kb",
            "x.post_text=a\rb",
            "x.post_text=a\x07b",
            "x.post_text=a\u{85}b",
        ] {
            assert!(parse_params(&[s("--param"), s(bad)]).is_err(), "{bad:?}");
        }
        let ok = parse_params(&[s("--param"), s("x.post_text=line1\nline2\tend")]).unwrap();
        assert_eq!(ok["x"]["post_text"], "line1\nline2\tend");
    }

    #[test]
    fn a_multiline_value_cannot_forge_a_section_header() {
        let p = parse_params(&[
            s("--param"),
            s("x.post_text=hello\n[tiktok]\n  post_text: evil"),
        ])
        .unwrap();
        let shown = render("c", &p);
        let headers: Vec<&str> = shown.lines().filter(|l| l.starts_with('[')).collect();
        assert_eq!(headers, vec!["[x]"], "{shown}");
        assert!(
            shown
                .lines()
                .filter(|l| l.starts_with("  post_text:"))
                .count()
                == 1,
            "{shown}"
        );
        assert!(shown.contains("evil"));
    }

    #[test]
    fn opening_sentence_ignores_inner_dots_and_extra_whitespace() {
        assert_eq!(
            opening_sentence("Version v1.2 is out. More"),
            "version v1.2 is out"
        );
        assert_eq!(
            opening_sentence("See https://a.b/c now! x"),
            "see https://a.b/c now"
        );
        assert_eq!(
            opening_sentence("Use e.g.cats   and\tv1.2 now. x"),
            "use e.g.cats and v1.2 now"
        );
        let p = parse_params(&[
            s("--param"),
            s("x.post_text=New   video  out. a"),
            s("--param"),
            s("fb.post_text=new video out. b"),
        ])
        .unwrap();
        assert!(check_distinct(&p).is_err());
    }

    #[test]
    fn invisible_and_bidi_characters_are_refused() {
        for bad in [
            "\u{202E}",
            "\u{2066}",
            "\u{200B}",
            "\u{2028}",
            "\u{2029}",
            "\u{FEFF}",
            "\u{2060}",
            "\u{200F}",
            "\u{E0041}",
        ] {
            let arg = format!("x.post_text=a{bad}b");
            assert!(parse_params(&[s("--param"), arg]).is_err(), "{bad:?}");
        }
        // emoji with ZWJ and variation selectors keep working
        let ok = parse_params(&[
            s("--param"),
            s("x.post_text=hi \u{1F468}\u{200D}\u{1F4BB} \u{2764}\u{FE0F}"),
        ]);
        assert!(ok.is_ok());
    }

    #[test]
    fn a_hand_edited_file_with_hidden_characters_is_refused_by_load() {
        let d = tempfile::tempdir().unwrap();
        let p = parse_params(&[s("--param"), s("x.post_text=hello")]).unwrap();
        approve(d.path(), "c", p, &mut |_| true, 1).unwrap();
        let path = d.path().join("c.json");
        let raw = std::fs::read_to_string(&path).unwrap();
        std::fs::write(&path, raw.replace("hello", "he\\u202Ello")).unwrap();
        assert!(load(d.path(), "c").unwrap_err().contains("202E"));
        std::fs::write(&path, raw.replace("\"x\"", "\"../x\"")).unwrap();
        assert!(load(d.path(), "c").is_err());
    }
}
