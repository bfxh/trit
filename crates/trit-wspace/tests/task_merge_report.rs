//! 合并报告、调权网格与 DARE 评估目标的测试。

use std::fs;
use std::path::PathBuf;
use std::process::{Command, Output};

use serde_json::{Value, json};
use trit_wspace::mcp::Mcp;
use trit_wspace::{
    DEFAULT_TUNE_GRID, Edit, Layer, Net, Oracle, Patch, TaskVector, Trit, TuneMerge, TuneMetric,
    merge_tasks_weighted_report, to_wsp, tune_task_weights_metric,
};
fn tuned(base: &Net, edits: Vec<Edit>) -> Net {
    let mut net = base.clone();
    Patch::new(edits).apply(&mut net).expect("apply tune");
    net
}
fn four_inputs() -> Vec<Vec<Trit>> {
    vec![
        vec![Trit::Neg, Trit::Neg],
        vec![Trit::Neg, Trit::Pos],
        vec![Trit::Pos, Trit::Neg],
        vec![Trit::Pos, Trit::Pos],
    ]
}
fn binary_fixture() -> (Net, Vec<TaskVector>, Oracle) {
    let base = Net::new(vec![
        Layer::new(2, 2, &[0i8; 4], &[0i8; 2]).expect("l0"),
        Layer::new(1, 2, &[0i8; 2], &[0i8; 1]).expect("l1"),
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
        &base,
        vec![Edit::SetBias {
            layer: 1,
            row: 0,
            to: Trit::Neg,
        }],
    );
    let tasks = vec![
        TaskVector::between(&base, &a).expect("a"),
        TaskVector::between(&base, &b).expect("b"),
    ];
    let oracle = Oracle::new(four_inputs(), vec![vec![Trit::Pos]; 4]).expect("oracle");
    (base, tasks, oracle)
}

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
fn merge_report_counts_votes_ties_and_ignored() {
    let (base, tasks, _) = binary_fixture();
    let (_, report) = merge_tasks_weighted_report(&base, &tasks, &[1.0, 1.0]).expect("report");
    assert_eq!(report.tasks(), 2);
    assert_eq!(report.ignored_tasks(), 0);
    assert_eq!(report.voted(), 1);
    assert_eq!(report.ties(), 1);
    assert_eq!(report.changed(), 0, "平票保留基准，不应有编辑");
    let (_, report) = merge_tasks_weighted_report(&base, &tasks, &[0.0, 1.0]).expect("report");
    assert_eq!(report.ignored_tasks(), 1);
    assert_eq!(report.voted(), 1);
    assert_eq!(report.ties(), 0);
    assert_eq!(report.changed(), 1, "只剩一个任务时应当改动");
    let scaled = tuned(
        &base,
        vec![Edit::SetScale {
            layer: 0,
            row: 0,
            to: 2.0,
        }],
    );
    let tasks = vec![TaskVector::between(&base, &scaled).expect("scale")];
    let (_, report) = merge_tasks_weighted_report(&base, &tasks, &[1.0]).expect("report");
    assert_eq!(report.scale_rows(), 1);
}

#[test]
fn tune_grid_is_configurable() {
    let (base, tasks, oracle) = binary_fixture();
    let default = tune_task_weights_metric(
        &base,
        &tasks,
        &oracle,
        2,
        TuneMetric::Exact,
        &DEFAULT_TUNE_GRID,
        TuneMerge::Plain,
    )
    .expect("default grid");
    let custom = tune_task_weights_metric(
        &base,
        &tasks,
        &oracle,
        2,
        TuneMetric::Exact,
        &[0.0, 2.0],
        TuneMerge::Plain,
    )
    .expect("custom grid");
    assert_eq!(custom.loss(), 0);
    assert!(
        custom.evaluations() < default.evaluations(),
        "自定义 2 项网格必须比默认 4 项网格评估更少"
    );
    assert!(
        tune_task_weights_metric(
            &base,
            &tasks,
            &oracle,
            1,
            TuneMetric::Exact,
            &[],
            TuneMerge::Plain,
        )
        .is_err()
    );
    assert!(
        tune_task_weights_metric(
            &base,
            &tasks,
            &oracle,
            0,
            TuneMetric::Exact,
            &[1.0, -1.0],
            TuneMerge::Plain,
        )
        .is_err(),
        "负候选必须在进网格校验时就被拒绝（rounds=0 没有下游权重校验兜底）"
    );
}

#[test]
fn tune_with_dare_changes_initial_loss() {
    let (base, tasks, oracle) = ten_class_fixture();
    let plain = tune_task_weights_metric(
        &base,
        &tasks,
        &oracle,
        0,
        TuneMetric::Argmax,
        &DEFAULT_TUNE_GRID,
        TuneMerge::Plain,
    )
    .expect("plain");
    let mut found = false;
    for seed in 0..64 {
        let dare = tune_task_weights_metric(
            &base,
            &tasks,
            &oracle,
            0,
            TuneMetric::Argmax,
            &DEFAULT_TUNE_GRID,
            TuneMerge::Dare { rate: 0.5, seed },
        )
        .expect("dare");
        if dare.initial_loss() != plain.initial_loss() {
            found = true;
            break;
        }
    }
    assert!(found, "DARE 评估必须改变 initial loss");
    let mut found_per_task = false;
    for seed in 0..64 {
        let per_task = tune_task_weights_metric(
            &base,
            &tasks,
            &oracle,
            0,
            TuneMetric::Argmax,
            &DEFAULT_TUNE_GRID,
            TuneMerge::PerTaskDare { rate: 0.5, seed },
        )
        .expect("per-task dare");
        if per_task.initial_loss() != plain.initial_loss() {
            found_per_task = true;
            break;
        }
    }
    assert!(found_per_task, "PerTaskDare 评估必须改变 initial loss");
}

#[test]
fn cli_task_merge_grid_and_report() {
    let dir = temp_dir("cli-grid");
    let (base, _, oracle) = binary_fixture();
    let a = tuned(
        &base,
        vec![Edit::SetBias {
            layer: 1,
            row: 0,
            to: Trit::Pos,
        }],
    );
    let b = tuned(
        &base,
        vec![Edit::SetBias {
            layer: 1,
            row: 0,
            to: Trit::Neg,
        }],
    );
    let base_path = dir.join("base.wsp");
    let a_path = dir.join("a.wsp");
    let b_path = dir.join("b.wsp");
    let oracle_path = dir.join("oracle.json");
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
        "--tune-grid",
        "0,2",
        "--tune-rounds",
        "1",
    ]));
    assert_eq!(output["tuning"]["grid"], json!([0.0, 2.0]));
    assert_eq!(output["tuning"]["merge"], json!("plain"));
    assert_eq!(output["merge_report"]["tasks"], json!(2));
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn cli_task_merge_tune_with_dare() {
    let dir = temp_dir("cli-tune-dare");
    let (base, _, oracle) = binary_fixture();
    let a = tuned(
        &base,
        vec![Edit::SetBias {
            layer: 1,
            row: 0,
            to: Trit::Pos,
        }],
    );
    let b = tuned(
        &base,
        vec![Edit::SetBias {
            layer: 1,
            row: 0,
            to: Trit::Neg,
        }],
    );
    let base_path = dir.join("base.wsp");
    let a_path = dir.join("a.wsp");
    let b_path = dir.join("b.wsp");
    let oracle_path = dir.join("oracle.json");
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
        "--tune-with-dare",
        "--dare-rate",
        "0.5",
        "--tune-rounds",
        "1",
    ]));
    assert_eq!(output["tuning"]["merge"], json!("dare"));
    assert_eq!(output["merge_report"]["tasks"], json!(2));
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn mcp_task_merge_grid_and_report() {
    let (base, _, oracle) = binary_fixture();
    let a = tuned(
        &base,
        vec![Edit::SetBias {
            layer: 1,
            row: 0,
            to: Trit::Pos,
        }],
    );
    let b = tuned(
        &base,
        vec![Edit::SetBias {
            layer: 1,
            row: 0,
            to: Trit::Neg,
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
            "tune_grid": [0.0, 2.0],
            "tune_rounds": 1,
        }),
    );
    assert_eq!(result["isError"], false);
    let value = payload(&result);
    assert_eq!(value["tuning"]["grid"], json!([0.0, 2.0]));
    assert_eq!(value["tuning"]["merge"], json!("plain"));
    assert_eq!(value["merge_report"]["tasks"], json!(2));
}
