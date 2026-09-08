//! 从系统剪贴板取图片。
//!
//! 终端本身传不了图片：你按下粘贴键时，终端只会把剪贴板里的**文字**发过来，
//! 剪贴板里是图的话什么都不会发生。所以 dct 自己去读剪贴板，把图片存成临时
//! 文件，再把文件路径当文字送给 agent —— agent 拿到路径就能读图。

use anyhow::Result;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};

static SEQ: AtomicU32 = AtomicU32::new(0);

/// 一次「粘图」的结果。**三种，不是两种。**
///
/// 原来的返回类型是 `Option<PathBuf>`，`None` 同时背着两个意思：
/// 「剪贴板里不是图」和「这个平台上 dct 根本读不了剪贴板」。前者是用户的
/// 状态，后者是这个构建的状态，而界面对两者说的是同一句「剪贴板里没有
/// 图片」——于是在 Linux（含容器）上，学生截了图、按了 F5，拿到的是一句
/// **听起来像是他自己没复制**的话。
///
/// 这个仓库对这种坏法有明确立场（`sys::proc`、`shell.rs` 里都是同一条：
/// 达不到同样强度的地方点名说清楚，不假装）。所以把这两件事分开命名，
/// 让调用方没法再用同一句话糊过去。
pub enum Pasted {
    /// 剪贴板里确实是图，已经存成文件了
    Image(PathBuf),
    /// 剪贴板里没有图（是文字、是空的）。**最常见的情况，不是异常**
    NoImage,
    /// **这个平台上读不了剪贴板里的图**，跟剪贴板里有什么无关
    Unsupported,
}

/// 读剪贴板这件事失败了。界面上只给码，句子由 `i18n::msg::error` 组。
#[cfg_attr(
    not(any(target_os = "macos", windows, test)),
    allow(dead_code, reason = "只有 mac/windows 那两支取图的路会用到；\
         别的平台直接答 Unsupported，但这几个函数照样编，免得跟着走散")
)]
fn read_failed() -> anyhow::Error {
    crate::proto::coded(crate::proto::ErrorCode::OperationFailed(
        crate::proto::Operation::ReadClipboard,
    ))
}

/// 存放粘贴出来的图片。放在系统临时目录里，不污染用户的项目。
#[cfg_attr(
    not(any(target_os = "macos", windows, test)),
    allow(dead_code, reason = "只有 mac/windows 那两支取图的路会用到；\
         别的平台直接答 Unsupported，但这几个函数照样编，免得跟着走散")
)]
fn paste_dir() -> PathBuf {
    std::env::temp_dir().join("dct-pastes")
}

/// 下一个还没人用过的 PNG 路径，顺带把目录建出来。
///
/// 名字里带 pid：同一台机器上可以同时开着好几个 dct 界面，光靠一个进程内的
/// 计数器，第二个界面第一次粘贴就会覆盖掉第一个界面刚存下的图——而那张图的
/// 路径可能已经躺在某个 agent 的输入框里了。
#[cfg_attr(
    not(any(target_os = "macos", windows, test)),
    allow(dead_code, reason = "只有 mac/windows 那两支取图的路会用到；\
         别的平台直接答 Unsupported，但这几个函数照样编，免得跟着走散")
)]
fn new_png_path() -> Result<PathBuf> {
    let dir = paste_dir();
    std::fs::create_dir_all(&dir).map_err(|_| read_failed())?;
    let n = SEQ.fetch_add(1, Ordering::SeqCst);
    Ok(dir.join(format!("paste-{}-{}.png", std::process::id(), n)))
}

/// 剪贴板里如果是图片，存成 PNG 并返回路径。见 [`Pasted`]。
#[cfg(target_os = "macos")]
pub fn image_to_file() -> Result<Pasted> {
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
        "OK" => Ok(Pasted::Image(path)),
        "NO_IMAGE" => Ok(Pasted::NoImage),
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
pub fn image_to_file() -> Result<Pasted> {
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
        "OK" => Ok(Pasted::Image(path)),
        "NO_IMAGE" => Ok(Pasted::NoImage),
        _ => match line.strip_prefix("FILE:") {
            Some(p) if !p.is_empty() => Ok(Pasted::Image(PathBuf::from(p))),
            _ => Err(read_failed()),
        },
    }
}

#[cfg(not(any(target_os = "macos", windows)))]
pub fn image_to_file() -> Result<Pasted> {
    // **答 `Unsupported`，不答 `NoImage`。** 这两个在界面上是两句不同的话，
    // 而这里的真相是后一句：dct 在这个平台上没实现读剪贴板，跟用户复制了
    // 什么没有关系。答 `NoImage` 就是把自己的没做完说成用户的操作失误。
    //
    // 远程版（浏览器里那个终端）也走这一支，而且**在那里补不上**：图片要
    // 从浏览器进来，得让 ttyd 的前端页面接住 paste 事件再传上来，而那一页
    // 是 ttyd 二进制里内嵌的 730 KB 单文件，换它等于把它整份 vendor 进来跟
    // 着版本走；那道门（`gate.rs`）又是一根刻意不解析 HTTP 正文的字节管道，
    // 在它身上改写页面会把那条「不解析所以没有走私问题」的保证一起拆掉。
    // 所以这一期的验收线就是这一句话说准，不是把功能补上。
    Ok(Pasted::Unsupported)
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

            assert!(matches!(image_to_file(), Ok(Pasted::NoImage)));
        }
        // Windows 上这条不自动化：唯一的做法是往用户**真的**剪贴板里写东西，
        // 跑一次测试就顺手清掉了开发者手里正拷着的内容。
    }

    /// 没实现取图的平台上必须答 `Unsupported`，**不能答 `NoImage`**。
    ///
    /// 这两个分支在界面上是两句不同的话，而 `NoImage` 那句
    /// （「剪贴板里没有图片」）在这里是假的——它把 dct 自己的没做完说成
    /// 用户的操作失误。容器里跑的正是这一支。
    #[cfg(not(any(target_os = "macos", windows)))]
    #[test]
    fn a_platform_without_image_paste_says_so_instead_of_blaming_the_clipboard() {
        assert!(matches!(image_to_file(), Ok(Pasted::Unsupported)));
    }
}
