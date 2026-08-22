//! socket 能开会话、能往会话里发任意输入——谁连得上，谁就能以你的身份
//! 在这台机器上执行任意命令。所以目录和 socket 都必须只有属主可访问。

//! **这一整个文件只在 Unix 上有意义**：它验的是权限位这个机制本身。
//! Windows 上同一道门是目录的 ACL，而且拦人的位置也不同（AF_UNIX 在那边
//! connect 时不校验文件权限）——不同的机制要另写一组断言，硬套 0600 只会
//! 得到一条永远红的测试。理由的全文在 `sys::ipc` 的 `after_bind`。
#![cfg(unix)]

use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::thread::sleep;
use std::time::Duration;

fn mode_of(p: &std::path::Path) -> u32 {
    std::fs::metadata(p).unwrap().permissions().mode() & 0o777
}

#[test]
fn socket_and_dir_are_owner_only() {
    let dir = tempfile::tempdir().unwrap();
    let sock: PathBuf = dir.path().join("sub").join("daemon.sock");
    let s = sock.clone();
    std::thread::spawn(move || {
        let _ = dct::daemon::run(&s);
    });
    for _ in 0..60 {
        if sock.exists() {
            break;
        }
        sleep(Duration::from_millis(50));
    }
    assert!(sock.exists(), "socket 没建出来");

    assert_eq!(
        mode_of(sock.parent().unwrap()),
        0o700,
        "socket 所在目录必须只有属主能进"
    );
    assert_eq!(mode_of(&sock), 0o600, "socket 本身必须只有属主能连");
}
