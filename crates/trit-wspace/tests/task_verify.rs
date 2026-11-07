//! 任务合并的验证集读数（可选 oracle）。

use std::fs;
use std::path::PathBuf;
use std::process::{Command, Output};

use serde_json::{Value, json};
use trit_wspace::mcp::Mcp;
use trit_wspace::{Edit, Layer, Net, Oracle, Patch, TaskVector, Trit, merge_tasks, to_wsp};

fn xor_net() -> Net {
    Net::new(vec![
        Layer::new(2, 2, &[-1, -1, -1, -1], &[-1, 1]).expect("l0"),
        Layer::new(1, 2, &[-1, 1], &[-1]).expect("l1"),
    ])
    .expect("xor net")
}

fn negate_output(base: &Net) -> Net {
    let mut net = base.clone();
    Patch::new(vec![Edit::NegateRow { layer: 1, row: 0 }])
        .apply(&mut net)
        .expect("negate");
    net
}

fn temp_dir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "trit-wspace-task-verify-{tag}-{}",
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
            "jsonrpc": "2.0", "id": 19, "method": "tools/call",
            "params": {"name": name, "arguments": args},
        }))
        .expect("response")["result"]
        .clone()
}

fn payload(result: &Value) -> Value {
    let raw = result["content"][0]["text"].as_str().expect("text content");
    serde_json::from_str(raw).expect("json payload")
}

fn oracle_value(oracle: &Oracle) -> Value {
    trit_wspace::wire::oracle_to_value(oracle)
}

/// 与 `base` 当前行为**逐样本相反**的验证集：合并前 loss 全满，取反输出后 loss=0。
fn opposite_oracle(base: &Net) -> Oracle {
    let inputs: Vec<Vec<Trit>> = vec![
        vec![Trit::Neg, Trit::Neg],
        vec![Trit::Neg, Trit::Pos],
        vec![Trit::Pos, Trit::Neg],
        vec![Trit::Pos, Trit::Pos],
    ];
    let probe = Oracle::new(inputs.clone(), vec![vec![Trit::Pos]; inputs.len()]).expect("probe");
    let outputs = probe.outputs_of(base).expect("outputs");
    let flipped: Vec<Vec<Trit>> = outputs
        .iter()
        .map(|row| {
            row.iter()
                .map(|trit| match trit {
                    Trit::Pos => Trit::Neg,
                    Trit::Neg => Trit::Pos,
                    Trit::Zero => Trit::Zero,
                })
                .collect()
        })
        .collect();
    Oracle::new(inputs, flipped).expect("opposite")
}

#[test]
fn cli_task_merge_verify_readings() {
    let dir = temp_dir("cli");
    let base = xor_net();
    let tuned = negate_output(&base);
    let oracle = opposite_oracle(&base);
    let base_path = dir.join("base.wsp");
    let tuned_path = dir.join("tuned.wsp");
    let oracle_path = dir.join("oracle.json");
    fs::write(&base_path, to_wsp(&base)).expect("write base");
    fs::write(&tuned_path, to_wsp(&tuned)).expect("write tuned");
    fs::write(
        &oracle_path,
        serde_json::to_string(&oracle_value(&oracle)).expect("oracle json"),
    )
    .expect("write oracle");
    let output = stdout_json(&run(&[
        "task-merge",
        &path_string(&base_path),
        &path_string(&tuned_path),
        "--oracle",
        &path_string(&oracle_path),
    ]));
    let tasks = [TaskVector::between(&base, &tuned).expect("task")];
    let merged = merge_tasks(&base, &tasks).expect("merge");
    assert_eq!(
        output["verify"]["before"]["loss"],
        json!(oracle.loss(&base).expect("before")),
        "before 必须是基准读数"
    );
    assert_eq!(
        output["verify"]["after"]["loss"],
        json!(oracle.loss(&merged).expect("after")),
        "after 必须是合并后读数"
    );
    assert_eq!(
        output["verify"]["after"]["exact"],
        json!(oracle.exact(&merged).expect("exact"))
    );
    assert_ne!(
        output["verify"]["before"]["loss"], output["verify"]["after"]["loss"],
        "夹具必须让合并前后读数不同"
    );
    let plain = stdout_json(&run(&[
        "task-merge",
        &path_string(&base_path),
        &path_string(&tuned_path),
    ]));
    assert!(
        plain["verify"].is_null(),
        "不传 oracle 时 verify 必须是 null"
    );
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn mcp_task_merge_verify_readings() {
    let base = xor_net();
    let tuned = negate_output(&base);
    let oracle = opposite_oracle(&base);
    let mut server = Mcp::new();
    let result = call(
        &mut server,
        "wsp_task_merge",
        json!({
            "base": to_wsp(&base),
            "tasks": [to_wsp(&tuned)],
            "oracle": oracle_value(&oracle),
        }),
    );
    assert_eq!(result["isError"], false);
    let value = payload(&result);
    let tasks = [TaskVector::between(&base, &tuned).expect("task")];
    let merged = merge_tasks(&base, &tasks).expect("merge");
    assert_eq!(
        value["verify"]["before"]["loss"],
        json!(oracle.loss(&base).expect("before"))
    );
    assert_eq!(
        value["verify"]["after"]["loss"],
        json!(oracle.loss(&merged).expect("after"))
    );
    assert_ne!(
        value["verify"]["before"]["loss"],
        value["verify"]["after"]["loss"]
    );
    let plain = call(
        &mut server,
        "wsp_task_merge",
        json!({"base": to_wsp(&base), "tasks": [to_wsp(&tuned)]}),
    );
    assert!(
        payload(&plain)["verify"].is_null(),
        "不传 oracle 时 verify 必须是 null"
    );
}
