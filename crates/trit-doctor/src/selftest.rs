//! `.agent/last-selftest.json` 面：只读落盘记录，不跑 selftest。

use std::fs;
use std::path::Path;

use serde_json::Value;

use crate::{Face, Status};

fn as_i64(value: &Value) -> Option<i64> {
    match value {
        Value::Number(number) => number
            .as_i64()
            .or_else(|| number.as_u64().map(|value| value as i64))
            .or_else(|| number.as_f64().map(|value| value as i64)),
        Value::String(text) => text.parse().ok(),
        Value::Bool(flag) => Some(i64::from(*flag)),
        _ => None,
    }
}

fn display(value: Option<&Value>) -> String {
    match value {
        Some(Value::String(text)) => text.clone(),
        Some(Value::Number(number)) => number.to_string(),
        Some(Value::Bool(flag)) => if *flag { "True" } else { "False" }.to_string(),
        Some(Value::Null) => "None".to_string(),
        None => "?".to_string(),
        Some(other) => other.to_string(),
    }
}

fn short(text: &str) -> String {
    text.chars().take(60).collect()
}

#[must_use]
pub fn face(root: &Path) -> Face {
    let path = root.join(".agent").join("last-selftest.json");
    let rel = ".agent/last-selftest.json";
    let rerun = "rerun: pwsh tools/selftest.ps1";
    if !path.is_file() {
        return Face::new(
            "selftest",
            Status::Unavailable,
            rel,
            format!("no record on disk; {rerun}"),
        );
    }
    let text = match fs::read(&path) {
        Ok(bytes) => String::from_utf8_lossy(&bytes).into_owned(),
        Err(error) => {
            let name = match error.kind() {
                std::io::ErrorKind::NotFound => "FileNotFoundError",
                std::io::ErrorKind::PermissionDenied => "PermissionError",
                _ => "OSError",
            };
            return Face::new(
                "selftest",
                Status::Unavailable,
                rel,
                format!(
                    "unreadable ({name}: {}); {rerun}",
                    short(&error.to_string())
                ),
            );
        }
    };
    let value: Value = match serde_json::from_str(&text) {
        Ok(value) => value,
        Err(error) => {
            return Face::new(
                "selftest",
                Status::Unavailable,
                rel,
                format!(
                    "unreadable (JSONDecodeError: {}); {rerun}",
                    short(&error.to_string())
                ),
            );
        }
    };
    let Some(passed) = value.get("passed").and_then(as_i64) else {
        return Face::new(
            "selftest",
            Status::Unavailable,
            rel,
            format!("unreadable (KeyError: 'passed'); {rerun}"),
        );
    };
    let Some(failed) = value.get("failed").and_then(as_i64) else {
        return Face::new(
            "selftest",
            Status::Unavailable,
            rel,
            format!("unreadable (KeyError: 'failed'); {rerun}"),
        );
    };
    let at = display(value.get("at"));
    let detail = format!(
        "passed={passed} failed={failed} skipped={} at={at}",
        display(value.get("skipped"))
    );
    Face::new(
        "selftest",
        if failed > 0 { Status::Fail } else { Status::Ok },
        &format!("{rel} at={at}"),
        detail,
    )
}
