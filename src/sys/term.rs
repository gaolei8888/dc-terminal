//! 进界面之前那几句问答，要的是一个「正常」的终端。
//!
//! dct 在拉起守护进程之前会在**普通终端**里问两句话（要不要接回上次的会话、
//! 要不要换掉旧的守护进程），用的是 `read_line`——一行一个回车，最朴素的那
//! 种读法。它有一个前提：终端处于行输入模式。
//!
//! 这个前提会被上一次的 dct 破坏。界面跑在 raw mode 里，正常退出时
//! `TerminalGuard` 会还原，被信号杀掉时 `sys::signal` 会还原——但**被硬杀
//! 时两条都不跑**：`kill -9`、任务管理器里「结束任务」、`TerminateProcess`、
//! 崩溃。那之后终端就一直停在 raw mode 里，而 raw mode 下键盘不回显、回车
//! 发的是 `\r` 不是 `\n`，于是下一次 dct 问话时：屏幕上什么都不出现，按
//! `y` 没反应，按回车也没反应，`read_line` 在等一个永远不会到的换行。
//!
//! 现场就是这么发作的一次。所以问话之前先把模式摆正——**这不是在还原什么，
//! 是在设置前提**：不管终端之前是什么样子，接下来这句问答需要行输入和回显。
//!
//! 一律尽力而为，出错就算了：stdin 不是终端（被重定向、被脚本调用）时这些
//! 调用注定失败，而那种场景下本来也没有人在敲键盘。

/// 把终端摆回「一行一个回车」的模式。
pub fn ensure_line_mode() {
    imp::ensure_line_mode()
}

#[cfg(unix)]
mod imp {
    /// crossterm 的 `disable_raw_mode` 认的是它自己存下来的那份原始 termios。
    /// 本进程没进过 raw mode 时它什么都不做——**那正好**：Unix 上留下 raw
    /// mode 的是上一个进程，而它的 termios 早随进程一起没了，我们能做的只有
    /// 「如果是我们自己搞的就还回去」。
    ///
    /// 真正把 Unix 上这个洞堵死要靠 `tcsetattr` 显式设一套 sane 值，那要把
    /// 一整套 termios 标志写进来。现场是在 Windows 上碰到的，先按平台各自
    /// 能做到的程度来；哪天 Unix 上真的撞见，再补那一段。
    pub fn ensure_line_mode() {
        let _ = crossterm::terminal::disable_raw_mode();
    }
}

#[cfg(windows)]
mod imp {
    use windows_sys::Win32::System::Console::{
        GetConsoleMode, GetStdHandle, SetConsoleMode, CONSOLE_MODE, ENABLE_ECHO_INPUT,
        ENABLE_LINE_INPUT, ENABLE_PROCESSED_INPUT, ENABLE_VIRTUAL_TERMINAL_INPUT, STD_INPUT_HANDLE,
    };

    /// Windows 上的控制台模式是**控制台自己的属性**，不是进程的——所以上一个
    /// 进程留下的设置，这个进程读得到也改得回。这跟 Unix 那边不一样，也是为
    /// 什么这一侧能真正把洞堵上：不需要知道原来是什么样，直接把这次要用的
    /// 三个标志打开。
    ///
    /// 顺手关掉 `ENABLE_VIRTUAL_TERMINAL_INPUT`：raw mode 会打开它，开着的
    /// 时候按键以转义序列的形式送进来，跟行输入不是一回事。
    pub fn ensure_line_mode() {
        unsafe {
            let h = GetStdHandle(STD_INPUT_HANDLE);
            if h.is_null() || h == windows_sys::Win32::Foundation::INVALID_HANDLE_VALUE {
                return;
            }
            let mut mode: CONSOLE_MODE = 0;
            // 失败说明 stdin 不是控制台（管道、重定向），那就没有模式可言。
            if GetConsoleMode(h, &mut mode) == 0 {
                return;
            }
            mode |= ENABLE_LINE_INPUT | ENABLE_ECHO_INPUT | ENABLE_PROCESSED_INPUT;
            mode &= !ENABLE_VIRTUAL_TERMINAL_INPUT;
            SetConsoleMode(h, mode);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 跑测试时 stdin 通常不是控制台。这条钉的是「那种情况下也不能炸」——
    /// 这个函数从头到尾都是尽力而为。
    #[test]
    fn is_a_no_op_when_stdin_is_not_a_terminal() {
        ensure_line_mode();
        ensure_line_mode();
    }
}
