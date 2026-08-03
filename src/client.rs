use anyhow::{bail, Context, Result};
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::proto::{Request, Response};

/// 读超时：TUI 主循环在 `term.draw` 之前会先发 `List`，守护进程一卡住整个界面
/// 就会跟着冻结，连 `q` 都按不动。5 秒对正常操作留了充分余量。
const READ_TIMEOUT: Duration = Duration::from_secs(5);

/// 一条到守护进程的连接。
///
/// **超时之后必须丢掉连接重连，不能接着用。** 超时只是"这次没等到"，
/// 迟到的响应仍然会留在 socket 里；接着发下一个请求就会读到上一次的
/// 响应，从此每次都差一格，界面会永远显示错的东西。丢掉重连是唯一
/// 能保证请求和响应对得上的办法。
pub struct Client {
    socket: PathBuf,
    conn: Option<Conn>,
}

struct Conn {
    reader: BufReader<UnixStream>,
    writer: UnixStream,
}

impl Client {
    pub fn connect(socket: &Path) -> Result<Client> {
        let mut c = Client {
            socket: socket.to_path_buf(),
            conn: None,
        };
        c.reconnect()?;
        Ok(c)
    }

    fn reconnect(&mut self) -> Result<()> {
        let stream = UnixStream::connect(&self.socket)
            .with_context(|| format!("连不上守护进程: {}", self.socket.display()))?;
        stream
            .set_read_timeout(Some(READ_TIMEOUT))
            .context("设置读超时失败")?;
        self.conn = Some(Conn {
            reader: BufReader::new(stream.try_clone()?),
            writer: stream,
        });
        Ok(())
    }

    pub fn call(&mut self, req: Request) -> Result<Response> {
        if self.conn.is_none() {
            self.reconnect()?;
        }
        match self.try_call(&req) {
            Ok(resp) => Ok(resp),
            Err(e) => {
                // 任何一次读写出错（含超时）都意味着这条连接的请求/响应
                // 可能已经错位，不能再用。下次调用会自动重连。
                self.conn = None;
                Err(e)
            }
        }
    }

    fn try_call(&mut self, req: &Request) -> Result<Response> {
        let conn = self.conn.as_mut().context("连接已断开")?;
        writeln!(conn.writer, "{}", serde_json::to_string(req)?)?;
        conn.writer.flush()?;

        let mut line = String::new();
        let n = conn
            .reader
            .read_line(&mut line)
            .context("守护进程没有回应")?;
        if n == 0 {
            bail!("守护进程关闭了连接");
        }
        Ok(serde_json::from_str(&line)?)
    }
}
