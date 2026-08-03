use anyhow::{Context, Result};
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::time::Duration;

use crate::proto::{Request, Response};

/// 读超时：TUI 主循环在 `term.draw` 之前会先发 `List`，守护进程一卡住整个界面
/// 就会跟着冻结，连 `q` 都按不动。设成 5 秒，比 `tests/concurrency.rs` 里那个
/// 故意造出的“几千个文件的仓库 + git worktree add”慢 `Create`（实测约 1 秒）
/// 留出充分余量，不会把正常的慢操作误判成断连。
const READ_TIMEOUT: Duration = Duration::from_secs(5);

pub struct Client {
    reader: BufReader<UnixStream>,
    writer: UnixStream,
}

impl Client {
    pub fn connect(socket: &Path) -> Result<Client> {
        let stream = UnixStream::connect(socket)
            .with_context(|| format!("连不上守护进程: {}", socket.display()))?;
        stream
            .set_read_timeout(Some(READ_TIMEOUT))
            .context("设置读超时失败")?;
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
