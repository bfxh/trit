//! 独立复核器的骨架：裁决写盘、PASS/reject 互斥、同目录可执行定位、检查表。
//!
//! 族特有逻辑（重跑导出、逐档比对）留在各复核器；这里只放每个复核器都必须一致的
//! 那几件事——否则「PASS 与 reject 同时存在」「记录写到别的目录」这类漂移会各写一遍。

use std::path::{Path, PathBuf};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Check {
    pub name: &'static str,
    pub ok: bool,
    pub detail: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Report {
    pub checks: Vec<Check>,
}

impl Report {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn check(mut self, name: &'static str, ok: bool, detail: impl Into<String>) -> Self {
        self.checks.push(Check {
            name,
            ok,
            detail: detail.into(),
        });
        self
    }

    pub fn ok(&self) -> bool {
        self.checks.iter().all(|c| c.ok)
    }

    pub fn failed(&self) -> Vec<String> {
        self.checks
            .iter()
            .filter(|c| !c.ok)
            .map(|c| {
                if c.detail.is_empty() {
                    c.name.to_string()
                } else {
                    format!("{}({})", c.name, c.detail)
                }
            })
            .collect()
    }
}

/// 与当前可执行文件同目录的可执行路径（Windows 自动补 `.exe`）。
pub fn sibling_exe(name: &str) -> PathBuf {
    let dir = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.to_path_buf()))
        .unwrap_or_default();
    if cfg!(windows) {
        dir.join(format!("{name}.exe"))
    } else {
        dir.join(name)
    }
}

/// 裁决记录的两个候选名：`<stem>.verify.json` / `<stem>.verify.reject.json`。
pub fn verdict_paths(dir: &Path, stem: &str) -> (PathBuf, PathBuf) {
    (
        dir.join(format!("{stem}.verify.json")),
        dir.join(format!("{stem}.verify.reject.json")),
    )
}

/// 写裁决记录；默认位置 PASS/reject 互斥，`custom`（`--json-out`）是旁路。
pub fn publish(
    dir: &Path,
    stem: &str,
    pass: bool,
    custom: Option<&Path>,
    body: &str,
) -> Result<PathBuf, String> {
    let (pass_path, reject_path) = verdict_paths(dir, stem);
    let path = match custom {
        Some(p) => p.to_path_buf(),
        None => {
            let stale = if pass { &reject_path } else { &pass_path };
            let _ = std::fs::remove_file(stale);
            if pass { pass_path } else { reject_path }
        }
    };
    std::fs::write(&path, body).map_err(|e| format!("写复核记录 {}: {e}", path.display()))?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(tag: &str) -> PathBuf {
        let p = std::env::temp_dir().join(format!("trit-verify-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&p);
        std::fs::create_dir_all(&p).unwrap();
        p
    }

    #[test]
    fn publish_keeps_pass_and_reject_exclusive() {
        let dir = tmp("exclusive");
        let p = publish(&dir, "asset", true, None, "ok").unwrap();
        assert!(p.ends_with("asset.verify.json"));
        assert_eq!(std::fs::read_to_string(&p).unwrap(), "ok");
        let r = publish(&dir, "asset", false, None, "bad").unwrap();
        assert!(r.ends_with("asset.verify.reject.json"));
        assert!(
            !dir.join("asset.verify.json").exists(),
            "reject 必须删掉旧的 PASS"
        );
        assert!(dir.join("asset.verify.reject.json").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn custom_json_out_does_not_touch_default_verdicts() {
        let dir = tmp("custom");
        let pass = dir.join("asset.verify.json");
        std::fs::write(&pass, "old").unwrap();
        let out = dir.join("side.json");
        let p = publish(&dir, "asset", false, Some(&out), "side").unwrap();
        assert_eq!(p, out);
        assert_eq!(std::fs::read_to_string(&pass).unwrap(), "old");
        assert!(!dir.join("asset.verify.reject.json").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn report_all_must_pass() {
        let ok = Report::new()
            .check("model", true, "")
            .check("data", true, "6 文件");
        assert!(ok.ok());
        assert!(ok.failed().is_empty());
        let bad = ok.check("levels", false, "1/3");
        assert!(!bad.ok());
        assert_eq!(bad.failed(), vec!["levels(1/3)".to_string()]);
    }

    #[test]
    fn sibling_exe_names_the_peer() {
        let p = sibling_exe("parts-3d");
        let name = p.file_name().unwrap().to_string_lossy();
        if cfg!(windows) {
            assert_eq!(name, "parts-3d.exe");
        } else {
            assert_eq!(name, "parts-3d");
        }
    }
}
