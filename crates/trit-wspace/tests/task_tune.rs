//! 用验证集自动选任务权重（坐标下降）。

use std::fs;
use std::path::PathBuf;
use std::process::{Command, Output};

use serde_json::{Value, json};
use trit_wspace::mcp::Mcp;
use trit_wspace::{
    Edit, Layer, Net, Oracle, Patch, TaskVector, Trit, merge_tasks_weighted, to_wsp,
    tune_task_weights, tune_task_weights_argmax, tune_task_weights_hamming,
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

/// 10 类 one-hot 夹具：任何权重都无法让样本**完全匹配**，但 Hamming 能区分接近程度。
fn ten_class_fixture() -> (Net, Vec<TaskVector>, Oracle) {
    let base = Net::new(vec![
        Layer::new(2, 2, &[0i8; 4], &[0i8; 2]).expect("l0"),
        Layer::new(10, 2, &[0i8; 20], &[-1i8; 10]).expect("l1"),
    ])
    .expect("net");
    let with_bias = |row: usize| {
        tuned(
            &base,
            vec![Edit::SetBias {
                layer: 1,
                row,
                to: Trit::Pos,
            }],
        )
    };
    let task_a = tuned(
        &with_bias(0),
        vec![Edit::SetBias {
            layer: 1,
            row: 1,
            to: Trit::Pos,
        }],
    );
    let task_b = tuned(
        &with_bias(0),
        vec![Edit::SetBias {
            layer: 1,
            row: 2,
            to: Trit::Pos,
        }],
    );
    let tasks = vec![
        TaskVector::between(&base, &task_a).expect("a"),
        TaskVector::between(&base, &task_b).expect("b"),
    ];
    let mut want = vec![Trit::Neg; 10];
    want[0] = Trit::Pos;
    let oracle = Oracle::new(four_inputs(), vec![want; 4]).expect("oracle");
    (base, tasks, oracle)
}

#[test]
fn tune_metric_exact_stalls_on_one_hot() {
    let (base, tasks, oracle) = ten_class_fixture();
    let tuning = tune_task_weights(&base, &tasks, &oracle, 2).expect("exact");
    assert_eq!(
        tuning.weights(),
        &[1.0, 1.0],
        "逐样本完全匹配在 one-hot 上没有信号：{tuning:?}"
    );
    assert_eq!(tuning.loss(), tuning.initial_loss());
}

#[test]
fn tune_metric_hamming_moves_weights() {
    let (base, tasks, oracle) = ten_class_fixture();
    let tuning = tune_task_weights_hamming(&base, &tasks, &oracle, 2).expect("hamming");
    assert!(
        tuning.loss() < tuning.initial_loss(),
        "hamming 必须能区分接近程度：初始 {}，调优 {}",
        tuning.initial_loss(),
        tuning.loss()
    );
    assert_eq!(tuning.weights()[0], 0.0, "把制造冲突的任务压到 0");
}

#[test]
fn cli_task_merge_tune_metric() {
    let dir = temp_dir("cli-metric");
    let (base, _, oracle) = ten_class_fixture();
    let base_path = dir.join("base.wsp");
    let a_path = dir.join("a.wsp");
    let b_path = dir.join("b.wsp");
    let oracle_path = dir.join("oracle.json");
    let base_net = base.clone();
    let task_a = tuned(
        &tuned(
            &base_net,
            vec![Edit::SetBias {
                layer: 1,
                row: 0,
                to: Trit::Pos,
            }],
        ),
        vec![Edit::SetBias {
            layer: 1,
            row: 1,
            to: Trit::Pos,
        }],
    );
    let task_b = tuned(
        &tuned(
            &base_net,
            vec![Edit::SetBias {
                layer: 1,
                row: 0,
                to: Trit::Pos,
            }],
        ),
        vec![Edit::SetBias {
            layer: 1,
            row: 2,
            to: Trit::Pos,
        }],
    );
    fs::write(&base_path, to_wsp(&base_net)).expect("write base");
    fs::write(&a_path, to_wsp(&task_a)).expect("write a");
    fs::write(&b_path, to_wsp(&task_b)).expect("write b");
    fs::write(
        &oracle_path,
        serde_json::to_string(&oracle_value(&oracle)).expect("oracle json"),
    )
    .expect("write oracle");
    let output = stdout_json(&run(&[
        "task-merge",
        &path_string(&base_path),
        &path_string(&a_path),
        &path_string(&b_path),
        "--oracle",
        &path_string(&oracle_path),
        "--tune-weights",
        "--tune-metric",
        "hamming",
        "--tune-rounds",
        "2",
    ]));
    assert_eq!(output["tuning"]["metric"], json!("hamming"));
    let tasks = [
        TaskVector::between(&base_net, &task_a).expect("a"),
        TaskVector::between(&base_net, &task_b).expect("b"),
    ];
    let expected = tune_task_weights_hamming(&base_net, &tasks, &oracle, 2).expect("expected");
    assert_eq!(output["weights"], json!(expected.weights()));
    assert_eq!(output["tuning"]["tuned_loss"], json!(expected.loss()));
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn mcp_task_merge_tune_metric() {
    let (base, tasks, oracle) = ten_class_fixture();
    let a = tol(&base, 0, 1);
    let b = tol(&base, 0, 2);
    let mut server = Mcp::new();
    let result = call(
        &mut server,
        "wsp_task_merge",
        json!({
            "base": to_wsp(&base),
            "tasks": [to_wsp(&a), to_wsp(&b)],
            "oracle": oracle_value(&oracle),
            "tune_weights": true,
            "tune_metric": "hamming",
            "tune_rounds": 2,
        }),
    );
    assert_eq!(result["isError"], false);
    let value = payload(&result);
    let expected = tune_task_weights_hamming(&base, &tasks, &oracle, 2).expect("expected");
    assert_eq!(value["tuning"]["metric"], json!("hamming"));
    assert_eq!(value["weights"], json!(expected.weights()));
    let bad = call(
        &mut server,
        "wsp_task_merge",
        json!({
            "base": to_wsp(&base),
            "tasks": [to_wsp(&a), to_wsp(&b)],
            "oracle": oracle_value(&oracle),
            "tune_weights": true,
            "tune_metric": "nope",
        }),
    );
    assert_eq!(bad["isError"], true);
}

fn tol(base: &Net, first: usize, extra: usize) -> Net {
    tuned(
        &tuned(
            base,
            vec![Edit::SetBias {
                layer: 1,
                row: first,
                to: Trit::Pos,
            }],
        ),
        vec![Edit::SetBias {
            layer: 1,
            row: extra,
            to: Trit::Pos,
        }],
    )
}

/// 分类夹具：base 的 argmax 错误；任务 A 单独正确、任务 B 单独仍错；等权合并平票回退到 base。
fn argmax_fixture() -> (Net, Vec<TaskVector>, Oracle) {
    let base = Net::new(vec![
        Layer::new(2, 2, &[0i8; 4], &[0i8; 2]).expect("l0"),
        Layer::new(10, 2, &[0i8; 20], &[0, 1, -1, -1, -1, -1, -1, -1, -1, -1]).expect("l1"),
    ])
    .expect("net");
    let a = tuned(
        &base,
        vec![Edit::SetBias {
            layer: 1,
            row: 0,
            to: Trit::Pos,
        }],
    );
    let b = tuned(
        &tuned(
            &base,
            vec![Edit::SetBias {
                layer: 1,
                row: 0,
                to: Trit::Neg,
            }],
        ),
        vec![Edit::SetBias {
            layer: 1,
            row: 2,
            to: Trit::Pos,
        }],
    );
    let tasks = vec![
        TaskVector::between(&base, &a).expect("a"),
        TaskVector::between(&base, &b).expect("b"),
    ];
    let mut want = vec![Trit::Neg; 10];
    want[0] = Trit::Pos;
    let oracle = Oracle::new(four_inputs(), vec![want; 4]).expect("oracle");
    (base, tasks, oracle)
}

#[test]
fn tune_metric_argmax_counts_classification_errors() {
    let (base, tasks, oracle) = argmax_fixture();
    let tuning = tune_task_weights_argmax(&base, &tasks, &oracle, 2).expect("argmax");
    assert_eq!(tuning.initial_loss(), 4, "等权合并分类全错");
    assert_eq!(tuning.loss(), 0, "argmax 目标必须把分类全部修好");
    assert_eq!(tuning.weights()[0], 2.0);
}

#[test]
fn tune_metric_argmax_differs_from_hamming() {
    let (base, tasks, oracle) = argmax_fixture();
    let argmax = tune_task_weights_argmax(&base, &tasks, &oracle, 2).expect("argmax");
    let hamming = tune_task_weights_hamming(&base, &tasks, &oracle, 2).expect("hamming");
    assert_eq!(argmax.loss(), 0);
    assert!(
        hamming.loss() > 0,
        "hamming 允许剩余位错，argmax 只关心分类"
    );
    assert_ne!(argmax.weights(), hamming.weights());
}

#[test]
fn cli_task_merge_argmax_metric() {
    let dir = temp_dir("cli-argmax");
    let (base, tasks, oracle) = argmax_fixture();
    let base_path = dir.join("base.wsp");
    let a_path = dir.join("a.wsp");
    let b_path = dir.join("b.wsp");
    let oracle_path = dir.join("oracle.json");
    let a = tuned(
        &base,
        vec![Edit::SetBias {
            layer: 1,
            row: 0,
            to: Trit::Pos,
        }],
    );
    let b = tuned(
        &tuned(
            &base,
            vec![Edit::SetBias {
                layer: 1,
                row: 0,
                to: Trit::Neg,
            }],
        ),
        vec![Edit::SetBias {
            layer: 1,
            row: 2,
            to: Trit::Pos,
        }],
    );
    fs::write(&base_path, to_wsp(&base)).expect("write base");
    fs::write(&a_path, to_wsp(&a)).expect("write a");
    fs::write(&b_path, to_wsp(&b)).expect("write b");
    fs::write(
        &oracle_path,
        serde_json::to_string(&oracle_value(&oracle)).expect("oracle json"),
    )
    .expect("write oracle");
    let output = stdout_json(&run(&[
        "task-merge",
        &path_string(&base_path),
        &path_string(&a_path),
        &path_string(&b_path),
        "--oracle",
        &path_string(&oracle_path),
        "--tune-weights",
        "--tune-metric",
        "argmax",
        "--tune-rounds",
        "2",
    ]));
    assert_eq!(output["tuning"]["metric"], json!("argmax"));
    let expected = tune_task_weights_argmax(&base, &tasks, &oracle, 2).expect("expected");
    assert_eq!(output["weights"], json!(expected.weights()));
    assert_eq!(output["tuning"]["tuned_loss"], json!(0));
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn mcp_task_merge_argmax_metric() {
    let (base, tasks, oracle) = argmax_fixture();
    let a = tuned(
        &base,
        vec![Edit::SetBias {
            layer: 1,
            row: 0,
            to: Trit::Pos,
        }],
    );
    let b = tuned(
        &tuned(
            &base,
            vec![Edit::SetBias {
                layer: 1,
                row: 0,
                to: Trit::Neg,
            }],
        ),
        vec![Edit::SetBias {
            layer: 1,
            row: 2,
            to: Trit::Pos,
        }],
    );
    let mut server = Mcp::new();
    let result = call(
        &mut server,
        "wsp_task_merge",
        json!({
            "base": to_wsp(&base),
            "tasks": [to_wsp(&a), to_wsp(&b)],
            "oracle": oracle_value(&oracle),
            "tune_weights": true,
            "tune_metric": "argmax",
            "tune_rounds": 2,
        }),
    );
    assert_eq!(result["isError"], false);
    let value = payload(&result);
    let expected = tune_task_weights_argmax(&base, &tasks, &oracle, 2).expect("expected");
    assert_eq!(value["tuning"]["metric"], json!("argmax"));
    assert_eq!(value["weights"], json!(expected.weights()));
    assert_eq!(value["tuning"]["tuned_loss"], json!(0));
}
