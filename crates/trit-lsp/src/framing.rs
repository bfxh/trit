//! LSP 的 stdio 帧：`Content-Length: N\r\n\r\n<JSON>`。

use std::io::{self, BufRead, Write};

use serde_json::Value;

/// 读一帧；对端 EOF 返回 `Ok(None)`。
///
/// 只认 `Content-Length`（LSP 必带）；其余头（`Content-Type` 等）跳过。
pub fn read_message<R: BufRead>(reader: &mut R) -> io::Result<Option<Value>> {
    let mut len: Option<usize> = None;
    loop {
        let mut line = String::new();
        let n = reader.read_line(&mut line)?;
        if n == 0 {
            return if len.is_none() {
                Ok(None)
            } else {
                Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "帧头之后遇到 EOF",
                ))
            };
        }
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
    let mut buf = vec![0u8; len];
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
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn round_trip() {
        let mut buf = Vec::new();
        let msg = serde_json::json!({"jsonrpc": "2.0", "id": 1, "method": "initialize"});
        write_message(&mut buf, &msg).expect("写");
        let mut cur = Cursor::new(buf);
        let got = read_message(&mut cur).expect("读").expect("有帧");
        assert_eq!(got, msg);
        assert!(read_message(&mut cur).expect("EOF 正常").is_none());
    }

    #[test]
    fn eof_without_frame_is_none() {
        let mut cur = Cursor::new(Vec::new());
        assert!(read_message(&mut cur).expect("EOF").is_none());
    }

    #[test]
    fn missing_length_is_error() {
        let mut cur = Cursor::new(b"Content-Type: application/json\r\n\r\n{}".to_vec());
        assert!(read_message(&mut cur).is_err());
    }

    #[test]
    fn truncated_body_is_error() {
        let mut cur = Cursor::new(b"Content-Length: 10\r\n\r\n{}".to_vec());
        assert!(read_message(&mut cur).is_err());
    }
}
