### Task 4: 棋盘配置和记录文件

**Files:**
- Create: `src/game/mod.rs`、`src/game/profile.rs`、`src/game/log.rs`
- Modify: `src/lib.rs`（加 `pub mod game;`，放在 `pub mod gate;` 上面）、`src/journal.rs`（`fn civil_from_days` 前面加 `pub(crate)`）

**Interfaces:**
- Consumes: `dct_game::play::Profile`。
- Produces: `profile::load(home: &Path, game: &str) -> Result<Loaded, String>`，`Loaded { profile, sha256, source }`；`profile::games_dir(home) -> PathBuf`；`log::LogFile::open(home, secs_since_epoch) -> io::Result<LogFile>`，`.path()`，`.append(&Value)`。

注意：`src/game/mod.rs` 这一步先只声明 `profile`、`log` 两个模块；`cli`、`skill`、`text` 在后面的任务里加（否则编不过）。


- [ ] **创建 `src/game/mod.rs`**（下面的代码已在临时副本里编译、跑过测试）

```rust
//! `dct game`：让 dct 自己玩三消游戏。选步是纯算法（`crates/dct-game`），这里只管棋盘配置、
//! 记录、给用户看的话、装给 agent 的说明卡，和命令本身（设计：
//! docs/superpowers/specs/2026-10-02-dct-match3-play-design.md）。
pub mod log;
pub mod profile;
```

- [ ] **创建 `src/game/profile.rs`**（下面的代码已在临时副本里编译、跑过测试）

```rust
//! 棋盘配置：窗口、棋盘在窗口里的位置、行列数、读棋盘的参数。第一轮手写：内置一份 Candy Crush，
//! `~/.dct/games/<游戏>.toml` 存在就用它。棋盘位置每关不同，换关要改——自动校准是下一轮的事。
use dct_game::play::Profile;
use serde::Deserialize;
use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};

/// 第 1712 关的版面（出处：dc-octo `tests/grid_real.rs` 的 region；odd_share 0.15 是 dco 实测：
/// 包装糖也标得出来，且不误标普通糖）。
pub const CANDY_CRUSH: &str = r#"window = { app = "iPhone Mirroring" }
region = { x = 0.2372, y = 0.3156, w = 0.5257, h = 0.4622 }
rows = 9
cols = 5
inset = 0.6
class_de = 24
top_div = 5
odd_share = 0.15
"#;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct File {
    window: Window,
    region: Region,
    rows: usize,
    cols: usize,
    inset: Option<f64>,
    class_de: Option<f64>,
    top_div: Option<u32>,
    odd_share: Option<f64>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Window {
    app: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Region {
    x: f64,
    y: f64,
    w: f64,
    h: f64,
}

pub struct Loaded {
    pub profile: Profile,
    /// 配置文本的 sha256（十六进制），记进每一步，事后才知道那一步是按哪份配置走的。
    pub sha256: String,
    /// 说给用户听的来源：「内置的」或文件路径。
    pub source: String,
}

pub fn games_dir(home: &Path) -> PathBuf {
    home.join(".dct").join("games")
}

pub fn load(home: &Path, game: &str) -> Result<Loaded, String> {
    if game.is_empty() || !game.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-') {
        return Err(format!("游戏名「{game}」只能用小写英文字母、数字和短横线"));
    }
    let path = games_dir(home).join(format!("{game}.toml"));
    let (text, source) = match std::fs::read_to_string(&path) {
        Ok(t) => (t, path.display().to_string()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            if game == "candy-crush" {
                (CANDY_CRUSH.to_string(), "内置的".to_string())
            } else {
                return Err(format!("不认识游戏「{game}」。现在只内置了 candy-crush，其它的要自己写 {}", path.display()));
            }
        }
        Err(e) => return Err(format!("读不了 {}：{e}", path.display())),
    };
    let f: File = toml::from_str(&text).map_err(|e| format!("{source} 写得不对：{e}"))?;
    check(&f).map_err(|m| format!("{source} 写得不对：{m}"))?;
    let mut extra = Map::new();
    if let Some(v) = f.inset {
        extra.insert("inset".into(), json!(v));
    }
    if let Some(v) = f.class_de {
        extra.insert("class_de".into(), json!(v));
    }
    if let Some(v) = f.top_div {
        extra.insert("top_div".into(), json!(v));
    }
    if let Some(v) = f.odd_share {
        extra.insert("odd_share".into(), json!(v));
    }
    let sha256 = Sha256::digest(text.as_bytes()).iter().map(|b| format!("{b:02x}")).collect();
    Ok(Loaded {
        profile: Profile {
            window: json!({ "app": f.window.app }),
            region: [f.region.x, f.region.y, f.region.w, f.region.h],
            rows: f.rows,
            cols: f.cols,
            extra: Value::Object(extra),
        },
        sha256,
        source,
    })
}

fn check(f: &File) -> Result<(), String> {
    let r = &f.region;
    let inside = |v: f64| (0.0..=1.0).contains(&v);
    if ![r.x, r.y, r.w, r.h].iter().all(|&v| inside(v)) || r.w <= 0.0 || r.h <= 0.0 || r.x + r.w > 1.0 || r.y + r.h > 1.0 {
        return Err("region 要整个落在窗口里：x、y、w、h 都取 0 到 1，w 和 h 大于 0，x+w、y+h 不超过 1".into());
    }
    if !(1..=32).contains(&f.rows) || !(1..=32).contains(&f.cols) {
        return Err("rows、cols 都要在 1 到 32 之间".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn home_with(game: &str, text: &str) -> tempfile::TempDir {
        let h = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(games_dir(h.path())).unwrap();
        std::fs::write(games_dir(h.path()).join(format!("{game}.toml")), text).unwrap();
        h
    }

    #[test]
    fn the_built_in_candy_crush_loads_and_passes_its_own_checks() {
        let h = tempfile::tempdir().unwrap();
        let l = load(h.path(), "candy-crush").unwrap();
        assert_eq!((l.profile.rows, l.profile.cols), (9, 5));
        assert_eq!(l.profile.window["app"], "iPhone Mirroring");
        assert_eq!(l.profile.extra["odd_share"], 0.15);
        assert_eq!(l.source, "内置的");
        assert_eq!(l.sha256.len(), 64);
    }

    #[test]
    fn a_file_in_the_games_folder_wins_and_changes_the_fingerprint() {
        let h = home_with("candy-crush", &CANDY_CRUSH.replace("rows = 9", "rows = 8"));
        let l = load(h.path(), "candy-crush").unwrap();
        assert_eq!(l.profile.rows, 8);
        let builtin = load(tempfile::tempdir().unwrap().path(), "candy-crush").unwrap();
        assert_ne!(l.sha256, builtin.sha256);
        assert!(l.source.ends_with("candy-crush.toml"));
    }

    #[test]
    fn a_typo_names_the_file_and_the_line() {
        let h = home_with("candy-crush", &CANDY_CRUSH.replace("cols = 5", "colz = 5"));
        let e = load(h.path(), "candy-crush").err().unwrap();
        assert!(e.contains("candy-crush.toml") && e.contains("colz"), "{e}");
    }

    #[test]
    fn out_of_range_values_are_refused_with_a_reason() {
        for (from, to) in [("rows = 9", "rows = 0"), ("cols = 5", "cols = 33"), ("w = 0.5257", "w = 0.9"), ("w = 0.5257", "w = 0.0")] {
            let h = home_with("candy-crush", &CANDY_CRUSH.replace(from, to));
            assert!(load(h.path(), "candy-crush").is_err(), "{to} 该被拒");
        }
    }

    #[test]
    fn unknown_games_and_unsafe_names_are_refused() {
        let h = tempfile::tempdir().unwrap();
        assert!(load(h.path(), "chess").err().unwrap().contains("不认识"));
        for bad in ["", "../x", "A", "a/b", "a.b"] {
            assert!(load(h.path(), bad).is_err(), "{bad:?}");
        }
    }
}
```

- [ ] **创建 `src/game/log.rs`**（下面的代码已在临时副本里编译、跑过测试）

```rust
//! 每一步一行 JSON，追加到 `~/.dct/games/log/<日期>.jsonl`。每行写完就落盘：玩到一半被 Ctrl-C，
//! 已经走过的那些步不会丢。以后拿这些记录和大模型的选择比。
use serde_json::Value;
use std::io::Write;
use std::path::{Path, PathBuf};

pub struct LogFile {
    path: PathBuf,
}

impl LogFile {
    pub fn open(home: &Path, secs_since_epoch: u64) -> std::io::Result<LogFile> {
        let dir = super::profile::games_dir(home).join("log");
        std::fs::create_dir_all(&dir)?;
        let (y, m, d) = crate::journal::civil_from_days((secs_since_epoch / 86_400) as i64);
        Ok(LogFile { path: dir.join(format!("{y:04}-{m:02}-{d:02}.jsonl")) })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn append(&self, v: &Value) -> std::io::Result<()> {
        let mut f = std::fs::OpenOptions::new().create(true).append(true).open(&self.path)?;
        writeln!(f, "{v}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn lines_are_appended_to_a_file_named_by_the_utc_date() {
        let h = tempfile::tempdir().unwrap();
        // 2026-10-02 12:00:00 UTC
        let l = LogFile::open(h.path(), 1_790_942_400).unwrap();
        assert!(l.path().ends_with("2026-10-02.jsonl"), "{:?}", l.path());
        l.append(&json!({"a": 1})).unwrap();
        l.append(&json!({"b": 2})).unwrap();
        let again = LogFile::open(h.path(), 1_790_942_400 + 60).unwrap();
        again.append(&json!({"c": 3})).unwrap();
        let text = std::fs::read_to_string(l.path()).unwrap();
        let lines: Vec<Value> = text.lines().map(|x| serde_json::from_str(x).unwrap()).collect();
        assert_eq!(lines, vec![json!({"a": 1}), json!({"b": 2}), json!({"c": 3})]);
    }

    #[test]
    fn a_new_day_is_a_new_file() {
        let h = tempfile::tempdir().unwrap();
        let a = LogFile::open(h.path(), 1_790_942_400).unwrap();
        let b = LogFile::open(h.path(), 1_790_942_400 + 86_400).unwrap();
        assert_ne!(a.path(), b.path());
    }
}
```

- [ ] **Step 2: 跑测试**

Run: `cargo test --lib game::`
Expected: `7 passed`（profile 5 个 + log 2 个）。`1790942400` 是 2026-10-02 12:00:00 UTC（已用 Python 核对）。

- [ ] **Step 3: 提交**

```bash
git add src/lib.rs src/journal.rs src/game
git commit -m "feat(game): board profile (built-in Candy Crush, overridable) and the per-step log file"
```


---

