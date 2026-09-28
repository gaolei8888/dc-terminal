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

