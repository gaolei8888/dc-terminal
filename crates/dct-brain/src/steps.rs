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

    // A–D 四组用例来自 dcv（dc-vault 会话用独立的 Python 实现算出，dct 这边另算一遍核对过）。

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

    // 用例 D：dcv 2.1 的 read_value（「标签 → 指标名」）。算法没变，只是新动作词。
    #[test]
    fn case_d_read_value_steps() {
        let params = vec!["account".to_string()];
        let steps = vec![
            step(1, "open_app", "X", None),
            step(2, "navigate_by_intent", "搜索 → {account} → 主页", None),
            step(3, "read_value", "「关注者」 → followers", Some("主页上显示「关注者」")),
            step(4, "read_value", "正在关注 → following", None),
        ];
        assert_eq!(
            steps_sha256(&params, &steps),
            "sha256:27e24c912984ad0ed99db4ba066db17cb3e7e667d70913532d9960329d147912"
        );
        assert!(steps.iter().filter(|s| s.action == "read_value").all(|s| crate::tier::tier_for_step(&s.action, &s.arg) == crate::tier::Tier::Read));
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
