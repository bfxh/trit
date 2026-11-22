//! 清单用的**有序 JSON**：读、写、转义共用这一份。
//!
//! 只覆盖清单会出现的东西：对象、数组、字符串、数字、布尔、null。对象保留写入顺序；
//! 数字保留原始字面量（`0.01` 不会变成 `0.009999999`），所以 `parse` 再 `write`
//! 与自家清单的紧凑写法逐字节相同。顶层换行与缩进由各导出器决定，这里只负责嵌套值。

use std::path::Path;

/// 一个 JSON 值；`Obj` 的字段顺序就是输出顺序。
#[derive(Clone, Debug, PartialEq)]
pub enum Json {
    Null,
    Bool(bool),
    /// 原始数字字面量（保真，不做 f64 往返）。
    Num(String),
    Str(String),
    Arr(Vec<Json>),
    Obj(Vec<(String, Json)>),
}

impl Json {
    pub fn obj() -> Self {
        Json::Obj(Vec::new())
    }

    /// 追加一个字段；重复键不覆盖（清单里不应该有）。
    pub fn put(mut self, key: impl Into<String>, value: impl Into<Json>) -> Self {
        if let Json::Obj(fields) = &mut self {
            fields.push((key.into(), value.into()));
        }
        self
    }

    pub fn arr(items: Vec<Json>) -> Self {
        Json::Arr(items)
    }

    /// 数值数组（清单里的 bbox / centroid）。
    pub fn nums<T: Copy + Into<Json>>(items: &[T]) -> Self {
        Json::Arr(items.iter().map(|v| (*v).into()).collect())
    }

    pub fn num(v: impl ToString) -> Self {
        Json::Num(v.to_string())
    }

    pub fn str(v: impl Into<String>) -> Self {
        Json::Str(v.into())
    }

    pub fn get(&self, key: &str) -> Option<&Json> {
        match self {
            Json::Obj(fields) => fields.iter().find(|(k, _)| k == key).map(|(_, v)| v),
            _ => None,
        }
    }

    pub fn get_str(&self, key: &str) -> Option<&str> {
        self.get(key)?.as_str()
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            Json::Str(s) => Some(s),
            _ => None,
        }
    }

    pub fn as_f64(&self) -> Option<f64> {
        match self {
            Json::Num(n) => n.parse().ok(),
            _ => None,
        }
    }

    pub fn as_u64(&self) -> Option<u64> {
        match self {
            Json::Num(n) => n.parse().ok(),
            _ => None,
        }
    }

    pub fn as_bool(&self) -> Option<bool> {
        match self {
            Json::Bool(b) => Some(*b),
            _ => None,
        }
    }

    pub fn as_array(&self) -> Option<&[Json]> {
        match self {
            Json::Arr(items) => Some(items),
            _ => None,
        }
    }

    /// 紧凑写法：`{"a": 1, "b": [2, 3]}`，与现有手写清单一致。
    pub fn dump(&self) -> String {
        let mut out = String::new();
        self.write(&mut out);
        out
    }

    pub fn write(&self, out: &mut String) {
        match self {
            Json::Null => out.push_str("null"),
            Json::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
            Json::Num(n) => out.push_str(n),
            Json::Str(s) => out.push_str(&json_str(s)),
            Json::Arr(items) => {
                out.push('[');
                for (i, item) in items.iter().enumerate() {
                    if i > 0 {
                        out.push_str(", ");
                    }
                    item.write(out);
                }
                out.push(']');
            }
            Json::Obj(fields) => {
                out.push('{');
                for (i, (k, v)) in fields.iter().enumerate() {
                    if i > 0 {
                        out.push_str(", ");
                    }
                    out.push_str(&json_str(k));
                    out.push_str(": ");
                    v.write(out);
                }
                out.push('}');
            }
        }
    }

    pub fn parse(text: &str) -> Result<Json, String> {
        let mut p = Parser {
            src: text.as_bytes(),
            pos: 0,
        };
        p.ws();
        let value = p.value()?;
        p.ws();
        if p.pos != p.src.len() {
            return Err(format!("位置 {}：JSON 后面还有内容", p.pos));
        }
        Ok(value)
    }
}

impl From<&str> for Json {
    fn from(v: &str) -> Self {
        Json::Str(v.to_string())
    }
}

impl From<&String> for Json {
    fn from(v: &String) -> Self {
        Json::Str(v.clone())
    }
}

impl From<String> for Json {
    fn from(v: String) -> Self {
        Json::Str(v)
    }
}

impl From<bool> for Json {
    fn from(v: bool) -> Self {
        Json::Bool(v)
    }
}

macro_rules! json_num {
    ($($t:ty),*) => {$(
        impl From<$t> for Json {
            fn from(v: $t) -> Self {
                Json::Num(v.to_string())
            }
        }
    )*};
}

json_num!(usize, u32, u64, i32, i64, f32, f64);

/// JSON 字符串转义（清单写入的唯一入口）。
pub fn json_str(s: &str) -> String {
    let mut o = String::from("\"");
    for c in s.chars() {
        match c {
            '"' => o.push_str("\\\""),
            '\\' => o.push_str("\\\\"),
            '\n' => o.push_str("\\n"),
            '\r' => o.push_str("\\r"),
            '\t' => o.push_str("\\t"),
            c if (c as u32) < 0x20 => o.push_str(&format!("\\u{:04x}", c as u32)),
            c => o.push(c),
        }
    }
    o.push('"');
    o
}

pub fn read_json(path: &Path) -> Result<Json, String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    Json::parse(&text).map_err(|e| format!("{}: {e}", path.display()))
}

/// 写清单：紧凑 JSON + 末尾换行（与现有清单文件的收尾一致）。
pub fn write_json(path: &Path, json: &Json) -> Result<(), String> {
    let mut body = json.dump();
    body.push('\n');
    std::fs::write(path, body).map_err(|e| format!("{}: {e}", path.display()))
}

struct Parser<'a> {
    src: &'a [u8],
    pos: usize,
}

impl Parser<'_> {
    fn ws(&mut self) {
        while matches!(self.src.get(self.pos), Some(b' ' | b'\t' | b'\n' | b'\r')) {
            self.pos += 1;
        }
    }

    fn peek(&self) -> Option<u8> {
        self.src.get(self.pos).copied()
    }

    fn expect(&mut self, byte: u8) -> Result<(), String> {
        if self.peek() == Some(byte) {
            self.pos += 1;
            Ok(())
        } else {
            Err(format!("位置 {}：期望 {:?}", self.pos, byte as char))
        }
    }

    fn value(&mut self) -> Result<Json, String> {
        match self.peek() {
            Some(b'{') => self.object(),
            Some(b'[') => self.array(),
            Some(b'"') => Ok(Json::Str(self.string()?)),
            Some(b't') => self.keyword("true", Json::Bool(true)),
            Some(b'f') => self.keyword("false", Json::Bool(false)),
            Some(b'n') => self.keyword("null", Json::Null),
            Some(c) if c == b'-' || c.is_ascii_digit() => self.number(),
            other => Err(format!("位置 {}：非法 JSON 值 {other:?}", self.pos)),
        }
    }

    fn keyword(&mut self, word: &str, value: Json) -> Result<Json, String> {
        if self.src[self.pos..].starts_with(word.as_bytes()) {
            self.pos += word.len();
            Ok(value)
        } else {
            Err(format!("位置 {}：期望 {word}", self.pos))
        }
    }

    fn object(&mut self) -> Result<Json, String> {
        self.expect(b'{')?;
        let mut fields = Vec::new();
        self.ws();
        if self.peek() == Some(b'}') {
            self.pos += 1;
            return Ok(Json::Obj(fields));
        }
        loop {
            self.ws();
            let key = self.string()?;
            self.ws();
            self.expect(b':')?;
            self.ws();
            let value = self.value()?;
            fields.push((key, value));
            self.ws();
            match self.peek() {
                Some(b',') => self.pos += 1,
                Some(b'}') => {
                    self.pos += 1;
                    return Ok(Json::Obj(fields));
                }
                _ => return Err(format!("位置 {}：对象里期望 , 或 }}", self.pos)),
            }
        }
    }

    fn array(&mut self) -> Result<Json, String> {
        self.expect(b'[')?;
        let mut items = Vec::new();
        self.ws();
        if self.peek() == Some(b']') {
            self.pos += 1;
            return Ok(Json::Arr(items));
        }
        loop {
            self.ws();
            items.push(self.value()?);
            self.ws();
            match self.peek() {
                Some(b',') => self.pos += 1,
                Some(b']') => {
                    self.pos += 1;
                    return Ok(Json::Arr(items));
                }
                _ => return Err(format!("位置 {}：数组里期望 , 或 ]", self.pos)),
            }
        }
    }

    fn string(&mut self) -> Result<String, String> {
        self.expect(b'"')?;
        let mut out = String::new();
        loop {
            match self.peek() {
                None => return Err("字符串未闭合".into()),
                Some(b'"') => {
                    self.pos += 1;
                    return Ok(out);
                }
                Some(b'\\') => {
                    self.pos += 1;
                    let esc = self.peek().ok_or("转义未闭合")?;
                    self.pos += 1;
                    match esc {
                        b'"' => out.push('"'),
                        b'\\' => out.push('\\'),
                        b'/' => out.push('/'),
                        b'b' => out.push('\u{8}'),
                        b'f' => out.push('\u{c}'),
                        b'n' => out.push('\n'),
                        b'r' => out.push('\r'),
                        b't' => out.push('\t'),
                        b'u' => {
                            let hex = self
                                .src
                                .get(self.pos..self.pos + 4)
                                .ok_or("\\u 后面不足 4 位")?;
                            let text = std::str::from_utf8(hex).map_err(|_| "\\u 不是 UTF-8")?;
                            let code =
                                u32::from_str_radix(text, 16).map_err(|_| "\\u 不是十六进制")?;
                            self.pos += 4;
                            let c = char::from_u32(code).ok_or("\\u 不是合法字符")?;
                            out.push(c);
                        }
                        _ => return Err(format!("位置 {}：非法转义", self.pos - 1)),
                    }
                }
                Some(_) => {
                    // 原样拷贝一个 UTF-8 字符（清单里中文按 UTF-8 存）。
                    let rest = std::str::from_utf8(&self.src[self.pos..])
                        .map_err(|_| "字符串不是 UTF-8")?;
                    let c = rest.chars().next().ok_or("字符串未闭合")?;
                    out.push(c);
                    self.pos += c.len_utf8();
                }
            }
        }
    }

    fn number(&mut self) -> Result<Json, String> {
        let start = self.pos;
        while matches!(
            self.peek(),
            Some(b'0'..=b'9' | b'-' | b'+' | b'.' | b'e' | b'E')
        ) {
            self.pos += 1;
        }
        let text = std::str::from_utf8(&self.src[start..self.pos]).map_err(|_| "数字不是 ASCII")?;
        text.parse::<f64>()
            .map_err(|_| format!("位置 {start}：非法数字 {text:?}"))?;
        Ok(Json::Num(text.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compact_output_matches_handwritten_manifest() {
        let entry = Json::obj()
            .put("cell", 0.01f32)
            .put("path", "lod-1cm.ply")
            .put("sha256", "e1")
            .put("bytes", 10u64)
            .put(
                "bbox_min",
                Json::arr(vec![
                    Json::num(-1.5f64),
                    Json::num(0.0f64),
                    Json::num(2.25f64),
                ]),
            );
        assert_eq!(
            entry.dump(),
            "{\"cell\": 0.01, \"path\": \"lod-1cm.ply\", \"sha256\": \"e1\", \"bytes\": 10, \
             \"bbox_min\": [-1.5, 0, 2.25]}"
        );
    }

    #[test]
    fn parse_roundtrips_nested_manifest() {
        let text = r#"{"kind": "extract-asset", "config": {"frames": [0, 1], "cells": [0.01, 0.02], "texture": false}, "levels": [{"cell": 0.01, "path": "lod-1cm.ply", "sha256": "e1", "verts": 11}, {"cell": 0.02, "path": "lod-2cm.ply", "sha256": "e2", "verts": 21}], "skipped": []}"#;
        let json = Json::parse(text).unwrap();
        assert_eq!(json.dump(), text);
        assert_eq!(json.get_str("kind"), Some("extract-asset"));
        let levels = json.get("levels").unwrap().as_array().unwrap();
        assert_eq!(levels.len(), 2);
        assert_eq!(levels[1].get("cell").unwrap().as_f64(), Some(0.02));
        assert_eq!(levels[1].get("verts").unwrap().as_u64(), Some(21));
        assert!(json.get("skipped").unwrap().as_array().unwrap().is_empty());
    }

    #[test]
    fn strings_escape_and_unescape() {
        let s = "a\"b\\c\nd\t\u{1}中文";
        let json = Json::Str(s.to_string());
        assert_eq!(json.dump(), json_str(s));
        let back = Json::parse(&json.dump()).unwrap();
        assert_eq!(back.as_str(), Some(s));
    }

    #[test]
    fn bad_json_is_rejected() {
        assert!(Json::parse("{\"a\": }").is_err());
        assert!(Json::parse("{\"a\": 1} trailing").is_err());
        assert!(Json::parse("[1, 2").is_err());
        assert!(Json::parse("{\"a\": 1, \"a\": 2}").is_ok());
    }

    #[test]
    fn numbers_keep_their_literal() {
        let json = Json::parse("{\"big\": 18446744073709551615, \"exp\": 1e-5}").unwrap();
        assert_eq!(
            json.dump(),
            "{\"big\": 18446744073709551615, \"exp\": 1e-5}"
        );
    }

    #[test]
    fn asset_level_entry_bytes_match_legacy_format() {
        let entry = Json::obj()
            .put("cell", 0.01f32)
            .put("path", "lod-1cm.ply")
            .put("sha256", "e1")
            .put("bytes", 10u64)
            .put("verts", 11usize)
            .put("tris", 12usize)
            .put("boundary_edges", 0usize)
            .put("nonmanifold_edges", 0usize)
            .put("unseen_vertices", 3usize);
        assert_eq!(
            entry.dump(),
            r#"{"cell": 0.01, "path": "lod-1cm.ply", "sha256": "e1", "bytes": 10, "verts": 11, "tris": 12, "boundary_edges": 0, "nonmanifold_edges": 0, "unseen_vertices": 3}"#
        );
    }

    #[test]
    fn objects_entry_bytes_match_legacy_format() {
        let entry = Json::obj()
            .put("id", 3u32)
            .put("path", "obj-3.ply")
            .put("sha256", "ab")
            .put("bytes", 120u64)
            .put("count", 42usize)
            .put(
                "bbox_min",
                Json::arr(vec![
                    Json::num(-1.5f64),
                    Json::num(0.0f64),
                    Json::num(2.25f64),
                ]),
            )
            .put(
                "bbox_max",
                Json::arr(vec![
                    Json::num(1.0f64),
                    Json::num(2.0f64),
                    Json::num(3.0f64),
                ]),
            )
            .put(
                "centroid",
                Json::arr(vec![
                    Json::num(0.5f64),
                    Json::num(1.0f64),
                    Json::num(1.5f64),
                ]),
            )
            .put("kind", "chair")
            .put(
                "gt_center",
                Json::arr(vec![
                    Json::num(1.0f64),
                    Json::num(2.0f64),
                    Json::num(3.0f64),
                ]),
            )
            .put("centroid_error", 0.01f64)
            .put("gt_diag", 2.5f64)
            .put("diag_ratio", 0.99f64);
        assert_eq!(
            entry.dump(),
            r#"{"id": 3, "path": "obj-3.ply", "sha256": "ab", "bytes": 120, "count": 42, "bbox_min": [-1.5, 0, 2.25], "bbox_max": [1, 2, 3], "centroid": [0.5, 1, 1.5], "kind": "chair", "gt_center": [1, 2, 3], "centroid_error": 0.01, "gt_diag": 2.5, "diag_ratio": 0.99}"#
        );
    }
}
