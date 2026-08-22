//! 这台机器上的 shell，以及「要启动的那个东西，怎么才真的启动得起来」。
//!
//! 后半句在 Unix 上不成其为问题：`execve` 认可执行位，`#!` 那一行由内核
//! 负责。Windows 没有这两样——`CreateProcess` 只会启动真正的可执行映像，
//! 交给它一个 `.cmd` 会直接失败。而 npm 装出来的 CLI 恰恰全是 `.cmd`
//! （`claude`、`codex`、`qwen` 在 Windows 上都是），也就是说少了这一层，
//! 菜单上认得出的 agent 一个都起不来。
use super::fs::command_exists;
// 只有 Windows 那半边要把名字换成真实路径；Unix 上 `launch_argv` 原样奉还，
// 用不上它。
#[cfg(windows)]
use super::fs::find_command;

/// 内置「命令行」profile 要跑的那个 shell。
pub fn login_shell() -> String {
    imp::login_shell()
}

/// 把 profile 里写的那条命令，翻译成这台机器上真能启动的那条。
///
/// Unix 上原样奉还。Windows 上做两件事：把名字换成 `find_command` 找到的
/// 那个真实路径（`claude` → `...npm/claude.cmd`），以及在它是脚本时前面
/// 垫上 `cmd.exe /c`。
pub fn launch_argv(argv: &[String]) -> Vec<String> {
    imp::launch_argv(argv)
}

#[cfg(unix)]
mod imp {
    use super::*;

    /// `$SHELL` 排第一：那是用户自己选的登录 shell，daemon 从用户的 shell
    /// 里起，这个变量跟着环境一路进来。后面三个是兜底，按「像不像一个人
    /// 愿意用的交互 shell」排；`/bin/sh` 在任何 Unix 上都在，所以最后那个
    /// `unwrap_or` 只是类型上的收尾，不是一条真会走到的路。
    ///
    /// 查不到也照样返回 `/bin/sh` 而不是空串：让 spawn 去报「起不来」，比
    /// 在菜单上写「没安装」离事实更近——用户装不了 `/bin/sh`，那句提示没有
    /// 出口。
    pub fn login_shell() -> String {
        if let Ok(s) = std::env::var("SHELL") {
            if !s.is_empty() && command_exists(&s) {
                return s;
            }
        }
        ["/bin/bash", "/bin/zsh", "/bin/sh"]
            .into_iter()
            .find(|c| command_exists(c))
            .unwrap_or("/bin/sh")
            .to_string()
    }

    pub fn launch_argv(argv: &[String]) -> Vec<String> {
        argv.to_vec()
    }
}

#[cfg(windows)]
mod imp {
    use super::*;

    /// cmd.exe 的位置从 `%ComSpec%` 问，不写死 `C:\Windows\System32\cmd.exe`：
    /// 系统盘不一定是 C:，`%SystemRoot%` 也不一定叫 Windows。
    fn comspec() -> String {
        std::env::var("ComSpec").unwrap_or_else(|_| "cmd.exe".to_string())
    }

    /// PowerShell 排在 cmd.exe 前面：`dct` 的用户在这个窗口里要敲的是
    /// `git`、`npm`、`cargo`，PowerShell 的补全和历史都强得多，而且它是
    /// Windows 10 起系统自带的那个终端里的默认。`pwsh`（PowerShell 7）又
    /// 排在 `powershell`（5.1）前面：装了新的说明是特意装的。
    ///
    /// `$SHELL` 仍然先问一句。它在 Windows 上不是本地概念，但 Git Bash /
    /// MSYS 环境里会有——而那种情况下它的值是 `/usr/bin/bash` 这种
    /// CreateProcess 起不来的路径，`command_exists` 会否掉它，自动落到下面。
    /// 也就是说这一问只在它**真的指向一个能起来的程序**时才生效。
    pub fn login_shell() -> String {
        if let Ok(s) = std::env::var("SHELL") {
            if !s.is_empty() && command_exists(&s) {
                return s;
            }
        }
        for c in ["pwsh.exe", "powershell.exe"] {
            if command_exists(c) {
                return c.to_string();
            }
        }
        comspec()
    }

    fn is_script(p: &std::path::Path) -> bool {
        p.extension().is_some_and(|e| {
            let e = e.to_string_lossy();
            e.eq_ignore_ascii_case("cmd") || e.eq_ignore_ascii_case("bat")
        })
    }

    pub fn launch_argv(argv: &[String]) -> Vec<String> {
        let Some(first) = argv.first() else {
            return argv.to_vec();
        };
        // 找不到就原样交出去。这条路上不该出现「找不到」——菜单在这之前
        // 已经用同一个 `find_command` 判过「装没装」了——但万一环境在两次
        // 之间变了，让 spawn 去报错，比在这里悄悄改写成别的东西强。
        let Some(path) = find_command(first) else {
            return argv.to_vec();
        };

        let mut out = Vec::new();
        if is_script(&path) {
            // `.cmd` / `.bat` 不是可执行映像，CreateProcess 起不来，只能由
            // cmd.exe 来解释。`/c` 是「跑完这条就退出」。
            out.push(comspec());
            out.push("/c".to_string());
        }
        // 用找到的绝对路径，不用用户敲的那个名字：portable-pty 自己那套查找
        // 不认 PATHEXT，把 `claude` 交给它只会得到「找不到」。
        out.push(path.to_string_lossy().into_owned());
        out.extend(argv[1..].iter().cloned());
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 「命令行」这一项必须指着本机真的能起来的那个 shell。
    ///
    /// 写死 `/bin/zsh` 的那一版在 Ubuntu 上整行是灰的（Windows 走 WSL 之后
    /// 那就是默认发行版），而它是唯一 `is_agent = false` 的内置项——它一灰，
    /// 在不是 git 仓库的目录里九项就一项都开不了，dct 整个看上去是坏的。
    /// 这条测试在三个平台上都跑，问的是同一句话：这个名字，`spawn` 得起来吗。
    #[test]
    fn the_login_shell_exists_on_this_machine() {
        let s = login_shell();
        assert!(!s.is_empty(), "登录 shell 不能是空串");
        assert!(command_exists(&s), "登录 shell 指向 {s}，本机起不来");
    }

    /// 翻译不能把参数弄丢，也不能把参数和命令名搞混。
    ///
    /// 用当前测试二进制自己当那个「命令」：它一定存在、一定可执行，而且
    /// 在两个平台上都是一个真正的可执行映像（不是脚本），所以两边的期望
    /// 是同一个——`argv[0]` 被换成某个能启动的东西，后面的参数原样跟着。
    #[test]
    fn launch_argv_keeps_the_arguments() {
        let exe = std::env::current_exe().unwrap();
        let argv = vec![
            exe.to_string_lossy().into_owned(),
            "--第一个".to_string(),
            "带 空格 的".to_string(),
        ];
        let out = launch_argv(&argv);
        assert!(!out.is_empty());
        assert_eq!(
            &out[out.len() - 2..],
            &["--第一个".to_string(), "带 空格 的".to_string()],
            "参数必须原样在最后，顺序不能变"
        );
        assert!(
            command_exists(&out[0]),
            "翻译出来的头一个必须是能启动的：{}",
            out[0]
        );
    }
}
