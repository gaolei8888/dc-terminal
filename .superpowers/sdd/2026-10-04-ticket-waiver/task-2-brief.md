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

