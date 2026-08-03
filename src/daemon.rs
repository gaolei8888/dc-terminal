use anyhow::Result;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use crate::profile::Profile;
use crate::proto::{Request, Response};
use crate::session::SessionManager;

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
    if let Some(parent) = socket.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let _ = std::fs::remove_file(socket);
    let listener = UnixListener::bind(socket)?;

    let tick_mgr = mgr.clone();
    std::thread::spawn(move || loop {
        std::thread::sleep(Duration::from_millis(200));
        tick_mgr.tick();
    });

    for conn in listener.incoming() {
        let conn = conn?;
        let m = mgr.clone();
        std::thread::spawn(move || {
            if let Err(e) = serve(conn, m) {
                eprintln!("连接处理失败: {e}");
            }
        });
    }
    Ok(())
}

fn serve(stream: UnixStream, mgr: Arc<SessionManager>) -> Result<()> {
    let mut out = stream.try_clone()?;
    let reader = BufReader::new(stream);
    for line in reader.lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        let resp = match serde_json::from_str::<Request>(&line) {
            Ok(req) => handle(req, &mgr),
            Err(e) => Response::Error(format!("请求解析失败: {e}")),
        };
        writeln!(out, "{}", serde_json::to_string(&resp)?)?;
        out.flush()?;
    }
    Ok(())
}

fn handle(req: Request, mgr: &Arc<SessionManager>) -> Response {
    let r: anyhow::Result<Response> = match req {
        Request::List => Ok(Response::Sessions(mgr.list())),
        Request::Profiles => Ok(Response::Profiles(
            Profile::builtin_names()
                .iter()
                .map(|s| s.to_string())
                .collect(),
        )),
        Request::Create { dir, profile } => mgr
            .create(&PathBuf::from(dir), &profile)
            .map(|id| Response::Created { id }),
        Request::Input { id, text } => mgr.send_input(id, &text).map(|_| Response::Ok),
        Request::Screen { id } => mgr
            .screen(id)
            .map(|(lines, cursor)| Response::Screen { lines, cursor }),
        Request::Stop { id } => mgr.stop(id).map(|_| Response::Ok),
        Request::Undo { id } => mgr.undo(id).map(|_| Response::Ok),
        Request::Diff { id } => mgr.diff(id).map(Response::Diff),
    };
    r.unwrap_or_else(|e| Response::Error(e.to_string()))
}
