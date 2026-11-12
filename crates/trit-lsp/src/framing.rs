//! LSP 的 stdio 帧：`Content-Length: N\r\n\r\n<JSON>`。

use std::io::{self, BufRead, Write};

use serde_json::Value;

mod header;
pub use header::{MAX_FRAME_BYTES, MAX_HEADER_LINE_BYTES};
use header::{frame_len_ok, read_header_line};

/// 读一帧；对端 EOF 返回 `Ok(None)`。
///
/// 只认 `Content-Length`（LSP 必带）；其余头（`Content-Type` 等）跳过。
/// 帧头行与帧体都有硬顶，超限在分配前拒绝（结构化错误，不撑内存）。
pub fn read_message<R: BufRead>(reader: &mut R) -> io::Result<Option<Value>> {
    let mut len: Option<usize> = None;
    loop {
        let Some(line) = read_header_line(reader)? else {
            return if len.is_none() {
                Ok(None)
            } else {
                Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "帧头之后遇到 EOF",
                ))
            };
        };
        let trimmed = line.trim_end_matches(['\r', '\n']);
        if trimmed.is_empty() {
            break;
        }
        if let Some(rest) = trimmed.strip_prefix("Content-Length:") {
            let parsed = rest.trim().parse::<usize>().map_err(|e| {
                io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("Content-Length 非法：{e}"),
                )
            })?;
            len = Some(parsed);
        }
    }
    let len =
        len.ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "帧头缺少 Content-Length"))?;
    if !frame_len_ok(len) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("Content-Length 超过上限（{MAX_FRAME_BYTES} 字节）：{len}"),
        ));
    }
    let mut buf: Vec<u8> = Vec::new();
    buf.try_reserve_exact(len).map_err(|_| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!("帧体分配失败（{len} 字节）"),
        )
    })?;
    buf.resize(len, 0u8);
    reader.read_exact(&mut buf)?;
    let text = String::from_utf8(buf)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, format!("帧体不是 UTF-8：{e}")))?;
    let value = serde_json::from_str(&text)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, format!("帧体不是 JSON：{e}")))?;
    Ok(Some(value))
}

/// 写一帧并 flush（stdio 管道必须 flush，否则客户端等不到）。
pub fn write_message<W: Write>(writer: &mut W, value: &Value) -> io::Result<()> {
    let body =
        serde_json::to_vec(value).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
    write!(writer, "Content-Length: {}\r\n\r\n", body.len())?;
    writer.write_all(&body)?;
    writer.flush()
}

#[cfg(test)]
mod tests;
