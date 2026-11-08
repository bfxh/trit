//! 三值任务算术：任务向量的缩放/取反。

use std::fs;
use std::path::PathBuf;
use std::process::{Command, Output};

use serde_json::{Value, json};
use trit_wspace::mcp::Mcp;
use trit_wspace::{
    Edit, Layer, Net, Oracle, Patch, TaskVector, Trit, scale_task, scale_task_layers, to_wsp,
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

fn with_scale(base: &Net, layer: usize, row: usize, scale: f32) -> Net {
    let mut net = base.clone();
    Patch::new(vec![Edit::SetScale {
        layer,
        row,
        to: scale,
    }])
    .apply(&mut net)
    .expect("scale");
    net
}

fn scale_fixture() -> (Net, Net) {
    let base = tuned(&xor_net(), vec![set(0, 0, 0, Trit::Zero)]);
    let tuned = with_scale(&tuned(&base, vec![set(0, 0, 0, Trit::Pos)]), 0, 0, 2.0);
    (base, tuned)
}

fn temp_dir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "trit-wspace-task-scale-{tag}-{}",
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
            "jsonrpc": "2.0", "id": 31, "method": "tools/call",
            "params": {"name": name, "arguments": args},
        }))
        .expect("response")["result"]
        .clone()
}

fn payload(result: &Value) -> Value {
    let raw = result["content"][0]["text"].as_str().expect("text content");
    serde_json::from_str(raw).expect("json payload")
}

#[test]
fn scale_positive_is_identity() {
    let (base, tuned_net) = scale_fixture();
    let vector = TaskVector::between(&base, &tuned_net).expect("vector");
    for lambda in [1.0, 2.0, 0.5] {
        let out = scale_task(&vector, lambda)
            .expect("scale")
            .apply(&base)
            .expect("apply");
        assert_eq!(out, tuned_net, "正系数 lambda={lambda} 应原样");
    }
}

#[test]
fn scale_zero_drops() {
    let (base, tuned_net) = scale_fixture();
    let vector = TaskVector::between(&base, &tuned_net).expect("vector");
    let scaled = scale_task(&vector, 0.0).expect("scale");
    assert!(scaled.edits().is_empty());
    assert!(scaled.scales().is_empty());
    assert_eq!(scaled.apply(&base).expect("apply"), base);
}

#[test]
fn scale_negative_negates_and_drops_scale() {
    let (base, tuned_net) = scale_fixture();
    let vector = TaskVector::between(&base, &tuned_net).expect("vector");
    let out = scale_task(&vector, -1.0)
        .expect("scale")
        .apply(&base)
        .expect("apply");
    assert_eq!(
        out.layer(0).expect("l0").trit(0, 0),
        Some(Trit::Neg),
        "λ<0 应把 Pos 取反成 Neg"
    );
    assert_eq!(
        out.layer(0).expect("l0").scale(0),
        Some(1.0),
        "λ<0 丢弃 scale 差分"
    );
}

#[test]
fn scale_rejects_non_finite() {
    let (base, tuned_net) = scale_fixture();
    let vector = TaskVector::between(&base, &tuned_net).expect("vector");
    assert!(scale_task(&vector, f32::NAN).is_err());
    assert!(scale_task(&vector, f32::INFINITY).is_err());
    assert!(scale_task(&vector, f32::NEG_INFINITY).is_err());
}

#[test]
fn cli_task_scale_round_trip() {
    let dir = temp_dir("cli");
    let (base, tuned_net) = scale_fixture();
    let base_path = dir.join("base.wsp");
    let tuned_path = dir.join("tuned.wsp");
    fs::write(&base_path, to_wsp(&base)).expect("write base");
    fs::write(&tuned_path, to_wsp(&tuned_net)).expect("write tuned");
    let zero = stdout_json(&run(&[
        "task-scale",
        &path_string(&base_path),
        &path_string(&tuned_path),
        "--lambda",
        "0",
    ]));
    let artifact = zero["artifact"].as_str().expect("artifact");
    assert_eq!(
        trit_wspace::from_wsp(artifact).expect("parse").state_hash(),
        base.state_hash()
    );
    let neg = stdout_json(&run(&[
        "task-scale",
        &path_string(&base_path),
        &path_string(&tuned_path),
        "--lambda",
        "-1",
    ]));
    let artifact = neg["artifact"].as_str().expect("artifact");
    let negated = trit_wspace::from_wsp(artifact).expect("parse");
    assert_eq!(negated.layer(0).expect("l0").trit(0, 0), Some(Trit::Neg));
    assert_eq!(negated.layer(0).expect("l0").scale(0), Some(1.0));
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn mcp_task_scale_round_trip() {
    let (base, tuned_net) = scale_fixture();
    let mut server = Mcp::new();
    let result = call(
        &mut server,
        "wsp_task_scale",
        json!({"base": to_wsp(&base), "tuned": to_wsp(&tuned_net), "lambda": -1.0}),
    );
    assert_eq!(result["isError"], false);
    let value = payload(&result);
    let artifact = value["artifact"].as_str().expect("artifact");
    let negated = trit_wspace::from_wsp(artifact).expect("parse");
    assert_eq!(negated.layer(0).expect("l0").trit(0, 0), Some(Trit::Neg));
    let bad = call(
        &mut server,
        "wsp_task_scale",
        json!({"base": to_wsp(&base), "tuned": to_wsp(&tuned_net), "lambda": "x"}),
    );
    assert_eq!(bad["isError"], true);
}

#[test]
fn cli_task_scale_lambdas_scan() {
    let dir = temp_dir("scan");
    let (base, tuned_net) = scale_fixture();
    let oracle =
        Oracle::new(vec![vec![Trit::Neg, Trit::Neg]], vec![vec![Trit::Pos]]).expect("oracle");
    let base_path = dir.join("base.wsp");
    let tuned_path = dir.join("tuned.wsp");
    let oracle_path = dir.join("oracle.json");
    fs::write(&base_path, to_wsp(&base)).expect("write base");
    fs::write(&tuned_path, to_wsp(&tuned_net)).expect("write tuned");
    fs::write(
        &oracle_path,
        serde_json::to_string(&trit_wspace::wire::oracle_to_value(&oracle)).expect("oracle json"),
    )
    .expect("write oracle");
    let value = stdout_json(&run(&[
        "task-scale",
        &path_string(&base_path),
        &path_string(&tuned_path),
        "--lambdas",
        "-1,0,1",
        "--oracle",
        &path_string(&oracle_path),
    ]));
    assert_eq!(value["lambdas"], json!([-1.0, 0.0, 1.0]));
    let results = value["results"].as_array().expect("results");
    assert_eq!(results.len(), 3);
    assert!(results[0]["verify"]["loss"].is_number());
    assert!(results[1]["verify"]["loss"].is_number());
    assert_eq!(
        results[1]["verify"]["loss"],
        json!(oracle.loss(&base).expect("base loss"))
    );
    assert_eq!(
        results[2]["verify"]["loss"],
        json!(oracle.loss(&tuned_net).expect("tuned loss"))
    );
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn cli_task_scale_rejects_lambda_and_lambdas() {
    let dir = temp_dir("scan-conflict");
    let (base, tuned_net) = scale_fixture();
    let base_path = dir.join("base.wsp");
    let tuned_path = dir.join("tuned.wsp");
    fs::write(&base_path, to_wsp(&base)).expect("write base");
    fs::write(&tuned_path, to_wsp(&tuned_net)).expect("write tuned");
    let output = run(&[
        "task-scale",
        &path_string(&base_path),
        &path_string(&tuned_path),
        "--lambda",
        "1",
        "--lambdas",
        "0,1",
    ]);
    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("只能给一个"),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn mcp_task_scale_lambdas_scan() {
    let (base, tuned_net) = scale_fixture();
    let oracle =
        Oracle::new(vec![vec![Trit::Neg, Trit::Neg]], vec![vec![Trit::Pos]]).expect("oracle");
    let mut server = Mcp::new();
    let result = call(
        &mut server,
        "wsp_task_scale",
        json!({
            "base": to_wsp(&base),
            "tuned": to_wsp(&tuned_net),
            "lambdas": [-1.0, 0.0, 1.0],
            "oracle": trit_wspace::wire::oracle_to_value(&oracle),
        }),
    );
    assert_eq!(result["isError"], false);
    let value = payload(&result);
    assert_eq!(value["lambdas"], json!([-1.0, 0.0, 1.0]));
    let results = value["results"].as_array().expect("results");
    assert_eq!(results.len(), 3);
    assert!(results[0]["verify"]["loss"].is_number());
    assert!(results[2]["verify"]["loss"].is_number());
}

#[test]
fn layer_lambdas_scale_per_layer() {
    let base = xor_net();
    let tuned_net = with_scale(
        &tuned(
            &base,
            vec![set(0, 0, 0, Trit::Pos), set(1, 0, 0, Trit::Pos)],
        ),
        0,
        0,
        2.0,
    );
    let vector = TaskVector::between(&base, &tuned_net).expect("vector");
    let kept = scale_task_layers(&vector, &[1.0, 0.0]).expect("scale");
    assert!(kept.edits().iter().all(|edit| match edit {
        Edit::SetTrit { layer, .. } | Edit::SetBias { layer, .. } => *layer == 0,
        _ => true,
    }));
    assert!(kept.scales().iter().all(|(layer, _, _)| *layer == 0));
    let negated = scale_task_layers(&vector, &[-1.0, 1.0]).expect("scale");
    let net = negated.apply(&base).expect("apply");
    assert_eq!(net.layer(0).expect("l0").trit(0, 0), Some(Trit::Neg));
    assert_eq!(net.layer(0).expect("l0").scale(0), Some(1.0));
    assert_eq!(net.layer(1).expect("l1").trit(0, 0), Some(Trit::Pos));
}

#[test]
fn cli_task_scale_layer_lambdas_round_trip() {
    let dir = temp_dir("layer-lambdas");
    let base = xor_net();
    let tuned_net = with_scale(
        &tuned(
            &base,
            vec![set(0, 0, 0, Trit::Pos), set(1, 0, 0, Trit::Pos)],
        ),
        0,
        0,
        2.0,
    );
    let base_path = dir.join("base.wsp");
    let tuned_path = dir.join("tuned.wsp");
    fs::write(&base_path, to_wsp(&base)).expect("write base");
    fs::write(&tuned_path, to_wsp(&tuned_net)).expect("write tuned");
    let value = stdout_json(&run(&[
        "task-scale",
        &path_string(&base_path),
        &path_string(&tuned_path),
        "--layer-lambdas",
        "1,0",
    ]));
    assert_eq!(value["layer_lambdas"], json!([1.0, 0.0]));
    let artifact = value["artifact"].as_str().expect("artifact");
    let net = trit_wspace::from_wsp(artifact).expect("parse");
    assert_eq!(net.layer(1).expect("l1").trit(0, 0), Some(Trit::Neg));
    assert_eq!(net.layer(0).expect("l0").trit(0, 0), Some(Trit::Pos));
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn layer_lambdas_length_checked_by_cli() {
    let dir = temp_dir("layer-lambdas-short");
    let base = xor_net();
    let tuned_net = tuned(&base, vec![set(0, 0, 0, Trit::Pos)]);
    let base_path = dir.join("base.wsp");
    let tuned_path = dir.join("tuned.wsp");
    fs::write(&base_path, to_wsp(&base)).expect("write base");
    fs::write(&tuned_path, to_wsp(&tuned_net)).expect("write tuned");
    let output = run(&[
        "task-scale",
        &path_string(&base_path),
        &path_string(&tuned_path),
        "--layer-lambdas",
        "1",
    ]);
    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("需要 2 个"),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn mcp_task_scale_layer_lambdas_round_trip() {
    let base = xor_net();
    let tuned_net = with_scale(
        &tuned(
            &base,
            vec![set(0, 0, 0, Trit::Pos), set(1, 0, 0, Trit::Pos)],
        ),
        0,
        0,
        2.0,
    );
    let mut server = Mcp::new();
    let result = call(
        &mut server,
        "wsp_task_scale",
        json!({
            "base": to_wsp(&base),
            "tuned": to_wsp(&tuned_net),
            "layer_lambdas": [1.0, 0.0],
        }),
    );
    assert_eq!(result["isError"], false);
    let value = payload(&result);
    assert_eq!(value["layer_lambdas"], json!([1.0, 0.0]));
    let artifact = value["artifact"].as_str().expect("artifact");
    let net = trit_wspace::from_wsp(artifact).expect("parse");
    assert_eq!(net.layer(1).expect("l1").trit(0, 0), Some(Trit::Neg));
}
