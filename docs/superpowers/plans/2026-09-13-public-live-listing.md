# 公开直播列表 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 在 `live.dataclue.cn/` 上列出老师显式选择公开的直播：持发布密钥才能公开，公开期间无令牌可看，随时能收回。

**Architecture:** 中转（`dct-srv`）是「公开」的唯一真相：房间上挂公开标记，密钥文件定期重载实现吊销/下线。老师本机的守护进程用推帧钥匙 + 自己存的发布密钥去公开；管理台用守护进程签发的「凭证」（推帧钥匙摘要的 HMAC）+ 管理台自己的发布密钥去公开，发布密钥不进学生容器。公开页和观看页住在 `dct-page`，中转发。

**Tech Stack:** Rust（`dct` 同步线程模型 + `ureq`；`dct-srv` 用 `axum` 0.8 / `tokio`；`dct-link` 共享常量与凭证函数）、`hmac` 0.12 + `sha2` 0.10、Node 22（`container/classroom`）、原生 JS（`dct-page`）。

**Spec:** `docs/superpowers/specs/2026-09-13-public-live-listing-design.md`

## Global Constraints

- 协议：`PROTOCOL_VERSION` 18 → 19（Task 6 一次到位）。
- 公开标题：1–60 个**字符**（`chars().count()`），常量 `dct_link::live::MAX_PUBLIC_TITLE_CHARS = 60`。
- 密钥文件重载间隔 10 秒（`dct_link::live::PUBLISH_KEYS_RELOAD`）；管理台自愈间隔 30 秒；公开页刷新 5 秒。
- 保留房间号：`public`（`dct_link::live::RESERVED_LIVE_ID`）。
- `PUT/DELETE /live/{id}/public` 答复码：房间不存在或推帧钥匙/凭证不对 401；没开公开功能 403；发布密钥不对或吊销 401；黑名单 403；标题越界 413；限流 429；成功 204。`DELETE` 幂等。
- 凭证：`hex(HMAC-SHA256(key = SHA-256(push_secret), msg = "publish:" + id))`，只认 `PUT/DELETE /live/{id}/public`。
- 发布密钥、推帧钥匙、凭证**不出现在任何 `Debug` 输出里**；发布密钥**从不出现在守护进程发给界面的任何响应里**。
- 标题和路名在网页里只用 `textContent`，`public.html` / `live.html` 源码里不许出现 `innerHTML`。
- 提交信息用英文，不加任何 AI 署名行（仓库约定）。
- 界面文案两种语言（`t!` 宏），不写死一种。
- 每个 Task 结束前跑 `env -u TERM cargo test --workspace --locked --no-fail-fast` 与 `cargo clippy --workspace --all-targets --locked -- -D warnings`（改了 Node 的 Task 另跑 `node --test`）；新测试逐条做变异检查（把对应实现改坏，确认测试变红，再改回来）。
- 本计划里的参考代码**不是权威**：动手前对照真实源码核对名字、签名和调用点，发现不对以源码为准并在报告里写明。

## 与 spec 的一处偏离（执行前已向用户说明）

spec「设置页新增公开直播密钥」改为：**直播面板里 `K` 填/换密钥；按 `p` 公开时守护进程报 `LivePublishKeyMissing` 就直接转到填密钥输入，填完自动继续公开**。理由：设置页的密钥输入（`View::EnterSecret`）绑定 agent profile 的校验与返回路径，塞进一个非 agent 的密钥要按名字特判；在需要的那一刻就地填，对老师也更少一步。Task 11 同步改 spec 这一句。

---

## File Structure

| 文件 | 职责 | Task |
|---|---|---|
| `crates/dct-link/Cargo.toml`、`crates/dct-link/src/live.rs` | 公开相关常量、`push_hash`、`publish_grant` | 1 |
| `crates/dct-srv/Cargo.toml`、`crates/dct-srv/src/keys.rs`（新） | 发布密钥文件：读写、增删、黑名单、重载 | 2 |
| `crates/dct-srv/src/main.rs`、`crates/dct-srv/src/lib.rs`（`parse_cli`） | `key add/revoke/list`、`takedown`、`--publish-keys` | 2 |
| `crates/dct-srv/src/live.rs` | 房间公开状态、无令牌读、公开列表、`reconcile` | 3 |
| `crates/dct-srv/src/lib.rs` | HTTP 路由、`AppState.keys`、重载任务 | 4 |
| `crates/dct-page/public.html`（新）、`crates/dct-page/live.html`、`crates/dct-page/src/lib.rs` | 公开页；观看页无令牌模式；`innerHTML` 守卫 | 5 |
| `src/proto.rs`、`src/secrets.rs`、`src/i18n.rs` | 协议 19、`LivePublic`、新请求/响应/错误码、文案 | 6 |
| `src/live.rs`、`src/daemon.rs` | 守护进程公开意图、推帧线程 PUT/DELETE/自愈、处理新请求 | 7 |
| `src/ui/live.rs`、`src/ui/view.rs`、`src/ui/mod.rs`、`src/ui/app.rs` | 面板 `p`/`K`、输入行、底栏公开提示 | 8 |
| `container/classroom/server.mjs`、`container/classroom/server.test.mjs` | 管理台公开/取消公开、自愈、错误映射、审计 | 9 |
| `container/classroom/admin.html` | 管理台界面 | 10 |
| `docs/deploy-live-relay.md`、`container/classroom/README.md`、`README.md`、`README.zh-CN.md`、spec | 部署与使用文档 | 11 |

依赖顺序：1 → 2 → 3 → 4 → 5；1 → 6 → 7 → 8；4 + 7 → 9 → 10；最后 11。

---

### Task 1: `dct-link` —— 公开常量与凭证函数

**Files:**
- Modify: `crates/dct-link/Cargo.toml`
- Modify: `crates/dct-link/src/live.rs`

**Interfaces:**
- Produces:
  - `pub const MAX_PUBLIC_TITLE_CHARS: usize = 60;`
  - `pub const RESERVED_LIVE_ID: &str = "public";`
  - `pub const PATH_PUBLIC_LIST: &str = "/live/public";`
  - `pub const PUBLISH_KEYS_RELOAD: Duration = Duration::from_secs(10);`
  - `pub fn public_path(id: &str) -> String` → `"/live/{id}/public"`
  - `pub fn push_hash(push_secret: &str) -> [u8; 32]`（SHA-256）
  - `pub fn publish_grant(push_hash: &[u8; 32], id: &str) -> String`（小写 hex，64 字符）

- [ ] **Step 1: 加依赖**

`crates/dct-link/Cargo.toml` 的 `[dependencies]` 末尾追加：

```toml
# 公开直播的「凭证」：HMAC-SHA256(推帧钥匙的摘要, "publish:" + id)。中转和守护进程
# 必须算出同一个值，所以算法只在这里写一次。两个都是纯 Rust，没有 C。
sha2 = "0.10"
hmac = "0.12"
```

- [ ] **Step 2: 写失败的测试**

在 `crates/dct-link/src/live.rs` 的 `mod tests` 里追加：

```rust
    /// **凭证是跨 crate 的契约**：中转验、守护进程签，两边调的都是这一个函数。
    /// 固定向量由 Python `hmac.new(sha256(b"p"*64).digest(), b"publish:abc", sha256)`
    /// 独立算出——改了算法或者拼接格式，这里当场红。
    #[test]
    fn the_publish_grant_matches_a_fixed_vector() {
        let h = push_hash(&"p".repeat(64));
        assert_eq!(
            publish_grant(&h, "abc"),
            "74df63dc9e0620cba63084d7dbeec29068ecbf8a7ed31168d1c22895a5a10281"
        );
        let other = push_hash(&"q".repeat(64));
        assert_eq!(
            publish_grant(&other, "abc"),
            "64b549e882f6a84b91dd4ffe27594e24667fa314ff7e0a236061114951ad5c79",
            "换一把推帧钥匙，凭证必须跟着变"
        );
        assert_ne!(publish_grant(&h, "abc"), publish_grant(&h, "abd"), "凭证要绑房间号");
    }

    #[test]
    fn the_public_path_is_built_in_exactly_one_place() {
        assert_eq!(public_path("abc"), "/live/abc/public");
        assert_eq!(PATH_PUBLIC_LIST, "/live/public");
        assert_eq!(RESERVED_LIVE_ID, "public", "列表路径和保留房间号必须是同一个词");
        assert!(PATH_PUBLIC_LIST.ends_with(RESERVED_LIVE_ID));
    }
```

- [ ] **Step 3: 跑测试确认失败**

Run: `cargo test -p dct-link`
Expected: 编译失败，`cannot find function push_hash`。

- [ ] **Step 4: 实现**

在 `crates/dct-link/src/live.rs` 里 `MAX_LANE_NAME_CHARS` 之后追加：

```rust
/// 公开直播的标题最多几个**字符**。中转拒收更长的；守护进程和管理台在发之前截断。
pub const MAX_PUBLIC_TITLE_CHARS: usize = 60;

/// 公开列表占用的那个词。它同时是 `GET /live/public` 的最后一段，所以不能再当房间号：
/// 不拒的话，房间号恰好叫 `public` 的那场直播，观看页 `/live/public` 会被列表接口挡住。
pub const RESERVED_LIVE_ID: &str = "public";

/// 公开列表的路径。
pub const PATH_PUBLIC_LIST: &str = "/live/public";

/// 中转多久检查一次发布密钥文件。吊销、下线最多这么久生效。
pub const PUBLISH_KEYS_RELOAD: Duration = Duration::from_secs(10);

/// 公开 / 取消公开这场直播的路径（`PUT` / `DELETE`）。
pub fn public_path(id: &str) -> String {
    format!("{LIVE_PREFIX}/{id}/public")
}

/// 推帧钥匙的摘要。中转只存它（`Session::push_hash`），凭证也以它为 HMAC 密钥——
/// 这样中转验得了凭证，而推帧钥匙原文始终不离开守护进程。
pub fn push_hash(push_secret: &str) -> [u8; 32] {
    use sha2::{Digest, Sha256};
    Sha256::digest(push_secret.as_bytes()).into()
}

/// 「只能用来切换这场直播公开状态」的凭证。管理台拿它代学生工作区公开，
/// 推帧、停播都不认它。
pub fn publish_grant(push_hash: &[u8; 32], id: &str) -> String {
    use hmac::{Hmac, Mac};
    let mut mac = <Hmac<sha2::Sha256> as Mac>::new_from_slice(push_hash)
        .expect("HMAC 接受任意长度的密钥");
    mac.update(b"publish:");
    mac.update(id.as_bytes());
    mac.finalize()
        .into_bytes()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}
```

- [ ] **Step 5: 跑测试确认通过**

Run: `cargo test -p dct-link`
Expected: PASS（含两条新测试）。

- [ ] **Step 6: 变异检查**

把 `mac.update(b"publish:")` 改成 `mac.update(b"publish")`，确认 `the_publish_grant_matches_a_fixed_vector` 红；改回。

- [ ] **Step 7: Commit**

```bash
git add crates/dct-link/Cargo.toml crates/dct-link/src/live.rs Cargo.lock
git commit -m "feat(link): constants and the publish grant shared by relay and daemon"
```

---

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

### Task 3: `dct-srv` —— 房间公开状态

**Files:**
- Modify: `crates/dct-srv/src/live.rs`

**Interfaces:**
- Consumes: Task 1 的 `MAX_PUBLIC_TITLE_CHARS`、`RESERVED_LIVE_ID`、`publish_grant`；Task 2 的 `keys::PublishKeys`（`name_for`、`is_blocked`）。
- Produces:
  - `pub enum Control<'a> { Push(&'a str), Grant(&'a str) }`
  - `pub struct PublicEntry { pub id: String, pub title: String, pub lanes: Vec<String>, pub viewers: u32 }`（`Serialize`）
  - `Live::publish(&self, id: &str, control: Control, keys: Option<&PublishKeys>, key: &str, title: &str) -> Result<(), LinkError>`
  - `Live::unpublish(&self, id: &str, control: Control) -> Result<(), LinkError>`
  - `Live::frame(&self, id: &str, token: Option<&str>, lane: usize) -> Result<(Vec<u8>, u64), LinkError>`（**签名变**：`token` 变 `Option`）
  - `Live::lanes(&self, id: &str, token: Option<&str>) -> Result<(Vec<String>, Option<String>), LinkError>`（**签名变**：多回公开标题）
  - `Live::public_list(&self) -> Vec<PublicEntry>`
  - `Live::reconcile(&self, keys: Option<&PublishKeys>)`

- [ ] **Step 1: 写失败的测试**

在 `crates/dct-srv/src/live.rs` 的 `mod tests` 里追加（`started()` 已存在：房间 `abc`、viewer `"t"*64`、push `"p"*64`、两路）：

```rust
    fn keys_with(name: &str) -> (crate::keys::PublishKeys, String) {
        let mut k = crate::keys::PublishKeys::default();
        let key = k.add(name, 0).unwrap();
        (k, key)
    }

    /// 公开的鉴权矩阵，逐行对应 spec 那张表。
    #[test]
    fn publishing_follows_the_answer_table() {
        let live = started();
        let (keys, key) = keys_with("姜老师");
        let push = "p".repeat(64);
        let grant = dct_link::live::publish_grant(&dct_link::live::push_hash(&push), "abc");

        // 房间不存在 / 推帧钥匙不对 / 凭证不对 → 401
        assert_eq!(live.publish("nope", Control::Push(&push), Some(&keys), &key, "课"), Err(LinkError::Unauthorized));
        assert_eq!(live.publish("abc", Control::Push(&"x".repeat(64)), Some(&keys), &key, "课"), Err(LinkError::Unauthorized));
        assert_eq!(live.publish("abc", Control::Grant("00"), Some(&keys), &key, "课"), Err(LinkError::Unauthorized));
        // 没开公开功能 → 403
        assert_eq!(live.publish("abc", Control::Push(&push), None, &key, "课"), Err(LinkError::NotYours));
        // 发布密钥不对 → 401
        assert_eq!(live.publish("abc", Control::Push(&push), Some(&keys), &"0".repeat(64), "课"), Err(LinkError::Unauthorized));
        // 标题越界 → 413
        assert_eq!(live.publish("abc", Control::Push(&push), Some(&keys), &key, ""), Err(LinkError::TooBig));
        let long = "字".repeat(dct_link::live::MAX_PUBLIC_TITLE_CHARS + 1);
        assert_eq!(live.publish("abc", Control::Push(&push), Some(&keys), &key, &long), Err(LinkError::TooBig));
        // 成功：推帧钥匙和凭证都行
        assert_eq!(live.publish("abc", Control::Push(&push), Some(&keys), &key, "第3课"), Ok(()));
        assert_eq!(live.publish("abc", Control::Grant(&grant), Some(&keys), &key, "第4课"), Ok(()));
        assert_eq!(live.public_list()[0].title, "第4课", "重复公开覆盖标题");
        // 黑名单 → 403
        let mut blocked = keys.clone();
        blocked.block("abc");
        assert_eq!(live.publish("abc", Control::Push(&push), Some(&blocked), &key, "课"), Err(LinkError::NotYours));
    }

    /// 凭证只认公开这件事：推帧、停播都不认它。
    #[test]
    fn a_grant_cannot_push_or_stop() {
        let live = started();
        let grant = dct_link::live::publish_grant(&dct_link::live::push_hash(&"p".repeat(64)), "abc");
        assert_eq!(live.push("abc", &grant, 0, b"x".to_vec()), Err(LinkError::Unauthorized));
        assert_eq!(live.stop("abc", &grant), Err(LinkError::Unauthorized));
    }

    #[test]
    fn unpublishing_is_idempotent_and_needs_control() {
        let live = started();
        let (keys, key) = keys_with("a");
        let push = "p".repeat(64);
        assert_eq!(live.unpublish("abc", Control::Push(&push)), Ok(()), "没公开也回成功");
        live.publish("abc", Control::Push(&push), Some(&keys), &key, "课").unwrap();
        assert_eq!(live.unpublish("abc", Control::Push(&"x".repeat(64))), Err(LinkError::Unauthorized));
        assert_eq!(live.unpublish("abc", Control::Push(&push)), Ok(()));
        assert!(live.public_list().is_empty());
    }

    /// 无令牌只能读公开的房间；私密、不存在的房间对无令牌请求同一个 401；
    /// 带错令牌不因为房间公开而放行。
    #[test]
    fn tokenless_reads_only_reach_public_rooms() {
        let live = started();
        let (keys, key) = keys_with("a");
        let push = "p".repeat(64);
        live.push("abc", &push, 0, b"hi".to_vec()).unwrap();

        assert_eq!(live.frame("abc", None, 0).err(), Some(LinkError::Unauthorized), "私密房间");
        assert_eq!(live.frame("zzz", None, 0).err(), Some(LinkError::Unauthorized), "不存在的房间");
        assert_eq!(live.lanes("abc", None).err(), Some(LinkError::Unauthorized));

        live.publish("abc", Control::Push(&push), Some(&keys), &key, "课").unwrap();
        assert_eq!(live.frame("abc", None, 0).unwrap().0, b"hi");
        assert_eq!(live.lanes("abc", None).unwrap(), (vec!["前端".to_string(), "后端".to_string()], Some("课".to_string())));
        assert_eq!(live.frame("abc", Some(&"w".repeat(64)), 0).err(), Some(LinkError::Unauthorized), "带错令牌照拒");
        assert_eq!(live.lanes("abc", Some(&"t".repeat(64))).unwrap().1, Some("课".to_string()));
    }

    #[test]
    fn the_public_list_has_title_lanes_viewers_and_no_publisher() {
        let live = started();
        let (keys, key) = keys_with("姜老师");
        live.publish("abc", Control::Push(&"p".repeat(64)), Some(&keys), &key, "第3课").unwrap();
        let list = live.public_list();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].id, "abc");
        assert_eq!(list[0].lanes, vec!["前端", "后端"]);
        let json = serde_json::to_string(&list).unwrap();
        assert!(!json.contains("姜老师"), "列表里不许出现发布者：{json}");
    }

    /// 吊销、下线、关总开关：重载之后公开状态立刻收回。
    #[test]
    fn reconcile_withdraws_revoked_blocked_and_disabled_publications() {
        let push = "p".repeat(64);
        for case in ["revoke", "block", "disable"] {
            let live = started();
            let (mut keys, key) = keys_with("a");
            live.publish("abc", Control::Push(&push), Some(&keys), &key, "课").unwrap();
            match case {
                "revoke" => {
                    keys.revoke("a");
                    live.reconcile(Some(&keys));
                }
                "block" => {
                    keys.block("abc");
                    live.reconcile(Some(&keys));
                }
                _ => live.reconcile(None),
            }
            assert!(live.public_list().is_empty(), "{case} 之后还挂在公开列表上");
            assert_eq!(live.frame("abc", None, 0).err(), Some(LinkError::Unauthorized), "{case}");
        }
    }

    #[test]
    fn reopening_a_room_does_not_inherit_its_public_state() {
        let live = started();
        let (keys, key) = keys_with("a");
        let push = "p".repeat(64);
        live.publish("abc", Control::Push(&push), Some(&keys), &key, "课").unwrap();
        live.start("abc".into(), "t".repeat(64), push, vec!["前端".into()]).unwrap();
        assert!(live.public_list().is_empty());
    }

    #[test]
    fn the_reserved_word_cannot_be_a_room_id() {
        assert_eq!(
            Live::new().start(dct_link::live::RESERVED_LIVE_ID.into(), "t".repeat(64), "p".repeat(64), vec!["x".into()]),
            Err(LinkError::Unauthorized)
        );
    }
```

同时把本文件既有测试里对 `frame(id, token, lane)` / `lanes(id, token)` 的调用改成 `frame(id, Some(token), lane)` / `lanes(id, Some(token))`，`lanes` 的返回值从 `Vec` 改成取 `.0`。

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test -p dct-srv --lib live::`
Expected: 编译失败（`Control`、`publish` 等未定义）。

- [ ] **Step 3: 实现**

在 `crates/dct-srv/src/live.rs`：

1. `use` 里加 `dct_link::live::{MAX_PUBLIC_TITLE_CHARS, RESERVED_LIVE_ID}` 和 `crate::keys::PublishKeys`。
2. `Session` 加字段：

```rust
    /// 这场直播是否公开，以及是哪把发布密钥公开的（名字只给服务器上的人看，
    /// 不进公开列表）。**重开同一个房间号不继承它**。
    public: Option<Public>,
```

并在文件里定义：

```rust
struct Public {
    title: String,
    key_name: String,
}

/// 证明「你控制这场直播」的两种方式。
pub enum Control<'a> {
    /// 老师本机守护进程的推帧钥匙。
    Push(&'a str),
    /// 守护进程签发给管理台的凭证，见 `dct_link::live::publish_grant`。
    Grant(&'a str),
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct PublicEntry {
    pub id: String,
    pub title: String,
    pub lanes: Vec<String>,
    pub viewers: u32,
}
```

3. `start` 里，在「id 只许字母数字和 `-`/`_`」那个 `if` 的条件里加 `|| id == RESERVED_LIVE_ID`；`rooms.insert(...)` 的 `Session { ... }` 加 `public: None,`。
4. 新增：

```rust
/// 这一方到底控不控制这个房间。常数时间，理由同 `same`。
fn controls(room: &Session, id: &str, control: &Control) -> bool {
    match control {
        Control::Push(secret) => same(&room.push_hash, &hash(secret)),
        Control::Grant(grant) => {
            let want = dct_link::live::publish_grant(&room.push_hash, id);
            same(&hash(&want), &hash(grant))
        }
    }
}
```

并在 `impl Live` 里追加：

```rust
    /// 公开这场直播。判断顺序就是 spec 那张答复表的顺序：先证明控制房间
    /// （401 不给探测差别），再看总开关（403），再验发布密钥（401），再看
    /// 黑名单（403），最后标题（413）。
    pub fn publish(
        &self,
        id: &str,
        control: Control,
        keys: Option<&PublishKeys>,
        key: &str,
        title: &str,
    ) -> Result<(), LinkError> {
        let mut rooms = self.rooms.lock().expect("live 锁");
        let room = rooms.get_mut(id).ok_or(LinkError::Unauthorized)?;
        if !controls(room, id, &control) {
            return Err(LinkError::Unauthorized);
        }
        let keys = keys.ok_or(LinkError::NotYours)?;
        let key_name = keys.name_for(key).ok_or(LinkError::Unauthorized)?;
        if keys.is_blocked(id) {
            return Err(LinkError::NotYours);
        }
        let n = title.chars().count();
        if n == 0 || n > MAX_PUBLIC_TITLE_CHARS {
            return Err(LinkError::TooBig);
        }
        room.public = Some(Public { title: title.to_string(), key_name });
        Ok(())
    }

    /// 取消公开。**幂等**：没公开也回成功。
    pub fn unpublish(&self, id: &str, control: Control) -> Result<(), LinkError> {
        let mut rooms = self.rooms.lock().expect("live 锁");
        let room = rooms.get_mut(id).ok_or(LinkError::Unauthorized)?;
        if !controls(room, id, &control) {
            return Err(LinkError::Unauthorized);
        }
        room.public = None;
        Ok(())
    }

    /// 公开列表，按在看人数降序。**不含发布者。**
    pub fn public_list(&self) -> Vec<PublicEntry> {
        let rooms = self.rooms.lock().expect("live 锁");
        let mut list: Vec<PublicEntry> = rooms
            .iter()
            .filter_map(|(id, r)| {
                r.public.as_ref().map(|p| PublicEntry {
                    id: id.clone(),
                    title: p.title.clone(),
                    lanes: r.lanes.iter().map(|l| l.name.clone()).collect(),
                    viewers: r.lanes.iter().map(|l| l.tx.receiver_count() as u32).sum(),
                })
            })
            .collect();
        list.sort_by(|a, b| b.viewers.cmp(&a.viewers).then_with(|| a.id.cmp(&b.id)));
        list
    }

    /// 密钥文件重载之后调：吊销了的密钥公开的、被下线的、以及总开关关掉时的
    /// 全部公开，立刻收回。
    pub fn reconcile(&self, keys: Option<&PublishKeys>) {
        let mut rooms = self.rooms.lock().expect("live 锁");
        for (id, room) in rooms.iter_mut() {
            let keep = match (keys, room.public.as_ref()) {
                (_, None) => continue,
                (None, Some(_)) => false,
                (Some(k), Some(p)) => {
                    !k.is_blocked(id) && k.keys.iter().any(|e| e.name == p.key_name)
                }
            };
            if !keep {
                room.public = None;
            }
        }
    }
```

5. 把 `authed` 改成接 `Option<&str>`：

```rust
/// 认证在取数之前，而且**认不出来和不存在回同一句话**。
///
/// 没带令牌（`None`）：只有公开的房间放行。带了令牌：照旧只认 viewer 那把，
/// 带错的**不会**因为房间公开而被放行。
fn authed<'a>(
    rooms: &'a HashMap<String, Session>,
    id: &str,
    token: Option<&str>,
) -> Result<&'a Session, LinkError> {
    let room = rooms.get(id).ok_or(LinkError::Unauthorized)?;
    match token {
        None if room.public.is_some() => Ok(room),
        None => Err(LinkError::Unauthorized),
        Some(t) if same(&room.viewer_hash, &hash(t)) => Ok(room),
        Some(_) => Err(LinkError::Unauthorized),
    }
}
```

6. `frame` / `lanes` 改签名：

```rust
    pub fn frame(&self, id: &str, token: Option<&str>, lane: usize) -> Result<(Vec<u8>, u64), LinkError> {
        let rooms = self.rooms.lock().expect("live 锁");
        let room = authed(&rooms, id, token)?;
        let slot = room.lanes.get(lane).ok_or(LinkError::Unauthorized)?;
        Ok((slot.frame.clone(), slot.etag))
    }

    /// 路名，以及公开标题（私密房间是 `None`）。推帧线程靠这第二个值得知
    /// 「我这场现在公不公开」——包括管理台代为公开的情况。
    pub fn lanes(&self, id: &str, token: Option<&str>) -> Result<(Vec<String>, Option<String>), LinkError> {
        let rooms = self.rooms.lock().expect("live 锁");
        let room = authed(&rooms, id, token)?;
        Ok((
            room.lanes.iter().map(|l| l.name.clone()).collect(),
            room.public.as_ref().map(|p| p.title.clone()),
        ))
    }
```

`lib.rs` 里 `live_frame_route` / `live_lanes_route` 的调用临时改成 `Some(token)` 与 `.0`，让 crate 编得过（Task 4 再改成真正的无令牌逻辑）。

- [ ] **Step 4: 跑测试确认通过**

Run: `cargo test -p dct-srv`
Expected: PASS。

- [ ] **Step 5: 变异检查**

逐条：`authed` 的 `None if room.public.is_some()` 改成 `None`（`tokenless_reads_only_reach_public_rooms` 红）；`publish` 删掉黑名单判断（`publishing_follows_the_answer_table` 红）；`reconcile` 的 `keep` 恒为 `true`（`reconcile_withdraws...` 红）；`start` 里 `public: None` 改成保留旧房间的 `public`（`reopening...` 红——若 `insert` 本就整体替换，改成先取旧房间的 `public` 再塞回去）；`controls` 的 `Grant` 分支恒 `true`（`publishing_follows...` 红）。每次改回。

- [ ] **Step 6: Commit**

```bash
git add crates/dct-srv/src/live.rs crates/dct-srv/src/lib.rs
git commit -m "feat(srv): rooms can be published, read without a token while public, and withdrawn"
```

---

### Task 4: `dct-srv` —— HTTP 路由与密钥重载

**Files:**
- Modify: `crates/dct-srv/src/lib.rs`
- Modify: `crates/dct-srv/src/main.rs`
- Modify: `crates/dct-srv/tests/serves.rs`（`serve` 多一个参数）

**Interfaces:**
- Consumes: Task 3 全部；Task 2 `KeyFile`；Task 5 的 `dct_page::public_page()`（**Task 5 之前先用** `axum::response::Html("<!doctype html><title>公开直播</title>")` 占位，Task 5 替换——占位只在两个 Task 之间存在）。
- Produces:
  - `pub struct AppState { pub relay, pub live, pub keys: Arc<std::sync::RwLock<Option<PublishKeys>>> }`
  - `pub async fn serve(listener, relay, live, routes, keys: Option<KeyFile>)`
  - 路由：`PUT/DELETE /live/{id}/public`、`GET /live/public`、`GET /`（`LiveOnly` 也挂）
  - `GET /live/{id}/lanes` 答复多 `"public": {"title": "..."} | null`

- [ ] **Step 1: 写失败的测试**

`lib.rs` 的 `mod tests` 里，把 `app()` / `app_with_live()` 构造 `AppState` 的地方加 `keys: Arc::new(std::sync::RwLock::new(None))`，并新增一个带密钥的底子与测试：

```rust
    fn app_with_keys() -> (Router, Arc<Live>, String) {
        let live = Arc::new(Live::new());
        let mut k = crate::keys::PublishKeys::default();
        let key = k.add("姜老师", 0).unwrap();
        let app = router(
            AppState {
                relay: Arc::new(Relay::new(cfg(200))),
                live: live.clone(),
                keys: Arc::new(std::sync::RwLock::new(Some(k))),
            },
            Routes::LiveOnly,
        );
        live.start("abc".into(), "t".repeat(64), push_secret(), vec!["前端".into()]).unwrap();
        (app, live, key)
    }

    async fn call(app: &Router, method: &str, uri: &str, headers: &[(&str, &str)], body: &str) -> (u16, String) {
        let mut req = Request::builder().method(method).uri(uri);
        for (k, v) in headers {
            req = req.header(*k, *v);
        }
        let res = app
            .clone()
            .oneshot(req.body(Body::from(body.to_string())).unwrap())
            .await
            .unwrap();
        let status = res.status().as_u16();
        let bytes = axum::body::to_bytes(res.into_body(), usize::MAX).await.unwrap();
        (status, String::from_utf8_lossy(&bytes).into_owned())
    }

    /// 公开 → 列表里出现 → 无令牌能读路名（带公开标题）→ 取消公开 → 无令牌被拒。
    #[tokio::test]
    async fn publishing_over_http_end_to_end() {
        let (app, _live, key) = app_with_keys();
        let push = push_secret();
        let body = format!(r#"{{"title":"第3课","key":"{key}"}}"#);
        let ct = ("content-type", "application/json");

        let (s, _) = call(&app, "PUT", "/live/abc/public", &[ct, ("x-live-push", &push)], &body).await;
        assert_eq!(s, 204);
        let (s, list) = call(&app, "GET", "/live/public", &[], "").await;
        assert_eq!(s, 200);
        assert!(list.contains("第3课") && list.contains("前端") && !list.contains("姜老师"), "{list}");
        let (s, lanes) = call(&app, "GET", "/live/abc/lanes", &[], "").await;
        assert_eq!(s, 200, "公开房间无令牌能读路名");
        assert!(lanes.contains(r#""public":{"title":"第3课"}"#), "{lanes}");

        let (s, _) = call(&app, "DELETE", "/live/abc/public", &[("x-live-push", &push)], "").await;
        assert_eq!(s, 204);
        let (s, _) = call(&app, "GET", "/live/abc/lanes", &[], "").await;
        assert_eq!(s, 401);
    }

    #[tokio::test]
    async fn a_grant_header_can_publish() {
        let (app, _live, key) = app_with_keys();
        let grant = dct_link::live::publish_grant(&dct_link::live::push_hash(&push_secret()), "abc");
        let body = format!(r#"{{"title":"课","key":"{key}"}}"#);
        let (s, _) = call(&app, "PUT", "/live/abc/public", &[("content-type", "application/json"), ("x-live-grant", &grant)], &body).await;
        assert_eq!(s, 204);
    }

    /// 服务器没开公开功能：403；公开页照样打得开（空列表）。
    #[tokio::test]
    async fn without_a_key_file_publishing_is_forbidden_and_the_list_is_empty() {
        let (app, live) = app_with_live();
        live.start("abc".into(), "t".repeat(64), push_secret(), vec!["前端".into()]).unwrap();
        let body = format!(r#"{{"title":"课","key":"{}"}}"#, "0".repeat(64));
        let (s, _) = call(&app, "PUT", "/live/abc/public", &[("content-type", "application/json"), ("x-live-push", &push_secret())], &body).await;
        assert_eq!(s, 403);
        assert_eq!(call(&app, "GET", "/live/public", &[], "").await, (200, "[]".to_string()));
        assert_eq!(call(&app, "GET", "/", &[], "").await.0, 200);
    }

    /// 既没带推帧钥匙也没带凭证：401，跟推帧、停播一样。
    #[tokio::test]
    async fn publishing_without_proof_of_control_is_unauthorized() {
        let (app, _live, key) = app_with_keys();
        let body = format!(r#"{{"title":"课","key":"{key}"}}"#);
        let (s, _) = call(&app, "PUT", "/live/abc/public", &[("content-type", "application/json")], &body).await;
        assert_eq!(s, 401);
    }

    /// 空的 `x-live-token` 当成没带：观看页没有令牌时不该因为发了个空头就被拒。
    #[tokio::test]
    async fn an_empty_token_header_counts_as_no_token() {
        let (app, _live, key) = app_with_keys();
        let body = format!(r#"{{"title":"课","key":"{key}"}}"#);
        let (s, _) = call(&app, "PUT", "/live/abc/public", &[("content-type", "application/json"), ("x-live-push", &push_secret())], &body).await;
        assert_eq!(s, 204);
        assert_eq!(call(&app, "GET", "/live/abc/lanes", &[("x-live-token", "")], "").await.0, 200);
    }
```

`tests/serves.rs` 两处 `dct_srv::serve(...)` 末尾加 `None`。

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test -p dct-srv`
Expected: 编译失败（`AppState` 没有 `keys`、路由不存在）。

- [ ] **Step 3: 实现**

在 `lib.rs`：

1. `AppState` 加字段：

```rust
    /// 当前生效的发布密钥。`None` = 没开公开功能（没带 `--publish-keys`）。
    /// 重载任务写，公开路由读。
    pub keys: Arc<std::sync::RwLock<Option<crate::keys::PublishKeys>>>,
```

`serve` 构造 `AppState` 时传入。
2. 读令牌的小工具（放在 `header` 旁边）：

```rust
/// 令牌头：没带、或者带了个空串，都当成没带。
fn token_header(headers: &HeaderMap) -> Option<&str> {
    header(headers, "x-live-token").filter(|t| !t.is_empty())
}

/// 证明控制房间的那个头：推帧钥匙优先，其次凭证。
fn control_header(headers: &HeaderMap) -> Option<crate::live::Control<'_>> {
    header(headers, "x-live-push")
        .map(crate::live::Control::Push)
        .or_else(|| header(headers, "x-live-grant").map(crate::live::Control::Grant))
}
```

3. `live_frame_route`：`let token = token_header(&headers);`（不再 `ok_or(Unauthorized)`），两处 `live.frame(&id, token, lane)`。
4. `live_lanes_route` 与答复：

```rust
#[derive(serde::Serialize, serde::Deserialize)]
struct PublicTitle {
    title: String,
}

#[derive(serde::Serialize, serde::Deserialize)]
struct LiveLanesResponse {
    lanes: Vec<String>,
    viewers: u32,
    public: Option<PublicTitle>,
}

async fn live_lanes_route(
    State(live): State<Arc<Live>>,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> Result<Json<LiveLanesResponse>, Rejected> {
    let (lanes, public) = live.lanes(&id, token_header(&headers))?;
    let viewers = live.viewers(&id);
    Ok(Json(LiveLanesResponse { lanes, viewers, public: public.map(|title| PublicTitle { title }) }))
}
```

5. 公开 / 取消公开 / 列表 / 公开页：

```rust
#[derive(serde::Deserialize)]
struct PublishRequest {
    title: String,
    key: String,
}

/// 公开这场直播。跟建房共用按来源的限流（同一本账）。
async fn live_publish_route(
    State(state): State<AppState>,
    Path(id): Path<String>,
    headers: HeaderMap,
    Json(req): Json<PublishRequest>,
) -> Result<StatusCode, Rejected> {
    state.live.note_start(&client_key(&headers), Instant::now())?;
    let control = control_header(&headers).ok_or(LinkError::Unauthorized)?;
    let keys = state.keys.read().expect("keys 锁");
    state.live.publish(&id, control, keys.as_ref(), &req.key, &req.title)?;
    Ok(StatusCode::NO_CONTENT)
}

async fn live_unpublish_route(
    State(live): State<Arc<Live>>,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> Result<StatusCode, Rejected> {
    let control = control_header(&headers).ok_or(LinkError::Unauthorized)?;
    live.unpublish(&id, control)?;
    Ok(StatusCode::NO_CONTENT)
}

async fn live_public_list_route(State(live): State<Arc<Live>>) -> Json<Vec<crate::live::PublicEntry>> {
    Json(live.public_list())
}

async fn public_page_route() -> axum::response::Html<&'static str> {
    axum::response::Html(dct_page::public_page())
}
```

（`Live::note_start` 当前是 `pub fn`，若可见性不同按源码调整；`FromRef<AppState> for Arc<Live>` 已有，`State(state): State<AppState>` 需要 `AppState: Clone`——它已经 `#[derive(Clone)]`。）
6. `router`：在 `app` 的直播那一组（`Routes` 两档都挂）里加：

```rust
        .route("/", get(public_page_route))
        .route(dct_link::live::PATH_PUBLIC_LIST, get(live_public_list_route))
        .route(
            "/live/{id}/public",
            axum::routing::put(live_publish_route)
                .delete(live_unpublish_route)
                .layer(DefaultBodyLimit::max(16 * 1024)),
        )
```

并把 `Routes::WithLink` 分支里原来的 `.route("/", get(page_route))` 删掉——`/` 从此是公开页；手机端网页在 `--with-link` 下改挂到 `/phone`（在那一行写注释说明原因，并更新 `the_relay_serves_the_very_same_page_the_daemon_does` 测试请求的路径为 `/phone`）。**先确认**：`page.html` 里有没有写死的 `/` 相对路径依赖（`fetch("/api/...")` 这类不受影响；`location.pathname` 若被用来拼路径则需要检查），有就在报告里写明并调整。
7. `serve` 加参数与重载任务：

```rust
pub async fn serve(
    listener: tokio::net::TcpListener,
    relay: Arc<Relay>,
    live: Arc<Live>,
    routes: Routes,
    keys: Option<crate::keys::KeyFile>,
) -> Result<(), std::io::Error> {
    let shared = Arc::new(std::sync::RwLock::new(keys.as_ref().map(|k| k.keys().clone())));
    if let Some(mut file) = keys {
        let shared = shared.clone();
        let live = live.clone();
        tokio::spawn(async move {
            let mut tick = tokio::time::interval(dct_link::live::PUBLISH_KEYS_RELOAD);
            loop {
                tick.tick().await;
                match file.reload_if_changed() {
                    Ok(true) => {
                        let fresh = file.keys().clone();
                        live.reconcile(Some(&fresh));
                        *shared.write().expect("keys 锁") = Some(fresh);
                    }
                    Ok(false) => {}
                    // 坏文件：保留上一份，只记日志（见 `KeyFile::reload_if_changed`）。
                    Err(why) => eprintln!("dct-srv：发布密钥文件没重新加载，继续用上一份：{why}"),
                }
            }
        });
    }
    // ……原有 TTL 清扫任务不变……
    axum::serve(listener, router(AppState { relay, live, keys: shared }, routes)).await
}
```

8. `main.rs`：把 Task 2 暂存的 `let _ = publish_keys;` 换成把 `publish_keys` 作为 `serve` 第五个参数传入；启动提示在开了公开功能时加一句「已开启公开直播（密钥文件：…）」。

- [ ] **Step 4: 跑测试确认通过**

Run: `cargo test -p dct-srv`
Expected: PASS。

- [ ] **Step 5: 变异检查**

`token_header` 去掉 `.filter(...)`（`an_empty_token_header_counts_as_no_token` 红）；`live_publish_route` 里把 `keys.as_ref()` 换成 `None`（`publishing_over_http_end_to_end` 红）；`LiveLanesResponse` 去掉 `public`（同一条红）。改回。

- [ ] **Step 6: Commit**

```bash
git add crates/dct-srv
git commit -m "feat(srv): publish, unpublish and list routes, with key file reloading"
```

---

### Task 5: `dct-page` —— 公开页与观看页无令牌模式

**Files:**
- Create: `crates/dct-page/public.html`
- Modify: `crates/dct-page/live.html`
- Modify: `crates/dct-page/src/lib.rs`
- Modify: `crates/dct-srv/src/lib.rs`（去掉 Task 4 的占位，调用 `dct_page::public_page()`）

**Interfaces:**
- Consumes: Task 4 的 `GET /live/public` 答复形状 `[{id,title,lanes,viewers}]`。
- Produces: `pub fn public_page() -> &'static str`

- [ ] **Step 1: 写失败的测试**

`crates/dct-page/src/lib.rs` 的 `mod tests` 追加：

```rust
    /// 公开页真的打包进来了，而且是给公众看的那一页。
    #[test]
    fn the_public_page_is_here() {
        let p = super::public_page();
        assert!(p.contains("<!doctype html>"));
        assert!(p.contains("/live/public"), "公开页要去拉公开列表");
    }

    /// **标题和路名是别人填的自由文本，只能当纯文本塞进页面。** 两页都不许
    /// 出现 `innerHTML`：一个 `<script>` 写进标题，就是公开页上的存储型 XSS。
    #[test]
    fn neither_page_ever_uses_inner_html() {
        for (name, src) in [("public.html", super::public_page()), ("live.html", super::live_page())] {
            assert!(!src.contains("innerHTML"), "{name} 里出现了 innerHTML");
            assert!(!src.contains("outerHTML"), "{name} 里出现了 outerHTML");
            assert!(!src.contains("insertAdjacentHTML"), "{name} 里出现了 insertAdjacentHTML");
        }
    }

    /// 观看页没有令牌时不能发一个空的 `x-live-token`，而要干脆不带。
    #[test]
    fn the_live_page_omits_the_token_header_when_it_has_none() {
        let src = super::live_page();
        assert!(src.contains("function tokenHeaders()"), "拉帧和拉路名要走同一个取头的函数");
        assert!(!src.contains(r#"headers: { "x-live-token": TOKEN }"#), "还有地方直接写死了令牌头");
    }
```

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test -p dct-page`
Expected: 编译失败（`public_page` 未定义）。

- [ ] **Step 3: 写 `public.html`**

```html
<!doctype html>
<html lang="zh">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<meta name="referrer" content="no-referrer">
<title>公开直播</title>
<!--
  公开直播列表。只由中转发（`GET /`）。

  规矩：
  - 标题和路名是老师或管理台填的自由文本：**只用 textContent**，不许 innerHTML
    （`dct-page` 的 `neither_page_ever_uses_inner_html` 钉着）。
  - 不设 cookie、不带 referrer、不引外部资源。
  - 每 5 秒拉一次 `/live/public`；标签页在后台时不拉，回到前台立刻拉一次。
-->
<style>
  :root { color-scheme: light dark; --fg: #1d1d1f; --bg: #fafafa; --card: #fff; --dim: #6e6e73; --accent: #0a7d5a; }
  @media (prefers-color-scheme: dark) { :root { --fg: #e8e8ea; --bg: #111; --card: #1c1c1e; --dim: #98989d; --accent: #3ccf9a; } }
  body { margin: 0; padding: 24px 16px; font: 16px/1.5 -apple-system, "PingFang SC", "Microsoft YaHei", system-ui, sans-serif; color: var(--fg); background: var(--bg); }
  main { max-width: 760px; margin: 0 auto; }
  h1 { font-size: 22px; margin: 0 0 16px; }
  .empty { color: var(--dim); }
  a.card { display: block; padding: 14px 16px; margin: 0 0 12px; border-radius: 12px; background: var(--card); color: inherit; text-decoration: none; box-shadow: 0 1px 2px rgba(0,0,0,.08); }
  .title { font-weight: 600; font-size: 18px; }
  .lanes { margin: 6px 0 0; display: flex; flex-wrap: wrap; gap: 6px; }
  .lane { font-size: 13px; padding: 2px 8px; border-radius: 999px; border: 1px solid var(--dim); color: var(--dim); }
  .viewers { margin-top: 6px; font-size: 13px; color: var(--accent); }
</style>
</head>
<body>
<main>
  <h1 id="heading"></h1>
  <p id="empty" class="empty" hidden></p>
  <div id="list"></div>
</main>
<script>
(function () {
  "use strict";
  var STRINGS = {
    zh: { heading: "正在公开直播", empty: "现在没有公开直播", viewers: function (n) { return n + " 人在看"; } },
    en: { heading: "Live now", empty: "Nothing is live publicly right now", viewers: function (n) { return n + " watching"; } }
  };
  var LANG = (navigator.language || "").toLowerCase().indexOf("zh") === 0 ? "zh" : "en";
  var S = STRINGS[LANG];
  document.documentElement.lang = LANG;
  document.getElementById("heading").textContent = S.heading;
  var emptyEl = document.getElementById("empty");
  var listEl = document.getElementById("list");
  var REFRESH_MS = 5000;

  function render(items) {
    listEl.textContent = "";
    emptyEl.hidden = items.length > 0;
    emptyEl.textContent = S.empty;
    items.forEach(function (it) {
      var a = document.createElement("a");
      a.className = "card";
      a.href = "/live/" + encodeURIComponent(it.id);
      var t = document.createElement("div");
      t.className = "title";
      t.textContent = it.title;
      a.appendChild(t);
      var lanes = document.createElement("div");
      lanes.className = "lanes";
      (it.lanes || []).forEach(function (name) {
        var l = document.createElement("span");
        l.className = "lane";
        l.textContent = name;
        lanes.appendChild(l);
      });
      a.appendChild(lanes);
      var v = document.createElement("div");
      v.className = "viewers";
      v.textContent = S.viewers(it.viewers || 0);
      a.appendChild(v);
      listEl.appendChild(a);
    });
  }

  function pull() {
    if (document.hidden) { return; }
    fetch("/live/public", { credentials: "omit", cache: "no-store" })
      .then(function (r) { return r.ok ? r.json() : null; })
      .then(function (items) { if (items) { render(items); } })
      .catch(function () {});
  }

  document.addEventListener("visibilitychange", function () { if (!document.hidden) { pull(); } });
  setInterval(pull, REFRESH_MS);
  pull();
})();
</script>
</body>
</html>
```

`lib.rs` 加：

```rust
const PUBLIC_SRC: &str = include_str!("../public.html");

/// 公开直播列表页，只由中转发（`GET /`）。不用共享渲染代码，所以不做占位符替换。
pub fn public_page() -> &'static str {
    PUBLIC_SRC
}
```

- [ ] **Step 4: 改 `live.html`**

1. 在 `TOKEN` 的定义之后加：

```js
  // 没有令牌就**不带**这个头：公开的直播不需要令牌，而一个空的 x-live-token
  // 会被当成「带了、但是不对」。拉帧和拉路名都从这里取头，别再各写一份。
  function tokenHeaders() {
    return TOKEN ? { "x-live-token": TOKEN } : {};
  }
```

2. 把文件里所有 `headers: { "x-live-token": TOKEN }`（`fetchLanes` 和拉帧那一处；用 `grep -n "x-live-token" crates/dct-page/live.html` 找全）改成 `headers: tokenHeaders()`。
3. `STRINGS.zh` / `STRINGS.en` 各加：

```js
      endedPublic: "这场直播已结束或不再公开",
      backToList: "回到公开列表",
```

```js
      endedPublic: "This live session has ended or is no longer public",
      backToList: "Back to the public list",
```

4. `render()` 改成：

```js
  function render() {
    dotEl.textContent = ICON[state];
    // 没有令牌的观众是从公开列表点进来的：结束和「切回私密」对他是同一件事
    // （中转回的也是同一个 401），说一句能涵盖两者的话，并给一条回列表的路。
    if (state === "ended" && !TOKEN) {
      statusEl.textContent = S.endedPublic + " · ";
      var back = document.createElement("a");
      back.href = "/";
      back.textContent = S.backToList;
      statusEl.appendChild(back);
    } else {
      statusEl.textContent = S[state];
    }
    headEl.className = state === "ended" ? "bad" : state === "paused" ? "warn" : "";
  }
```

5. 在 `crates/dct-srv/src/lib.rs` 把 `public_page_route` 里 Task 4 的占位换成 `dct_page::public_page()`（若 Task 4 已直接写成调用，这一步确认即可）。

- [ ] **Step 5: 跑测试确认通过**

Run: `cargo test -p dct-page && cargo test -p dct-srv`
Expected: PASS。

- [ ] **Step 6: 变异检查**

在 `public.html` 的 `t.textContent = it.title;` 改成 `t.innerHTML = it.title;`，确认 `neither_page_ever_uses_inner_html` 红；在 `live.html` 恢复一处写死的令牌头，确认 `the_live_page_omits_the_token_header_when_it_has_none` 红。改回。

- [ ] **Step 7: 手工验收脚手架**

在 `crates/dct-srv/tests/serves.rs` 追加一条 `#[ignore]` 测试，起中转（带临时密钥文件）、开一场房间、公开、推一帧，打印地址后挂 10 分钟：

```rust
/// 手工看公开页：`cargo test -p dct-srv --test serves serve_a_public_room_for_a_manual_look -- --ignored --nocapture`
/// 然后浏览器打开打印出来的地址。
#[tokio::test]
#[ignore]
async fn serve_a_public_room_for_a_manual_look() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("keys.json");
    let mut k = dct_srv::keys::PublishKeys::default();
    let key = k.add("手工验收", 0).unwrap();
    k.save(&file).unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let live = Arc::new(Live::new());
    let push = "p".repeat(64);
    live.start("demo01".into(), "t".repeat(64), push.clone(), vec!["前端".into(), "后端".into()]).unwrap();
    let keys = dct_srv::keys::KeyFile::open(file).unwrap();
    live.publish("demo01", dct_srv::Control::Push(&push), Some(keys.keys()), &key, "手工验收 · <script>alert(1)</script>").unwrap();
    // 房间 60 秒没推帧会被 TTL 回收：每 20 秒推一次空帧保活。
    let keepalive = live.clone();
    let keepalive_secret = push.clone();
    tokio::spawn(async move {
        loop {
            let _ = keepalive.push("demo01", &keepalive_secret, 0, Vec::new());
            tokio::time::sleep(Duration::from_secs(20)).await;
        }
    });
    tokio::spawn(dct_srv::serve(listener, Arc::new(Relay::new(Config::default())), live, Routes::LiveOnly, Some(keys)));
    println!("公开页：http://{addr}/   （标题里那段 <script> 必须原样显示成文字）");
    tokio::time::sleep(Duration::from_secs(600)).await;
}
```

`Control` 要从 `dct_srv` 顶层可见：在 `lib.rs` 加 `pub use live::{Control, PublicEntry};`。

- [ ] **Step 8: Commit**

```bash
git add crates/dct-page crates/dct-srv
git commit -m "feat(page): public listing page and tokenless viewing of public rooms"
```

---

### Task 6: `dct` 协议 19 —— 类型、错误码、文案

**Files:**
- Modify: `src/proto.rs`
- Modify: `src/secrets.rs`
- Modify: `src/i18n.rs`
- Modify: 所有构造 `LiveInfo { ... }` 的地方（`grep -rn "LiveInfo {" src` 列全；当前在 `src/live.rs`、`src/ui/live.rs`、`src/ui/mod.rs`、`src/ui/app.rs`、`src/proto.rs` 测试里）

**Interfaces:**
- Consumes: 无（纯类型）。
- Produces:
  - `pub enum LivePublic { Private, Pending { title: String }, Listed { title: String }, Failed { title: String, reason: LiveFailure } }`（`Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default`，默认 `Private`）
  - `LiveInfo.public: LivePublic`（`#[serde(default)]`）
  - `Request::LivePublish { title: String }`、`Request::LiveUnpublish`、`Request::LivePublishGrant`
  - `pub struct LiveGrantToken(pub String)`（`Serialize/Deserialize` 透明、`Debug` 打码）与 `Response::LiveGrant(LiveGrantToken)`
  - `ErrorCode::LivePublishKeyMissing`
  - `pub const LIVE_PUBLISH_KEY: &str = "__live_publish__";`（`src/secrets.rs`）
  - `PROTOCOL_VERSION = 19`
  - i18n：`msg::live_on_air_public(lang, title: &str, routes: usize, viewers: u32) -> String`、`msg::live_publish_failed(lang, why: &LiveFailure) -> String`；`Key::LivePublishToggle`（「公开/取消公开」）、`Key::LiveChangeKey`（「换公开密钥」）、`Key::LiveTitlePrompt`（「公开标题：」）、`Key::LiveKeyPrompt`（「公开直播密钥：」）、`Key::LivePublicPending`（「正在公开…」）、`Key::LiveKeySaved`（「公开直播密钥已保存」）、`Key::LiveUnpublished`（「已取消公开」）、`Key::LiveTitleEmpty`（「标题不能为空」）

- [ ] **Step 1: 写失败的测试**

`src/proto.rs` 测试模块：
- 把 `the_request_shape_is_pinned_to_the_protocol_version` 的 `all` 列表末尾加 `Request::LivePublish { title: "t".into() }, Request::LiveUnpublish, Request::LivePublishGrant`，期望串末尾对应加 `,{"LivePublish":{"title":"t"}},"LiveUnpublish","LivePublishGrant"`，四处期望版本号 `18` 改 `19`（含 `the_no_relay_error_is_a_bare_string_on_the_wire`）。
- 追加：

```rust
    /// `LiveInfo` 的公开状态上线形状，以及旧 JSON（没有这个字段）读成 `Private`。
    #[test]
    fn the_live_public_shape_is_pinned() {
        let shape = |p: &LivePublic| serde_json::to_string(p).unwrap();
        assert_eq!(
            (PROTOCOL_VERSION, shape(&LivePublic::Private), shape(&LivePublic::Listed { title: "课".into() })),
            (19, r#""Private""#.to_string(), r#"{"Listed":{"title":"课"}}"#.to_string())
        );
        assert_eq!(
            shape(&LivePublic::Failed { title: "课".into(), reason: LiveFailure::Refused(401) }),
            r#"{"Failed":{"title":"课","reason":{"Refused":401}}}"#
        );
    }

    /// 凭证能当一次公开，不许在任何日志里原样出现。
    #[test]
    fn a_live_grant_is_redacted_in_debug() {
        let r = Response::LiveGrant(LiveGrantToken("deadbeef".repeat(8)));
        assert!(!format!("{r:?}").contains("deadbeef"));
        assert_eq!(serde_json::to_string(&r).unwrap(), format!(r#"{{"LiveGrant":"{}"}}"#, "deadbeef".repeat(8)));
    }
```

`src/i18n.rs`：`every_error_code_composes_in_both_languages` 的列表加 `LivePublishKeyMissing,`；追加：

```rust
    #[test]
    fn public_live_strings_compose_in_both_languages() {
        use crate::proto::LiveFailure;
        for l in Lang::all() {
            let on = msg::live_on_air_public(*l, "第3课", 2, 7);
            assert!(on.contains("第3课") && on.contains('7'), "{on}");
            for code in [401u16, 403, 413, 429, 500] {
                let s = msg::live_publish_failed(*l, &LiveFailure::Refused(code));
                assert!(!s.trim().is_empty());
                if *l == Lang::En {
                    assert!(!has_han(&s), "{s}");
                }
            }
            assert!(!msg::live_publish_failed(*l, &LiveFailure::Unreachable).is_empty());
        }
    }
```

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test --lib proto:: i18n::`
Expected: 编译失败。

- [ ] **Step 3: 实现**

`src/proto.rs`：
1. `PROTOCOL_VERSION` 改 19，文档注释追加一段：

```rust
/// 19 = 公开直播列表：多了 `Request::LivePublish`/`LiveUnpublish`/`LivePublishGrant`、
/// `Response::LiveGrant`、`ErrorCode::LivePublishKeyMissing`，`LiveInfo` 多了 `public`。
/// 新增 `Request` 变体那条规矩同 14。
```

2. `LiveFailure` 之后加：

```rust
/// 这场直播在公开列表上的状态。**以中转为准**：推帧线程每次保活时从
/// `GET /live/{id}/lanes` 的 `public` 字段读回来（包括管理台代为公开的情况）。
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum LivePublic {
    #[default]
    Private,
    /// 本机请求了公开，中转还没答应。
    Pending { title: String },
    Listed { title: String },
    /// 本机请求的公开被拒了。原因是码，组句在界面（`msg::live_publish_failed`）。
    Failed { title: String, reason: LiveFailure },
}

/// 守护进程签发给管理台的公开凭证（见 `dct_link::live::publish_grant`）。
/// 手写 `Debug`：它能拿去公开这场直播，不许原样进日志。
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct LiveGrantToken(pub String);

impl std::fmt::Debug for LiveGrantToken {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("LiveGrantToken(<redacted>)")
    }
}
```

3. `LiveInfo` 加字段（在 `readiness` 之后）：

```rust
    /// 公开列表上的状态，见 [`LivePublic`]。旧 JSON 没有这个字段时读成 `Private`。
    #[serde(default)]
    pub public: LivePublic,
```

`impl Debug for LiveInfo` 加 `.field("public", &self.public)`。
4. `Request` 在 `LiveStatus` 之后加：

```rust
    /// 公开这场直播。守护进程用自己存的发布密钥（`secrets::LIVE_PUBLISH_KEY`），
    /// 密钥**不在这条请求里**。标题超过 `MAX_PUBLIC_TITLE_CHARS` 由守护进程截断。
    LivePublish { title: String },
    LiveUnpublish,
    /// 给管理台一张只能切换公开状态的凭证。没在播回 `LiveStagingRejected(NotLive)`。
    LivePublishGrant,
```

`Request` 的手写 `Debug` 对应加三支（`LivePublish` 显示标题，另两支显示名字）。
5. `Response` 加 `LiveGrant(LiveGrantToken),`；`ErrorCode` 加：

```rust
    /// 要公开直播，但本机还没填公开直播密钥。
    LivePublishKeyMissing,
```

6. 所有 `LiveInfo { ... }` 字面量加 `public: LivePublic::Private,`（测试里需要别的值的按需写）。

`src/secrets.rs` 在 `GATE_TOKEN_KEY` 之后加：

```rust
/// 公开直播的发布密钥（运营方在中转上用 `dct-srv key add` 签发）。同样用一个
/// profile 不可能占用的名字；它只在守护进程里用，不经过任何发给界面的响应。
pub const LIVE_PUBLISH_KEY: &str = "__live_publish__";
```

`src/i18n.rs`：
- `msg::error` 的 `match` 加：

```rust
            LivePublishKeyMissing => t!(
                lang,
                en: "no public live key yet — press K in the live panel to enter the one your operator gave you".to_string(),
                zh: "还没填公开直播密钥：在直播面板里按 K，填上管理员发给你的那把".to_string(),
            ),
```

- `msg` 里 `live_on_air` 之后加：

```rust
    pub fn live_on_air_public(lang: Lang, title: &str, routes: usize, viewers: u32) -> String {
        t!(
            lang,
            en: format!("\u{25cf} LIVE PUBLICLY \u{b7} {title} \u{b7} {routes} lane(s) \u{b7} {viewers} watching"),
            zh: format!("\u{25cf} 正在公开直播 \u{b7} {title} \u{b7} {routes} 路 \u{b7} {viewers} 人在看"),
        )
    }

    /// 公开失败的整句话。401/403/413/429 各有一句人话，别的码照实报出来。
    pub fn live_publish_failed(lang: Lang, why: &crate::proto::LiveFailure) -> String {
        use crate::proto::LiveFailure;
        let reason = match why {
            LiveFailure::Unreachable => t!(lang,
                en: "cannot reach the relay, will keep retrying".to_string(),
                zh: "连不上中转，稍后会自动重试".to_string()),
            LiveFailure::Refused(401) => t!(lang,
                en: "the public live key is wrong or has been revoked".to_string(),
                zh: "公开直播密钥不对，或者已被吊销".to_string()),
            LiveFailure::Refused(403) => t!(lang,
                en: "the server does not allow public lives, or this one was taken down".to_string(),
                zh: "服务器没有开启公开直播，或者这场直播已被下线".to_string()),
            LiveFailure::Refused(413) => t!(lang,
                en: "the title is too long".to_string(),
                zh: "标题太长".to_string()),
            LiveFailure::Refused(429) => t!(lang,
                en: "too many attempts, try again in a minute".to_string(),
                zh: "操作太频繁，一分钟后再试".to_string()),
            LiveFailure::Refused(code) => t!(lang,
                en: format!("the relay refused it (HTTP {code})"),
                zh: format!("中转拒绝了（状态码 {code}）")),
        };
        t!(lang, en: format!("Not public: {reason}"), zh: format!("没能公开：{reason}"))
    }
```

- `Key` 枚举加八个键，`text()` 对应中英文案（按 Interfaces 里写的中文；英文：`p public/private`、`K change key`、`Public title: `、`Public live key: `、`Making it public…`、`Public live key saved`、`No longer public`、`The title cannot be empty`）；词条完整性守卫测试（`every_key_has_text` 一类，`grep -n "fn every_" src/i18n.rs` 找）里把新键加进列表。

- [ ] **Step 4: 跑测试确认通过**

Run: `env -u TERM cargo test --workspace --no-fail-fast`
Expected: PASS。

- [ ] **Step 5: 变异检查**

`LiveGrantToken` 的 `Debug` 改成 `f.write_str(&self.0)`（`a_live_grant_is_redacted_in_debug` 红）；`PROTOCOL_VERSION` 退回 18（形状测试红）。改回。

- [ ] **Step 6: Commit**

```bash
git add src
git commit -m "feat(proto): protocol 19 carries the public live state, publish requests and grant"
```

---

### Task 7: 守护进程 —— 公开意图、推帧线程、自愈

**Files:**
- Modify: `src/live.rs`
- Modify: `src/daemon.rs`

**Interfaces:**
- Consumes: Task 1 `publish_grant`、`push_hash`、`public_path`、`MAX_PUBLIC_TITLE_CHARS`；Task 6 全部。
- Produces:
  - `LiveState::publish(&self, title: String, key: String) -> Option<LiveInfo>`
  - `LiveState::unpublish(&self) -> Option<LiveInfo>`
  - `LiveState::grant(&self) -> Option<String>`
  - 纯函数 `fn public_action(want: Option<&str>, applied: bool, stuck: bool, rev: u64) -> PublicAction`，`enum PublicAction { Nothing, Put, Delete }`
  - 纯函数 `fn seen_public(want: Option<&str>, current: &LivePublic, relay_title: Option<String>) -> (LivePublic, bool /*需要重新 PUT*/)`
  - 守护进程处理 `LivePublish` / `LiveUnpublish` / `LivePublishGrant`

- [ ] **Step 1: 写失败的测试（纯函数与状态槽）**

`src/live.rs` 的 `mod tests` 追加：

```rust
    fn live_with_room() -> LiveState {
        let live = LiveState::new("https://x".to_string());
        live.start(vec![(1, "一".into())]);
        live
    }

    #[test]
    fn publishing_needs_a_room_and_records_pending() {
        let live = LiveState::new("https://x".to_string());
        assert!(live.publish("课".into(), "k".into()).is_none(), "没在播不能公开");
        let live = live_with_room();
        let info = live.publish("课".into(), "k".into()).unwrap();
        assert_eq!(info.public, LivePublic::Pending { title: "课".into() });
        assert_eq!(live.unpublish().unwrap().public, LivePublic::Private);
    }

    #[test]
    fn the_grant_is_the_shared_hmac_of_this_rooms_push_secret() {
        let live = live_with_room();
        let secret = live.push_secret().unwrap();
        let id = live.info().id;
        assert_eq!(
            live.grant().unwrap(),
            dct_link::live::publish_grant(&dct_link::live::push_hash(&secret), &id)
        );
        live.stop();
        assert!(live.grant().is_none());
    }

    /// 发布密钥不许从 `info()` 或 `Debug` 漏出去。
    #[test]
    fn the_publish_key_never_leaves_the_state() {
        let live = live_with_room();
        let info = live.publish("课".into(), "SUPER-SECRET-KEY".into()).unwrap();
        assert!(!serde_json::to_string(&info).unwrap().contains("SUPER-SECRET-KEY"));
        assert!(!format!("{info:?}").contains("SUPER-SECRET-KEY"));
    }

    #[test]
    fn what_to_tell_the_relay_about_publicity() {
        use PublicAction::*;
        assert_eq!(public_action(None, false, false, 0), Nothing, "从没碰过公开");
        assert_eq!(public_action(Some("课"), false, false, 1), Put);
        assert_eq!(public_action(Some("课"), true, false, 1), Nothing, "这一版已经送达");
        assert_eq!(public_action(Some("课"), false, true, 1), Nothing, "被 401/403/413 拒过，不重试");
        assert_eq!(public_action(None, false, false, 2), Delete, "撤销过公开");
        assert_eq!(public_action(None, true, false, 2), Nothing);
    }

    /// 以中转为准，外加唯一一条自愈规则。
    #[test]
    fn reading_publicity_back_from_the_relay() {
        let listed = LivePublic::Listed { title: "课".into() };
        // 本机不想公开：照实显示中转说的（管理台代为公开的就是这种）
        assert_eq!(seen_public(None, &LivePublic::Private, Some("课".into())), (listed.clone(), false));
        assert_eq!(seen_public(None, &listed, None), (LivePublic::Private, false));
        // 本机想公开、中转也说公开
        assert_eq!(seen_public(Some("课"), &LivePublic::Pending { title: "课".into() }, Some("课".into())), (listed.clone(), false));
        // 本机想公开、之前已上列表、中转却说不公开 → 回到 Pending 并要求再 PUT 一次
        assert_eq!(seen_public(Some("课"), &listed, None), (LivePublic::Pending { title: "课".into() }, true));
        // 已经 Failed（被拒）的，中转说不公开是意料之中：保持失败，不重试
        let failed = LivePublic::Failed { title: "课".into(), reason: LiveFailure::Refused(401) };
        assert_eq!(seen_public(Some("课"), &failed, None), (failed.clone(), false));
    }
```

- [ ] **Step 2: 写失败的测试（推帧线程对假中转）**

扩展 `FakeState`：

```rust
        /// `PUT .../public` 要答的状态码（默认 204）。
        public_status: u16,
        /// `GET .../lanes` 答复里的 `public` 标题（默认 `None`）。
        lanes_public: Option<String>,
```

（`#[derive(Default)]` 下 `public_status` 会是 0，`serve_one` 里当 0 为 204。）`serve_one` 里，把现在的

```rust
        let lanes_query = path.ends_with("/lanes");
        st.seen.push(Seen {
```

改成

```rust
        let lanes_query = path.ends_with("/lanes");
        let public_put = method == "PUT" && path.ends_with("/public");
        let lanes_public = st.lanes_public.clone();
        let public_status = st.public_status;
        st.seen.push(Seen {
```

再把 `drop(st);` 之后的答复分支整个换成：

```rust
        let out = if lanes_query {
            let public = match lanes_public {
                Some(t) => format!(r#"{{"title":"{t}"}}"#),
                None => "null".to_string(),
            };
            let json = format!(r#"{{"lanes":["前端"],"viewers":5,"public":{public}}}"#);
            format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{json}", json.len())
        } else if public_put && public_status != 0 && public_status != 204 {
            format!("HTTP/1.1 {public_status} X\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
        } else {
            "HTTP/1.1 204 X\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".to_string()
        };
```

（`st_public_title` / `st_public_status` 是在持锁时 `st.lanes_public.clone()` / `st.public_status` 取出来的局部变量——实现者按现有 `serve_one` 的锁范围摆放。）

追加测试（沿用本文件已有的 `until(...)` 和起推帧线程的方式；`grep -n "spawn_pusher(" src/live.rs` 看既有测试怎么起线程、怎么造 `SessionManager`）：

```rust
    /// 公开：推帧线程带着推帧钥匙和发布密钥去 PUT，成功后状态翻成 Listed。
    #[test]
    fn the_pusher_publishes_with_the_push_secret_and_the_key() {
        let srv = FakeSrv::start();
        let live = Arc::new(LiveState::new(srv.base()));
        let mgr = Arc::new(crate::session::SessionManager::new());
        live.start(vec![]);
        live.publish("第3课".into(), "the-key".into());
        let pusher = spawn_pusher(live.clone(), mgr);
        until("PUT 发出去", || srv.seen().iter().any(|s| s.method == "PUT" && s.path.ends_with("/public")));
        let put = srv.seen().into_iter().find(|s| s.method == "PUT").unwrap();
        assert_eq!(put.headers.get("x-live-push"), live.push_secret().as_ref());
        let body = String::from_utf8(put.body).unwrap();
        assert!(body.contains(r#""title":"第3课""#) && body.contains(r#""key":"the-key""#), "{body}");
        until("状态翻成 Listed", || live.info().public == LivePublic::Listed { title: "第3课".into() });
        pusher.stop();
    }

    /// 被 401 拒：Failed，而且**不再重试**。
    #[test]
    fn a_refused_publication_is_not_retried() {
        let srv = FakeSrv::start();
        recover(srv.state.lock()).public_status = 401;
        let live = Arc::new(LiveState::new(srv.base()));
        let mgr = Arc::new(crate::session::SessionManager::new());
        live.start(vec![]);
        live.publish("课".into(), "bad".into());
        let pusher = spawn_pusher(live.clone(), mgr);
        until("失败", || matches!(live.info().public, LivePublic::Failed { .. }));
        std::thread::sleep(PUSH_INTERVAL * 6);
        let puts = srv.seen().iter().filter(|s| s.method == "PUT" && s.path.ends_with("/public")).count();
        assert_eq!(puts, 1, "被拒之后又去撞了中转 {puts} 次");
        pusher.stop();
    }

    /// 取消公开：发 DELETE。
    #[test]
    fn unpublishing_sends_a_delete() {
        let srv = FakeSrv::start();
        let live = Arc::new(LiveState::new(srv.base()));
        let mgr = Arc::new(crate::session::SessionManager::new());
        live.start(vec![]);
        live.publish("课".into(), "k".into());
        let pusher = spawn_pusher(live.clone(), mgr);
        until("先公开", || matches!(live.info().public, LivePublic::Listed { .. }));
        live.unpublish();
        until("DELETE 发出去", || srv.seen().iter().any(|s| s.method == "DELETE" && s.path.ends_with("/public")));
        pusher.stop();
    }

    /// 管理台代为公开：本机没请求过，但中转的 lanes 说公开 → 界面显示 Listed，且本机不发 PUT。
    #[test]
    fn a_publication_made_elsewhere_is_reflected_without_a_put() {
        let srv = FakeSrv::start();
        recover(srv.state.lock()).lanes_public = Some("管理台公开的".into());
        let live = Arc::new(LiveState::new(srv.base()));
        let mgr = Arc::new(crate::session::SessionManager::new());
        live.start(vec![]);
        let pusher = spawn_pusher(live.clone(), mgr);
        until("读到公开", || live.info().public == LivePublic::Listed { title: "管理台公开的".into() });
        assert!(!srv.seen().iter().any(|s| s.method == "PUT"), "不是本机发起的公开，本机不许 PUT");
        pusher.stop();
    }
```

（`live.start(vec![])` 空名单跟既有推帧线程测试一样：这里测的是开房与公开，不需要真的会话。）

`src/daemon.rs` 测试模块追加（沿用 `bare_handle_deps`、`one_staged_session`、`test_live` 等助手）：

```rust
    #[test]
    fn live_publish_without_a_key_is_refused_with_a_code() {
        let (mgr, store, secrets, profiles_dir) = bare_handle_deps();
        let (id, _dir) = one_staged_session(&mgr);
        let live = test_live();
        live.start(vec![(id, "一".into())]);
        let resp = handle(Request::LivePublish { title: "课".into() }, &mgr, &store, &secrets, profiles_dir.path(),
            &test_phone(), &test_bridge(), &test_event_tx(), None, &test_pairs(), &live);
        assert!(matches!(resp, Response::Error(ErrorCode::LivePublishKeyMissing)), "{resp:?}");
    }

    #[test]
    fn live_publish_uses_the_stored_key_and_truncates_the_title() {
        let (mgr, store, secrets, profiles_dir) = bare_handle_deps();
        let (id, _dir) = one_staged_session(&mgr);
        recover(secrets.lock()).set(crate::secrets::LIVE_PUBLISH_KEY, "k").unwrap();
        let live = test_live();
        live.start(vec![(id, "一".into())]);
        let long = "字".repeat(dct_link::live::MAX_PUBLIC_TITLE_CHARS + 5);
        let resp = handle(Request::LivePublish { title: long }, &mgr, &store, &secrets, profiles_dir.path(),
            &test_phone(), &test_bridge(), &test_event_tx(), None, &test_pairs(), &live);
        let Response::Live(info) = resp else { panic!("{resp:?}") };
        let LivePublic::Pending { title } = info.public else { panic!("{:?}", info.public) };
        assert_eq!(title.chars().count(), dct_link::live::MAX_PUBLIC_TITLE_CHARS);
    }

    #[test]
    fn a_grant_needs_a_live_room() {
        let (mgr, store, secrets, profiles_dir) = bare_handle_deps();
        let live = test_live();
        let resp = handle(Request::LivePublishGrant, &mgr, &store, &secrets, profiles_dir.path(),
            &test_phone(), &test_bridge(), &test_event_tx(), None, &test_pairs(), &live);
        assert!(matches!(resp, Response::Error(ErrorCode::LiveStagingRejected(LiveStagingProblem::NotLive))), "{resp:?}");
    }
```

- [ ] **Step 3: 跑测试确认失败**

Run: `cargo test --lib live:: daemon::`
Expected: 编译失败。

- [ ] **Step 4: 实现 `src/live.rs`**

1. `Room` 加字段：

```rust
    /// 本机想让它公开时的标题；`None` = 本机没请求公开（可能是管理台公开的）。
    want_public: Option<String>,
    /// 本机请求公开时拿到的发布密钥。**只活在守护进程内存里**，不进 `LiveInfo`。
    public_key: Option<String>,
    /// 界面看到的公开状态，见 `LivePublic`。
    public: LivePublic,
    /// 公开意图改过几次。推帧线程靠它认出「要重新 PUT/DELETE」。
    public_rev: u64,
```

`start()` 里新房间这四个字段为 `None, None, LivePublic::Private, 0`；`info()` / `restage()` 构造 `LiveInfo` 时带 `public: room.public.clone()`（没在播时 `LivePublic::Private`）。
2. `RoomSnapshot` 加 `want_public: Option<String>`、`public_key: Option<String>`、`public_rev: u64`，`snapshot()` 抄过去。
3. `impl LiveState` 加：

```rust
    pub fn publish(&self, title: String, key: String) -> Option<LiveInfo> {
        {
            let mut guard = recover(self.room.lock());
            let room = guard.as_mut()?;
            room.public = LivePublic::Pending { title: title.clone() };
            room.want_public = Some(title);
            room.public_key = Some(key);
            room.public_rev += 1;
        }
        Some(self.info())
    }

    pub fn unpublish(&self) -> Option<LiveInfo> {
        {
            let mut guard = recover(self.room.lock());
            let room = guard.as_mut()?;
            room.public = LivePublic::Private;
            room.want_public = None;
            room.public_key = None;
            room.public_rev += 1;
        }
        Some(self.info())
    }

    /// 管理台用的公开凭证。没在播是 `None`。
    pub fn grant(&self) -> Option<String> {
        recover(self.room.lock()).as_ref().map(|r| {
            dct_link::live::publish_grant(&dct_link::live::push_hash(&r.push_secret), &r.id)
        })
    }

    pub(crate) fn mark_public(&self, id: &str, rev: u64, public: LivePublic) {
        if let Some(room) = recover(self.room.lock()).as_mut() {
            if room.id == id && room.public_rev == rev {
                room.public = public;
            }
        }
    }

    /// 推帧线程把中转说的公开状态写回；回 `true` 表示要重新 PUT 一次（自愈）。
    pub(crate) fn set_public_seen(&self, id: &str, relay_title: Option<String>) -> bool {
        let mut guard = recover(self.room.lock());
        let Some(room) = guard.as_mut() else { return false };
        if room.id != id {
            return false;
        }
        let (next, heal) = seen_public(room.want_public.as_deref(), &room.public, relay_title);
        room.public = next;
        heal
    }
```

（`push_secret()` 上的 `#[allow(dead_code)]` 若因此不再需要就删掉。）
4. 纯函数：

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PublicAction {
    Nothing,
    Put,
    Delete,
}

/// 这一版公开意图（`rev`）该对中转做什么。`applied` = 这一版已经送达；
/// `stuck` = 这一版被 401/403/413 拒过（不重试，直到老师再按一次 `p`）。
fn public_action(want: Option<&str>, applied: bool, stuck: bool, rev: u64) -> PublicAction {
    if applied || stuck {
        return PublicAction::Nothing;
    }
    match want {
        Some(_) => PublicAction::Put,
        None if rev > 0 => PublicAction::Delete,
        None => PublicAction::Nothing,
    }
}

/// 以中转为准写回公开状态，并判断要不要自愈。唯一的自愈规则：本机想公开、
/// 之前已上列表、中转却说不公开 → 回到 Pending，再 PUT 一次，由那次答复定结果。
fn seen_public(want: Option<&str>, current: &LivePublic, relay_title: Option<String>) -> (LivePublic, bool) {
    match (want, relay_title) {
        (None, Some(t)) => (LivePublic::Listed { title: t }, false),
        (None, None) => (LivePublic::Private, false),
        (Some(_), Some(t)) => (LivePublic::Listed { title: t }, false),
        (Some(w), None) => match current {
            LivePublic::Listed { .. } => (LivePublic::Pending { title: w.to_string() }, true),
            other => (other.clone(), false),
        },
    }
}
```

5. HTTP 小函数：

```rust
#[derive(serde::Serialize)]
struct PublishBody<'a> {
    title: &'a str,
    key: &'a str,
}

fn put_public(agent: &ureq::Agent, base: &str, id: &str, secret: &str, title: &str, key: &str) -> Result<(), LiveFailure> {
    let url = format!("{base}{}", dct_link::live::public_path(id));
    match agent.put(&url).set("x-live-push", secret).send_json(PublishBody { title, key }) {
        Ok(_) => Ok(()),
        Err(ureq::Error::Status(code, _)) => Err(LiveFailure::Refused(code)),
        Err(ureq::Error::Transport(_)) => Err(LiveFailure::Unreachable),
    }
}

fn delete_public(agent: &ureq::Agent, base: &str, id: &str, secret: &str) -> bool {
    let url = format!("{base}{}", dct_link::live::public_path(id));
    agent.delete(&url).set("x-live-push", secret).call().is_ok()
}
```

`LanesBody` 加 `#[serde(default)] public: Option<PublicTitleBody>`，`struct PublicTitleBody { title: String }`；`fetch_viewers` 改名为 `fetch_lanes`，返回 `Option<(u32, Option<String>)>`，并更新 `fetch_viewers_asks_the_lanes_route_with_the_read_only_token` 测试里的调用。
6. `pusher_loop`：在 `started` 相关变量旁加

```rust
    // 这一场、这一版公开意图（`(id, public_rev)`）已经送达 / 被拒不再重试。
    let mut public_applied: Option<(String, u64)> = None;
    let mut public_stuck: Option<(String, u64)> = None;
    // 连不上中转时的下一次重试时间，复用开播的退避表。
    let mut public_retry_at: Option<Instant> = None;
    let mut public_fails: u32 = 0;
```

在 `start_room` 成功的那一支（`started = Some(this);` 之后）加 `public_applied = None; public_stuck = None;`——房间在中转上是新开的，公开状态不继承，需要重新送达。在读人数那一段之前插入：

```rust
                let pub_this = (room.id.clone(), room.public_rev);
                let due = public_retry_at.is_none_or(|t| Instant::now() >= t);
                if due {
                    match public_action(
                        room.want_public.as_deref(),
                        public_applied.as_ref() == Some(&pub_this),
                        public_stuck.as_ref() == Some(&pub_this),
                        room.public_rev,
                    ) {
                        PublicAction::Nothing => {}
                        PublicAction::Put => {
                            let title = room.want_public.clone().unwrap_or_default();
                            let key = room.public_key.clone().unwrap_or_default();
                            match put_public(&agent, &base, &room.id, &room.push_secret, &title, &key) {
                                Ok(()) => {
                                    live.mark_public(&room.id, room.public_rev, LivePublic::Listed { title });
                                    public_applied = Some(pub_this);
                                    public_fails = 0;
                                    public_retry_at = None;
                                }
                                Err(LiveFailure::Refused(code)) if matches!(code, 401 | 403 | 413) => {
                                    live.mark_public(&room.id, room.public_rev, LivePublic::Failed { title, reason: LiveFailure::Refused(code) });
                                    public_stuck = Some(pub_this);
                                }
                                Err(reason) => {
                                    live.mark_public(&room.id, room.public_rev, LivePublic::Failed { title, reason });
                                    public_fails += 1;
                                    public_retry_at = Some(Instant::now() + start_backoff(public_fails));
                                }
                            }
                        }
                        PublicAction::Delete => {
                            if delete_public(&agent, &base, &room.id, &room.push_secret) {
                                public_applied = Some(pub_this);
                            }
                        }
                    }
                }
```

读人数那一段改成：

```rust
                    if let Some((n, public)) = fetch_lanes(&agent, &base, &room.id, &room.token) {
                        live.set_viewers(&room.id, n);
                        if live.set_public_seen(&room.id, public) {
                            // 自愈：让下一轮重新送达这一版公开意图。
                            public_applied = None;
                        }
                    }
```

`None =>`（没在播）那一支把四个公开变量都清掉。
7. `start_backoff` 若是私有函数，保持同一文件内调用即可。

- [ ] **Step 5: 实现 `src/daemon.rs`**

在 `Request::LiveStatus => ...` 之后加：

```rust
        Request::LivePublish { title } => Ok(live_publish(live, secrets, title)),
        Request::LiveUnpublish => Ok(match live.unpublish() {
            Some(info) => Response::Live(info),
            None => Response::Error(ErrorCode::LiveStagingRejected(LiveStagingProblem::NotLive)),
        }),
        Request::LivePublishGrant => Ok(match live.grant() {
            Some(g) => Response::LiveGrant(crate::proto::LiveGrantToken(g)),
            None => Response::Error(ErrorCode::LiveStagingRejected(LiveStagingProblem::NotLive)),
        }),
```

并加函数（`secrets` 的具体类型按 `handle` 签名写）：

```rust
/// 公开这场直播：用本机存的发布密钥。**密钥从这里进 `LiveState`，不经过任何响应。**
/// 标题截到中转收得下的长度，理由同路名（`validated_staging`）。
fn live_publish(
    live: &Arc<crate::live::LiveState>,
    secrets: &Arc<Mutex<SecretStore>>,
    title: String,
) -> Response {
    let Some(key) = recover(secrets.lock()).get(crate::secrets::LIVE_PUBLISH_KEY).map(str::to_string) else {
        return Response::Error(ErrorCode::LivePublishKeyMissing);
    };
    let title: String = title.trim().chars().take(dct_link::live::MAX_PUBLIC_TITLE_CHARS).collect();
    match live.publish(title, key) {
        Some(info) => Response::Live(info),
        None => Response::Error(ErrorCode::LiveStagingRejected(LiveStagingProblem::NotLive)),
    }
}
```

- [ ] **Step 6: 跑测试确认通过**

Run: `env -u TERM cargo test --workspace --no-fail-fast`
Expected: PASS。

- [ ] **Step 7: 变异检查**

`public_action` 里 `applied || stuck` 改成 `applied`（`a_refused_publication_is_not_retried` 红）；`seen_public` 的 `(None, Some(t))` 改成 `(LivePublic::Private, false)`（`a_publication_made_elsewhere...` 红）；`start_room` 成功支删掉 `public_applied = None;`（写一条临时测试或确认 `reading_publicity_back_from_the_relay` + 手工推理，报告里写明结论）；`live_publish` 不截断标题（`live_publish_uses_the_stored_key_and_truncates_the_title` 红）。改回。

- [ ] **Step 8: Commit**

```bash
git add src/live.rs src/daemon.rs
git commit -m "feat(live): the daemon publishes with its stored key and heals publicity after relay restarts"
```

---

### Task 8: 界面 —— 直播面板 `p`/`K` 与底栏公开提示

**Files:**
- Modify: `src/ui/view.rs`（`View::Live` 加 `input`、`LiveInput`、按键表与逃生提示）
- Modify: `src/ui/live.rs`（按键、输入行、横幅文案与样式）
- Modify: `src/ui/mod.rs`（`View::Live` 的模式匹配、`bar_live_style`、实色底栏守卫加一屏）
- Modify: `src/ui/app.rs`（`last_public_title: String`）

**Interfaces:**
- Consumes: Task 6 的 `LivePublic`、新请求、`ErrorCode::LivePublishKeyMissing`、文案键；`secrets::LIVE_PUBLISH_KEY`。
- Produces:
  - `pub enum LiveInput { Title(String), Key { buf: String, then_publish: Option<String> } }`（`Clone, Debug, PartialEq`）
  - `View::Live { state: ListState, input: Option<LiveInput> }`

- [ ] **Step 1: 写失败的测试**

`src/ui/live.rs` 的 `mod tests` 里已有 `fake_live_daemon(answer)`（2026-09-13 的轮询测试用它）。给它加第二个参数 `has_key: std::sync::Arc<std::sync::atomic::AtomicBool>`，线程闭包里 `let has_key2 = has_key.clone();` 带进去，并把 `let resp = match req { ... }` 换成：

```rust
                    let resp = match req {
                        Request::LiveStatus => {
                            asked2.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                            Response::Live(answer.lock().unwrap().clone())
                        }
                        Request::LivePublish { title } => {
                            if has_key2.load(std::sync::atomic::Ordering::SeqCst) {
                                let mut info = answer.lock().unwrap().clone();
                                info.public = LivePublic::Pending { title };
                                Response::Live(info)
                            } else {
                                Response::Error(crate::proto::ErrorCode::LivePublishKeyMissing)
                            }
                        }
                        Request::SetSecret { profile, .. } if profile == crate::secrets::LIVE_PUBLISH_KEY => {
                            has_key2.store(true, std::sync::atomic::Ordering::SeqCst);
                            Response::Ok
                        }
                        Request::LiveUnpublish => {
                            let mut info = answer.lock().unwrap().clone();
                            info.public = LivePublic::Private;
                            Response::Live(info)
                        }
                        _ => Response::Ok,
                    };
```

既有两条调用它的测试补第二个实参 `std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false))`。然后追加测试：

```rust
    fn press_live(app: &mut App, code: KeyCode) {
        handle_key(app, KeyEvent::new(code, crossterm::event::KeyModifiers::NONE)).unwrap();
    }

    fn type_text(app: &mut App, s: &str) {
        for c in s.chars() {
            press_live(app, KeyCode::Char(c));
        }
    }

    /// 在播时按 `p` → 填标题 → Enter → 没密钥就转到填密钥 → Enter → 自动继续公开。
    #[test]
    fn publishing_asks_for_a_title_then_a_key_when_missing_then_continues() {
        let has_key = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let answer = Arc::new(Mutex::new(info(vec![(1, "一".into())], 0, LiveReadiness::Ready)));
        let (sock, _fake, _asked) = fake_live_daemon(answer, has_key.clone());
        let (mut app, _dir) = App::test_app();
        app.client = Some(crate::client::Client::connect(&sock).unwrap());
        app.connected = true;
        app.live = info(vec![(1, "一".into())], 0, LiveReadiness::Ready);
        app.view = View::Live { state: ListState::default(), input: None };

        press_live(&mut app, KeyCode::Char('p'));
        assert!(matches!(&app.view, View::Live { input: Some(LiveInput::Title(_)), .. }));
        type_text(&mut app, "第3课");
        press_live(&mut app, KeyCode::Enter);
        assert!(
            matches!(&app.view, View::Live { input: Some(LiveInput::Key { then_publish: Some(t), .. }), .. } if t == "第3课"),
            "没密钥要转去填密钥，并记着要继续公开"
        );
        type_text(&mut app, "the-key");
        press_live(&mut app, KeyCode::Enter);
        assert!(has_key.load(std::sync::atomic::Ordering::SeqCst), "密钥要存进守护进程");
        assert!(matches!(app.live.public, LivePublic::Pending { .. }), "存完密钥要自动继续公开");
        assert!(matches!(&app.view, View::Live { input: None, .. }));
        assert_eq!(app.last_public_title, "第3课");
    }

    /// 密钥输入不许出现在屏幕上。
    #[test]
    fn the_key_being_typed_is_never_drawn() {
        let (mut app, _dir) = App::test_app();
        app.live = info(vec![(1, "一".into())], 0, LiveReadiness::Ready);
        app.view = View::Live {
            state: ListState::default(),
            input: Some(LiveInput::Key { buf: "SUPER-SECRET".into(), then_publish: None }),
        };
        let screen = screen_of(&mut app, 100, 40);
        assert!(!screen.contains("SUPER-SECRET"), "{screen}");
    }

    #[test]
    fn an_empty_title_is_refused_on_the_spot() {
        let (mut app, _dir) = App::test_app();
        app.live = info(vec![(1, "一".into())], 0, LiveReadiness::Ready);
        app.view = View::Live { state: ListState::default(), input: Some(LiveInput::Title(String::new())) };
        press_live(&mut app, KeyCode::Enter);
        assert!(app.message.error, "空标题要当场说");
        assert!(matches!(&app.view, View::Live { input: Some(LiveInput::Title(_)), .. }), "输入行不关");
    }

    /// 公开中按 `p` 是取消公开，不再弹标题。
    #[test]
    fn p_on_a_public_live_unpublishes() {
        let has_key = Arc::new(std::sync::atomic::AtomicBool::new(true));
        let answer = Arc::new(Mutex::new(info(vec![(1, "一".into())], 0, LiveReadiness::Ready)));
        let (sock, _fake, _asked) = fake_live_daemon(answer, has_key);
        let (mut app, _dir) = App::test_app();
        app.client = Some(crate::client::Client::connect(&sock).unwrap());
        app.connected = true;
        let mut live = info(vec![(1, "一".into())], 0, LiveReadiness::Ready);
        live.public = LivePublic::Listed { title: "课".into() };
        app.live = live;
        app.view = View::Live { state: ListState::default(), input: None };
        press_live(&mut app, KeyCode::Char('p'));
        assert_eq!(app.live.public, LivePublic::Private);
        assert!(matches!(&app.view, View::Live { input: None, .. }));
    }

    #[test]
    fn the_banner_says_public_with_the_title() {
        let mut live = info(vec![(1, "一".into())], 7, LiveReadiness::Ready);
        live.public = LivePublic::Listed { title: "第3课".into() };
        let s = live_banner(&live, Lang::Zh);
        assert!(s.contains("正在公开直播") && s.contains("第3课") && s.contains('7'), "{s}");
        live.public = LivePublic::Failed { title: "第3课".into(), reason: LiveFailure::Refused(401) };
        assert!(live_banner(&live, Lang::Zh).contains("吊销"));
    }
```

`src/ui/mod.rs` 的 `nothing_on_a_solid_bar_takes_its_color_from_the_terminal_theme`：`cases` 数组长度 +2，加：

```rust
            ("正在公开直播", || {
                let (mut app, dir) = live(crate::proto::LiveReadiness::Ready);
                app.live.public = crate::proto::LivePublic::Listed { title: "第3课".into() };
                (app, dir)
            }),
            ("公开失败", || {
                let (mut app, dir) = live(crate::proto::LiveReadiness::Ready);
                app.live.public = crate::proto::LivePublic::Failed {
                    title: "第3课".into(),
                    reason: crate::proto::LiveFailure::Refused(401),
                };
                (app, dir)
            }),
```

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test --lib ui::`
Expected: 编译失败。

- [ ] **Step 3: 实现**

1. `src/ui/view.rs`：

```rust
/// 直播面板里正在填的那一行。
#[derive(Clone, Debug, PartialEq)]
pub enum LiveInput {
    /// 公开标题。
    Title(String),
    /// 公开直播密钥。`then_publish` = 填完之后继续用这个标题公开
    /// （按 `p` 时守护进程报 `LivePublishKeyMissing` 转过来的）。
    Key { buf: String, then_publish: Option<String> },
}
```

`View::Live { state: ListState }` 改成 `View::Live { state: ListState, input: Option<LiveInput> }`。全仓 `View::Live { state }` 的解构改成 `View::Live { state, .. }` 或按需取 `input`，构造处补 `input: None`（`grep -rn "View::Live" src` 列全）。
按键表（`help_of` 里 `View::Live` 那一支）：`input` 为 `Some` 时只给 `Enter 确认` / `Esc 取消`；否则在播时追加 `("p", Key::LivePublishToggle)` 与 `("K", Key::LiveChangeKey)`。逃生提示：`input` 为 `Some` 时是 `Esc 取消`。
2. `src/ui/app.rs`：`App` 加 `pub last_public_title: String,`，初始化为空串。
3. `src/ui/live.rs`：
- `handle_key` 开头改成解构 `let View::Live { mut state, input } = app.view.clone() else { return Ok(()) };`，并在处理普通按键之前：

```rust
    if let Some(field) = input {
        let next = edit_live_input(app, field, key);
        app.view = View::Live { state, input: next };
        return Ok(());
    }
```

- 普通按键里加：

```rust
        KeyCode::Char('p') if is_plain_key(&key) && is_live(&app.live) => {
            if matches!(app.live.public, LivePublic::Private) {
                app.view = View::Live { state, input: Some(LiveInput::Title(app.last_public_title.clone())) };
                return Ok(());
            }
            unpublish(app);
        }
        KeyCode::Char('K') if is_plain_key(&key) => {
            app.view = View::Live { state, input: Some(LiveInput::Key { buf: String::new(), then_publish: None }) };
            return Ok(());
        }
```

- 新函数：

```rust
/// 输入行的一次按键。回下一步的输入状态（`None` = 关掉输入行）。
fn edit_live_input(app: &mut App, field: LiveInput, key: KeyEvent) -> Option<LiveInput> {
    let buf_of = |f: &LiveInput| match f {
        LiveInput::Title(b) => b.clone(),
        LiveInput::Key { buf, .. } => buf.clone(),
    };
    let with_buf = |f: LiveInput, b: String| match f {
        LiveInput::Title(_) => LiveInput::Title(b),
        LiveInput::Key { then_publish, .. } => LiveInput::Key { buf: b, then_publish },
    };
    match key.code {
        KeyCode::Esc => None,
        KeyCode::Backspace => {
            let mut b = buf_of(&field);
            b.pop();
            Some(with_buf(field, b))
        }
        KeyCode::Char(c) if !key.modifiers.contains(crossterm::event::KeyModifiers::CONTROL) => {
            let mut b = buf_of(&field);
            b.push(c);
            Some(with_buf(field, b))
        }
        KeyCode::Enter => match field {
            LiveInput::Title(t) => {
                let t = t.trim().to_string();
                if t.is_empty() {
                    app.message = Msg::err(text(Key::LiveTitleEmpty, app.lang).into());
                    return Some(LiveInput::Title(t));
                }
                publish(app, t)
            }
            LiveInput::Key { buf, then_publish } => {
                let saved = app.client().and_then(|c| {
                    c.call(Request::SetSecret { profile: crate::secrets::LIVE_PUBLISH_KEY.into(), value: buf })
                });
                match saved {
                    Ok(Response::Ok) => {
                        app.message = text(Key::LiveKeySaved, app.lang).into();
                        match then_publish {
                            Some(t) => publish(app, t),
                            None => None,
                        }
                    }
                    _ => {
                        app.message = Msg::err(text(Key::RequestFailed, app.lang).into());
                        None
                    }
                }
            }
        },
        _ => Some(field),
    }
}

/// 发 `LivePublish`。没密钥就转去填密钥，并记着标题。
fn publish(app: &mut App, title: String) -> Option<LiveInput> {
    match app.client().and_then(|c| c.call(Request::LivePublish { title: title.clone() })) {
        Ok(Response::Live(info)) => {
            app.live = info;
            app.last_public_title = title;
            None
        }
        Ok(Response::Error(ErrorCode::LivePublishKeyMissing)) => {
            Some(LiveInput::Key { buf: String::new(), then_publish: Some(title) })
        }
        Ok(Response::Error(e)) => {
            app.message = Msg::err(msg::error(app.lang, &e));
            None
        }
        _ => {
            app.message = Msg::err(text(Key::RequestFailed, app.lang).into());
            None
        }
    }
}

fn unpublish(app: &mut App) {
    match app.client().and_then(|c| c.call(Request::LiveUnpublish)) {
        Ok(Response::Live(info)) => {
            app.live = info;
            app.message = text(Key::LiveUnpublished, app.lang).into();
        }
        _ => app.message = Msg::err(text(Key::RequestFailed, app.lang).into()),
    }
}
```

- `live_banner` 改成：

```rust
pub(crate) fn live_banner(info: &LiveInfo, lang: Lang) -> String {
    let base = match &info.public {
        LivePublic::Listed { title } | LivePublic::Pending { title } | LivePublic::Failed { title, .. } => {
            msg::live_on_air_public(lang, title, info.staged.len(), info.viewers)
        }
        LivePublic::Private => msg::live_on_air(lang, info.staged.len(), info.viewers),
    };
    let base = match &info.public {
        LivePublic::Pending { .. } => format!("{base} · {}", text(Key::LivePublicPending, lang)),
        LivePublic::Failed { reason, .. } => format!("{base} · {}", msg::live_publish_failed(lang, reason)),
        _ => base,
    };
    match &info.readiness {
        LiveReadiness::Ready => base,
        LiveReadiness::Pending => format!("{base} · {}", text(Key::LiveConnectingToRelay, lang)),
        LiveReadiness::Failed(why) => format!("{base} · {}", msg::live_start_failed(lang, why)),
    }
}
```

- `banner_style`：`LivePublic::Failed { .. }` 时返回 `danger()`（面板内），其余照旧。
- `draw`：在 `lines` 开头（`is_live` 分支里横幅之后）若 `input` 为 `Some`：`Title(b)` 画 `format!("{}{}▏", text(Key::LiveTitlePrompt, lang), b)`；`Key { buf, .. }` 画 `format!("{}{}▏", text(Key::LiveKeyPrompt, lang), "•".repeat(buf.chars().count()))`——**绝不画 `buf` 本身**。
4. `src/ui/mod.rs`：`bar_live_style` 在实色档下，`LivePublic::Failed { .. }` 返回 `bar_danger(t)`，其余按原 `readiness` 规则。`View::Live` 相关匹配补 `..`。

- [ ] **Step 4: 跑测试确认通过**

Run: `env -u TERM cargo test --workspace --no-fail-fast && cargo clippy --workspace --all-targets --locked -- -D warnings`
Expected: PASS。

- [ ] **Step 5: 变异检查**

`draw` 里把圆点改回 `buf`（`the_key_being_typed_is_never_drawn` 红）；`publish` 的 `LivePublishKeyMissing` 分支改成 `None`（`publishing_asks_for_a_title...` 红）；`bar_live_style` 的 `Failed` 改成 `danger()`（实色底栏守卫红）。改回。

- [ ] **Step 6: Commit**

```bash
git add src/ui
git commit -m "feat(ui): publish and unpublish from the live panel, and say so on the bar"
```

---

### Task 9: 管理台后端 —— 公开、取消、自愈、错误映射

**Files:**
- Modify: `container/classroom/server.mjs`
- Modify: `container/classroom/server.test.mjs`

**Interfaces:**
- Consumes: 守护进程 `LivePublishGrant`（Task 7）、中转 `PUT/DELETE /live/{id}/public`、`GET /live/public`（Task 4）。
- Produces:
  - `new Classroom({..., live: {relay: string, publishKey: string}})`（缺省或缺 `publishKey` = 不开公开功能）
  - 管理接口：`POST /admin/api/students/{id}/live-public` `{title}`、`POST /admin/api/students/{id}/live-private`；`live-start` 接受 `{public: true, title}`
  - `GET /admin/api/state` 答复多 `features: {publicLive: boolean}`；每行 `live` 多 `public: {title} | null`、学生行多 `livePublicError: string | null`
  - `Classroom#healPublic()`（后台每 30 秒调一次；测试直接调）

- [ ] **Step 1: 写失败的测试**

在 `server.test.mjs` 追加（沿用「强制直播」那条测试的 `Store` / fake driver / `request` 写法）：

```js
test('公开直播：凭证 + 管理台密钥去中转公开；错误码说人话；自愈遇到吊销就停', async () => {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'dcw-public-test-'));
  const store = new Store(dir, {password: 'test-admin'});
  const [ming] = store.add(['小明']);
  // 假中转：记下请求，按吩咐回状态码，并维护一份「公开列表」
  const seen = []; let putStatus = 204; const listed = new Set();
  const relay = http.createServer((req, res) => {
    let body = ''; req.on('data', c => body += c); req.on('end', () => {
      seen.push({method: req.method, url: req.url, headers: req.headers, body});
      if (req.method === 'GET' && req.url === '/live/public') { res.writeHead(200, {'content-type': 'application/json'}); return res.end(JSON.stringify([...listed].map(id => ({id, title: 't', lanes: [], viewers: 0})))); }
      const m = req.url.match(/^\/live\/([^/]+)\/public$/);
      if (m && req.method === 'PUT') { if (putStatus === 204) listed.add(m[1]); res.writeHead(putStatus); return res.end(); }
      if (m && req.method === 'DELETE') { listed.delete(m[1]); res.writeHead(204); return res.end(); }
      res.writeHead(404); res.end();
    });
  });
  await new Promise(r => relay.listen(0, '127.0.0.1', r));
  const relayUrl = `http://127.0.0.1:${relay.address().port}`;
  let live = {id: 'room01', token: 't'.repeat(64), url: `${relayUrl}/live/room01#t=${'t'.repeat(64)}`, staged: [[1, '小明']], viewers: 2, readiness: 'Ready', public: 'Private'};
  const driver = {
    maxRunning: 2,
    status: async () => ({status: 'running'}),
    rpc: async (_, request) => {
      if (request === 'LiveStatus') return {Live: live};
      if (request === 'LivePublishGrant') return {LiveGrant: 'g'.repeat(64)};
      if (request === 'List') return {Sessions: [{id: 1, profile: 'claude', state: 'Idle'}]};
      if (request.LiveStart) return {Live: live};
      return {Ok: null};
    },
  };
  const app = new Classroom({store, driver, origin: 'http://localhost', secure: false, live: {relay: relayUrl, publishKey: 'K'.repeat(64)}});
  await new Promise(r => app.server.listen(0, '127.0.0.1', r));
  const origin = `http://127.0.0.1:${app.server.address().port}`;
  const request = (url, body, cookie) => fetch(origin + url, {method: body === undefined ? 'GET' : 'POST', headers: {...(body === undefined ? {} : {'Content-Type': 'application/json'}), ...(cookie ? {Cookie: cookie} : {})}, body: body === undefined ? undefined : JSON.stringify(body)});
  try {
    const admin = (await request('/admin/api/login', {password: 'test-admin'})).headers.get('set-cookie').split(';')[0];
    assert.equal((await (await request('/admin/api/state', undefined, admin)).json()).features.publicLive, true);

    const ok = await request(`/admin/api/students/${ming.id}/live-public`, {title: '小明 的工作区'}, admin);
    assert.equal(ok.status, 200);
    const put = seen.find(s => s.method === 'PUT');
    assert.equal(put.url, '/live/room01/public');
    assert.equal(put.headers['x-live-grant'], 'g'.repeat(64), '用凭证，不用推帧钥匙');
    assert.deepEqual(JSON.parse(put.body), {title: '小明 的工作区', key: 'K'.repeat(64)});
    assert.deepEqual(new Store(dir).student(ming.id).livePublic, {title: '小明 的工作区'});
    assert.ok(new Store(dir).data.audit.some(a => /公开直播工作区/.test(a.action || a.text || JSON.stringify(a))));

    // 自愈：中转重启丢了公开状态 → 重新 PUT
    listed.clear(); seen.length = 0;
    await app.healPublic();
    assert.ok(seen.some(s => s.method === 'PUT'), '中转说不公开时要重新公开');

    // 吊销：中转回 401 → 不再重试、清记录、行上带原因
    listed.clear(); putStatus = 401; seen.length = 0;
    await app.healPublic();
    await app.healPublic();
    assert.equal(seen.filter(s => s.method === 'PUT').length, 1, '被吊销之后又去撞了中转');
    const state = await (await request('/admin/api/state', undefined, admin)).json();
    const row = state.students.find(s => s.id === ming.id);
    assert.match(row.livePublicError, /吊销/);

    // 错误映射
    for (const [code, text] of [[401, /吊销/], [403, /没有开启公开直播|下线/], [413, /标题太长/]]) {
      putStatus = code;
      const r = await request(`/admin/api/students/${ming.id}/live-public`, {title: 't'}, admin);
      assert.equal(r.status, 409);
      assert.match((await r.json()).error, text);
    }

    // 取消公开
    putStatus = 204;
    await request(`/admin/api/students/${ming.id}/live-public`, {title: 't'}, admin);
    const off = await request(`/admin/api/students/${ming.id}/live-private`, {}, admin);
    assert.equal(off.status, 200);
    assert.ok(seen.some(s => s.method === 'DELETE' && s.headers['x-live-grant'] === 'g'.repeat(64)));
    assert.equal(new Store(dir).student(ming.id).livePublic, undefined);
  } finally { app.server.close(); relay.close(); fs.rmSync(dir, {recursive: true, force: true}); }
});

test('没配发布密钥：管理台不开公开功能', async () => {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'dcw-public-off-'));
  const store = new Store(dir, {password: 'test-admin'});
  const [ming] = store.add(['小明']);
  const driver = {maxRunning: 2, status: async () => ({status: 'running'}), rpc: async () => ({Ok: null})};
  const app = new Classroom({store, driver, origin: 'http://localhost', secure: false});
  await new Promise(r => app.server.listen(0, '127.0.0.1', r));
  const origin = `http://127.0.0.1:${app.server.address().port}`;
  const request = (url, body, cookie) => fetch(origin + url, {method: body === undefined ? 'GET' : 'POST', headers: {...(body === undefined ? {} : {'Content-Type': 'application/json'}), ...(cookie ? {Cookie: cookie} : {})}, body: body === undefined ? undefined : JSON.stringify(body)});
  try {
    const admin = (await request('/admin/api/login', {password: 'test-admin'})).headers.get('set-cookie').split(';')[0];
    assert.equal((await (await request('/admin/api/state', undefined, admin)).json()).features.publicLive, false);
    assert.equal((await request(`/admin/api/students/${ming.id}/live-public`, {title: 't'}, admin)).status, 404);
  } finally { app.server.close(); fs.rmSync(dir, {recursive: true, force: true}); }
});
```

（`store.student(id)` 若不存在，按 `server.mjs` 里取学生记录的现有方法改；审计记录的字段名按 `Store#audit` 的真实实现改断言。）文件顶部 `import http from 'node:http'` 若没有就加。

- [ ] **Step 2: 跑测试确认失败**

Run: `cd container/classroom && node --test server.test.mjs`
Expected: 两条新测试 FAIL。

- [ ] **Step 3: 实现**

`server.mjs`：
1. 构造函数接 `live` 选项并起自愈定时器：

```js
    // 公开直播：管理台自己的发布密钥 + 中转地址。缺一样就不开这个功能。
    this.publicLive = live && live.relay && live.publishKey ? {relay: live.relay.replace(/\/+$/, ''), key: live.publishKey} : null;
    if (this.publicLive) this.healTimer = setInterval(() => this.healPublic().catch(() => {}), 30000);
```

`close()` 里 `clearInterval(this.healTimer)`。
2. 错误映射与中转调用：

```js
const PUBLIC_ERRORS = {
  401: '发布密钥无效或已吊销',
  403: '服务器没有开启公开直播，或这场直播已被下线',
  413: '标题太长（最多 60 个字）',
  429: '操作太频繁，请一分钟后再试',
};
```

```js
  // 对中转公开 / 取消公开这一场。凭证由学生工作区的守护进程签发，发布密钥只在管理台进程里。
  async relayPublic(w, method, title) {
    const status = await this.liveStatus(w);
    if (!status) throw fail(409, '这个工作区现在没在直播');
    const id = new URL(status.url).pathname.split('/').pop();
    const answer = await this.liveRpc(w, 'LivePublishGrant');
    const grant = answer.LiveGrant;
    if (typeof grant !== 'string') throw fail(409, '这个工作区的 dct 版本还不支持，请先停止它再启动（会换成新镜像）');
    const headers = {'x-live-grant': grant};
    let body;
    if (method === 'PUT') { headers['content-type'] = 'application/json'; body = JSON.stringify({title: [...title].slice(0, 60).join(''), key: this.publicLive.key}); }
    let res;
    try { res = await fetch(`${this.publicLive.relay}/live/${encodeURIComponent(id)}/public`, {method, headers, body}); }
    catch { throw fail(409, '连不上直播中转，请稍后再试'); }
    if (res.status !== 204) throw Object.assign(fail(409, PUBLIC_ERRORS[res.status] || `直播中转拒绝了（状态码 ${res.status}）`), {relayStatus: res.status});
    return id;
  }

  // 后台自愈：记着要公开的工作区，中转说不公开就再公开一次；被吊销/下线就停。
  async healPublic() {
    if (!this.publicLive) return;
    let listed;
    try { const r = await fetch(`${this.publicLive.relay}/live/public`); listed = new Set((await r.json()).map(x => x.id)); }
    catch { return; }
    for (const w of this.store.data.students.filter(s => s.livePublic)) {
      await this.store.mutate(async () => {
        const status = await this.liveStatus(w).catch(() => null);
        if (!status) { delete w.livePublic; return; }
        const id = new URL(status.url).pathname.split('/').pop();
        if (listed.has(id)) return;
        try { await this.relayPublic(w, 'PUT', w.livePublic.title); delete w.livePublicError; }
        catch (e) {
          if (e.relayStatus === 401 || e.relayStatus === 403) {
            w.livePublicError = e.message;
            delete w.livePublic;
            this.store.audit(`公开直播被中转拒绝（${e.message}）`, w);
          }
        }
      });
    }
  }
```

（`fail(...)` 的返回值若不是 `Error` 子类，把 `Object.assign` 换成对应写法；`store.mutate` 的用法照文件里现有调用。）
3. 路由：在 `live-start` / `live-stop` 那一段之前加：

```js
      if (action === 'live-public' || action === 'live-private') {
        if (!this.publicLive) throw fail(404, '接口不存在');
        if (action === 'live-public') {
          const title = String(data.title || '').trim();
          if (!title) throw fail(400, '请填写公开标题');
          await this.relayPublic(w, 'PUT', title);
          w.livePublic = {title}; delete w.livePublicError;
          this.store.audit('公开直播工作区', w);
        } else {
          await this.relayPublic(w, 'DELETE');
          delete w.livePublic;
          this.store.audit('取消公开直播', w);
        }
        return json(res, 200, {live: await this.liveStatus(w)});
      }
```

这一段需要请求体：把 `await body(req);` 改成 `const data = await body(req);`（确认 `body()` 的返回值就是解析后的对象）。`live-start` 成功之后若 `data.public && this.publicLive` 就接着走一次 `live-public` 的逻辑（抽成一个小函数复用，失败时直播照常开、答复里带 `publicError`）。`live-stop` 成功后 `delete w.livePublic`。
4. `liveStatus` 的返回值加 `public: info.public && info.public.Listed ? {title: info.public.Listed.title} : null`；`safeRow` 加 `livePublicError: w.livePublicError || null`；`state()` 答复加 `features: {publicLive: !!this.publicLive}`（按 `state()` 现有返回结构放）。
5. 入口：

```js
  const publishKeyFile = process.env.CLASSROOM_LIVE_PUBLISH_KEY_FILE;
  const live = publishKeyFile ? {relay: process.env.CLASSROOM_LIVE_RELAY, publishKey: fs.readFileSync(publishKeyFile, 'utf8').trim()} : undefined;
```

传给 `new Classroom({..., live})`。

- [ ] **Step 4: 跑测试确认通过**

Run: `cd container/classroom && node --test server.test.mjs publishing.test.mjs sso.test.mjs`
Expected: PASS。

- [ ] **Step 5: 变异检查**

`healPublic` 里去掉 401/403 时的 `delete w.livePublic`（「被吊销之后又去撞了中转」红）；`relayPublic` 把 `x-live-grant` 改成空串（凭证断言红）。改回。

- [ ] **Step 6: Commit**

```bash
git add container/classroom/server.mjs container/classroom/server.test.mjs
git commit -m "feat(classroom): publish student lives through a daemon grant, heal after relay restarts"
```

---

### Task 10: 管理台界面

**Files:**
- Modify: `container/classroom/admin.html`

**Interfaces:**
- Consumes: Task 9 的 `features.publicLive`、`live.public`、`livePublicError`、`live-public` / `live-private` / `live-start {public,title}`。

- [ ] **Step 1: 改渲染与菜单**

`admin.html` 的脚本是压缩成一行的；用 `grep -o` 定位下面几个片段后替换（先 `cp admin.html /tmp/admin.html.bak` 便于比对）。

1. 状态文本：把

```js
(student.live?' · ● 直播中 · '+(student.live.viewers??0)+' 人在看':'')
```

换成

```js
(student.live?(student.live.public?' · ● 公开直播中 · '+student.live.public.title+' · ':' · ● 直播中 · ')+(student.live.viewers??0)+' 人在看':'')+(student.livePublicError?' · 公开失败：'+student.livePublicError:'')
```

（`metrics.textContent = ...` 本来就是 `textContent`，标题不会被当成 HTML。）
2. 在「复制直播链接」那一项之后加两项：

```js
if(features.publicLive&&student.live&&!student.live.public)menuAction('公开这场直播',async()=>{const title=prompt('公开标题（学生姓名和屏幕会出现在 live.dataclue.cn 的公开列表上，任何人都能看）',student.name+' 的工作区');if(title&&title.trim()){await api(route(student.id,'live-public'),{title:title.trim()});await loadState();message('已公开，live.dataclue.cn 上现在看得到这场直播。');}});if(features.publicLive&&student.live&&student.live.public)menuAction('取消公开',async()=>{await api(route(student.id,'live-private'),{});await loadState();message('已取消公开，私密链接照常能看。');},true);
```

3. 「直播这个工作区」的确认之后、调用 `live-start` 之前，加「同时公开」：把 `const r=await api(route(student.id,'live-start'),{});` 换成

```js
let extra={};if(features.publicLive&&confirm('同时公开这场直播？\n\n学生姓名和屏幕会出现在 live.dataclue.cn 的公开列表上，任何人都能看。选「取消」就只开私密直播。')){const title=prompt('公开标题',student.name+' 的工作区');if(title&&title.trim())extra={public:true,title:title.trim()};}const r=await api(route(student.id,'live-start'),extra);
```

4. `loadState` 里保存 `features`：在把 `students` 赋值的地方旁边加 `features=data.features||{};`，并在脚本顶部变量声明处加 `let features={};`（照现有 `students` 的声明方式）。

- [ ] **Step 2: 手工验收**

Run（仓库根）：`cd container/classroom && node --test server.test.mjs`，确认 Task 9 的测试仍然 PASS；另起一个本机管理台（按 `README.md`「验证」一节的方式，传入假的 `live` 选项）用浏览器点一遍：私密开播、同时公开、公开、取消公开、状态文字。**确认标题里写 `<b>x</b>` 时状态行显示的是原样文字。**

- [ ] **Step 3: Commit**

```bash
git add container/classroom/admin.html
git commit -m "feat(classroom): admin controls to publish and unpublish a live workspace"
```

---

### Task 11: 文档

**Files:**
- Modify: `docs/deploy-live-relay.md`
- Modify: `container/classroom/README.md`
- Modify: `README.md`、`README.zh-CN.md`（直播一节）
- Modify: `docs/superpowers/specs/2026-09-13-public-live-listing-design.md`（「设置页」那一句）

- [ ] **Step 1: `docs/deploy-live-relay.md`**

1. Caddy 配置块在 `handle /live/*` 之后加：

```caddyfile
	# 公开直播列表页。**精确匹配 `/`**，不是 `/*`——其余路径照旧落到下面的 404。
	handle / {
		reverse_proxy 127.0.0.1:8787
	}
```

2. 验收段落：`/` 从「必须 404」改为「必须 200（公开页）」；`/link/*` 仍须 404。
3. 新增一节「三、公开直播：发布密钥」：

```markdown
## 三、公开直播：发布密钥

中转默认没有公开功能。要开，先签发密钥，再带着密钥文件启动：

​```bash
dct-srv key add 管理台 --file /etc/dct-srv/publish-keys.json   # 打印一次密钥，交给管理台
dct-srv key add 姜老师 --file /etc/dct-srv/publish-keys.json   # 每位老师一把
chmod 600 /etc/dct-srv/publish-keys.json
​```

systemd 的 `ExecStart` 改成 `/usr/local/bin/dct-srv 127.0.0.1:8787 --publish-keys /etc/dct-srv/publish-keys.json`。

- 吊销：`dct-srv key revoke 姜老师 --file ...`，运行中的中转最多 10 秒生效，那把密钥公开的直播自动变回私密。
- 下线某一场：`dct-srv takedown <房间号> --file ...`，同样 10 秒内生效，停播之前公开不了。
- 查看：`dct-srv key list --file ...`。
- 文件写坏了：中转保留上一份并在日志里报错；首次启动就坏则拒绝启动。
- **回滚**：去掉 `--publish-keys` 重启，公开列表清空，私密直播不受影响。
```

（代码块围栏里的零宽字符是为了在本计划里嵌套显示，写进文档时去掉。）

- [ ] **Step 2: `container/classroom/README.md`**

在 `CLASSROOM_LIVE_RELAY` 那一段之后加：

```markdown
配置 `CLASSROOM_LIVE_PUBLISH_KEY_FILE=<文件>`（内容是运营方用 `dct-srv key add` 签给管理台的那把
密钥，权限 0600）后，管理台出现「公开这场直播 / 取消公开」和开播时的「同时公开」。公开由管理台
向学生工作区要一张只能切换公开状态的凭证、再带着自己的密钥去中转完成，密钥不进学生容器。
管理台每 30 秒检查一次，中转重启后自动重新公开；密钥被吊销或直播被下线时停止并在该行显示原因。
```

- [ ] **Step 3: 两份 README 的直播一节**

中文版在「屏幕上一直写着你在播」那条之后加：

```markdown
- **公开到固定地址**：面板里按 `p`，填一个标题，这场直播就出现在中转首页的公开列表上，谁都能看，
  不需要链接。第一次会让你填管理员发的「公开直播密钥」（以后按 `K` 可以换）。再按一次 `p` 取消
  公开，私密链接照常能用。公开期间底栏写的是「● 正在公开直播 · 标题」。
```

英文版对应：

```markdown
- **Publish to a fixed address**: press `p` in the panel and give it a title; the session appears on the
  relay's public list, watchable by anyone without a link. The first time you'll be asked for the public
  live key your operator issued (`K` changes it later). Press `p` again to unpublish; private links keep
  working. While public, the bar reads "● LIVE PUBLICLY · <title>".
```

- [ ] **Step 4: spec 同步偏离**

把 spec「### 设置页与密钥存放」第一条改为：

```markdown
- 直播面板里按 `K` 填 / 换公开直播密钥；按 `p` 公开时若守护进程报 `LivePublishKeyMissing`，
  直接转到填密钥，填完自动继续公开。存进密钥仓，保留键 `__live_publish__`（同 `__gate__`、`__web__`
  的做法）。（原设计写的是设置页，实施计划阶段改为就地填写，理由见计划「与 spec 的一处偏离」。）
```

- [ ] **Step 5: Commit**

```bash
git add docs container/classroom/README.md README.md README.zh-CN.md
git commit -m "docs: deploy, operate and use the public live listing"
```

---

## 收尾检查（全部 Task 完成后）

- [ ] `env -u TERM cargo test --workspace --locked --no-fail-fast` 全绿
- [ ] `cargo clippy --workspace --all-targets --locked -- -D warnings` 无警告
- [ ] `cd container/classroom && node --test server.test.mjs publishing.test.mjs sso.test.mjs` 全绿
- [ ] `git diff --check` 干净
- [ ] 跑 Task 5 的手工验收脚手架，浏览器确认：公开页列出房间、标题里的 `<script>` 原样显示成文字、点进去无令牌能看、取消公开后页面显示「这场直播已结束或不再公开」
- [ ] **CI 的 Windows 一格**：发版依赖它；若仍是红的，在最终报告里明确写出「本功能已完成但发不了版」
