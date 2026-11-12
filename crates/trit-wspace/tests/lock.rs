//! CLI 写锁：`--out` / `--log` 锁文件、超时、stale 回收。

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::{Duration, SystemTime};

use trit_wspace::{Layer, Net, to_wsp};

fn temp_dir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("trit-lock-{tag}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).expect("temp dir");
    dir
}

fn sample() -> Net {
    Net::new(vec![
        Layer::new(2, 3, &[1, 0, -1, 0, 1, -1], &[1, -1]).expect("layer"),
    ])
    .expect("net")
}

fn fixture(dir: &Path) -> (PathBuf, PathBuf) {
    let source = dir.join("model.wsp");
    let patch = dir.join("tile.json");
    fs::write(&source, to_wsp(&sample())).expect("wsp1");
    fs::write(
        &patch,
        r#"{"version":1,"edits":[{"op":"fill_tile","layer":0,"row_start":0,"rows":1,"col_start":0,"cols":1,"to":"+"}]}"#,
    )
    .expect("patch");
    (source, patch)
}

fn run(args: &[&str], wait_ms: &str) -> Output {
    Command::new(env!("CARGO_BIN_EXE_trit-wspace"))
        .args(args)
        .env("TRIT_LOCK_WAIT_MS", wait_ms)
        .output()
        .expect("run cli")
}

#[test]
fn out_lock_blocks_patch() {
    let dir = temp_dir("out");
    let (source, patch) = fixture(&dir);
    let out = dir.join("edited.wsp");
    let lock = dir.join("edited.wsp.lock");
    fs::write(&lock, "pid=4242\nunix_ms=0\n").expect("lock");
    let output = run(
        &[
            "patch",
            source.to_str().expect("source"),
            "--patch",
            patch.to_str().expect("patch"),
            "--out",
            out.to_str().expect("out"),
        ],
        "100",
    );
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("锁定"), "stderr: {stderr}");
    assert!(stderr.contains("pid=4242"), "stderr: {stderr}");
    assert!(!out.exists());
}

#[test]
fn log_lock_blocks_patch() {
    let dir = temp_dir("log");
    let (source, patch) = fixture(&dir);
    let out = dir.join("edited.wsp");
    let log = dir.join("audit.jsonl");
    let lock = dir.join("audit.jsonl.lock");
    fs::write(&lock, "pid=4243\nunix_ms=0\n").expect("lock");
    let output = run(
        &[
            "patch",
            source.to_str().expect("source"),
            "--patch",
            patch.to_str().expect("patch"),
            "--out",
            out.to_str().expect("out"),
            "--log",
            log.to_str().expect("log"),
        ],
        "100",
    );
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("锁定"));
    assert!(!out.exists());
}

#[test]
fn successful_patch_removes_locks() {
    let dir = temp_dir("ok");
    let (source, patch) = fixture(&dir);
    let out = dir.join("edited.wsp");
    let log = dir.join("audit.jsonl");
    let output = run(
        &[
            "patch",
            source.to_str().expect("source"),
            "--patch",
            patch.to_str().expect("patch"),
            "--out",
            out.to_str().expect("out"),
            "--log",
            log.to_str().expect("log"),
        ],
        "100",
    );
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(out.exists());
    assert!(log.exists());
    assert!(!dir.join("edited.wsp.lock").exists());
    assert!(!dir.join("audit.jsonl.lock").exists());
}

#[test]
fn stale_lock_is_reclaimed() {
    let dir = temp_dir("stale");
    let (source, patch) = fixture(&dir);
    let out = dir.join("edited.wsp");
    let lock = dir.join("edited.wsp.lock");
    fs::write(&lock, "pid=4244\nunix_ms=0\n").expect("lock");
    let file = fs::OpenOptions::new()
        .write(true)
        .open(&lock)
        .expect("open lock");
    let old = SystemTime::now() - Duration::from_secs(1200);
    file.set_times(fs::FileTimes::new().set_modified(old))
        .expect("mtime");
    drop(file);
    let output = run(
        &[
            "patch",
            source.to_str().expect("source"),
            "--patch",
            patch.to_str().expect("patch"),
            "--out",
            out.to_str().expect("out"),
        ],
        "100",
    );
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(out.exists());
    assert!(!lock.exists());
}
