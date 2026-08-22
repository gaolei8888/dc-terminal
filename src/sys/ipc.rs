//! 界面和守护进程之间那条线。
//!
//! Unix 是 domain socket，一个摆在 `~/.dct/daemon.sock` 的文件。Windows 从
//! 10 1803 起也有 AF_UNIX，同样是一个路径——所以这一层选的是它，而不是命名
//! 管道：路径式的 socket 让「socket 在哪，别的状态文件就在哪」这条规矩
//! （`projects.json`、`secrets.toml`、`last-sessions.toml` 全靠
//! `store_path_for_socket` 之类从它推出来）在两个平台上原样成立，测试把
//! socket 放进临时目录就自动隔离，也原样成立。标准库没给 Windows 开这个
//! 口子，`uds_windows` 补上，形状跟 `std::os::unix::net` 一模一样。
//!
//! 两处 Windows 达不到 Unix 强度的地方，各自在下面点名：谁能连（`bind_private`）
//! 和对面是谁（`peer_pid`）。
use std::io;
use std::path::Path;

#[cfg(unix)]
pub use std::os::unix::net::{UnixListener as Listener, UnixStream as Stream};
#[cfg(windows)]
pub use uds_windows::{UnixListener as Listener, UnixStream as Stream};

/// 建一个只有属主能连的 socket。
///
/// 权限必须收紧。这个 socket 能开会话、能往会话里发任意输入——**谁连得上，
/// 谁就能在这台机器上以你的身份执行任意命令**。
pub fn bind_private(socket: &Path) -> io::Result<Listener> {
    if let Some(parent) = socket.parent() {
        std::fs::create_dir_all(parent)?;
        // 目录先收紧再 bind：socket 文件是在这个目录里出生的。
        crate::sys::fs::restrict_dir_to_owner(parent)?;
    }
    // 上一个守护进程死掉时留下的那个文件还摆在那，不删就 bind 不上。
    let _ = std::fs::remove_file(socket);
    let listener = Listener::bind(socket)?;
    imp::after_bind(socket)?;
    Ok(listener)
}

#[cfg(unix)]
mod imp {
    use super::*;

    pub fn after_bind(socket: &Path) -> io::Result<()> {
        // 默认的 0755 意味着同机器的其它账号都能连。
        crate::sys::fs::restrict_file_to_owner(socket)
    }

    /// 内核记着是谁 bind 的这个 socket，这是唯一一个不需要对面同意的问法。
    /// macOS 和 Linux 是同一件事、两个名字。
    #[cfg(target_os = "macos")]
    pub fn peer_pid(stream: &Stream) -> Option<u32> {
        use std::os::unix::io::AsRawFd;
        let fd = stream.as_raw_fd();
        let mut pid: libc::pid_t = 0;
        let mut len = std::mem::size_of::<libc::pid_t>() as libc::socklen_t;
        let rc = unsafe {
            libc::getsockopt(
                fd,
                libc::SOL_LOCAL,
                libc::LOCAL_PEERPID,
                &mut pid as *mut _ as *mut libc::c_void,
                &mut len,
            )
        };
        (rc == 0 && pid > 0).then_some(pid as u32)
    }

    #[cfg(target_os = "linux")]
    pub fn peer_pid(stream: &Stream) -> Option<u32> {
        use std::os::unix::io::AsRawFd;
        let fd = stream.as_raw_fd();
        let mut cred: libc::ucred = unsafe { std::mem::zeroed() };
        let mut len = std::mem::size_of::<libc::ucred>() as libc::socklen_t;
        let rc = unsafe {
            libc::getsockopt(
                fd,
                libc::SOL_SOCKET,
                libc::SO_PEERCRED,
                &mut cred as *mut _ as *mut libc::c_void,
                &mut len,
            )
        };
        (rc == 0 && cred.pid > 0).then_some(cred.pid as u32)
    }

    /// 剩下的 Unix 没实现。返回 `None` 的后果是 `restart_daemon` 认不出旧
    /// 守护进程、于是拒绝动手——比拿一个猜出来的 pid 去 kill 强。
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    pub fn peer_pid(_stream: &Stream) -> Option<u32> {
        None
    }
}

#[cfg(windows)]
mod imp {
    use super::*;

    /// 守护进程把自己的 pid 摆在 socket 旁边。
    fn pid_path(socket: &Path) -> std::path::PathBuf {
        let mut s = socket.as_os_str().to_os_string();
        s.push(".pid");
        std::path::PathBuf::from(s)
    }

    /// socket 文件本身不再单独设 ACL：AF_UNIX 的那个文件在 Windows 上是个
    /// 重解析点，`SetNamedSecurityInfoW` 对它的行为没有保证。**而且就算设了
    /// 也不顶用**——Windows 的 AF_UNIX 不像 Unix 那样在 connect 时校验文件
    /// 权限。这里真正拦人的是上一层目录的 ACL：进不了 `~/.dct`，就没法用
    /// 这条路径去 connect。
    ///
    /// 这是这一层里 Windows 比 Unix 弱的第一处：Unix 是文件权限位直接挡在
    /// connect 上，Windows 是靠目录挡住「走到这个路径」。同一个用户下的
    /// 另一个进程，两边都拦不住（那本来也不是这道门要防的）。
    pub fn after_bind(socket: &Path) -> io::Result<()> {
        std::fs::write(pid_path(socket), std::process::id().to_string())
    }

    /// Windows 的 AF_UNIX 没有 `SO_PEERCRED` 那样的对应物——内核不告诉你
    /// 对面是谁。于是这里退一步：读守护进程自己在 `bind` 时写下的那个 pid
    /// 文件。**这也是为什么这一侧的入口要多带一个 socket 路径**：Unix 那边
    /// 从连接本身就问得出来，这边只有路径这一条线索。
    ///
    /// 这是**靠对面配合**的答案，比 Unix 那边弱一档。之所以在 Windows 上
    /// 可以接受：这个平台上不存在「老到不认识新机制的守护进程」——AF_UNIX
    /// 这条路和这个 pid 文件是同一次移植里一起出生的，能 bind 上的都写了它。
    ///
    /// 拿到 pid 之后还要过两道：进程还活着，而且它**就是当初写下这个文件的
    /// 那个进程**。第二道不能省——调用方拿这个 pid 去 kill，而 pid 是会被
    /// 系统回收再分配的。一个陈旧的 pid 文件加上一次回收，就是「dct 升级把
    /// 用户正在跑的别的程序杀了」。
    pub fn peer_pid_at(socket: &Path) -> Option<u32> {
        let path = pid_path(socket);
        let pid: u32 = std::fs::read_to_string(&path).ok()?.trim().parse().ok()?;
        if !crate::sys::proc::alive(pid) {
            return None;
        }
        started_before(pid, &path).then_some(pid)
    }

    /// 这个进程是不是在那个 pid 文件写下**之前**就已经在跑了。
    ///
    /// 这是「它是不是当初那个进程」的判据。看的是时间不是名字：名字靠不住，
    /// 守护进程完全可能是从另一个位置、另一个文件名起的（现场那次就是
    /// `target/release/dct` 对上 `~/.local/bin/dct`；测试里更是特意把二进制
    /// 复制成一个独一无二的名字，好让收尾时的清理不误伤开发机上真正在跑的
    /// 那个）。而时间是硬的：文件是那个进程起来之后写的，所以「它的启动时间
    /// 早于文件的修改时间」永远成立；一个回收来的 pid 只可能是在文件写下
    /// 之后才启动的，必然被这一条挡住。
    ///
    /// 留 2 秒余量：两个时间戳虽然出自同一口钟，但一个来自内核的进程表、
    /// 一个来自文件系统，没必要赌它们在毫秒级上完全一致。这点余量不影响
    /// 结论——pid 回收要经过整整一轮 pid 空间，绝不会发生在两秒内。
    fn started_before(pid: u32, pidfile: &Path) -> bool {
        use std::time::{Duration, SystemTime};
        use windows_sys::Win32::Foundation::{CloseHandle, FILETIME};
        use windows_sys::Win32::System::Threading::{
            GetProcessTimes, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
        };

        let Ok(written) = pidfile.metadata().and_then(|m| m.modified()) else {
            return false;
        };

        let mut created: FILETIME = unsafe { std::mem::zeroed() };
        let mut ignored: [FILETIME; 3] = unsafe { std::mem::zeroed() };
        let ok = unsafe {
            let h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
            if h.is_null() {
                return false;
            }
            let ok = GetProcessTimes(
                h,
                &mut created,
                &mut ignored[0],
                &mut ignored[1],
                &mut ignored[2],
            );
            CloseHandle(h);
            ok
        };
        if ok == 0 {
            return false;
        }

        // FILETIME 是 1601-01-01 起的 100 纳秒数；UNIX 纪元比它晚
        // 11644473600 秒。启动时间早于纪元是不可能的，出现就当认不出来。
        const EPOCH_DIFF_SECS: u64 = 11_644_473_600;
        let ticks = ((created.dwHighDateTime as u64) << 32) | created.dwLowDateTime as u64;
        let secs = ticks / 10_000_000;
        let Some(unix_secs) = secs.checked_sub(EPOCH_DIFF_SECS) else {
            return false;
        };
        let nanos = (ticks % 10_000_000) * 100;
        let started = SystemTime::UNIX_EPOCH + Duration::new(unix_secs, nanos as u32);

        started <= written + Duration::from_secs(2)
    }
}

/// 知道 socket 路径时问「那头是谁」。
///
/// Unix 上路径用不着——内核从连接本身就答得出来；Windows 上恰恰只有这条路
/// 可走（见那边的 `peer_pid_at`）。调用方两个都给，由这一层决定用哪个。
pub fn peer_pid_of(stream: &Stream, socket: &Path) -> Option<u32> {
    #[cfg(unix)]
    {
        let _ = socket;
        imp::peer_pid(stream)
    }
    #[cfg(windows)]
    {
        let _ = stream;
        imp::peer_pid_at(socket)
    }
}
