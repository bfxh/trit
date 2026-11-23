//! 轨迹级 `pipeline --trajectories`：一条命令串 gate → rewrite → mix，并落统一报告。

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use trit_learn::json::Json;

fn temp_dir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("trit-traj-{tag}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).expect("temp dir");
    dir
}
fn run(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_trit-data"))
        .args(args)
        .output()
        .expect("run trit-data")
}
fn report(dir: &Path) -> Json {
    let path = dir.join("trajectory.pipeline.report.json");
    let text = fs::read_to_string(&path).expect("report");
    Json::parse(&text).expect("report json")
}
fn u64_at(json: &Json, path: &[&str]) -> u64 {
    let mut cursor = json;
    for key in path {
        cursor = cursor.get(key).unwrap_or_else(|| panic!("缺 {key}"));
    }
    cursor
        .as_u64()
        .unwrap_or_else(|| panic!("{} 不是整数", path.join(".")))
}
fn bool_at(json: &Json, path: &[&str]) -> bool {
    let mut cursor = json;
    for key in path {
        cursor = cursor.get(key).unwrap_or_else(|| panic!("缺 {key}"));
    }
    cursor
        .as_bool()
        .unwrap_or_else(|| panic!("{} 不是布尔", path.join(".")))
}

fn fixture() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("testdata/trajectories.jsonl")
}

#[test]
fn gate_rewrite_mix_run_as_one_command() {
    let out = temp_dir("all");
    let rewriter = if cfg!(windows) {
        r#"powershell -NoProfile -Command "[Console]::Out.Write(([Console]::In.ReadToEnd() -replace '(?m)^','first'))""#
    } else {
        "sed s/^/first/"
    };
    let result = run(&[
        "pipeline",
        "--trajectories",
        fixture().to_str().expect("fixture"),
        "--out",
        out.to_str().expect("out"),
        "--no-lang-quality",
        "--cmd",
        rewriter,
        "--mix-total",
        "2",
    ]);
    assert!(
        result.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&result.stderr)
    );
    let report = report(&out);
    assert_eq!(u64_at(&report, &["input", "records"]), 4);
    assert_eq!(u64_at(&report, &["gate", "accept"]), 2);
    assert_eq!(u64_at(&report, &["gate", "defer"]), 1);
    assert_eq!(u64_at(&report, &["gate", "reject"]), 1);
    assert!(bool_at(&report, &["rewrite", "enabled"]));
    assert_eq!(u64_at(&report, &["rewrite", "counts", "rewritten"]), 1);
    assert_eq!(
        u64_at(&report, &["rewrite", "counts", "accepted"]),
        1,
        "deferred 样本加 thought 后应回门 accept"
    );
    assert_eq!(
        u64_at(&report, &["rewrite", "metrics", "fidelity", "passed"]),
        1
    );
    assert_eq!(u64_at(&report, &["mix", "available"]), 3);
    assert_eq!(u64_at(&report, &["mix", "selected"]), 2);
    assert_eq!(u64_at(&report, &["mix", "shortfall"]), 0);
    assert_eq!(
        u64_at(&report, &["manifest", "files"]),
        9,
        "三支 + 四个重写桶 + mix/mixed"
    );
    for name in [
        "accepted.jsonl",
        "deferred.jsonl",
        "rejected.jsonl",
        "rewritten.accepted.jsonl",
        "rewritten.deferred.jsonl",
        "rewritten.rejected.jsonl",
        "rewrite.failed.jsonl",
        "mix.json",
        "mixed.jsonl",
        "manifest.json",
    ] {
        assert!(out.join(name).exists(), "缺 {name}");
    }
    assert_eq!(
        fs::read_to_string(out.join("mixed.jsonl"))
            .expect("mixed")
            .lines()
            .count(),
        2
    );
}

#[test]
fn parse_error_goes_to_reject_bucket() {
    let out = temp_dir("parse");
    let input = out.join("bad.jsonl");
    fs::write(&input, "{not-json}\n").expect("input");
    let result = run(&[
        "pipeline",
        "--trajectories",
        input.to_str().expect("input"),
        "--out",
        out.join("out").to_str().expect("out"),
    ]);
    assert!(
        result.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&result.stderr)
    );
    let report = report(&out.join("out"));
    assert_eq!(u64_at(&report, &["input", "parse_errors"]), 1);
    assert_eq!(u64_at(&report, &["gate", "reject"]), 1);
    assert_eq!(u64_at(&report, &["gate", "reasons", "parse_error"]), 1);
    assert!(!bool_at(&report, &["rewrite", "enabled"]));
    assert_eq!(u64_at(&report, &["mix", "selected"]), 0);
}
