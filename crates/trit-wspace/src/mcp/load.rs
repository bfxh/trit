//! mcp 的参数/路径装载 + 工作区根限界（从 src/mcp.rs 拆出，控制行数预算）。

use crate::codec::from_wsp;
use crate::net::Net;
use crate::oracle::Oracle;
use crate::wire;
use serde_json::Value;
use std::path::{Path, PathBuf};

pub(super) fn read_log(raw: &str) -> Result<String, String> {
    if raw.trim().is_empty() {
        return Ok(String::new());
    }
    if raw.trim_start().starts_with('{') || raw.contains('\n') {
        Ok(raw.to_string())
    } else {
        std::fs::read_to_string(confined(raw)?).map_err(|_| "读取失败".to_string())
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
        std::fs::read_to_string(confined(raw)?).map_err(|_| "读取失败".to_string())?
    };
    from_wsp(&text).map_err(err)
}

pub(super) fn read_path(path: &str) -> Result<Net, String> {
    let text = std::fs::read_to_string(confined(path)?).map_err(|_| "读取失败".to_string())?;
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

/// 工作区根 = 进程 cwd（MCP server 通常在工作区根启动）。
fn workspace_root() -> Result<PathBuf, String> {
    std::env::current_dir().map_err(|_| "无法确定工作区根".to_string())
}

/// 把输入路径限制在工作区根内（canonicalize 后比对前缀）；越界 / 不可解析一律拒绝（M19）。
/// 用 `root` 参数便于单测（不依赖进程 cwd）。
fn confined_in(root: &Path, path: &str) -> Result<PathBuf, String> {
    let root = root
        .canonicalize()
        .map_err(|_| "工作区根不可用".to_string())?;
    let candidate = if Path::new(path).is_absolute() {
        PathBuf::from(path)
    } else {
        root.join(path)
    };
    let real = candidate
        .canonicalize()
        .map_err(|_| "路径不可用（不存在或非法）".to_string())?;
    if real.starts_with(&root) {
        Ok(real)
    } else {
        Err("路径越出工作区范围".into())
    }
}

/// 只允许读工作区根内的文件；错误不回显路径/内容（M19）。
fn confined(path: &str) -> Result<PathBuf, String> {
    confined_in(&workspace_root()?, path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn confined_rejects_escapes_and_accepts_inside() {
        // M19：工作区根内的相对路径放行；绝对外部路径、`..` 越界一律拒绝。
        let base = std::env::temp_dir().join(format!("trit-mcp-conf-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let root = base.join("ws");
        std::fs::create_dir_all(root.join("sub")).unwrap();
        std::fs::write(root.join("sub/ok.wsp"), "wsp1").unwrap();
        let outside = base.join("outside.txt");
        std::fs::write(&outside, "secret").unwrap();

        assert!(confined_in(&root, "sub/ok.wsp").is_ok(), "根内相对路径放行");
        assert!(
            confined_in(&root, outside.to_str().unwrap()).is_err(),
            "绝对外部路径拒绝"
        );
        assert!(
            confined_in(&root, "../outside.txt").is_err(),
            "`..` 越界拒绝"
        );
        let _ = std::fs::remove_dir_all(&base);
    }
}
