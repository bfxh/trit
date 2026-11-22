//! PostgreSQL 扩展查询协议的最小实现：Parse/Describe/Bind/Execute/Close/Sync。
//!
//! 只支持 text 格式的参数与结果；`$n` 占位符会转成 SQLite 的 `?n`。

use std::collections::HashMap;
use std::io;
use std::net::TcpStream;

use rusqlite::Connection;
use rusqlite::types::ValueRef;

use super::{TEXT_OID, error_response, push_bytes, push_text, ready, write_message};

#[derive(Default)]
pub(super) struct Session {
    statements: HashMap<String, String>,
    portals: HashMap<String, Portal>,
    ignore: bool,
}

struct Portal {
    sql: String,
    params: Vec<Option<String>>,
}

impl Session {
    /// 处理一条扩展协议消息；错误后进入「忽略到 Sync」状态（与 PG 一致）。
    pub(super) fn handle(
        &mut self,
        tag: u8,
        body: &[u8],
        conn: &Connection,
        stream: &mut TcpStream,
    ) -> io::Result<()> {
        if self.ignore && tag != b'S' {
            return Ok(());
        }
        let result = match tag {
            b'P' => self.parse(body, stream),
            b'B' => self.bind(body, stream),
            b'D' => self.describe(body, conn, stream),
            b'E' => self.execute(body, conn, stream),
            b'C' => self.close(body, stream),
            b'S' => {
                self.ignore = false;
                ready(stream)
            }
            _ => {
                error_response(stream, "08P01", "unsupported message")?;
                self.ignore = true;
                Ok(())
            }
        };
        if result.is_err() {
            self.ignore = true;
        }
        result
    }

    fn parse(&mut self, body: &[u8], stream: &mut TcpStream) -> io::Result<()> {
        let mut offset = 0;
        let name = take_cstr(body, &mut offset)?;
        let sql = take_cstr(body, &mut offset)?;
        take_i16(body, &mut offset)?;
        let sql = convert_placeholders(&sql);
        let params = placeholder_count(&sql);
        self.statements.insert(name, sql);
        write_message(stream, b'1', &[])?;
        let mut description = Vec::new();
        description.extend_from_slice(&(params as i16).to_be_bytes());
        for _ in 0..params {
            description.extend_from_slice(&TEXT_OID.to_be_bytes());
        }
        write_message(stream, b't', &description)
    }

    fn bind(&mut self, body: &[u8], stream: &mut TcpStream) -> io::Result<()> {
        let mut offset = 0;
        let portal = take_cstr(body, &mut offset)?;
        let statement = take_cstr(body, &mut offset)?;
        let formats = take_i16(body, &mut offset)?;
        offset += (formats.max(0) as usize) * 2;
        let count = take_i16(body, &mut offset)?;
        let mut params = Vec::with_capacity(count.max(0) as usize);
        for _ in 0..count.max(0) {
            let length = take_i32(body, &mut offset)?;
            if length < 0 {
                params.push(None);
            } else {
                let end = offset + length as usize;
                if end > body.len() {
                    return Err(io::Error::other("bind parameter out of range"));
                }
                params.push(Some(
                    String::from_utf8_lossy(&body[offset..end]).into_owned(),
                ));
                offset = end;
            }
        }
        take_i16(body, &mut offset)?;
        let Some(sql) = self.statements.get(&statement).cloned() else {
            return error_response(stream, "26000", "unknown prepared statement");
        };
        self.portals.insert(portal, Portal { sql, params });
        write_message(stream, b'2', &[])
    }

    fn describe(
        &mut self,
        body: &[u8],
        conn: &Connection,
        stream: &mut TcpStream,
    ) -> io::Result<()> {
        if body.is_empty() {
            return Err(io::Error::other("describe without kind"));
        }
        let kind = body[0];
        let mut offset = 1;
        let name = take_cstr(body, &mut offset)?;
        let sql = if kind == b'S' {
            self.statements.get(&name).cloned()
        } else {
            self.portals.get(&name).map(|portal| portal.sql.clone())
        };
        let Some(sql) = sql else {
            return error_response(stream, "26000", "unknown statement or portal");
        };
        describe_sql(conn, stream, &sql)
    }

    fn execute(
        &mut self,
        body: &[u8],
        conn: &Connection,
        stream: &mut TcpStream,
    ) -> io::Result<()> {
        let mut offset = 0;
        let portal = take_cstr(body, &mut offset)?;
        let _max_rows = take_i32(body, &mut offset)?;
        let Some(portal) = self.portals.get(&portal) else {
            return error_response(stream, "34000", "unknown portal");
        };
        execute_portal(conn, stream, &portal.sql, &portal.params)
    }

    fn close(&mut self, body: &[u8], stream: &mut TcpStream) -> io::Result<()> {
        if body.is_empty() {
            return Err(io::Error::other("close without kind"));
        }
        let kind = body[0];
        let mut offset = 1;
        let name = take_cstr(body, &mut offset)?;
        if kind == b'S' {
            self.statements.remove(&name);
        } else {
            self.portals.remove(&name);
        }
        write_message(stream, b'3', &[])
    }
}

fn describe_sql(conn: &Connection, stream: &mut TcpStream, sql: &str) -> io::Result<()> {
    let stmt = match conn.prepare(sql) {
        Ok(stmt) => stmt,
        Err(error) => return error_response(stream, "42000", &error.to_string()),
    };
    let columns = stmt.column_count();
    let mut description = Vec::new();
    description.extend_from_slice(&(columns as i16).to_be_bytes());
    for index in 0..columns {
        let name = stmt.column_name(index).unwrap_or("?");
        description.extend_from_slice(name.as_bytes());
        description.push(0);
        description.extend_from_slice(&0u32.to_be_bytes());
        description.extend_from_slice(&0i16.to_be_bytes());
        description.extend_from_slice(&TEXT_OID.to_be_bytes());
        description.extend_from_slice(&(-1i16).to_be_bytes());
        description.extend_from_slice(&(-1i32).to_be_bytes());
        description.extend_from_slice(&0i16.to_be_bytes());
    }
    write_message(stream, b'T', &description)
}

fn execute_portal(
    conn: &Connection,
    stream: &mut TcpStream,
    sql: &str,
    params: &[Option<String>],
) -> io::Result<()> {
    let mut stmt = match conn.prepare(sql) {
        Ok(stmt) => stmt,
        Err(error) => return error_response(stream, "42000", &error.to_string()),
    };
    let columns = stmt.column_count();
    let mut rows = match stmt.query(rusqlite::params_from_iter(
        params.iter().map(|p| p.as_deref()),
    )) {
        Ok(rows) => rows,
        Err(error) => return error_response(stream, "42000", &error.to_string()),
    };
    let mut count = 0usize;
    while let Some(row) = rows.next().map_err(io::Error::other)? {
        let mut body = Vec::new();
        body.extend_from_slice(&(columns as i16).to_be_bytes());
        for index in 0..columns {
            match row.get_ref(index).map_err(io::Error::other)? {
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
    write_message(stream, b'C', &tag)
}

fn take_cstr(body: &[u8], offset: &mut usize) -> io::Result<String> {
    let start = *offset;
    let Some(end) = body[start..].iter().position(|byte| *byte == 0) else {
        return Err(io::Error::other("unterminated string"));
    };
    let value = String::from_utf8_lossy(&body[start..start + end]).into_owned();
    *offset = start + end + 1;
    Ok(value)
}

fn take_i16(body: &[u8], offset: &mut usize) -> io::Result<i16> {
    if *offset + 2 > body.len() {
        return Err(io::Error::other("truncated int16"));
    }
    let value = i16::from_be_bytes([body[*offset], body[*offset + 1]]);
    *offset += 2;
    Ok(value)
}

fn take_i32(body: &[u8], offset: &mut usize) -> io::Result<i32> {
    if *offset + 4 > body.len() {
        return Err(io::Error::other("truncated int32"));
    }
    let value = i32::from_be_bytes([
        body[*offset],
        body[*offset + 1],
        body[*offset + 2],
        body[*offset + 3],
    ]);
    *offset += 4;
    Ok(value)
}

/// `$n` → `?n`；跳过单引号字符串里的内容。
fn convert_placeholders(sql: &str) -> String {
    let mut out = String::with_capacity(sql.len());
    let mut chars = sql.chars().peekable();
    let mut in_string = false;
    while let Some(ch) = chars.next() {
        if ch == '\'' {
            in_string = !in_string;
            out.push(ch);
            continue;
        }
        if !in_string && ch == '$' {
            let mut digits = String::new();
            while let Some(&next) = chars.peek() {
                if next.is_ascii_digit() {
                    digits.push(next);
                    chars.next();
                } else {
                    break;
                }
            }
            if digits.is_empty() {
                out.push('$');
            } else {
                out.push('?');
                out.push_str(&digits);
            }
            continue;
        }
        out.push(ch);
    }
    out
}

/// SQL 里 `$n` 的最大序号。
fn placeholder_count(sql: &str) -> usize {
    let mut max = 0usize;
    let bytes = sql.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'$' {
            let mut end = index + 1;
            while end < bytes.len() && bytes[end].is_ascii_digit() {
                end += 1;
            }
            if end > index + 1
                && let Ok(value) = sql[index + 1..end].parse::<usize>()
            {
                max = max.max(value);
            }
            index = end;
            continue;
        }
        index += 1;
    }
    max
}
