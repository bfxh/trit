//! 三值任务算术组合：按 λ 加/减多个任务向量。

use std::fs;
use std::path::PathBuf;
use std::process::{Command, Output};

use serde_json::{Value, json};
use trit_wspace::mcp::Mcp;
use trit_wspace::{Edit, Layer, Net, Patch, TaskVector, Trit, combine_tasks, to_wsp};

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

fn fixture() -> (Net, Net, Net) {
    let base = xor_net();
    let a = tuned(&base, vec![set(0, 0, 0, Trit::Pos)]);
    let b = tuned(&base, vec![set(0, 1, 0, Trit::Pos)]);
    (base, a, b)
}

fn temp_dir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "trit-wspace-task-combine-{tag}-{}",
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
            "jsonrpc": "2.0", "id": 61, "method": "tools/call",
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
fn combine_tasks_adds_and_subtracts() {
    let (base, a, b) = fixture();
    let tasks = [
        TaskVector::between(&base, &a).expect("a"),
        TaskVector::between(&base, &b).expect("b"),
    ];
    let add = combine_tasks(&base, &tasks, &[1.0, 1.0]).expect("add");
    assert_eq!(add.layer(0).expect("l0").trit(0, 0), Some(Trit::Pos));
    assert_eq!(add.layer(0).expect("l0").trit(1, 0), Some(Trit::Pos));
    let sub = combine_tasks(&base, &tasks, &[1.0, -1.0]).expect("sub");
    assert_eq!(sub.layer(0).expect("l0").trit(0, 0), Some(Trit::Pos));
    assert_eq!(sub.layer(0).expect("l0").trit(1, 0), Some(Trit::Neg));
    let only_b = combine_tasks(&base, &tasks, &[0.0, 1.0]).expect("only b");
    assert_eq!(only_b.layer(0).expect("l0").trit(0, 0), Some(Trit::Neg));
    assert_eq!(only_b.layer(0).expect("l0").trit(1, 0), Some(Trit::Pos));
}

#[test]
fn combine_length_mismatch() {
    let (base, a, _b) = fixture();
    let tasks = [TaskVector::between(&base, &a).expect("a")];
    assert!(combine_tasks(&base, &tasks, &[1.0, 0.0]).is_err());
}

#[test]
fn cli_task_combine_round_trip() {
    let dir = temp_dir("cli");
    let (base, a, b) = fixture();
    let base_path = dir.join("base.wsp");
    let a_path = dir.join("a.wsp");
    let b_path = dir.join("b.wsp");
    fs::write(&base_path, to_wsp(&base)).expect("write base");
    fs::write(&a_path, to_wsp(&a)).expect("write a");
    fs::write(&b_path, to_wsp(&b)).expect("write b");
    let value = stdout_json(&run(&[
        "task-combine",
        &path_string(&base_path),
        "--tasks",
        &format!("{},{}", path_string(&a_path), path_string(&b_path)),
        "--lambdas",
        "1,-1",
    ]));
    assert_eq!(value["lambdas"], json!([1.0, -1.0]));
    let artifact = value["artifact"].as_str().expect("artifact");
    let net = trit_wspace::from_wsp(artifact).expect("parse");
    assert_eq!(net.layer(0).expect("l0").trit(0, 0), Some(Trit::Pos));
    assert_eq!(net.layer(0).expect("l0").trit(1, 0), Some(Trit::Neg));
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn mcp_task_combine_round_trip() {
    let (base, a, b) = fixture();
    let mut server = Mcp::new();
    let result = call(
        &mut server,
        "wsp_task_combine",
        json!({
            "base": to_wsp(&base),
            "tasks": [to_wsp(&a), to_wsp(&b)],
            "lambdas": [1.0, -1.0],
        }),
    );
    assert_eq!(result["isError"], false);
    let value = payload(&result);
    let artifact = value["artifact"].as_str().expect("artifact");
    let net = trit_wspace::from_wsp(artifact).expect("parse");
    assert_eq!(net.layer(0).expect("l0").trit(0, 0), Some(Trit::Pos));
    assert_eq!(net.layer(0).expect("l0").trit(1, 0), Some(Trit::Neg));
}
