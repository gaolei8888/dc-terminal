# dct 大脑第一步：定档、执行票、Mac 钥匙 实施计划

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 建好共享库 `crates/dct-brain`（定档、`dcv-steps-v1` 指纹、执行票、签名和验签、流程批准记录），在 dct 里接上 Mac 安全芯片的两把钥匙，做出 `dct keys` 和 `dct procedure approve|ticket` 两组命令，让 dct 能为一条已确认的流程签出一张 dco 可以自己验证的执行票。

**Architecture:** 纯逻辑都放进新 crate `dct-brain`（没有 C 依赖、不碰网络和磁盘），dct 和以后 dco 的安卓 App 共用。Mac 安全芯片用 Swift（CryptoKit）写成静态库，由 dct 根包的 `build.rs` 只在 macOS 上编进来，实现 `dct_brain::sign::Signer`。dct 通过 `dcv show / approve --json` 取流程，用户按 Touch ID 批准每一步的档位，批准记录存在 `~/.dct/approvals/`，出票时核对。

**Tech Stack:** Rust 2021；`p256 0.13`（纯 Rust ECDSA P-256）、`sha2 0.10`、`base64 0.22`、`serde`/`serde_json`；Swift 5 + CryptoKit + LocalAuthentication（仅 macOS）。

**Spec:** `docs/superpowers/specs/2026-09-27-dct-brain-design.md`（用户已审）。本计划只做设计里「先做」的那一段：共享库骨架、按意图定档、执行票、Mac 上的两把钥匙。动作卡片、手机批准页、排程、从纠正里学、连 dco、「看数据」都不在本计划里。**判断模块（大模型 judge）也不在本计划里**：按 judge 设计，它的第一步是 `dct judge --probe` 验证网关给不给概率，而这台机器还没有 DC 网关密钥；它会单独出计划，代码直接写进 `dct-brain`。本计划的「按意图定档」先用规则（Task 3），以后由 judge 补上规则判断不了的情况。

## Global Constraints

- `crates/dct-brain` **没有 C 依赖**，**不直接读写磁盘、不连网络**；需要的东西由调用方传进来。
- 提交信息用英文，**不加任何 AI 署名 / Co-Authored-By 行**。
- `cargo` 在 `~/.cargo/bin`，不在默认 PATH：每条命令前先 `export PATH="$HOME/.cargo/bin:$PATH"`。
- 每个任务结束前：`cargo test --workspace` 全绿；`cargo clippy --workspace --all-targets -- -D warnings` 没有警告；`cargo check --target x86_64-pc-windows-msvc --all-targets` 能过（Windows 用户是主力，不能只在 Mac 上能编）。
- 仓库本来就不是 rustfmt 干净的：只对**自己改动的行**跑格式，不要顺手格式化别处。
- 档位名字（`read / self / physical / content / money`）、`sha256:<64 位小写十六进制>` 的写法，跟 dcv、dco 保持一致，不能改。
- `dcv-steps-v1` 的定义以 dcv 定稿为准（见 Task 2），一个字节都不能自己改。
- 只在自己的分支上做：`git switch -c feat/dct-brain-step1`，全部做完、测试全绿后再由主会话合并。

---

## File Structure

| 文件 | 职责 |
|---|---|
| `Cargo.toml`（改） | workspace 加 `crates/dct-brain`；dct 依赖它；dev-dependencies 打开 `soft-signer` |
| `crates/dct-brain/Cargo.toml`（新） | 新 crate |
| `crates/dct-brain/src/lib.rs`（新） | 模块清单和总说明 |
| `crates/dct-brain/src/canon.rs`（新） | 长度前缀字段编码、`sha256:` 指纹，三份规范共用 |
| `crates/dct-brain/src/steps.rs`（新） | `dcv-steps-v1` |
| `crates/dct-brain/src/tier.rs`（新） | 档位、谁来签、按意图定档、按钮文字只往高调 |
| `crates/dct-brain/src/ticket.rs`（新） | 执行票、参数和动作参数的指纹 |
| `crates/dct-brain/src/sign.rs`（新） | `Signer`、签名、批量签、验签；测试用的软件钥匙 |
| `crates/dct-brain/src/approval.rs`（新） | 流程批准记录：每步档位 + 用户签名 |
| `build.rs`（新） | 只在 macOS 上把 `swift/*.swift` 编成静态库链进 dct |
| `swift/Keys.swift`（新） | 安全芯片：建钥匙、取公钥、签名（Touch ID 在这里弹） |
| `src/keys/mod.rs`（新） | 钥匙目录、公钥文件、`SecureEnclave` 抽象、`dct keys` 命令 |
| `src/keys/mac.rs`（新） | Swift 函数的 Rust 绑定 |
| `src/procedures.rs`（新） | 调 dcv、批准流程、出票、`dct procedure` 命令 |
| `src/lib.rs`、`src/main.rs`（改） | 挂上两个新模块和两个子命令 |

---

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

### Task 2: `dcv-steps-v1`（流程步骤指纹）

**Files:**
- Create: `crates/dct-brain/src/canon.rs`
- Create: `crates/dct-brain/src/steps.rs`
- Modify: `crates/dct-brain/src/lib.rs`（加 `pub mod canon; pub mod steps;`）

**Interfaces:**
- Produces: `dct_brain::canon::{field(out: &mut Vec<u8>, s: &str), sha256_id(bytes: &[u8]) -> String}`；
  `dct_brain::steps::{Step { n: u32, action: String, arg: String, done_when: Option<String> }, steps_bytes(params: &[String], steps: &[Step]) -> Vec<u8>, steps_sha256(params: &[String], steps: &[Step]) -> String}`。

**定义（dcv 定稿，一字不改，dco 也按这份做）：**
- `field(s)` = s 的 UTF-8 字节数写成十进制 ASCII（不补零）+ `:` + s 的原始 UTF-8 字节。
- 整串 = `field("dcv-steps-v1")` + `field(params 个数)` + 按声明顺序每个 `field(参数名)` + `field(steps 个数)` + 每一步依次 `field(n)`、`field(action)`、`field(arg)`、`field("1" 或 "0")`（有没有完成标志）、`field(done_when 的 arg，没有就是空串)`。
- 输出 `sha256:` + 64 位小写十六进制。文本不做任何规范化；`{参数名}` 占位原样保留，不替换参数值。

- [ ] **Step 1: 写失败的测试**

`crates/dct-brain/src/steps.rs`：

```rust
#[cfg(test)]
mod tests {
    use super::*;

    fn step(n: u32, action: &str, arg: &str, done_when: Option<&str>) -> Step {
        Step {
            n,
            action: action.into(),
            arg: arg.into(),
            done_when: done_when.map(Into::into),
        }
    }

    // 三组用例来自 dcv（dc-vault 会话用独立的 Python 实现算出，dct 这边另算一遍核对过）。

    #[test]
    fn case_a_chinese_and_steps_without_done_marks() {
        let params = vec!["bio".to_string()];
        let steps = vec![
            step(1, "open_app", "TikTok", None),
            step(2, "navigate_by_intent", "我的 → 编辑资料 → 简介", None),
            step(3, "type_param", "bio", Some("简介栏显示 {bio}")),
            step(4, "tap_by_intent", "保存", None),
        ];
        assert_eq!(
            String::from_utf8(steps_bytes(&params, &steps)).unwrap(),
            "12:dcv-steps-v11:13:bio1:41:18:open_app6:TikTok1:00:1:218:navigate_by_intent34:我的 → 编辑资料 → 简介1:00:1:310:type_param3:bio1:121:简介栏显示 {bio}1:413:tap_by_intent6:保存1:00:"
        );
        assert_eq!(
            steps_sha256(&params, &steps),
            "sha256:4d1077af6efac1184e410d23b0b7b120fd42f29d7dade3304ef2041ca10b565c"
        );
    }

    #[test]
    fn case_b_no_params_and_an_empty_arg() {
        let steps = vec![step(1, "back", "", None)];
        assert_eq!(steps_bytes(&[], &steps), b"12:dcv-steps-v11:01:11:14:back0:1:00:".to_vec());
        assert_eq!(
            steps_sha256(&[], &steps),
            "sha256:8beaee6897d706732db3d29816cc0c00cc4bd24397eed6c5f9c12e6e079877db"
        );
    }

    #[test]
    fn case_c_two_params_and_fullwidth_brackets() {
        let params = vec!["photo".to_string(), "caption".to_string()];
        let steps = vec![
            step(1, "pick_file", "photo", Some("预览里出现「{photo}」")),
            step(2, "set_checked", "同时发到 Story = off", None),
        ];
        assert_eq!(
            steps_sha256(&params, &steps),
            "sha256:014dba248173199af5e3c250d123137720811f3bae1594a3a4a3a188579fc1c0"
        );
    }

    #[test]
    fn any_change_to_a_step_changes_the_fingerprint() {
        let base = vec![step(1, "tap_by_intent", "保存", None)];
        let h = steps_sha256(&[], &base);
        assert_ne!(h, steps_sha256(&[], &[step(1, "tap_by_intent", "发布", None)]));
        assert_ne!(h, steps_sha256(&[], &[step(2, "tap_by_intent", "保存", None)]));
        assert_ne!(h, steps_sha256(&[], &[step(1, "tap_by_intent", "保存", Some(""))]));
        assert_ne!(h, steps_sha256(&["x".to_string()], &base));
    }
}
```

- [ ] **Step 2: 跑测试，确认失败**

Run: `cargo test -p dct-brain steps`
Expected: 编译失败，`Step` / `steps_bytes` 没有定义。

- [ ] **Step 3: 实现**

`crates/dct-brain/src/canon.rs`：

```rust
//! 三份规范（流程步骤、执行票、批准记录）共用的编码：每个字段写成
//! 「UTF-8 字节数的十进制 ASCII + `:` + 原始字节」。这是 dcv 定的 dcv-steps-v1 的编码，
//! 执行票和批准记录沿用同一种，只是开头的版本串不同。带长度前缀，所以字段里有什么
//! 字符都不会跟相邻字段粘在一起被误读。
use sha2::{Digest, Sha256};

pub fn field(out: &mut Vec<u8>, s: &str) {
    out.extend_from_slice(s.len().to_string().as_bytes());
    out.push(b':');
    out.extend_from_slice(s.as_bytes());
}

/// `sha256:<64 位小写十六进制>`，跟 dcv 的 `body_sha256`、`approved_body` 同一种写法。
pub fn sha256_id(bytes: &[u8]) -> String {
    format!("sha256:{:x}", Sha256::digest(bytes))
}
```

在 `steps.rs` 顶部（测试模块上面）加：

```rust
//! `dcv-steps-v1`：执行票签的流程步骤指纹。定义以 dcv 定稿为准，dct、dco、dcv 三边
//! 必须算出一模一样的字节。**不做任何规范化**，拿到的就是 `dcv show --json` 里 steps
//! 各字段的原值；`{参数名}` 占位原样保留，不替换参数值。
use crate::canon::{field, sha256_id};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Step {
    pub n: u32,
    pub action: String,
    pub arg: String,
    /// 完成标志（`verify_text` 的参数）。没有就是 `None`。
    pub done_when: Option<String>,
}

pub fn steps_bytes(params: &[String], steps: &[Step]) -> Vec<u8> {
    let mut o = Vec::new();
    field(&mut o, "dcv-steps-v1");
    field(&mut o, &params.len().to_string());
    for p in params {
        field(&mut o, p);
    }
    field(&mut o, &steps.len().to_string());
    for s in steps {
        field(&mut o, &s.n.to_string());
        field(&mut o, &s.action);
        field(&mut o, &s.arg);
        // 没有完成标志时这一格照样写，写空串，不跳过（dcv 定稿）。
        field(&mut o, if s.done_when.is_some() { "1" } else { "0" });
        field(&mut o, s.done_when.as_deref().unwrap_or(""));
    }
    o
}

pub fn steps_sha256(params: &[String], steps: &[Step]) -> String {
    sha256_id(&steps_bytes(params, steps))
}
```

`lib.rs` 加 `pub mod canon;` 和 `pub mod steps;`。

- [ ] **Step 4: 跑测试，确认通过**

Run: `cargo test -p dct-brain`
Expected: 全部 PASS（含三组 dcv 用例）。

- [ ] **Step 5: Commit**

```bash
git add crates/dct-brain
git commit -m "feat(brain): dcv-steps-v1 step fingerprint, checked against dcv's three cases"
```

---

### Task 3: 按意图定档，按钮文字只往高了调

**Files:**
- Modify: `crates/dct-brain/src/tier.rs`

**Interfaces:**
- Consumes: Task 1 的 `Tier`。
- Produces（dct 批准流程时给提议，dco 执行前做兜底）：
  - `tier_for_step(action: &str, arg: &str) -> Tier`：**提议**这一步的档位，用户批准时可以改。
  - `tier_for_label(label: &str) -> Tier`：按钮文字**至少**意味着哪一档；说明不了什么时返回 `Tier::Read`（不往上调）。
  - `floor_for_element(label: &str, confirmed: bool) -> Tier`：没确认过的元素至少按 `Content`。
  - `raise(approved: Tier, label: &str, confirmed: bool) -> Tier` = `approved.max(floor_for_element(label, confirmed))`，**永远不会比 `approved` 低**。

设计依据（第 4 节「按意图定档」）：只看按钮字两头都错。「Next / Continue / OK / 下一步」这类字说不清是不是对外，提议时按对外算，交给用户在批准时改；「不保存 / Don't save / Not now」是否定，不算保存；没确认过的元素一律按最高档。

- [ ] **Step 1: 写失败的测试**

在 `tier.rs` 的 `mod tests` 里追加：

```rust
    #[test]
    fn reading_steps_are_read() {
        for a in ["verify_text", "wait", "read_value"] {
            assert_eq!(tier_for_step(a, "粉丝"), Tier::Read, "{a}");
        }
    }

    #[test]
    fn moving_around_and_filling_in_only_affect_yourself() {
        for (a, arg) in [
            ("open_app", "TikTok"),
            ("navigate_by_intent", "我的 → 编辑资料 → 简介"),
            ("scroll", "down"),
            ("back", ""),
            ("type_param", "bio"),
            ("set_checked", "同时发到 Story = off"),
            ("tap_by_intent", "编辑资料"),
            // 「Posts」标签页不是「post」：英文按整词比。
            ("tap_by_intent", "Posts"),
        ] {
            assert_eq!(tier_for_step(a, arg), Tier::SelfOnly, "{a} {arg}");
        }
    }

    #[test]
    fn outward_words_and_handing_over_files_are_content() {
        for (a, arg) in [
            ("tap_by_intent", "保存"),
            ("tap_by_intent", "立即发布"),
            ("tap_by_intent", "Post"),
            ("tap_by_intent", "Save"),
            ("pick_file", "photo"),
        ] {
            assert_eq!(tier_for_step(a, arg), Tier::Content, "{a} {arg}");
        }
    }

    #[test]
    fn ambiguous_last_buttons_are_proposed_as_content() {
        // 发布流程的最后一步可能只写着这些——提议时宁可严，用户批准时可以改低。
        for arg in ["Next", "Continue", "OK", "下一步", "继续", "确定"] {
            assert_eq!(tier_for_step("tap_by_intent", arg), Tier::Content, "{arg}");
        }
        assert_eq!(tier_for_step("confirm_dialog", "OK"), Tier::Content);
    }

    #[test]
    fn negations_are_not_the_thing_they_negate() {
        for arg in ["Don't save", "Don’t save", "不保存", "取消发布", "Not now", "Cancel", "暂不上传", "Discard post"] {
            assert_eq!(tier_for_step("tap_by_intent", arg), Tier::SelfOnly, "{arg}");
        }
        assert_eq!(tier_for_step("confirm_dialog", "Cancel"), Tier::SelfOnly);
    }

    #[test]
    fn money_words_are_money() {
        assert_eq!(tier_for_step("tap_by_intent", "立即支付"), Tier::Money);
        assert_eq!(tier_for_step("tap_by_intent", "Checkout"), Tier::Money);
    }

    #[test]
    fn unknown_actions_are_treated_as_outward() {
        assert_eq!(tier_for_step("delete_everything", ""), Tier::Content);
    }

    #[test]
    fn label_floor_only_speaks_when_the_text_says_something() {
        assert_eq!(tier_for_label("Save"), Tier::Content);
        assert_eq!(tier_for_label("Pay now"), Tier::Money);
        assert_eq!(tier_for_label(""), Tier::Read);
        assert_eq!(tier_for_label("Posts"), Tier::Read);
        assert_eq!(tier_for_label("Don't save"), Tier::Read);
        // 运行时的兜底不管「Next」：那是批准流程时的事，运行时再拦会重回「太严」。
        assert_eq!(tier_for_label("Next"), Tier::Read);
    }

    #[test]
    fn raising_never_lowers() {
        assert_eq!(raise(Tier::SelfOnly, "Post", true), Tier::Content);
        assert_eq!(raise(Tier::Content, "Cancel", true), Tier::Content);
        assert_eq!(raise(Tier::Money, "", true), Tier::Money);
        assert_eq!(raise(Tier::SelfOnly, "编辑资料", true), Tier::SelfOnly);
    }

    #[test]
    fn unconfirmed_elements_count_as_outward_at_least() {
        assert_eq!(raise(Tier::SelfOnly, "Next", false), Tier::Content);
        assert_eq!(raise(Tier::SelfOnly, "", false), Tier::Content);
        assert_eq!(raise(Tier::SelfOnly, "支付", false), Tier::Money);
        assert_eq!(floor_for_element("编辑资料", true), Tier::Read);
    }
```

- [ ] **Step 2: 跑测试，确认失败**

Run: `cargo test -p dct-brain tier`
Expected: 编译失败，`tier_for_step` 等没有定义。

- [ ] **Step 3: 实现**

在 `tier.rs` 的 `impl Tier` 下面加：

```rust
// 词表照抄 dco 设计第 2 节，只许增加，不许删。中文按子串比（「立即发布」），
// 英文按整词比（「Posts」标签页不是「post」）。
const CONTENT_ZH: &[&str] = &["发布", "发表", "发送", "分享", "转发", "上传", "保存", "提交", "确认", "完成"];
const CONTENT_EN: &[&str] = &["post", "publish", "send", "share", "repost", "upload", "save", "submit", "confirm", "done"];
const MONEY_ZH: &[&str] = &["支付", "购买", "下单", "充值", "付款"];
const MONEY_EN: &[&str] = &["pay", "buy", "order", "checkout", "purchase"];
// 发布流程的最后一步常常只写这些。单看字说不清是不是对外，**提议**时按对外算，
// 用户批准时可以改低。运行时的兜底不看这张表，否则又回到「太严」。
const AMBIGUOUS_ZH: &[&str] = &["下一步", "继续", "确定", "好的"];
const AMBIGUOUS_EN: &[&str] = &["next", "continue", "ok", "okay", "yes", "allow"];
// 否定：「不保存」「Don't save」不是保存。中文只认开头，免得「保存不了」被当成否定；
// 英文只认第一个词，免得「Save for later」被当成否定。
const NEGATION_ZH: &[&str] = &["不", "取消", "暂不", "以后再说", "稍后", "放弃", "别"];
const NEGATION_EN: &[&str] = &["don't", "dont", "not", "cancel", "no", "skip", "later", "discard"];

fn words(label: &str) -> Vec<String> {
    label
        .replace('’', "'")
        .to_lowercase()
        .split(|c: char| !(c.is_ascii_alphanumeric() || c == '\''))
        .filter(|w| !w.is_empty())
        .map(String::from)
        .collect()
}

fn hit(label: &str, zh: &[&str], en: &[&str]) -> bool {
    zh.iter().any(|z| label.contains(z)) || words(label).iter().any(|w| en.contains(&w.as_str()))
}

fn negated(label: &str) -> bool {
    let t = label.trim_start();
    NEGATION_ZH.iter().any(|n| t.starts_with(n))
        || words(t).first().is_some_and(|w| NEGATION_EN.contains(&w.as_str()))
}

/// 按钮上的字**至少**意味着哪一档。说明不了什么（空、否定、普通词）就是 `Read`，
/// 也就是不往上调。
pub fn tier_for_label(label: &str) -> Tier {
    let label = label.trim();
    if label.is_empty() || negated(label) {
        return Tier::Read;
    }
    if hit(label, MONEY_ZH, MONEY_EN) {
        Tier::Money
    } else if hit(label, CONTENT_ZH, CONTENT_EN) {
        Tier::Content
    } else {
        Tier::Read
    }
}

/// 没确认过的元素（模型第一次认出来的图标、按钮）至少按对外算（设计第 4 节）。
pub fn floor_for_element(label: &str, confirmed: bool) -> Tier {
    let by_text = tier_for_label(label);
    if confirmed {
        by_text
    } else {
        by_text.max(Tier::Content)
    }
}

/// 兜底：只往高了调，永远不比 `approved` 低。
pub fn raise(approved: Tier, label: &str, confirmed: bool) -> Tier {
    approved.max(floor_for_element(label, confirmed))
}

/// 批准流程时给每一步**提议**的档位；用户确认或改过之后才生效。
pub fn tier_for_step(action: &str, arg: &str) -> Tier {
    match action {
        "verify_text" | "wait" | "read_value" => Tier::Read,
        "open_app" | "navigate_by_intent" | "scroll" | "back" | "type_param" | "set_checked" => {
            Tier::SelfOnly
        }
        // 选文件就是把本机文件交出去（dcv 会话提醒的）。
        "pick_file" => Tier::Content,
        "tap_by_intent" | "confirm_dialog" => {
            let a = arg.trim();
            if negated(a) {
                return Tier::SelfOnly;
            }
            let default = if action == "confirm_dialog" || hit(a, AMBIGUOUS_ZH, AMBIGUOUS_EN) {
                Tier::Content
            } else {
                Tier::SelfOnly
            };
            tier_for_label(a).max(default)
        }
        // dcv 写入时就拒绝词表外的动作；万一漏进来，按对外算。
        _ => Tier::Content,
    }
}
```

- [ ] **Step 4: 跑测试，确认通过**

Run: `cargo test -p dct-brain`
Expected: 全部 PASS。

- [ ] **Step 5: Commit**

```bash
git add crates/dct-brain/src/tier.rs
git commit -m "feat(brain): propose tiers from each step's intent; button text only raises"
```

---

### Task 4: 执行票

**Files:**
- Create: `crates/dct-brain/src/ticket.rs`
- Modify: `crates/dct-brain/src/lib.rs`（加 `pub mod ticket;`）

**Interfaces:**
- Consumes: `canon::{field, sha256_id}`、`tier::Tier`。
- Produces: `ticket::{TICKET_VERSION: u32 = 1, Subject, Ticket, params_sha256(&BTreeMap<String, String>) -> String, args_sha256(&serde_json::Value) -> String, canonical_json(&serde_json::Value) -> String}`；`Ticket::canonical_bytes(&self) -> Vec<u8>`、`Ticket::digest(&self) -> String`。
  - `Subject::Procedure { name, steps_sha256, body_sha256 }`（name 写全，如 `social:edit-bio`）/ `Subject::Action { tool, args_sha256 }`。JSON 里用 `"kind": "procedure" | "action"` 区分。
  - `Ticket { v, nonce, device, tier, subject, params_sha256, earliest, expires }`；`nonce` 是 32 位小写十六进制（调用方用系统随机数生成）；`earliest`、`expires` 是 Unix 秒，有效窗口是 `[earliest, expires)`。

**规范（本任务定稿，dco 照这份验）：**
- 票：`field("dct-ticket-v1")`、`field(v)`、`field(nonce)`、`field(device)`、`field(tier 名)`，然后流程写 `field("procedure")`、`field(name)`、`field(steps_sha256)`、`field(body_sha256)`，单个动作写 `field("action")`、`field(tool)`、`field(args_sha256)`；最后 `field(params_sha256)`、`field(earliest)`、`field(expires)`。数字都写十进制。
- 参数指纹：`field("dct-params-v1")`、`field(个数)`，然后**按键名排序**逐个 `field(键)`、`field(值)`。
- 动作参数指纹：`field("dct-args-v1")`、`field(canonical_json(args))`；`canonical_json` 是键名排序、没有空白、非 ASCII 原样输出的紧凑 JSON。

- [ ] **Step 1: 写失败的测试**

`crates/dct-brain/src/ticket.rs`：

```rust
#[cfg(test)]
mod tests {
    use super::*;

    fn bio_params() -> BTreeMap<String, String> {
        BTreeMap::from([("bio".to_string(), "Founder, Learning Tech.".to_string())])
    }

    fn procedure_ticket() -> Ticket {
        Ticket {
            v: TICKET_VERSION,
            nonce: "0123456789abcdef0123456789abcdef".into(),
            device: "mac-lei".into(),
            tier: Tier::Content,
            subject: Subject::Procedure {
                name: "social:edit-bio".into(),
                steps_sha256: "sha256:4d1077af6efac1184e410d23b0b7b120fd42f29d7dade3304ef2041ca10b565c".into(),
                body_sha256: format!("sha256:{}", "ab".repeat(32)),
            },
            params_sha256: params_sha256(&bio_params()),
            earliest: 1_790_000_000,
            expires: 1_790_007_200,
        }
    }

    // 期望值用一份独立的 Python 实现算出（计划作者核对过），dco 也按这些值验。

    #[test]
    fn params_fingerprint_is_stable() {
        assert_eq!(
            params_sha256(&bio_params()),
            "sha256:aeaac10a352f12e708de6e82ceed0ce52da7fd089575058d734f0c0fadd6d730"
        );
        assert_eq!(
            params_sha256(&BTreeMap::new()),
            "sha256:3847df4681bdecc3230accbeb09e3461f858f0f2e733fd77e1cdceba6b9bcbdc"
        );
    }

    #[test]
    fn procedure_ticket_digest_is_stable() {
        let t = procedure_ticket();
        assert!(t.canonical_bytes().starts_with(b"13:dct-ticket-v11:132:0123456789abcdef0123456789abcdef7:mac-"));
        assert_eq!(
            t.digest(),
            "sha256:2215e2abe4c81f86b8c35d7be809bd6559d6a90eff5c758974fc5a1308c612c1"
        );
    }

    #[test]
    fn action_ticket_digest_is_stable() {
        let args = serde_json::json!({"text": "hi", "element": 3});
        assert_eq!(canonical_json(&args), r#"{"element":3,"text":"hi"}"#);
        let t = Ticket {
            v: TICKET_VERSION,
            nonce: "fedcba9876543210fedcba9876543210".into(),
            device: "mac-lei".into(),
            tier: Tier::SelfOnly,
            subject: Subject::Action {
                tool: "tap".into(),
                args_sha256: args_sha256(&args),
            },
            params_sha256: params_sha256(&BTreeMap::new()),
            earliest: 1_790_000_000,
            expires: 1_790_000_300,
        };
        assert_eq!(
            args_sha256(&args),
            "sha256:a065f016beb00a4c119463a9da8bd9b0dc87416af021de5a82e13a6d400fe573"
        );
        assert_eq!(
            t.digest(),
            "sha256:4ee473f5dececb43e8b583cf200002bfc42f8290e8701914907c033f63d14490"
        );
    }

    #[test]
    fn every_field_is_in_the_digest() {
        let base = procedure_ticket();
        let d = base.digest();
        let mut changed = Vec::new();
        let mut t = base.clone(); t.nonce = "1".repeat(32); changed.push(t);
        let mut t = base.clone(); t.device = "phone".into(); changed.push(t);
        let mut t = base.clone(); t.tier = Tier::SelfOnly; changed.push(t);
        let mut t = base.clone(); t.params_sha256 = params_sha256(&BTreeMap::new()); changed.push(t);
        let mut t = base.clone(); t.earliest += 1; changed.push(t);
        let mut t = base.clone(); t.expires += 1; changed.push(t);
        let mut t = base.clone();
        if let Subject::Procedure { body_sha256, .. } = &mut t.subject { *body_sha256 = "sha256:x".into(); }
        changed.push(t);
        for c in changed {
            assert_ne!(c.digest(), d, "{c:?}");
        }
    }

    #[test]
    fn tickets_round_trip_through_json() {
        let t = procedure_ticket();
        let j = serde_json::to_value(&t).unwrap();
        assert_eq!(j["subject"]["kind"], "procedure");
        assert_eq!(j["tier"], "content");
        assert_eq!(serde_json::from_value::<Ticket>(j).unwrap(), t);
    }
}
```

- [ ] **Step 2: 跑测试，确认失败**

Run: `cargo test -p dct-brain ticket`
Expected: 编译失败，`Ticket` 等没有定义。

- [ ] **Step 3: 实现**

在 `ticket.rs` 顶部加：

```rust
//! 执行票：dct 签、dco 验。「除了看以外每一步都要票」（设计第 2 节）。
//! 编码沿用 dcv-steps-v1 的长度前缀字段（见 `canon`），开头的版本串不同。
use crate::canon::{field, sha256_id};
use crate::tier::Tier;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

pub const TICKET_VERSION: u32 = 1;

/// 这张票准许做什么。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Subject {
    /// 一整条流程。`name` 写全（`库:名字`）；签 `steps_sha256`，`body_sha256` 留作审计。
    Procedure {
        name: String,
        steps_sha256: String,
        body_sha256: String,
    },
    /// 单个动作（dco 的一个 MCP 工具 + 它的参数）。
    Action { tool: String, args_sha256: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Ticket {
    pub v: u32,
    /// 32 位小写十六进制，一次性；dco 记住用过的，同一张票不能用第二次。
    pub nonce: String,
    /// 在哪台设备上执行（dco 名片里的设备名）。
    pub device: String,
    pub tier: Tier,
    pub subject: Subject,
    pub params_sha256: String,
    /// 有效窗口 `[earliest, expires)`，Unix 秒。「提前批准，到点执行」靠它。
    pub earliest: u64,
    pub expires: u64,
}

impl Ticket {
    pub fn canonical_bytes(&self) -> Vec<u8> {
        let mut o = Vec::new();
        field(&mut o, "dct-ticket-v1");
        field(&mut o, &self.v.to_string());
        field(&mut o, &self.nonce);
        field(&mut o, &self.device);
        field(&mut o, self.tier.name());
        match &self.subject {
            Subject::Procedure { name, steps_sha256, body_sha256 } => {
                field(&mut o, "procedure");
                field(&mut o, name);
                field(&mut o, steps_sha256);
                field(&mut o, body_sha256);
            }
            Subject::Action { tool, args_sha256 } => {
                field(&mut o, "action");
                field(&mut o, tool);
                field(&mut o, args_sha256);
            }
        }
        field(&mut o, &self.params_sha256);
        field(&mut o, &self.earliest.to_string());
        field(&mut o, &self.expires.to_string());
        o
    }

    pub fn digest(&self) -> String {
        sha256_id(&self.canonical_bytes())
    }
}

/// 参数值的指纹：按键名排序，值原样。参数值只在运行时传，不写进流程，但要签进票。
pub fn params_sha256(params: &BTreeMap<String, String>) -> String {
    let mut o = Vec::new();
    field(&mut o, "dct-params-v1");
    field(&mut o, &params.len().to_string());
    for (k, v) in params {
        field(&mut o, k);
        field(&mut o, v);
    }
    sha256_id(&o)
}

pub fn args_sha256(args: &serde_json::Value) -> String {
    let mut o = Vec::new();
    field(&mut o, "dct-args-v1");
    field(&mut o, &canonical_json(args));
    sha256_id(&o)
}

/// 键名排序、没有空白、非 ASCII 原样输出的紧凑 JSON。自己排序，不依赖 serde_json
/// 有没有打开 `preserve_order`——两边编出来的顺序不一样，指纹就对不上。
pub fn canonical_json(v: &serde_json::Value) -> String {
    use serde_json::Value;
    match v {
        Value::Object(m) => {
            let mut keys: Vec<&String> = m.keys().collect();
            keys.sort();
            let parts: Vec<String> = keys
                .into_iter()
                .map(|k| format!("{}:{}", serde_json::to_string(k).unwrap(), canonical_json(&m[k])))
                .collect();
            format!("{{{}}}", parts.join(","))
        }
        Value::Array(a) => format!("[{}]", a.iter().map(canonical_json).collect::<Vec<_>>().join(",")),
        other => serde_json::to_string(other).unwrap(),
    }
}
```

`lib.rs` 加 `pub mod ticket;`。

- [ ] **Step 4: 跑测试，确认通过**

Run: `cargo test -p dct-brain`
Expected: 全部 PASS。

- [ ] **Step 5: Commit**

```bash
git add crates/dct-brain
git commit -m "feat(brain): execution tickets with a length-prefixed canonical form"
```

---

### Task 5: 签名、批量签、验签

**Files:**
- Create: `crates/dct-brain/src/sign.rs`
- Modify: `crates/dct-brain/src/lib.rs`（加 `pub mod sign;`）

**Interfaces:**
- Consumes: `canon`、`ticket::{Ticket, TICKET_VERSION}`、`tier::{Tier, SignerRole}`。
- Produces:
  - `trait Signer { fn role(&self) -> SignerRole; fn public_key(&self) -> [u8; 65]; fn sign(&self, msg: &[u8], reason: &str) -> Result<[u8; 64], SignError>; }`：对 `msg` 签名（内部先做 SHA-256），返回 64 字节 `r||s`；`public_key` 是 SEC1 未压缩格式（65 字节，`0x04` 开头）。CryptoKit 的 `x963Representation` / `rawRepresentation` 正好是这两种格式。
  - `enum SignError { Unavailable, Cancelled, Other(String) }`（实现 `Display` 和 `std::error::Error`）。
  - `key_id(&[u8; 65]) -> String`（公钥的 `sha256:` 指纹）。
  - `struct Signature { key_id, alg /* "es256" */, sig /* base64 标准编码的 r||s */ }`。
  - `struct SignedTicket { ticket: Ticket, signature: Signature, batch: Option<Vec<String>> }`。
  - `struct TrustedKey { role: SignerRole, public_key: [u8; 65] }`。
  - `enum VerifyError { Unsupported, UnknownKey, WrongRole { need: SignerRole }, BadSignature, NotInBatch, WrongDevice, TooEarly, Expired, StepsChanged }`（`StepsChanged` 给 Task 6 用）。
  - `batch_bytes(&[String]) -> Vec<u8>`、`sign_one(Ticket, &dyn Signer, &str) -> Result<SignedTicket, SignError>`、`sign_batch(Vec<Ticket>, &dyn Signer, &str) -> Result<Vec<SignedTicket>, SignError>`、`verify_sig(&Signature, &[u8], &[TrustedKey]) -> Result<SignerRole, VerifyError>`、`verify(&SignedTicket, &[TrustedKey], device: &str, now: u64) -> Result<(), VerifyError>`。
  - `soft::SoftSigner`（只在 `test` 或 `soft-signer` 特性下有）：`SoftSigner::from_seed(role, seed: u8)`、`.trusted() -> TrustedKey`。
- **一次性编号不在这里查**：`verify` 通过后，由调用方（dco）记住 `nonce`，直到票过期。

**批量签（设计第 3 节）：** 签的是 `field("dct-batch-v1") + field(个数) + 每个 field(票的 digest)`；每张票都带着整个 `batch` 列表，验的时候先确认自己的 digest 在列表里，再对列表验签。

**谁能签什么：** 票的档位 `required_signer()` 是 `None` 或 `Auto` 时，用户钥匙和自动钥匙都行；是 `User` 时，只认用户钥匙。

- [ ] **Step 1: 写失败的测试**

`crates/dct-brain/src/sign.rs`：

```rust
#[cfg(test)]
mod tests {
    use super::soft::SoftSigner;
    use super::*;
    use crate::ticket::{params_sha256, Subject, TICKET_VERSION};
    use std::collections::BTreeMap;

    fn ticket(tier: Tier, nonce_byte: char) -> Ticket {
        Ticket {
            v: TICKET_VERSION,
            nonce: nonce_byte.to_string().repeat(32),
            device: "mac-lei".into(),
            tier,
            subject: Subject::Procedure {
                name: "social:edit-bio".into(),
                steps_sha256: "sha256:s".into(),
                body_sha256: "sha256:b".into(),
            },
            params_sha256: params_sha256(&BTreeMap::new()),
            earliest: 100,
            expires: 200,
        }
    }

    fn keys() -> (SoftSigner, SoftSigner, Vec<TrustedKey>) {
        let auto = SoftSigner::from_seed(SignerRole::Auto, 1);
        let user = SoftSigner::from_seed(SignerRole::User, 2);
        let trusted = vec![auto.trusted(), user.trusted()];
        (auto, user, trusted)
    }

    #[test]
    fn a_self_ticket_signed_by_the_automatic_key_verifies() {
        let (auto, _, trusted) = keys();
        let st = sign_one(ticket(Tier::SelfOnly, 'a'), &auto, "").unwrap();
        assert_eq!(verify(&st, &trusted, "mac-lei", 150), Ok(()));
    }

    #[test]
    fn an_outward_ticket_needs_the_users_key() {
        let (auto, user, trusted) = keys();
        let by_auto = sign_one(ticket(Tier::Content, 'a'), &auto, "").unwrap();
        assert_eq!(
            verify(&by_auto, &trusted, "mac-lei", 150),
            Err(VerifyError::WrongRole { need: SignerRole::User })
        );
        let by_user = sign_one(ticket(Tier::Content, 'a'), &user, "发到 TikTok").unwrap();
        assert_eq!(verify(&by_user, &trusted, "mac-lei", 150), Ok(()));
        // 用户钥匙什么档都能签。
        let self_by_user = sign_one(ticket(Tier::SelfOnly, 'b'), &user, "").unwrap();
        assert_eq!(verify(&self_by_user, &trusted, "mac-lei", 150), Ok(()));
    }

    #[test]
    fn any_tampering_breaks_the_signature() {
        let (_, user, trusted) = keys();
        let mut st = sign_one(ticket(Tier::Content, 'a'), &user, "").unwrap();
        st.ticket.tier = Tier::SelfOnly;
        assert_eq!(verify(&st, &trusted, "mac-lei", 150), Err(VerifyError::BadSignature));
    }

    #[test]
    fn keys_not_paired_are_refused() {
        let (_, user, _) = keys();
        let stranger = SoftSigner::from_seed(SignerRole::User, 9);
        let st = sign_one(ticket(Tier::Content, 'a'), &stranger, "").unwrap();
        assert_eq!(verify(&st, &[user.trusted()], "mac-lei", 150), Err(VerifyError::UnknownKey));
    }

    #[test]
    fn device_and_time_window_are_enforced() {
        let (auto, _, trusted) = keys();
        let st = sign_one(ticket(Tier::SelfOnly, 'a'), &auto, "").unwrap();
        assert_eq!(verify(&st, &trusted, "phone", 150), Err(VerifyError::WrongDevice));
        assert_eq!(verify(&st, &trusted, "mac-lei", 99), Err(VerifyError::TooEarly));
        assert_eq!(verify(&st, &trusted, "mac-lei", 100), Ok(()));
        assert_eq!(verify(&st, &trusted, "mac-lei", 200), Err(VerifyError::Expired));
    }

    #[test]
    fn one_signature_covers_a_whole_batch() {
        let (_, user, trusted) = keys();
        let batch = sign_batch(vec![ticket(Tier::Content, 'a'), ticket(Tier::Content, 'b')], &user, "5 个平台").unwrap();
        assert_eq!(batch.len(), 2);
        assert_eq!(batch[0].signature, batch[1].signature);
        for st in &batch {
            assert_eq!(verify(st, &trusted, "mac-lei", 150), Ok(()));
        }
        // 一张不在这批里的票，拿这批的签名冒充不行。
        let mut forged = batch[0].clone();
        forged.ticket = ticket(Tier::Money, 'c');
        assert_eq!(verify(&forged, &trusted, "mac-lei", 150), Err(VerifyError::NotInBatch));
    }

    #[test]
    fn batch_bytes_are_stable() {
        let d = vec![
            "sha256:2215e2abe4c81f86b8c35d7be809bd6559d6a90eff5c758974fc5a1308c612c1".to_string(),
            "sha256:4ee473f5dececb43e8b583cf200002bfc42f8290e8701914907c033f63d14490".to_string(),
        ];
        assert_eq!(
            crate::canon::sha256_id(&batch_bytes(&d)),
            "sha256:5a95c7bf18b94dbc8d93bfc0c86018ed7a84f30565df37aee83a3f0748256c35"
        );
    }

    #[test]
    fn signed_tickets_round_trip_through_json() {
        let (auto, _, trusted) = keys();
        let st = sign_one(ticket(Tier::SelfOnly, 'a'), &auto, "").unwrap();
        let back: SignedTicket = serde_json::from_str(&serde_json::to_string(&st).unwrap()).unwrap();
        assert_eq!(verify(&back, &trusted, "mac-lei", 150), Ok(()));
        assert!(!serde_json::to_string(&st).unwrap().contains("batch"));
    }
}
```

- [ ] **Step 2: 跑测试，确认失败**

Run: `cargo test -p dct-brain sign`
Expected: 编译失败。

- [ ] **Step 3: 实现**

在 `sign.rs` 顶部加：

```rust
//! 签名和验签。算法是 ECDSA P-256 + SHA-256（「es256」）：Mac 安全芯片、手机 passkey
//! 签出来的都是它，纯 Rust 实现，没有 C。
//!
//! 验签只回答「这张票是不是配对过的钥匙签的、档位够不够、设备和时间对不对」。
//! **一次性编号不在这里查**：通过之后由调用方（dco）记住 nonce，直到票过期。
use crate::canon::{field, sha256_id};
use crate::ticket::{Ticket, TICKET_VERSION};
use crate::tier::{SignerRole, Tier};
use base64::{engine::general_purpose::STANDARD, Engine as _};
use p256::ecdsa::{signature::Verifier as _, Signature as EcSig, VerifyingKey};
use serde::{Deserialize, Serialize};

pub const ALG: &str = "es256";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SignError {
    /// 这台机器没有安全芯片 / 钥匙还没建。
    Unavailable,
    /// 用户取消了 Touch ID，或者没通过。
    Cancelled,
    Other(String),
}

impl std::fmt::Display for SignError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SignError::Unavailable => write!(f, "这台电脑上用不了安全芯片签名"),
            SignError::Cancelled => write!(f, "没有通过指纹确认"),
            SignError::Other(m) => write!(f, "签名失败：{m}"),
        }
    }
}

impl std::error::Error for SignError {}

pub trait Signer {
    fn role(&self) -> SignerRole;
    /// SEC1 未压缩公钥，65 字节，`0x04` 开头。
    fn public_key(&self) -> [u8; 65];
    /// 对 `msg` 签名（内部先做 SHA-256），返回 64 字节 `r||s`。
    /// `reason` 是给用户看的那句人话，Touch ID 弹窗里显示它；自动钥匙忽略。
    fn sign(&self, msg: &[u8], reason: &str) -> Result<[u8; 64], SignError>;
}

pub fn key_id(public_key: &[u8; 65]) -> String {
    sha256_id(public_key)
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Signature {
    pub key_id: String,
    pub alg: String,
    /// base64（标准编码）的 64 字节 `r||s`。
    pub sig: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SignedTicket {
    pub ticket: Ticket,
    pub signature: Signature,
    /// 批量签时这一批所有票的 digest；单张签时没有。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub batch: Option<Vec<String>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrustedKey {
    pub role: SignerRole,
    pub public_key: [u8; 65],
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VerifyError {
    Unsupported,
    UnknownKey,
    WrongRole { need: SignerRole },
    BadSignature,
    NotInBatch,
    WrongDevice,
    TooEarly,
    Expired,
    /// 批准记录对应的流程步骤已经变了（Task 6）。
    StepsChanged,
}

impl std::fmt::Display for VerifyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            VerifyError::Unsupported => "票的版本或算法不认识",
            VerifyError::UnknownKey => "签名的钥匙没配对过",
            VerifyError::WrongRole { .. } => "这一档要用户本人签名",
            VerifyError::BadSignature => "签名不对",
            VerifyError::NotInBatch => "这张票不在签过的那一批里",
            VerifyError::WrongDevice => "这张票不是给这台设备的",
            VerifyError::TooEarly => "还没到可以执行的时间",
            VerifyError::Expired => "票已经过期",
            VerifyError::StepsChanged => "流程的步骤改过了，要重新批准",
        };
        f.write_str(s)
    }
}

impl std::error::Error for VerifyError {}

pub fn batch_bytes(digests: &[String]) -> Vec<u8> {
    let mut o = Vec::new();
    field(&mut o, "dct-batch-v1");
    field(&mut o, &digests.len().to_string());
    for d in digests {
        field(&mut o, d);
    }
    o
}

pub(crate) fn make_signature(s: &dyn Signer, msg: &[u8], reason: &str) -> Result<Signature, SignError> {
    let raw = s.sign(msg, reason)?;
    Ok(Signature {
        key_id: key_id(&s.public_key()),
        alg: ALG.into(),
        sig: STANDARD.encode(raw),
    })
}

pub fn sign_one(ticket: Ticket, s: &dyn Signer, reason: &str) -> Result<SignedTicket, SignError> {
    let signature = make_signature(s, &ticket.canonical_bytes(), reason)?;
    Ok(SignedTicket { ticket, signature, batch: None })
}

/// 一次签一批：同一条视频发 5 个平台，只按 1 次指纹（设计第 3 节）。
pub fn sign_batch(tickets: Vec<Ticket>, s: &dyn Signer, reason: &str) -> Result<Vec<SignedTicket>, SignError> {
    let digests: Vec<String> = tickets.iter().map(Ticket::digest).collect();
    let signature = make_signature(s, &batch_bytes(&digests), reason)?;
    Ok(tickets
        .into_iter()
        .map(|ticket| SignedTicket {
            ticket,
            signature: signature.clone(),
            batch: Some(digests.clone()),
        })
        .collect())
}

/// 验 `msg` 上的签名，返回签名钥匙的角色。
pub fn verify_sig(sig: &Signature, msg: &[u8], trusted: &[TrustedKey]) -> Result<SignerRole, VerifyError> {
    if sig.alg != ALG {
        return Err(VerifyError::Unsupported);
    }
    let key = trusted
        .iter()
        .find(|k| key_id(&k.public_key) == sig.key_id)
        .ok_or(VerifyError::UnknownKey)?;
    let raw = STANDARD.decode(&sig.sig).map_err(|_| VerifyError::BadSignature)?;
    let es = EcSig::from_slice(&raw).map_err(|_| VerifyError::BadSignature)?;
    let vk = VerifyingKey::from_sec1_bytes(&key.public_key).map_err(|_| VerifyError::BadSignature)?;
    vk.verify(msg, &es).map_err(|_| VerifyError::BadSignature)?;
    Ok(key.role)
}

fn role_may_sign(tier: Tier, role: SignerRole) -> bool {
    match tier.required_signer() {
        None | Some(SignerRole::Auto) => true,
        Some(SignerRole::User) => role == SignerRole::User,
    }
}

pub fn verify(st: &SignedTicket, trusted: &[TrustedKey], device: &str, now: u64) -> Result<(), VerifyError> {
    let t = &st.ticket;
    if t.v != TICKET_VERSION {
        return Err(VerifyError::Unsupported);
    }
    if t.device != device {
        return Err(VerifyError::WrongDevice);
    }
    if now < t.earliest {
        return Err(VerifyError::TooEarly);
    }
    if now >= t.expires {
        return Err(VerifyError::Expired);
    }
    let msg = match &st.batch {
        None => t.canonical_bytes(),
        Some(list) => {
            if !list.contains(&t.digest()) {
                return Err(VerifyError::NotInBatch);
            }
            batch_bytes(list)
        }
    };
    let role = verify_sig(&st.signature, &msg, trusted)?;
    if !role_may_sign(t.tier, role) {
        return Err(VerifyError::WrongRole { need: SignerRole::User });
    }
    Ok(())
}

/// 软件钥匙：只给测试用，冒充安全芯片。正式构建里没有这个模块。
#[cfg(any(test, feature = "soft-signer"))]
pub mod soft {
    use super::*;
    use p256::ecdsa::{signature::Signer as _, SigningKey};

    pub struct SoftSigner {
        role: SignerRole,
        key: SigningKey,
    }

    impl SoftSigner {
        /// 固定的私钥，测试结果可复现。不同 `seed` 得到不同的钥匙。
        pub fn from_seed(role: SignerRole, seed: u8) -> Self {
            let mut b = [0x11u8; 32];
            b[31] = seed.max(1);
            SoftSigner {
                role,
                key: SigningKey::from_slice(&b).expect("valid scalar"),
            }
        }

        pub fn trusted(&self) -> TrustedKey {
            TrustedKey {
                role: self.role,
                public_key: self.public_key(),
            }
        }
    }

    impl Signer for SoftSigner {
        fn role(&self) -> SignerRole {
            self.role
        }

        fn public_key(&self) -> [u8; 65] {
            let p = self.key.verifying_key().to_encoded_point(false);
            let mut a = [0u8; 65];
            a.copy_from_slice(p.as_bytes());
            a
        }

        fn sign(&self, msg: &[u8], _reason: &str) -> Result<[u8; 64], SignError> {
            let s: EcSig = self.key.sign(msg);
            let mut a = [0u8; 64];
            a.copy_from_slice(&s.to_bytes());
            Ok(a)
        }
    }
}
```

`lib.rs` 加 `pub mod sign;`。如果 `p256 0.13` 的某个方法名跟上面不一样（例如 `from_slice`、`to_encoded_point`），以 docs.rs 上 `p256 0.13` / `ecdsa 0.16` 的文档为准改调用，**不要换库、不要加带 C 的依赖**。

- [ ] **Step 4: 跑测试，确认通过；确认没有引入 C**

Run: `cargo test -p dct-brain && cargo tree -p dct-brain -e normal | grep -iE "\bcc v|openssl|ring v|-sys v" ; echo "exit=$?"`
Expected: 测试全部 PASS；`cargo tree` 那一段什么都没打印（`exit=1`），也就是依赖树里没有 C。

- [ ] **Step 5: Commit**

```bash
git add crates/dct-brain Cargo.lock
git commit -m "feat(brain): sign and verify tickets with P-256, one signature for a batch"
```

---

### Task 6: 流程批准记录

**Files:**
- Create: `crates/dct-brain/src/approval.rs`
- Modify: `crates/dct-brain/src/lib.rs`（加 `pub mod approval;`）

**Interfaces:**
- Consumes: `canon`、`sign::{Signer, Signature, TrustedKey, VerifyError, SignError, verify_sig, make_signature}`、`tier::{Tier, SignerRole}`。
- Produces: `approval::{APPROVAL_VERSION: u32 = 1, Approval, SignedApproval, sign_approval, verify_approval}`：
  - `Approval { v, procedure /* 库:名字 */, steps_sha256, body_sha256, step_tiers: Vec<Tier>, approved_at: u64 }`；`Approval::canonical_bytes(&self) -> Vec<u8>`；`Approval::run_tier(&self) -> Tier`（每步档位的最大值，空流程为 `Read`）。
  - `SignedApproval { approval: Approval, signature: Signature }`。
  - `sign_approval(Approval, &dyn Signer, reason: &str) -> Result<SignedApproval, SignError>`：**只接受用户钥匙**，其他角色返回 `SignError::Other`。
  - `verify_approval(&SignedApproval, &[TrustedKey], steps_sha256: &str) -> Result<(), VerifyError>`：版本不对 → `Unsupported`；步骤指纹对不上 → `StepsChanged`；不是用户钥匙 → `WrongRole`。

设计依据（第 4 节）：用户批准流程时同时批准每一步的档位；档位随批准记录保存；**批准记录必须带用户签名**，否则 agent 改一下本机文件，就能把「发布」那一步改成「只影响自己」。

**规范：** `field("dct-approval-v1")`、`field(v)`、`field(procedure)`、`field(steps_sha256)`、`field(body_sha256)`、`field(档位个数)`、每步 `field(档位名)`、`field(approved_at)`。

- [ ] **Step 1: 写失败的测试**

`crates/dct-brain/src/approval.rs`：

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::sign::soft::SoftSigner;

    const STEPS: &str = "sha256:4d1077af6efac1184e410d23b0b7b120fd42f29d7dade3304ef2041ca10b565c";

    fn approval() -> Approval {
        Approval {
            v: APPROVAL_VERSION,
            procedure: "social:edit-bio".into(),
            steps_sha256: STEPS.into(),
            body_sha256: format!("sha256:{}", "ab".repeat(32)),
            step_tiers: vec![Tier::SelfOnly, Tier::SelfOnly, Tier::SelfOnly, Tier::Content],
            approved_at: 1_790_000_000,
        }
    }

    #[test]
    fn canonical_form_is_stable() {
        assert_eq!(
            crate::canon::sha256_id(&approval().canonical_bytes()),
            "sha256:4ba1f58585f9df3f798cc0178ea13e16f9c351aa0fc6adcc72a89ac17867432a"
        );
    }

    #[test]
    fn the_run_needs_the_strictest_steps_signer() {
        assert_eq!(approval().run_tier(), Tier::Content);
        let mut a = approval();
        a.step_tiers = vec![];
        assert_eq!(a.run_tier(), Tier::Read);
    }

    #[test]
    fn only_the_user_can_approve() {
        let auto = SoftSigner::from_seed(SignerRole::Auto, 1);
        assert!(sign_approval(approval(), &auto, "").is_err());
        let user = SoftSigner::from_seed(SignerRole::User, 2);
        let sa = sign_approval(approval(), &user, "批准").unwrap();
        assert_eq!(verify_approval(&sa, &[user.trusted()], STEPS), Ok(()));
    }

    #[test]
    fn editing_the_record_or_the_steps_voids_it() {
        let user = SoftSigner::from_seed(SignerRole::User, 2);
        let sa = sign_approval(approval(), &user, "").unwrap();
        let mut lowered = sa.clone();
        lowered.approval.step_tiers[3] = Tier::SelfOnly;
        assert_eq!(verify_approval(&lowered, &[user.trusted()], STEPS), Err(VerifyError::BadSignature));
        assert_eq!(verify_approval(&sa, &[user.trusted()], "sha256:other"), Err(VerifyError::StepsChanged));
    }

    #[test]
    fn an_automatic_key_signature_is_not_an_approval() {
        // 就算有人绕过 sign_approval、直接用自动钥匙签了批准记录，验的时候也不认。
        let auto = SoftSigner::from_seed(SignerRole::Auto, 1);
        let sig = crate::sign::make_signature(&auto, &approval().canonical_bytes(), "").unwrap();
        let sa = SignedApproval { approval: approval(), signature: sig };
        assert_eq!(
            verify_approval(&sa, &[auto.trusted()], STEPS),
            Err(VerifyError::WrongRole { need: SignerRole::User })
        );
    }
}
```

- [ ] **Step 2: 跑测试，确认失败**

Run: `cargo test -p dct-brain approval`
Expected: 编译失败。

- [ ] **Step 3: 实现**

在 `approval.rs` 顶部加：

```rust
//! 流程批准记录：用户确认一条流程时，同时确认每一步的档位（设计第 4 节）。
//! 流程文件本身不写档位（dcv 定的），档位随这份记录保存；记录带用户签名，
//! 本机文件被人改过就不再认。
use crate::canon::field;
use crate::sign::{make_signature, verify_sig, SignError, Signature, Signer, TrustedKey, VerifyError};
use crate::tier::{SignerRole, Tier};
use serde::{Deserialize, Serialize};

pub const APPROVAL_VERSION: u32 = 1;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Approval {
    pub v: u32,
    /// `库:名字`
    pub procedure: String,
    pub steps_sha256: String,
    pub body_sha256: String,
    /// 跟流程步骤一一对应，按 n 的顺序。
    pub step_tiers: Vec<Tier>,
    pub approved_at: u64,
}

impl Approval {
    pub fn canonical_bytes(&self) -> Vec<u8> {
        let mut o = Vec::new();
        field(&mut o, "dct-approval-v1");
        field(&mut o, &self.v.to_string());
        field(&mut o, &self.procedure);
        field(&mut o, &self.steps_sha256);
        field(&mut o, &self.body_sha256);
        field(&mut o, &self.step_tiers.len().to_string());
        for t in &self.step_tiers {
            field(&mut o, t.name());
        }
        field(&mut o, &self.approved_at.to_string());
        o
    }

    /// 整条流程的票按最严的那一步来签。
    pub fn run_tier(&self) -> Tier {
        self.step_tiers.iter().copied().max().unwrap_or(Tier::Read)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SignedApproval {
    pub approval: Approval,
    pub signature: Signature,
}

pub fn sign_approval(a: Approval, s: &dyn Signer, reason: &str) -> Result<SignedApproval, SignError> {
    if s.role() != SignerRole::User {
        return Err(SignError::Other("批准流程必须用用户本人的钥匙".into()));
    }
    let signature = make_signature(s, &a.canonical_bytes(), reason)?;
    Ok(SignedApproval { approval: a, signature })
}

pub fn verify_approval(sa: &SignedApproval, trusted: &[TrustedKey], steps_sha256: &str) -> Result<(), VerifyError> {
    if sa.approval.v != APPROVAL_VERSION {
        return Err(VerifyError::Unsupported);
    }
    if sa.approval.steps_sha256 != steps_sha256 {
        return Err(VerifyError::StepsChanged);
    }
    let role = verify_sig(&sa.signature, &sa.approval.canonical_bytes(), trusted)?;
    if role != SignerRole::User {
        return Err(VerifyError::WrongRole { need: SignerRole::User });
    }
    Ok(())
}
```

`lib.rs` 加 `pub mod approval;`。

- [ ] **Step 4: 跑测试，确认通过**

Run: `cargo test -p dct-brain`
Expected: 全部 PASS。

- [ ] **Step 5: Commit**

```bash
git add crates/dct-brain
git commit -m "feat(brain): signed procedure approvals that carry each step's tier"
```

---

### Task 7: Mac 安全芯片的两把钥匙，`dct keys`

**Files:**
- Create: `build.rs`
- Create: `swift/Keys.swift`
- Create: `src/keys/mod.rs`
- Create: `src/keys/mac.rs`
- Modify: `src/lib.rs`（加 `pub mod keys;`）
- Modify: `src/main.rs`（加 `Some("keys")` 分支）

**Interfaces:**
- Consumes: `dct_brain::sign::{Signer, SignError, TrustedKey, key_id}`、`dct_brain::tier::SignerRole`、`crate::sys::fs::create_private`、`crate::proto::socket_path`。
- Produces:
  - `keys::SecureEnclave` trait：`available(&self) -> bool`、`create(&self, biometric: bool) -> Result<(Vec<u8>, [u8; 65]), SignError>`、`sign(&self, blob: &[u8], msg: &[u8], reason: &str) -> Result<[u8; 64], SignError>`。
  - `keys::platform_enclave() -> Box<dyn SecureEnclave>`：macOS 上是真的安全芯片，其他平台 `available()` 为 `false`、其余方法返回 `SignError::Unavailable`。
  - `keys::KeyStore`：`KeyStore::at(dir: PathBuf)`、`KeyStore::default_dir() -> PathBuf`（`~/.dct/keys`，跟 `daemon.sock` 同一个目录下）、`init(&self, se: &dyn SecureEnclave) -> anyhow::Result<PublicKeys>`（已有就原样返回，不重建）、`public_keys(&self) -> anyhow::Result<Option<PublicKeys>>`、`trusted(&self) -> anyhow::Result<Vec<TrustedKey>>`、`signer<'a>(&self, role: SignerRole, se: &'a dyn SecureEnclave) -> anyhow::Result<EnclaveSigner<'a>>`。
  - `keys::PublicKeys { auto: PublicEntry, user: PublicEntry }`，`PublicEntry { key_id: String, public_key: String /* base64 SEC1 */ }`；存在 `~/.dct/keys/public.json`（公钥，不是秘密，普通写入）。
  - `keys::EnclaveSigner<'a>`：实现 `dct_brain::sign::Signer`。
  - `keys::run_cli(args: &[String]) -> i32`：`dct keys init | show | test`。

**钥匙怎么存：** 用 CryptoKit 的 `SecureEnclave.P256.Signing.PrivateKey` 建钥匙，把它的 `dataRepresentation`（安全芯片加密过的「钥匙把手」，**只有这台 Mac 的安全芯片能用，拿走没用**）存成 `~/.dct/keys/auto.se`、`user.se`，用 `sys::fs::create_private` 写（只有属主能读）。用户钥匙建的时候加 `.biometryCurrentSet`：**每次签名都要当场按 Touch ID**，换过指纹就作废。

**⚠️ 跟设计的一处差距（要在交付时告诉用户）：** 设计里写「自动钥匙只有签过名的 dct 程序本身能用，agent 拿不到」。要做到这一点，钥匙得放进带访问组的钥匙串，这要求 dct 用 Apple 开发者证书签名、带 provisioning profile。这一步本计划不做：自动钥匙的把手文件只受文件权限保护，同一个系统账号下的 agent 如果去读它，可以自己签出 `self` / `physical` 档的票。**用户钥匙不受影响**：签名必须当场按 Touch ID，对外和动钱的票伪造不了。后续有开发者证书时，把两个把手换进钥匙串访问组即可，格式不变。

**CI 说明：** GitHub 的 macOS 机器是虚拟机，没有安全芯片。所以涉及真实安全芯片的只做手动验证（`dct keys test`），自动测试一律用假的 `SecureEnclave`（包一层 `SoftSigner`）。

- [ ] **Step 1: 写失败的测试**

`src/keys/mod.rs` 里先放测试（实现写在它上面）：

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use dct_brain::sign::soft::SoftSigner;
    use dct_brain::sign::Signer;
    use std::cell::RefCell;

    /// 假的安全芯片：「把手」就是一个字节的种子，签名用软件钥匙。
    struct FakeEnclave {
        next_seed: RefCell<u8>,
        cancel_user: bool,
    }

    impl FakeEnclave {
        fn new() -> Self {
            FakeEnclave { next_seed: RefCell::new(1), cancel_user: false }
        }
        fn soft(blob: &[u8]) -> SoftSigner {
            let role = if blob[1] == 1 { SignerRole::User } else { SignerRole::Auto };
            SoftSigner::from_seed(role, blob[0])
        }
    }

    impl SecureEnclave for FakeEnclave {
        fn available(&self) -> bool {
            true
        }
        fn create(&self, biometric: bool) -> Result<(Vec<u8>, [u8; 65]), SignError> {
            let mut s = self.next_seed.borrow_mut();
            let blob = vec![*s, biometric as u8];
            *s += 1;
            Ok((blob.clone(), Self::soft(&blob).public_key()))
        }
        fn sign(&self, blob: &[u8], msg: &[u8], reason: &str) -> Result<[u8; 64], SignError> {
            if blob[1] == 1 && self.cancel_user {
                return Err(SignError::Cancelled);
            }
            Self::soft(blob).sign(msg, reason)
        }
    }

    #[test]
    fn init_creates_both_keys_once() {
        let d = tempfile::tempdir().unwrap();
        let ks = KeyStore::at(d.path().join("keys"));
        let se = FakeEnclave::new();
        let first = ks.init(&se).unwrap();
        assert_ne!(first.auto.key_id, first.user.key_id);
        assert!(d.path().join("keys/auto.se").exists());
        assert!(d.path().join("keys/user.se").exists());
        // 第二次不重建：公钥不变，否则配对过的 dco 全都不认了。
        let second = ks.init(&se).unwrap();
        assert_eq!(first, second);
    }

    #[test]
    fn signers_sign_with_the_right_role_and_verify_against_the_public_file() {
        let d = tempfile::tempdir().unwrap();
        let ks = KeyStore::at(d.path().join("keys"));
        let se = FakeEnclave::new();
        ks.init(&se).unwrap();
        let trusted = ks.trusted().unwrap();
        for role in [SignerRole::Auto, SignerRole::User] {
            let s = ks.signer(role, &se).unwrap();
            assert_eq!(s.role(), role);
            let sig = dct_brain::sign::make_signature_for_tests(&s, b"hello");
            assert_eq!(dct_brain::sign::verify_sig(&sig, b"hello", &trusted), Ok(role));
        }
    }

    #[test]
    fn a_cancelled_touch_id_is_reported_as_cancelled() {
        let d = tempfile::tempdir().unwrap();
        let ks = KeyStore::at(d.path().join("keys"));
        let mut se = FakeEnclave::new();
        ks.init(&se).unwrap();
        se.cancel_user = true;
        let s = ks.signer(SignerRole::User, &se).unwrap();
        assert_eq!(s.sign(b"x", "发到 TikTok"), Err(SignError::Cancelled));
    }

    #[test]
    fn no_keys_yet_means_no_signer() {
        let d = tempfile::tempdir().unwrap();
        let ks = KeyStore::at(d.path().join("keys"));
        assert!(ks.public_keys().unwrap().is_none());
        assert!(ks.signer(SignerRole::Auto, &FakeEnclave::new()).is_err());
    }
}
```

上面用到的 `make_signature_for_tests` 在 `dct-brain` 里补一个（`make_signature` 是 `pub(crate)`，dct 用不到）。在 `crates/dct-brain/src/sign.rs` 的 `soft` 模块**外面**加：

```rust
/// 测试用：对任意消息签一个 `Signature`，好在 crate 外面验证 `Signer` 的实现。
#[cfg(any(test, feature = "soft-signer"))]
pub fn make_signature_for_tests(s: &dyn Signer, msg: &[u8]) -> Signature {
    make_signature(s, msg, "").expect("sign")
}
```

- [ ] **Step 2: 跑测试，确认失败**

Run: `cargo test --lib keys::`
Expected: 编译失败，`KeyStore` 等没有定义。

- [ ] **Step 3: 写 Swift 和 build.rs**

`swift/Keys.swift`：

```swift
// dct 的两把钥匙放在 Mac 的安全芯片里（设计：docs/superpowers/specs/2026-09-27-dct-brain-design.md 第 2 节）。
// 私钥永远不出安全芯片；我们拿到的 dataRepresentation 是它加密过的「把手」，只有这台 Mac 能用。
// 状态码：0 成功，1 没有安全芯片，2 把手不对，3 用户没通过指纹 / 取消，4 缓冲区太小，5 其它错误。
import CryptoKit
import Foundation
import LocalAuthentication
import Security

@_cdecl("dct_se_available")
public func dct_se_available() -> Bool {
    SecureEnclave.isAvailable
}

@_cdecl("dct_se_create")
public func dct_se_create(
    _ biometric: Bool,
    _ blobOut: UnsafeMutablePointer<UInt8>, _ blobCap: Int, _ blobLen: UnsafeMutablePointer<Int>,
    _ pubOut: UnsafeMutablePointer<UInt8>
) -> Int32 {
    guard SecureEnclave.isAvailable else { return 1 }
    var flags: SecAccessControlCreateFlags = [.privateKeyUsage]
    if biometric { flags.insert(.biometryCurrentSet) }
    var err: Unmanaged<CFError>?
    guard let ac = SecAccessControlCreateWithFlags(nil, kSecAttrAccessibleWhenUnlockedThisDeviceOnly, flags, &err) else {
        return 5
    }
    do {
        let key = try SecureEnclave.P256.Signing.PrivateKey(accessControl: ac)
        let blob = key.dataRepresentation
        guard blob.count <= blobCap else { return 4 }
        blob.copyBytes(to: blobOut, count: blob.count)
        blobLen.pointee = blob.count
        let pub = key.publicKey.x963Representation  // 65 字节，0x04 开头
        guard pub.count == 65 else { return 5 }
        pub.copyBytes(to: pubOut, count: 65)
        return 0
    } catch {
        return 5
    }
}

@_cdecl("dct_se_sign")
public func dct_se_sign(
    _ blob: UnsafePointer<UInt8>, _ blobLen: Int,
    _ msg: UnsafePointer<UInt8>, _ msgLen: Int,
    _ reason: UnsafePointer<CChar>,
    _ sigOut: UnsafeMutablePointer<UInt8>
) -> Int32 {
    guard SecureEnclave.isAvailable else { return 1 }
    let ctx = LAContext()
    ctx.localizedReason = String(cString: reason)  // Touch ID 弹窗里显示的那句人话
    let key: SecureEnclave.P256.Signing.PrivateKey
    do {
        key = try SecureEnclave.P256.Signing.PrivateKey(
            dataRepresentation: Data(bytes: blob, count: blobLen), authenticationContext: ctx)
    } catch {
        return 2
    }
    do {
        let sig = try key.signature(for: Data(bytes: msg, count: msgLen))
        sig.rawRepresentation.copyBytes(to: sigOut, count: 64)  // r||s
        return 0
    } catch {
        return 3
    }
}
```

`build.rs`（照 dc-octo 的做法，只在 macOS 上编，别的平台直接返回）：

```rust
//! 只在 macOS 上把 swift/*.swift 编成静态库链进 dct：安全芯片只能从 CryptoKit 用。
//! 别的平台什么都不做——Windows、Linux 的构建不需要 Swift，也不该需要。
use std::{env, path::PathBuf, process::Command};

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    if env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("macos") {
        return;
    }
    let out = PathBuf::from(env::var("OUT_DIR").unwrap());
    let arch = match env::var("CARGO_CFG_TARGET_ARCH").unwrap().as_str() {
        "aarch64" => "arm64",
        "x86_64" => "x86_64",
        a => panic!("unsupported arch {a}"),
    };
    let mut sources: Vec<PathBuf> = std::fs::read_dir("swift")
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().is_some_and(|e| e == "swift"))
        .collect();
    sources.sort();
    println!("cargo:rerun-if-changed=swift");
    for s in &sources {
        println!("cargo:rerun-if-changed={}", s.display());
    }
    let lib = out.join("libDctMac.a");
    let status = Command::new("xcrun")
        .args(["swiftc", "-parse-as-library", "-emit-library", "-static", "-module-name", "DctMac", "-swift-version", "5", "-O"])
        .args(["-target", &format!("{arch}-apple-macos13.0"), "-o"])
        .arg(&lib)
        .args(&sources)
        .status()
        .expect("run xcrun swiftc");
    assert!(status.success(), "swiftc failed");
    println!("cargo:rustc-link-search=native={}", out.display());
    println!("cargo:rustc-link-lib=static=DctMac");
    let sdk = Command::new("xcrun").args(["--show-sdk-path"]).output().expect("xcrun --show-sdk-path").stdout;
    println!("cargo:rustc-link-search=native={}/usr/lib/swift", String::from_utf8(sdk).unwrap().trim());
    println!("cargo:rustc-link-arg=-Wl,-rpath,/usr/lib/swift");
    for f in ["CryptoKit", "Foundation", "LocalAuthentication", "Security"] {
        println!("cargo:rustc-link-lib=framework={f}");
    }
}
```

- [ ] **Step 4: 写 Rust 实现**

`src/keys/mac.rs`：

```rust
//! swift/Keys.swift 的绑定。Swift 的 `Int` 是 64 位有符号，对应 `isize`。
use dct_brain::sign::SignError;
use std::ffi::CString;
use std::os::raw::c_char;

extern "C" {
    fn dct_se_available() -> bool;
    fn dct_se_create(biometric: bool, blob_out: *mut u8, blob_cap: isize, blob_len: *mut isize, pub_out: *mut u8) -> i32;
    fn dct_se_sign(blob: *const u8, blob_len: isize, msg: *const u8, msg_len: isize, reason: *const c_char, sig_out: *mut u8) -> i32;
}

fn status(code: i32) -> SignError {
    match code {
        1 => SignError::Unavailable,
        3 => SignError::Cancelled,
        2 => SignError::Other("钥匙文件不对，可能是从别的电脑拷来的".into()),
        4 => SignError::Other("钥匙太大".into()),
        c => SignError::Other(format!("安全芯片返回 {c}")),
    }
}

pub struct MacEnclave;

impl super::SecureEnclave for MacEnclave {
    fn available(&self) -> bool {
        unsafe { dct_se_available() }
    }

    fn create(&self, biometric: bool) -> Result<(Vec<u8>, [u8; 65]), SignError> {
        let mut blob = vec![0u8; 1024];
        let mut len: isize = 0;
        let mut public = [0u8; 65];
        let rc = unsafe { dct_se_create(biometric, blob.as_mut_ptr(), blob.len() as isize, &mut len, public.as_mut_ptr()) };
        if rc != 0 {
            return Err(status(rc));
        }
        blob.truncate(len as usize);
        Ok((blob, public))
    }

    fn sign(&self, blob: &[u8], msg: &[u8], reason: &str) -> Result<[u8; 64], SignError> {
        // 人话里不该有 NUL；有就去掉，别让整次签名失败。
        let reason = CString::new(reason.replace('\0', "")).unwrap();
        let mut sig = [0u8; 64];
        let rc = unsafe {
            dct_se_sign(blob.as_ptr(), blob.len() as isize, msg.as_ptr(), msg.len() as isize, reason.as_ptr(), sig.as_mut_ptr())
        };
        if rc != 0 {
            return Err(status(rc));
        }
        Ok(sig)
    }
}
```

`src/keys/mod.rs`（放在 Step 1 的测试模块上面）：

```rust
//! dct 的两把钥匙（设计第 2 节）：自动钥匙签 `self` / `physical`，用户钥匙签
//! `content` / `money`，每次都要当场按 Touch ID。私钥在 Mac 安全芯片里，磁盘上只有
//! 安全芯片加密过的「把手」（只有这台 Mac 能用）和公钥。
use anyhow::{anyhow, Context, Result};
use base64::{engine::general_purpose::STANDARD, Engine as _};
use dct_brain::sign::{key_id, SignError, Signer, TrustedKey};
use dct_brain::tier::SignerRole;
use serde::{Deserialize, Serialize};
use std::io::Write;
use std::path::PathBuf;

#[cfg(target_os = "macos")]
mod mac;

/// 安全芯片的三件事。抽出来是为了测试能换成假的——CI 的 Mac 是虚拟机，没有安全芯片。
pub trait SecureEnclave {
    fn available(&self) -> bool;
    /// 建一把新钥匙，返回（把手，SEC1 公钥）。`biometric` 为真时每次签名都要按 Touch ID。
    fn create(&self, biometric: bool) -> Result<(Vec<u8>, [u8; 65]), SignError>;
    fn sign(&self, blob: &[u8], msg: &[u8], reason: &str) -> Result<[u8; 64], SignError>;
}

struct NoEnclave;

impl SecureEnclave for NoEnclave {
    fn available(&self) -> bool {
        false
    }
    fn create(&self, _: bool) -> Result<(Vec<u8>, [u8; 65]), SignError> {
        Err(SignError::Unavailable)
    }
    fn sign(&self, _: &[u8], _: &[u8], _: &str) -> Result<[u8; 64], SignError> {
        Err(SignError::Unavailable)
    }
}

/// 这台机器上的安全芯片。没有 passkey 的平台上，对外和动钱两档直接关闭，
/// 不退化成「点一下同意」（控制层设计）。
pub fn platform_enclave() -> Box<dyn SecureEnclave> {
    #[cfg(target_os = "macos")]
    {
        Box::new(mac::MacEnclave)
    }
    #[cfg(not(target_os = "macos"))]
    {
        Box::new(NoEnclave)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PublicEntry {
    pub key_id: String,
    /// base64 的 SEC1 未压缩公钥。
    pub public_key: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PublicKeys {
    pub auto: PublicEntry,
    pub user: PublicEntry,
}

pub struct KeyStore {
    dir: PathBuf,
}

fn entry(public: &[u8; 65]) -> PublicEntry {
    PublicEntry { key_id: key_id(public), public_key: STANDARD.encode(public) }
}

fn decode(e: &PublicEntry) -> Result<[u8; 65]> {
    let raw = STANDARD.decode(&e.public_key).context("公钥文件坏了")?;
    raw.try_into().map_err(|_| anyhow!("公钥长度不对"))
}

impl KeyStore {
    pub fn at(dir: PathBuf) -> Self {
        KeyStore { dir }
    }

    /// `~/.dct/keys`，跟 daemon.sock 同一个目录下。
    pub fn default_dir() -> PathBuf {
        crate::proto::socket_path()
            .parent()
            .map(|p| p.join("keys"))
            .unwrap_or_else(|| PathBuf::from("keys"))
    }

    fn blob_path(&self, role: SignerRole) -> PathBuf {
        self.dir.join(match role {
            SignerRole::Auto => "auto.se",
            SignerRole::User => "user.se",
        })
    }

    fn public_path(&self) -> PathBuf {
        self.dir.join("public.json")
    }

    pub fn public_keys(&self) -> Result<Option<PublicKeys>> {
        match std::fs::read(self.public_path()) {
            Ok(b) => Ok(Some(serde_json::from_slice(&b).context("公钥文件坏了")?)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    /// 已经有钥匙就原样返回，**不重建**：重建会换公钥，配对过的 dco 就全都不认了。
    pub fn init(&self, se: &dyn SecureEnclave) -> Result<PublicKeys> {
        if let Some(k) = self.public_keys()? {
            return Ok(k);
        }
        if !se.available() {
            return Err(anyhow!("这台电脑没有安全芯片，没法建钥匙"));
        }
        std::fs::create_dir_all(&self.dir)?;
        let mut made = Vec::new();
        for (role, biometric) in [(SignerRole::Auto, false), (SignerRole::User, true)] {
            let (blob, public) = se.create(biometric).map_err(|e| anyhow!("{e}"))?;
            let mut f = crate::sys::fs::create_private(&self.blob_path(role))?;
            f.write_all(&blob)?;
            made.push(entry(&public));
        }
        let keys = PublicKeys { user: made.pop().unwrap(), auto: made.pop().unwrap() };
        std::fs::write(self.public_path(), serde_json::to_vec_pretty(&keys)?)?;
        Ok(keys)
    }

    /// 两把公钥，给验签用（批准记录、执行票；以后配对 dco 时也交给它）。
    pub fn trusted(&self) -> Result<Vec<TrustedKey>> {
        let k = self.public_keys()?.ok_or_else(|| anyhow!("还没建钥匙，先运行 dct keys init"))?;
        Ok(vec![
            TrustedKey { role: SignerRole::Auto, public_key: decode(&k.auto)? },
            TrustedKey { role: SignerRole::User, public_key: decode(&k.user)? },
        ])
    }

    pub fn signer<'a>(&self, role: SignerRole, se: &'a dyn SecureEnclave) -> Result<EnclaveSigner<'a>> {
        let keys = self.public_keys()?.ok_or_else(|| anyhow!("还没建钥匙，先运行 dct keys init"))?;
        let public = decode(match role {
            SignerRole::Auto => &keys.auto,
            SignerRole::User => &keys.user,
        })?;
        let blob = std::fs::read(self.blob_path(role)).context("读不到钥匙文件")?;
        Ok(EnclaveSigner { role, blob, public, se })
    }
}

pub struct EnclaveSigner<'a> {
    role: SignerRole,
    blob: Vec<u8>,
    public: [u8; 65],
    se: &'a dyn SecureEnclave,
}

impl Signer for EnclaveSigner<'_> {
    fn role(&self) -> SignerRole {
        self.role
    }
    fn public_key(&self) -> [u8; 65] {
        self.public
    }
    fn sign(&self, msg: &[u8], reason: &str) -> Result<[u8; 64], SignError> {
        self.se.sign(&self.blob, msg, reason)
    }
}

/// `dct keys init | show | test`。
pub fn run_cli(args: &[String]) -> i32 {
    let ks = KeyStore::at(KeyStore::default_dir());
    let se = platform_enclave();
    let r = match args.first().map(String::as_str) {
        Some("init") => ks.init(se.as_ref()).map(|k| {
            println!("两把钥匙已就绪：\n  自动钥匙 {}\n  指纹钥匙 {}", k.auto.key_id, k.user.key_id);
        }),
        Some("show") => ks.public_keys().and_then(|k| {
            let k = k.ok_or_else(|| anyhow!("还没建钥匙，先运行 dct keys init"))?;
            println!("{}", serde_json::to_string_pretty(&k)?);
            Ok(())
        }),
        Some("test") => (|| {
            let s = ks.signer(SignerRole::User, se.as_ref())?;
            let sig = s
                .sign(b"dct keys test", "测试：dct 用指纹签一条测试消息")
                .map_err(|e| anyhow!("{e}"))?;
            let signature = dct_brain::sign::Signature {
                key_id: key_id(&s.public_key()),
                alg: dct_brain::sign::ALG.into(),
                sig: STANDARD.encode(sig),
            };
            dct_brain::sign::verify_sig(&signature, b"dct keys test", &ks.trusted()?)
                .map_err(|e| anyhow!("{e}"))?;
            println!("指纹钥匙能用：签名已核对");
            Ok(())
        })(),
        _ => {
            eprintln!("用法：dct keys init | show | test");
            return 2;
        }
    };
    match r {
        Ok(()) => 0,
        Err(e) => {
            eprintln!("{e}");
            1
        }
    }
}
```

`src/lib.rs` 加 `pub mod keys;`（放在字母序对应的位置）。`src/main.rs` 的子命令 `match` 里、`Some("llm")` 那一行前面加：

```rust
        Some("keys") => std::process::exit(dct::keys::run_cli(&args[1..])),
```

如果 `anyhow` / `base64` 还不在 dct 的 `[dependencies]` 里，按 Cargo.lock 已有的版本加上（`base64 = "0.22"`）。

- [ ] **Step 5: 跑测试，确认通过；检查各平台都能编**

Run: `cargo test --lib keys:: && cargo test --workspace -q 2>&1 | grep -E "test result|FAILED" && cargo clippy --workspace --all-targets -q -- -D warnings && cargo check --target x86_64-pc-windows-msvc --all-targets -q`
Expected: 4 个 keys 测试 PASS；workspace 全绿；clippy 没有警告；Windows 检查能过（Windows 上 `build.rs` 直接返回，没有 Swift）。

- [ ] **Step 6: 在真 Mac 上手动验证安全芯片**

Run: `cargo build -q && ./target/debug/dct keys init && ./target/debug/dct keys show && ./target/debug/dct keys test`
Expected: `init` 打印两个 `sha256:` 开头的指纹；`show` 打印公钥 JSON；`test` **弹出 Touch ID**，提示文字是「测试：dct 用指纹签一条测试消息」，按下后打印「指纹钥匙能用：签名已核对」；取消则打印「没有通过指纹确认」、退出码 1。
注意：这一步会在**你自己的** `~/.dct/keys` 里建真钥匙。由执行任务的 agent 把命令和结果贴给主会话，Touch ID 那一下由用户本人按。

- [ ] **Step 7: Commit**

```bash
git add build.rs swift/Keys.swift src/keys src/lib.rs src/main.rs Cargo.toml Cargo.lock crates/dct-brain/src/sign.rs
git commit -m "feat(keys): an automatic key and a Touch ID key in the Mac's Secure Enclave"
```

---

### Task 8: `dct procedure approve | ticket`

**Files:**
- Create: `src/procedures.rs`
- Modify: `src/lib.rs`（加 `pub mod procedures;`）
- Modify: `src/main.rs`（加 `Some("procedure")` 分支）

**Interfaces:**
- Consumes: `dct_brain::{steps::{Step, steps_sha256}, tier::{tier_for_step, Tier, SignerRole}, approval::*, ticket::*, sign::{sign_one, SignedTicket}}`、`keys::{KeyStore, SecureEnclave, platform_enclave}`、`crate::sys::proc::no_console`。
- Produces:
  - `procedures::Dcv` trait：`show(&self, full_name: &str) -> anyhow::Result<serde_json::Value>`（`dcv show <库:名字> --json`）、`approve(&self, full_name: &str) -> anyhow::Result<String>`（`dcv approve <库:名字> --json`，返回 JSON 里的 `approved_body`）。`RealDcv` 调本机的 `dcv` 命令。
  - `procedures::Shown { full_name, params: Vec<String>, steps: Vec<Step>, body_sha256, executable: bool, why_not: Option<String> }`，`Shown::parse(full_name, &Value) -> Result<Shown>`。`dcv show --json` 的流程形状：`params`（名字数组）、`steps`（每步 `{n, action, arg, done_when: {action: "verify_text", arg} | null}`）、`body_sha256`、`executable`、`why_not`。
  - `procedures::Approvals`：`at(dir)`、`default_dir()`（`~/.dct/approvals`）、`save(&SignedApproval)`、`load(steps_sha256: &str) -> Result<Option<SignedApproval>>`；文件名是指纹的 64 位十六进制 + `.json`。
  - `approve(dcv, keys, se, approvals, full_name, overrides: &[(u32, Tier)], now: u64) -> Result<SignedApproval>`：
    1. `dcv.show` → `Shown`；
    2. 算 `steps_sha256`，每步用 `tier_for_step` 提议档位，再套上用户的改动 `overrides`（`--tier 4=content` 这种，按 n 找）；
    3. **先**用用户钥匙签批准记录（弹 Touch ID，人话写清楚：「批准流程 social:edit-bio：第 4 步「保存」是对外，其余只影响自己」）。用户取消就什么都不写；
    4. **再**调 `dcv.approve`，把返回的 `approved_body` 跟第 1 步看到的 `body_sha256` 比；不一样就说明两步之间流程被改过，报错、不保存批准记录；
    5. 保存批准记录。
  - `ticket(dcv, keys, se, approvals, full_name, device, params: BTreeMap<String,String>, start_in: u64, window: u64, now: u64) -> Result<SignedTicket>`：
    1. `dcv.show` → 必须 `executable`，否则报 `why_not`；
    2. 参数的名字必须跟流程声明的**一模一样**（不多不少）；
    3. 算 `steps_sha256`，取批准记录并 `verify_approval`（钥匙用 `keys.trusted()`），没有或不对就报「这条流程还没批准，或者改过了，先运行 dct procedure approve」；
    4. 票的档位 = `approval.run_tier()`；`nonce` 用 `getrandom` 取 16 字节转小写十六进制；`earliest = now + start_in`，`expires = earliest + window`；
    5. 签名：`run_tier.required_signer()` 为 `Some(User)` 用用户钥匙（Touch ID 人话：「执行流程 social:edit-bio（第 4 步对外）」），否则用自动钥匙；
    6. 返回的 `SignedTicket` 由命令打印成 JSON，以后交给 dco 的 `run_procedure`。
  - `run_cli(args: &[String]) -> i32`：
    - `dct procedure approve <库:名字> [--tier N=档位]...`
    - `dct procedure ticket <库:名字> --device <设备> [--param 名=值]... [--in 秒] [--window 秒]`（`--in` 默认 0，`--window` 默认 600）

**说明：** dcv 第二版的 `show` / `approve` 还在 dc-vault 那边实现中，本任务的测试全部用假的 `Dcv`。等 dcv 装上第二版，再做一次手动端到端（Step 6）。

- [ ] **Step 1: 写失败的测试**

`src/procedures.rs` 末尾：

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::keys::{KeyStore, SecureEnclave};
    use dct_brain::sign::soft::SoftSigner;
    use dct_brain::sign::{SignError, Signer};
    use serde_json::json;
    use std::cell::{Cell, RefCell};

    struct FakeEnclave {
        next: Cell<u8>,
        cancel_user: Cell<bool>,
        reasons: RefCell<Vec<String>>,
    }
    impl FakeEnclave {
        fn new() -> Self {
            FakeEnclave { next: Cell::new(1), cancel_user: Cell::new(false), reasons: RefCell::new(vec![]) }
        }
        fn soft(blob: &[u8]) -> SoftSigner {
            SoftSigner::from_seed(if blob[1] == 1 { SignerRole::User } else { SignerRole::Auto }, blob[0])
        }
    }
    impl SecureEnclave for FakeEnclave {
        fn available(&self) -> bool {
            true
        }
        fn create(&self, biometric: bool) -> Result<(Vec<u8>, [u8; 65]), SignError> {
            let blob = vec![self.next.get(), biometric as u8];
            self.next.set(self.next.get() + 1);
            Ok((blob.clone(), Self::soft(&blob).public_key()))
        }
        fn sign(&self, blob: &[u8], msg: &[u8], reason: &str) -> Result<[u8; 64], SignError> {
            if blob[1] == 1 {
                self.reasons.borrow_mut().push(reason.to_string());
                if self.cancel_user.get() {
                    return Err(SignError::Cancelled);
                }
            }
            Self::soft(blob).sign(msg, reason)
        }
    }

    struct FakeDcv {
        show: RefCell<serde_json::Value>,
        approved_body: RefCell<String>,
        approve_calls: Cell<u32>,
    }
    impl Dcv for FakeDcv {
        fn show(&self, _: &str) -> Result<serde_json::Value> {
            Ok(self.show.borrow().clone())
        }
        fn approve(&self, _: &str) -> Result<String> {
            self.approve_calls.set(self.approve_calls.get() + 1);
            Ok(self.approved_body.borrow().clone())
        }
    }

    fn edit_bio(executable: bool) -> serde_json::Value {
        json!({
            "kind": "procedure", "name": "edit-bio", "params": ["bio"],
            "steps": [
                {"n": 1, "action": "open_app", "arg": "TikTok", "done_when": null},
                {"n": 2, "action": "navigate_by_intent", "arg": "我的 → 编辑资料 → 简介", "done_when": null},
                {"n": 3, "action": "type_param", "arg": "bio", "done_when": {"action": "verify_text", "arg": "简介栏显示 {bio}"}},
                {"n": 4, "action": "tap_by_intent", "arg": "保存", "done_when": null}
            ],
            "body_sha256": format!("sha256:{}", "ab".repeat(32)),
            "executable": executable, "why_not": if executable { serde_json::Value::Null } else { json!("还没确认") },
            "schema": 1
        })
    }

    struct World {
        _d: tempfile::TempDir,
        dcv: FakeDcv,
        keys: KeyStore,
        se: FakeEnclave,
        approvals: Approvals,
    }

    fn world() -> World {
        let d = tempfile::tempdir().unwrap();
        let keys = KeyStore::at(d.path().join("keys"));
        let se = FakeEnclave::new();
        keys.init(&se).unwrap();
        World {
            dcv: FakeDcv {
                show: RefCell::new(edit_bio(false)),
                approved_body: RefCell::new(format!("sha256:{}", "ab".repeat(32))),
                approve_calls: Cell::new(0),
            },
            approvals: Approvals::at(d.path().join("approvals")),
            keys,
            se,
            _d: d,
        }
    }

    const STEPS_A: &str = "sha256:4d1077af6efac1184e410d23b0b7b120fd42f29d7dade3304ef2041ca10b565c";

    #[test]
    fn show_output_parses_into_the_steps_dcv_fingerprints() {
        let s = Shown::parse("social:edit-bio", &edit_bio(true)).unwrap();
        assert_eq!(steps_sha256(&s.params, &s.steps), STEPS_A);
        assert!(s.executable);
    }

    #[test]
    fn approving_proposes_tiers_asks_for_touch_id_then_marks_dcv() {
        let w = world();
        let sa = approve(&w.dcv, &w.keys, &w.se, &w.approvals, "social:edit-bio", &[], 1000).unwrap();
        assert_eq!(
            sa.approval.step_tiers,
            vec![Tier::SelfOnly, Tier::SelfOnly, Tier::SelfOnly, Tier::Content]
        );
        assert_eq!(sa.approval.steps_sha256, STEPS_A);
        assert_eq!(w.dcv.approve_calls.get(), 1);
        let reasons = w.se.reasons.borrow();
        assert!(reasons[0].contains("social:edit-bio") && reasons[0].contains("保存"), "{reasons:?}");
        assert!(w.approvals.load(STEPS_A).unwrap().is_some());
    }

    #[test]
    fn the_user_can_change_a_proposed_tier() {
        let w = world();
        let sa = approve(&w.dcv, &w.keys, &w.se, &w.approvals, "social:edit-bio", &[(4, Tier::Money)], 1000).unwrap();
        assert_eq!(sa.approval.step_tiers[3], Tier::Money);
    }

    #[test]
    fn cancelling_touch_id_approves_nothing() {
        let w = world();
        w.se.cancel_user.set(true);
        assert!(approve(&w.dcv, &w.keys, &w.se, &w.approvals, "social:edit-bio", &[], 1000).is_err());
        assert_eq!(w.dcv.approve_calls.get(), 0, "没按指纹就不该去 dcv 里确认");
        assert!(w.approvals.load(STEPS_A).unwrap().is_none());
    }

    #[test]
    fn a_procedure_changed_between_show_and_approve_is_not_saved() {
        let w = world();
        *w.dcv.approved_body.borrow_mut() = "sha256:changed".into();
        assert!(approve(&w.dcv, &w.keys, &w.se, &w.approvals, "social:edit-bio", &[], 1000).is_err());
        assert!(w.approvals.load(STEPS_A).unwrap().is_none());
    }

    #[test]
    fn a_ticket_for_an_approved_outward_procedure_is_signed_by_the_user() {
        let w = world();
        approve(&w.dcv, &w.keys, &w.se, &w.approvals, "social:edit-bio", &[], 1000).unwrap();
        *w.dcv.show.borrow_mut() = edit_bio(true);
        let params = BTreeMap::from([("bio".to_string(), "Founder, Learning Tech.".to_string())]);
        let st = ticket(&w.dcv, &w.keys, &w.se, &w.approvals, "social:edit-bio", "mac-lei", params, 0, 600, 2000).unwrap();
        assert_eq!(st.ticket.tier, Tier::Content);
        assert_eq!(st.ticket.earliest, 2000);
        assert_eq!(st.ticket.expires, 2600);
        assert_eq!(st.ticket.nonce.len(), 32);
        assert_eq!(dct_brain::sign::verify(&st, &w.keys.trusted().unwrap(), "mac-lei", 2000), Ok(()));
        assert!(w.se.reasons.borrow().last().unwrap().contains("执行流程"));
    }

    #[test]
    fn a_self_only_procedure_gets_an_automatic_ticket_without_touch_id() {
        let w = world();
        approve(&w.dcv, &w.keys, &w.se, &w.approvals, "social:edit-bio", &[(4, Tier::SelfOnly)], 1000).unwrap();
        let before = w.se.reasons.borrow().len();
        *w.dcv.show.borrow_mut() = edit_bio(true);
        let params = BTreeMap::from([("bio".to_string(), "x".to_string())]);
        let st = ticket(&w.dcv, &w.keys, &w.se, &w.approvals, "social:edit-bio", "mac-lei", params, 0, 600, 2000).unwrap();
        assert_eq!(st.ticket.tier, Tier::SelfOnly);
        assert_eq!(w.se.reasons.borrow().len(), before, "只影响自己的票不该弹 Touch ID");
    }

    #[test]
    fn tickets_are_refused_without_an_approval_or_with_wrong_params() {
        let w = world();
        *w.dcv.show.borrow_mut() = edit_bio(true);
        let good = BTreeMap::from([("bio".to_string(), "x".to_string())]);
        assert!(ticket(&w.dcv, &w.keys, &w.se, &w.approvals, "social:edit-bio", "mac-lei", good.clone(), 0, 600, 2000).is_err());
        *w.dcv.show.borrow_mut() = edit_bio(false);
        approve(&w.dcv, &w.keys, &w.se, &w.approvals, "social:edit-bio", &[], 1000).unwrap();
        *w.dcv.show.borrow_mut() = edit_bio(true);
        let extra = BTreeMap::from([("bio".to_string(), "x".to_string()), ("x".to_string(), "y".to_string())]);
        assert!(ticket(&w.dcv, &w.keys, &w.se, &w.approvals, "social:edit-bio", "mac-lei", extra, 0, 600, 2000).is_err());
        assert!(ticket(&w.dcv, &w.keys, &w.se, &w.approvals, "social:edit-bio", "mac-lei", BTreeMap::new(), 0, 600, 2000).is_err());
    }

    #[test]
    fn a_non_executable_procedure_gets_no_ticket() {
        let w = world();
        approve(&w.dcv, &w.keys, &w.se, &w.approvals, "social:edit-bio", &[], 1000).unwrap();
        *w.dcv.show.borrow_mut() = edit_bio(false);
        let params = BTreeMap::from([("bio".to_string(), "x".to_string())]);
        let e = ticket(&w.dcv, &w.keys, &w.se, &w.approvals, "social:edit-bio", "mac-lei", params, 0, 600, 2000).unwrap_err();
        assert!(e.to_string().contains("还没确认"), "{e}");
    }

    #[test]
    fn a_tampered_approval_file_is_not_trusted() {
        let w = world();
        let mut sa = approve(&w.dcv, &w.keys, &w.se, &w.approvals, "social:edit-bio", &[], 1000).unwrap();
        sa.approval.step_tiers[3] = Tier::SelfOnly;
        w.approvals.save(&sa).unwrap();
        *w.dcv.show.borrow_mut() = edit_bio(true);
        let params = BTreeMap::from([("bio".to_string(), "x".to_string())]);
        assert!(ticket(&w.dcv, &w.keys, &w.se, &w.approvals, "social:edit-bio", "mac-lei", params, 0, 600, 2000).is_err());
    }
}
```

- [ ] **Step 2: 跑测试，确认失败**

Run: `cargo test --lib procedures::`
Expected: 编译失败。

- [ ] **Step 3: 实现**

`src/procedures.rs` 测试模块上面：

```rust
//! 流程的批准和出票（设计第 2、4 节）。流程从 dcv 来，批准记录和钥匙在 dct，
//! 票交给 dco。这里只做「批准」「出票」两件事；动作卡片、排程、连 dco 在后面的计划里。
use crate::keys::{platform_enclave, KeyStore, SecureEnclave};
use anyhow::{anyhow, bail, Context, Result};
use dct_brain::approval::{sign_approval, verify_approval, Approval, SignedApproval, APPROVAL_VERSION};
use dct_brain::sign::{sign_one, SignedTicket};
use dct_brain::steps::{steps_sha256, Step};
use dct_brain::ticket::{params_sha256, Subject, Ticket, TICKET_VERSION};
use dct_brain::tier::{tier_for_step, SignerRole, Tier};
use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

pub trait Dcv {
    fn show(&self, full_name: &str) -> Result<serde_json::Value>;
    /// 返回 dcv 记下的 `approved_body`。
    fn approve(&self, full_name: &str) -> Result<String>;
}

pub struct RealDcv;

fn run_dcv(args: &[&str]) -> Result<serde_json::Value> {
    let mut c = std::process::Command::new("dcv");
    c.args(args);
    crate::sys::proc::no_console(&mut c);
    let out = c.output().context("找不到 dcv 命令")?;
    if !out.status.success() {
        bail!("{}", String::from_utf8_lossy(&out.stderr).trim());
    }
    serde_json::from_slice(&out.stdout).context("dcv 的回复读不懂")
}

impl Dcv for RealDcv {
    fn show(&self, full_name: &str) -> Result<serde_json::Value> {
        run_dcv(&["show", full_name, "--json"])
    }
    fn approve(&self, full_name: &str) -> Result<String> {
        let v = run_dcv(&["approve", full_name, "--json"])?;
        v["approved_body"].as_str().map(String::from).ok_or_else(|| anyhow!("dcv 没有回 approved_body"))
    }
}

pub struct Shown {
    pub full_name: String,
    pub params: Vec<String>,
    pub steps: Vec<Step>,
    pub body_sha256: String,
    pub executable: bool,
    pub why_not: Option<String>,
}

impl Shown {
    pub fn parse(full_name: &str, v: &serde_json::Value) -> Result<Shown> {
        if v["kind"] != "procedure" {
            bail!("{full_name} 不是流程");
        }
        let params = v["params"]
            .as_array()
            .ok_or_else(|| anyhow!("dcv 回复缺 params"))?
            .iter()
            .map(|p| p.as_str().map(String::from).ok_or_else(|| anyhow!("参数名不是字符串")))
            .collect::<Result<Vec<_>>>()?;
        let steps = v["steps"]
            .as_array()
            .ok_or_else(|| anyhow!("dcv 回复缺 steps"))?
            .iter()
            .map(|s| {
                Ok(Step {
                    n: s["n"].as_u64().ok_or_else(|| anyhow!("步骤缺 n"))? as u32,
                    action: s["action"].as_str().ok_or_else(|| anyhow!("步骤缺 action"))?.to_string(),
                    arg: s["arg"].as_str().unwrap_or("").to_string(),
                    done_when: s["done_when"]["arg"].as_str().map(String::from),
                })
            })
            .collect::<Result<Vec<_>>>()?;
        Ok(Shown {
            full_name: full_name.to_string(),
            params,
            steps,
            body_sha256: v["body_sha256"].as_str().ok_or_else(|| anyhow!("dcv 回复缺 body_sha256"))?.to_string(),
            executable: v["executable"].as_bool().unwrap_or(false),
            why_not: v["why_not"].as_str().map(String::from),
        })
    }
}

pub struct Approvals {
    dir: PathBuf,
}

impl Approvals {
    pub fn at(dir: PathBuf) -> Self {
        Approvals { dir }
    }

    pub fn default_dir() -> PathBuf {
        crate::proto::socket_path()
            .parent()
            .map(|p| p.join("approvals"))
            .unwrap_or_else(|| PathBuf::from("approvals"))
    }

    fn path(&self, steps_sha256: &str) -> PathBuf {
        self.dir.join(format!("{}.json", steps_sha256.trim_start_matches("sha256:")))
    }

    pub fn save(&self, sa: &SignedApproval) -> Result<()> {
        std::fs::create_dir_all(&self.dir)?;
        std::fs::write(self.path(&sa.approval.steps_sha256), serde_json::to_vec_pretty(sa)?)?;
        Ok(())
    }

    pub fn load(&self, steps_sha256: &str) -> Result<Option<SignedApproval>> {
        match std::fs::read(self.path(steps_sha256)) {
            Ok(b) => Ok(Some(serde_json::from_slice(&b).context("批准记录坏了")?)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e.into()),
        }
    }
}

fn tier_zh(t: Tier) -> &'static str {
    match t {
        Tier::Read => "只读",
        Tier::SelfOnly => "只影响自己",
        Tier::Physical => "物理动作",
        Tier::Content => "对外",
        Tier::Money => "动钱",
    }
}

/// Touch ID 弹窗里的人话：点名每一步对外或动钱的动作，其余一句带过。
fn approval_reason(name: &str, steps: &[Step], tiers: &[Tier]) -> String {
    let strict: Vec<String> = steps
        .iter()
        .zip(tiers)
        .filter(|(_, t)| **t >= Tier::Content)
        .map(|(s, t)| format!("第 {} 步「{}」是{}", s.n, s.arg, tier_zh(*t)))
        .collect();
    if strict.is_empty() {
        format!("批准流程 {name}：每一步都只影响自己")
    } else {
        format!("批准流程 {name}：{}，其余只影响自己", strict.join("，"))
    }
}

pub fn approve(
    dcv: &dyn Dcv,
    keys: &KeyStore,
    se: &dyn SecureEnclave,
    approvals: &Approvals,
    full_name: &str,
    overrides: &[(u32, Tier)],
    now: u64,
) -> Result<SignedApproval> {
    let shown = Shown::parse(full_name, &dcv.show(full_name)?)?;
    let steps_sha = steps_sha256(&shown.params, &shown.steps);
    let mut tiers: Vec<Tier> = shown.steps.iter().map(|s| tier_for_step(&s.action, &s.arg)).collect();
    for (n, t) in overrides {
        let i = shown
            .steps
            .iter()
            .position(|s| s.n == *n)
            .ok_or_else(|| anyhow!("这条流程没有第 {n} 步"))?;
        tiers[i] = *t;
    }
    let approval = Approval {
        v: APPROVAL_VERSION,
        procedure: full_name.to_string(),
        steps_sha256: steps_sha,
        body_sha256: shown.body_sha256.clone(),
        step_tiers: tiers.clone(),
        approved_at: now,
    };
    // 先按指纹，再去 dcv 里确认：用户取消就什么都不留下。
    let user = keys.signer(SignerRole::User, se)?;
    let signed = sign_approval(approval, &user, &approval_reason(full_name, &shown.steps, &tiers))
        .map_err(|e| anyhow!("{e}"))?;
    let approved_body = dcv.approve(full_name)?;
    if approved_body != shown.body_sha256 {
        bail!("流程在确认的过程中被改过了（看到的是 {}，dcv 确认的是 {approved_body}），这次不算，请重新看一遍再批准", shown.body_sha256);
    }
    approvals.save(&signed)?;
    Ok(signed)
}

fn random_nonce() -> Result<String> {
    let mut b = [0u8; 16];
    getrandom::getrandom(&mut b).map_err(|e| anyhow!("取不到随机数：{e}"))?;
    Ok(b.iter().map(|x| format!("{x:02x}")).collect())
}

#[allow(clippy::too_many_arguments)]
pub fn ticket(
    dcv: &dyn Dcv,
    keys: &KeyStore,
    se: &dyn SecureEnclave,
    approvals: &Approvals,
    full_name: &str,
    device: &str,
    params: BTreeMap<String, String>,
    start_in: u64,
    window: u64,
    now: u64,
) -> Result<SignedTicket> {
    let shown = Shown::parse(full_name, &dcv.show(full_name)?)?;
    if !shown.executable {
        bail!("这条流程现在不能执行：{}", shown.why_not.as_deref().unwrap_or("dcv 没说原因"));
    }
    let declared: BTreeSet<&str> = shown.params.iter().map(String::as_str).collect();
    let given: BTreeSet<&str> = params.keys().map(String::as_str).collect();
    if declared != given {
        bail!("参数对不上：流程要的是 {declared:?}，给的是 {given:?}");
    }
    let steps_sha = steps_sha256(&shown.params, &shown.steps);
    let not_approved = || anyhow!("这条流程还没批准，或者改过了，先运行 dct procedure approve {full_name}");
    let sa = approvals.load(&steps_sha)?.ok_or_else(not_approved)?;
    verify_approval(&sa, &keys.trusted()?, &steps_sha).map_err(|_| not_approved())?;
    let tier = sa.approval.run_tier();
    let earliest = now + start_in;
    let t = Ticket {
        v: TICKET_VERSION,
        nonce: random_nonce()?,
        device: device.to_string(),
        tier,
        subject: Subject::Procedure {
            name: full_name.to_string(),
            steps_sha256: steps_sha,
            body_sha256: shown.body_sha256,
        },
        params_sha256: params_sha256(&params),
        earliest,
        expires: earliest + window,
    };
    let role = match tier.required_signer() {
        Some(SignerRole::User) => SignerRole::User,
        _ => SignerRole::Auto,
    };
    let signer = keys.signer(role, se)?;
    let strict: Vec<String> = shown
        .steps
        .iter()
        .zip(&sa.approval.step_tiers)
        .filter(|(_, t)| **t >= Tier::Content)
        .map(|(s, _)| format!("第 {} 步", s.n))
        .collect();
    let reason = format!("执行流程 {full_name}（{}对外）", strict.join("、"));
    sign_one(t, &signer, &reason).map_err(|e| anyhow!("{e}"))
}

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn parse_tier(s: &str) -> Result<Tier> {
    serde_json::from_value(serde_json::Value::String(s.to_string()))
        .map_err(|_| anyhow!("不认识的档位 {s}（可用：read self physical content money）"))
}

/// `dct procedure approve <库:名字> [--tier N=档位]...`
/// `dct procedure ticket <库:名字> --device <设备> [--param 名=值]... [--in 秒] [--window 秒]`
pub fn run_cli(args: &[String]) -> i32 {
    let r = (|| -> Result<()> {
        let (cmd, name) = match (args.first(), args.get(1)) {
            (Some(c), Some(n)) => (c.as_str(), n.as_str()),
            _ => bail!("用法：dct procedure approve|ticket <库:名字> ..."),
        };
        let keys = KeyStore::at(KeyStore::default_dir());
        let se = platform_enclave();
        let approvals = Approvals::at(Approvals::default_dir());
        let mut overrides = Vec::new();
        let mut params = BTreeMap::new();
        let (mut device, mut start_in, mut window) = (None, 0u64, 600u64);
        let mut it = args[2..].iter();
        while let Some(flag) = it.next() {
            let val = it.next().ok_or_else(|| anyhow!("{flag} 后面缺值"))?;
            match flag.as_str() {
                "--tier" => {
                    let (n, t) = val.split_once('=').ok_or_else(|| anyhow!("--tier 要写成 步号=档位"))?;
                    overrides.push((n.parse().context("步号不是数字")?, parse_tier(t)?));
                }
                "--param" => {
                    let (k, v) = val.split_once('=').ok_or_else(|| anyhow!("--param 要写成 名=值"))?;
                    params.insert(k.to_string(), v.to_string());
                }
                "--device" => device = Some(val.clone()),
                "--in" => start_in = val.parse().context("--in 不是秒数")?,
                "--window" => window = val.parse().context("--window 不是秒数")?,
                f => bail!("不认识的选项 {f}"),
            }
        }
        match cmd {
            "approve" => {
                let sa = approve(&RealDcv, &keys, se.as_ref(), &approvals, name, &overrides, now_secs())?;
                for (i, t) in sa.approval.step_tiers.iter().enumerate() {
                    println!("第 {} 步：{}", i + 1, tier_zh(*t));
                }
                println!("已批准 {name}");
            }
            "ticket" => {
                let device = device.ok_or_else(|| anyhow!("出票要写 --device"))?;
                let st = ticket(&RealDcv, &keys, se.as_ref(), &approvals, name, &device, params, start_in, window, now_secs())?;
                println!("{}", serde_json::to_string_pretty(&st)?);
            }
            c => bail!("不认识的子命令 {c}"),
        }
        Ok(())
    })();
    match r {
        Ok(()) => 0,
        Err(e) => {
            eprintln!("{e}");
            1
        }
    }
}
```

`src/lib.rs` 加 `pub mod procedures;`；`src/main.rs` 在 `Some("keys")` 那一行下面加：

```rust
        Some("procedure") => std::process::exit(dct::procedures::run_cli(&args[1..])),
```

`crate::sys::proc::no_console` 的签名以现有代码为准（`src/git.rs` 里就是这么用的：`crate::sys::proc::no_console(&mut c);`）。

- [ ] **Step 4: 跑测试，确认通过；全面检查**

Run: `cargo test --lib procedures:: && cargo test --workspace -q 2>&1 | grep -E "test result|FAILED" && cargo clippy --workspace --all-targets -q -- -D warnings && cargo check --target x86_64-pc-windows-msvc --all-targets -q`
Expected: procedures 的 10 个测试 PASS；workspace 全绿；clippy 没有警告；Windows 检查能过。

- [ ] **Step 5: Commit**

```bash
git add src/procedures.rs src/lib.rs src/main.rs
git commit -m "feat(procedure): approve a procedure's tiers with Touch ID and sign tickets for it"
```

- [ ] **Step 6（等 dcv 第二版装好后做）: 手动端到端**

前提：本机 `dcv` 已经是第二版（`dcv show <库:名字> --json` 能返回流程），并且有一条流程草稿（比如 dc-vault 会话建的 `social:edit-bio`）。

Run:
```bash
./target/debug/dct procedure approve social:edit-bio
./target/debug/dct procedure ticket social:edit-bio --device mac-lei --param bio="Founder, Learning Tech."
```
Expected: `approve` 弹 Touch ID，提示里点名第 4 步「保存」是对外；按下后逐步打印档位、「已批准」，`dcv show` 里这条变成 `executable: true`。`ticket` 再弹一次 Touch ID（因为有对外的一步），打印一张 `SignedTicket` JSON：`tier` 是 `content`，`subject.steps_sha256` 跟 `dcv show --json` 里的 `steps_sha256`（dcv 第二版 Task 15 加上后）一致。
dcv 还没装好时，这一步留给主会话在交付说明里写清「未做」。

---

## 交付时要告诉用户的

1. **自动钥匙的保护比设计弱一档**（Task 7 的说明）：没有 Apple 开发者证书，自动钥匙只受文件权限保护，同一账号下的 agent 能签出 `self` / `physical` 档的票；对外和动钱的票不受影响（必须当场按 Touch ID）。以后有开发者证书时可以补上。
2. **端到端还没跑**：dcv 第二版的 `show` / `approve` 还没实现，本计划的测试都用假的 dcv；Task 8 Step 6 等 dcv 装好后再做。
3. **判断模块（judge）还没做**：卡在没有 DC 网关密钥上，单独出计划；本计划的定档先靠规则。
4. dco 那边要做的：用 `dct-brain` 的 `sign::verify` 验票、自己记 nonce、按 `tier::raise` 做兜底，把 `Tier` 换成 `dct-brain` 里那份（多了 `physical`）。这需要通知 dc-octo 会话，不在本计划里改 dco。
