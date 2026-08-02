use anyhow::Result;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crate::profile::Profile;
use crate::proto::{Request, Response};
use crate::session::SessionManager;

pub fn run(socket: &Path) -> Result<()> {
    if let Some(parent) = socket.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let _ = std::fs::remove_file(socket);
    let listener = UnixListener::bind(socket)?;

    let mgr = Arc::new(Mutex::new(SessionManager::new()));

    let tick_mgr = mgr.clone();
    std::thread::spawn(move || loop {
        std::thread::sleep(Duration::from_millis(200));
        tick_mgr.lock().unwrap().tick();
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

fn serve(stream: UnixStream, mgr: Arc<Mutex<SessionManager>>) -> Result<()> {
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

fn handle(req: Request, mgr: &Arc<Mutex<SessionManager>>) -> Response {
    let mut m = mgr.lock().unwrap();
    let r: anyhow::Result<Response> = match req {
        Request::List => Ok(Response::Sessions(m.list())),
        Request::Profiles => Ok(Response::Profiles(
            Profile::builtin_names()
                .iter()
                .map(|s| s.to_string())
                .collect(),
        )),
        Request::Create { dir, profile } => m
            .create(&PathBuf::from(dir), &profile)
            .map(|id| Response::Created { id }),
        Request::Input { id, text } => m.send_input(id, &text).map(|_| Response::Ok),
        Request::Screen { id } => m.screen(id).map(Response::Screen),
        Request::Stop { id } => m.stop(id).map(|_| Response::Ok),
        Request::Undo { id } => m.undo(id).map(|_| Response::Ok),
        Request::Diff { id } => m.diff(id).map(Response::Diff),
    };
    r.unwrap_or_else(|e| Response::Error(e.to_string()))
}
