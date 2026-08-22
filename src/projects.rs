use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// 列表上限。20 条足够覆盖手头在做的项目，再多列表本身就难挑了。
const MAX: usize = 20;

/// 磁盘格式。包一层对象而不是直接存数组，是为了将来加字段时老文件仍能读。
#[derive(Default, Serialize, Deserialize)]
struct Disk {
    #[serde(default)]
    recent: Vec<String>,
    /// **旧字段，留着当兜底。** 升级之前只有这一个全局值；升级之后新会话
    /// 一律写进 `project_profiles`，但还没开过会话的老项目要靠它，否则
    /// 一升级所有项目的 `n` 都退化成弹选择器。
    #[serde(default)]
    last_profile: Option<String>,
    /// 用户按 `p` 摆上看板、还没有会话的项目。落盘而不是只放内存里：
    /// 规则是「`x` 才能移除」，不落盘的话重启 dct 就自己没了，两句话对不上。
    ///
    /// 存的是**用户当初敲的那条路径**，不是 `key_of` 归一化之后的那条。
    /// 这一份同时是界面上组头 name/parent 的显示来源（见
    /// `ui::view::group_sessions`），而重启之后界面手里的 `pinned` 完全来自
    /// 这个文件——存归一化结果的话，用户 pin 的 `…/我敲的名字` 会在下次
    /// 启动时自己变成 `/private/var/…/真实名字`，「canon 只用于比较、
    /// 永不用于显示」这条规矩就在进程边界上破了。判重、`unpin` 一律走
    /// `key_of` 现算，不靠存的形式。
    #[serde(default)]
    pinned: Vec<String>,
    /// 项目目录 → 上次在这个项目里开会话用的 agent。
    /// 用 `BTreeMap` 不是 `HashMap`：落盘顺序稳定，`projects.json` 的 diff
    /// 才不会每次都乱跳。
    #[serde(default)]
    project_profiles: BTreeMap<String, String>,
}

/// 最近开过会话的项目目录，最近使用的在最前；外加上次用的 agent。
pub struct Store {
    path: PathBuf,
    recent: Vec<String>,
    last_profile: Option<String>,
    pinned: Vec<String>,
    project_profiles: BTreeMap<String, String>,
}

/// 存放位置跟着 socket 走，而不是直接拼 `$HOME`。生产环境 socket 在
/// `~/.dct/daemon.sock`，推出来就是 `~/.dct/projects.json`，与直接拼 `$HOME` 同一个
/// 文件；而集成测试把 socket 建在临时目录里，于是自动拿到一份隔离的 store，
/// 不会去动你真实的那份。
pub fn store_path_for_socket(socket: &Path) -> PathBuf {
    match socket.parent() {
        Some(d) => d.join("projects.json"),
        None => PathBuf::from("projects.json"),
    }
}

/// 以路径为键时统一走这里。`.` 和 `/abs/path` 必须落在同一个键上，
/// 否则同一个项目会在 `recent`、`pinned`、`project_profiles` 里各占一行。
///
/// 归一失败（目录刚被删）就用原样：丢掉这一条比存个粗糙的路径更糟。
fn key_of(dir: &Path) -> String {
    std::fs::canonicalize(dir)
        .unwrap_or_else(|_| dir.to_path_buf())
        .display()
        .to_string()
}

impl Store {
    /// 文件不存在、JSON 语法错、字段类型不对——一律当空列表。
    /// 这是便利性缓存，不值得为它让守护进程起不来。
    pub fn load(path: &Path) -> Store {
        let disk = std::fs::read_to_string(path)
            .ok()
            .and_then(|s| serde_json::from_str::<Disk>(&s).ok())
            .unwrap_or_default();
        Store {
            path: path.to_path_buf(),
            recent: disk.recent,
            last_profile: disk.last_profile,
            pinned: disk.pinned,
            project_profiles: disk.project_profiles,
        }
    }

    pub fn list(&self) -> Vec<String> {
        self.recent.clone()
    }

    /// 记一笔：去重、提到最前、截断、落盘。
    pub fn touch(&mut self, dir: &Path) {
        let key = key_of(dir);
        self.recent.retain(|p| p != &key);
        self.recent.insert(0, key);
        self.recent.truncate(MAX);
        self.save();
    }

    /// 这个项目上次用的 agent。没有单独记录就吃全局的旧值（见 `Disk::last_profile`）。
    pub fn last_profile_for(&self, dir: &Path) -> Option<String> {
        self.project_profiles
            .get(&key_of(dir))
            .cloned()
            .or_else(|| self.last_profile.clone())
    }

    /// 记一笔「这个项目上次用的 agent」。同时刷新全局兜底值——一个刚被
    /// `p` 摆上看板、从没开过会话的新项目，`n` 该给的是「你最近在用的那个」，
    /// 而不是空。
    pub fn set_last_profile_for(&mut self, dir: &Path, name: &str) {
        self.project_profiles.insert(key_of(dir), name.to_string());
        self.last_profile = Some(name.to_string());
        self.save();
    }

    pub fn pinned(&self) -> Vec<String> {
        self.pinned.clone()
    }

    /// 摆一个项目上看板。已经在里面就什么都不做——重复 pin 不该出现两行。
    ///
    /// **存原样、按归一化判重。** 存的那条要拿去显示（组头上的项目名），
    /// 判重的那条得认得出「同一个目录的两种拼法」（走符号链接、`/tmp` 与
    /// `/private/tmp`）——两件事要的不是同一个字符串，所以分开。
    pub fn pin(&mut self, dir: &Path) {
        let key = key_of(dir);
        if !self.pinned.iter().any(|p| key_of(Path::new(p)) == key) {
            self.pinned.push(dir.display().to_string());
            self.save();
        }
    }

    /// 同上，按归一化后的路径删：存的是用户的拼写，用户这次可能换了另一种
    /// 拼法来敲。字面比对删不掉的话，`x` 看起来就是「按了没反应」。
    pub fn unpin(&mut self, dir: &Path) {
        let key = key_of(dir);
        self.pinned.retain(|p| key_of(Path::new(p)) != key);
        self.save();
    }

    /// 落盘失败一律忽略：丢的是便利性，不是数据。内存里的列表照常可用。
    fn save(&self) {
        let Some(parent) = self.path.parent() else {
            return;
        };
        if std::fs::create_dir_all(parent).is_err() {
            return;
        }
        let Ok(json) = serde_json::to_string(&Disk {
            recent: self.recent.clone(),
            last_profile: self.last_profile.clone(),
            pinned: self.pinned.clone(),
            project_profiles: self.project_profiles.clone(),
        }) else {
            return;
        };
        // 原子写：先写同目录的临时文件再 rename。直接覆写的话，写到一半断电
        // 会留下半截 JSON，下次 load 解析失败就把整个列表丢了。
        let tmp = self.path.with_extension("json.tmp");
        if std::fs::write(&tmp, json).is_err() {
            return;
        }
        let _ = std::fs::rename(&tmp, &self.path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// canonicalize 会把 macOS 上 `/var/...` 的临时目录解成 `/private/var/...`，
    /// 所以断言里的期望值必须做同样的归一，否则测试在 macOS 上必失败。
    fn canon(p: &std::path::Path) -> String {
        std::fs::canonicalize(p).unwrap().display().to_string()
    }

    #[test]
    fn touch_moves_existing_entry_to_front() {
        let tmp = tempfile::tempdir().unwrap();
        let a = tmp.path().join("a");
        let b = tmp.path().join("b");
        std::fs::create_dir(&a).unwrap();
        std::fs::create_dir(&b).unwrap();

        let mut s = Store::load(&tmp.path().join("projects.json"));
        s.touch(&a);
        s.touch(&b);
        s.touch(&a);

        assert_eq!(
            s.list(),
            vec![canon(&a), canon(&b)],
            "重复项要去重并提到最前"
        );
    }

    #[test]
    fn touch_caps_at_twenty() {
        let tmp = tempfile::tempdir().unwrap();
        let mut s = Store::load(&tmp.path().join("projects.json"));
        for i in 0..25 {
            let d = tmp.path().join(format!("p{i}"));
            std::fs::create_dir(&d).unwrap();
            s.touch(&d);
        }
        let list = s.list();
        assert_eq!(list.len(), 20, "上限 20 条");
        assert_eq!(list[0], canon(&tmp.path().join("p24")), "最新的在最前");
        assert!(
            !list.contains(&canon(&tmp.path().join("p0"))),
            "最旧的应当被挤掉"
        );
    }

    #[test]
    fn corrupt_json_degrades_to_empty() {
        let tmp = tempfile::tempdir().unwrap();
        let f = tmp.path().join("projects.json");
        std::fs::write(&f, "{ 这不是 JSON").unwrap();
        let s = Store::load(&f);
        assert!(s.list().is_empty(), "损坏的文件必须当空列表，不能 panic");
    }

    #[test]
    fn missing_file_degrades_to_empty() {
        let tmp = tempfile::tempdir().unwrap();
        let s = Store::load(&tmp.path().join("没有这个文件.json"));
        assert!(s.list().is_empty());
    }

    #[test]
    fn touch_survives_reload() {
        let tmp = tempfile::tempdir().unwrap();
        let f = tmp.path().join("projects.json");
        let d = tmp.path().join("proj");
        std::fs::create_dir(&d).unwrap();

        let mut s = Store::load(&f);
        s.touch(&d);
        drop(s);

        let s2 = Store::load(&f);
        assert_eq!(
            s2.list(),
            vec![canon(&d)],
            "touch 必须落盘，重新 load 读得回"
        );
    }

    #[test]
    fn touch_keeps_unresolvable_path_as_is() {
        let tmp = tempfile::tempdir().unwrap();
        let gone = tmp.path().join("已经删掉了");
        let mut s = Store::load(&tmp.path().join("projects.json"));
        s.touch(&gone);
        assert_eq!(
            s.list(),
            vec![gone.display().to_string()],
            "canonicalize 失败时存原样，不能丢掉这一条"
        );
    }

    #[test]
    fn store_path_sits_next_to_socket() {
        let p = store_path_for_socket(std::path::Path::new("/home/x/.dct/daemon.sock"));
        assert_eq!(p, std::path::PathBuf::from("/home/x/.dct/projects.json"));
    }

    #[test]
    fn old_file_without_last_profile_still_loads() {
        // 已经在用 dct 的人，projects.json 里没有这个字段
        let tmp = tempfile::tempdir().unwrap();
        let f = tmp.path().join("projects.json");
        std::fs::write(&f, r#"{"recent":["/a"]}"#).unwrap();
        let s = Store::load(&f);
        assert_eq!(s.list(), vec!["/a".to_string()]);
        assert_eq!(s.last_profile_for(tmp.path()), None);
    }

    #[test]
    fn each_project_remembers_its_own_agent() {
        let tmp = tempfile::tempdir().unwrap();
        let a = tmp.path().join("a");
        let b = tmp.path().join("b");
        std::fs::create_dir(&a).unwrap();
        std::fs::create_dir(&b).unwrap();

        let f = tmp.path().join("projects.json");
        let mut s = Store::load(&f);
        s.set_last_profile_for(&a, "claude");
        s.set_last_profile_for(&b, "codex");

        let s = Store::load(&f);
        assert_eq!(s.last_profile_for(&a).as_deref(), Some("claude"));
        assert_eq!(s.last_profile_for(&b).as_deref(), Some("codex"));
    }

    /// 老文件里只有一个全局 `last_profile`。升级之后每个项目都还没有自己的记录，
    /// 这时候必须回退到那个全局值——否则老用户一升级，所有项目的 `n` 都变成
    /// 「弹选择器」，看起来像是设置丢了。
    #[test]
    fn an_unknown_project_falls_back_to_the_old_global_agent() {
        let tmp = tempfile::tempdir().unwrap();
        let f = tmp.path().join("projects.json");
        std::fs::write(&f, r#"{"recent":[],"last_profile":"kimi"}"#).unwrap();

        let s = Store::load(&f);
        assert_eq!(
            s.last_profile_for(tmp.path()).as_deref(),
            Some("kimi"),
            "没有单独记录的项目要吃全局兜底"
        );
    }

    #[test]
    fn pinned_projects_dedupe_and_survive_reload() {
        let tmp = tempfile::tempdir().unwrap();
        let a = tmp.path().join("a");
        std::fs::create_dir(&a).unwrap();
        let f = tmp.path().join("projects.json");

        let mut s = Store::load(&f);
        s.pin(&a);
        s.pin(&a);
        assert_eq!(
            Store::load(&f).pinned(),
            vec![a.display().to_string()],
            "重复 pin 不该出现两行"
        );

        let mut s = Store::load(&f);
        s.unpin(&a);
        assert!(Store::load(&f).pinned().is_empty());
    }

    /// **pin 上去的项目，重启之后名字不许自己变。**
    ///
    /// `pinned` 同时是界面上组头 name/parent 的显示来源，而重启之后界面手里
    /// 那一份完全来自这个文件。存归一化结果的话，用户 pin 的
    /// `…/我敲的名字`（一条符号链接）下次启动会显示成 `…/真实名字`，在
    /// macOS 上还会连父目录一起变成 `/private/var/…`——「canon 只用于比较、
    /// 永不用于显示」这条规矩在一次进程重启上就破了，而用户什么都没做。
    /// 符号链接：Windows 上建它要开发者模式或管理员权限，摆不出这个现场。
    #[test]
    #[cfg(unix)]
    fn a_pinned_project_keeps_the_users_spelling_across_a_reload() {
        let tmp = tempfile::tempdir().unwrap();
        let real = tmp.path().join("真实名字");
        std::fs::create_dir(&real).unwrap();
        let typed = tmp.path().join("我敲的名字");
        std::os::unix::fs::symlink(&real, &typed).unwrap();
        let f = tmp.path().join("projects.json");

        let mut s = Store::load(&f);
        s.pin(&typed);
        drop(s);

        assert_eq!(
            Store::load(&f).pinned(),
            vec![typed.display().to_string()],
            "重新 load 出来的必须还是用户敲的那条路径"
        );
        // 另一种拼法指的是同一个项目：既不该多出一行，也要 unpin 得掉
        let mut s = Store::load(&f);
        s.pin(&real);
        assert_eq!(Store::load(&f).pinned().len(), 1, "两种拼法是同一个项目");
        let mut s = Store::load(&f);
        s.unpin(&real);
        assert!(
            Store::load(&f).pinned().is_empty(),
            "换一种拼法也要拿得掉，不然 `x` 看起来像按了没反应"
        );
    }

    /// 老文件没有 `pinned` / `project_profiles` 两个字段，必须照常读出来，
    /// 不能整份 JSON 解析失败把 `recent` 也一起丢掉。
    #[test]
    fn an_old_file_without_the_new_fields_still_loads() {
        let tmp = tempfile::tempdir().unwrap();
        let f = tmp.path().join("projects.json");
        std::fs::write(&f, r#"{"recent":["/x"],"last_profile":"claude"}"#).unwrap();

        let s = Store::load(&f);
        assert_eq!(s.list(), vec!["/x".to_string()]);
        assert!(s.pinned().is_empty());
    }
}
