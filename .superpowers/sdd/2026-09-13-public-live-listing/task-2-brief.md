### Task 2: `dct-srv` —— 发布密钥文件与命令行

**Files:**
- Create: `crates/dct-srv/src/keys.rs`
- Modify: `crates/dct-srv/Cargo.toml`（加 `getrandom = "0.2"`）
- Modify: `crates/dct-srv/src/lib.rs`（`pub mod keys;`、`pub fn parse_cli`、`pub enum Cli`）
- Modify: `crates/dct-srv/src/main.rs`

**Interfaces:**
- Consumes: 无。
- Produces:
  - `pub struct PublishKeys { pub keys: Vec<KeyEntry>, pub blocked: Vec<String> }`（`Serialize/Deserialize/Clone/Default/Debug/PartialEq`）
  - `pub struct KeyEntry { pub name: String, pub hash: String, pub created: u64 }`
  - `impl PublishKeys`：`load(&Path) -> Result<PublishKeys, String>`、`save(&self, &Path) -> std::io::Result<()>`、`add(&mut self, name: &str, now: u64) -> Result<String, String>`、`revoke(&mut self, name: &str) -> bool`、`block(&mut self, id: &str)`、`name_for(&self, key: &str) -> Option<String>`、`is_blocked(&self, id: &str) -> bool`
  - `pub struct KeyFile`：`open(PathBuf) -> Result<KeyFile, String>`、`keys(&self) -> &PublishKeys`、`reload_if_changed(&mut self) -> Result<bool, String>`
  - `pub enum Cli { Serve { addr: String, with_link: bool, publish_keys: Option<PathBuf> }, KeyAdd { name: String, file: PathBuf }, KeyRevoke { name: String, file: PathBuf }, KeyList { file: PathBuf }, Takedown { id: String, file: PathBuf } }`
  - `pub fn parse_cli(args: &[String]) -> Result<Cli, String>`

- [ ] **Step 1: 写失败的测试**

创建 `crates/dct-srv/src/keys.rs`，先只放测试模块（实现留空会编译失败，正是这一步要的）：

```rust
//! 发布密钥文件。见 `docs/superpowers/specs/2026-09-13-public-live-listing-design.md`
//! 「发布密钥与总开关」。

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp_file() -> (tempfile::TempDir, std::path::PathBuf) {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("publish-keys.json");
        (d, p)
    }

    /// 生成的密钥只返回一次，文件里只有摘要；拿原文能认出是谁的。
    #[test]
    fn a_key_is_stored_as_a_digest_and_recognised_by_name() {
        let mut k = PublishKeys::default();
        let key = k.add("姜老师", 1).unwrap();
        assert_eq!(key.len(), 64);
        assert!(!serde_json::to_string(&k).unwrap().contains(&key), "文件里不许有原文");
        assert_eq!(k.name_for(&key).as_deref(), Some("姜老师"));
        assert_eq!(k.name_for(&"0".repeat(64)), None);
        assert!(k.add("姜老师", 2).is_err(), "重名要拒");
    }

    #[test]
    fn revoking_and_blocking() {
        let mut k = PublishKeys::default();
        let key = k.add("a", 1).unwrap();
        assert!(k.revoke("a"));
        assert!(!k.revoke("a"), "吊销不存在的名字回 false");
        assert_eq!(k.name_for(&key), None);
        k.block("room1");
        k.block("room1");
        assert!(k.is_blocked("room1"));
        assert_eq!(k.blocked.len(), 1, "重复下线不重复记");
    }

    #[test]
    fn save_then_load_round_trips_with_owner_only_permissions() {
        let (_d, p) = tmp_file();
        let mut k = PublishKeys::default();
        k.add("a", 1).unwrap();
        k.save(&p).unwrap();
        assert_eq!(PublishKeys::load(&p).unwrap(), k);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&p).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o600);
        }
    }

    /// **写坏文件不能等于吊销全部，也不能等于全部放行**：保留上一份。
    #[test]
    fn a_broken_file_keeps_the_last_good_keys() {
        let (_d, p) = tmp_file();
        let mut k = PublishKeys::default();
        let key = k.add("a", 1).unwrap();
        k.save(&p).unwrap();
        let mut f = KeyFile::open(p.clone()).unwrap();
        // 保证 mtime 真的变了（有些文件系统精度是秒）
        std::thread::sleep(std::time::Duration::from_millis(1100));
        std::fs::write(&p, "{ not json").unwrap();
        assert!(f.reload_if_changed().is_err());
        assert_eq!(f.keys().name_for(&key).as_deref(), Some("a"), "坏文件不能冲掉旧密钥");
    }

    #[test]
    fn a_changed_file_is_picked_up() {
        let (_d, p) = tmp_file();
        let mut k = PublishKeys::default();
        let key = k.add("a", 1).unwrap();
        k.save(&p).unwrap();
        let mut f = KeyFile::open(p.clone()).unwrap();
        assert_eq!(f.reload_if_changed(), Ok(false), "没变就不重读");
        std::thread::sleep(std::time::Duration::from_millis(1100));
        k.revoke("a");
        k.save(&p).unwrap();
        assert_eq!(f.reload_if_changed(), Ok(true));
        assert_eq!(f.keys().name_for(&key), None);
    }

    #[test]
    fn opening_a_broken_file_fails_outright() {
        let (_d, p) = tmp_file();
        std::fs::write(&p, "nope").unwrap();
        assert!(KeyFile::open(p).is_err(), "首次启动就坏的文件：拒绝启动");
    }
}
```

`crates/dct-srv/Cargo.toml` 的 `[dev-dependencies]` 加 `tempfile = "3"`（先在根 `Cargo.lock` 里确认已有 `tempfile`，版本跟根包一致）。

在 `crates/dct-srv/src/lib.rs` 顶部 `mod live;` 旁加 `pub mod keys;`，并在 `lib.rs` 的 `mod tests` 里追加命令行解析测试：

```rust
    #[test]
    fn the_command_line_is_parsed_into_one_of_five_shapes() {
        let a = |s: &str| s.split_whitespace().map(String::from).collect::<Vec<_>>();
        assert_eq!(
            parse_cli(&a("")).unwrap(),
            Cli::Serve { addr: "127.0.0.1:8787".into(), with_link: false, publish_keys: None }
        );
        assert_eq!(
            parse_cli(&a("127.0.0.1:9000 --with-link --publish-keys /k.json")).unwrap(),
            Cli::Serve {
                addr: "127.0.0.1:9000".into(),
                with_link: true,
                publish_keys: Some("/k.json".into())
            }
        );
        assert_eq!(
            parse_cli(&a("key add 姜老师 --file /k.json")).unwrap(),
            Cli::KeyAdd { name: "姜老师".into(), file: "/k.json".into() }
        );
        assert_eq!(
            parse_cli(&a("key revoke a --file /k.json")).unwrap(),
            Cli::KeyRevoke { name: "a".into(), file: "/k.json".into() }
        );
        assert_eq!(parse_cli(&a("key list --file /k.json")).unwrap(), Cli::KeyList { file: "/k.json".into() });
        assert_eq!(
            parse_cli(&a("takedown abc --file /k.json")).unwrap(),
            Cli::Takedown { id: "abc".into(), file: "/k.json".into() }
        );
        assert!(parse_cli(&a("key add a")).is_err(), "管理命令必须写明 --file");
        assert!(parse_cli(&a("--publish-keys")).is_err(), "参数缺值要报错");
    }
```

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test -p dct-srv`
Expected: 编译失败（`PublishKeys`、`KeyFile`、`parse_cli` 未定义）。

- [ ] **Step 3: 实现 `keys.rs`**

在 `keys.rs` 的测试模块之前写：

```rust
use std::path::{Path, PathBuf};
use std::time::SystemTime;

#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct PublishKeys {
    #[serde(default)]
    pub keys: Vec<KeyEntry>,
    /// 被运营方下线的房间号（`dct-srv takedown`）。停播之前都公开不了。
    #[serde(default)]
    pub blocked: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct KeyEntry {
    pub name: String,
    /// SHA-256 的小写 hex。**原文不落盘**，同直播那两把钥匙的做法。
    pub hash: String,
    /// 创建时间，Unix 秒。只给 `key list` 看。
    pub created: u64,
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn digest_hex(key: &str) -> String {
    use sha2::{Digest, Sha256};
    hex(&Sha256::digest(key.as_bytes()))
}

/// 常数时间比较两个等长 hex 串；长度不同直接不等（长度不是秘密）。
fn same_hex(a: &str, b: &str) -> bool {
    a.len() == b.len() && a.bytes().zip(b.bytes()).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

impl PublishKeys {
    pub fn load(path: &Path) -> Result<PublishKeys, String> {
        let raw = std::fs::read_to_string(path).map_err(|e| format!("读不了 {}：{e}", path.display()))?;
        serde_json::from_str(&raw).map_err(|e| format!("{} 不是合法的密钥文件：{e}", path.display()))
    }

    /// 先写同目录临时文件再改名：写到一半断电，读到的要么是旧文件要么是新文件。
    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        let tmp = path.with_extension("tmp");
        let body = serde_json::to_vec_pretty(self).expect("PublishKeys 总能序列化");
        {
            let mut opts = std::fs::OpenOptions::new();
            opts.write(true).create(true).truncate(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                opts.mode(0o600);
            }
            let mut f = opts.open(&tmp)?;
            std::io::Write::write_all(&mut f, &body)?;
            f.sync_all()?;
        }
        std::fs::rename(&tmp, path)
    }

    pub fn add(&mut self, name: &str, now: u64) -> Result<String, String> {
        if name.trim().is_empty() {
            return Err("名字不能为空".into());
        }
        if self.keys.iter().any(|k| k.name == name) {
            return Err(format!("已经有一把叫「{name}」的密钥了"));
        }
        let mut raw = [0u8; 32];
        getrandom::getrandom(&mut raw).map_err(|e| format!("系统随机数不可用：{e}"))?;
        let key = hex(&raw);
        self.keys.push(KeyEntry { name: name.to_string(), hash: digest_hex(&key), created: now });
        Ok(key)
    }

    pub fn revoke(&mut self, name: &str) -> bool {
        let before = self.keys.len();
        self.keys.retain(|k| k.name != name);
        self.keys.len() != before
    }

    pub fn block(&mut self, id: &str) {
        if !self.is_blocked(id) {
            self.blocked.push(id.to_string());
        }
    }

    /// 这把密钥是谁的。**把每一条都比一遍**，不在第一条命中时提前返回。
    pub fn name_for(&self, key: &str) -> Option<String> {
        let want = digest_hex(key);
        let mut found = None;
        for k in &self.keys {
            if same_hex(&k.hash, &want) {
                found = Some(k.name.clone());
            }
        }
        found
    }

    pub fn is_blocked(&self, id: &str) -> bool {
        self.blocked.iter().any(|b| b == id)
    }
}

/// 运行中的中转手里那份密钥文件：记着上次读到的修改时间，变了才重读。
pub struct KeyFile {
    path: PathBuf,
    stamp: Option<SystemTime>,
    keys: PublishKeys,
}

impl KeyFile {
    /// 首次打开：读不了或者解析失败就报错——调用方据此拒绝启动。
    pub fn open(path: PathBuf) -> Result<KeyFile, String> {
        let keys = PublishKeys::load(&path)?;
        let stamp = std::fs::metadata(&path).and_then(|m| m.modified()).ok();
        Ok(KeyFile { path, stamp, keys })
    }

    pub fn keys(&self) -> &PublishKeys {
        &self.keys
    }

    /// 文件变了就重读。**解析失败保留上一份**并报错，由调用方写日志。
    pub fn reload_if_changed(&mut self) -> Result<bool, String> {
        let stamp = std::fs::metadata(&self.path).and_then(|m| m.modified()).ok();
        if stamp == self.stamp {
            return Ok(false);
        }
        self.stamp = stamp;
        self.keys = PublishKeys::load(&self.path)?;
        Ok(true)
    }
}
```

`crates/dct-srv/Cargo.toml` 的 `[dependencies]` 加：

```toml
# 发布密钥由 `dct-srv key add` 当场生成，必须来自系统随机数（同 `dct` 的 getrandom 用法）。
getrandom = "0.2"
```

- [ ] **Step 4: 实现 `parse_cli`**

在 `crates/dct-srv/src/lib.rs` 的 `must_be_loopback` 之后追加：

```rust
/// 中转的五种启动方式。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Cli {
    Serve { addr: String, with_link: bool, publish_keys: Option<std::path::PathBuf> },
    KeyAdd { name: String, file: std::path::PathBuf },
    KeyRevoke { name: String, file: std::path::PathBuf },
    KeyList { file: std::path::PathBuf },
    Takedown { id: String, file: std::path::PathBuf },
}

/// 手写解析：五种形状，不值得为此引一个参数库。
pub fn parse_cli(args: &[String]) -> Result<Cli, String> {
    fn flag_value(args: &[String], flag: &str) -> Result<Option<std::path::PathBuf>, String> {
        match args.iter().position(|a| a == flag) {
            None => Ok(None),
            Some(i) => args
                .get(i + 1)
                .filter(|v| !v.starts_with("--"))
                .map(|v| Some(v.into()))
                .ok_or_else(|| format!("{flag} 后面要跟一个文件路径")),
        }
    }
    let need_file = |args: &[String]| {
        flag_value(args, "--file")?.ok_or_else(|| "管理命令要写明 --file <密钥文件>".to_string())
    };
    match args.first().map(String::as_str) {
        Some("key") => {
            let name = || args.get(2).cloned().ok_or_else(|| "缺少名字".to_string());
            match args.get(1).map(String::as_str) {
                Some("add") => Ok(Cli::KeyAdd { name: name()?, file: need_file(args)? }),
                Some("revoke") => Ok(Cli::KeyRevoke { name: name()?, file: need_file(args)? }),
                Some("list") => Ok(Cli::KeyList { file: need_file(args)? }),
                _ => Err("用法：dct-srv key add|revoke|list ...".into()),
            }
        }
        Some("takedown") => Ok(Cli::Takedown {
            id: args.get(1).cloned().ok_or_else(|| "缺少房间号".to_string())?,
            file: need_file(args)?,
        }),
        _ => {
            let publish_keys = flag_value(args, "--publish-keys")?;
            let mut skip_next = false;
            let mut addr = None;
            for a in args {
                if skip_next {
                    skip_next = false;
                    continue;
                }
                if a == "--publish-keys" {
                    skip_next = true;
                } else if !a.starts_with("--") && addr.is_none() {
                    addr = Some(a.clone());
                }
            }
            Ok(Cli::Serve {
                addr: addr.unwrap_or_else(|| "127.0.0.1:8787".into()),
                with_link: args.iter().any(|a| a == "--with-link"),
                publish_keys,
            })
        }
    }
}
```

- [ ] **Step 5: 改 `main.rs` 用 `parse_cli`**

把 `main.rs` 里现在的参数处理（`WITH_LINK` 常量、`args`、`routes`、`addr` 那几行）换成：

```rust
    let args: Vec<String> = std::env::args().skip(1).collect();
    let cli = dct_srv::parse_cli(&args)?;
    let now = || {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0)
    };
    let load_or_new = |file: &std::path::Path| {
        if file.exists() {
            dct_srv::keys::PublishKeys::load(file)
        } else {
            Ok(dct_srv::keys::PublishKeys::default())
        }
    };
    let (addr, routes, publish_keys) = match cli {
        dct_srv::Cli::KeyAdd { name, file } => {
            let mut k = load_or_new(&file)?;
            let key = k.add(&name, now())?;
            k.save(&file)?;
            println!("{key}");
            eprintln!("这把密钥只显示这一次。文件里只存了它的摘要。");
            return Ok(());
        }
        dct_srv::Cli::KeyRevoke { name, file } => {
            let mut k = dct_srv::keys::PublishKeys::load(&file)?;
            if !k.revoke(&name) {
                return Err(format!("没有叫「{name}」的密钥").into());
            }
            k.save(&file)?;
            eprintln!("已吊销「{name}」。运行中的中转最多 10 秒后生效。");
            return Ok(());
        }
        dct_srv::Cli::KeyList { file } => {
            for e in dct_srv::keys::PublishKeys::load(&file)?.keys {
                println!("{}\t创建于 {}", e.name, e.created);
            }
            return Ok(());
        }
        dct_srv::Cli::Takedown { id, file } => {
            let mut k = load_or_new(&file)?;
            k.block(&id);
            k.save(&file)?;
            eprintln!("已下线「{id}」。运行中的中转最多 10 秒后生效。");
            return Ok(());
        }
        dct_srv::Cli::Serve { addr, with_link, publish_keys } => (
            addr,
            if with_link { Routes::WithLink } else { Routes::LiveOnly },
            publish_keys,
        ),
    };
    // 首次打开就坏的密钥文件：拒绝启动，而不是悄悄当成「没开公开功能」。
    let publish_keys = publish_keys.map(dct_srv::keys::KeyFile::open).transpose()?;
```

`serve` 的调用在 Task 4 才加上 `publish_keys` 参数；这一步先让 `publish_keys` 以 `let _ = publish_keys;` 暂存，Task 4 替换。

- [ ] **Step 6: 跑测试确认通过**

Run: `cargo test -p dct-srv`
Expected: PASS。

- [ ] **Step 7: 变异检查**

把 `name_for` 里的 `same_hex(&k.hash, &want)` 改成 `true`，确认 `a_key_is_stored_as_a_digest_and_recognised_by_name` 红；把 `reload_if_changed` 的 `self.keys = PublishKeys::load(&self.path)?;` 改成先 `self.keys = PublishKeys::default();` 再 load，确认 `a_broken_file_keeps_the_last_good_keys` 红；改回。

- [ ] **Step 8: Commit**

```bash
git add crates/dct-srv Cargo.lock
git commit -m "feat(srv): publish key file with add, revoke, list and takedown commands"
```

---

