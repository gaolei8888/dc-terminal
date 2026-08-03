use serde::{Deserialize, Serialize};
use std::path::PathBuf;

use crate::git::FileStat;
use crate::pty::ScreenSpan;
use crate::session::SessionInfo;

#[derive(Debug, Serialize, Deserialize)]
pub enum Request {
    List,
    Create { dir: String, profile: String },
    Input { id: u32, text: String },
    Screen { id: u32 },
    Resize { id: u32, rows: u16, cols: u16 },
    Stop { id: u32 },
    Undo { id: u32 },
    Diff { id: u32 },
    Profiles,
    Projects,
}

#[derive(Debug, Serialize, Deserialize)]
pub enum Response {
    Sessions(Vec<SessionInfo>),
    Created {
        id: u32,
    },
    Screen {
        lines: Vec<Vec<ScreenSpan>>,
        cursor: (u16, u16),
    },
    Diff(Vec<FileStat>),
    Profiles(Vec<String>),
    Projects(Vec<String>),
    Ok,
    Error(String),
}

pub fn socket_path() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".into());
    PathBuf::from(home).join(".dct").join("daemon.sock")
}
