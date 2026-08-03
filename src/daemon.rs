use anyhow::Result;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crate::profile::Profile;
use crate::projects::{store_path_for_socket, Store};
use crate::proto::{Request, Response};
use crate::session::{recover, SessionManager};

pub fn run(socket: &Path) -> Result<()> {
    run_with_manager(socket, Arc::new(SessionManager::new()))
}

/// 供测试注入自定义 `SessionManager`（比如预先 `register_profile` 一个专供测试用的慢
/// profile），`run()` 只是用一个全新的 manager 调用它。
///
/// 这里不再包一层 `Mutex<SessionManager>`：`SessionManager` 自己已经是内部可变的
/// （见 `session.rs` 的注释），各方法自己负责该锁多细的粒度。如果外面再套一把大锁，
/// 就白做了那些细粒度设计——一个连接处理慢请求时又会把其它连接一起卡住。
pub fn run_with_manager(socket: &Path, mgr: Arc<SessionManager>) -> Result<()> {
    // 权限必须收紧到只有属主可访问。这个 socket 能开会话、能往会话里发任意
    // 输入——谁连得上，谁就能在这台机器上以你的身份执行任意命令。默认的 0755
    // 意味着同机器的其它账号都能连。
    if let Some(parent) = socket.parent() {
        std::fs::create_dir_all(parent)?;
        std::fs::set_permissions(parent, std::fs::Permissions::from_mode(0o700))?;
    }
    let _ = std::fs::remove_file(socket);
    let listener = UnixListener::bind(socket)?;
    std::fs::set_permissions(socket, std::fs::Permissions::from_mode(0o600))?;

    // 存放位置跟着 socket 走，测试把 socket 放临时目录就自动隔离，
    // 不会去动真实的 ~/.dct/projects.json。
    let store = Arc::new(Mutex::new(Store::load(&store_path_for_socket(socket))));

    let tick_mgr = mgr.clone();
    std::thread::spawn(move || loop {
        std::thread::sleep(Duration::from_millis(200));
        tick_mgr.tick();
    });

    for conn in listener.incoming() {
        let conn = conn?;
        let m = mgr.clone();
        let s = store.clone();
        std::thread::spawn(move || {
            if let Err(e) = serve(conn, m, s) {
                eprintln!("连接处理失败: {e}");
            }
        });
    }
    Ok(())
}

fn serve(stream: UnixStream, mgr: Arc<SessionManager>, store: Arc<Mutex<Store>>) -> Result<()> {
    let mut out = stream.try_clone()?;
    let reader = BufReader::new(stream);
    for line in reader.lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        let resp = match serde_json::from_str::<Request>(&line) {
            Ok(req) => handle(req, &mgr, &store),
            Err(e) => Response::Error(format!("请求解析失败: {e}")),
        };
        writeln!(out, "{}", serde_json::to_string(&resp)?)?;
        out.flush()?;
    }
    Ok(())
}

fn handle(req: Request, mgr: &Arc<SessionManager>, store: &Arc<Mutex<Store>>) -> Response {
    let r: anyhow::Result<Response> = match req {
        Request::List => Ok(Response::Sessions(mgr.list())),
        Request::Profiles => Ok(Response::Profiles(
            Profile::builtin_names()
                .iter()
                .map(|s| s.to_string())
                .collect(),
        )),
        Request::Projects => Ok(Response::Projects(recover(store.lock()).list())),
        Request::Create { dir, profile } => {
            let dir = PathBuf::from(dir);
            let r = mgr
                .create(&dir, &profile)
                .map(|id| Response::Created { id });
            // 只有建成功了才记账。建失败的目录进了「最近项目」，
            // 下次还会被选中、还会失败。
            if r.is_ok() {
                recover(store.lock()).touch(&dir);
            }
            r
        }
        Request::Input { id, text } => mgr.send_input(id, &text).map(|_| Response::Ok),
        Request::Screen { id } => mgr
            .screen(id)
            .map(|(lines, cursor)| Response::Screen { lines, cursor }),
        Request::Resize { id, rows, cols } => mgr.resize(id, rows, cols).map(|_| Response::Ok),
        Request::Stop { id } => mgr.stop(id).map(|_| Response::Ok),
        Request::Undo { id } => mgr.undo(id).map(|_| Response::Ok),
        Request::Diff { id } => mgr.diff(id).map(Response::Diff),
    };
    r.unwrap_or_else(|e| Response::Error(e.to_string()))
}
