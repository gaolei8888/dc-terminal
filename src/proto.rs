use serde::{Deserialize, Serialize};
use std::path::PathBuf;

use crate::git::FileStat;
use crate::session::SessionInfo;

#[derive(Debug, Serialize, Deserialize)]
pub enum Request {
    List,
    Create { dir: String, profile: String },
    Input { id: u32, text: String },
    Screen { id: u32 },
    Stop { id: u32 },
    Undo { id: u32 },
    Diff { id: u32 },
    Profiles,
}

#[derive(Debug, Serialize, Deserialize)]
pub enum Response {
    Sessions(Vec<SessionInfo>),
    Created { id: u32 },
    Screen { text: String, cursor: (u16, u16) },
    Diff(Vec<FileStat>),
    Profiles(Vec<String>),
    Ok,
    Error(String),
}

pub fn socket_path() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".into());
    PathBuf::from(home).join(".dct").join("daemon.sock")
}
