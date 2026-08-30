//! 守护进程的生死。
//!
//! 这是两个系统差得最远的一处，而且差的不是 API 名字，是能不能表达一个
//! 意思：Unix 有 SIGTERM——「请你自己收拾干净再走」；Windows 没有这句话。
//! 一个没有控制台的后台进程（我们的守护进程正是）收不到任何 Ctrl 事件，
//! 剩下的只有 `TerminateProcess`，也就是 Unix 的 SIGKILL：立刻停，不给
//! 任何代码执行的机会。
//!
//! 后果不能靠这一层解决，只能在这里说清楚：守护进程被换掉时，它来不及
//! 关掉自己手里的 pty，也就来不及让每个 agent 自己收尾。
//!
//! 那些 agent 进程会不会因此变成孤儿，取决于 ConPTY：伪控制台的句柄随
//! 进程一起被内核关掉时，挂在上面的客户端进程应当跟着结束。**这一条是
//! 按文档推的，还没有在真机上验过**——如果实测发现换一次守护进程就漏
//! 一批 agent 进程，补法是给每个会话的子进程套一个带
//! `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE` 的 job object，那是 Windows 上
//! 「父死子亡」唯一硬保证。别在验之前先写：没验过的兜底代码会盖住真正
//! 的症状。
use std::io;
use std::process::{Child, Command};

/// 拉起一个脱离当前终端的子进程。
///
/// Unix 上是 `setsid`：不这么做的话它跟 TUI 在同一个 session 里，关掉终端
/// 窗口时 SIGHUP 会把它一起带走——而「关掉窗口不影响会话」正是这个产品
/// 存在的理由。Windows 上对应的是 `DETACHED_PROCESS`：不继承调用者的控制台，
/// 于是关掉那个窗口不会波及它。
pub fn spawn_detached(cmd: &mut Command) -> io::Result<Child> {
    imp::detach(cmd);
    imp::spawn(cmd)
}

/// 别让这个子进程弹出控制台窗口。
///
/// 守护进程自己是 `DETACHED_PROCESS` 起来的（见上），也就是**它没有控制台**。
/// 而 Windows 上一个没有控制台的进程去起一个控制台程序时，内核会替那个孩子
/// 新开一个控制台——连着窗口。于是每发一条消息，检查点那几次 `git.exe` 就在
/// 屏幕上闪几个黑框。`CREATE_NO_WINDOW` 是「给它控制台，但别给窗口」那一句。
///
/// 凡是守护进程会起的短命子进程都得加上这个：git、走 CLI 的那个模型后端。
/// pty 里的 agent 不在此列——那条路走 ConPTY，伪控制台本来就没有窗口。
///
/// Unix 上没有这个概念，是空操作。
pub fn no_console(cmd: &mut Command) -> &mut Command {
    imp::no_console(cmd);
    cmd
}

/// 这个进程还在不在。
pub fn alive(pid: u32) -> bool {
    imp::alive(pid)
}

/// 请它走。Unix 上是 SIGTERM，进程有机会自己收尾；**Windows 上没有这个
/// 中间档**，直接等同于 [`hard_kill`]，见本文件开头。
pub fn ask_to_stop(pid: u32) {
    imp::ask_to_stop(pid)
}

/// 立刻停，不给宽限期。
pub fn hard_kill(pid: u32) {
    imp::hard_kill(pid)
}

#[cfg(unix)]
mod imp {
    use std::process::{Child, Command};

    /// Unix 上没有 Windows 那个句柄继承的坑（`exec` 之后除了 fd 0/1/2 之外
    /// 的描述符都带着 `CLOEXEC`），起就是起。
    pub fn spawn(cmd: &mut Command) -> std::io::Result<Child> {
        cmd.spawn()
    }

    pub fn detach(cmd: &mut Command) {
        use std::os::unix::process::CommandExt;
        // SAFETY: pre_exec 里只调用 setsid()——async-signal-safe，不分配内存，
        // 不碰锁。fork 和 exec 之间只能做这一类事。
        unsafe {
            cmd.pre_exec(|| {
                if libc::setsid() == -1 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
    }

    pub fn no_console(_cmd: &mut Command) {}

    pub fn alive(pid: u32) -> bool {
        // 0 号信号不发信号，只问「这个进程还在不在」。
        unsafe { libc::kill(pid as i32, 0) == 0 }
    }

    pub fn ask_to_stop(pid: u32) {
        unsafe { libc::kill(pid as i32, libc::SIGTERM) };
    }

    pub fn hard_kill(pid: u32) {
        unsafe { libc::kill(pid as i32, libc::SIGKILL) };
    }
}

#[cfg(windows)]
mod imp {
    use std::process::{Child, Command};
    use windows_sys::Win32::Foundation::{
        CloseHandle, GetHandleInformation, SetHandleInformation, HANDLE, HANDLE_FLAG_INHERIT,
        INVALID_HANDLE_VALUE,
    };
    use windows_sys::Win32::System::Console::{
        GetStdHandle, STD_ERROR_HANDLE, STD_INPUT_HANDLE, STD_OUTPUT_HANDLE,
    };
    use windows_sys::Win32::System::Threading::{
        GetExitCodeProcess, OpenProcess, TerminateProcess, CREATE_NEW_PROCESS_GROUP,
        CREATE_NO_WINDOW, DETACHED_PROCESS, PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_TERMINATE,
    };

    /// `GetExitCodeProcess` 用来表示「还在跑」的那个值。它同时是一个**合法的
    /// 退出码**——一个真的以 259 退出的进程会被认成还活着。这个歧义是
    /// Windows API 自带的，没有绕开的办法；我们只拿它判守护进程，而守护
    /// 进程的退出码由我们自己写，不会是 259。
    const STILL_ACTIVE: u32 = 259;

    pub fn detach(cmd: &mut Command) {
        use std::os::windows::process::CommandExt;
        // CREATE_NEW_PROCESS_GROUP 一起加：不然它会跟着调用者收到 Ctrl+C，
        // 而「按 Ctrl+C 退出界面」不该顺手打断后台的会话。
        cmd.creation_flags(DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP);
    }

    /// 起这个子进程，**但别把我们自己那几根管子递给它**。
    ///
    /// Windows 上 `CreateProcess` 必须以 `bInheritHandles=TRUE` 调用，标准库
    /// 才能把子进程的三个标准句柄递过去。可 TRUE 的含义不是"递这三个"，而是
    /// **"我手上每一个标记了可继承的句柄，孩子都拿一份"**。于是即使我们已经
    /// 把守护进程的 stdin/stdout/stderr 全指到 NUL（见 `client::spawn_daemon`），
    /// 它照样会拿到**我们自己的** stdout——而我们的 stdout 很可能是别人的管子。
    ///
    /// 症状是这样的：`dct restart -y | tee log`，或者任何一个用管道收 dct
    /// 输出的脚本，会在 dct 早就退出之后继续挂着，因为管子的写端还在那个
    /// 刚被拉起来、准备活好几天的守护进程手里。它永远不会关。
    ///
    /// （这是被 `restart_with_yes_swaps_the_daemon_process` 逮住的：那条测试
    /// 用 `Command::output()` 收输出，于是整轮测试卡死，还每次留下一个野
    /// 守护进程。它就是这件事的回归测试。）
    ///
    /// 所以起之前把那三个句柄的可继承标志摘掉，起完立刻装回去。**必须装
    /// 回去**：标准库的 `Stdio::inherit()` 靠的就是这个标志，一直摘着的话，
    /// 这个进程之后再想让某个子进程共用自己的终端就会失败。
    pub fn spawn(cmd: &mut Command) -> std::io::Result<Child> {
        let saved = take_inheritance();
        let out = cmd.spawn();
        for (h, flags) in saved {
            unsafe { SetHandleInformation(h, HANDLE_FLAG_INHERIT, flags) };
        }
        out
    }

    /// 把三个标准句柄的可继承标志摘掉，返回原来的值好装回去。
    ///
    /// 拿不到句柄、或者它本来就不可继承的，一律跳过——不记就不会去改它，
    /// 也就不会把一个本来没有的标志"还"上去。
    fn take_inheritance() -> Vec<(HANDLE, u32)> {
        let mut saved = Vec::new();
        for which in [STD_INPUT_HANDLE, STD_OUTPUT_HANDLE, STD_ERROR_HANDLE] {
            unsafe {
                let h = GetStdHandle(which);
                if h.is_null() || h == INVALID_HANDLE_VALUE {
                    continue;
                }
                let mut flags = 0u32;
                if GetHandleInformation(h, &mut flags) == 0 || flags & HANDLE_FLAG_INHERIT == 0 {
                    continue;
                }
                if SetHandleInformation(h, HANDLE_FLAG_INHERIT, 0) != 0 {
                    saved.push((h, flags & HANDLE_FLAG_INHERIT));
                }
            }
        }
        saved
    }

    pub fn no_console(cmd: &mut Command) {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }

    /// 句柄拿不到就当它不在了。拿不到的常见原因正是「这个 pid 已经没了」；
    /// 另一种是权限不够，但守护进程是当前用户自己起的，那一种不会发生。
    fn with_handle<T>(pid: u32, access: u32, f: impl FnOnce(isize) -> T, none: T) -> T {
        let h = unsafe { OpenProcess(access, 0, pid) };
        if h.is_null() {
            return none;
        }
        let out = f(h as isize);
        unsafe { CloseHandle(h) };
        out
    }

    pub fn alive(pid: u32) -> bool {
        with_handle(
            pid,
            PROCESS_QUERY_LIMITED_INFORMATION,
            |h| {
                let mut code: u32 = 0;
                let ok = unsafe { GetExitCodeProcess(h as _, &mut code) };
                ok != 0 && code == STILL_ACTIVE
            },
            false,
        )
    }

    pub fn ask_to_stop(pid: u32) {
        hard_kill(pid)
    }

    pub fn hard_kill(pid: u32) {
        with_handle(
            pid,
            PROCESS_TERMINATE,
            |h| {
                // 退出码 1：随便挑的非零值，只是别让被强杀的进程看起来
                // 像是正常退出的。
                unsafe { TerminateProcess(h as _, 1) };
            },
            (),
        )
    }
}
