//! 只读 pgwire 服务：把 SQLite 的 `trit_trace` 物化库直接暴露成 PostgreSQL 协议。
//!
//! 支持简单查询（`Q`）与扩展查询（Parse/Describe/Bind/Execute/Close/Sync，
//! 见 [`extended`]）；不支持 COPY 与认证/TLS——`SSLRequest` 回 `N`，客户端退回明文。
//!
//! 这样轨迹 WAL 完全不参与服务，写入方与 `trace sync` 都能与服务并发。

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};

use rusqlite::types::ValueRef;
use rusqlite::{Connection, OpenFlags};

use crate::Error;

#[path = "pgwire_extended.rs"]
mod extended;
const PROTOCOL_V3: i32 = 196608;
const SSL_REQUEST: i32 = 80877103;
const MAX_STARTUP: usize = 1 << 20;
const MAX_QUERY: usize = 1 << 22;
/// 所有列按 text 格式返回（OID 25）：SQLite 是动态类型，文本最不容易出错。
pub(super) const TEXT_OID: u32 = 25;
/// 在 `listen` 上提供只读查询；阻塞直到监听关闭。
pub fn serve(sqlite: &Path, listen: &str) -> Result<(), Error> {
    let listener = TcpListener::bind(listen).map_err(|error| io(listen, error))?;
    for stream in listener.incoming() {
        let Ok(stream) = stream else { continue };
        let path = sqlite.to_path_buf();
        std::thread::spawn(move || {
            let _ = handle_session(stream, &path);
        });
    }
    Ok(())
}
fn handle_session(mut stream: TcpStream, sqlite: &Path) -> std::io::Result<()> {
    if !handshake(&mut stream)? {
        return Ok(());
    }
    let flags = OpenFlags::SQLITE_OPEN_READ_ONLY;
    let conn = match Connection::open_with_flags(sqlite, flags) {
        Ok(conn) => conn,
        Err(error) => {
            let _ = error_response(&mut stream, "58000", &error.to_string());
            let _ = ready(&mut stream);
            return Ok(());
        }
    };
    let mut session = extended::Session::default();
    loop {
        let mut tag = [0u8; 1];
        if stream.read_exact(&mut tag).is_err() {
            return Ok(());
        }
        match tag[0] {
            b'X' => return Ok(()),
            b'P' | b'B' | b'D' | b'E' | b'C' | b'S' => {
                let body = read_body(&mut stream, MAX_QUERY)?;
                session.handle(tag[0], &body, &conn, &mut stream)?;
            }
            b'Q' => {
                let body = read_body(&mut stream, MAX_QUERY)?;
                let sql = c_string(&body);
                run_query(&conn, &mut stream, &sql)?;
            }
            _ => {
                let _ = read_body(&mut stream, MAX_QUERY);
                error_response(
                    &mut stream,
                    "0A000",
                    "only the simple query protocol is supported",
                )?;
                ready(&mut stream)?;
            }
        }
    }
}
/// startup（含 SSLRequest 往返）；返回 `false` 表示握手失败、不要继续。
fn handshake(stream: &mut TcpStream) -> std::io::Result<bool> {
    let mut payload = read_startup(stream)?;
    loop {
        let code = protocol_of(&payload)?;
        if code == SSL_REQUEST {
            stream.write_all(b"N")?;
            payload = read_startup(stream)?;
            continue;
        }
        if code != PROTOCOL_V3 {
            error_response(stream, "08P01", "unsupported protocol version")?;
            return Ok(false);
        }
        break;
    }
    write_message(stream, b'R', &0i32.to_be_bytes())?;
    for (name, value) in [
        ("server_version", "14.0"),
        ("client_encoding", "UTF8"),
        ("DateStyle", "ISO, MDY"),
        ("integer_datetimes", "on"),
        ("standard_conforming_strings", "on"),
    ] {
        let mut body = Vec::new();
        body.extend_from_slice(name.as_bytes());
        body.push(0);
        body.extend_from_slice(value.as_bytes());
        body.push(0);
        write_message(stream, b'S', &body)?;
    }
    let mut key = Vec::new();
    key.extend_from_slice(&(std::process::id() as i32).to_be_bytes());
    key.extend_from_slice(&now_secret().to_be_bytes());
    write_message(stream, b'K', &key)?;
    ready(stream)?;
    Ok(true)
}
fn run_query(conn: &Connection, stream: &mut TcpStream, sql: &str) -> std::io::Result<()> {
    let trimmed = sql.trim().trim_end_matches(';').trim();
    if trimmed.is_empty() {
        return write_message(stream, b'I', &[]);
    }
    let mut stmt = match conn.prepare(trimmed) {
        Ok(stmt) => stmt,
        Err(error) => {
            error_response(stream, "42000", &error.to_string())?;
            return ready(stream);
        }
    };
    let columns = stmt.column_count();
    if columns == 0 {
        error_response(
            stream,
            "0A000",
            "only read-only SELECT statements are supported",
        )?;
        return ready(stream);
    }
    let names: Vec<String> = (0..columns)
        .map(|index| stmt.column_name(index).unwrap_or("?").to_string())
        .collect();
    let mut description = Vec::new();
    description.extend_from_slice(&(columns as i16).to_be_bytes());
    for name in &names {
        description.extend_from_slice(name.as_bytes());
        description.push(0);
        description.extend_from_slice(&0u32.to_be_bytes()); // table oid
        description.extend_from_slice(&0i16.to_be_bytes()); // attnum
        description.extend_from_slice(&TEXT_OID.to_be_bytes());
        description.extend_from_slice(&(-1i16).to_be_bytes()); // typlen
        description.extend_from_slice(&(-1i32).to_be_bytes()); // typmod
        description.extend_from_slice(&0i16.to_be_bytes()); // text format
    }
    write_message(stream, b'T', &description)?;
    let mut rows = match stmt.query([]) {
        Ok(rows) => rows,
        Err(error) => {
            error_response(stream, "42000", &error.to_string())?;
            return ready(stream);
        }
    };
    let mut count = 0usize;
    while let Some(row) = rows.next().map_err(other)? {
        let mut body = Vec::new();
        body.extend_from_slice(&(columns as i16).to_be_bytes());
        for index in 0..columns {
            let value = row.get_ref(index).map_err(other)?;
            match value {
                ValueRef::Null => body.extend_from_slice(&(-1i32).to_be_bytes()),
                ValueRef::Integer(number) => push_text(&mut body, number.to_string()),
                ValueRef::Real(number) => push_text(&mut body, number.to_string()),
                ValueRef::Text(bytes) => push_bytes(&mut body, bytes),
                ValueRef::Blob(bytes) => push_bytes(&mut body, bytes),
            }
        }
        write_message(stream, b'D', &body)?;
        count += 1;
    }
    let mut tag = format!("SELECT {count}").into_bytes();
    tag.push(0);
    write_message(stream, b'C', &tag)?;
    ready(stream)
}
pub(super) fn push_text(body: &mut Vec<u8>, text: String) {
    body.extend_from_slice(&(text.len() as i32).to_be_bytes());
    body.extend_from_slice(text.as_bytes());
}
pub(super) fn push_bytes(body: &mut Vec<u8>, bytes: &[u8]) {
    body.extend_from_slice(&(bytes.len() as i32).to_be_bytes());
    body.extend_from_slice(bytes);
}

fn read_startup(stream: &mut TcpStream) -> std::io::Result<Vec<u8>> {
    read_body(stream, MAX_STARTUP)
}

/// 读一条消息体（长度字段含自身 4 字节）。
fn read_body(stream: &mut TcpStream, max: usize) -> std::io::Result<Vec<u8>> {
    let mut header = [0u8; 4];
    stream.read_exact(&mut header)?;
    let length = i32::from_be_bytes(header);
    if length < 4 || (length as usize) > max {
        return Err(other(format!("invalid message length {length}")));
    }
    let mut body = vec![0u8; length as usize - 4];
    stream.read_exact(&mut body)?;
    Ok(body)
}

fn protocol_of(payload: &[u8]) -> std::io::Result<i32> {
    if payload.len() < 4 {
        return Err(other("startup packet too short"));
    }
    Ok(i32::from_be_bytes([
        payload[0], payload[1], payload[2], payload[3],
    ]))
}

fn c_string(body: &[u8]) -> String {
    let end = body
        .iter()
        .position(|byte| *byte == 0)
        .unwrap_or(body.len());
    String::from_utf8_lossy(&body[..end]).into_owned()
}

pub(super) fn write_message(stream: &mut TcpStream, tag: u8, body: &[u8]) -> std::io::Result<()> {
    let mut buffer = Vec::with_capacity(body.len() + 5);
    buffer.push(tag);
    buffer.extend_from_slice(&((body.len() + 4) as i32).to_be_bytes());
    buffer.extend_from_slice(body);
    stream.write_all(&buffer)
}

pub(super) fn ready(stream: &mut TcpStream) -> std::io::Result<()> {
    write_message(stream, b'Z', b"I")
}

pub(super) fn error_response(
    stream: &mut TcpStream,
    code: &str,
    message: &str,
) -> std::io::Result<()> {
    let mut body = Vec::new();
    body.push(b'S');
    body.extend_from_slice(b"ERROR\0");
    body.push(b'V');
    body.extend_from_slice(b"ERROR\0");
    body.push(b'C');
    body.extend_from_slice(code.as_bytes());
    body.push(0);
    body.push(b'M');
    body.extend_from_slice(message.as_bytes());
    body.push(0);
    body.push(0);
    write_message(stream, b'E', &body)
}

fn now_secret() -> i32 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.subsec_nanos() as i32)
        .unwrap_or(0)
}

fn other(message: impl std::fmt::Display) -> std::io::Error {
    std::io::Error::other(message.to_string())
}

fn io(path: &str, error: impl std::fmt::Display) -> Error {
    Error::TraceIo {
        path: path.to_string(),
        message: error.to_string(),
    }
}

/// 供测试与调用方确认服务目标库。
pub fn materialized_path(db: &Path) -> PathBuf {
    db.with_extension("sqlite")
}
