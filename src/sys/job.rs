//! 「父死子亡」这条保证从哪儿来。
//!
//! Unix 上它是白送的：pty 的从端是会话的控制终端，`kill()` 打的那一下落在
//! 前台进程组上，主端一关内核还会补一轮 SIGHUP。孙子进程跑不掉。
//!
//! Windows 上一样都没有。而且这里的进程链**天生比 Unix 深一层**：
//! `sys::shell::launch_argv` 把 `claude` 翻译成 `cmd.exe /c ...\claude.CMD`，
//! 所以我们 spawn 的那个直接子进程是 `cmd.exe`，真正的 agent 是它的孩子。
//! `TerminateProcess` 只认句柄，不认血缘——杀掉 `cmd.exe` 之后 `claude.exe`
//! 原地不动，继续占着 API 配额、继续锁着 `claude.exe` 这个文件（别的会话的
//! 自动更新会因此报 `claude.exe in use` 失败）。
//!
//! **这一条是 2026-09-05 在真机上验出来的，不是照文档推的**：`dct kill 3`
//! 打印了 `Killed session 3`、`dct ps` 立刻变 `stopped`，而 `claude.exe`
//! 还在进程表里活得好好的。`sys::proc` 开头那段注释预告过这个洞，也写明了
//! 补法就是这里这个 job object——它当时说「别在验之前先写」，现在验过了。
//!
//! job object 是 Windows 上唯一一个**按血缘**而不是按句柄的杀法：进程一旦
//! 被圈进去，它之后创建的所有子孙自动也在圈里，`TerminateJobObject` 一下
//! 全带走。`JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE` 再补一道：句柄关掉（包括
//! 守护进程自己被 `TerminateProcess` 时内核替它关的那一下）圈里的进程一起
//! 走，所以换守护进程不会漏一批 agent 出来——那正是 `sys::proc` 开头担心
//! 但没法确认的那件事。
//!
//! **有一个够不着的缝**，说清楚而不假装没有：`CreateProcess` 返回和我们
//! 调用 `AssignProcessToJobObject` 之间有几微秒，那期间那个子进程如果已经
//! 生出了孩子，孩子不在圈里。堵死它要用 `CREATE_SUSPENDED` 起进程、圈好
//! 再 resume，而进程是 portable-pty 起的，那个口子它没开。实际上够不着：
//! `cmd.exe` 光是启动、读命令行就要几十毫秒，而我们是在同一个函数里紧接着
//! 圈的。

pub use imp::Job;

#[cfg(unix)]
mod imp {
    /// Unix 上不需要这层东西（见模块头），所以这里没有一个能构造出来的值。
    ///
    /// 不做成「空实现的 job」而是让 [`Job::confine`] 永远返回 `None`：空实现
    /// 会让调用点看起来两个平台都上了保险，而事实是 Unix 的保险在别处（pty
    /// 的进程组），写成 `None` 才对得上。
    pub struct Job(());

    impl Job {
        pub fn confine(_pid: u32) -> Option<Job> {
            None
        }

        pub fn terminate(&self) {}
    }
}

#[cfg(windows)]
mod imp {
    use std::ffi::c_void;

    use windows_sys::Win32::Foundation::{CloseHandle, HANDLE};
    use windows_sys::Win32::System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, JobObjectExtendedLimitInformation,
        SetInformationJobObject, TerminateJobObject, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
        JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
    };
    use windows_sys::Win32::System::Threading::{
        OpenProcess, PROCESS_SET_QUOTA, PROCESS_TERMINATE,
    };

    /// 一个会话的整棵进程树。
    pub struct Job(HANDLE);

    // SAFETY: 里面是一个内核对象的句柄。句柄是进程范围的、跟线程无关，
    // 上面用到的三个 API（Assign/Terminate/Close）本身都是线程安全的。
    // 需要这两个 impl 是因为 `PtySession` 要跨线程走（守护进程每个会话
    // 一个读线程），而裸指针默认既不 Send 也不 Sync。
    unsafe impl Send for Job {}
    unsafe impl Sync for Job {}

    impl Job {
        /// 把 `pid` 和它此后的所有子孙圈进一个新的 job。
        ///
        /// 每个会话一个 job，不共用一个全局的：`TerminateJobObject` 是整圈
        /// 一起杀，共用的话停一个会话会把所有会话带走。
        ///
        /// 圈不上就返回 `None`，不报错、不重试。调用方（`PtySession::spawn`）
        /// 拿到 `None` 时会话照常起——**少一层保险不该等于起不来**。真圈不上
        /// 的现实原因只有一个：这台机器上的进程已经在一个不许 breakaway 的
        /// 老式 job 里（Win7 那套语义，Win8 起支持嵌套就没有这回事了）。
        pub fn confine(pid: u32) -> Option<Job> {
            unsafe {
                let job = CreateJobObjectW(std::ptr::null(), std::ptr::null());
                if job.is_null() {
                    return None;
                }

                let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
                info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
                let set = SetInformationJobObject(
                    job,
                    JobObjectExtendedLimitInformation,
                    &info as *const _ as *const c_void,
                    std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
                );
                if set == 0 {
                    // 没设上 KILL_ON_JOB_CLOSE 的 job 是个陷阱：它照样能
                    // Assign、照样能 Terminate，但守护进程被强杀那条路上
                    // 不再兜底。宁可不要，也不要一个只保一半的。
                    CloseHandle(job);
                    return None;
                }

                // 只要这两个权限：SET_QUOTA 是 Assign 要的，TERMINATE 是
                // 圈里的进程被杀时要的。
                let proc = OpenProcess(PROCESS_SET_QUOTA | PROCESS_TERMINATE, 0, pid);
                if proc.is_null() {
                    CloseHandle(job);
                    return None;
                }
                let assigned = AssignProcessToJobObject(job, proc);
                CloseHandle(proc);
                if assigned == 0 {
                    CloseHandle(job);
                    return None;
                }

                Some(Job(job))
            }
        }

        /// 整圈一起杀，立刻，不给宽限期。
        ///
        /// 退出码 1 跟 `sys::proc::hard_kill` 保持一致：随便挑的非零值，
        /// 只是别让被强杀的进程看起来像是正常退出的。
        pub fn terminate(&self) {
            unsafe {
                TerminateJobObject(self.0, 1);
            }
        }
    }

    impl Drop for Job {
        fn drop(&mut self) {
            // `KILL_ON_JOB_CLOSE` 挂在这一下上：最后一个句柄关掉时，圈里
            // 还活着的进程一起走。所以这不只是「还个句柄」。
            unsafe {
                CloseHandle(self.0);
            }
        }
    }
}
