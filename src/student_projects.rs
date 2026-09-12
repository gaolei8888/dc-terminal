//! Persistent, per-gate project library. Ending a practice never deletes code.
//! The gate credential is the ownership boundary (one container per student).

use anyhow::{bail, ensure, Context, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Component, Path, PathBuf};
use std::sync::Mutex;

pub const MAX_ARCHIVE_BYTES: u64 = 128 * 1024 * 1024;
const MAX_FILES: usize = 10_000;
const MAX_PROJECTS: usize = 100;
const MAX_ARCHIVES_TOTAL: u64 = 512 * 1024 * 1024;
pub const EXCLUDED: &[&str] = &[
    "密钥、.env 和账号配置",
    "依赖包和缓存",
    "Git 内部目录",
    "符号链接、硬链接和特殊文件",
];

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Project {
    pub id: String,
    pub name: String,
    pub dir: PathBuf,
    pub saved_at: Option<u64>,
    #[serde(default)]
    archive: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct Entry {
    pub path: String,
    pub size: u64,
}

#[derive(Debug, Serialize)]
pub struct Saved {
    pub saved_at: u64,
    pub bytes: u64,
    pub files: usize,
}

pub struct Library {
    base: PathBuf,
    legacy: PathBuf,
    pub socket: PathBuf,
    /// Serializes mutations/download preparation, including upload, for this gate.
    pub operation: Mutex<()>,
}

impl Library {
    pub fn open(base: PathBuf, legacy: PathBuf, socket: PathBuf) -> Result<Self> {
        fs::create_dir_all(&base)?;
        let base = base.canonicalize()?;
        let legacy = legacy.canonicalize()?;
        ensure!(!base.starts_with(&legacy), "项目库必须在工作目录之外");
        for d in ["work", "archives"] {
            fs::create_dir_all(base.join(d))?;
            ensure!(
                !fs::symlink_metadata(base.join(d))?.file_type().is_symlink(),
                "项目库目录不能是链接"
            );
        }
        let s = Self {
            base,
            legacy,
            socket,
            operation: Mutex::new(()),
        };
        if !s.base.join("current.json").exists() {
            s.write_project(&Project {
                id: "current".into(),
                name: "原有项目".into(),
                dir: s.legacy.clone(),
                saved_at: None,
                archive: None,
            })?;
        }
        Ok(s)
    }

    pub fn list(&self) -> Result<Vec<Project>> {
        let mut out = Vec::new();
        for item in fs::read_dir(&self.base)? {
            let p = item?.path();
            if p.extension().is_some_and(|x| x == "json") {
                if let Some(id) = p.file_stem().and_then(|x| x.to_str()) {
                    out.push(self.project(id)?);
                }
            }
        }
        out.sort_by(|a, b| a.id.cmp(&b.id));
        Ok(out)
    }

    pub fn project(&self, id: &str) -> Result<Project> {
        ensure!(
            id == "current" || (id.len() == 32 && id.bytes().all(|c| c.is_ascii_hexdigit())),
            "项目不存在"
        );
        let f = open_relative(&self.base, &format!("{id}.json"))?;
        let p: Project = serde_json::from_reader(f.take(16 * 1024))?;
        let expected = if id == "current" {
            self.legacy.clone()
        } else {
            self.base.join("work").join(id)
        };
        ensure!(p.id == id && p.dir == expected, "项目记录无效");
        // Never trust a project root that was replaced with a symlink.
        ensure!(
            open_relative(&p.dir, "")?.metadata()?.is_dir(),
            "项目目录不可用"
        );
        Ok(p)
    }

    pub fn create(&self, name: &str) -> Result<Project> {
        let name = name.trim();
        ensure!(
            !name.is_empty() && name.chars().count() <= 80 && !name.chars().any(char::is_control),
            "项目名需要 1–80 个字符"
        );
        ensure!(self.list()?.len() < MAX_PROJECTS, "项目数量已达 100 个上限");
        let id = unique()?;
        let dir = self.base.join("work").join(&id);
        fs::create_dir(&dir)?;
        let p = Project {
            id,
            name: name.into(),
            dir,
            saved_at: None,
            archive: None,
        };
        self.write_project(&p)?;
        Ok(p)
    }

    fn write_project(&self, project: &Project) -> Result<()> {
        let mut tmp = Temp::new(&self.base)?;
        serde_json::to_writer(&mut tmp.file, project)?;
        tmp.file.sync_all()?;
        fs::rename(&tmp.path, self.base.join(format!("{}.json", project.id)))?;
        sync_dir(&self.base)?;
        Ok(())
    }

    pub fn files(&self, p: &Project) -> Result<Vec<Entry>> {
        let mut entries = Vec::new();
        walk(&p.dir, Path::new(""), &mut entries)?;
        entries.sort_by(|a, b| a.path.cmp(&b.path));
        ensure!(
            entries.iter().map(|e| e.size).sum::<u64>() <= MAX_ARCHIVE_BYTES,
            "可导出文件超过 128 MiB，请移走大文件后重试"
        );
        Ok(entries)
    }

    pub fn file(&self, p: &Project, path: &str) -> Result<File> {
        check_export_path(path)?;
        let file = open_relative(&p.dir, path)?;
        ensure!(regular(&file.metadata()?), "只能下载普通文件");
        ensure!(
            file.metadata()?.len() <= MAX_ARCHIVE_BYTES,
            "文件超过下载上限"
        );
        Ok(file)
    }

    pub fn upload(
        &self,
        p: &Project,
        name: &str,
        input: &mut dyn Read,
        len: u64,
    ) -> Result<String> {
        ensure!(len <= 64 * 1024 * 1024, "单文件上传上限为 64 MiB");
        ensure!(
            name.len() <= 200
                && !name.starts_with('.')
                && !name.contains('/')
                && !name.contains('\\'),
            "文件名无效"
        );
        check_export_path(name)?;
        upload_to(p, name, input, len)?;
        Ok(name.into())
    }

    /// Serve a completed file snapshot, never an inode an agent is still editing.
    pub fn download_file(&self, p: &Project, path: &str) -> Result<File> {
        let mut source = self.file(p, path)?;
        let size = source.metadata()?.len();
        let mut tmp = Temp::new(&self.base.join("archives"))?;
        let mut hash = Sha256::new();
        let mut copied = 0u64;
        let mut buf = [0u8; 65536];
        loop {
            let n = source.read(&mut buf)?;
            if n == 0 {
                break;
            }
            copied += n as u64;
            ensure!(copied <= size, "文件正在变化，请重试下载");
            hash.update(&buf[..n]);
            tmp.file.write_all(&buf[..n])?;
        }
        let mut verify = Sha256::new();
        let n = std::io::copy(&mut self.file(p, path)?.take(size + 1), &mut verify)?;
        ensure!(
            copied == size && n == size && hash.finalize() == verify.finalize(),
            "文件正在变化，请重试下载"
        );
        tmp.file.seek(SeekFrom::Start(0))?;
        Ok(tmp.file.try_clone()?)
    }

    /// Hash the tree before, while and after writing. A changing tree fails closed;
    /// it never replaces the previous completed archive with a partial result.
    pub fn save(&self, p: &Project) -> Result<(Saved, File)> {
        self.save_limit(p, MAX_ARCHIVE_BYTES)
    }

    fn save_limit(&self, p: &Project, limit: u64) -> Result<(Saved, File)> {
        let before = self.manifest(p)?;
        let total: u64 = before.iter().map(|(_, n, _)| n).sum();
        ensure!(total <= limit, "项目超过归档上限，原文件和上次归档均已保留");
        let archives = self.base.join("archives");
        let used = fs::read_dir(&archives)?.try_fold(0u64, |sum, e| -> Result<u64> {
            Ok(sum + e?.metadata()?.len())
        })?;
        ensure!(
            used + total <= MAX_ARCHIVES_TOTAL,
            "归档空间不足，原文件和上次归档均已保留"
        );
        let mut tmp = Temp::new(&archives)?;
        {
            let mut zip = zip::ZipWriter::new(&mut tmp.file);
            let options = zip::write::FileOptions::default()
                .compression_method(zip::CompressionMethod::Deflated)
                .unix_permissions(0o644);
            for (path, size, digest) in &before {
                zip.start_file(path, options)?;
                let mut file = self.file(p, path)?.take(size + 1);
                let mut hash = Sha256::new();
                let mut n = 0;
                let mut buffer = [0u8; 64 * 1024];
                loop {
                    let read = file.read(&mut buffer)?;
                    if read == 0 {
                        break;
                    }
                    n += read as u64;
                    hash.update(&buffer[..read]);
                    zip.write_all(&buffer[..read])?;
                }
                ensure!(
                    n == *size && hash.finalize().as_slice() == digest,
                    "文件正在变化，请稍后重试；未覆盖上次归档"
                );
            }
            zip.finish()?;
        }
        ensure!(
            before == self.manifest(p)?,
            "文件正在变化，请稍后重试；未覆盖上次归档"
        );
        tmp.file.sync_all()?;
        let bytes = tmp.file.metadata()?.len();
        ensure!(
            used + bytes <= MAX_ARCHIVES_TOTAL,
            "归档空间不足，原文件和上次归档均已保留"
        );
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_secs();
        // Publish a new generation, then atomically commit its metadata pointer.
        // A failed metadata write must leave the previous generation untouched.
        let generation = format!("{}-{}.zip", p.id, unique()?);
        fs::rename(&tmp.path, archives.join(&generation))?;
        sync_dir(&archives)?;
        let mut p = p.clone();
        let old_archive = self.project(&p.id)?.archive;
        p.archive = Some(generation);
        p.saved_at = Some(now);
        self.write_project(&p)?;
        if let Some(old) = old_archive.filter(|x| valid_archive_name(&p.id, x)) {
            let _ = fs::remove_file(archives.join(old));
        }
        tmp.file.seek(SeekFrom::Start(0))?;
        Ok((
            Saved {
                saved_at: now,
                bytes,
                files: before.len(),
            },
            tmp.file.try_clone()?,
        ))
    }

    fn manifest(&self, p: &Project) -> Result<Vec<(String, u64, Vec<u8>)>> {
        self.files(p)?
            .into_iter()
            .map(|e| {
                let mut f = self.file(p, &e.path)?.take(e.size + 1);
                let mut h = Sha256::new();
                let n = std::io::copy(&mut f, &mut h)?;
                ensure!(n == e.size, "文件正在变化，请稍后重试");
                Ok((e.path, e.size, h.finalize().to_vec()))
            })
            .collect()
    }

    pub fn continue_project(&self, p: &Project, profile: &str) -> Result<u32> {
        use crate::proto::{Request, Response};
        ensure!(
            ["claude", "codex", "shell"].contains(&profile),
            "不支持的会话类型"
        );
        let mut c = crate::client::Client::connect(&self.socket).context("连接会话服务失败")?;
        if let Response::Sessions(sessions) = c.call(Request::List)? {
            if let Some(s) = sessions.iter().find(|s| {
                Path::new(&s.dir) == p.dir
                    && s.profile == profile
                    && s.state != crate::session::SessionState::Stopped
            }) {
                return Ok(s.id);
            }
        } else {
            bail!("读取运行会话失败");
        }
        let dir = p.dir.to_string_lossy().into_owned();
        ensure!(
            matches!(
                c.call(Request::PinProject { dir: dir.clone() })?,
                Response::Ok
            ),
            "添加到终端项目列表失败"
        );
        match c.call(Request::Create {
            dir,
            profile: profile.into(),
            remember: true,
        })? {
            Response::Created { id } => Ok(id),
            _ => bail!("会话启动失败，请在终端检查 agent 配置；项目文件已保留"),
        }
    }

    pub fn end(&self, p: &Project) -> Result<(Saved, usize)> {
        use crate::proto::{Request, Response};
        // Fail before touching runtime when even the initial snapshot cannot save.
        self.save(p)
            .context("归档失败，尚未停止会话；文件已保留，请重试保存")?;
        let mut c = crate::client::Client::connect(&self.socket)
            .context("已归档，但连接会话服务失败，未结束练习")?;
        let Response::Sessions(sessions) = c
            .call(Request::List)
            .context("已归档，但读取运行会话失败；文件已保留")?
        else {
            bail!("已归档，但读取运行会话失败");
        };
        let mut stopped = 0;
        for s in sessions {
            if Path::new(&s.dir).starts_with(&p.dir)
                && s.state != crate::session::SessionState::Stopped
            {
                ensure!(
                    matches!(
                        c.call(Request::Stop { id: s.id })
                            .context("停止会话未完成；部分会话可能已停止，文件已保留，请重试")?,
                        Response::Ok
                    ),
                    "停止会话失败，项目文件已保留"
                );
                stopped += 1;
            }
        }
        let Response::Sessions(sessions) = c
            .call(Request::List)
            .context("无法确认会话结束；文件已保留，请重试")?
        else {
            bail!("无法确认会话结束；文件已保留");
        };
        ensure!(
            !sessions
                .iter()
                .any(|s| Path::new(&s.dir).starts_with(&p.dir)
                    && s.state != crate::session::SessionState::Stopped),
            "仍有会话运行，请重试；文件已保留"
        );
        let (saved, _) = self
            .save(p)
            .context("会话已停止，最后归档失败；原文件和之前的归档均保留，请重试保存")?;
        Ok((saved, stopped))
    }
}

fn excluded(name: &str) -> bool {
    let n = name.to_ascii_lowercase();
    matches!(
        n.as_str(),
        ".git"
            | ".ssh"
            | ".dct"
            | ".claude"
            | ".codex"
            | ".aws"
            | ".azure"
            | ".config"
            | "node_modules"
            | ".venv"
            | "venv"
            | "target"
            | "__pycache__"
            | ".cache"
            | ".next"
            | ".nuxt"
            | ".npm"
            | ".npmrc"
            | ".pypirc"
            | ".netrc"
            | "credentials"
            | "credentials.json"
            | "secrets.toml"
            | "secrets.json"
            | "id_rsa"
            | "id_ed25519"
    ) || n.starts_with(".env")
        || n.starts_with(".dcw-")
        || n.ends_with(".pem")
        || n.ends_with(".key")
        || n.ends_with(".p12")
        || n.ends_with(".pfx")
}

fn valid_archive_name(id: &str, name: &str) -> bool {
    name.strip_prefix(&format!("{id}-"))
        .and_then(|s| s.strip_suffix(".zip"))
        .is_some_and(|s| s.len() == 32 && s.bytes().all(|b| b.is_ascii_hexdigit()))
}

fn check_export_path(path: &str) -> Result<()> {
    ensure!(
        !path.is_empty() && !path.contains(['\\', ':']) && !path.chars().any(char::is_control),
        "文件路径无效"
    );
    ensure!(
        Path::new(path)
            .components()
            .all(|c| matches!(c, Component::Normal(n) if n.to_str().is_some_and(|n| !excluded(n)))),
        "文件路径无效或不可导出"
    );
    Ok(())
}

fn regular(m: &fs::Metadata) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        m.is_file() && m.nlink() == 1
    }
    #[cfg(not(unix))]
    {
        m.is_file()
    }
}

#[cfg(unix)]
fn upload_to(p: &Project, name: &str, input: &mut dyn Read, len: u64) -> Result<()> {
    use std::os::fd::{AsRawFd, FromRawFd};
    let root = open_relative(&p.dir, "")?;
    let uploads = std::ffi::CString::new("uploads")?;
    let result = unsafe { libc::mkdirat(root.as_raw_fd(), uploads.as_ptr(), 0o700) };
    if result < 0 && std::io::Error::last_os_error().raw_os_error() != Some(libc::EEXIST) {
        return Err(std::io::Error::last_os_error().into());
    }
    let dir_fd = unsafe {
        libc::openat(
            root.as_raw_fd(),
            uploads.as_ptr(),
            libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
        )
    };
    ensure!(dir_fd >= 0, "上传目录不可用（不能是符号链接）");
    let dir = unsafe { File::from_raw_fd(dir_fd) };
    let tmp = std::ffi::CString::new(format!(".dcw-upload-{}", unique()?))?;
    let name = std::ffi::CString::new(name)?;
    let fd = unsafe {
        libc::openat(
            dir.as_raw_fd(),
            tmp.as_ptr(),
            libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL | libc::O_NOFOLLOW | libc::O_CLOEXEC,
            0o600,
        )
    };
    ensure!(fd >= 0, "无法创建上传文件");
    let mut file = unsafe { File::from_raw_fd(fd) };
    let result = (|| -> Result<()> {
        ensure!(
            std::io::copy(&mut input.take(len), &mut file)? == len,
            "上传中断，请重试"
        );
        file.sync_all()?;
        // linkat fails if the destination exists, so a retry cannot overwrite
        // student work. All path resolution stays anchored to the open uploads fd.
        ensure!(
            unsafe {
                libc::linkat(
                    dir.as_raw_fd(),
                    tmp.as_ptr(),
                    dir.as_raw_fd(),
                    name.as_ptr(),
                    0,
                )
            } == 0,
            "同名文件已存在或无法保存，请改名后重试"
        );
        Ok(())
    })();
    unsafe {
        libc::unlinkat(dir.as_raw_fd(), tmp.as_ptr(), 0);
    }
    dir.sync_all()?;
    result
}

#[cfg(not(unix))]
fn upload_to(_p: &Project, _name: &str, _input: &mut dyn Read, _len: u64) -> Result<()> {
    bail!("项目上传目前仅支持 Linux/macOS 工作区")
}

fn walk(root: &Path, rel: &Path, entries: &mut Vec<Entry>) -> Result<()> {
    ensure!(rel.components().count() <= 32, "项目目录层级过深");
    for e in fs::read_dir(root.join(rel))? {
        let e = e?;
        let name = e.file_name();
        let Some(name) = name.to_str() else { continue };
        if excluded(name) || name.contains(['\\', ':']) || name.chars().any(char::is_control) {
            continue;
        }
        let child = rel.join(name);
        let m = fs::symlink_metadata(e.path())?;
        if m.file_type().is_symlink() || !(m.is_file() || m.is_dir()) {
            continue;
        }
        let file = open_relative(root, child.to_str().context("文件名无效")?)?;
        let m = file.metadata()?;
        if m.is_dir() {
            walk(root, &child, entries)?;
        } else if regular(&m) {
            ensure!(entries.len() < MAX_FILES, "文件数量超过 10000 个归档上限");
            entries.push(Entry {
                path: child.to_string_lossy().into_owned(),
                size: m.len(),
            });
        }
    }
    Ok(())
}

/// Open each component relative to an already-open directory descriptor. This
/// closes the check/open symlink race, including substituted ancestor directories.
#[cfg(unix)]
fn open_relative(root: &Path, rel: &str) -> Result<File> {
    use std::os::fd::{AsRawFd, FromRawFd};
    use std::os::unix::ffi::OsStrExt;
    ensure!(root.is_absolute(), "项目根目录无效");
    let mut file = File::open("/")?;
    for c in root.components().chain(Path::new(rel).components()) {
        match c {
            Component::RootDir => continue,
            Component::Normal(n) => {
                let n = std::ffi::CString::new(n.as_bytes())?;
                let fd = unsafe {
                    libc::openat(
                        file.as_raw_fd(),
                        n.as_ptr(),
                        libc::O_RDONLY | libc::O_NOFOLLOW | libc::O_CLOEXEC | libc::O_NONBLOCK,
                    )
                };
                if fd < 0 {
                    return Err(std::io::Error::last_os_error().into());
                }
                file = unsafe { File::from_raw_fd(fd) };
            }
            _ => bail!("文件路径无效"),
        }
    }
    Ok(file)
}

#[cfg(not(unix))]
fn open_relative(root: &Path, rel: &str) -> Result<File> {
    let mut p = root.to_owned();
    ensure!(
        !fs::symlink_metadata(&p)?.file_type().is_symlink(),
        "项目根目录不能是链接"
    );
    for c in Path::new(rel).components() {
        let Component::Normal(n) = c else {
            bail!("文件路径无效");
        };
        p.push(n);
        ensure!(
            !fs::symlink_metadata(&p)?.file_type().is_symlink(),
            "文件不能是链接"
        );
    }
    ensure!(
        p.canonicalize()?.starts_with(root.canonicalize()?),
        "文件不在项目内"
    );
    Ok(File::open(p)?)
}

fn unique() -> Result<String> {
    let mut b = [0u8; 16];
    getrandom::getrandom(&mut b).map_err(|e| anyhow::anyhow!("随机数不可用: {e}"))?;
    Ok(b.iter().map(|x| format!("{x:02x}")).collect())
}

pub struct Temp {
    pub file: File,
    pub path: PathBuf,
}
impl Temp {
    pub fn new(dir: &Path) -> Result<Self> {
        let path = dir.join(format!(".dcw-{}", unique()?));
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .open(&path)?;
        Ok(Self { file, path })
    }
}
impl Drop for Temp {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}
fn sync_dir(path: &Path) -> Result<()> {
    #[cfg(unix)]
    File::open(path)?.sync_all()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fixture() -> (tempfile::TempDir, Library) {
        let d = tempfile::tempdir().unwrap();
        fs::create_dir(d.path().join("legacy")).unwrap();
        let l = Library::open(
            d.path().join("state"),
            d.path().join("legacy"),
            d.path().join("daemon.sock"),
        )
        .unwrap();
        (d, l)
    }
    #[test]
    fn archive_survives_reopen_and_failed_save_keeps_previous() {
        let (_d, l) = fixture();
        let p = l.create("练习一").unwrap();
        fs::write(p.dir.join("main.py"), b"print(42)").unwrap();
        fs::write(p.dir.join(".env"), b"SECRET=x").unwrap();
        let (s, f) = l.save(&p).unwrap();
        assert_eq!(s.files, 1);
        let mut z = zip::ZipArchive::new(f).unwrap();
        assert_eq!(z.len(), 1);
        let mut text = String::new();
        z.by_name("main.py")
            .unwrap()
            .read_to_string(&mut text)
            .unwrap();
        assert_eq!(text, "print(42)");
        let archive = l
            .base
            .join("archives")
            .join(l.project(&p.id).unwrap().archive.unwrap());
        let old = fs::read(&archive).unwrap();
        fs::write(p.dir.join("main.py"), b"larger changed content").unwrap();
        assert!(l.save_limit(&p, 1).is_err());
        assert_eq!(old, fs::read(&archive).unwrap());
        let reopened = Library::open(l.base.clone(), l.legacy.clone(), l.socket.clone()).unwrap();
        assert_eq!(reopened.project(&p.id).unwrap().saved_at, Some(s.saved_at));
        assert_eq!(reopened.list().unwrap().len(), 2);
    }
    #[test]
    fn export_paths_cannot_escape_or_include_credentials() {
        let (_d, l) = fixture();
        let p = l.project("current").unwrap();
        for path in [
            "../secret",
            "/etc/passwd",
            "a/../../s",
            "a\\..\\s",
            ".env",
            "src/.ssh/id_rsa",
            "key.pem",
        ] {
            assert!(l.file(&p, path).is_err(), "{path}");
        }
        assert!(l.project("../current").is_err());
        assert!(l.create("").is_err());
        assert!(l.create(&"x".repeat(81)).is_err());
    }
    #[test]
    #[cfg(unix)]
    fn upload_is_atomic_and_never_overwrites_or_follows_links() {
        use std::os::unix::fs::symlink;
        let (d, l) = fixture();
        let p = l.project("current").unwrap();
        assert!(l.upload(&p, "x.txt", &mut &b"partial"[..], 99).is_err());
        assert!(!p.dir.join("uploads/x.txt").exists());
        l.upload(&p, "x.txt", &mut &b"hello"[..], 5).unwrap();
        assert!(l.upload(&p, "x.txt", &mut &b"bad"[..], 3).is_err());
        assert_eq!(fs::read(p.dir.join("uploads/x.txt")).unwrap(), b"hello");
        assert_eq!(l.files(&p).unwrap().len(), 1);
        fs::rename(p.dir.join("uploads"), p.dir.join("old")).unwrap();
        symlink(d.path(), p.dir.join("uploads")).unwrap();
        assert!(l.upload(&p, "outside.txt", &mut &b"bad"[..], 3).is_err());
        assert!(!d.path().join("outside.txt").exists());
    }
    #[test]
    #[cfg(unix)]
    fn sockets_are_skipped_and_download_is_a_snapshot() {
        let (_d, l) = fixture();
        let p = l.project("current").unwrap();
        let _socket = std::os::unix::net::UnixListener::bind(p.dir.join("app.sock")).unwrap();
        fs::write(p.dir.join("file.txt"), "before").unwrap();
        assert_eq!(l.files(&p).unwrap().len(), 1);
        let mut snapshot = l.download_file(&p, "file.txt").unwrap();
        fs::write(p.dir.join("file.txt"), "after").unwrap();
        let mut text = String::new();
        snapshot.read_to_string(&mut text).unwrap();
        assert_eq!(text, "before");
        assert_eq!(l.save(&p).unwrap().0.files, 1);
    }
    #[test]
    fn metadata_commit_failure_keeps_previous_archive_generation() {
        let (_d, l) = fixture();
        let p = l.project("current").unwrap();
        fs::write(p.dir.join("file.txt"), "original").unwrap();
        l.save(&p).unwrap();
        let old = l
            .base
            .join("archives")
            .join(l.project(&p.id).unwrap().archive.unwrap());
        let bytes = fs::read(&old).unwrap();
        // A directory at the metadata target makes its atomic rename fail, after
        // the new ZIP was written. The old archive must still be readable.
        fs::rename(l.base.join("current.json"), l.base.join("saved-record")).unwrap();
        fs::create_dir(l.base.join("current.json")).unwrap();
        fs::write(p.dir.join("file.txt"), "changed").unwrap();
        assert!(l.save(&p).is_err());
        assert_eq!(fs::read(old).unwrap(), bytes);
        assert_eq!(fs::read(p.dir.join("file.txt")).unwrap(), b"changed");
    }
    #[test]
    #[cfg(unix)]
    fn symlink_and_hardlink_exports_are_excluded() {
        use std::os::unix::fs::symlink;
        let (d, l) = fixture();
        let p = l.project("current").unwrap();
        let secret = d.path().join("secret");
        fs::write(&secret, "private").unwrap();
        symlink(&secret, p.dir.join("link")).unwrap();
        fs::hard_link(&secret, p.dir.join("hard")).unwrap();
        symlink(d.path(), p.dir.join("outside")).unwrap();
        assert!(l.files(&p).unwrap().is_empty());
        assert!(l.file(&p, "link").is_err());
        assert!(l.file(&p, "hard").is_err());
        assert!(l.file(&p, "outside/secret").is_err());
    }
}
