//! 从系统剪贴板取图片。
//!
//! 终端本身传不了图片：你按下粘贴键时，终端只会把剪贴板里的**文字**发过来，
//! 剪贴板里是图的话什么都不会发生。所以 dct 自己去读剪贴板，把图片存成临时
//! 文件，再把文件路径当文字送给 agent —— agent 拿到路径就能读图。

use anyhow::Result;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};

static SEQ: AtomicU32 = AtomicU32::new(0);

/// 读剪贴板这件事失败了。界面上只给码，句子由 `i18n::msg::error` 组。
fn read_failed() -> anyhow::Error {
    crate::proto::coded(crate::proto::ErrorCode::OperationFailed(
        crate::proto::Operation::ReadClipboard,
    ))
}

/// 存放粘贴出来的图片。放在系统临时目录里，不污染用户的项目。
fn paste_dir() -> PathBuf {
    std::env::temp_dir().join("dct-pastes")
}

/// 下一个还没人用过的 PNG 路径，顺带把目录建出来。
///
/// 名字里带 pid：同一台机器上可以同时开着好几个 dct 界面，光靠一个进程内的
/// 计数器，第二个界面第一次粘贴就会覆盖掉第一个界面刚存下的图——而那张图的
/// 路径可能已经躺在某个 agent 的输入框里了。
fn new_png_path() -> Result<PathBuf> {
    let dir = paste_dir();
    std::fs::create_dir_all(&dir).map_err(|_| read_failed())?;
    let n = SEQ.fetch_add(1, Ordering::SeqCst);
    Ok(dir.join(format!("paste-{}-{}.png", std::process::id(), n)))
}

/// 剪贴板里如果是图片，存成 PNG 并返回路径；不是图片返回 `None`。
#[cfg(target_os = "macos")]
pub fn image_to_file() -> Result<Option<PathBuf>> {
    let path = new_png_path()?;
    let path_str = path.to_str().ok_or_else(read_failed)?;

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
        .map_err(|_| read_failed())?;

    match String::from_utf8_lossy(&out.stdout).trim() {
        "OK" => Ok(Some(path)),
        "NO_IMAGE" => Ok(None),
        _ => Err(read_failed()),
    }
}

/// Windows 上取图的那段 PowerShell。
///
/// 为什么是 PowerShell 而不是直接调 Win32：拿一张剪贴板里的位图存成 PNG，
/// 走 API 是「开剪贴板、认格式、解 DIB 头、自己编码 PNG」一串活，而
/// `System.Windows.Forms.Clipboard` 已经把这一串做完了，代价只是起一个
/// 短命进程（几百毫秒，用户按一次键等得起）。macOS 那条路走 `osascript`
/// 也是同一个取舍。
///
/// **这段脚本里一个双引号都没有，全用单引号。** Rust 把整段当一个参数交给
/// `powershell.exe -Command`，而它拿到的其实是一整条命令行字符串、要自己
/// 再拆一次——脚本里的双引号在这一步会被吃掉，留下一条语法不对的命令。
/// 单引号在 PowerShell 里是等价的字符串写法，穿过去毫发无损。
///
/// **目标路径也不拼进脚本**，走环境变量 `DCT_PASTE_PATH`：拼进去就得回答
/// 「路径里带一个单引号怎么办」，而环境变量这条路上没有任何转义。
///
/// 三种剪贴板内容按顺序试：
/// 1. `PNG` 格式——截图工具（Win+Shift+S）会同时放一份真的 PNG 字节，
///    原样落盘，不重编码也不丢透明通道；
/// 2. 位图——从旧程序拷来的图只有 DIB，交给 `GetImage` 再存成 PNG；
/// 3. 文件列表——在资源管理器里拷了一个图片文件。这一类不用另存，
///    直接把它自己的路径给 agent。**只认单个文件、且后缀是图片**：
///    拷了一个 `.zip` 也回一条路径的话，用户按的是「粘贴图片」，
///    拿到的却是一句让 agent 去读压缩包的指令。
#[cfg(windows)]
const READ_CLIPBOARD_PS1: &str = concat!(
    "$ErrorActionPreference='Stop';",
    "Add-Type -AssemblyName System.Windows.Forms,System.Drawing;",
    "$out=$env:DCT_PASTE_PATH;",
    "$d=[Windows.Forms.Clipboard]::GetDataObject();",
    "if($d -and $d.GetDataPresent('PNG')){",
    "$s=$d.GetData('PNG');$s.Position=0;",
    "$f=[IO.File]::Create($out);$s.CopyTo($f);$f.Close();",
    "Write-Output 'OK'",
    "}elseif([Windows.Forms.Clipboard]::ContainsImage()){",
    "$i=[Windows.Forms.Clipboard]::GetImage();",
    "$i.Save($out,[Drawing.Imaging.ImageFormat]::Png);$i.Dispose();",
    "Write-Output 'OK'",
    "}elseif([Windows.Forms.Clipboard]::ContainsFileDropList()){",
    "$p=[Windows.Forms.Clipboard]::GetFileDropList();",
    "$ok=@('.png','.jpg','.jpeg','.gif','.bmp','.webp');",
    "if($p.Count -eq 1 -and $ok -contains [IO.Path]::GetExtension($p[0]).ToLower()){",
    "Write-Output ('FILE:'+$p[0])",
    "}else{Write-Output 'NO_IMAGE'}",
    "}else{Write-Output 'NO_IMAGE'}",
);

#[cfg(windows)]
pub fn image_to_file() -> Result<Option<PathBuf>> {
    let path = new_png_path()?;

    let mut cmd = std::process::Command::new("powershell.exe");
    cmd.args([
        "-NoProfile",
        "-NonInteractive",
        // 剪贴板是 COM 里的单线程套间对象：MTA 线程上读它拿到的是一个
        // 「线程状态无效」的异常，而不是空剪贴板。控制台版 PowerShell
        // 默认已经是 STA，写出来是为了不依赖那个默认。
        "-Sta",
        "-Command",
        READ_CLIPBOARD_PS1,
    ])
    .env("DCT_PASTE_PATH", &path);
    // 界面进程自己是有控制台的，而 Windows 会让子进程继承它——一个继承了
    // 我们这块控制台的 PowerShell 一旦往屏幕上写点什么，写的就是 TUI 正
    // 画着的那一屏。`CREATE_NO_WINDOW` 给它一块自己的、没有窗口的控制台。
    crate::sys::proc::no_console(&mut cmd);

    let out = cmd.output().map_err(|_| read_failed())?;

    // 失败时**不往 stderr 印诊断**：这时候 TUI 正占着备用屏，印出去的每
    // 一个字都落在画面上，而且 ratatui 只重画有变化的格子，那片脏字会一直
    // 留在那儿。用户拿到的是底栏上那句「读不了剪贴板」。
    let stdout = String::from_utf8_lossy(&out.stdout);
    let line = stdout.trim();
    match line {
        "OK" => Ok(Some(path)),
        "NO_IMAGE" => Ok(None),
        _ => match line.strip_prefix("FILE:") {
            Some(p) if !p.is_empty() => Ok(Some(PathBuf::from(p))),
            _ => Err(read_failed()),
        },
    }
}

#[cfg(not(any(target_os = "macos", windows)))]
pub fn image_to_file() -> Result<Option<PathBuf>> {
    // 其它平台暂不支持；返回 None 让调用方给出「剪贴板里没有图片」那句话。
    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paste_dir_is_under_temp() {
        assert!(paste_dir().starts_with(std::env::temp_dir()));
    }

    /// 同一个进程里连着粘两张图，第二张不能盖掉第一张——第一张的路径
    /// 可能已经在 agent 的输入框里了。
    #[test]
    fn each_paste_gets_its_own_path() {
        let a = new_png_path().unwrap();
        let b = new_png_path().unwrap();
        assert_ne!(a, b);
    }

    /// 那段 PowerShell 的不变量，理由见 `READ_CLIPBOARD_PS1` 的文档：
    /// 双引号活不过 `powershell.exe -Command` 那一层的重新拆分。加一句
    /// 字符串时最容易顺手打的就是双引号，这条守卫替人记着。
    #[cfg(windows)]
    #[test]
    fn the_clipboard_script_has_no_double_quotes() {
        assert!(!READ_CLIPBOARD_PS1.contains('"'));
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
        // Windows 上这条不自动化：唯一的做法是往用户**真的**剪贴板里写东西，
        // 跑一次测试就顺手清掉了开发者手里正拷着的内容。
    }
}
