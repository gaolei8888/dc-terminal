//! dct 自带的那份 Node 运行时。
//!
//! **为什么有这个模块：** 看板上九个 agent 里，真正能干活的那几个都是 npm
//! 装出来的（`claude`、`codex`、`opencode`、`qwen`），而 npm 要 Node。于是
//! 「装个 dct 就能开始」这句话在最后一米断掉：学生装完 dct，选中 Claude，
//! 按下去，得到的是一句 `npm 不是内部或外部命令`——一句操作系统的原话，
//! 既不告诉他缺什么，也不告诉他下一步该干嘛。
//!
//! 所以 dct 自己下一份 Node，放在 `~/.dct/runtime/node`。它只服务 dct：
//! 不进系统 PATH，不写注册表，不碰用户可能已经装好的那份 Node。用户哪天
//! 想删 dct，连 `~/.dct` 一起删掉就干净了。
//!
//! ## 三个已经验过的事实，不是推断
//!
//! **一、`npm i -g` 的产物就落在 node 目录里。** 用一份解压出来的便携 Node
//! 跑 `npm i -g <带 bin 的包>`，Windows 上产物是 `<node>/x.cmd`（还有 `x`
//! 和 `x.ps1`），Unix 上是 `<node>/bin/x`。所以要加进 PATH 的目录，
//! Windows 是 node 目录本身，Unix 是它下面的 `bin`——`node_bin_dir` 就这一件事。
//!
//! **二、解压不需要新依赖。** Windows 从 10 的 1803 版起自带 bsdtar
//! （`tar.exe`），而 dct 本来就要求 1803 以上——AF_UNIX 也是从那一版才有的
//! （见 `sys::ipc`）。bsdtar 连 zip 一起解，实测过。Unix 上 tar 到处都在。
//! 加 `flate2` + `tar` + `zip` 三个 crate 换不来任何东西，只会让依赖树长胖。
//!
//! **三、官方和国内镜像的路径结构一模一样。** `nodejs.org/dist` 和
//! `npmmirror.com/mirrors/node` 下面都是 `v<版本>/<文件名>`，连
//! `SHASUMS256.txt` 都在同样的位置。所以换镜像只需要换一个前缀，
//! 校验和照验不误——`DCT_NODE_BASE` 就是这个前缀。
//!
//! ## 默认走官方源，镜像是一个环境变量
//!
//! 国内的课堂几乎一定要切到镜像，但 dct 是个双语的公开工具，把淘宝镜像
//! 设成所有人的默认是错的。所以默认官方，`DCT_NODE_BASE` 一变量切走，
//! 老师在教室里设一次就够。

use std::io::Read;
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

/// 钉死版本，不去问「现在最新的 LTS 是哪个」。
///
/// 问一次要么解析 `index.json`（几百 KB，且格式随时能变），要么跟着
/// `latest-v22.x` 这类会移动的地址走——两条路的共同问题是：**每个学生装到
/// 的可能是不同的 Node**，而排查「他那台为什么不行」时，第一个要排除的
/// 就是这一条。钉死之后所有人手里是同一份，校验和也才有意义。
pub const NODE_VERSION: &str = "v22.11.0";

const DEFAULT_NODE_BASE: &str = "https://nodejs.org/dist";

/// 国内课堂用这个。写死在代码里是为了让出错时那句话能把地址原样印出来，
/// 让老师照着抄一行，而不是拿着「换个镜像」四个字去搜。
pub const CN_NODE_BASE: &str = "https://npmmirror.com/mirrors/node";
pub const CN_NPM_REGISTRY: &str = "https://registry.npmmirror.com";

/// 一个平台对应的 Node 包。`stem` 是解压出来的那个顶层目录名，
/// 也是文件名去掉后缀的部分——Node 的命名规则如此，两者永远一致。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NodeAsset {
    pub file: String,
    pub stem: String,
}

/// 这个平台该下哪个包。认不出来返回 `None`——那不是错误，是「这台机器
/// 得自己装 Node」，调用方负责把这句话说清楚。
pub fn node_asset(os: &str, arch: &str, version: &str) -> Option<NodeAsset> {
    let plat = match (os, arch) {
        ("windows", "x86_64") => "win-x64",
        // ARM64 的 Windows 跑 x64 靠系统自带的模拟，透明且够快。Node 有
        // win-arm64，但多一个包就多一条没人验过的路，而收益只是省一层模拟。
        ("windows", "aarch64") => "win-x64",
        ("linux", "x86_64") => "linux-x64",
        ("linux", "aarch64") => "linux-arm64",
        ("macos", "x86_64") => "darwin-x64",
        ("macos", "aarch64") => "darwin-arm64",
        _ => return None,
    };
    let stem = format!("node-{version}-{plat}");
    // Node 在 Windows 上只发 zip，别的平台发 tar.gz。两种 bsdtar 都解得开。
    let ext = if os == "windows" { "zip" } else { "tar.gz" };
    Some(NodeAsset {
        file: format!("{stem}.{ext}"),
        stem,
    })
}

pub fn node_base() -> String {
    std::env::var("DCT_NODE_BASE")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| DEFAULT_NODE_BASE.to_string())
}

/// `<base>/v22.11.0/<文件名>`。`base` 末尾带不带斜杠都得对——老师复制
/// 粘贴一个地址进环境变量时，带不带那一下完全是随机的。
pub fn dist_url(base: &str, version: &str, file: &str) -> String {
    format!("{}/{version}/{file}", base.trim_end_matches('/'))
}

/// 跟 socket 同目录，跟 `config.toml`、`secrets.toml`、`profiles/` 一样。
/// 测试因此可以把整个运行时指到临时目录里去。
pub fn runtime_dir_for_socket(socket: &Path) -> PathBuf {
    match socket.parent() {
        Some(d) => d.join("runtime"),
        None => PathBuf::from("runtime"),
    }
}

pub fn node_dir(runtime: &Path) -> PathBuf {
    runtime.join("node")
}

/// 要加进 PATH 的那个目录。**这就是事实一。**
pub fn node_bin_dir(runtime: &Path) -> PathBuf {
    let n = node_dir(runtime);
    if cfg!(windows) {
        n
    } else {
        n.join("bin")
    }
}

/// 这份自带的 Node 装好了没有。认的是 node 可执行文件在不在，不是目录
/// 在不在——上次解压到一半断电的话，目录是在的，而里面没有能跑的东西。
pub fn node_installed(runtime: &Path) -> bool {
    let exe = if cfg!(windows) { "node.exe" } else { "node" };
    node_bin_dir(runtime).join(exe).is_file()
}

/// 从 `SHASUMS256.txt` 里挑出某个文件的哈希。
///
/// 按字段比对文件名，不 grep 整行：Node 那个文件里几十个包名互相是前缀
/// （`node-v22.11.0-linux-x64.tar.gz` 和 `...-linux-x64.tar.xz`），
/// 子串匹配会挑错行，而挑错了就是拿另一个包的哈希去验这个包。
pub fn sha_for(sums: &str, file: &str) -> Option<String> {
    sums.lines().find_map(|line| {
        let mut it = line.split_whitespace();
        let hash = it.next()?;
        let name = it.next()?.trim_start_matches('*');
        (name == file).then(|| hash.to_string())
    })
}

/// 把 `dir` 放到 PATH 最前面。已经在里面就原样返回——重复追加会让
/// PATH 在每次启动时长一截，而那东西在 Windows 上是有长度上限的。
pub fn prepend_path(existing: &str, dir: &Path) -> String {
    let sep = if cfg!(windows) { ';' } else { ':' };
    let d = dir.to_string_lossy();
    let already = existing.split(sep).any(|p| {
        let p = p.trim().trim_end_matches(['/', '\\']);
        !p.is_empty() && p.eq_ignore_ascii_case(d.trim_end_matches(['/', '\\']))
    });
    if already || d.is_empty() {
        existing.to_string()
    } else if existing.is_empty() {
        d.to_string()
    } else {
        format!("{d}{sep}{existing}")
    }
}

/// 把自带的运行时挂到**当前进程**的 PATH 上。
///
/// 只有守护进程该调它。理由是仓库里已经定过的那一条（见 `profile.rs` 的
/// `command_exists`）：**守护进程的 PATH 才是子进程真正会拿到的那个**，
/// 所以「装没装」这个判断也必须在同一个环境里问。在别处改，会得到
/// 「菜单说能用，一开就失败」，或者反过来的「明明装好了却说没装」。
pub fn activate(runtime: &Path) {
    if !node_installed(runtime) {
        return;
    }
    let bin = node_bin_dir(runtime);
    let current = std::env::var("PATH").unwrap_or_default();
    let next = prepend_path(&current, &bin);
    if next != current {
        std::env::set_var("PATH", next);
    }
}

/// 下载进度怎么说出去。抽成 trait 是因为这件事有两个去处：`dct install`
/// 印在终端上给学生看，测试则什么都不印。
pub trait Progress {
    fn line(&self, text: &str);
    fn percent(&self, done: u64, total: Option<u64>);
    fn done(&self);
}

/// 什么都不说。测试用。
pub struct Silent;
impl Progress for Silent {
    fn line(&self, _: &str) {}
    fn percent(&self, _: u64, _: Option<u64>) {}
    fn done(&self) {}
}

fn agent() -> ureq::Agent {
    // 这里**不设** `.timeout()`。那是整次调用的上限，而这次调用要拖着
    // 几十 MB 的包走完——设了就是给下载定一个「超过 N 秒算失败」的死线，
    // 而慢正是教室网络的常态。要防的是「卡住不动」，那是
    // `timeout_read` 的事：每一次读的间隔有上限，整体多久不管。
    //
    // `timeout_connect` 必须显式设，否则退回 ureq 默认的 30 秒——
    // 同 `verify.rs` 和 `llm/http.rs` 里那条已经踩过的坑。
    crate::sys::tls::agent_builder()
        .timeout_connect(std::time::Duration::from_secs(20))
        .timeout_read(std::time::Duration::from_secs(60))
        .build()
}

/// 下载失败的原因。**装的是可以给用户看的话所需要的信息，不是原始错误。**
/// 原始错误是英文的 socket 报错，对着它没人能采取下一步行动。
#[derive(Debug, PartialEq, Eq)]
pub enum FetchError {
    /// 连不上，或者对面没给我们这个文件
    Unreachable { url: String },
    /// 下回来了，但跟官方校验和对不上
    Corrupt,
    /// 这个平台没有现成的 Node 包
    NoAssetForPlatform,
    /// 解压失败（`tar` 不在，或者包是坏的）
    CannotUnpack,
    /// 写不进磁盘
    CannotWrite,
}

/// 原始的网络错误往哪儿去。
///
/// **不给用户看**：那是一句英文的 socket/TLS 报错，对着它没人能采取下一步
/// 行动——这是仓库里已有的规矩（见 `profile.rs::io_reason` 的注释：原始详情
/// 不丢，写到 stderr，不冒泡到界面上）。但也不能真的丢掉：「下不到东西」
/// 在教室里有十几种原因，而分辨它们只能靠这一行。
///
/// 所以默认一个字都不印，`DCT_DEBUG` 设了才印到 stderr。帮学生查问题的人
/// 多敲一个变量就看得到全部，学生自己永远看不到。
fn note_raw(url: &str, e: impl std::fmt::Display) {
    if std::env::var_os("DCT_DEBUG").is_some() {
        eprintln!("[dct] {url}: {e}");
    }
}

fn get_text(url: &str) -> Result<String, FetchError> {
    let unreachable = || FetchError::Unreachable {
        url: url.to_string(),
    };
    agent()
        .get(url)
        .call()
        .map_err(|e| {
            note_raw(url, e);
            unreachable()
        })?
        .into_string()
        .map_err(|e| {
            note_raw(url, e);
            unreachable()
        })
}

/// 边下边算哈希，不落地两次。返回真实哈希，由调用方跟期望值比。
fn download_verified(
    url: &str,
    to: &Path,
    expect: &str,
    p: &dyn Progress,
) -> Result<(), FetchError> {
    let resp = agent().get(url).call().map_err(|e| {
        note_raw(url, e);
        FetchError::Unreachable {
            url: url.to_string(),
        }
    })?;
    let total = resp
        .header("Content-Length")
        .and_then(|s| s.parse::<u64>().ok());

    let mut out = std::fs::File::create(to).map_err(|_| FetchError::CannotWrite)?;
    let mut reader = resp.into_reader();
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 64 * 1024];
    let mut done: u64 = 0;
    loop {
        let n = reader.read(&mut buf).map_err(|_| FetchError::Unreachable {
            url: url.to_string(),
        })?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
        std::io::Write::write_all(&mut out, &buf[..n]).map_err(|_| FetchError::CannotWrite)?;
        done += n as u64;
        p.percent(done, total);
    }
    p.done();

    let got = format!("{:x}", hasher.finalize());
    if !got.eq_ignore_ascii_case(expect) {
        // 坏文件不留在盘上。留着的话，下次重跑会看到一个「已经下过了」的
        // 文件，而它正是上次失败的那一个。
        let _ = std::fs::remove_file(to);
        return Err(FetchError::Corrupt);
    }
    Ok(())
}

/// 解压。shell 出去调 `tar`——理由见模块注释里的事实二。
///
/// `tar` 认 zip 也认 tar.gz，所以两个平台一条命令。`-C` 指定解到哪儿，
/// 解出来的是包自带的那个顶层目录（`node-v22.11.0-win-x64`）。
fn unpack(archive: &Path, into: &Path) -> Result<(), FetchError> {
    std::fs::create_dir_all(into).map_err(|_| FetchError::CannotWrite)?;
    let status = std::process::Command::new("tar")
        .arg("-xf")
        .arg(archive)
        .arg("-C")
        .arg(into)
        .status()
        .map_err(|_| FetchError::CannotUnpack)?;
    if status.success() {
        Ok(())
    } else {
        Err(FetchError::CannotUnpack)
    }
}

/// 确保 `~/.dct/runtime/node` 里有一份能跑的 Node。已经有了就直接返回，
/// 不重下——这个函数会在每次装 agent 之前被调用。
///
/// 装好之后**顺手把它挂到当前进程的 PATH 上**，这样调用方接着跑 `npm`
/// 就能找到它，不必自己拼路径。
pub fn ensure_node(
    runtime: &Path,
    lang: crate::i18n::Lang,
    p: &dyn Progress,
) -> Result<(), FetchError> {
    if node_installed(runtime) {
        activate(runtime);
        return Ok(());
    }

    let asset = node_asset(std::env::consts::OS, std::env::consts::ARCH, NODE_VERSION)
        .ok_or(FetchError::NoAssetForPlatform)?;
    let base = node_base();

    p.line(&crate::i18n::msg::node_fetching(lang, NODE_VERSION));

    let sums_url = dist_url(&base, NODE_VERSION, "SHASUMS256.txt");
    let sums = get_text(&sums_url)?;
    let want = sha_for(&sums, &asset.file).ok_or(FetchError::Corrupt)?;

    std::fs::create_dir_all(runtime).map_err(|_| FetchError::CannotWrite)?;
    // 解到一个临时目录再挪过去。中途断了不会留下一个半拉的 node 目录
    // 骗过 `node_installed`——那种状态最难查，因为它看起来是装好的。
    let staging = runtime.join(".node-staging");
    let _ = std::fs::remove_dir_all(&staging);

    let archive = runtime.join(&asset.file);
    let url = dist_url(&base, NODE_VERSION, &asset.file);
    download_verified(&url, &archive, &want, p)?;

    let r = unpack(&archive, &staging);
    // 包留着没有意义，几十 MB。不管解得开解不开都删掉。
    let _ = std::fs::remove_file(&archive);
    r?;

    let unpacked = staging.join(&asset.stem);
    if !unpacked.is_dir() {
        let _ = std::fs::remove_dir_all(&staging);
        return Err(FetchError::CannotUnpack);
    }
    let target = node_dir(runtime);
    let _ = std::fs::remove_dir_all(&target);
    std::fs::rename(&unpacked, &target).map_err(|_| FetchError::CannotWrite)?;
    let _ = std::fs::remove_dir_all(&staging);

    if !node_installed(runtime) {
        return Err(FetchError::CannotUnpack);
    }
    activate(runtime);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_platform_dct_ships_for_has_a_node_package() {
        // 这四个正是 release.yml 里出包的那四个目标。dct 装得上的机器，
        // 就必须也拿得到 Node——否则「只装 dct 就能开始」在那台机器上是假的。
        for (os, arch) in [
            ("windows", "x86_64"),
            ("linux", "x86_64"),
            ("macos", "x86_64"),
            ("macos", "aarch64"),
        ] {
            let a = node_asset(os, arch, NODE_VERSION).unwrap_or_else(|| {
                panic!("{os}/{arch} 拿不到 Node 包，那台机器上 dct 装了也没 agent 可用")
            });
            assert!(a.file.starts_with(&a.stem), "文件名该是目录名加后缀：{a:?}");
        }
        assert_eq!(node_asset("plan9", "x86_64", NODE_VERSION), None);
    }

    #[test]
    fn windows_gets_a_zip_and_the_rest_get_tarballs() {
        assert!(node_asset("windows", "x86_64", "v22.11.0")
            .unwrap()
            .file
            .ends_with(".zip"));
        assert!(node_asset("linux", "x86_64", "v22.11.0")
            .unwrap()
            .file
            .ends_with(".tar.gz"));
    }

    /// 老师往 `DCT_NODE_BASE` 里粘一个地址时，末尾带不带斜杠完全是随机的。
    /// 带了就拼出 `//v22.11.0`，多数服务器忍得了，但不是所有——而这个错
    /// 只会在教室里那台机器上出现。
    #[test]
    fn a_trailing_slash_on_the_mirror_does_not_double_up() {
        assert_eq!(
            dist_url("https://m.example/node/", "v1", "a.zip"),
            "https://m.example/node/v1/a.zip"
        );
        assert_eq!(
            dist_url("https://m.example/node", "v1", "a.zip"),
            "https://m.example/node/v1/a.zip"
        );
    }

    /// 国内镜像跟官方的路径结构必须一样，否则换镜像这件事就不是换个前缀。
    /// 这条钉的是那个假设本身。
    #[test]
    fn the_mirror_uses_the_same_layout_as_the_official_source() {
        let a = node_asset("linux", "x86_64", NODE_VERSION).unwrap();
        let official = dist_url(DEFAULT_NODE_BASE, NODE_VERSION, &a.file);
        let mirror = dist_url(CN_NODE_BASE, NODE_VERSION, &a.file);
        assert!(official.ends_with(&format!("/{NODE_VERSION}/{}", a.file)));
        assert!(mirror.ends_with(&format!("/{NODE_VERSION}/{}", a.file)));
    }

    /// SHASUMS256.txt 里几十个包名互相是前缀。挑错行 = 拿另一个包的哈希
    /// 去验这个包，而那种失败看起来像「网络不好」。
    #[test]
    fn a_checksum_is_matched_by_whole_filename_not_by_substring() {
        let sums = "\
aaa  node-v22.11.0-linux-x64.tar.xz
bbb  node-v22.11.0-linux-x64.tar.gz
ccc  node-v22.11.0-win-x64.zip
";
        assert_eq!(
            sha_for(sums, "node-v22.11.0-linux-x64.tar.gz").as_deref(),
            Some("bbb")
        );
        assert_eq!(
            sha_for(sums, "node-v22.11.0-linux-x64.tar.xz").as_deref(),
            Some("aaa")
        );
        assert_eq!(sha_for(sums, "node-v99-nope.zip"), None);
    }

    /// 有些校验和文件用二进制模式的 `哈希 *文件名`。同 `install.sh` 里
    /// 那处踩过的坑。
    #[test]
    fn a_binary_mode_star_before_the_name_still_matches() {
        assert_eq!(sha_for("aaa *x.zip", "x.zip").as_deref(), Some("aaa"));
    }

    #[test]
    fn the_bin_dir_is_the_node_dir_on_windows_and_bin_below_it_elsewhere() {
        let rt = Path::new("/r");
        let bin = node_bin_dir(rt);
        if cfg!(windows) {
            assert_eq!(bin, node_dir(rt));
        } else {
            assert_eq!(bin, node_dir(rt).join("bin"));
        }
    }

    /// 守护进程每次启动都会 activate 一次。不去重的话，PATH 每启动一次
    /// 长一截，而 Windows 上那东西有长度上限。
    #[test]
    fn activating_twice_does_not_add_the_directory_twice() {
        let dir = Path::new(if cfg!(windows) {
            "C:\\r\\node"
        } else {
            "/r/node"
        });
        let once = prepend_path("/usr/bin", dir);
        let twice = prepend_path(&once, dir);
        assert_eq!(once, twice, "第二次不该再加一遍：{twice}");
    }

    #[test]
    fn prepending_puts_our_node_ahead_of_whatever_else_is_there() {
        let dir = Path::new(if cfg!(windows) {
            "C:\\r\\node"
        } else {
            "/r/node"
        });
        let out = prepend_path("/usr/bin", dir);
        assert!(
            out.starts_with(&*dir.to_string_lossy()),
            "自带的那份要排在前面，否则系统里那份旧 Node 会赢：{out}"
        );
        assert!(out.contains("/usr/bin"), "原来的 PATH 不能丢：{out}");
    }

    #[test]
    fn an_empty_path_does_not_grow_a_stray_separator() {
        let dir = Path::new(if cfg!(windows) {
            "C:\\r\\node"
        } else {
            "/r/node"
        });
        let out = prepend_path("", dir);
        assert_eq!(out, dir.to_string_lossy());
    }

    /// 路径都从 socket 推出来，跟 config.toml / secrets.toml / profiles 一样。
    /// 这条是测试能把整份运行时指进临时目录的前提。
    #[test]
    fn the_runtime_lives_next_to_the_socket() {
        let rt = runtime_dir_for_socket(Path::new("/home/x/.dct/daemon.sock"));
        assert_eq!(rt, PathBuf::from("/home/x/.dct/runtime"));
    }

    /// **这条是整个模块存在的理由，钉的是那条端到端的因果。**
    ///
    /// 装进自带运行时的 agent，必须能被 `command_exists` 看见——那正是
    /// 看板判断「这个 agent 可用吗」用的同一个函数（`profile.rs`）。
    /// 看不见的话，学生刚刚看着 dct 把 claude 装完，回到看板上它还是灰的。
    ///
    /// 改进程级的 PATH 是有意的：仓库的测试本来就要求 `--test-threads=1`
    /// （见 README），而 `pty.rs` 里已有的几条测试也是这么做的。
    #[test]
    fn an_agent_installed_into_our_own_runtime_is_found_by_the_same_check_the_board_uses() {
        let tmp = tempfile::tempdir().unwrap();
        let rt = tmp.path();
        let bin = node_bin_dir(rt);
        std::fs::create_dir_all(&bin).unwrap();

        // 一份假的 node，只为让 `node_installed` 认账——`activate` 拒绝
        // 挂一个还没装好的运行时。
        let node = bin.join(if cfg!(windows) { "node.exe" } else { "node" });
        std::fs::write(&node, b"not really node").unwrap();

        // npm 装出来的 agent 长什么样：Windows 上是 `.cmd`，Unix 上是一个
        // 带可执行位的文件。这正是 `sys::fs::command_exists` 两边分别认的东西。
        let agent = bin.join(if cfg!(windows) {
            "pretend-agent.cmd"
        } else {
            "pretend-agent"
        });
        std::fs::write(
            &agent,
            b"#!/bin/sh
exit 0
",
        )
        .unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&agent, std::fs::Permissions::from_mode(0o755)).unwrap();
        }

        let saved = std::env::var("PATH").unwrap_or_default();
        assert!(
            !crate::profile::command_exists("pretend-agent"),
            "挂之前不该找得到，否则这条测试证明不了任何事"
        );
        activate(rt);
        let found = crate::profile::command_exists("pretend-agent");
        std::env::set_var("PATH", &saved);

        assert!(
            found,
            "装进自带运行时的 agent 必须被看板那个判断看见，否则装完还是灰的"
        );
    }

    /// 目录在、但里面没有 node，得算「没装」。上次解压到一半断电就是这个
    /// 状态，而它是最难查的一种：看起来是装好的。
    #[test]
    fn a_half_unpacked_node_directory_does_not_count_as_installed() {
        let tmp = tempfile::tempdir().unwrap();
        let rt = tmp.path();
        assert!(!node_installed(rt));
        std::fs::create_dir_all(node_bin_dir(rt)).unwrap();
        assert!(!node_installed(rt), "空目录不算装好");
    }
}
