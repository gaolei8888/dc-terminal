### Task 1: `Waiver`、`Region`、`Ticket.waiver` 和 `canonical_bytes`

**Files:**
- Modify: `crates/dct-brain/src/ticket.rs`；以及所有构造 `Ticket { .. }` 的地方（`crates/dct-brain/src/*.rs` 的测试、`src/procedures.rs`、`src/**` 里凡是出现 `Ticket {` 的）补 `waiver: None`
- Test: `crates/dct-brain/src/ticket.rs` 的 `tests` 模块

**Interfaces:**
- Produces:
```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Region { pub x_bp: u16, pub y_bp: u16, pub w_bp: u16, pub h_bp: u16 }

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Waiver {
    pub kind: String,          // 现在只有 "no_text"
    pub rule: String,          // 库:名字
    pub rule_sha256: String,   // dcv 的 sha
    pub approved_at: String,   // dcv 的 approved_at
    pub region: Region,
}
// Ticket 加：
#[serde(default, skip_serializing_if = "Option::is_none")]
pub waiver: Option<Waiver>,
```
- `canonical_bytes`：在现有最后一个 `field(&mut o, &self.expires.to_string());` 之后，**仅当 `self.waiver.is_some()`** 追加：`field("waiver")`、`field(kind)`、`field(rule)`、`field(rule_sha256)`、`field(approved_at)`、四个区域整数（`to_string()`，顺序 x_bp、y_bp、w_bp、h_bp）。`waiver` 为 `None` 时一个字节都不追加。

- [ ] **Step 1: 写失败的测试**

```rust
    fn base_ticket() -> Ticket {
        Ticket {
            v: TICKET_VERSION,
            nonce: "a".repeat(32),
            device: "mac".into(),
            tier: Tier::Content,
            subject: Subject::Action { tool: "tap_at".into(), args_sha256: "sha256:00".into() },
            params_sha256: "sha256:11".into(),
            earliest: 100,
            expires: 130,
            waiver: None,
        }
    }
    fn waiver() -> Waiver {
        Waiver {
            kind: "no_text".into(),
            rule: "games:darkness-and-flame-rules".into(),
            rule_sha256: "sha256:d868".into(),
            approved_at: "2026-10-04T13:18:12Z".into(),
            region: Region { x_bp: 300, y_bp: 1200, w_bp: 7500, h_bp: 6600 },
        }
    }

    #[test]
    fn a_ticket_without_a_waiver_has_the_same_bytes_as_before() {
        // 把 canonical_bytes 的结尾和「旧实现」逐字节对：最后一个字段是 expires，之后没有任何字节
        let t = base_ticket();
        let bytes = t.canonical_bytes();
        let mut want = Vec::new();
        field(&mut want, "dct-ticket-v1");
        field(&mut want, "1");
        field(&mut want, &"a".repeat(32));
        field(&mut want, "mac");
        field(&mut want, "content");
        field(&mut want, "action");
        field(&mut want, "tap_at");
        field(&mut want, "sha256:00");
        field(&mut want, "sha256:11");
        field(&mut want, "100");
        field(&mut want, "130");
        assert_eq!(bytes, want);
    }

    #[test]
    fn a_waiver_is_appended_and_changes_the_digest() {
        let plain = base_ticket();
        let mut w = base_ticket();
        w.waiver = Some(waiver());
        assert!(w.canonical_bytes().starts_with(&plain.canonical_bytes()));
        assert!(w.canonical_bytes().len() > plain.canonical_bytes().len());
        assert_ne!(w.digest(), plain.digest());
    }

    #[test]
    fn changing_any_waiver_field_changes_the_digest() {
        let mut base = base_ticket();
        base.waiver = Some(waiver());
        let d0 = base.digest();
        let tweaks: Vec<Box<dyn Fn(&mut Waiver)>> = vec![
            Box::new(|w| w.kind = "other".into()),
            Box::new(|w| w.rule = "games:x".into()),
            Box::new(|w| w.rule_sha256 = "sha256:beef".into()),
            Box::new(|w| w.approved_at = "2026-10-05T00:00:00Z".into()),
            Box::new(|w| w.region.x_bp += 1),
            Box::new(|w| w.region.y_bp += 1),
            Box::new(|w| w.region.w_bp -= 1),
            Box::new(|w| w.region.h_bp -= 1),
        ];
        for t in tweaks {
            let mut x = base.clone();
            t(x.waiver.as_mut().unwrap());
            assert_ne!(x.digest(), d0);
        }
    }

    #[test]
    fn json_has_no_waiver_key_when_absent_and_old_json_still_reads() {
        let t = base_ticket();
        let j = serde_json::to_string(&t).unwrap();
        assert!(!j.contains("waiver"));
        let back: Ticket = serde_json::from_str(&j).unwrap();
        assert_eq!(back, t);
        let mut w = base_ticket();
        w.waiver = Some(waiver());
        let back: Ticket = serde_json::from_str(&serde_json::to_string(&w).unwrap()).unwrap();
        assert_eq!(back, w);
    }
```

（`field` 来自 `crate::canon::field`，测试里 `use crate::canon::field;`。如果现有测试模块里已经有向量测试，沿用，**不要改现有断言**。）

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo +1.99.0 test -p dct-brain ticket::`
Expected: 编译失败（`Waiver`、`Region`、`waiver` 字段未定义）。

- [ ] **Step 3: 写实现**

按 Interfaces 写；`canonical_bytes` 末尾：

```rust
        if let Some(w) = &self.waiver {
            field(&mut o, "waiver");
            field(&mut o, &w.kind);
            field(&mut o, &w.rule);
            field(&mut o, &w.rule_sha256);
            field(&mut o, &w.approved_at);
            field(&mut o, &w.region.x_bp.to_string());
            field(&mut o, &w.region.y_bp.to_string());
            field(&mut o, &w.region.w_bp.to_string());
            field(&mut o, &w.region.h_bp.to_string());
        }
```

给所有现有的 `Ticket { .. }` 字面量补 `waiver: None,`（`grep -rn "Ticket {" crates src` 找全，编译器会逐个报）。

- [ ] **Step 4: 跑全部测试**

Run: `cargo +1.99.0 test -p dct-brain` 然后 `cargo +1.99.0 test --workspace --locked`；clippy。
Expected: 全绿；**现有测试一个断言都没改**。

- [ ] **Step 5: 变异检查，然后提交**

把「仅当 `is_some()` 才追加」改成「总是追加一个空标记」→ `a_ticket_without_a_waiver_has_the_same_bytes_as_before` 变红；去掉 `region.h_bp` 的追加 → `changing_any_waiver_field…` 变红；给字段去掉 `skip_serializing_if` → JSON 测试变红。改回。

```bash
git add -A crates src && git commit -m "feat(brain): tickets can carry an optional waiver naming the confirmed rule, its fingerprint and the allowed region; tickets without one are byte-for-byte unchanged"
```

---

