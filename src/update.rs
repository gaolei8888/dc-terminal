//! agent 有没有新版本。
//!
//! **为什么有这个模块：** 看板上能干活的那几个 agent 都是 npm 装的，发版
//! 很勤，而 `dct install` 看到命令找得到就说「装好了」，装完再也不看版本。
//! 学生停在装那天的版本上，直到某天撞上一个新版才修好的问题。
//!
//! 这里只回答三个问题：这个 profile 装的是 npm 上哪个包、本机装的是几、
//! 仓库上最新是几。**不自动升**——`runtime.rs` 钉死 Node 版本的理由在这儿
//! 同样成立：一个教室里所有人手上应该是同一份。
//!
//! ## 已经验过的事实
//!
//! （实施 Task 2 时用 curl 验，把结果写在这儿：两个仓库对
//! `/@anthropic-ai/claude-code/latest` 是否直接回 200 + JSON。）

use std::path::Path;

pub const DEFAULT_REGISTRY: &str = "https://registry.npmjs.org";

/// `DCT_NPM_REGISTRY`，跟 `runtime.rs` 的 `DCT_NODE_BASE` 分成两个变量，
/// 理由写在原来 `cli.rs` 那份注释里（镜像站可能只镜像了其中一样）。
pub fn npm_registry_override() -> Option<String> {
    std::env::var("DCT_NPM_REGISTRY")
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

pub fn registry() -> String {
    npm_registry_override().unwrap_or_else(|| DEFAULT_REGISTRY.to_string())
}

pub fn latest_url(registry: &str, package: &str) -> String {
    format!("{}/{package}/latest", registry.trim_end_matches('/'))
}

/// `[install].command` 里那个 npm 包名：`npm i -g <pkg>` / `npm install -g <pkg>`
/// 里 `i`/`install` 之后第一个不以 `-` 开头的参数。不是 npm 的安装命令
/// （brew、pip、npx）→ `None`，那种 agent 不归 dct 管。
pub fn npm_package_of(command: &[String]) -> Option<String> {
    let mut it = command.iter();
    if it.next().map(String::as_str) != Some("npm") {
        return None;
    }
    match it.next().map(String::as_str) {
        Some("i") | Some("install") | Some("add") => {}
        _ => return None,
    }
    it.find(|a| !a.starts_with('-')).cloned()
}

/// 本机装的是几。`exe` 是 `sys::fs::find_command` 找到的那个文件。
///
/// 先按 npm 全局安装的两种布局找 `package.json`（Unix `../lib/node_modules`，
/// Windows 同级 `node_modules`），都找不到再顺着符号链接走到真实文件、往上
/// 找名为 `<package>` 的目录。三条都落空 → `None`，意思是「这份不是 npm 装的」。
pub fn installed_version(exe: &Path, package: &str) -> Option<String> {
    let dir = exe.parent()?;
    let candidates = [
        dir.join("../lib/node_modules").join(package),
        dir.join("node_modules").join(package),
    ];
    for c in candidates {
        if let Some(v) = read_version(&c.join("package.json")) {
            return Some(v);
        }
    }
    let real = std::fs::canonicalize(exe).ok()?;
    let mut cur = real.parent();
    while let Some(d) = cur {
        if d.ends_with(package) {
            return read_version(&d.join("package.json"));
        }
        cur = d.parent();
    }
    None
}

fn read_version(path: &Path) -> Option<String> {
    let body = std::fs::read_to_string(path).ok()?;
    parse_latest(&body)
}

/// 仓库 `/<pkg>/latest` 和 `package.json` 长得一样：都是一个带 `version`
/// 字段的对象。一个函数两处用。
pub fn parse_latest(body: &str) -> Option<String> {
    let v: serde_json::Value = serde_json::from_str(body).ok()?;
    v.get("version")?.as_str().map(|s| s.to_string())
}

/// 三段数字严格比较。任一边不是 `a.b.c` 纯数字（带 `-beta`、只有两段、
/// 空的）都返回 false——不认识的东西不提示，比提示错了好。
pub fn newer(installed: &str, latest: &str) -> bool {
    match (parts(installed), parts(latest)) {
        (Some(a), Some(b)) => b > a,
        _ => false,
    }
}

fn parts(v: &str) -> Option<[u64; 3]> {
    let mut it = v.split('.');
    let a = it.next()?.parse().ok()?;
    let b = it.next()?.parse().ok()?;
    let c = it.next()?.parse().ok()?;
    if it.next().is_some() {
        return None;
    }
    Some([a, b, c])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(s: &[&str]) -> Vec<String> {
        s.iter().map(|x| x.to_string()).collect()
    }

    #[test]
    fn package_name_comes_from_the_npm_install_command() {
        assert_eq!(
            npm_package_of(&v(&["npm", "i", "-g", "@anthropic-ai/claude-code"])).as_deref(),
            Some("@anthropic-ai/claude-code")
        );
        assert_eq!(
            npm_package_of(&v(&["npm", "install", "-g", "opencode-ai", "--foo"])).as_deref(),
            Some("opencode-ai")
        );
        // 参数顺序：`-g` 在包名后面也认。
        assert_eq!(
            npm_package_of(&v(&["npm", "i", "@qwen-code/qwen-code", "-g"])).as_deref(),
            Some("@qwen-code/qwen-code")
        );
    }

    #[test]
    fn non_npm_installers_have_no_package() {
        assert_eq!(npm_package_of(&v(&["brew", "install", "x"])), None);
        assert_eq!(npm_package_of(&v(&["npx", "x"])), None);
        assert_eq!(npm_package_of(&v(&["npm", "i", "-g"])), None);
        assert_eq!(npm_package_of(&v(&[])), None);
    }

    fn write_pkg(dir: &Path, package: &str, body: &str) {
        let d = dir.join(package);
        std::fs::create_dir_all(&d).unwrap();
        std::fs::write(d.join("package.json"), body).unwrap();
    }

    #[test]
    fn installed_version_is_read_from_the_unix_global_layout() {
        let t = tempfile::tempdir().unwrap();
        let prefix = t.path();
        std::fs::create_dir_all(prefix.join("bin")).unwrap();
        write_pkg(
            &prefix.join("lib/node_modules"),
            "@anthropic-ai/claude-code",
            r#"{"name":"@anthropic-ai/claude-code","version":"2.1.251"}"#,
        );
        let exe = prefix.join("bin/claude");
        std::fs::write(&exe, "").unwrap();
        assert_eq!(
            installed_version(&exe, "@anthropic-ai/claude-code").as_deref(),
            Some("2.1.251")
        );
    }

    #[test]
    fn installed_version_is_read_from_the_windows_global_layout() {
        let t = tempfile::tempdir().unwrap();
        let prefix = t.path();
        write_pkg(
            &prefix.join("node_modules"),
            "@qwen-code/qwen-code",
            r#"{"version":"0.22.3"}"#,
        );
        let exe = prefix.join("qwen.cmd");
        std::fs::write(&exe, "").unwrap();
        assert_eq!(
            installed_version(&exe, "@qwen-code/qwen-code").as_deref(),
            Some("0.22.3")
        );
    }

    #[test]
    fn a_launcher_with_no_package_json_nearby_is_not_managed() {
        // 官方安装器装的 claude：~/.local/bin/claude 旁边什么都没有。
        let t = tempfile::tempdir().unwrap();
        let exe = t.path().join("bin/claude");
        std::fs::create_dir_all(exe.parent().unwrap()).unwrap();
        std::fs::write(&exe, "").unwrap();
        assert_eq!(installed_version(&exe, "@anthropic-ai/claude-code"), None);
    }

    #[test]
    fn a_package_json_without_a_version_field_is_not_a_version() {
        let t = tempfile::tempdir().unwrap();
        let prefix = t.path();
        write_pkg(&prefix.join("lib/node_modules"), "x", r#"{"name":"x"}"#);
        let exe = prefix.join("bin/x");
        std::fs::create_dir_all(exe.parent().unwrap()).unwrap();
        std::fs::write(&exe, "").unwrap();
        assert_eq!(installed_version(&exe, "x"), None);
    }

    #[cfg(unix)]
    #[test]
    fn a_relocated_prefix_is_found_by_following_the_symlink() {
        // bin/claude -> ../lib/node_modules/@a/b/cli.js，但 bin 目录被挪到了
        // 别处（比如 prefix 改过）。相对路径找不到，顺着链接能找到。
        let t = tempfile::tempdir().unwrap();
        let real = t.path().join("elsewhere/lib/node_modules");
        write_pkg(&real, "@a/b", r#"{"version":"1.0.0"}"#);
        std::fs::write(real.join("@a/b/cli.js"), "").unwrap();
        let bin = t.path().join("bin");
        std::fs::create_dir_all(&bin).unwrap();
        let exe = bin.join("b");
        std::os::unix::fs::symlink(real.join("@a/b/cli.js"), &exe).unwrap();
        assert_eq!(installed_version(&exe, "@a/b").as_deref(), Some("1.0.0"));
    }

    #[test]
    fn latest_is_the_version_field_of_the_registry_json() {
        assert_eq!(
            parse_latest(r#"{"name":"x","version":"2.1.260","dist":{}}"#).as_deref(),
            Some("2.1.260")
        );
        assert_eq!(parse_latest(r#"{"name":"x"}"#), None);
        assert_eq!(parse_latest("not json"), None);
        assert_eq!(parse_latest(r#"{"version":42}"#), None);
    }

    #[test]
    fn newer_compares_three_numeric_parts() {
        assert!(newer("1.2.3", "1.2.10"));
        assert!(newer("1.9.0", "1.10.0"));
        assert!(newer("0.22.3", "1.0.0"));
        assert!(!newer("1.2.3", "1.2.3"));
        assert!(!newer("1.2.10", "1.2.3"));
    }

    #[test]
    fn prereleases_and_garbage_never_prompt() {
        assert!(!newer("1.2.3", "1.3.0-beta.1"));
        assert!(!newer("1.2.3-rc.1", "1.2.3"));
        assert!(!newer("1.2", "1.3.0"));
        assert!(!newer("1.2.3", "latest"));
        assert!(!newer("", "1.0.0"));
    }

    #[test]
    fn latest_url_keeps_the_scope_slash_and_trims_the_registry() {
        assert_eq!(
            latest_url("https://registry.npmjs.org/", "@anthropic-ai/claude-code"),
            "https://registry.npmjs.org/@anthropic-ai/claude-code/latest"
        );
        assert_eq!(
            latest_url("https://registry.npmmirror.com", "opencode-ai"),
            "https://registry.npmmirror.com/opencode-ai/latest"
        );
    }
}
