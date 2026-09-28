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

