//! 按行/按块 λ 的通配/区间模式（从 `tests/task_scale.rs` 拆出）。

#![allow(clippy::single_range_in_vec_init)]

use std::fs;
use std::path::PathBuf;
use std::process::{Command, Output};

use serde_json::{Value, json};
use trit_wspace::mcp::Mcp;
use trit_wspace::{
    Edit, Layer, Net, Patch, TaskVector, Trit, UnitPart, UnitPattern, scale_task_blocks_patterns,
    scale_task_rows_patterns, to_wsp,
};

fn xor_net() -> Net {
    Net::new(vec![
        Layer::new(2, 2, &[-1, -1, -1, -1], &[-1, 1]).expect("l0"),
        Layer::new(1, 2, &[-1, 1], &[-1]).expect("l1"),
    ])
    .expect("xor net")
}

fn tuned(base: &Net, edits: Vec<Edit>) -> Net {
    let mut net = base.clone();
    Patch::new(edits).apply(&mut net).expect("apply tune");
    net
}

fn set(layer: usize, row: usize, col: usize, to: Trit) -> Edit {
    Edit::SetTrit {
        layer,
        row,
        col,
        to,
    }
}

fn temp_dir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "trit-wspace-task-scale-pattern-{tag}-{}",
        std::process::id()
    ));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).expect("temp dir");
    dir
}

fn path_string(path: &std::path::Path) -> String {
    path.to_string_lossy().into_owned()
}

fn run(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_trit-wspace"))
        .args(args)
        .output()
        .expect("run cli")
}

fn stdout_json(output: &Output) -> Value {
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).expect("json stdout")
}

fn call(server: &mut Mcp, name: &str, args: Value) -> Value {
    server
        .dispatch(&json!({
            "jsonrpc": "2.0", "id": 71, "method": "tools/call",
            "params": {"name": name, "arguments": args},
        }))
        .expect("response")["result"]
        .clone()
}

fn payload(result: &Value) -> Value {
    let raw = result["content"][0]["text"].as_str().expect("text content");
    serde_json::from_str(raw).expect("json payload")
}

fn row_pattern(parts: Vec<UnitPart>, lambda: f32) -> UnitPattern {
    UnitPattern { parts, lambda }
}

#[test]
fn row_patterns_wildcard() {
    let base = xor_net();
    let tuned_net = tuned(
        &base,
        vec![
            set(0, 0, 0, Trit::Pos),
            set(0, 1, 0, Trit::Pos),
            set(1, 0, 0, Trit::Pos),
        ],
    );
    let vector = TaskVector::between(&base, &tuned_net).expect("vector");
    let patterns = vec![row_pattern(
        vec![UnitPart::Ranges(vec![0..=0]), UnitPart::Any],
        0.0,
    )];
    let net = scale_task_rows_patterns(&vector, &patterns)
        .expect("patterns")
        .apply(&base)
        .expect("apply");
    assert_eq!(net.layer(0).expect("l0").trit(0, 0), Some(Trit::Neg));
    assert_eq!(net.layer(0).expect("l0").trit(1, 0), Some(Trit::Neg));
    assert_eq!(net.layer(1).expect("l1").trit(0, 0), Some(Trit::Pos));
}

#[test]
fn row_patterns_exact_beats_wildcard() {
    let base = xor_net();
    let tuned_net = tuned(
        &base,
        vec![
            set(0, 0, 0, Trit::Pos),
            set(0, 1, 0, Trit::Pos),
            set(1, 0, 0, Trit::Pos),
        ],
    );
    let vector = TaskVector::between(&base, &tuned_net).expect("vector");
    let patterns = vec![
        row_pattern(vec![UnitPart::Any, UnitPart::Any], 0.0),
        row_pattern(
            vec![UnitPart::Ranges(vec![0..=0]), UnitPart::Ranges(vec![0..=0])],
            1.0,
        ),
    ];
    let net = scale_task_rows_patterns(&vector, &patterns)
        .expect("patterns")
        .apply(&base)
        .expect("apply");
    assert_eq!(net.layer(0).expect("l0").trit(0, 0), Some(Trit::Pos));
    assert_eq!(net.layer(0).expect("l0").trit(1, 0), Some(Trit::Neg));
    assert_eq!(net.layer(1).expect("l1").trit(0, 0), Some(Trit::Neg));
}

#[test]
fn block_patterns_range() {
    let base = xor_net();
    let tuned_net = tuned(
        &base,
        vec![
            set(0, 0, 0, Trit::Pos),
            set(0, 0, 1, Trit::Pos),
            set(0, 1, 0, Trit::Pos),
            set(1, 0, 0, Trit::Pos),
        ],
    );
    let vector = TaskVector::between(&base, &tuned_net).expect("vector");
    let patterns = vec![row_pattern(
        vec![
            UnitPart::Ranges(vec![0..=0]),
            UnitPart::Ranges(vec![0..=0]),
            UnitPart::Ranges(vec![0..=1]),
        ],
        0.0,
    )];
    let net = scale_task_blocks_patterns(&vector, &patterns, 2)
        .expect("patterns")
        .apply(&base)
        .expect("apply");
    assert_eq!(net.layer(0).expect("l0").trit(0, 0), Some(Trit::Neg));
    assert_eq!(net.layer(0).expect("l0").trit(0, 1), Some(Trit::Neg));
    assert_eq!(net.layer(0).expect("l0").trit(1, 0), Some(Trit::Pos));
    assert_eq!(net.layer(1).expect("l1").trit(0, 0), Some(Trit::Pos));
}

#[test]
fn cli_row_patterns_round_trip() {
    let dir = temp_dir("row-patterns");
    let base = xor_net();
    let tuned_net = tuned(
        &base,
        vec![
            set(0, 0, 0, Trit::Pos),
            set(0, 1, 0, Trit::Pos),
            set(1, 0, 0, Trit::Pos),
        ],
    );
    let base_path = dir.join("base.wsp");
    let tuned_path = dir.join("tuned.wsp");
    let map_path = dir.join("map.json");
    fs::write(&base_path, to_wsp(&base)).expect("write base");
    fs::write(&tuned_path, to_wsp(&tuned_net)).expect("write tuned");
    fs::write(&map_path, "{\"0:*\": 0.0}").expect("write map");
    let value = stdout_json(&run(&[
        "task-scale",
        &path_string(&base_path),
        &path_string(&tuned_path),
        "--row-lambdas-patterns",
        &path_string(&map_path),
    ]));
    let artifact = value["artifact"].as_str().expect("artifact");
    let net = trit_wspace::from_wsp(artifact).expect("parse");
    assert_eq!(net.layer(0).expect("l0").trit(0, 0), Some(Trit::Neg));
    assert_eq!(net.layer(0).expect("l0").trit(1, 0), Some(Trit::Neg));
    assert_eq!(net.layer(1).expect("l1").trit(0, 0), Some(Trit::Pos));
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn mcp_row_patterns_round_trip() {
    let base = xor_net();
    let tuned_net = tuned(
        &base,
        vec![
            set(0, 0, 0, Trit::Pos),
            set(0, 1, 0, Trit::Pos),
            set(1, 0, 0, Trit::Pos),
        ],
    );
    let mut server = Mcp::new();
    let result = call(
        &mut server,
        "wsp_task_scale",
        json!({
            "base": to_wsp(&base),
            "tuned": to_wsp(&tuned_net),
            "row_lambdas_patterns": {"0:*": 0.0},
        }),
    );
    assert_eq!(result["isError"], false);
    let value = payload(&result);
    let artifact = value["artifact"].as_str().expect("artifact");
    let net = trit_wspace::from_wsp(artifact).expect("parse");
    assert_eq!(net.layer(0).expect("l0").trit(0, 0), Some(Trit::Neg));
    assert_eq!(net.layer(1).expect("l1").trit(0, 0), Some(Trit::Pos));
}

#[test]
fn row_patterns_reject_bad_range() {
    let dir = temp_dir("row-patterns-bad");
    let base = xor_net();
    let tuned_net = tuned(&base, vec![set(0, 0, 0, Trit::Pos)]);
    let base_path = dir.join("base.wsp");
    let tuned_path = dir.join("tuned.wsp");
    let map_path = dir.join("map.json");
    fs::write(&base_path, to_wsp(&base)).expect("write base");
    fs::write(&tuned_path, to_wsp(&tuned_net)).expect("write tuned");
    fs::write(&map_path, "{\"0:2-1\": 0.0}").expect("write map");
    let output = run(&[
        "task-scale",
        &path_string(&base_path),
        &path_string(&tuned_path),
        "--row-lambdas-patterns",
        &path_string(&map_path),
    ]);
    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("起点大于终点"),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn row_patterns_set() {
    let base = xor_net();
    let tuned_net = tuned(
        &base,
        vec![
            set(0, 0, 0, Trit::Pos),
            set(0, 1, 0, Trit::Pos),
            set(1, 0, 0, Trit::Pos),
        ],
    );
    let vector = TaskVector::between(&base, &tuned_net).expect("vector");
    let patterns = vec![row_pattern(
        vec![
            UnitPart::Ranges(vec![0..=0]),
            UnitPart::Ranges(vec![0..=0, 1..=1]),
        ],
        0.0,
    )];
    let net = scale_task_rows_patterns(&vector, &patterns)
        .expect("patterns")
        .apply(&base)
        .expect("apply");
    assert_eq!(net.layer(0).expect("l0").trit(0, 0), Some(Trit::Neg));
    assert_eq!(net.layer(0).expect("l0").trit(1, 0), Some(Trit::Neg));
    assert_eq!(net.layer(1).expect("l1").trit(0, 0), Some(Trit::Pos));
}

#[test]
fn cli_row_patterns_set_round_trip() {
    let dir = temp_dir("row-patterns-set");
    let base = xor_net();
    let tuned_net = tuned(
        &base,
        vec![
            set(0, 0, 0, Trit::Pos),
            set(0, 1, 0, Trit::Pos),
            set(1, 0, 0, Trit::Pos),
        ],
    );
    let base_path = dir.join("base.wsp");
    let tuned_path = dir.join("tuned.wsp");
    let map_path = dir.join("map.json");
    fs::write(&base_path, to_wsp(&base)).expect("write base");
    fs::write(&tuned_path, to_wsp(&tuned_net)).expect("write tuned");
    fs::write(&map_path, "{\"0:0|1\": 0.0}").expect("write map");
    let value = stdout_json(&run(&[
        "task-scale",
        &path_string(&base_path),
        &path_string(&tuned_path),
        "--row-lambdas-patterns",
        &path_string(&map_path),
    ]));
    let artifact = value["artifact"].as_str().expect("artifact");
    let net = trit_wspace::from_wsp(artifact).expect("parse");
    assert_eq!(net.layer(0).expect("l0").trit(0, 0), Some(Trit::Neg));
    assert_eq!(net.layer(0).expect("l0").trit(1, 0), Some(Trit::Neg));
    assert_eq!(net.layer(1).expect("l1").trit(0, 0), Some(Trit::Pos));
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn row_patterns_reject_mixed_star() {
    let dir = temp_dir("row-patterns-mixed");
    let base = xor_net();
    let tuned_net = tuned(&base, vec![set(0, 0, 0, Trit::Pos)]);
    let base_path = dir.join("base.wsp");
    let tuned_path = dir.join("tuned.wsp");
    let map_path = dir.join("map.json");
    fs::write(&base_path, to_wsp(&base)).expect("write base");
    fs::write(&tuned_path, to_wsp(&tuned_net)).expect("write tuned");
    fs::write(&map_path, "{\"0:*|1\": 0.0}").expect("write map");
    let output = run(&[
        "task-scale",
        &path_string(&base_path),
        &path_string(&tuned_path),
        "--row-lambdas-patterns",
        &path_string(&map_path),
    ]);
    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("不能把 * 与数字混用"),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let _ = fs::remove_dir_all(&dir);
}
