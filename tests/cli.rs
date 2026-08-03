use std::process::Command;

#[test]
fn daemon_subcommand_is_recognized() {
    // --help 必须提到 daemon 子命令
    let out = Command::new(env!("CARGO_BIN_EXE_dct"))
        .arg("--help")
        .output()
        .unwrap();
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(text.contains("daemon"), "帮助里应当有 daemon: {text}");
}

#[test]
fn unknown_subcommand_exits_nonzero() {
    let out = Command::new(env!("CARGO_BIN_EXE_dct"))
        .arg("bogus")
        .output()
        .unwrap();
    assert!(!out.status.success());
}
