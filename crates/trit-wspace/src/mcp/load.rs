//! mcp 的参数/路径装载（工作区根与只读装载在 `load/fsread.rs`）。

use crate::codec::from_wsp;
use crate::net::Net;
use crate::oracle::Oracle;
use crate::wire;
use serde_json::Value;

mod fsread;
use fsread::{confined, read_text};
pub(crate) use fsread::{refuse_wide_default_root, set_root};

pub(super) fn read_log(raw: &str) -> Result<String, String> {
    if raw.trim().is_empty() {
        return Ok(String::new());
    }
    if raw.trim_start().starts_with('{') || raw.contains('\n') {
        Ok(raw.to_string())
    } else {
        read_text(&confined(raw)?)
    }
}

pub(super) fn load_optional(args: &Value) -> Result<Option<Net>, String> {
    if args.get("artifact").is_some() || args.get("path").is_some() {
        load(args).map(Some)
    } else {
        Ok(None)
    }
}

pub(super) fn load(args: &Value) -> Result<Net, String> {
    if let Some(raw) = args.get("artifact").and_then(Value::as_str) {
        return parse_or_path(raw);
    }
    if let Some(path) = args.get("path").and_then(Value::as_str) {
        return read_path(path);
    }
    Err("需要 artifact（WSP1 文本）或 path".into())
}

pub(super) fn load_key(args: &Value, key: &str) -> Result<Net, String> {
    let raw = args
        .get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| format!("需要 {key}"))?;
    parse_or_path(raw)
}

pub(super) fn parse_or_path(raw: &str) -> Result<Net, String> {
    let text = if raw.trim_start().starts_with("wsp1") || raw.contains('\n') {
        raw.to_string()
    } else {
        read_text(&confined(raw)?)?
    };
    from_wsp(&text).map_err(err)
}

pub(super) fn read_path(path: &str) -> Result<Net, String> {
    let text = read_text(&confined(path)?)?;
    from_wsp(&text).map_err(err)
}

pub(super) fn required_oracle(args: &Value) -> Result<Oracle, String> {
    let value = args
        .get("oracle")
        .ok_or_else(|| "需要 oracle".to_string())?;
    wire::oracle_from_value(value).map_err(err)
}

pub(super) fn optional_oracle(args: &Value) -> Result<Option<Oracle>, String> {
    args.get("oracle")
        .map(|value| wire::oracle_from_value(value).map_err(err))
        .transpose()
}

pub(super) fn flag(args: &Value, name: &str) -> bool {
    args.get(name).and_then(Value::as_bool).unwrap_or(false)
}

pub(super) fn opt_usize(args: &Value, name: &str, default: usize) -> usize {
    args.get(name)
        .and_then(Value::as_u64)
        .map_or(default, |value| value as usize)
}

pub(super) fn opt_u64(args: &Value, name: &str, default: u64) -> u64 {
    args.get(name).and_then(Value::as_u64).unwrap_or(default)
}

pub(super) fn err(error: crate::error::Error) -> String {
    error.to_string()
}
