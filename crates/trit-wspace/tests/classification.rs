//! 分类读数：per-class 精度与混淆矩阵。

use std::fs;
use std::path::PathBuf;
use std::process::{Command, Output};

use serde_json::{Value, json};
use trit_wspace::mcp::Mcp;
use trit_wspace::wire::classification_value;
use trit_wspace::{Layer, Net, Oracle, Trit, to_wsp};

fn two_class_net() -> Net {
    Net::new(vec![
        Layer::new(2, 2, &[1, 0, 0, 1], &[0, 0]).expect("l0"),
        Layer::new(2, 2, &[1, 0, 0, 1], &[0, 0]).expect("l1"),
    ])
    .expect("net")
}

fn binary_net() -> Net {
    Net::new(vec![
        Layer::new(2, 2, &[0, 0, 0, 0], &[0, 0]).expect("l0"),
        Layer::new(1, 2, &[0, 0], &[-1]).expect("l1"),
    ])
    .expect("net")
}

fn mixed_oracle() -> Oracle {
    Oracle::new(
        vec![vec![Trit::Pos, Trit::Neg], vec![Trit::Neg, Trit::Pos]],
        vec![vec![Trit::Neg, Trit::Pos], vec![Trit::Neg, Trit::Pos]],
    )
    .expect("oracle")
}

fn temp_dir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "trit-wspace-classification-{tag}-{}",
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
            "jsonrpc": "2.0", "id": 41, "method": "tools/call",
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
fn classification_per_class_and_confusion() {
    let net = two_class_net();
    let oracle = mixed_oracle();
    let value = classification_value(&net, &oracle).expect("classification");
    assert_eq!(value["classes"], json!(2));
    assert_eq!(value["per_class"][1]["count"], json!(2));
    assert_eq!(value["per_class"][1]["correct"], json!(1));
    assert_eq!(value["per_class"][1]["accuracy"], json!(0.5));
    assert_eq!(value["confusion"][1], json!([1, 1]));
    assert_eq!(value["per_class"][0]["count"], json!(0));
}

#[test]
fn classification_one_dim_uses_two_classes() {
    let net = binary_net();
    let oracle =
        Oracle::new(vec![vec![Trit::Pos, Trit::Neg]], vec![vec![Trit::Pos]]).expect("oracle");
    let value = classification_value(&net, &oracle).expect("classification");
    assert_eq!(value["classes"], json!(2));
    assert_eq!(value["per_class"][1]["count"], json!(1));
    assert_eq!(value["per_class"][1]["correct"], json!(0));
    assert_eq!(value["confusion"][1], json!([1, 0]));
}

#[test]
fn cli_verify_reports_classification() {
    let dir = temp_dir("cli");
    let artifact_path = dir.join("net.wsp");
    let oracle_path = dir.join("oracle.json");
    fs::write(&artifact_path, to_wsp(&two_class_net())).expect("write artifact");
    fs::write(
        &oracle_path,
        serde_json::to_string(&trit_wspace::wire::oracle_to_value(&mixed_oracle()))
            .expect("oracle json"),
    )
    .expect("write oracle");
    let output = stdout_json(&run(&[
        "verify",
        &path_string(&artifact_path),
        "--oracle",
        &path_string(&oracle_path),
    ]));
    assert_eq!(output["classification"]["classes"], json!(2));
    assert_eq!(output["classification"]["per_class"][1]["count"], json!(2));
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn mcp_verify_reports_classification() {
    let mut server = Mcp::new();
    let result = call(
        &mut server,
        "wsp_verify",
        json!({
            "artifact": to_wsp(&two_class_net()),
            "oracle": trit_wspace::wire::oracle_to_value(&mixed_oracle()),
        }),
    );
    assert_eq!(result["isError"], false);
    let value = payload(&result);
    assert_eq!(value["classification"]["classes"], json!(2));
    assert_eq!(value["classification"]["per_class"][1]["count"], json!(2));
}
