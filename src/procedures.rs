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
use std::io::Write;
use std::path::PathBuf;

/// 「提前多久执行」最多能写多远：30 天。
const MAX_START_IN: u64 = 30 * 24 * 3600;
/// 票的有效窗口最长：24 小时。
const MAX_WINDOW: u64 = 24 * 3600;

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
                let n_raw = s["n"].as_u64().ok_or_else(|| anyhow!("步骤缺 n"))?;
                let n: u32 = n_raw.try_into().map_err(|_| anyhow!("步骤序号 {n_raw} 超出范围"))?;
                let arg = s["arg"].as_str().ok_or_else(|| anyhow!("第{n}步缺 arg"))?.to_string();
                Ok(Step {
                    n,
                    action: s["action"].as_str().ok_or_else(|| anyhow!("第{n}步缺 action"))?.to_string(),
                    arg,
                    done_when: s["done_when"]["arg"].as_str().map(String::from),
                })
            })
            .collect::<Result<Vec<_>>>()?;
        let mut seen = BTreeSet::new();
        for s in &steps {
            if !seen.insert(s.n) {
                bail!("第{}步的序号重复了", s.n);
            }
        }
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

    /// 批准记录里可能带着「哪一步该改低哪一档」这种敏感判断，目录 0700、
    /// 文件 0600，跟钥匙文件一个待遇——走仓库里现成的那套私有文件助手，
    /// 不自己另起一套权限逻辑。
    pub fn save(&self, sa: &SignedApproval) -> Result<()> {
        std::fs::create_dir_all(&self.dir).with_context(|| format!("建批准记录目录 {} 失败", self.dir.display()))?;
        crate::sys::fs::restrict_dir_to_owner(&self.dir)
            .with_context(|| format!("批准记录目录 {} 的权限设置失败", self.dir.display()))?;
        let path = self.path(&sa.approval.steps_sha256);
        let mut f = crate::sys::fs::create_private(&path).with_context(|| format!("写批准记录 {} 失败", path.display()))?;
        f.write_all(&serde_json::to_vec_pretty(sa)?)
            .with_context(|| format!("写批准记录 {} 失败", path.display()))?;
        Ok(())
    }

    pub fn load(&self, steps_sha256: &str) -> Result<Option<SignedApproval>> {
        let path = self.path(steps_sha256);
        match std::fs::read(&path) {
            Ok(b) => Ok(Some(serde_json::from_slice(&b).context("批准记录坏了")?)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e).with_context(|| format!("读批准记录 {} 失败", path.display())),
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

/// Touch ID 弹窗里步骤描述的摘要：太长的参数截断，别把整段文案糊满屏幕。
fn arg_summary(arg: &str) -> String {
    const MAX_CHARS: usize = 20;
    let s = arg.trim();
    let mut chars = s.chars();
    let head: String = chars.by_ref().take(MAX_CHARS).collect();
    if chars.next().is_some() {
        format!("{head}…")
    } else {
        head
    }
}

/// Touch ID 弹窗里的人话：改过档位的步骤点名「原为 X，改为 Y」，没改过、
/// 但档位不低（物理/对外/动钱）的步骤点名「是 X」，其余的步骤既没改也不
/// 严格，笼统带过——**笼统带过的那句话必须站得住**：能落进这个桶的步骤
/// 只可能是「只读」或「只影响自己」，不能说成别的。
fn approval_reason(name: &str, steps: &[Step], proposed: &[Tier], final_tiers: &[Tier], overrides: &[(u32, Tier)]) -> String {
    let overridden: BTreeSet<u32> = overrides.iter().map(|(n, _)| *n).collect();
    let parts: Vec<String> = steps
        .iter()
        .enumerate()
        .filter_map(|(i, s)| {
            if overridden.contains(&s.n) {
                Some(format!(
                    "第{}步「{}」原为{}，改为{}",
                    s.n,
                    arg_summary(&s.arg),
                    tier_zh(proposed[i]),
                    tier_zh(final_tiers[i])
                ))
            } else if final_tiers[i] >= Tier::Physical {
                Some(format!("第{}步「{}」是{}", s.n, arg_summary(&s.arg), tier_zh(final_tiers[i])))
            } else {
                None
            }
        })
        .collect();
    if parts.is_empty() {
        format!("批准流程 {name}：每一步都只读或只影响自己")
    } else {
        format!("批准流程 {name}：{}，其余只读或只影响自己", parts.join("，"))
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
    let shown = Shown::parse(full_name, &dcv.show(full_name).with_context(|| format!("读流程 {full_name} 失败"))?)?;
    let steps_sha = steps_sha256(&shown.params, &shown.steps);
    let proposed: Vec<Tier> = shown.steps.iter().map(|s| tier_for_step(&s.action, &s.arg)).collect();
    let mut tiers = proposed.clone();
    for (n, t) in overrides {
        let i = shown
            .steps
            .iter()
            .position(|s| s.n == *n)
            .ok_or_else(|| anyhow!("这条流程没有第{n}步"))?;
        // 动钱的步骤只能维持原档或改得更严，不能改低——这一档不是「提议」，
        // 是硬约束（控制层设计）。
        if proposed[i] == Tier::Money && *t < Tier::Money {
            bail!("第{n}步涉及付钱，不能改低");
        }
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
    let reason = approval_reason(full_name, &shown.steps, &proposed, &tiers, overrides);
    let signed = sign_approval(approval, &user, &reason).map_err(|e| anyhow!("{e}"))?;
    let approved_body = dcv.approve(full_name).with_context(|| format!("确认流程 {full_name} 失败"))?;
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

/// 参数名字的集合印成人话，不是 Rust 的 `{:?}` 调试语法——那不是给不懂
/// 编程的人看的。`BTreeSet` 本来就按字母序，直接拼接即可。
fn join_names(names: &BTreeSet<&str>) -> String {
    if names.is_empty() {
        "不需要参数".to_string()
    } else {
        names.iter().copied().collect::<Vec<_>>().join("、")
    }
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
    if start_in > MAX_START_IN {
        bail!("能提前多久执行最多 30 天，给的是 {start_in} 秒");
    }
    if window > MAX_WINDOW {
        bail!("有效期最长 24 小时，给的是 {window} 秒");
    }
    let shown = Shown::parse(full_name, &dcv.show(full_name).with_context(|| format!("读流程 {full_name} 失败"))?)?;
    if !shown.executable {
        bail!("这条流程现在不能执行：{}", shown.why_not.as_deref().unwrap_or("dcv 没说原因"));
    }
    let declared: BTreeSet<&str> = shown.params.iter().map(String::as_str).collect();
    let given: BTreeSet<&str> = params.keys().map(String::as_str).collect();
    if declared != given {
        bail!("参数对不上：流程要的是 {}，给的是 {}", join_names(&declared), join_names(&given));
    }
    let steps_sha = steps_sha256(&shown.params, &shown.steps);
    let not_approved = || anyhow!("这条流程还没批准，或者改过了，先运行 dct procedure approve {full_name}");
    let sa = approvals.load(&steps_sha)?.ok_or_else(not_approved)?;
    verify_approval(&sa, &keys.trusted()?, full_name, &steps_sha).map_err(|_| not_approved())?;
    let tier = sa.approval.run_tier();
    // 时间只能往上加，绝不能悄悄绕回去：溢出算错误，不算「立刻生效」。
    let earliest = now.checked_add(start_in).ok_or_else(|| anyhow!("时间算不出来了：现在时间加上提前量超出了范围"))?;
    let expires = earliest.checked_add(window).ok_or_else(|| anyhow!("时间算不出来了：结束时间超出了范围"))?;
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
        expires,
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
        .map(|(s, _)| format!("第{}步", s.n))
        .collect();
    // 有效期从什么时候开始、能用多久，弹窗里得说清楚，别让人瞎猜「现在按了
    // 是不是马上就执行」。
    let period = if start_in == 0 {
        format!("从现在起 {window} 秒内有效")
    } else {
        format!("{start_in} 秒后开始，此后 {window} 秒内有效")
    };
    let reason = if strict.is_empty() {
        format!("执行流程 {full_name}：{period}")
    } else {
        format!("执行流程 {full_name}（{}对外）：{period}", strict.join("、"))
    };
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
                    if params.insert(k.to_string(), v.to_string()).is_some() {
                        bail!("参数 {k} 给了不止一次");
                    }
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
                // 按步骤真正的编号打印，不是数组下标——两者在有跳号的流程里不是一回事。
                let shown = Shown::parse(name, &RealDcv.show(name).with_context(|| format!("批准之后重新读流程 {name} 失败"))?)?;
                for (s, t) in shown.steps.iter().zip(sa.approval.step_tiers.iter()) {
                    println!("第{}步：{}", s.n, tier_zh(*t));
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
        let e = ticket(&w.dcv, &w.keys, &w.se, &w.approvals, "social:edit-bio", "mac-lei", extra, 0, 600, 2000).unwrap_err();
        assert!(!e.to_string().contains('{'), "参数不对的提示不该带 Rust 的调试花括号：{e}");
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

    #[test]
    fn a_positive_start_in_pushes_earliest_and_expires_forward() {
        let w = world();
        approve(&w.dcv, &w.keys, &w.se, &w.approvals, "social:edit-bio", &[], 1000).unwrap();
        *w.dcv.show.borrow_mut() = edit_bio(true);
        let params = BTreeMap::from([("bio".to_string(), "x".to_string())]);
        let st = ticket(&w.dcv, &w.keys, &w.se, &w.approvals, "social:edit-bio", "mac-lei", params, 300, 600, 2000).unwrap();
        assert_eq!(st.ticket.earliest, 2300);
        assert_eq!(st.ticket.expires, 2900);
    }

    #[test]
    fn a_procedure_changed_after_approval_gets_no_ticket() {
        let w = world();
        approve(&w.dcv, &w.keys, &w.se, &w.approvals, "social:edit-bio", &[], 1000).unwrap();
        let mut changed = edit_bio(true);
        changed["steps"][3]["arg"] = json!("发布");
        *w.dcv.show.borrow_mut() = changed;
        let params = BTreeMap::from([("bio".to_string(), "x".to_string())]);
        let e = ticket(&w.dcv, &w.keys, &w.se, &w.approvals, "social:edit-bio", "mac-lei", params, 0, 600, 2000).unwrap_err();
        assert!(e.to_string().contains("先运行 dct procedure approve"), "{e}");
    }

    #[test]
    fn an_approval_for_one_procedure_does_not_authorize_another_with_the_same_steps() {
        let w = world();
        approve(&w.dcv, &w.keys, &w.se, &w.approvals, "social:edit-bio", &[], 1000).unwrap();
        *w.dcv.show.borrow_mut() = edit_bio(true);
        let params = BTreeMap::from([("bio".to_string(), "x".to_string())]);
        let e = ticket(&w.dcv, &w.keys, &w.se, &w.approvals, "social:edit-bio-copy", "mac-lei", params, 0, 600, 2000).unwrap_err();
        assert!(e.to_string().contains("先运行 dct procedure approve"), "{e}");
    }

    #[test]
    fn an_override_of_a_nonexistent_step_is_an_error() {
        let w = world();
        assert!(approve(&w.dcv, &w.keys, &w.se, &w.approvals, "social:edit-bio", &[(99, Tier::Content)], 1000).is_err());
    }

    #[test]
    fn lowering_a_money_step_is_refused() {
        let w = world();
        let mut v = edit_bio(false);
        v["steps"][3]["arg"] = json!("立即支付");
        *w.dcv.show.borrow_mut() = v;
        let e = approve(&w.dcv, &w.keys, &w.se, &w.approvals, "social:edit-bio", &[(4, Tier::Content)], 1000).unwrap_err();
        assert!(e.to_string().contains("涉及付钱") && e.to_string().contains("不能改低"), "{e}");
        assert_eq!(w.dcv.approve_calls.get(), 0, "钱档被拒的改动不该走到 dcv 那一步");
    }

    #[test]
    fn raising_a_money_step_still_works() {
        let w = world();
        let mut v = edit_bio(false);
        v["steps"][3]["arg"] = json!("立即支付");
        *w.dcv.show.borrow_mut() = v;
        let sa = approve(&w.dcv, &w.keys, &w.se, &w.approvals, "social:edit-bio", &[(4, Tier::Money)], 1000).unwrap();
        assert_eq!(sa.approval.step_tiers[3], Tier::Money);
    }

    #[test]
    fn the_prompt_names_overridden_steps() {
        let w = world();
        approve(&w.dcv, &w.keys, &w.se, &w.approvals, "social:edit-bio", &[(4, Tier::Money)], 1000).unwrap();
        let reasons = w.se.reasons.borrow();
        assert!(
            reasons[0].contains("第4步") && reasons[0].contains("原为对外") && reasons[0].contains("改为动钱"),
            "{reasons:?}"
        );
    }

    #[test]
    fn window_and_start_in_over_their_caps_are_rejected() {
        let w = world();
        approve(&w.dcv, &w.keys, &w.se, &w.approvals, "social:edit-bio", &[], 1000).unwrap();
        *w.dcv.show.borrow_mut() = edit_bio(true);
        let params = BTreeMap::from([("bio".to_string(), "x".to_string())]);
        let e1 = ticket(&w.dcv, &w.keys, &w.se, &w.approvals, "social:edit-bio", "mac-lei", params.clone(), 2_592_001, 600, 2000)
            .unwrap_err();
        assert!(e1.to_string().contains("30 天"), "{e1}");
        let e2 = ticket(&w.dcv, &w.keys, &w.se, &w.approvals, "social:edit-bio", "mac-lei", params, 0, 86_401, 2000).unwrap_err();
        assert!(e2.to_string().contains("24 小时"), "{e2}");
    }

    #[test]
    fn shown_parse_rejects_a_step_number_outside_u32() {
        let mut v = edit_bio(true);
        v["steps"][0]["n"] = json!(9_000_000_000_u64);
        assert!(Shown::parse("social:edit-bio", &v).is_err());
    }

    #[test]
    fn shown_parse_rejects_a_missing_arg() {
        let mut v = edit_bio(true);
        v["steps"][0].as_object_mut().unwrap().remove("arg");
        assert!(Shown::parse("social:edit-bio", &v).is_err());
    }

    #[test]
    fn shown_parse_rejects_duplicate_step_numbers() {
        let mut v = edit_bio(true);
        v["steps"][1]["n"] = json!(1);
        assert!(Shown::parse("social:edit-bio", &v).is_err());
    }
}
