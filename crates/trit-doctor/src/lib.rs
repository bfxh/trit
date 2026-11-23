//! `trit-doctor`: Python `tools/doctor.py` 的 Rust 试点。
//!
//! 七个面、同一份 JSON schema；状态内部走 Belnap 四值格（见 [`status`]）。
//! 这里不做诊断，只聚合有出处的证据；FAIL 不改变退出码。

#![forbid(unsafe_code)]

use std::fs;
use std::path::Path;

pub mod cache;
pub mod face;
pub mod gates;
pub mod git;
pub mod ledger;
pub mod registry;
pub mod selftest;
pub mod simchain;
pub mod status;
pub mod time;

pub use face::{Face, Report};
pub use status::Status;

fn read_optional(path: &Path) -> Option<String> {
    fs::read(path)
        .ok()
        .map(|bytes| String::from_utf8_lossy(&bytes).into_owned())
}

/// 采集七个面。`refresh` 与 Python doctor 同义：绕过 gates 缓存。
#[must_use]
pub fn run(root: &Path, refresh: bool) -> Report {
    let ledger_text = read_optional(&root.join("docs").join("ERRORS.md"));
    let ledger = ledger_text.as_deref().map(ledger::parse_ledger);
    let selftest_text = read_optional(&root.join("tools").join("selftest.ps1"));
    let registry = registry::load(root);
    let registry_ids = registry.as_ref().map(|doc| &doc.ids);
    let fingerprint = gates::tree_fingerprint(root);

    let faces = vec![
        selftest::face(root),
        ledger::face_ledger(ledger.as_ref(), selftest_text.as_deref(), registry_ids),
        registry::face_registry(root, registry.as_ref()),
        ledger::face_backlog(ledger.as_ref()),
        git::face_git(root, &fingerprint, refresh),
        gates::face_gates(root, &fingerprint, refresh),
        simchain::face(root),
    ];

    Report {
        tool: "doctor",
        at: time::utc_now(),
        root: ".".to_string(),
        faces,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn seven_faces_in_the_python_order() {
        let root = std::env::temp_dir().join("trit-doctor-run-test");
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join(".agent")).unwrap();
        fs::create_dir_all(root.join("docs")).unwrap();
        fs::write(
            root.join("docs/ERRORS.md"),
            "| L01 | x | y | z | AI30 |\n## D. 欠账\n| L90 | x |\n",
        )
        .unwrap();
        fs::create_dir_all(root.join("tools")).unwrap();
        fs::write(root.join("tools/selftest.ps1"), "AI30 L01").unwrap();
        fs::write(
            root.join(".agent/last-selftest.json"),
            r#"{"passed":3,"failed":0,"skipped":0,"at":"2026-11-22T00:00:00Z"}"#,
        )
        .unwrap();
        let report = run(&root, true);
        let names: Vec<&str> = report.faces.iter().map(|face| face.face.as_str()).collect();
        assert_eq!(
            names,
            vec![
                "selftest", "ledger", "registry", "backlog", "git", "gates", "simchain"
            ]
        );
        assert_eq!(report.faces[0].status, Status::Ok);
        assert_eq!(report.faces[1].status, Status::Ok);
        assert_eq!(report.faces[2].status, Status::Unavailable); // 无 registry
        assert_eq!(report.faces[3].status, Status::Warn); // D 节有欠账
        assert_eq!(report.faces[6].status, Status::Unavailable); // 无 simchain
        let _ = fs::remove_dir_all(&root);
    }
}
