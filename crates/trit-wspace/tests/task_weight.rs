//! 三值任务向量的加权合并。

use std::fs;
use std::path::PathBuf;
use std::process::{Command, Output};

use serde_json::{Value, json};
use trit_wspace::mcp::Mcp;
use trit_wspace::{
    Edit, Layer, Net, Patch, TaskVector, Trit, dare_drop, merge_tasks, merge_tasks_per_task_dare,
    merge_tasks_weighted, merge_tasks_weighted_dare, merge_tasks_weighted_per_task_dare, to_wsp,
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

fn full_net(value: i8) -> Net {
    let mut built = Vec::new();
    for _ in 0..2 {
        built.push(Layer::new(4, 4, &[value; 16], &[value; 4]).expect("full layer"));
    }
    Net::new(built).expect("full net")
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

fn temp_dir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "trit-wspace-task-weight-{tag}-{}",
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
            "jsonrpc": "2.0", "id": 17, "method": "tools/call",
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
fn merge_weight_all_ones_matches_plain_merge() {
    let base = xor_net();
    let mut first = with_scale(&base, 0, 0, 2.0);
    Patch::new(vec![set(0, 0, 0, Trit::Zero), set(0, 1, 1, Trit::Zero)])
        .apply(&mut first)
        .expect("first");
    let mut second = with_scale(&base, 0, 0, 4.0);
    Patch::new(vec![set(0, 0, 0, Trit::Pos), set(0, 1, 0, Trit::Pos)])
        .apply(&mut second)
        .expect("second");
    let tasks = [
        TaskVector::between(&base, &first).expect("first"),
        TaskVector::between(&base, &second).expect("second"),
    ];
    let plain = merge_tasks(&base, &tasks).expect("plain");
    let weighted = merge_tasks_weighted(&base, &tasks, &[1.0, 1.0]).expect("weighted");
    assert_eq!(weighted, plain, "等权必须与 merge_tasks 逐位一致");
}

#[test]
fn merge_weight_flips_tie() {
    let base = tuned(&xor_net(), vec![set(0, 0, 0, Trit::Zero)]);
    let pos = tuned(&base, vec![set(0, 0, 0, Trit::Pos)]);
    let neg = tuned(&base, vec![set(0, 0, 0, Trit::Neg)]);
    let tasks = [
        TaskVector::between(&base, &pos).expect("pos"),
        TaskVector::between(&base, &neg).expect("neg"),
    ];
    let tied = merge_tasks_weighted(&base, &tasks, &[1.0, 1.0]).expect("tie");
    assert_eq!(
        tied.layer(0).expect("l0").trit(0, 0),
        Some(Trit::Zero),
        "等权平票保留基准"
    );
    let neg_wins = merge_tasks_weighted(&base, &tasks, &[1.0, 2.0]).expect("neg wins");
    assert_eq!(neg_wins.layer(0).expect("l0").trit(0, 0), Some(Trit::Neg));
    let pos_wins = merge_tasks_weighted(&base, &tasks, &[2.0, 1.0]).expect("pos wins");
    assert_eq!(pos_wins.layer(0).expect("l0").trit(0, 0), Some(Trit::Pos));
}

#[test]
fn merge_weight_zero_ignores_task() {
    let base = xor_net();
    let left = tuned(&base, vec![set(0, 0, 0, Trit::Zero)]);
    let right = tuned(&base, vec![set(0, 0, 1, Trit::Zero)]);
    let tasks = [
        TaskVector::between(&base, &left).expect("left"),
        TaskVector::between(&base, &right).expect("right"),
    ];
    let only_right = merge_tasks_weighted(&base, &tasks, &[0.0, 1.0]).expect("right");
    assert_eq!(
        only_right,
        merge_tasks(&base, &tasks[1..]).expect("right only"),
        "权重 0 的任务必须完全不参与"
    );
    let only_left = merge_tasks_weighted(&base, &tasks, &[1.0, 0.0]).expect("left");
    assert_eq!(
        only_left,
        merge_tasks(&base, &tasks[..1]).expect("left only")
    );
    let none = merge_tasks_weighted(&base, &tasks, &[0.0, 0.0]).expect("none");
    assert_eq!(none, base, "全零权重等于不合并");
}

#[test]
fn merge_weight_scale_median() {
    let base = xor_net();
    let tasks = [2.0_f32, 4.0, 100.0]
        .map(|scale| TaskVector::between(&base, &with_scale(&base, 0, 0, scale)).expect("task"));
    let equal = merge_tasks_weighted(&base, &tasks, &[1.0, 1.0, 1.0]).expect("equal");
    assert_eq!(equal.layer(0).expect("l0").scale(0), Some(4.0));
    let heavy = merge_tasks_weighted(&base, &tasks, &[1.0, 1.0, 3.0]).expect("heavy");
    assert_eq!(
        heavy.layer(0).expect("l0").scale(0),
        Some(100.0),
        "大权重把加权中位数拉向自己"
    );
    let zero_tail = merge_tasks_weighted(&base, &tasks, &[1.0, 1.0, 0.0]).expect("zero tail");
    assert_eq!(
        zero_tail.layer(0).expect("l0").scale(0),
        Some(3.0),
        "0 权重不进统计，两个值取平均"
    );
    let zero_middle = merge_tasks_weighted(&base, &tasks, &[3.0, 1.0, 0.0]).expect("zero middle");
    assert_eq!(zero_middle.layer(0).expect("l0").scale(0), Some(2.0));
}

#[test]
fn merge_weight_rejects_invalid() {
    let base = xor_net();
    let changed = tuned(&base, vec![set(0, 0, 0, Trit::Zero)]);
    let tasks = [TaskVector::between(&base, &changed).expect("task")];
    assert!(
        merge_tasks_weighted(&base, &tasks, &[]).is_err(),
        "长度不符"
    );
    assert!(
        merge_tasks_weighted(&base, &tasks, &[1.0, 1.0]).is_err(),
        "多余权重"
    );
    assert!(
        merge_tasks_weighted(&base, &tasks, &[-1.0]).is_err(),
        "负权重"
    );
    assert!(
        merge_tasks_weighted(&base, &tasks, &[f32::NAN]).is_err(),
        "NaN"
    );
    assert!(
        merge_tasks_weighted(&base, &tasks, &[f32::INFINITY]).is_err(),
        "无穷"
    );
    let foreign = tuned(&base, vec![set(0, 0, 1, Trit::Zero)]);
    assert!(
        merge_tasks_weighted(&foreign, &tasks, &[1.0]).is_err(),
        "基准不一致"
    );
}

#[test]
fn merge_weight_empty_inputs() {
    let base = xor_net();
    assert_eq!(merge_tasks_weighted(&base, &[], &[]).expect("empty"), base);
}

#[test]
fn merge_weighted_dare_zero_matches_weighted() {
    let base = tuned(&xor_net(), vec![set(0, 0, 0, Trit::Zero)]);
    let pos = tuned(&base, vec![set(0, 0, 0, Trit::Pos)]);
    let mut neg = tuned(&base, vec![set(0, 0, 0, Trit::Neg)]);
    Patch::new(vec![Edit::SetScale {
        layer: 0,
        row: 1,
        to: 2.0,
    }])
    .apply(&mut neg)
    .expect("scale");
    let tasks = [
        TaskVector::between(&base, &pos).expect("pos"),
        TaskVector::between(&base, &neg).expect("neg"),
    ];
    let weights = [1.0, 9.0];
    let plain = merge_tasks_weighted(&base, &tasks, &weights).expect("weighted");
    assert_eq!(
        plain.layer(0).expect("l0").trit(0, 0),
        Some(Trit::Neg),
        "加权投票必须取 Neg"
    );
    assert_ne!(
        plain,
        merge_tasks(&base, &tasks).expect("plain"),
        "权重必须改变合并结果"
    );
    let dare_zero = merge_tasks_weighted_dare(&base, &tasks, &weights, 0.0, 7).expect("rate 0");
    assert_eq!(dare_zero, plain);
    let outcome = dare_drop(&base, &plain, 0.5, 7).expect("drop");
    let mut expected = base.clone();
    outcome.patch.apply(&mut expected).expect("apply drop");
    let got = merge_tasks_weighted_dare(&base, &tasks, &weights, 0.5, 7).expect("dare");
    assert_eq!(got, expected, "稀疏化必须建立在加权合并结果上");
    assert_ne!(got, plain);
}

#[test]
fn merge_weighted_per_task_dare_zero_matches_weighted() {
    let base = full_net(-1);
    let first = full_net(1);
    let second = with_scale(&base, 0, 0, 2.0);
    let tasks = [
        TaskVector::between(&base, &first).expect("first"),
        TaskVector::between(&base, &second).expect("second"),
    ];
    let weights = [2.0, 1.0];
    let plain = merge_tasks_weighted(&base, &tasks, &weights).expect("weighted");
    let per = merge_tasks_weighted_per_task_dare(&base, &tasks, &weights, 0.0, 7).expect("rate 0");
    assert_eq!(per, plain);
    assert!(
        merge_tasks_weighted_per_task_dare(&base, &tasks, &weights, 1.0, 0).is_err(),
        "非法 rate"
    );
}

#[test]
fn merge_weighted_per_task_dare_keeps_weights() {
    let base = tuned(&xor_net(), vec![set(0, 0, 0, Trit::Zero)]);
    let pos = tuned(&base, vec![set(0, 0, 0, Trit::Pos)]);
    let neg = tuned(&base, vec![set(0, 0, 0, Trit::Neg)]);
    let tasks = [
        TaskVector::between(&base, &pos).expect("pos"),
        TaskVector::between(&base, &neg).expect("neg"),
    ];
    let mut checked = false;
    for seed in 0..128 {
        let plain = merge_tasks_per_task_dare(&base, &tasks, 0.5, seed).expect("plain");
        if plain.layer(0).expect("l0").trit(0, 0) != Some(Trit::Zero) {
            continue;
        }
        let weighted = merge_tasks_weighted_per_task_dare(&base, &tasks, &[1.0, 9.0], 0.5, seed)
            .expect("weighted");
        assert_eq!(
            weighted.layer(0).expect("l0").trit(0, 0),
            Some(Trit::Neg),
            "per-task 稀疏化后仍必须按权重投票（seed {seed}）"
        );
        checked = true;
        break;
    }
    assert!(checked, "需要一个两票都存活的 seed");
}

#[test]
fn cli_task_merge_weights() {
    let dir = temp_dir("cli-weights");
    let base = full_net(-1);
    let tuned = full_net(1);
    let base_path = dir.join("base.wsp");
    let tuned_path = dir.join("tuned.wsp");
    let out_path = dir.join("out.wsp");
    fs::write(&base_path, to_wsp(&base)).expect("write base");
    fs::write(&tuned_path, to_wsp(&tuned)).expect("write tuned");
    let output = stdout_json(&run(&[
        "task-merge",
        &path_string(&base_path),
        &path_string(&tuned_path),
        &path_string(&tuned_path),
        "--weight",
        "0,1",
        "--out",
        &path_string(&out_path),
    ]));
    assert_eq!(output["weights"], json!([0.0, 1.0]));
    let tasks = [
        TaskVector::between(&base, &tuned).expect("first"),
        TaskVector::between(&base, &tuned).expect("second"),
    ];
    let expected = merge_tasks_weighted(&base, &tasks, &[0.0, 1.0]).expect("expected");
    let written =
        trit_wspace::from_wsp(&fs::read_to_string(&out_path).expect("read out")).expect("parse");
    assert_eq!(written.state_hash(), expected.state_hash());
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn mcp_task_merge_weights() {
    let base = full_net(-1);
    let tuned = full_net(1);
    let mut server = Mcp::new();
    let result = call(
        &mut server,
        "wsp_task_merge",
        json!({"base": to_wsp(&base), "tasks": [to_wsp(&tuned)], "weights": [2.0]}),
    );
    assert_eq!(result["isError"], false);
    let value = payload(&result);
    assert_eq!(value["weights"], json!([2.0]));
    let tasks = [TaskVector::between(&base, &tuned).expect("task")];
    let expected = merge_tasks_weighted(&base, &tasks, &[2.0]).expect("expected");
    let artifact = value["artifact"].as_str().expect("artifact");
    assert_eq!(
        trit_wspace::from_wsp(artifact).expect("parse").state_hash(),
        expected.state_hash()
    );
    let bad = call(
        &mut server,
        "wsp_task_merge",
        json!({"base": to_wsp(&base), "tasks": [to_wsp(&tuned)], "weights": "heavy"}),
    );
    assert_eq!(bad["isError"], true);
}
