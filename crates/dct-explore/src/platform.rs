//! 平台表：平台名 -> 固定域名、入口网址、登录闸、发布词等。是数据，不是代码。

#[derive(Debug, Clone, PartialEq, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Platform {
    /// 小写短名，同 dcv check_platform：x、tiktok
    pub name: String,
    /// 写死的主机名
    pub domain: String,
    /// https，主机名必须等于 domain
    pub entry_url: String,
    /// list_windows 的 app
    pub browser: String,
    /// dcv 库
    pub vault: String,
    /// 只有登录后才有的字
    pub login_gate: Option<String>,
    pub publish_words: Vec<String>,
    pub publish_done: Option<String>,
    pub never_tap: Vec<String>,
    /// 登录常是手机扫码。只是信息：登录/二维码画面本来就会停下交给用户，字不发给任何模型。
    #[serde(default)]
    pub login_qr: bool,
    /// 最后的发布一下永远留给用户点（重放和 `dct do` 照此办理）。
    #[serde(default)]
    pub publish_by_user_only: bool,
    /// 哪些字段还没在真机上看过
    #[serde(default)]
    pub unverified: Vec<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Table {
    pub rows: Vec<Platform>,
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct File {
    #[serde(default)]
    platform: Vec<Platform>,
}

fn parse(text: &str) -> Result<Vec<Platform>, String> {
    let f: File = toml::from_str(text).map_err(|e| format!("平台表读不懂：{e}"))?;
    for p in &f.platform {
        validate(p).map_err(|e| format!("平台 `{}`：{e}", p.name))?;
    }
    Ok(f.platform)
}

impl Table {
    /// 编进程序的种子表。
    pub fn seed() -> Table {
        let rows = parse(include_str!("platforms.toml")).expect("内置 platforms.toml 必须合法");
        Table { rows }
    }

    /// 用户覆盖：同名整行替换，新名追加；每行的校验和种子一样。
    pub fn with_override(mut self, toml_text: &str) -> Result<Table, String> {
        for p in parse(toml_text)? {
            match self.rows.iter_mut().find(|r| r.name == p.name) {
                Some(slot) => *slot = p,
                None => self.rows.push(p),
            }
        }
        Ok(self)
    }

    pub fn get(&self, name: &str) -> Option<&Platform> {
        self.rows.iter().find(|r| r.name == name)
    }
}

/// 同 dcv `check_platform`：小写英文开头，只含小写、数字、`-`、`_`，最长 32。
fn check_name(v: &str) -> Result<(), String> {
    let b = v.as_bytes();
    let ok = !b.is_empty()
        && b.len() <= 32
        && b[0].is_ascii_lowercase()
        && b.iter()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || *c == b'-' || *c == b'_');
    if ok {
        Ok(())
    } else {
        Err(format!(
            "`name` 要写小写英文短名（如 x、tiktok），这里是 `{v}`"
        ))
    }
}

pub fn validate(p: &Platform) -> Result<(), String> {
    check_name(&p.name)?;
    if p.domain.is_empty() {
        return Err("`domain` 不能为空".into());
    }
    let rest = p
        .entry_url
        .strip_prefix("https://")
        .ok_or_else(|| format!("`entry_url` 必须以 https:// 开头：{}", p.entry_url))?;
    let host_end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let host = &rest[..host_end];
    if host.contains('@') || host.contains(':') {
        return Err(format!(
            "`entry_url` 的主机名不许带 @ 或端口：{}",
            p.entry_url
        ));
    }
    if host != p.domain {
        return Err(format!(
            "`entry_url` 的主机名 `{host}` 必须等于 domain `{}`",
            p.domain
        ));
    }
    if p.publish_words.is_empty() || p.publish_words.iter().any(|w| w.trim().is_empty()) {
        return Err("`publish_words` 不能为空（没有发布词就认不出最后那一下）".into());
    }
    if p.browser.trim().is_empty() {
        return Err("`browser` 不能为空".into());
    }
    if p.vault.trim().is_empty() {
        return Err("`vault` 不能为空".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn seed_has_the_six_platforms_and_all_rows_are_valid() {
        let t = Table::seed();
        for n in [
            "x",
            "tiktok",
            "youtube",
            "facebook",
            "instagram",
            "xiaohongshu",
        ] {
            let p = t.get(n).unwrap_or_else(|| panic!("缺 {n}"));
            validate(p).unwrap();
        }
        assert_eq!(t.get("x").unwrap().entry_url, "https://x.com/compose/post");
        assert_eq!(
            t.get("tiktok").unwrap().entry_url,
            "https://www.tiktok.com/tiktokstudio/upload"
        );
    }

    #[test]
    fn entry_url_must_be_https_on_the_fixed_domain() {
        let mut p = Table::seed().get("x").unwrap().clone();
        p.entry_url = "http://x.com/compose/post".into();
        assert!(validate(&p).is_err());
        p.entry_url = "https://x.com.evil.example/compose".into();
        assert!(validate(&p).is_err());
        p.entry_url = "https://evil.example/?u=x.com".into();
        assert!(validate(&p).is_err());
        p.entry_url = "https://x.com@evil.example/".into();
        assert!(validate(&p).is_err());
        p.entry_url = "https://x.com:8443/compose".into();
        assert!(validate(&p).is_err());
        p.entry_url = "https://x.com#frag".into();
        assert!(validate(&p).is_ok());
    }

    #[test]
    fn override_replaces_a_row_and_adds_new_ones_with_the_same_checks() {
        let t = Table::seed()
            .with_override(
                r#"
[[platform]]
name = "x"
domain = "x.com"
entry_url = "https://x.com/compose/post"
browser = "Microsoft Edge"
vault = "social-global"
login_gate = "What's happening?"
publish_words = ["Post"]
never_tap = ["Subscribe"]
"#,
            )
            .unwrap();
        assert_eq!(t.get("x").unwrap().browser, "Microsoft Edge");
        assert_eq!(
            t.rows.len(),
            Table::seed().rows.len(),
            "same name replaces, not appends"
        );
        let t2 = Table::seed()
            .with_override("[[platform]]\nname = \"bsky\"\ndomain = \"bsky.app\"\nentry_url = \"https://bsky.app/\"\nbrowser = \"b\"\nvault = \"v\"\npublish_words = [\"Post\"]\nnever_tap = []\n")
            .unwrap();
        assert_eq!(t2.rows.len(), Table::seed().rows.len() + 1);
        assert!(Table::seed().with_override("[[platform]]\nname = \"Bad Name\"\ndomain = \"a.com\"\nentry_url = \"https://a.com\"\nbrowser = \"b\"\nvault = \"v\"\npublish_words = [\"Go\"]\nnever_tap = []\n").is_err());
        assert!(Table::seed().with_override("not toml [[").is_err());
    }

    #[test]
    fn a_row_without_publish_words_is_refused() {
        let mut p = Table::seed().get("x").unwrap().clone();
        p.publish_words.clear();
        assert!(
            validate(&p).is_err(),
            "没有发布词就认不出最后那一下，不能摸索"
        );
    }

    const NEW_ROWS: [&str; 10] = [
        "weibo",
        "bilibili",
        "zhihu",
        "zhihu_article",
        "douyin",
        "kuaishou",
        "wechat_channels",
        "toutiao",
        "wechat_mp",
        "baijiahao",
    ];

    #[test]
    fn chinese_platforms_are_seeded_and_valid() {
        let t = Table::seed();
        assert_eq!(t.rows.len(), 16);
        for n in NEW_ROWS {
            let p = t.get(n).unwrap_or_else(|| panic!("缺 {n}"));
            validate(p).unwrap();
            for f in ["entry_url", "publish_words"] {
                assert!(p.unverified.iter().any(|u| u == f), "{n} 应标 {f} 未核实");
            }
            assert!(p.login_gate.is_none() && p.publish_done.is_none(), "{n}");
        }
    }

    #[test]
    fn qr_login_and_publish_by_user_flags() {
        let t = Table::seed();
        for n in [
            "douyin",
            "xiaohongshu",
            "wechat_channels",
            "wechat_mp",
            "weibo",
            "bilibili",
        ] {
            assert!(t.get(n).unwrap().login_qr, "{n} login_qr");
        }
        let by_user: Vec<&str> = t
            .rows
            .iter()
            .filter(|p| p.publish_by_user_only)
            .map(|p| p.name.as_str())
            .collect();
        assert_eq!(by_user, ["wechat_mp"]);
    }

    #[test]
    fn override_without_the_new_fields_defaults_both_to_false() {
        let t = Table::seed()
            .with_override("[[platform]]\nname = \"bsky\"\ndomain = \"bsky.app\"\nentry_url = \"https://bsky.app/\"\nbrowser = \"b\"\nvault = \"v\"\npublish_words = [\"Post\"]\nnever_tap = []\n")
            .unwrap();
        let p = t.get("bsky").unwrap();
        assert!(!p.login_qr && !p.publish_by_user_only);
    }
}
