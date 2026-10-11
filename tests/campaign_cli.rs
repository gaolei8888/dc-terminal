//! `dct do` 只用批准时按过指纹签的活动文字：文件被改过 / 没签名 → 退出码 2，在碰 dco 之前。
//! 用软件钥匙冒充指纹钥匙，不碰真的 Touch ID；dco 根本没起（HOME 是空的临时目录）。
#![cfg(unix)]
use base64::{engine::general_purpose::STANDARD, Engine as _};
use dct_brain::canon::sha256_id;
use dct_brain::sign::soft::SoftSigner;
use dct_brain::sign::Signer;
use dct_brain::tier::SignerRole;
use std::path::Path;
use std::process::Command;

fn entry(s: &SoftSigner) -> serde_json::Value {
    let pk = s.public_key();
    serde_json::json!({"key_id": sha256_id(&pk), "public_key": STANDARD.encode(pk)})
}

/// HOME 里放好公钥（软件钥匙）和一份签过的活动 `launch`。
fn setup(home: &Path) {
    let user = SoftSigner::from_seed(SignerRole::User, 2);
    let auto = SoftSigner::from_seed(SignerRole::Auto, 1);
    let keys = home.join(".dct/keys");
    std::fs::create_dir_all(&keys).unwrap();
    std::fs::write(
        keys.join("public.json"),
        serde_json::json!({"auto": entry(&auto), "user": entry(&user)}).to_string(),
    )
    .unwrap();
    let per = dct::campaign::parse_params(&["--param".into(), "x.post_text=Hello from X".into()])
        .unwrap();
    dct::campaign::approve(
        &home.join(".dct/campaigns"),
        "launch",
        per,
        &mut |_| true,
        &user,
        1,
    )
    .unwrap();
}

fn run_do(home: &Path, json: bool) -> (i32, String, String) {
    let mut args = vec!["do", "post it", "--platform", "x", "--campaign", "launch"];
    if json {
        args.push("--json");
    }
    let o = Command::new(env!("CARGO_BIN_EXE_dct"))
        .args(args)
        .env("HOME", home)
        .stdin(std::process::Stdio::null())
        .output()
        .unwrap();
    (
        o.status.code().unwrap(),
        String::from_utf8_lossy(&o.stdout).into(),
        String::from_utf8_lossy(&o.stderr).into(),
    )
}

fn campaign_file(home: &Path) -> std::path::PathBuf {
    home.join(".dct/campaigns/launch.json")
}

#[test]
fn a_campaign_edited_after_approval_stops_dct_do_before_dco() {
    let h = tempfile::tempdir().unwrap();
    setup(h.path());
    let raw = std::fs::read_to_string(campaign_file(h.path())).unwrap();
    std::fs::write(
        campaign_file(h.path()),
        raw.replace("Hello from X", "Buy my coin"),
    )
    .unwrap();
    let (code, _, err) = run_do(h.path(), false);
    assert_eq!(code, 2, "{err}");
    assert!(err.contains("改过") && err.contains("重新批准"), "{err}");
    assert!(!h.path().join(".dco").exists(), "没碰 dco");
}

#[test]
fn tampered_campaign_in_json_mode_is_one_error_event() {
    let h = tempfile::tempdir().unwrap();
    setup(h.path());
    let raw = std::fs::read_to_string(campaign_file(h.path())).unwrap();
    std::fs::write(
        campaign_file(h.path()),
        raw.replace("Hello from X", "Buy my coin"),
    )
    .unwrap();
    let (code, out, _) = run_do(h.path(), true);
    assert_eq!(code, 2);
    let line = out.lines().next().expect(&out);
    let v: serde_json::Value = serde_json::from_str(line).unwrap();
    assert_eq!(v["event"], "error", "{out}");
    assert_eq!(v["exit"], 2, "{out}");
    assert!(v["message"].as_str().unwrap().contains("改过"), "{out}");
}

#[test]
fn an_unsigned_legacy_campaign_stops_dct_do() {
    let h = tempfile::tempdir().unwrap();
    setup(h.path());
    let mut v: serde_json::Value =
        serde_json::from_slice(&std::fs::read(campaign_file(h.path())).unwrap()).unwrap();
    v.as_object_mut().unwrap().remove("signature");
    std::fs::write(campaign_file(h.path()), v.to_string()).unwrap();
    let (code, _, err) = run_do(h.path(), false);
    assert_eq!(code, 2, "{err}");
    assert!(
        err.contains("没有指纹签名") && err.contains("dct campaign approve launch"),
        "{err}"
    );
    assert!(!h.path().join(".dco").exists());
}

#[test]
fn an_untouched_signed_campaign_passes_the_gate() {
    let h = tempfile::tempdir().unwrap();
    setup(h.path());
    // 过了活动这一关，才会去连 dco；dco 没起 → 退出码 1，话里是 dco 而不是活动。
    let (code, _, err) = run_do(h.path(), false);
    assert_eq!(code, 1, "{err}");
    assert!(!err.contains("指纹签名") && !err.contains("改过"), "{err}");
}
