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

    let shard_dir = dir.join("shards");
    let sharded = stdout_json(&run(&[
        "shard",
        &path_string(&log),
        "--size",
        "1",
        "--out",
        &path_string(&shard_dir),
    ]));
    assert_eq!(sharded["shards"].as_u64(), Some(1));
    assert_eq!(sharded["entries"].as_u64(), Some(1));
    assert!(shard_dir.join("shard-000.jsonl").is_file());
    let sharded_final = dir.join("sharded-final.wsp");
    let checked = stdout_json(&run(&[
        "audit-shards",
        &path_string(&shard_dir),
        "--artifact",
        &path_string(&artifact),
        "--out",
        &path_string(&sharded_final),
    ]));
    assert_eq!(checked["valid"], true);
    assert_eq!(checked["entries"].as_u64(), Some(1));
    assert_eq!(
        checked["replay"]["final_checksum"].as_str().expect("sum"),
        format!("{broken_sum:016x}")
    );
    let sharded_sum = checksum(
        &from_wsp(&fs::read_to_string(&sharded_final).expect("sharded final"))
            .expect("parse sharded final"),
    );
    assert_eq!(sharded_sum, broken_sum);

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

    let triples = stdout_json(&run(&[
        "propose",
        &path_string(&broken),
        "--oracle",
        &path_string(&oracle),
        "--depth",
        "3",
        "--limit",
        "2",
    ]));
    assert_eq!(triples["depth"].as_u64(), Some(3));
    assert!(triples["candidates"].as_array().expect("triples").len() <= 2);

    let explained = stdout_json(&run(&[
        "explain",
        &path_string(&broken),
        "--oracle",
        &path_string(&oracle),
        "--sample",
        "0",
        "--top",
        "4",
    ]));
    assert_eq!(explained["index"].as_u64(), Some(0));
    assert_eq!(explained["candidates"][0]["fixes"], true);
    assert_eq!(explained["candidates"][0]["after_loss"].as_u64(), Some(0));

    let probed = stdout_json(&run(&[
        "probe",
        &path_string(&artifact),
        "--oracle",
        &path_string(&oracle),
        "--top",
        "0",
    ]));
    assert_eq!(probed["rows"].as_u64(), Some(3));
    assert_eq!(probed["sampled"], false);
    assert_eq!(probed["selection"].as_str(), Some("all"));
    let output_row = probed["top"]
        .as_array()
        .expect("top")
        .iter()
        .find(|impact| impact["layer"].as_u64() == Some(1) && impact["row"].as_u64() == Some(0))
        .expect("output row");
    assert_eq!(output_row["zero"]["changed"].as_u64(), Some(4));

    let adaptive = stdout_json(&run(&[
        "probe",
        &path_string(&artifact),
        "--oracle",
        &path_string(&oracle),
        "--budget",
        "1",
        "--adaptive",
    ]));
    assert_eq!(adaptive["selection"].as_str(), Some("adaptive"));
    assert_eq!(adaptive["probed_rows"].as_u64(), Some(1));
    assert_eq!(adaptive["top"][0]["layer"].as_u64(), Some(1));
    assert_eq!(adaptive["top"][0]["row"].as_u64(), Some(0));

    let localized = stdout_json(&run(&[
        "localize",
        &path_string(&broken),
        "--oracle",
        &path_string(&oracle),
        "--rows",
        "2",
        "--row-budget",
        "2",
        "--max-trits",
        "0",
        "--top-trits",
        "4",
    ]));
    assert_eq!(localized["probe"]["selection"].as_str(), Some("adaptive"));
    let rows = localized["rows"].as_array().expect("rows");
    assert_eq!(rows.len(), 2);
    assert!(rows.iter().all(|row| row["top"].is_array()));

    let candidate = dir.join("candidate.json");
    let fixed2 = dir.join("fixed2.wsp");
    let log2 = dir.join("audit2.jsonl");
    fs::write(
        &candidate,
        r#"{"edit":{"op":"set_trit","layer":0,"row":0,"col":0,"to":"-"}}"#,
    )
    .expect("candidate");
    let applied = stdout_json(&run(&[
        "apply-candidate",
        &path_string(&broken),
        "--candidate",
        &path_string(&candidate),
        "--oracle",
        &path_string(&oracle),
        "--require-after",
        "--out",
        &path_string(&fixed2),
        "--log",
        &path_string(&log2),
    ]));
    assert_eq!(applied["applied"], true);
    assert_eq!(applied["verify"]["exact"], true);
    assert!(fixed2.is_file());
    let audit2 = stdout_json(&run(&[
        "audit",
        &path_string(&log2),
        "--artifact",
        &path_string(&broken),
    ]));
    assert_eq!(audit2["valid"], true);
    assert_eq!(audit2["entries"].as_u64(), Some(1));
    assert_eq!(audit2["replay"]["behavior_checked"].as_u64(), Some(1));

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

#[test]
fn shard_manifest_detects_missing_extra_and_tampering() {
    let dir = std::env::temp_dir().join(format!("trit-wspace-manifest-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).expect("temp dir");
    let artifact = dir.join("xor.wsp");
    let oracle = dir.join("xor.json");
    let patch = dir.join("break.json");
    let fix = dir.join("fix.json");
    let log = dir.join("audit.jsonl");
    let shards = dir.join("shards");
    let checkpoints = dir.join("checkpoints");
    let broken = dir.join("broken.wsp");
    let fixed = dir.join("fixed.wsp");
    fs::write(&artifact, to_wsp(&xor_net())).expect("artifact");
    fs::write(&oracle, r#"{"domain":"boolean","vars":2,"table":"0110"}"#).expect("oracle");
    fs::write(
        &patch,
        r#"{"version":1,"edits":[{"op":"set_trit","layer":0,"row":0,"col":0,"to":"0"}]}"#,
    )
    .expect("patch");
    fs::write(
        &fix,
        r#"{"version":1,"edits":[{"op":"set_trit","layer":0,"row":0,"col":0,"to":"-"}]}"#,
    )
    .expect("fix");
    let patched = run(&[
        "patch",
        &path_string(&artifact),
        "--patch",
        &path_string(&patch),
        "--oracle",
        &path_string(&oracle),
        "--log",
        &path_string(&log),
        "--out",
        &path_string(&broken),
    ]);
    assert!(patched.status.success());
    let patched = run(&[
        "patch",
        &path_string(&broken),
        "--patch",
        &path_string(&fix),
        "--oracle",
        &path_string(&oracle),
        "--log",
        &path_string(&log),
        "--out",
        &path_string(&fixed),
    ]);
    assert!(patched.status.success());

    let split = stdout_json(&run(&[
        "shard",
        &path_string(&log),
        "--size",
        "1",
        "--out",
        &path_string(&shards),
    ]));
    assert_eq!(split["shards"].as_u64(), Some(2));
    assert!(shards.join("manifest.json").is_file());
    let checked = stdout_json(&run(&[
        "audit-shards",
        &path_string(&shards),
        "--artifact",
        &path_string(&artifact),
    ]));
    assert_eq!(checked["manifest_checked"], true);

    let checkpointed = stdout_json(&run(&[
        "audit-shards",
        &path_string(&shards),
        "--artifact",
        &path_string(&artifact),
        "--checkpoint-every",
        "1",
        "--checkpoint-dir",
        &path_string(&checkpoints),
    ]));
    assert_eq!(checkpointed["checkpoints"].as_u64(), Some(2));
    assert!(!checkpointed["resumed"].as_bool().expect("resumed"));
    let resumed_out = dir.join("resumed.wsp");
    let resumed = stdout_json(&run(&[
        "audit-shards",
        &path_string(&shards),
        "--resume",
        &path_string(&checkpoints.join("checkpoint-001.json")),
        "--out",
        &path_string(&resumed_out),
    ]));
    assert_eq!(resumed["resumed"], true);
    assert_eq!(
        resumed["replay"]["final_checksum"],
        checkpointed["replay"]["final_checksum"]
    );
    let resumed_net =
        from_wsp(&fs::read_to_string(&resumed_out).expect("resumed")).expect("parse resumed");
    let fixed_net = from_wsp(&fs::read_to_string(&fixed).expect("fixed")).expect("parse fixed");
    assert_eq!(resumed_net, fixed_net);

    let checkpoint_path = checkpoints.join("checkpoint-001.json");
    let checkpoint_text = fs::read_to_string(&checkpoint_path).expect("checkpoint");
    let mut checkpoint: Value = serde_json::from_str(&checkpoint_text).expect("checkpoint json");
    checkpoint["head"] = serde_json::json!("0000000000000000");
    fs::write(
        &checkpoint_path,
        serde_json::to_string_pretty(&checkpoint).expect("json") + "\n",
    )
    .expect("tamper checkpoint");
    let bad = run(&[
        "audit-shards",
        &path_string(&shards),
        "--resume",
        &path_string(&checkpoint_path),
    ]);
    assert!(!bad.status.success());
    assert!(
        String::from_utf8_lossy(&bad.stderr).contains("检查点"),
        "stderr: {}",
        String::from_utf8_lossy(&bad.stderr)
    );

    let shard_file = shards.join("shard-000.jsonl");
    let extra = shards.join("shard-999.jsonl");
    fs::copy(&shard_file, &extra).expect("extra");
    let bad = run(&["audit-shards", &path_string(&shards)]);
    assert!(!bad.status.success());
    assert!(
        String::from_utf8_lossy(&bad.stderr).contains("清单外"),
        "stderr: {}",
        String::from_utf8_lossy(&bad.stderr)
    );
    fs::remove_file(&extra).expect("remove extra");

    let original = fs::read(&shard_file).expect("shard");
    let mut tampered = original.clone();
    tampered.push(b'\n');
    fs::write(&shard_file, &tampered).expect("tamper");
    let bad = run(&["audit-shards", &path_string(&shards)]);
    assert!(!bad.status.success());
    assert!(
        String::from_utf8_lossy(&bad.stderr).contains("摘要"),
        "stderr: {}",
        String::from_utf8_lossy(&bad.stderr)
    );
    fs::write(&shard_file, &original).expect("restore");

    let moved = shards.join("moved.jsonl");
    fs::rename(&shard_file, &moved).expect("move");
    let bad = run(&["audit-shards", &path_string(&shards)]);
    assert!(!bad.status.success());
    assert!(
        String::from_utf8_lossy(&bad.stderr).contains("不存在"),
        "stderr: {}",
        String::from_utf8_lossy(&bad.stderr)
    );
    fs::rename(&moved, &shard_file).expect("restore");
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn script_runs_atomic_sequence_and_writes_artifact() {
    let dir = std::env::temp_dir().join(format!("trit-wspace-script-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).expect("temp dir");
    let artifact = dir.join("xor.wsp");
    let oracle = dir.join("xor.json");
    let script = dir.join("script.json");
    let bad_script = dir.join("bad.json");
    let out = dir.join("out.wsp");
    fs::write(&artifact, to_wsp(&xor_net())).expect("artifact");
    fs::write(&oracle, r#"{"domain":"boolean","vars":2,"table":"0110"}"#).expect("oracle");
    fs::write(
        &script,
        r#"{"type":"sequence","name":"repair","steps":[
            {"type":"step","name":"break","patch":{"version":1,"edits":[
                {"op":"set_trit","layer":0,"row":0,"col":0,"to":"0"}]}},
            {"type":"step","name":"fix","patch":{"version":1,"edits":[
                {"op":"set_trit","layer":0,"row":0,"col":0,"to":"-"}]}}
        ]}"#,
    )
    .expect("script");
    fs::write(
        &bad_script,
        r#"{"type":"sequence","name":"bad","steps":[
            {"type":"step","name":"break","patch":{"version":1,"edits":[
                {"op":"set_trit","layer":0,"row":0,"col":0,"to":"0"}]}},
            {"type":"step","name":"bad-fix","after":{"outputs":["0","0","0","0"]},
             "patch":{"version":1,"edits":[
                {"op":"set_trit","layer":0,"row":0,"col":0,"to":"-"}]}}
        ]}"#,
    )
    .expect("bad script");

    let ran = stdout_json(&run(&[
        "script",
        &path_string(&artifact),
        "--script",
        &path_string(&script),
        "--oracle",
        &path_string(&oracle),
        "--out",
        &path_string(&out),
    ]));
    assert_eq!(ran["report"]["type"], "sequence");
    assert_eq!(ran["report"]["rolled_back"], false);
    assert_eq!(ran["report"]["steps"].as_array().expect("steps").len(), 2);
    assert_eq!(ran["report"]["steps"][0]["type"], "step");
    let final_net = from_wsp(&fs::read_to_string(&out).expect("out")).expect("parse out");
    assert_eq!(checksum(&final_net), checksum(&xor_net()));

    let rolled = stdout_json(&run(&[
        "script",
        &path_string(&artifact),
        "--script",
        &path_string(&bad_script),
        "--oracle",
        &path_string(&oracle),
        "--out",
        &path_string(&out),
    ]));
    assert_eq!(rolled["report"]["rolled_back"], true);
    let final_net = from_wsp(&fs::read_to_string(&out).expect("out")).expect("parse out");
    assert_eq!(final_net, xor_net(), "原子序列整体回滚");
}
