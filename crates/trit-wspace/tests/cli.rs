//! CLI 端到端：读 → 验证 → 打坏 → 修补 → 落盘。

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use serde_json::Value;
use trit_wspace::{Layer, Net, checksum, from_wsp, to_wsp};

fn xor_net() -> Net {
    Net::new(vec![
        Layer::new(2, 2, &[-1, -1, -1, -1], &[-1, 1]).expect("l0"),
        Layer::new(1, 2, &[-1, 1], &[-1]).expect("l1"),
    ])
    .expect("xor net")
}

fn temp_dir() -> PathBuf {
    let dir = std::env::temp_dir().join(format!("trit-wspace-cli-{}", std::process::id()));
    fs::create_dir_all(&dir).expect("temp dir");
    dir
}

fn bin() -> &'static str {
    env!("CARGO_BIN_EXE_trit-wspace")
}

fn run(args: &[&str]) -> Output {
    Command::new(bin()).args(args).output().expect("run cli")
}

fn stdout_json(output: &Output) -> Value {
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).expect("json stdout")
}

fn path_string(path: &Path) -> String {
    path.to_str().expect("utf-8 path").to_string()
}

#[test]
fn cli_read_verify_patch_repair_round_trip() {
    let dir = temp_dir();
    let artifact = dir.join("xor.wsp");
    let oracle = dir.join("xor.json");
    let patch = dir.join("break.json");
    let broken = dir.join("broken.wsp");
    let fixed = dir.join("fixed.wsp");
    let log = dir.join("audit.jsonl");
    let replayed = dir.join("replayed.wsp");
    fs::write(&artifact, to_wsp(&xor_net())).expect("artifact");
    fs::write(&oracle, r#"{"domain":"boolean","vars":2,"table":"0110"}"#).expect("oracle");
    fs::write(
        &patch,
        r#"{"version":1,"edits":[{"op":"set_trit","layer":0,"row":0,"col":0,"to":"0"}]}"#,
    )
    .expect("patch");

    let read = stdout_json(&run(&["read", &path_string(&artifact)]));
    assert_eq!(read["summary"]["arch"], serde_json::json!([2, 2, 1]));

    let verify = stdout_json(&run(&[
        "verify",
        &path_string(&artifact),
        "--oracle",
        &path_string(&oracle),
    ]));
    assert_eq!(verify["exact"], true);
    assert_eq!(verify["loss"].as_u64(), Some(0));

    let patched = stdout_json(&run(&[
        "patch",
        &path_string(&artifact),
        "--patch",
        &path_string(&patch),
        "--oracle",
        &path_string(&oracle),
        "--out",
        &path_string(&broken),
        "--log",
        &path_string(&log),
    ]));
    assert_eq!(patched["report"]["applied"], true);
    assert_eq!(patched["report"]["cost"]["touched"].as_u64(), Some(1));
    let broken_verify = stdout_json(&run(&[
        "verify",
        &path_string(&broken),
        "--oracle",
        &path_string(&oracle),
    ]));
    assert_eq!(broken_verify["exact"], false);

    let broken_sum =
        checksum(&from_wsp(&fs::read_to_string(&broken).expect("broken")).expect("parse broken"));
    let audit = stdout_json(&run(&[
        "audit",
        &path_string(&log),
        "--artifact",
        &path_string(&artifact),
    ]));
    assert_eq!(audit["valid"], true);
    assert_eq!(audit["entries"].as_u64(), Some(1));
    assert_eq!(audit["replay"]["behavior_checked"].as_u64(), Some(1));
    assert_eq!(
        audit["replay"]["final_checksum"].as_str().expect("sum"),
        format!("{broken_sum:016x}")
    );

    let replayed_report = stdout_json(&run(&[
        "replay",
        &path_string(&log),
        "--artifact",
        &path_string(&artifact),
        "--out",
        &path_string(&replayed),
    ]));
    assert_eq!(
        replayed_report["final_checksum"].as_str().expect("sum"),
        format!("{broken_sum:016x}")
    );
    assert_eq!(replayed_report["behavior_checked"].as_u64(), Some(1));
    assert!(replayed.is_file());

    let tampered =
        fs::read_to_string(&log)
            .expect("log")
            .replacen("\"to\":\"0\"", "\"to\":\"+\"", 1);
    fs::write(&log, tampered).expect("tamper");
    let bad = run(&["audit", &path_string(&log)]);
    assert!(!bad.status.success());
    assert!(
        String::from_utf8_lossy(&bad.stderr).starts_with("FAIL "),
        "stderr: {}",
        String::from_utf8_lossy(&bad.stderr)
    );

    let proposed = stdout_json(&run(&[
        "propose",
        &path_string(&broken),
        "--oracle",
        &path_string(&oracle),
        "--limit",
        "4",
    ]));
    let candidates = proposed["candidates"].as_array().expect("candidates");
    assert!(!candidates.is_empty() && candidates.len() <= 4);
    assert!(proposed["before_loss"].as_u64().expect("loss") > 0);
    assert_eq!(candidates[0]["after_loss"].as_u64(), Some(0));

    let pairs = stdout_json(&run(&[
        "propose",
        &path_string(&broken),
        "--oracle",
        &path_string(&oracle),
        "--pairs",
        "--limit",
        "2",
    ]));
    assert_eq!(pairs["depth"].as_u64(), Some(2));
    assert!(pairs["candidates"].as_array().expect("pairs").len() <= 2);

    let repaired = stdout_json(&run(&[
        "repair",
        &path_string(&broken),
        "--oracle",
        &path_string(&oracle),
        "--out",
        &path_string(&fixed),
    ]));
    assert_eq!(repaired["found"], true);
    assert_eq!(repaired["verify"]["exact"], true);
    assert!(fixed.is_file());

    let final_verify = stdout_json(&run(&[
        "verify",
        &path_string(&fixed),
        "--oracle",
        &path_string(&oracle),
    ]));
    assert_eq!(final_verify["exact"], true);

    let diff = stdout_json(&run(&[
        "diff",
        &path_string(&artifact),
        &path_string(&fixed),
    ]));
    assert!(diff["patch"]["edits"].is_array());
    assert!(diff["cost"]["touched"].is_u64());

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn cli_usage_errors_use_fail_prefix() {
    let output = run(&[]);
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.starts_with("FAIL "), "stderr: {stderr}");
}

#[test]
fn cli_supports_stdin_for_artifact() {
    use std::io::Write;
    use std::process::Stdio;

    let mut child = Command::new(bin())
        .args(["read", "-"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn");
    child
        .stdin
        .take()
        .expect("stdin")
        .write_all(to_wsp(&xor_net()).as_bytes())
        .expect("write stdin");
    let output = child.wait_with_output().expect("wait");
    let value = stdout_json(&output);
    assert_eq!(value["summary"]["arch"], serde_json::json!([2, 2, 1]));
}
