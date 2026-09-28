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
