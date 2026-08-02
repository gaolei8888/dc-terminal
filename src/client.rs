use anyhow::{Context, Result};
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::Path;

use crate::proto::{Request, Response};

pub struct Client {
    reader: BufReader<UnixStream>,
    writer: UnixStream,
}

impl Client {
    pub fn connect(socket: &Path) -> Result<Client> {
        let stream = UnixStream::connect(socket)
            .with_context(|| format!("连不上守护进程: {}", socket.display()))?;
        Ok(Client {
            reader: BufReader::new(stream.try_clone()?),
            writer: stream,
        })
    }

    pub fn call(&mut self, req: Request) -> Result<Response> {
        writeln!(self.writer, "{}", serde_json::to_string(&req)?)?;
        self.writer.flush()?;
        let mut line = String::new();
        self.reader
            .read_line(&mut line)
            .context("守护进程没有回应")?;
        Ok(serde_json::from_str(&line)?)
    }
}
