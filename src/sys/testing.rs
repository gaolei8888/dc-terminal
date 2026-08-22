//! 测试夹具要用的 POSIX 小工具，在 Windows 上从哪儿来。
//!
//! 一大批测试是靠 `/bin/sh -c "echo X; sleep 5"` 这种脚本摆现场的：要一个
//! 会说话、会停住、会按要求崩掉的子进程。这些脚本用的是 sh 的语义——`$*`、
//! `$VAR`、`sleep 0.3`、`clear`——改写成 `cmd.exe` 的说法不是翻译，是重写，
//! 而重写过的夹具很容易「因为别的原因通过」，那比不测还糟。
//!
//! 所以 Windows 上不改脚本，改的是**去哪儿找 sh**：Git for Windows 自带一整
//! 套（`<Git>\usr\bin\sh.exe`、`cat.exe`、`sleep.exe`……）。这不是一个额外的
//! 依赖——dct 本来就要 git 才能工作（每轮对话前的快照是 shell 出去调 git 做
//! 的），所以凡是 dct 跑得起来的 Windows 机器，这些工具就一定在。
//!
//! 只在 `cfg(test)` 下编译：产品代码一个字节都不该知道这件事。

// 只有 Windows 那半边拼得到路径；Unix 上问的是 PATH，拿回来就是字符串。
#[cfg(windows)]
use std::path::PathBuf;

/// 一个 POSIX 小工具的完整路径。`name` 写不带扩展名的那个（`sh`、`cat`、
/// `sleep`、`true`、`echo`）。
///
/// 找不到就 panic，而不是让测试静静地跳过：跳过的测试和通过的测试在输出
/// 里长得一模一样，而这里「找不到」意味着这台机器上根本跑不了 dct。
pub fn tool(name: &str) -> String {
    match locate(name) {
        Some(p) => p,
        None => panic!(
            "找不到 POSIX 工具 `{name}`。Windows 上这些夹具借用 Git for Windows \
             自带的那一套（<Git>\\usr\\bin），装了 git 就有——而 dct 本来就要 git。"
        ),
    }
}

/// `/bin/sh` 的等价物。用得太多，单开一个名字。
pub fn sh() -> String {
    tool("sh")
}

#[cfg(unix)]
fn locate(name: &str) -> Option<String> {
    // Unix 上就是 PATH 上的那一个。`sh` 特殊：夹具里原本写死 `/bin/sh`，
    // 而 PATH 上的 `sh` 可能是别的东西（某些发行版把它指向 dash 之外的
    // shell），保持原来的那一个，免得脚本行为跟着变。
    if name == "sh" {
        return Some("/bin/sh".to_string());
    }
    super::fs::find_command(name).map(|p| p.display().to_string())
}

#[cfg(windows)]
fn locate(name: &str) -> Option<String> {
    let exe = format!("{name}.exe");
    // git.exe 通常在 `<Git>\cmd\git.exe`，工具在 `<Git>\usr\bin\`。
    let git = super::fs::find_command("git")?;
    let root = git.parent()?.parent()?;
    let p: PathBuf = root.join("usr").join("bin").join(&exe);
    if p.is_file() {
        return Some(p.display().to_string());
    }
    // 退一步：PATH 上直接有同名的也行（比如用户自己装了 busybox，或者
    // 把 Git 的 usr\bin 加进了 PATH）。
    super::fs::find_command(&exe).map(|p| p.display().to_string())
}

/// Windows 上借来的这个 sh 要用登录 shell 起。
///
/// 它是 MSYS 的 bash，PATH 从 Windows 环境继承过来——里面没有 `/usr/bin`。
/// 后果很阴：`echo` 是**内建**命令，照常有输出，所以夹具看上去在跑；而
/// `sleep`、`clear` 是外部程序，一律 command not found。于是脚本说了话、
/// 没睡觉、没清屏，测试拿到的是一个「跑了一半」的现场，而失败信息指向的是
/// 状态判定，跟真实原因隔着十万八千里。`-l` 让它读 `/etc/profile`，那里把
/// PATH 摆正。
///
/// Unix 上不加，也不该加：登录 shell 会 source 用户自己的 rc 文件，那是一份
/// 不受测试控制的输入——`ui::tests` 那条滚屏测试就是被真实的 `~/.zshrc`
/// 拖成假红的。
fn login_flags() -> &'static [&'static str] {
    if cfg!(windows) {
        &["-l"]
    } else {
        &[]
    }
}

/// `sh -c <脚本>` 的完整 argv。夹具要一个会说话、会停住、会按要求崩掉的
/// 子进程时，用这个。
pub fn sh_c(script: &str) -> Vec<String> {
    let mut v = vec![sh()];
    v.extend(login_flags().iter().map(|f| f.to_string()));
    v.push("-c".to_string());
    v.push(script.to_string());
    v
}

/// 把夹具 TOML 里写的 `/bin/sh` 换成这台机器上真正的那一个，顺带补上
/// [`login_flags`]。
///
/// TOML 的基本字符串里反斜杠是转义符，而 Windows 上这个路径长成
/// `C:\Program Files\Git\usr\bin\sh.exe`——原样塞进去，TOML 解析器看到的是
/// `\P`、`\G` 这些非法转义，报的错跟真实原因毫无关系。所以这里连转义一起做。
pub fn toml_with_sh(toml: &str) -> String {
    let escaped = sh().replace('\\', "\\\\");
    let flags: String = login_flags().iter().map(|f| format!(", \"{f}\"")).collect();
    // 连 `-c` 一起匹配，是为了把 `-l` 插在它**前面**——这里的参数顺序是死的：
    // `-c` 之后的一切都算脚本内容。夹具里没有这个形状时退回只换程序名，
    // 那种情况下脚本大概也用不着外部命令。
    let with_flags = toml.replace(
        "[\"/bin/sh\", \"-c\"",
        &format!("[\"{escaped}\"{flags}, \"-c\""),
    );
    with_flags.replace("/bin/sh", &escaped)
}

/// `sh <参数…>` 的完整 argv，不带 `-c`。给那种要一个**交互** shell 的夹具
/// （比如要它画出提示符再等输入），脚本形式的 [`sh_c`] 不适用。
///
/// **这里不加 `-l`**，跟 [`sh_c`] 相反。交互夹具靠 `PS1` 钉死一个固定的提示
/// 符，等它出现就知道 shell 已经能收输入了；而登录 shell 会读 `/etc/profile`，
/// 那里会把 `PS1` 换成它自己那一套，于是那个提示符永远等不到——测试挂在
/// 「提示符一直没出来」上，而真实原因是多了一个参数。
///
/// 代价是这种夹具里用不了外部命令（Windows 上 PATH 里没有 `/usr/bin`，见
/// [`login_flags`]）。目前用它的那条只敲内建命令，够用。
pub fn sh_argv(args: &[&str]) -> Vec<String> {
    let mut v = vec![sh()];
    v.extend(args.iter().map(|a| a.to_string()));
    v
}
