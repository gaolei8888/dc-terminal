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

