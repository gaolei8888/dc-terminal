//! 从系统剪贴板取图片。
//!
//! 终端本身传不了图片：你按下 Cmd+V 时，终端只会把剪贴板里的**文字**发过来，
//! 剪贴板里是图的话什么都不会发生。所以 dct 自己去读剪贴板，把图片存成临时
//! 文件，再把文件路径当文字送给 agent —— agent 拿到路径就能读图。

use anyhow::{bail, Context, Result};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};

static SEQ: AtomicU32 = AtomicU32::new(0);

/// 存放粘贴出来的图片。放在系统临时目录里，不污染用户的项目。
fn paste_dir() -> PathBuf {
    std::env::temp_dir().join("dct-pastes")
}

/// 剪贴板里如果是图片，存成 PNG 并返回路径；不是图片返回 `None`。
#[cfg(target_os = "macos")]
pub fn image_to_file() -> Result<Option<PathBuf>> {
    let dir = paste_dir();
    std::fs::create_dir_all(&dir).context("建不了临时目录")?;

    let n = SEQ.fetch_add(1, Ordering::SeqCst);
    let path = dir.join(format!("paste-{}-{}.png", std::process::id(), n));
    let path_str = path.to_str().context("临时文件路径不是合法 UTF-8")?;

    let script = format!(
        r#"set outFile to POSIX file "{path_str}"
try
    set imgData to the clipboard as «class PNGf»
on error
    return "NO_IMAGE"
end try
set fh to open for access outFile with write permission
set eof fh to 0
write imgData to fh
close access fh
return "OK""#
    );

    let out = std::process::Command::new("osascript")
        .arg("-e")
        .arg(&script)
        .output()
        .context("调用 osascript 失败")?;

    let stdout = String::from_utf8_lossy(&out.stdout);
    if stdout.trim() == "OK" {
        Ok(Some(path))
    } else if stdout.trim() == "NO_IMAGE" {
        Ok(None)
    } else {
        let err = String::from_utf8_lossy(&out.stderr);
        bail!("读剪贴板失败: {}", err.trim())
    }
}

#[cfg(not(target_os = "macos"))]
pub fn image_to_file() -> Result<Option<PathBuf>> {
    // 其它平台暂不支持；返回 None 让调用方退回普通文字粘贴。
    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paste_dir_is_under_temp() {
        assert!(paste_dir().starts_with(std::env::temp_dir()));
    }

    /// 剪贴板里没有图时必须安静地返回 None，而不是报错——
    /// 用户按了粘贴键但剪贴板里是文字，这是最常见的情况，不是异常。
    #[test]
    fn no_image_is_not_an_error() {
        // 先把剪贴板设成纯文字
        #[cfg(target_os = "macos")]
        {
            use std::io::Write;
            let mut c = std::process::Command::new("pbcopy")
                .stdin(std::process::Stdio::piped())
                .spawn()
                .unwrap();
            c.stdin.as_mut().unwrap().write_all(b"just text").unwrap();
            c.wait().unwrap();

            assert!(matches!(image_to_file(), Ok(None)));
        }
    }
}
