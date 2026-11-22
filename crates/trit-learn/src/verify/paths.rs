//! 裁决记录的落盘：路径命名、PASS/reject 互斥、`--json-out` 旁路。
//!
//! 两种命名都支持：目录 + 固定 stem（`asset.verify.json`）与基准前缀
//! （`<模型>.verify.json`）。族特有逻辑不在这里，这里只保证「写哪、删哪」一致。

use std::path::{Path, PathBuf};

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

/// 目录 + 固定 stem 的两个候选名：`<stem>.verify.json` / `<stem>.verify.reject.json`。
pub fn verdict_paths(dir: &Path, stem: &str) -> (PathBuf, PathBuf) {
    (
        dir.join(format!("{stem}.verify.json")),
        dir.join(format!("{stem}.verify.reject.json")),
    )
}

fn with_suffix(base: &Path, suffix: &str) -> PathBuf {
    let mut s = base.as_os_str().to_owned();
    s.push(suffix);
    PathBuf::from(s)
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
    publish_paths(&pass_path, &reject_path, pass, custom, body)
}

/// 以 `base` 为前缀写裁决：`<base>.verify.json` / `<base>.verify.reject.json`。
pub fn publish_base(
    base: &Path,
    pass: bool,
    custom: Option<&Path>,
    body: &str,
) -> Result<PathBuf, String> {
    publish_paths(
        &with_suffix(base, ".verify.json"),
        &with_suffix(base, ".verify.reject.json"),
        pass,
        custom,
        body,
    )
}

fn publish_paths(
    pass_path: &Path,
    reject_path: &Path,
    pass: bool,
    custom: Option<&Path>,
    body: &str,
) -> Result<PathBuf, String> {
    let path = match custom {
        Some(p) => p.to_path_buf(),
        None => {
            let stale = if pass { reject_path } else { pass_path };
            let _ = std::fs::remove_file(stale);
            if pass {
                pass_path.to_path_buf()
            } else {
                reject_path.to_path_buf()
            }
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
    fn publish_base_uses_the_given_prefix() {
        let dir = tmp("base");
        let base = dir.join("model.dcn");
        let p = publish_base(&base, true, None, "ok").unwrap();
        assert_eq!(p, dir.join("model.dcn.verify.json"));
        let r = publish_base(&base, false, None, "bad").unwrap();
        assert_eq!(r, dir.join("model.dcn.verify.reject.json"));
        assert!(!dir.join("model.dcn.verify.json").exists());
        assert!(dir.join("model.dcn.verify.reject.json").exists());
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
