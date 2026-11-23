//! `.agent/last-simchain.json` 面：读模拟器落盘报告，不跑模拟器。
use crate::{Face, Status};
use serde_json::Value;
use std::fs;
use std::path::Path;

pub const SIMCHAIN_REL: &str = ".agent/last-simchain.json";
pub const SIMCHAIN_RERUN: &str = "rerun: python tools/simchain.py simulate --seed 0 \
--faults kill,corrupt,concurrent,truncate --runs 50";

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

fn python_str(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        Value::Number(number) => number.to_string(),
        Value::Bool(flag) => if *flag { "True" } else { "False" }.to_string(),
        Value::Null => "None".to_string(),
        other => other.to_string(),
    }
}

/// Python `bool(value)` 的近似：空串/空表/空对象/0/False/None 为假。
fn truthy(value: Option<&Value>) -> bool {
    match value {
        Some(Value::String(text)) => !text.is_empty(),
        Some(Value::Array(items)) => !items.is_empty(),
        Some(Value::Object(map)) => !map.is_empty(),
        Some(Value::Bool(flag)) => *flag,
        Some(Value::Number(number)) => number.as_f64().is_some_and(|value| value != 0.0),
        Some(Value::Null) | None => false,
    }
}

fn short(text: &str) -> String {
    text.chars().take(60).collect()
}

fn inconsistent(reason: &str) -> Face {
    Face::new(
        "simchain",
        Status::Unavailable,
        SIMCHAIN_REL,
        format!("inconsistent report: {reason}; {SIMCHAIN_RERUN}"),
    )
}

#[must_use]
pub fn face(root: &Path) -> Face {
    let path = root.join(SIMCHAIN_REL);
    if !path.is_file() {
        return Face::new(
            "simchain",
            Status::Unavailable,
            SIMCHAIN_REL,
            format!("no report on disk; {SIMCHAIN_RERUN}"),
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
                "simchain",
                Status::Unavailable,
                SIMCHAIN_REL,
                format!(
                    "unreadable ({name}: {}); {SIMCHAIN_RERUN}",
                    short(&error.to_string())
                ),
            );
        }
    };
    let value: Value = match serde_json::from_str(&text) {
        Ok(value) => value,
        Err(error) => {
            return Face::new(
                "simchain",
                Status::Unavailable,
                SIMCHAIN_REL,
                format!(
                    "unreadable (JSONDecodeError: {}); {SIMCHAIN_RERUN}",
                    short(&error.to_string())
                ),
            );
        }
    };
    let Some(object) = value.as_object() else {
        return Face::new(
            "simchain",
            Status::Unavailable,
            SIMCHAIN_REL,
            format!("unreadable (ValueError: not a simchain report); {SIMCHAIN_RERUN}"),
        );
    };
    if object.get("tool").and_then(Value::as_str) != Some("simchain") {
        return Face::new(
            "simchain",
            Status::Unavailable,
            SIMCHAIN_REL,
            format!("unreadable (ValueError: not a simchain report); {SIMCHAIN_RERUN}"),
        );
    }
    let seeds = object.get("seeds").and_then(as_i64);
    let faults = object.get("faults").and_then(as_i64);
    let scenarios = object.get("scenarios").and_then(as_i64);
    let ok = object.get("invariants_ok").and_then(as_i64);
    let bad = object.get("violations").and_then(as_i64);
    let (Some(seeds), Some(faults), Some(scenarios), Some(ok), Some(bad)) =
        (seeds, faults, scenarios, ok, bad)
    else {
        return Face::new(
            "simchain",
            Status::Unavailable,
            SIMCHAIN_REL,
            format!("unreadable (KeyError: counts); {SIMCHAIN_RERUN}"),
        );
    };
    if scenarios <= 0 || ok + bad != scenarios {
        return inconsistent(&format!(
            "scenarios={scenarios} invariants_ok={ok} violations={bad}"
        ));
    }
    if let Some(details) = object
        .get("scenario_details")
        .filter(|value| !value.is_null())
    {
        let Some(rows) = details.as_array() else {
            return inconsistent("scenario_details shape");
        };
        if rows.iter().any(|row| !row.is_object()) {
            return inconsistent("scenario_details shape");
        }
        if rows.len() != scenarios as usize {
            return inconsistent(&format!(
                "scenario_details rows={} but scenarios={scenarios}",
                rows.len()
            ));
        }
        let row_bad = rows
            .iter()
            .filter(|row| !truthy(row.get("invariants_ok")) || truthy(row.get("violations")))
            .count();
        if row_bad != bad as usize {
            return inconsistent(&format!(
                "per-scenario rows say violations={row_bad}, top-level says {bad}"
            ));
        }
    }
    let kinds: Vec<String> = object
        .get("violation_kinds")
        .and_then(Value::as_array)
        .map(|items| items.iter().map(python_str).collect())
        .unwrap_or_default();
    let at = object.get("at").map_or("?".to_string(), python_str);
    let duration = object.get("duration_s").map_or("?".to_string(), python_str);
    let mut detail = format!(
        "seeds={seeds} faults={faults} scenarios={scenarios} invariants_ok={ok} violations={bad} duration_s={duration}"
    );
    if !kinds.is_empty() {
        detail.push_str(" kinds=");
        detail.push_str(&kinds.iter().take(8).cloned().collect::<Vec<_>>().join(","));
    }
    detail.push_str(" at=");
    detail.push_str(&at);
    Face::new(
        "simchain",
        if bad > 0 { Status::Fail } else { Status::Ok },
        &format!("{SIMCHAIN_REL} at={at}"),
        detail,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_report_is_unavailable_with_rerun() {
        let root = std::env::temp_dir().join("trit-doctor-sim-missing");
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join(".agent")).unwrap();
        let face = face(&root);
        assert_eq!(face.status, Status::Unavailable);
        assert!(face.detail.contains("simchain.py simulate"));
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn self_contradictory_report_is_not_green() {
        let root = std::env::temp_dir().join("trit-doctor-sim-bad");
        let _ = fs::create_dir_all(root.join(".agent"));
        let report = r#"{"tool":"simchain","seeds":1,"faults":1,"scenarios":2,
            "invariants_ok":2,"violations":0,"scenario_details":[
            {"invariants_ok":true,"violations":""},
            {"invariants_ok":false,"violations":"x"}]}"#;
        fs::write(root.join(SIMCHAIN_REL), report).unwrap();
        let face = face(&root);
        assert_eq!(face.status, Status::Unavailable);
        assert!(face.detail.contains("per-scenario rows say violations=1"));
        let _ = fs::remove_dir_all(&root);
    }
}
