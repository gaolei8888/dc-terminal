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
//! 2026-09-06 在开发机上 curl 过，带作用域的包名里那个 `/` **不转义**，
//! 两个仓库都直接回答（所以 `latest_url` 不做 `%2F`）：
//!
//! - `https://registry.npmjs.org/@anthropic-ai/claude-code/latest` → 200，
//!   JSON 里有 `"version": "2.1.263"`。
//! - `https://registry.npmmirror.com/@anthropic-ai/claude-code/latest` → 200，
//!   同样有 `"version": "2.1.263"`（两边同一版）。
//!
//! 也就是说 `DCT_NPM_REGISTRY` 指到淘宝镜像的教室里，这条路径照样通，
//! 不需要为镜像站单开一套 URL 拼法。

use crate::session::recover;
use std::collections::BTreeMap;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

pub const DEFAULT_REGISTRY: &str = "https://registry.npmjs.org";

/// npm 该去哪个仓库拿包。国内课堂设 `DCT_NPM_REGISTRY`，别处不设就是官方。
///
/// 跟 `DCT_NODE_BASE` 分成两个变量而不是一个「中国模式」开关：这两件事
/// 会分别失效（镜像站可能只镜像了其中一样），而一个开关同时管两样的话，
/// 出问题时没法只换一半。
pub fn npm_registry_override() -> Option<String> {
    std::env::var("DCT_NPM_REGISTRY")
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

pub fn registry() -> String {
    let r = npm_registry_override().unwrap_or_else(|| DEFAULT_REGISTRY.to_string());
    r.trim_end_matches('/').to_string()
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
        Some("i") | Some("install") => {}
        _ => return None,
    }
    it.find(|a| !a.starts_with('-')).cloned()
}

/// 这个 profile 要更新的话，动的是 npm 上哪个包。
///
/// **自己没有 `[install]` 的，跟着 owner 走。** kimi/glm/deepseek/qwen-api
/// 的 `command[0]` 都是 `claude`，它们自己一行 `[install]` 都没有——但磁盘上
/// 那份 `claude` 正是 dct 用 `@anthropic-ai/claude-code` 装的。不认这层关系
/// 的话，这四个在列表上永远不带「有新版本」，而在它们身上按 `u` 会得到
/// 一句「这份 Kimi 不是 dct 装的」——一句假话。
///
/// `owner.name == p.name` 那一句是**写给读的人看的，不是为了改结果**：
/// `claude` 自己的 owner 就是它自己（见 `profile::dependency_owner`），
/// 而走到那儿意味着上面那行已经从 `p.install` 里没问出包名——同一个
/// `install` 再问一遍还是 `None`。删掉它一条测试都不会红，试过了。
/// 留着是因为「跟着别人走」这句话里的「别人」得在代码里看得见。
pub fn package_for(p: &crate::profile::Profile, all: &[crate::profile::Profile]) -> Option<String> {
    if let Some(pkg) = p.install.as_ref().and_then(|i| npm_package_of(&i.command)) {
        return Some(pkg);
    }
    let owner = crate::profile::dependency_owner(all, p.command.first()?)?;
    if owner.name == p.name {
        return None;
    }
    owner
        .install
        .as_ref()
        .and_then(|i| npm_package_of(&i.command))
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

/// 一个 agent 的版本现状。**不落盘**：守护进程本来就常驻，重启之后
/// 30 秒内会重新查到，而落盘会多出一份能跟现实不一致的旧数据。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UpdateInfo {
    /// 例：`@anthropic-ai/claude-code`
    pub package: String,
    /// 本机装的那一版，例：`2.1.251`
    pub installed: String,
    /// 仓库上最新那一版。`None` = 还没查 / 查不到——两者在界面上是同一
    /// 回事，理由见模块头。
    pub latest: Option<String>,
    pub checked_at: Option<SystemTime>,
}

/// profile 名 → 现状。**表里有它就等于「归 dct 管」**，所以不归 dct 管的
/// 必须真的从表里删掉，不能只是不更新它。
pub type UpdateTable = Arc<Mutex<BTreeMap<String, UpdateInfo>>>;

pub fn new_table() -> UpdateTable {
    Arc::new(Mutex::new(BTreeMap::new()))
}

/// 守护进程起来多久之后查第一次。**不在启动路径上**：第一个连上来的界面
/// 不该为了一次网络请求多等一会儿。
pub const FIRST_CHECK_AFTER: Duration = Duration::from_secs(30);
/// 之后隔多久查一次。一天一次——这些 CLI 一周几个版本，再勤也没有意义，
/// 而每台机器每天问一次仓库是个不会被人注意到的量。
pub const CHECK_EVERY: Duration = Duration::from_secs(24 * 60 * 60);
/// 单次请求的上限。查不到就是没有提示，所以宁可早点放弃。
const FETCH_TIMEOUT: Duration = Duration::from_secs(5);

/// 该不该给用户看「有新版本」。查不到、已经是最新、本机反而更新（装了
/// 预览版）——三种都不提示。
pub fn prompt_for(info: &UpdateInfo) -> Option<crate::proto::UpdatePrompt> {
    let latest = info.latest.as_deref()?;
    newer(&info.installed, latest).then(|| crate::proto::UpdatePrompt {
        installed: info.installed.clone(),
        latest: latest.to_string(),
    })
}

/// 把 `profiles_dir` 下归 dct 管的 agent 逐个查一遍，结果写进 `table`。
/// `only` 限定一个 profile 名（`RecheckUpdates` 用），`None` = 全查。
///
/// `fetch` 是「问仓库要 latest」那一步，注入进来只为一件事：**测试不发真
/// 实 HTTP**。生产上传的是 [`fetch_latest`]。
///
/// **追溯不到 npm 包的从表里删掉**，不是跳过：用户把 npm 那份卸了、改用
/// 官方安装器装，表里不能还留着一条永远也升不动的旧提示。
pub fn check_updates(
    profiles_dir: &Path,
    table: &UpdateTable,
    only: Option<&str>,
    fetch: &dyn Fn(&str) -> Option<String>,
) {
    let (all, _) = crate::profile::all_profiles(profiles_dir);
    // 一个包只问仓库一次。**五个 profile 跑同一个 `claude`**（claude 自己
    // 加上 kimi/glm/deepseek/qwen-api），逐个问就是五次同样的请求、五份
    // 同样的答案；网差的教室里那是五个 5 秒。
    let mut asked: BTreeMap<String, Option<String>> = BTreeMap::new();
    for p in all.iter().filter(|p| only.is_none_or(|o| o == p.name)) {
        // 「装没装」「装的是几」都必须在守护进程这个 PATH 里问——同
        // `profile::command_exists` 的理由，答案只能来自子进程真正会
        // 拿到的那个环境。
        let managed = package_for(p, &all).and_then(|pkg| {
            let exe = crate::sys::fs::find_command(p.command.first()?)?;
            let installed = installed_version(&exe, &pkg)?;
            Some((pkg, installed))
        });
        let Some((package, installed)) = managed else {
            recover(table.lock()).remove(&p.name);
            continue;
        };
        let latest = match asked.get(&package) {
            Some(v) => v.clone(),
            None => {
                let v = fetch(&package);
                if v.is_none() {
                    // 界面上什么都不显示（没网的教室里不该冒一行关于 npm
                    // 仓库的红字），但排查的人得有一行可看。**跟着请求印，
                    // 不是跟着 profile 印**：一次没问到不该刷出五行。
                    eprintln!("检查更新：{package} 没问到最新版本");
                }
                asked.insert(package.clone(), v.clone());
                v
            }
        };
        recover(table.lock()).insert(
            p.name.clone(),
            UpdateInfo {
                package,
                installed,
                latest,
                checked_at: Some(SystemTime::now()),
            },
        );
    }
}

/// 真的去问仓库。任何一步不顺利都返回 `None`——查不到就是没有提示，
/// 不是错误，理由见模块头。
///
/// 用 `sys::tls` 那份 agent：Windows 上的 TLS 后端要在那儿装（见
/// `Cargo.toml` 里 ureq 那条注释），自己 `ureq::agent()` 会在 Windows 上
/// 静默地一个 HTTPS 都发不出去。
pub fn fetch_latest(package: &str) -> Option<String> {
    let url = latest_url(&registry(), package);
    let agent = crate::sys::tls::agent_builder()
        .timeout(FETCH_TIMEOUT)
        .timeout_connect(FETCH_TIMEOUT)
        .build();
    let body = agent.get(&url).call().ok()?.into_string().ok()?;
    parse_latest(&body)
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
    use std::path::PathBuf;

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

    /// `registry()` 跟 `latest_url` 一样要去掉末尾的 `/`——老师把镜像地址
    /// 抄进环境变量时，带不带这一个斜杠全凭手感，两种都不能拼出
    /// `.../` + `/@pkg/latest` 这种双斜杠。
    ///
    /// 用完把变量还原：这个变量在整个测试进程里只有这一处读写
    /// （见 `grep DCT_NPM_REGISTRY src/`），跟 `runtime.rs` 里
    /// `the_mingit_address_can_be_pointed_at_a_classroom_mirror`
    /// 对 `DCT_MINGIT_URL` 的做法一样，不会跟其它测试抢同一个变量。
    #[test]
    fn registry_trims_a_trailing_slash_from_the_override() {
        let saved = std::env::var("DCT_NPM_REGISTRY").ok();
        std::env::set_var("DCT_NPM_REGISTRY", "https://x/");
        assert_eq!(registry(), "https://x");
        match saved {
            Some(v) => std::env::set_var("DCT_NPM_REGISTRY", v),
            None => std::env::remove_var("DCT_NPM_REGISTRY"),
        }
    }

    /// 一份假的「npm 全局前缀」：`prefix/bin/fakeagent` 旁边摆着
    /// `prefix/lib/node_modules/fake-agent/package.json`（1.0.0）。回那个
    /// 可执行文件的绝对路径。
    fn fake_npm_prefix(t: &tempfile::TempDir) -> PathBuf {
        let prefix = t.path().join("prefix");
        std::fs::create_dir_all(prefix.join("bin")).unwrap();
        write_pkg(
            &prefix.join("lib/node_modules"),
            "fake-agent",
            r#"{"version":"1.0.0"}"#,
        );
        let exe = prefix.join("bin/fakeagent");
        std::fs::write(&exe, "#!/bin/sh\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&exe, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        exe
    }

    /// 一份假的「npm 全局前缀」外加一个指着它的 profile：
    /// `prefix/bin/fakeagent` + `prefix/lib/node_modules/fake-agent/package.json`。
    /// `command` 写绝对路径，`find_command` 就不用去碰这台机器真实的 PATH。
    fn managed_profile_dir(t: &tempfile::TempDir) -> (PathBuf, PathBuf) {
        let exe = fake_npm_prefix(t);
        let profiles = t.path().join("profiles");
        std::fs::create_dir_all(&profiles).unwrap();
        std::fs::write(
            profiles.join("fake.toml"),
            format!(
                "name = \"fake\"\ncommand = [{:?}]\nis_agent = true\n[install]\ncommand = [\"npm\", \"i\", \"-g\", \"fake-agent\"]\n",
                exe.display()
            ),
        )
        .unwrap();
        (profiles, exe)
    }

    #[test]
    fn check_updates_records_installed_and_latest_for_a_managed_agent() {
        let t = tempfile::tempdir().unwrap();
        let (profiles, _) = managed_profile_dir(&t);
        let table = new_table();
        check_updates(&profiles, &table, Some("fake"), &|pkg| {
            assert_eq!(pkg, "fake-agent", "问仓库要的必须是 [install] 里那个包名");
            Some("1.2.0".into())
        });
        let g = table.lock().unwrap();
        let info = g.get("fake").expect("fake 应该被记进表里");
        assert_eq!(info.installed, "1.0.0");
        assert_eq!(info.latest.as_deref(), Some("1.2.0"));
        assert!(info.checked_at.is_some());
        let p = prompt_for(info).expect("1.0.0 → 1.2.0 该提示");
        assert_eq!(
            (p.installed.as_str(), p.latest.as_str()),
            ("1.0.0", "1.2.0")
        );
    }

    /// `only = None` 就是「全查一遍」。这台机器上除了 fake 还可能有真装着的
    /// npm agent（codex、opencode……），所以只断言 fake 那一条——**不断言
    /// 表里只有它**，那会让这条测试的绿灯取决于跑它的人装了什么。
    #[test]
    fn checking_everything_covers_the_managed_agent_too() {
        let t = tempfile::tempdir().unwrap();
        let (profiles, _) = managed_profile_dir(&t);
        let table = new_table();
        check_updates(&profiles, &table, None, &|_| Some("1.2.0".into()));
        let g = table.lock().unwrap();
        assert_eq!(g.get("fake").unwrap().latest.as_deref(), Some("1.2.0"));
    }

    #[test]
    fn a_failed_fetch_leaves_no_prompt_but_keeps_the_installed_version() {
        let t = tempfile::tempdir().unwrap();
        let (profiles, _) = managed_profile_dir(&t);
        let table = new_table();
        check_updates(&profiles, &table, Some("fake"), &|_| None);
        let g = table.lock().unwrap();
        let info = g.get("fake").unwrap();
        assert_eq!(info.installed, "1.0.0");
        assert_eq!(info.latest, None);
        assert!(prompt_for(info).is_none());
    }

    #[test]
    fn an_up_to_date_agent_has_no_prompt() {
        let info = UpdateInfo {
            package: "x".into(),
            installed: "1.0.0".into(),
            latest: Some("1.0.0".into()),
            checked_at: None,
        };
        assert!(prompt_for(&info).is_none());
    }

    /// 本机比仓库新（装了预览版、或者 `latest` 标签退回过一版）也不提示。
    /// 单独钉一条：`prompt_for` 里把 `newer` 换成 `!=` 的话，上面那条
    /// 「已经是最新」照样绿，只有这一条会红。
    #[test]
    fn an_agent_newer_than_the_registry_has_no_prompt() {
        let info = UpdateInfo {
            package: "x".into(),
            installed: "2.0.0".into(),
            latest: Some("1.0.0".into()),
            checked_at: None,
        };
        assert!(prompt_for(&info).is_none());
    }

    /// 没有 `[install]` 的 profile（`shell`）不归 dct 管：不问仓库，也不进表。
    #[test]
    fn unmanaged_profiles_are_never_asked_about() {
        let t = tempfile::tempdir().unwrap();
        let profiles = t.path().join("profiles");
        std::fs::create_dir_all(&profiles).unwrap();
        let table = new_table();
        check_updates(&profiles, &table, Some("shell"), &|_| {
            panic!("shell 没有 [install]，不该发请求")
        });
        assert!(!table.lock().unwrap().contains_key("shell"));
    }

    /// 表里的旧记录会被删掉：用户把 npm 那份卸了、换成官方安装器装的，
    /// 提示不能还挂在那儿。
    #[test]
    fn a_profile_that_stopped_being_managed_is_dropped_from_the_table() {
        let t = tempfile::tempdir().unwrap();
        let profiles = t.path().join("profiles");
        std::fs::create_dir_all(&profiles).unwrap();
        let table = new_table();
        table.lock().unwrap().insert(
            "shell".into(),
            UpdateInfo {
                package: "stale".into(),
                installed: "1.0.0".into(),
                latest: Some("2.0.0".into()),
                checked_at: None,
            },
        );
        check_updates(&profiles, &table, Some("shell"), &|_| {
            panic!("shell 没有 [install]，不该发请求")
        });
        assert!(
            !table.lock().unwrap().contains_key("shell"),
            "不归 dct 管了就得从表里消失，不然旧提示永远挂着"
        );
    }

    /// 同一份假前缀，两个 profile：`fakeagent` 有 `[install]`，`faketwin`
    /// 没有、跑的是同一个二进制。现实里就是 claude 与 kimi/glm/deepseek/
    /// qwen-api 的关系。
    ///
    /// owner 那份的**名字必须等于二进制的文件名**——`dependency_owner`
    /// 就是这么找 owner 的（`claude` 这个 profile owns `claude` 这条命令）。
    fn dependent_profile_dir(t: &tempfile::TempDir) -> PathBuf {
        let exe = fake_npm_prefix(t);
        let profiles = t.path().join("profiles");
        std::fs::create_dir_all(&profiles).unwrap();
        std::fs::write(
            profiles.join("fakeagent.toml"),
            format!(
                "name = \"fakeagent\"\ncommand = [{:?}]\nis_agent = true\n[install]\ncommand = [\"npm\", \"i\", \"-g\", \"fake-agent\"]\n",
                exe.display()
            ),
        )
        .unwrap();
        std::fs::write(
            profiles.join("faketwin.toml"),
            format!(
                "name = \"faketwin\"\ncommand = [{:?}]\nis_agent = true\n",
                exe.display()
            ),
        )
        .unwrap();
        profiles
    }

    /// kimi/glm/deepseek/qwen-api 那一类：自己一行 `[install]` 都没有，
    /// 但跑的 `claude` 正是 dct 装的。不认这层关系的话，这四个永远不带
    /// 「有新版本」，按 `u` 还会被告知「这份 Kimi 不是 dct 装的」——假话。
    #[test]
    fn a_profile_without_install_inherits_its_binary_owners_package() {
        let t = tempfile::tempdir().unwrap();
        let profiles = dependent_profile_dir(&t);
        let table = new_table();
        check_updates(&profiles, &table, Some("faketwin"), &|pkg| {
            assert_eq!(pkg, "fake-agent", "问的得是 owner 那个包");
            Some("1.2.0".into())
        });
        let g = table.lock().unwrap();
        let info = g.get("faketwin").expect("跟着 owner 走，该进表");
        assert_eq!(info.package, "fake-agent");
        assert_eq!(info.installed, "1.0.0");
        assert_eq!(info.latest.as_deref(), Some("1.2.0"));
        assert!(prompt_for(info).is_some(), "1.0.0 → 1.2.0 该提示");
    }

    /// 没有 `[install]`、命令也追溯不到任何 owner（没有哪个 profile 叫
    /// 这个名字）→ 不归 dct 管。**这一条钉的是 `package_for` 里那个
    /// 「owner 得是另一个 profile」的边界**：少了它，一个自己就是自己
    /// owner、却没有 `[install]` 的 profile 会绕回来问自己一遍。
    #[test]
    fn a_profile_with_no_install_and_no_owner_is_not_managed() {
        let t = tempfile::tempdir().unwrap();
        let exe = fake_npm_prefix(&t);
        let profiles = t.path().join("profiles");
        std::fs::create_dir_all(&profiles).unwrap();
        std::fs::write(
            profiles.join("lonely.toml"),
            format!(
                "name = \"lonely\"\ncommand = [{:?}]\nis_agent = true\n",
                exe.display()
            ),
        )
        .unwrap();
        let table = new_table();
        check_updates(&profiles, &table, Some("lonely"), &|_| {
            panic!("没有 [install]、也没有 owner，不该发请求")
        });
        assert!(!table.lock().unwrap().contains_key("lonely"));
    }

    /// 内置的那几个真名字，钉在这儿。上面两条用的是假 profile，而这条
    /// 回答的是当初报上来的那个 bug 本身：在 Kimi 上按 `u`，dct 到底认不认
    /// 得那份 claude。内置清单是编进二进制的，跟这台机器装了什么无关。
    #[test]
    fn the_claude_family_all_map_to_the_claude_code_package() {
        let all = crate::profile::Profile::builtins();
        let pkg = |name: &str| {
            let p = all.iter().find(|p| p.name == name).expect(name);
            package_for(p, &all)
        };
        for name in ["claude", "kimi", "glm", "deepseek", "qwen-api"] {
            assert_eq!(
                pkg(name).as_deref(),
                Some("@anthropic-ai/claude-code"),
                "{name} 跑的是 claude 这个二进制，更新动的就是这个包"
            );
        }
        // `dc` 自己有 `[install]`，用自己的那条，不去问 owner。
        assert_eq!(pkg("dc").as_deref(), Some("@qwen-code/qwen-code"));
        // `shell` 没有 `[install]`，`command[0]` 也不归任何 profile 所有。
        assert_eq!(pkg("shell"), None);
    }

    /// 两个 profile 落到同一个包上，只问仓库一次。五个 profile 跑同一个
    /// `claude`，逐个问就是五次同样的请求；网差的教室里那是五个 5 秒。
    ///
    /// **只数 `fake-agent` 那些请求**：`only = None` 会把这台机器上真装着
    /// 的 npm agent 也扫一遍，数全部的话这条测试的绿灯就取决于跑它的人
    /// 装了什么（同 `checking_everything_covers_the_managed_agent_too`）。
    #[test]
    fn two_profiles_sharing_a_package_are_only_asked_once() {
        let t = tempfile::tempdir().unwrap();
        let profiles = dependent_profile_dir(&t);
        let table = new_table();
        // `Cell` 而不是 `let mut`：`fetch` 是 `&dyn Fn`，闭包改不了捕获的变量。
        let calls = std::cell::Cell::new(0);
        check_updates(&profiles, &table, None, &|pkg| {
            if pkg == "fake-agent" {
                calls.set(calls.get() + 1);
            }
            Some("1.2.0".into())
        });
        assert_eq!(calls.get(), 1, "同一个包问两遍就是白等一次网络往返");
        let g = table.lock().unwrap();
        for name in ["fakeagent", "faketwin"] {
            assert_eq!(
                g.get(name).unwrap().latest.as_deref(),
                Some("1.2.0"),
                "{name} 也得拿到那一次的结果，不是只有先问的那个有"
            );
        }
    }

    #[test]
    fn only_rechecks_the_named_profile() {
        let t = tempfile::tempdir().unwrap();
        let (profiles, _) = managed_profile_dir(&t);
        let table = new_table();
        // `Cell` 而不是 `let mut`：`fetch` 是 `&dyn Fn`，闭包改不了捕获的变量。
        let calls = std::cell::Cell::new(0);
        check_updates(&profiles, &table, Some("shell"), &|_| {
            calls.set(calls.get() + 1);
            None
        });
        assert_eq!(calls.get(), 0, "只查 shell，fake 一次都不该问");
        assert!(table.lock().unwrap().is_empty());
    }
}
