//! 拼一个「只有当前用户」的 ACL。Windows 专用。
//!
//! 这一段 unsafe 的存在理由，是 Windows 上没有别的办法把 0600 那句话说完整。
//! 常见的省事写法有两种，两种都不够：
//!
//! - 建完文件再 `SetNamedSecurityInfoW`：中间那一小段时间里，文件带着从父
//!   目录继承来的 ACL 躺在盘上。密钥文件不能有这一段。
//! - 干脆不管，靠 `%USERPROFILE%` 继承下来的默认条目：在自家机器上通常
//!   确实只有你自己，但域环境里管理员可以往用户目录上挂条目，共享盘上更是
//!   什么都可能。「通常」不是保证。
//!
//! 所以这里老老实实走全套：取当前进程令牌里的用户 SID，拿它做一条
//! `FILE_ALL_ACCESS` 的 ACE，`SetEntriesInAclW` 拼成 ACL，塞进一个安全描述
//! 符，交给 `CreateFileW` 在创建那一刻生效。事后那次 `SetNamedSecurityInfoW`
//! 带 `PROTECTED_DACL_SECURITY_INFORMATION`——**这个标志才是关键**，它切断
//! 继承：没有它，父目录后来新增的条目会自动流到这个文件上，等于白设。
use std::io;
use std::os::windows::ffi::OsStrExt;
use std::path::Path;

use windows_sys::Win32::Foundation::{LocalFree, HANDLE};
use windows_sys::Win32::Security::Authorization::{
    SetEntriesInAclW, SetNamedSecurityInfoW, EXPLICIT_ACCESS_W, SET_ACCESS, SE_FILE_OBJECT,
    TRUSTEE_IS_SID, TRUSTEE_IS_USER, TRUSTEE_W,
};
use windows_sys::Win32::Security::{
    GetTokenInformation, InitializeSecurityDescriptor, SetSecurityDescriptorDacl, TokenUser, ACL,
    DACL_SECURITY_INFORMATION, NO_INHERITANCE, PROTECTED_DACL_SECURITY_INFORMATION, PSID,
    SECURITY_DESCRIPTOR, TOKEN_QUERY, TOKEN_USER,
};
use windows_sys::Win32::Storage::FileSystem::FILE_ALL_ACCESS;
use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

/// 路径转成 Windows API 要的宽字符串，末尾补 NUL。
pub fn wide(p: &Path) -> Vec<u16> {
    p.as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect()
}

/// 当前进程的用户 SID。返回的 `Vec` 是 `TOKEN_USER` 的原始缓冲区，SID 指向
/// 它内部——所以缓冲区必须比 SID 活得久，不能只把指针传出去。
fn current_user_sid() -> io::Result<Vec<u8>> {
    unsafe {
        let mut token: HANDLE = std::ptr::null_mut();
        if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) == 0 {
            return Err(io::Error::last_os_error());
        }
        let mut need: u32 = 0;
        // 第一次调用注定失败，只为问出要多大的缓冲区——SID 是变长的。
        GetTokenInformation(token, TokenUser, std::ptr::null_mut(), 0, &mut need);
        let mut buf = vec![0u8; need as usize];
        let ok = GetTokenInformation(token, TokenUser, buf.as_mut_ptr().cast(), need, &mut need);
        windows_sys::Win32::Foundation::CloseHandle(token);
        if ok == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(buf)
    }
}

fn sid_of(buf: &[u8]) -> PSID {
    // TOKEN_USER 的第一个字段就是 SID_AND_ATTRIBUTES，它的第一个字段是 PSID。
    unsafe { (*(buf.as_ptr() as *const TOKEN_USER)).User.Sid }
}

/// 一个「只有当前用户」的安全描述符，连同它引用的那块 ACL 内存。
///
/// 三样东西的生命周期是绑在一起的：SID 在 `_token` 里，ACL 里存着指向 SID
/// 的指针，安全描述符里存着指向 ACL 的指针。谁先死都会留下悬垂指针，所以
/// 它们只能整个一起活、一起死。
pub struct OwnerOnly {
    sd: Box<SECURITY_DESCRIPTOR>,
    acl: *mut ACL,
    _token: Vec<u8>,
}

impl OwnerOnly {
    pub fn new() -> io::Result<OwnerOnly> {
        let token = current_user_sid()?;
        let sid = sid_of(&token);

        let mut ea: EXPLICIT_ACCESS_W = unsafe { std::mem::zeroed() };
        ea.grfAccessPermissions = FILE_ALL_ACCESS;
        ea.grfAccessMode = SET_ACCESS;
        // NO_INHERITANCE：这条 ACE 只管这个对象自己。目录上设它不会往下传，
        // 传下去反而会把「新建文件继承什么」这件事悄悄改掉。
        ea.grfInheritance = NO_INHERITANCE;
        ea.Trustee = TRUSTEE_W {
            pMultipleTrustee: std::ptr::null_mut(),
            MultipleTrusteeOperation: 0,
            TrusteeForm: TRUSTEE_IS_SID,
            TrusteeType: TRUSTEE_IS_USER,
            ptstrName: sid.cast(),
        };

        let mut acl: *mut ACL = std::ptr::null_mut();
        let rc = unsafe { SetEntriesInAclW(1, &ea, std::ptr::null_mut(), &mut acl) };
        if rc != 0 {
            return Err(io::Error::from_raw_os_error(rc as i32));
        }

        let mut sd: Box<SECURITY_DESCRIPTOR> = Box::new(unsafe { std::mem::zeroed() });
        unsafe {
            if InitializeSecurityDescriptor(
                (&mut *sd as *mut SECURITY_DESCRIPTOR).cast(),
                1, // SECURITY_DESCRIPTOR_REVISION
            ) == 0
            {
                let e = io::Error::last_os_error();
                LocalFree(acl.cast());
                return Err(e);
            }
            // 第四个参数 bDaclDefaulted = 0：这份 DACL 是我们明确指定的，
            // 不是系统给的默认值。写成 1 的话内核在某些继承计算里会当它可
            // 覆盖，正好是我们不想要的。
            if SetSecurityDescriptorDacl((&mut *sd as *mut SECURITY_DESCRIPTOR).cast(), 1, acl, 0)
                == 0
            {
                let e = io::Error::last_os_error();
                LocalFree(acl.cast());
                return Err(e);
            }
        }
        Ok(OwnerOnly {
            sd,
            acl,
            _token: token,
        })
    }

    pub fn as_mut_ptr(&mut self) -> *mut std::ffi::c_void {
        (&mut *self.sd as *mut SECURITY_DESCRIPTOR).cast()
    }
}

impl Drop for OwnerOnly {
    fn drop(&mut self) {
        if !self.acl.is_null() {
            unsafe { LocalFree(self.acl.cast()) };
        }
    }
}

/// 把一个已经存在的文件或目录的 DACL 换成「只有当前用户」，并切断继承。
pub fn set_owner_only(path: &Path) -> io::Result<()> {
    let token = current_user_sid()?;
    let sid = sid_of(&token);

    let mut ea: EXPLICIT_ACCESS_W = unsafe { std::mem::zeroed() };
    ea.grfAccessPermissions = FILE_ALL_ACCESS;
    ea.grfAccessMode = SET_ACCESS;
    ea.grfInheritance = NO_INHERITANCE;
    ea.Trustee = TRUSTEE_W {
        pMultipleTrustee: std::ptr::null_mut(),
        MultipleTrusteeOperation: 0,
        TrusteeForm: TRUSTEE_IS_SID,
        TrusteeType: TRUSTEE_IS_USER,
        ptstrName: sid.cast(),
    };

    let mut acl: *mut ACL = std::ptr::null_mut();
    let rc = unsafe { SetEntriesInAclW(1, &ea, std::ptr::null_mut(), &mut acl) };
    if rc != 0 {
        return Err(io::Error::from_raw_os_error(rc as i32));
    }

    let mut w = wide(path);
    let rc = unsafe {
        SetNamedSecurityInfoW(
            w.as_mut_ptr(),
            SE_FILE_OBJECT,
            DACL_SECURITY_INFORMATION | PROTECTED_DACL_SECURITY_INFORMATION,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            acl,
            std::ptr::null_mut(),
        )
    };
    unsafe { LocalFree(acl.cast()) };
    if rc != 0 {
        return Err(io::Error::from_raw_os_error(rc as i32));
    }
    Ok(())
}
