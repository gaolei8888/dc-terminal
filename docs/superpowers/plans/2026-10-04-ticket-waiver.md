# 放行凭据（dct-brain 这一半） Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans. Steps use checkbox (`- [ ]`) syntax.

**Goal:** 让「用户在 dcv 里确认过的规矩」能放行一次没有字的点击：票多一个可选的放行凭据（`Waiver`），`verify` 在这一种情况下接受自动钥匙签的内容档票，并提供一个纯函数判断「这一点在不在规矩放行的区域里」。**不带凭据的票，字节、指纹、行为和今天逐字节相同。**

**Architecture:** 全在 `crates/dct-brain`（dct 和 dco 共用的票格式）。`Waiver` 和 `Region` 在 `ticket.rs`；`Ticket.waiver: Option<Waiver>`；`canonical_bytes` 只在有凭据时才追加字段；`sign.rs` 的 `role_may_sign` 和 `verify` 加唯一一条例外；一个纯函数 `waiver_for_tap` 判断点和区域。**这一半先单独合到 main**（dco 的 Cargo 用路径依赖编译 `dc-terminal/crates/dct-brain`）。签票（要密钥、要读 dcv、要 `list_windows`）在 `src/` 里，是后面的另一个计划。

**Tech Stack:** Rust，`serde`/`serde_json`，已有的 `canon::field`、`sha256_id`。

**Spec:** `/Users/lei/work/dc/dc-terminal/docs/superpowers/specs/2026-10-04-ticket-waiver-design.md`（dc-octo 已审定，包含 `region`）

## Global Constraints

- 提交信息用英文，**不加任何 AI 署名或 Co-Authored-By 行**。不推送；这个分支做完、审完直接合并到 `main` 并推送（用户要自动，dco 在等）。
- **最重要：不带 `waiver` 的票，`canonical_bytes`、`digest`、签名、`verify` 结果和今天逐字节相同。** 用现有测试向量钉住；任何现有测试不许改断言。
- 放行范围极窄：只有 `tier == Content`、`tier.rule_can_waive()`、`waiver.kind == "no_text"` 这一种；`Critical`、`Money` 永远不放行；`Physical`/`SelfOnly` 本来就允许自动钥匙，不受影响。
- 区域是整数万分比 `u16`（0–10000）；`w_bp`、`h_bp` 不能为 0；`x_bp + w_bp ≤ 10000`，`y_bp + h_bp ≤ 10000`；不合格的区域在 `verify` 里拒绝。
- `args_sha256` 的对象是 `{"window":{"window_id":N},"x_bp":..,"y_bp":..}`（不含 ticket），dco 自己算；本计划不改 `args_sha256`。
- 面向用户的文字用大白话中文（错误消息）。
- 提交前：`cargo +1.99.0 clippy -p dct-brain --all-targets -- -D warnings`、`cargo +1.99.0 test -p dct-brain`，再 `cargo +1.99.0 test --workspace --locked` 和 workspace clippy（`PATH` 含 `$HOME/.cargo/bin`；`CARGO_TARGET_DIR=/private/tmp/claude-502/-Users-lei-work-dc-dc-terminal/d52f1f4f-cb49-40cc-ba88-c37403102743/scratchpad/target-waiver`，用单独的目录，别和别的会话争锁）。已知偶发失败（机器忙时）：`daemon::web_tests::enabling_starts_a_listener_and_disabling_stops_it`、`session::tests::recovering_from_a_failure_after_real_input_still_does_not_count`、`list_is_not_blocked_by_slow_create`、`zombie_reaping`，单独重跑。
- 工作目录是 `/Users/lei/work/dc/dc-terminal-waiver`（git worktree，分支 `feat/ticket-waiver`）。**不要碰** `/Users/lei/work/dc/dc-terminal`（别的会话在用）。

## Review Focus

1. 带凭据的 `Content` 票由 `Auto` 签：通过；由 `User` 签：也通过（凭据只是多一个可选项，不削弱用户本人签）。
2. 带凭据但 `tier` 是 `Money`/`Critical`：`Auto` 签的拒绝。带凭据但 `kind` 不是 `no_text`：`Auto` 签的拒绝。没带凭据的 `Content`：`Auto` 签的拒绝（今天的行为）。
3. 凭据里改一个字（规矩名、指纹、时间、区域任一个）：签名对不上（`canonical_bytes` 包含全部字段）。
4. 区域边界：点正好落在边上算在内；差一个万分比算在外；`w_bp==0`、越界的区域被拒。
5. `Ticket` 的 JSON：没有凭据时序列化结果里**没有 `waiver` 键**（旧的 JSON 读写不变）；旧 JSON（没有这个键）能读。

---

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

### Task 2: `role_may_sign` / `verify` 的那一条例外

**Files:**
- Modify: `crates/dct-brain/src/sign.rs`
- Test: `crates/dct-brain/src/sign.rs` 的 `tests` 模块

**Interfaces:**
- Consumes: Task 1 的 `Waiver`、`Region`、`Ticket.waiver`。
- Produces: `role_may_sign(tier, role, waiver: Option<&Waiver>) -> bool`（私有函数改签名即可）；`VerifyError` 加两个变体：`BadWaiver`（凭据 `kind` 不认识，或区域不合格）；其余沿用。`verify` 的其他检查顺序和今天一样。

规则：
```text
role_may_sign(tier, role, waiver):
  match tier.required_signer():
    None | Some(Auto) => true
    Some(User) => role == User
                  || (role == Auto && tier == Content && tier.rule_can_waive()
                      && waiver.is_some_and(|w| w.kind == "no_text"))
```
区域检查（只在带凭据时，放在 `verify` 里验签之后）：`w_bp > 0 && h_bp > 0 && x_bp as u32 + w_bp as u32 <= 10000 && y_bp as u32 + h_bp as u32 <= 10000`，不满足 → `Err(VerifyError::BadWaiver)`。`kind` 不是 `"no_text"` 的凭据，在 `verify` 里直接 `BadWaiver`（现在只认这一种，将来新增再放开）。

- [ ] **Step 1: 写失败的测试**（用文件里已有的 `SoftSigner`、`SignerRole`、`verify`、`TrustedKey` 写法，沿用已有的辅助函数造票和签名；`Tier` 档位的票用 `sign_one` 签）

- 带凭据的 `Content` 票：`Auto` 签 → `verify` 通过；`User` 签 → 通过。
- 带凭据的 `Money`、`Critical` 票：`Auto` 签 → `verify` 拒绝（`RoleNotAllowed` 或现有对应的变体）。
- 带凭据、`kind: "other"` 的 `Content` 票：不管谁签 → `Err(BadWaiver)`。
- 没带凭据的 `Content` 票：`Auto` 签 → 拒绝（今天的行为，用同一个断言）。
- 区域不合格：`w_bp == 0`、`h_bp == 0`、`x_bp + w_bp > 10000`、`y_bp + h_bp > 10000` 各一条 → `Err(BadWaiver)`；边界 `x_bp + w_bp == 10000` 通过。
- 凭据被改（签完再改 `rule_sha256`）：签名对不上（现有的「签名不对」变体）。
- 无凭据的 `Physical` 票由 `Auto` 签：通过（不受影响）。

- [ ] **Step 2–5：** 跑测试确认失败 → 实现 → 全部测试和 clippy → 变异检查（把 `tier == Content` 去掉 → `Money` 的测试变红；去掉 `kind == "no_text"` → `kind: "other"` 的测试变红；去掉区域检查 → 区域测试变红）→ 提交：`feat(brain): verify accepts an auto-key signature on a content ticket only when it carries a no_text waiver with a sane region`。

---

### Task 3: `waiver_for_tap`（纯函数：这一点在不在规矩放行的区域里）

**Files:**
- Create: `crates/dct-brain/src/waiver.rs`；Modify: `crates/dct-brain/src/lib.rs`（`pub mod waiver;`）
- Test: `waiver.rs` 的 `tests`

**Interfaces:**
- Consumes: Task 1 的 `Waiver`、`Region`。
- Produces:
```rust
pub struct TapRule {          // dcv games --json 里一条已确认规矩的这一块
    pub rule: String,         // 库:名字
    pub rule_sha256: String,
    pub approved_at: Option<String>,   // None = 没确认
    pub tap_allow: Vec<TapAllow>,
}
pub struct TapAllow { pub kind: String, pub x: f64, pub y: f64, pub w: f64, pub h: f64 }  // 窗口 0–1 比例，dcv 的 region

#[derive(Debug, PartialEq, Eq)]
pub enum Refusal {
    NotApproved,        // approved_at 为空
    NoBlankAreaRule,    // 没有 kind == "no_text" 的区域
    OutsideRegion,      // 点不在任何区域里
    BadPoint,           // x_bp/y_bp 超过 10000
}
impl Refusal { pub fn zh(&self) -> &'static str }   // 大白话

pub fn waiver_for_tap(rule: &TapRule, x_bp: u16, y_bp: u16) -> Result<Waiver, Refusal>
```
行为：`x_bp > 10000 || y_bp > 10000` → `BadPoint`；`approved_at` 为 `None` → `NotApproved`；筛出 `kind == "no_text"` 的区域，没有 → `NoBlankAreaRule`；把区域换算成整数万分比（`round(v * 10000)`，夹在 0..=10000，保证 `x_bp + w_bp ≤ 10000`，换算后不合格的区域跳过）；点落在**任一**区域内（含边界）就返回 `Waiver { kind: "no_text", rule, rule_sha256, approved_at, region: 那块区域 }`；否则 `OutsideRegion`。

- [ ] **Step 1: 写失败的测试**
- 用真实的那条规矩（`tap_allow: no_text 0.03 0.12 0.75 0.66`）：点 (1800, 5800) 在区域内 → `Ok`，`region == {300, 1200, 7500, 6600}`，`rule`/`rule_sha256`/`approved_at` 原样带出。
- 边界：点正好在左上角 (300, 1200) 和右下角 (7800, 7800) 在内；(299, 1200)、(7801, 7800) 在外。
- 没确认（`approved_at: None`）→ `NotApproved`，即使点在区域内。
- 只有 `kind: "with_text"` 之类的区域 → `NoBlankAreaRule`；空 `tap_allow` → `NoBlankAreaRule`。
- 点超过 10000 → `BadPoint`。
- 区域换算后越界（例如 `x=0.9, w=0.2`）→ 被跳过，点在其中也 `OutsideRegion`（不返回越界的凭据）。
- 多个区域：点在第二块里 → 返回第二块的 region。
- 每个 `Refusal::zh()` 非空、无英文术语。

- [ ] **Step 2–5：** 跑测试确认失败 → 实现 → 全部测试和 clippy → 变异检查（把「含边界」改成不含 → 边界测试变红；去掉 `approved_at` 检查 → 没确认的测试变红；不过滤 `kind` → `with_text` 的测试变红）→ 提交：`feat(brain): waiver_for_tap says whether a point lies in a confirmed rule's blank-area region and returns the waiver to sign`。

---

## Self-Review

1. **覆盖：** spec 的「票多一个可选凭据（含区域）」「verify 例外」「纯函数判断点在不在区域」→ Task 1/2/3；签票、读 dcv、`list_windows`、`tap_at` 本身不在本计划（写在 spec 里的「之后」）。
2. **占位：** 没有；`Ticket {` 字面量的补全由编译器逐个指出。
3. **类型一致：** `Region`/`Waiver`（Task 1）→ Task 2 的 `verify`、Task 3 的 `waiver_for_tap`；`Refusal`（Task 3）。
4. **Review Focus：** 五条分别落在 Task 2（签名/档位/kind）、Task 1（凭据任一字段变则指纹变、JSON）、Task 3（边界、区域）。
