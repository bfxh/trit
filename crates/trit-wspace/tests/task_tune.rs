//! 用验证集自动选任务权重（坐标下降）。

use std::fs;
use std::path::PathBuf;
use std::process::{Command, Output};

use serde_json::{Value, json};
use trit_wspace::mcp::Mcp;
use trit_wspace::{
    Edit, Layer, Net, Oracle, Patch, TaskVector, Trit, merge_tasks_weighted, to_wsp,
    tune_task_weights,
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

fn four_inputs() -> Vec<Vec<Trit>> {
    vec![
        vec![Trit::Neg, Trit::Neg],
        vec![Trit::Neg, Trit::Pos],
        vec![Trit::Pos, Trit::Neg],
        vec![Trit::Pos, Trit::Pos],
    ]
}

fn oracle_for(net: &Net) -> Oracle {
    let inputs = four_inputs();
    let probe = Oracle::new(inputs.clone(), vec![vec![Trit::Pos]; inputs.len()]).expect("probe");
    let outputs = probe.outputs_of(net).expect("outputs");
    Oracle::new(inputs, outputs).expect("oracle")
}

/// 基准输出层坐标是 `0`；任务 A 改成 `+`（验证集就按 A 的行为写），任务 B 改成 `-`。
/// 等权投票在这一坐标平票 ⇒ 回退到基准，loss 变大；把 B 的权重压到 0 应该恢复 A。
fn fixture() -> (Net, Vec<TaskVector>, Oracle) {
    let base = tuned(&xor_net(), vec![set(1, 0, 0, Trit::Zero)]);
    let pos = tuned(&base, vec![set(1, 0, 0, Trit::Pos)]);
    let neg = tuned(&base, vec![set(1, 0, 0, Trit::Neg)]);
    let oracle = oracle_for(&pos);
    let tasks = vec![
        TaskVector::between(&base, &pos).expect("pos"),
        TaskVector::between(&base, &neg).expect("neg"),
    ];
    (base, tasks, oracle)
}

fn temp_dir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "trit-wspace-task-tune-{tag}-{}",
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
            "jsonrpc": "2.0", "id": 21, "method": "tools/call",
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

#[test]
fn tune_weight_finds_lower_loss() {
    let (base, tasks, oracle) = fixture();
    let equal = oracle
        .loss(&merge_tasks_weighted(&base, &tasks, &[1.0, 1.0]).expect("equal"))
        .expect("loss");
    let tuning = tune_task_weights(&base, &tasks, &oracle, 2).expect("tune");
    assert_eq!(tuning.initial_loss(), equal);
    assert!(
        tuning.loss() < equal,
        "调优必须严格降低 loss：等权 {equal}，调优 {}",
        tuning.loss()
    );
    assert_ne!(
        tuning.weights(),
        &[1.0_f32, 1.0],
        "必须至少调整一个权重：{:?}",
        tuning.weights()
    );
    assert!(tuning.evaluations() > 1);
}

#[test]
fn tune_weight_zero_rounds_keeps_equal_weights() {
    let (base, tasks, oracle) = fixture();
    let tuning = tune_task_weights(&base, &tasks, &oracle, 0).expect("tune");
    assert_eq!(tuning.weights(), &[1.0, 1.0]);
    assert_eq!(tuning.loss(), tuning.initial_loss());
    assert_eq!(tuning.evaluations(), 1);
}

#[test]
fn tune_weight_counts_evaluations() {
    let (base, tasks, oracle) = fixture();
    let tuning = tune_task_weights(&base, &tasks, &oracle, 1).expect("tune");
    assert_eq!(tuning.evaluations(), 1 + tasks.len() * 3);
}

#[test]
fn tune_weight_rejects_empty() {
    let base = xor_net();
    let oracle = oracle_for(&base);
    assert!(tune_task_weights(&base, &[], &oracle, 1).is_err());
}

#[test]
fn cli_task_merge_tune_weights() {
    let dir = temp_dir("cli");
    let (base, tasks, oracle) = fixture();
    let base_path = dir.join("base.wsp");
    let pos_path = dir.join("pos.wsp");
    let neg_path = dir.join("neg.wsp");
    let oracle_path = dir.join("oracle.json");
    let out_path = dir.join("out.wsp");
    let pos = tuned(&base, vec![set(1, 0, 0, Trit::Pos)]);
    let neg = tuned(&base, vec![set(1, 0, 0, Trit::Neg)]);
    fs::write(&base_path, to_wsp(&base)).expect("write base");
    fs::write(&pos_path, to_wsp(&pos)).expect("write pos");
    fs::write(&neg_path, to_wsp(&neg)).expect("write neg");
    fs::write(
        &oracle_path,
        serde_json::to_string(&oracle_value(&oracle)).expect("oracle json"),
    )
    .expect("write oracle");
    let output = stdout_json(&run(&[
        "task-merge",
        &path_string(&base_path),
        &path_string(&pos_path),
        &path_string(&neg_path),
        "--oracle",
        &path_string(&oracle_path),
        "--tune-weights",
        "--tune-rounds",
        "2",
        "--out",
        &path_string(&out_path),
    ]));
    let expected = tune_task_weights(&base, &tasks, &oracle, 2).expect("expected");
    assert_eq!(output["weights"], json!(expected.weights()));
    assert_eq!(output["tuning"]["rounds"], json!(2));
    assert_eq!(
        output["tuning"]["evaluations"],
        json!(expected.evaluations())
    );
    assert_eq!(
        output["tuning"]["initial_loss"],
        json!(expected.initial_loss())
    );
    assert_eq!(output["tuning"]["tuned_loss"], json!(expected.loss()));
    assert_eq!(output["verify"]["after"]["loss"], json!(expected.loss()));
    let merged =
        trit_wspace::from_wsp(&fs::read_to_string(&out_path).expect("read out")).expect("parse");
    let want = merge_tasks_weighted(&base, &tasks, expected.weights()).expect("merge");
    assert_eq!(merged.state_hash(), want.state_hash());
    let bad = run(&[
        "task-merge",
        &path_string(&base_path),
        &path_string(&pos_path),
        "--tune-weights",
    ]);
    assert!(!bad.status.success());
    assert!(
        String::from_utf8_lossy(&bad.stderr).contains("--tune-weights 需要 --oracle"),
        "stderr: {}",
        String::from_utf8_lossy(&bad.stderr)
    );
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn mcp_task_merge_tune_weights() {
    let (base, tasks, oracle) = fixture();
    let mut server = Mcp::new();
    let result = call(
        &mut server,
        "wsp_task_merge",
        json!({
            "base": to_wsp(&base),
            "tasks": [to_wsp(&tuned(&base, vec![set(1, 0, 0, Trit::Pos)])),
                      to_wsp(&tuned(&base, vec![set(1, 0, 0, Trit::Neg)]))],
            "oracle": oracle_value(&oracle),
            "tune_weights": true,
            "tune_rounds": 2,
        }),
    );
    assert_eq!(result["isError"], false);
    let value = payload(&result);
    let expected = tune_task_weights(&base, &tasks, &oracle, 2).expect("expected");
    assert_eq!(value["weights"], json!(expected.weights()));
    assert_eq!(value["tuning"]["tuned_loss"], json!(expected.loss()));
    assert_eq!(value["verify"]["after"]["loss"], json!(expected.loss()));
    let bad = call(
        &mut server,
        "wsp_task_merge",
        json!({"base": to_wsp(&base), "tasks": [to_wsp(&tuned(&base, vec![set(1, 0, 0, Trit::Pos)]))], "tune_weights": true}),
    );
    assert_eq!(bad["isError"], true);
}
