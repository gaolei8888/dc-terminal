### Task 1: 新建 `dct-brain`，定义档位

**Files:**
- Modify: `Cargo.toml`（`[workspace] members`、`[dependencies]`、`[dev-dependencies]`）
- Create: `crates/dct-brain/Cargo.toml`
- Create: `crates/dct-brain/src/lib.rs`
- Create: `crates/dct-brain/src/tier.rs`

**Interfaces:**
- Produces: `dct_brain::tier::{Tier, SignerRole}`；`Tier::name(self) -> &'static str`；`Tier::required_signer(self) -> Option<SignerRole>`。`Tier` 的顺序（`Ord`）就是「谁更严」：`Read < SelfOnly < Physical < Content < Money`，后面所有「往高了调」都用 `max`。

- [ ] **Step 1: 建 crate 和 workspace 条目**

`Cargo.toml` 的 workspace 那一行改成：

```toml
[workspace]
members = ["crates/dct-link", "crates/dct-page", "crates/dct-srv", "crates/dct-brain"]
```

`[dependencies]` 里 `dct-page = ...` 下面加：

```toml
# 定档、执行票、签名核对。dct 和 dco 的安卓 App 共用，只写一份——见 crates/dct-brain 的模块说明。
dct-brain = { path = "crates/dct-brain" }
```

`[dev-dependencies]` 里加（测试要用软件钥匙冒充安全芯片）：

```toml
dct-brain = { path = "crates/dct-brain", features = ["soft-signer"] }
```

`crates/dct-brain/Cargo.toml`：

```toml
[package]
name = "dct-brain"
version = "0.1.0"
edition = "2021"
license = "MIT"

[features]
# 软件钥匙：只给测试用，冒充安全芯片。正式构建不打开。
soft-signer = []

[dependencies]
serde = { version = "1", features = ["derive"] }
serde_json = "1"
sha2 = "0.10"
base64 = "0.22"
# 纯 Rust 的 P-256 ECDSA，没有 C。安全芯片和手机 passkey 签出来的都是这条曲线。
p256 = { version = "0.13", default-features = false, features = ["ecdsa", "std"] }
```

`crates/dct-brain/src/lib.rs`：

```rust
//! dct 的「大脑」：定档、执行票、签名核对。dct 和 dco 的安卓 App 共用，只写一份，
//! 手机才能在电脑关着时自己判断、自己把关（设计：docs/superpowers/specs/2026-09-27-dct-brain-design.md）。
//!
//! 两条约束：
//! - 没有 C 依赖（dct 的老规矩，也方便安卓用 NDK 编）。
//! - 不直接碰磁盘和网络：钥匙怎么存、流程从哪来，都由调用方传进来。测试不打网络，
//!   同一份代码在手机上也能跑。
pub mod tier;
```

- [ ] **Step 2: 写失败的测试**

`crates/dct-brain/src/tier.rs`：

```rust
use serde::{Deserialize, Serialize};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stricter_tiers_sort_higher() {
        assert!(Tier::Read < Tier::SelfOnly);
        assert!(Tier::SelfOnly < Tier::Physical);
        assert!(Tier::Physical < Tier::Content);
        assert!(Tier::Content < Tier::Money);
    }

    #[test]
    fn names_match_dcv_and_dco() {
        for (t, n) in [
            (Tier::Read, "read"),
            (Tier::SelfOnly, "self"),
            (Tier::Physical, "physical"),
            (Tier::Content, "content"),
            (Tier::Money, "money"),
        ] {
            assert_eq!(t.name(), n);
            assert_eq!(serde_json::to_string(&t).unwrap(), format!("\"{n}\""));
            assert_eq!(serde_json::from_str::<Tier>(&format!("\"{n}\"")).unwrap(), t);
        }
    }

    #[test]
    fn who_signs_each_tier() {
        assert_eq!(Tier::Read.required_signer(), None);
        assert_eq!(Tier::SelfOnly.required_signer(), Some(SignerRole::Auto));
        // 物理动作全自动：用户定的「便利性第一位」。
        assert_eq!(Tier::Physical.required_signer(), Some(SignerRole::Auto));
        assert_eq!(Tier::Content.required_signer(), Some(SignerRole::User));
        assert_eq!(Tier::Money.required_signer(), Some(SignerRole::User));
    }
}
```

- [ ] **Step 3: 跑测试，确认失败**

Run: `cargo test -p dct-brain`
Expected: 编译失败，`Tier` / `SignerRole` 没有定义。

- [ ] **Step 4: 实现**

在 `tier.rs` 的 `use` 下面、`#[cfg(test)]` 上面加：

```rust
/// 一个动作能影响多大范围。**顺序就是「谁更严」**：取 `max` 就是往高了调，
/// 所有兜底都只能这么调，永远不往低了调。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum Tier {
    #[serde(rename = "read")]
    Read,
    #[serde(rename = "self")]
    SelfOnly,
    #[serde(rename = "physical")]
    Physical,
    #[serde(rename = "content")]
    Content,
    #[serde(rename = "money")]
    Money,
}

/// 这一档的票该由谁来签。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum SignerRole {
    /// 自动钥匙：不打扰用户。
    #[serde(rename = "auto")]
    Auto,
    /// 用户本人：Touch ID 或手机 passkey。
    #[serde(rename = "user")]
    User,
}

impl Tier {
    pub fn name(self) -> &'static str {
        match self {
            Tier::Read => "read",
            Tier::SelfOnly => "self",
            Tier::Physical => "physical",
            Tier::Content => "content",
            Tier::Money => "money",
        }
    }

    /// `None` 表示不要票（只读）。物理动作全自动是用户定的（设计第 4 节）。
    pub fn required_signer(self) -> Option<SignerRole> {
        match self {
            Tier::Read => None,
            Tier::SelfOnly | Tier::Physical => Some(SignerRole::Auto),
            Tier::Content | Tier::Money => Some(SignerRole::User),
        }
    }
}
```

- [ ] **Step 5: 跑测试，确认通过；检查整个 workspace**

Run: `cargo test -p dct-brain && cargo test --workspace -q 2>&1 | grep -E "test result|FAILED" && cargo check --target x86_64-pc-windows-msvc --all-targets -q`
Expected: 3 个新测试 PASS；workspace 全绿；Windows 检查能过。

- [ ] **Step 6: Commit**

```bash
git add Cargo.toml Cargo.lock crates/dct-brain
git commit -m "feat(brain): new dct-brain crate with the five tiers and who signs each"
```

---

