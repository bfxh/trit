//! 双轨并行构建：分配方案（dry-run）与真实并行的集成测试。

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

fn bin() -> PathBuf {
    let mut dir = std::env::current_exe().expect("current exe");
    dir.pop();
    if dir.ends_with("deps") {
        dir.pop();
    }
    dir.join(format!("trit-build{}", std::env::consts::EXE_SUFFIX))
}

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "trit-build-plan-{}-{}-{}",
        name,
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    ));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).expect("创建临时目录");
    dir
}

fn run(args: &[&str]) -> (String, String, i32) {
    let out = Command::new(bin())
        .args(args)
        .output()
        .expect("运行 trit-build");
    (
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
        out.status.code().unwrap_or(-1),
    )
}

fn write_manifest(dir: &Path) -> PathBuf {
    for (name, expr) in [("a", "x & y"), ("b", "x | !y"), ("c", "x ^ y")] {
        fs::write(dir.join(format!("{name}.tri")), format!("{expr}\n")).expect("写 .tri");
    }
    let manifest = dir.join("jobs.json");
    fs::write(
        &manifest,
        r#"{"jobs":[
            {"name":"a","kind":"ternary","source":"a.tri"},
            {"name":"lib","kind":"binary","cmd":["rustc","--version"]},
            {"name":"b","kind":"ternary","source":"b.tri"},
            {"name":"c","kind":"ternary","source":"c.tri"}
        ]}"#,
    )
    .expect("写清单");
    manifest
}

fn names(stdout: &str) -> Vec<String> {
    let value: serde_json::Value = serde_json::from_str(stdout).expect("JSON 输出");
    value["plan"]
        .as_array()
        .expect("plan 数组")
        .iter()
        .map(|item| item["name"].as_str().unwrap().to_string())
        .collect()
}

#[test]
fn dry_run_schedules_ternary_first_then_binary() {
    let dir = scratch("dry");
    let manifest = write_manifest(&dir);
    let out = dir.join("out");
    let (stdout, stderr, code) = run(&[
        "--manifest",
        manifest.to_str().unwrap(),
        "--out",
        out.to_str().unwrap(),
        "--dry-run",
    ]);
    assert_eq!(code, 0, "{stderr}");
    assert_eq!(names(&stdout), ["a", "b", "c", "lib"]);
    let value: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(value["plan"][3]["owner"], "binary-track");
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn dry_run_can_support_binary_track_instead() {
    let dir = scratch("dry-bin");
    let manifest = write_manifest(&dir);
    let out = dir.join("out");
    let (stdout, stderr, code) = run(&[
        "--manifest",
        manifest.to_str().unwrap(),
        "--out",
        out.to_str().unwrap(),
        "--dry-run",
        "--prefer",
        "binary",
    ]);
    assert_eq!(code, 0, "{stderr}");
    assert_eq!(names(&stdout), ["lib", "a", "b", "c"]);
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn balanced_keeps_manifest_order() {
    let dir = scratch("balanced");
    let manifest = write_manifest(&dir);
    let out = dir.join("out");
    let (stdout, stderr, code) = run(&[
        "--manifest",
        manifest.to_str().unwrap(),
        "--out",
        out.to_str().unwrap(),
        "--dry-run",
        "--prefer",
        "balanced",
    ]);
    assert_eq!(code, 0, "{stderr}");
    assert_eq!(names(&stdout), ["a", "lib", "b", "c"]);
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn parallel_run_executes_both_tracks() {
    let dir = scratch("run");
    let manifest = write_manifest(&dir);
    let out = dir.join("out");
    let (stdout, stderr, code) = run(&[
        "--manifest",
        manifest.to_str().unwrap(),
        "--out",
        out.to_str().unwrap(),
        "--jobs",
        "3",
    ]);
    assert_eq!(code, 0, "stdout={stdout}\nstderr={stderr}");
    let value: serde_json::Value = serde_json::from_str(&stdout).expect("JSON 输出");
    assert_eq!(value["mode"], "parallel");
    assert_eq!(value["failed"], 0);
    let jobs = value["jobs"].as_array().unwrap();
    assert_eq!(jobs.len(), 4);
    assert!(
        jobs.iter().all(|job| job["ok"] == serde_json::json!(true)),
        "{value}"
    );
    let tracks: Vec<&str> = jobs.iter().map(|j| j["track"].as_str().unwrap()).collect();
    assert_eq!(tracks, ["ternary", "binary", "ternary", "ternary"]);
    // 三个 .tri 的产物都在。
    for name in ["a", "b", "c"] {
        assert!(out.join(name).join("truth.tritbin").exists(), "缺 {name}");
    }
    let _ = fs::remove_dir_all(&dir);
}
