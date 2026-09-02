//! 把 `[llm]` 打开——**只在配对屏上学生当面勾过的时候**。
//!
//! `config.rs` 开头那段注释说这是隐私边界不是默认值，那句话仍然成立：
//! 这里不给任何默认值，它只执行一个人刚刚看着文案做出的决定。
//!
//! **追加，不重写。** 整份反序列化再序列化会把用户的注释全吃掉，而
//! `~/.dct/config.toml` 是一份人手写、人要再读的文件。

pub fn enable(config_path: &std::path::Path, provider: &str, model: &str) -> Result<bool, String> {
    let existing = std::fs::read_to_string(config_path).unwrap_or_default();
    // 已经有 [llm] 就不动：用户（或上一次配对）已经决定过了。
    if existing
        .lines()
        .any(|l| l.trim_start().starts_with("[llm]"))
    {
        return Ok(false);
    }
    let mut out = existing;
    if !out.is_empty() && !out.ends_with('\n') {
        out.push('\n');
    }
    // 不写 base_url：留空让 resolve() 落回 profile 自己的 [api].base_url，
    // 地址只在一个地方维护。
    out.push_str(&format!(
        "\n# 由 dct 配对写入：学生在配对屏上勾了「报错看不懂时让 AI 解释」。\n\
         [llm]\nprovider = \"{provider}\"\nmodel = \"{model}\"\ntransport = \"http\"\n"
    ));
    if let Some(parent) = config_path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("{e}"))?;
    }
    std::fs::write(config_path, out).map_err(|e| format!("{e}"))?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 文件里已经有 [llm] 就一个字都不动。用户已经做过这个决定了。
    #[test]
    fn an_existing_llm_section_is_left_alone() {
        let d = tempfile::tempdir().unwrap();
        let f = d.path().join("config.toml");
        std::fs::write(&f, "[llm]\nprovider = \"kimi\"\nmodel = \"k2\"\n").unwrap();
        assert!(!enable(&f, "dc", "claude-x").unwrap(), "不该写");
        let after = std::fs::read_to_string(&f).unwrap();
        assert!(after.contains("kimi"), "{after}");
    }

    /// 追加而不是重写：config.toml 里还有 [menu] 之类，而且用户写了注释。
    /// 整份重新序列化会把注释全吃掉。
    #[test]
    fn other_sections_and_comments_survive() {
        let d = tempfile::tempdir().unwrap();
        let f = d.path().join("config.toml");
        std::fs::write(&f, "# 我的注释\n[menu]\nshort = true\n").unwrap();
        assert!(enable(&f, "dc", "claude-x").unwrap());
        let after = std::fs::read_to_string(&f).unwrap();
        assert!(after.contains("# 我的注释"), "注释不许丢：{after}");
        assert!(after.contains("[menu]"), "{after}");
        assert!(after.contains("provider = \"dc\""), "{after}");
    }

    /// 写完要能被 Config::load 读回来，否则等于没写。
    #[test]
    fn what_we_write_parses_back() {
        let d = tempfile::tempdir().unwrap();
        let f = d.path().join("config.toml");
        enable(&f, "qwen", "qwen3.8:27b").unwrap();
        let c = crate::config::Config::load(&f);
        let llm = c.llm.expect("写完就该是 Some");
        assert_eq!(llm.provider, "qwen");
        assert_eq!(llm.model.as_deref(), Some("qwen3.8:27b"));
    }
}
