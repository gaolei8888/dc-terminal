//! swift/Keys.swift 的绑定。Swift 的 `Int` 是 64 位有符号，对应 `isize`。
use dct_brain::sign::SignError;
use std::ffi::CString;
use std::os::raw::c_char;

extern "C" {
    fn dct_se_available() -> bool;
    fn dct_se_create(biometric: bool, blob_out: *mut u8, blob_cap: isize, blob_len: *mut isize, pub_out: *mut u8) -> i32;
    fn dct_se_sign(blob: *const u8, blob_len: isize, msg: *const u8, msg_len: isize, reason: *const c_char, sig_out: *mut u8) -> i32;
}

fn status(code: i32) -> SignError {
    match code {
        1 => SignError::Unavailable,
        3 => SignError::Cancelled,
        2 => SignError::Other("钥匙文件不对，可能是从别的电脑拷来的".into()),
        4 => SignError::Other("钥匙太大".into()),
        c => SignError::Other(format!("安全芯片返回 {c}")),
    }
}

pub struct MacEnclave;

impl super::SecureEnclave for MacEnclave {
    fn available(&self) -> bool {
        unsafe { dct_se_available() }
    }

    fn create(&self, biometric: bool) -> Result<(Vec<u8>, [u8; 65]), SignError> {
        let mut blob = vec![0u8; 1024];
        let mut len: isize = 0;
        let mut public = [0u8; 65];
        let rc = unsafe { dct_se_create(biometric, blob.as_mut_ptr(), blob.len() as isize, &mut len, public.as_mut_ptr()) };
        if rc != 0 {
            return Err(status(rc));
        }
        blob.truncate(len as usize);
        Ok((blob, public))
    }

    fn sign(&self, blob: &[u8], msg: &[u8], reason: &str) -> Result<[u8; 64], SignError> {
        // 人话里不该有 NUL；有就去掉，别让整次签名失败。
        let reason = CString::new(reason.replace('\0', "")).unwrap();
        let mut sig = [0u8; 64];
        let rc = unsafe {
            dct_se_sign(blob.as_ptr(), blob.len() as isize, msg.as_ptr(), msg.len() as isize, reason.as_ptr(), sig.as_mut_ptr())
        };
        if rc != 0 {
            return Err(status(rc));
        }
        Ok(sig)
    }
}
