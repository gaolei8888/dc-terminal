//! 「只有你自己能读」这件事，两个系统的说法。
//!
//! Unix 是权限位：0600 一个数字说完，而且能在 `open(2)` 那一刻就带上，
//! 文件从不存在直接变成「只有属主可读写」，中间没有窗口。
//!
//! Windows 没有权限位，只有 ACL——一串「谁能干什么」的条目，默认从父目录
//! 继承。继承来的那份通常已经只有你自己（`%USERPROFILE%` 底下如此），但
//! 「通常」不是保证：域环境里管理员可以往用户目录上挂条目，共享盘上更是
//! 什么都可能。密钥文件不能靠「通常」。
//!
//! 所以 Windows 这一侧走的是同一条路而不是省事的那条：自己拼一个只有当前
//! 用户一条 ACE 的安全描述符，交给 `CreateFileW` 在创建的那一刻生效，
//! 并且显式**不继承**父目录的条目。中间没有窗口这一点，两边是一样的。
use std::path::Path;

/// 新建一份只有属主能读写的文件（已存在就截断）。
///
/// **不是「先建再收紧」。** 那中间有一段别的账号能读到内容的时间，密钥文件
/// 不能有这一段——`secrets.rs` 的注释里点了名，这里是它依赖的那半边。
pub fn create_private(path: &Path) -> std::io::Result<std::fs::File> {
    imp::create_private(path)
}

/// 把一个已经存在的文件收紧到只有属主。
pub fn restrict_file_to_owner(path: &Path) -> std::io::Result<()> {
    imp::restrict(path, false)
}

/// 把一个已经存在的目录收紧到只有属主（Unix 上是 0700：还要能进去）。
pub fn restrict_dir_to_owner(path: &Path) -> std::io::Result<()> {
    imp::restrict(path, true)
}

/// `cmd` 在这台机器上能不能执行。带路径分隔符当路径查，否则遍历 PATH。
///
/// **这个判断必须和实际 spawn 用同一个环境**，所以只能在守护进程里调用——
/// 界面进程的 PATH 可能不一样，那会导致「菜单说能用，一开就失败」。
pub fn command_exists(cmd: &str) -> bool {
    find_command(cmd).is_some()
}

/// 同 [`command_exists`]，但把找到的那个文件交出来。
///
/// 谁要它：Windows 上「用户敲的名字」和「真正要启动的文件」不是同一个东西
/// （`claude` → `claude.cmd`），而 `.cmd` 还不能直接 CreateProcess，得
/// 绕一趟 cmd.exe（见 `sys::shell::launch_argv`）。那一步需要的是路径，
/// 不是一个是非。
pub fn find_command(cmd: &str) -> Option<std::path::PathBuf> {
    imp::find_command(cmd)
}

#[cfg(unix)]
mod imp {
    use super::*;
    use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};

    pub fn create_private(path: &Path) -> std::io::Result<std::fs::File> {
        std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(path)
    }

    pub fn restrict(path: &Path, dir: bool) -> std::io::Result<()> {
        // 目录要 0700 而不是 0600：少了执行位就进不去，里面的文件谁也读不到，
        // 包括我们自己。
        let bits = if dir { 0o700 } else { 0o600 };
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(bits))
    }

    fn is_exec(p: &Path) -> bool {
        std::fs::metadata(p)
            .map(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
            .unwrap_or(false)
    }

    pub fn find_command(cmd: &str) -> Option<std::path::PathBuf> {
        if cmd.contains('/') {
            let p = Path::new(cmd);
            return is_exec(p).then(|| p.to_path_buf());
        }
        std::env::var("PATH")
            .ok()?
            .split(':')
            .filter(|d| !d.is_empty())
            .map(|d| Path::new(d).join(cmd))
            .find(|p| is_exec(p))
    }
}

#[cfg(windows)]
mod imp {
    use super::*;
    use std::os::windows::io::FromRawHandle;

    /// PATHEXT 没设时的兜底。这四个是 cmd.exe 自己的默认里跟我们有关的部分：
    /// npm 装出来的 CLI 是 `.cmd`（`claude.cmd`），Rust 编出来的是 `.exe`。
    const DEFAULT_PATHEXT: &str = ".COM;.EXE;.BAT;.CMD";

    pub fn create_private(path: &Path) -> std::io::Result<std::fs::File> {
        // 先删掉可能存在的那一份，而不是 CREATE_ALWAYS 覆盖它。
        //
        // 差别在 ACL：`CreateFileW` 手里的安全描述符**只在真的新建时生效**，
        // 覆盖一个已存在的文件时它被整个忽略，留下的是上一次的 ACL。删了再
        // 建，「这个文件的权限由这一行代码说了算」才是永远成立的。
        //
        // 事后补一次 `SetNamedSecurityInfoW` 也能达到同样效果，但那要在文件
        // 已经建好之后再打开它一次——正是我们不想要的那个窗口，也正是
        // Unix 那边用 `mode(0o600)` 一次说完的东西。删不掉（不存在，或者
        // 被人占着）就交给下面的 CreateFileW 去报错，它的错误信息更准。
        let _ = std::fs::remove_file(path);

        let mut sd = crate::sys::acl::OwnerOnly::new()?;
        let sa = windows_sys::Win32::Security::SECURITY_ATTRIBUTES {
            nLength: std::mem::size_of::<windows_sys::Win32::Security::SECURITY_ATTRIBUTES>()
                as u32,
            lpSecurityDescriptor: sd.as_mut_ptr(),
            bInheritHandle: 0,
        };
        let wide = crate::sys::acl::wide(path);
        // 共享位必须给全，尤其是 FILE_SHARE_DELETE。
        //
        // 这两处调用方（`secrets.rs`、`last_sessions.rs`）写的都是「先写临时
        // 文件、再 rename 覆盖正式文件」，而它们是照 Unix 的习惯写的：rename
        // 的时候句柄还开着。Unix 上这毫无问题；Windows 上，一个以
        // dwShareMode = 0 打开的文件**连改名都不许**，于是保存密钥会以一句
        // 「操作失败」告终，而错误里没有任何东西指向真正的原因。
        //
        // 给全共享位不放松任何权限——谁能打开这个文件由上面那条 ACL 决定，
        // 共享位只决定「已经打开的人和后来者能不能同时持有它」。
        let share = windows_sys::Win32::Storage::FileSystem::FILE_SHARE_READ
            | windows_sys::Win32::Storage::FileSystem::FILE_SHARE_WRITE
            | windows_sys::Win32::Storage::FileSystem::FILE_SHARE_DELETE;
        let handle = unsafe {
            windows_sys::Win32::Storage::FileSystem::CreateFileW(
                wide.as_ptr(),
                windows_sys::Win32::Foundation::GENERIC_WRITE,
                share,
                &sa,
                windows_sys::Win32::Storage::FileSystem::CREATE_ALWAYS,
                windows_sys::Win32::Storage::FileSystem::FILE_ATTRIBUTE_NORMAL,
                std::ptr::null_mut(),
            )
        };
        if handle == windows_sys::Win32::Foundation::INVALID_HANDLE_VALUE {
            return Err(std::io::Error::last_os_error());
        }
        Ok(unsafe { std::fs::File::from_raw_handle(handle as _) })
    }

    pub fn restrict(path: &Path, _dir: bool) -> std::io::Result<()> {
        // 目录和文件在这里不分家：Windows 的 ACL 没有「执行位」这一说，
        // 「只有属主」就是同一串条目。dir 参数留着是为了两边同一个签名。
        crate::sys::acl::set_owner_only(path)
    }

    fn is_exec(p: &Path) -> bool {
        p.is_file()
    }

    /// Windows 上「能不能执行」不看权限位，看扩展名——而且用户敲的
    /// `claude` 和磁盘上的 `claude.cmd` 不是同一个字符串。npm 装出来的
    /// CLI 全是这个形状，认不出来的话菜单会把装好的 agent 说成没装。
    pub fn find_command(cmd: &str) -> Option<std::path::PathBuf> {
        let exts: Vec<String> = std::env::var("PATHEXT")
            .unwrap_or_else(|_| DEFAULT_PATHEXT.to_string())
            .split(';')
            .filter(|e| !e.is_empty())
            .map(|e| e.to_string())
            .collect();

        // 自带扩展名的（`node.exe`）不要再往后面缀一遍。
        let has_ext = Path::new(cmd).extension().is_some_and(|e| {
            exts.iter()
                .any(|x| x[1..].eq_ignore_ascii_case(&e.to_string_lossy()))
        });

        let hit = |base: &Path| -> Option<std::path::PathBuf> {
            if has_ext {
                return is_exec(base).then(|| base.to_path_buf());
            }
            // **先试扩展名，别先认那个光秃秃的同名文件。**
            //
            // npm 给每个 CLI 装三个文件：`claude.cmd`（cmd.exe 用）、
            // `claude.ps1`（PowerShell 用），还有一个**没有扩展名**的
            // `claude`——那是给 Git Bash 之类的 POSIX shell 用的 sh 脚本。
            // 顺序反过来的话，第一个撞上的就是它，而 `CreateProcess` 起不了
            // 一个 sh 脚本：菜单上 Claude 认得出、装得好，一按下去就失败。
            //
            // PATHEXT 内部的顺序就是优先级：同一个目录里 `foo.exe` 和
            // `foo.cmd` 都在时，cmd.exe 挑前者，我们也挑前者。
            let by_ext = exts.iter().find_map(|e| {
                let mut s = base.as_os_str().to_os_string();
                s.push(e);
                let p = std::path::PathBuf::from(s);
                is_exec(&p).then_some(p)
            });
            if by_ext.is_some() {
                return by_ext;
            }
            // 一个扩展名都不中，才轮到光秃秃的那个。Windows 确实起得了没有
            // 扩展名的 PE 文件，只要给全路径——保留这一支是为了那种情况，
            // 不是为了上面那个 sh 脚本。
            is_exec(base).then(|| base.to_path_buf())
        };

        // 正斜杠也算路径：profile 里写 `/usr/bin/env` 这类值在 Windows 上
        // 注定找不到，但那属于「这台机器上没有」，不该被当成「PATH 里的
        // 名字」满 PATH 找一遍。
        if cmd.contains('\\') || cmd.contains('/') {
            return hit(Path::new(cmd));
        }
        std::env::var("PATH")
            .ok()?
            .split(';')
            .filter(|d| !d.is_empty())
            .find_map(|d| hit(&Path::new(d).join(cmd)))
    }
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;

    /// npm 给每个 CLI 装三个文件，其中一个是**没有扩展名**的 sh 脚本
    /// （给 Git Bash 用的）。`CreateProcess` 起不了它——真机上的表现是
    /// 菜单里 Claude 明明认得出、也装好了，一按下去就起不来。
    ///
    /// 所以查找必须先试 PATHEXT，再考虑同名的光秃秃文件。这条测试不碰
    /// PATH（那是进程级的，并行测试之间会互相踩），直接给绝对路径——
    /// 走的是同一段挑选逻辑。
    #[test]
    fn an_extension_wins_over_the_bare_file_of_the_same_name() {
        let tmp = tempfile::tempdir().unwrap();
        let bare = tmp.path().join("agent");
        let cmd = tmp.path().join("agent.cmd");
        std::fs::write(&bare, "#!/bin/sh\nexec node x.js\n").unwrap();
        std::fs::write(&cmd, "@echo off\n").unwrap();

        let found = find_command(&bare.display().to_string()).expect("该找得到");
        // 比较时不看大小写：扩展名是从 PATHEXT 里取的，而那个变量的值通常是
        // 大写（`.CMD`），拼出来的路径于是也是大写。在 Windows 上这和磁盘上
        // 的 `agent.cmd` 是同一个文件——真正要断言的是「挑中了带扩展名的
        // 那一个」，不是大小写怎么写。
        assert_eq!(
            found.display().to_string().to_lowercase(),
            cmd.display().to_string().to_lowercase(),
            "要挑 .cmd 那个；挑中没有扩展名的那个的话，CreateProcess 起不来"
        );
    }

    /// 反过来：一个扩展名都不中时，光秃秃的那个仍然算数——Windows 确实
    /// 起得了没有扩展名的 PE 文件，只要给的是全路径。
    #[test]
    fn the_bare_file_still_counts_when_nothing_else_matches() {
        let tmp = tempfile::tempdir().unwrap();
        let bare = tmp.path().join("lonely");
        std::fs::write(&bare, "MZ").unwrap();

        assert_eq!(find_command(&bare.display().to_string()), Some(bare));
    }
}
