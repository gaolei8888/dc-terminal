//! 「被外面杀掉之前，把终端还回去」。
//!
//! 界面跑在 raw mode 里。进程正常退出时 `TerminalGuard` 的 `Drop` 会还原，
//! 但被外面杀掉时 `Drop` 不跑——终端留在 raw mode 里，用户回到 shell 面对
//! 一个不回显、不换行的窗口，只能盲敲 `reset`。所以这条路必须单独铺。
//!
//! 两边都不走「在处理函数里直接干活」这条路，理由却不同：
//!
//! - Unix：信号处理函数里只能调 async-signal-safe 的东西，而 crossterm 的
//!   `disable_raw_mode()` 要锁一把全局 Mutex 去取原始 termios——信号打断的
//!   正好是持锁的主线程时就死锁。`sigwait` 在普通线程上下文里返回，这个
//!   约束整个消失。
//! - Windows：控制台处理函数由系统在一个**新线程**上调用，普通线程上下文，
//!   锁是安全的。真正的约束是时间——`CTRL_CLOSE_EVENT`（用户点了窗口的
//!   叉）之后系统只给几秒，还完终端就得立刻走，不能再做别的。
//!
//! 两边也都不走「置个标志位让主循环自己退」：主循环卡在 `client.call` 上
//! （守护进程死了、socket 不回）时永远轮不到下一个 tick，而那正是用户会去
//! 别的窗口 kill 的场景——恰好是最需要它工作的时候不工作。

/// 装一个「进程被外部终止时先还原终端」的钩子。`restore` 必须是幂等的：
/// 正常退出路径上 `TerminalGuard` 已经调过一次了。
pub fn restore_terminal_when_killed(restore: fn()) {
    imp::install(restore)
}

#[cfg(unix)]
mod imp {
    /// 屏蔽掩码会被子进程继承（`execve` 之后仍保留），但这里不用担心：TUI
    /// 进程在 `run()` 里不 fork 任何东西，PTY 全在守护进程里（`src/pty.rs`），
    /// 而守护进程在 `src/main.rs` 里早于 `ui::run` 就已经拉起。
    ///
    /// raw mode 下 Ctrl+C 不产生 SIGINT（termios 关了 ISIG），所以屏蔽 SIGINT
    /// 不影响 Ctrl+C 透传给 agent；这条只对外部 `kill -INT` 生效。
    pub fn install(restore: fn()) {
        let mut set: libc::sigset_t = unsafe { std::mem::zeroed() };
        unsafe {
            libc::sigemptyset(&mut set);
            libc::sigaddset(&mut set, libc::SIGTERM);
            libc::sigaddset(&mut set, libc::SIGINT);
            libc::sigaddset(&mut set, libc::SIGHUP);
            // 主线程先屏蔽，之后 spawn 出来的线程继承这份掩码，于是这三个
            // 信号只会被下面的 sigwait 取走，不会走默认处置直接杀进程。
            libc::pthread_sigmask(libc::SIG_BLOCK, &set, std::ptr::null_mut());
        }

        std::thread::spawn(move || {
            let mut signo: libc::c_int = 0;
            if unsafe { libc::sigwait(&set, &mut signo) } != 0 {
                return;
            }
            restore();
            // 不能用 `exit`：它会跑 atexit 和静态析构，而主线程此刻还在跑自己的
            // 事，两边可能同时清理终端或撞上同一把锁。终端已经在上一行还原好了，
            // 立刻走人。退出码 128 + signo 是 shell 惯例，SIGTERM 就是 143，
            // 脚本还能判断死因。
            unsafe { libc::_exit(128 + signo) };
        });
    }
}

#[cfg(windows)]
mod imp {
    use std::sync::OnceLock;
    use windows_sys::Win32::Foundation::BOOL;
    use windows_sys::Win32::System::Console::{
        SetConsoleCtrlHandler, CTRL_BREAK_EVENT, CTRL_CLOSE_EVENT, CTRL_C_EVENT, CTRL_LOGOFF_EVENT,
        CTRL_SHUTDOWN_EVENT,
    };
    use windows_sys::Win32::System::Threading::ExitProcess;

    /// 处理函数是系统调的，签名固定，塞不进闭包，也没有 `void*` 参数可以
    /// 带上下文——所以只能走一个静态量。`OnceLock` 而不是 `static mut`：
    /// 读它的是系统另起的线程，需要一个真的同步过的发布。
    static RESTORE: OnceLock<fn()> = OnceLock::new();

    unsafe extern "system" fn handler(event: u32) -> BOOL {
        match event {
            CTRL_C_EVENT | CTRL_BREAK_EVENT | CTRL_CLOSE_EVENT | CTRL_LOGOFF_EVENT
            | CTRL_SHUTDOWN_EVENT => {
                if let Some(f) = RESTORE.get() {
                    f();
                }
                // 与 Unix 那边的 `_exit` 对齐：不跑 atexit、不跑静态析构，
                // 主线程还在跑自己的事，两边可能同时清理终端。终端已经还
                // 干净了，立刻走人。
                //
                // 130 是 shell 里 Ctrl+C 的惯例值（128 + SIGINT）。Windows
                // 没有信号号，这里统一用它——脚本至少能看出「不是正常退出」。
                ExitProcess(130);
            }
            // 认不出来的事件交回系统默认处置。
            _ => 0,
        }
    }

    pub fn install(restore: fn()) {
        let _ = RESTORE.set(restore);
        // 第二个参数 1 = 装上（0 是卸掉）。失败只可能是系统资源问题，
        // 没有补救动作可做——装不上的后果是被杀时终端留在 raw mode，
        // 跟没有这个功能一样，不值得让界面起不来。
        unsafe { SetConsoleCtrlHandler(Some(handler), 1) };
    }
}
