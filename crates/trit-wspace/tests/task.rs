//! 三值任务向量：差分、套用、多任务投票合并。

use std::fs;
use std::path::PathBuf;
use std::process::{Command, Output};

use serde_json::{Value, json};
use trit_wspace::mcp::Mcp;
use trit_wspace::{
    Edit, Layer, Net, Patch, TaskVector, Trit, checksum, dare_drop, merge_tasks, merge_tasks_dare,
    merge_tasks_per_task_dare, to_wsp,
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

#[test]
fn vector_round_trip_and_base_guard() {
    let base = xor_net();
    let changed = tuned(&base, vec![set(0, 0, 0, Trit::Zero)]);
    let vector = TaskVector::between(&base, &changed).expect("vector");
    assert_eq!(vector.base(), checksum(&base));
    assert_eq!(vector.edits().len(), 1);
    assert_eq!(vector.apply(&base).expect("apply"), changed);
    assert!(vector.apply(&changed).is_err(), "基准不一致应拒绝");
}

#[test]
fn merge_keeps_disjoint_tasks() {
    let base = xor_net();
    let left = tuned(&base, vec![set(0, 0, 0, Trit::Zero)]);
    let right = tuned(&base, vec![set(0, 0, 1, Trit::Zero)]);
    let tasks = [
        TaskVector::between(&base, &left).expect("left"),
        TaskVector::between(&base, &right).expect("right"),
    ];
    let merged = merge_tasks(&base, &tasks).expect("merge");
    let layer = merged.layer(0).expect("layer");
    assert_eq!(layer.trit(0, 0), Some(Trit::Zero));
    assert_eq!(layer.trit(0, 1), Some(Trit::Zero));
    assert_eq!(merged.layer(1), base.layer(1), "未投票的行保持原样");
}

#[test]
fn merge_votes_targets_and_breaks_ties_to_base() {
    let base = xor_net();
    let zero_a = tuned(&base, vec![set(0, 0, 0, Trit::Zero)]);
    let zero_b = tuned(&base, vec![set(0, 0, 0, Trit::Zero)]);
    let tasks = [
        TaskVector::between(&base, &zero_a).expect("a"),
        TaskVector::between(&base, &zero_b).expect("b"),
    ];
    let merged = merge_tasks(&base, &tasks).expect("merge zeros");
    assert_eq!(merged.layer(0).expect("l0").trit(0, 0), Some(Trit::Zero));

    let zero_base = tuned(&base, vec![set(0, 0, 0, Trit::Zero)]);
    let pos = tuned(&zero_base, vec![set(0, 0, 0, Trit::Pos)]);
    let neg = tuned(&zero_base, vec![set(0, 0, 0, Trit::Neg)]);
    let tasks = [
        TaskVector::between(&zero_base, &pos).expect("pos"),
        TaskVector::between(&zero_base, &neg).expect("neg"),
    ];
    let merged = merge_tasks(&zero_base, &tasks).expect("merge tie");
    assert_eq!(
        merged.layer(0).expect("l0").trit(0, 0),
        Some(Trit::Zero),
        "平票保留基准"
    );

    let tasks = [
        TaskVector::between(&zero_base, &pos).expect("pos"),
        TaskVector::between(&zero_base, &pos).expect("pos2"),
        TaskVector::between(&zero_base, &neg).expect("neg"),
    ];
    let merged = merge_tasks(&zero_base, &tasks).expect("merge majority");
    assert_eq!(merged.layer(0).expect("l0").trit(0, 0), Some(Trit::Pos));
}

#[test]
fn aligned_vector_ignores_hidden_permutation() {
    let base = xor_net();
    let permuted = base.permute_rows(0, &[1, 0]).expect("permute");
    let naive = TaskVector::between(&base, &permuted).expect("naive");
    assert!(!naive.edits().is_empty(), "未对齐会看到大量假差分");
    let aligned = TaskVector::between_aligned(&base, &permuted).expect("aligned");
    assert!(aligned.edits().is_empty(), "对齐后同一行为应无差分");

    let mut edited = permuted.clone();
    Patch::new(vec![set(0, 0, 0, Trit::Zero)])
        .apply(&mut edited)
        .expect("edit");
    let vector = TaskVector::between_aligned(&base, &edited).expect("aligned edit");
    assert_eq!(vector.edits().len(), 1);
    assert_eq!(
        vector.apply(&base).expect("apply"),
        trit_wspace::merge::align(&base, &edited).expect("align")
    );
}

#[test]
fn merge_rejects_foreign_base() {
    let base = xor_net();
    let other = tuned(&base, vec![set(0, 0, 0, Trit::Zero)]);
    let vector = TaskVector::between(&base, &other).expect("vector");
    assert!(merge_tasks(&other, std::slice::from_ref(&vector)).is_err());
}

#[test]
fn task_vector_requires_same_arch() {
    let base = xor_net();
    let small = Net::new(vec![Layer::new(1, 2, &[1, 1], &[0]).expect("l0")]).expect("net");
    assert!(TaskVector::between(&base, &small).is_err());
}

#[test]
fn merge_without_tasks_is_identity() {
    let base = xor_net();
    assert_eq!(merge_tasks(&base, &[]).expect("merge"), base);
}

fn full_net(value: i8) -> Net {
    let mut built = Vec::new();
    for _ in 0..2 {
        built.push(Layer::new(4, 4, &[value; 16], &[value; 4]).expect("full layer"));
    }
    Net::new(built).expect("full net")
}

fn temp_dir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("trit-wspace-task-{tag}-{}", std::process::id()));
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
fn merge_dare_rate_zero_matches_plain_merge() {
    let base = xor_net();
    let left = tuned(&base, vec![set(0, 0, 0, Trit::Zero)]);
    let right = tuned(&base, vec![set(0, 0, 1, Trit::Zero)]);
    let tasks = [
        TaskVector::between(&base, &left).expect("left"),
        TaskVector::between(&base, &right).expect("right"),
    ];
    let plain = merge_tasks(&base, &tasks).expect("plain merge");
    let dare = merge_tasks_dare(&base, &tasks, 0.0, 7).expect("dare merge");
    assert_eq!(dare, plain);
}

#[test]
fn merge_dare_sparsifies_after_vote() {
    let base = full_net(-1);
    let tuned = full_net(1);
    let tasks = [TaskVector::between(&base, &tuned).expect("task")];
    let plain = merge_tasks(&base, &tasks).expect("plain merge");
    assert_eq!(plain, tuned);
    let outcome = dare_drop(&base, &plain, 0.5, 7).expect("drop");
    let mut expected = base.clone();
    outcome.patch.apply(&mut expected).expect("apply drop");
    let got = merge_tasks_dare(&base, &tasks, 0.5, 7).expect("dare merge");
    assert_eq!(got, expected);
    assert_ne!(got, plain, "稀疏化必须改变结果");
}

#[test]
fn merge_dare_seed_changes_selection() {
    let base = full_net(-1);
    let tuned = full_net(1);
    let tasks = [TaskVector::between(&base, &tuned).expect("task")];
    let a = merge_tasks_dare(&base, &tasks, 0.5, 7).expect("seed 7");
    let b = merge_tasks_dare(&base, &tasks, 0.5, 8).expect("seed 8");
    assert_ne!(a, b);
}

#[test]
fn merge_dare_pinned_selection() {
    let base = full_net(-1);
    let tuned = full_net(1);
    let tasks = [TaskVector::between(&base, &tuned).expect("task")];
    let outcome = dare_drop(&base, &tuned, 0.5, 7).expect("drop");
    assert_eq!(outcome.kept, 21);
    assert_eq!(outcome.dropped, 19);
    assert_eq!(outcome.rescaled_rows, 8);
    let mut expected = base.clone();
    outcome.patch.apply(&mut expected).expect("apply drop");
    assert_eq!(
        merge_tasks_dare(&base, &tasks, 0.5, 7).expect("dare merge"),
        expected
    );
}

#[test]
fn merge_dare_rejects_invalid_rate() {
    let base = full_net(-1);
    let tuned = full_net(1);
    let tasks = [TaskVector::between(&base, &tuned).expect("task")];
    assert!(merge_tasks_dare(&base, &tasks, 1.0, 0).is_err());
    assert!(merge_tasks_dare(&base, &tasks, -0.5, 0).is_err());
    assert!(merge_tasks_dare(&base, &tasks, f32::NAN, 0).is_err());
}

#[test]
fn merge_dare_multi_task_matches_vote_then_drop() {
    let base = full_net(-1);
    let tuned = full_net(1);
    let half = base.clone();
    let tasks = [
        TaskVector::between(&base, &tuned).expect("full"),
        TaskVector::between(&base, &half).expect("half"),
    ];
    let voted = merge_tasks(&base, &tasks).expect("vote");
    let outcome = dare_drop(&base, &voted, 0.5, 11).expect("drop");
    let mut expected = base.clone();
    outcome.patch.apply(&mut expected).expect("apply drop");
    assert_eq!(
        merge_tasks_dare(&base, &tasks, 0.5, 11).expect("dare merge"),
        expected
    );
}

#[test]
fn cli_task_merge_dare_writes_artifact() {
    let dir = temp_dir("cli");
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
        "--dare-rate",
        "0.5",
        "--dare-seed",
        "7",
        "--out",
        &path_string(&out_path),
    ]));
    assert_eq!(output["dare_rate"], json!(0.5));
    assert_eq!(output["dare_seed"], json!(7));
    let tasks = [TaskVector::between(&base, &tuned).expect("task")];
    let expected = merge_tasks_dare(&base, &tasks, 0.5, 7).expect("expected");
    let written =
        trit_wspace::from_wsp(&fs::read_to_string(&out_path).expect("read out")).expect("parse");
    assert_eq!(written.state_hash(), expected.state_hash());
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn cli_task_merge_without_dare_is_unchanged() {
    let dir = temp_dir("cli-plain");
    let base = full_net(-1);
    let tuned = full_net(1);
    let base_path = dir.join("base.wsp");
    let tuned_path = dir.join("tuned.wsp");
    fs::write(&base_path, to_wsp(&base)).expect("write base");
    fs::write(&tuned_path, to_wsp(&tuned)).expect("write tuned");
    let output = stdout_json(&run(&[
        "task-merge",
        &path_string(&base_path),
        &path_string(&tuned_path),
    ]));
    assert!(output["dare_rate"].is_null());
    let tasks = [TaskVector::between(&base, &tuned).expect("task")];
    let expected = merge_tasks(&base, &tasks).expect("plain");
    let artifact = output["artifact"].as_str().expect("artifact");
    assert_eq!(
        trit_wspace::from_wsp(artifact).expect("parse").state_hash(),
        expected.state_hash()
    );
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn mcp_task_merge_dare_round_trip() {
    let base = full_net(-1);
    let tuned = full_net(1);
    let mut server = Mcp::new();
    let result = call(
        &mut server,
        "wsp_task_merge",
        json!({
            "base": to_wsp(&base),
            "tasks": [to_wsp(&tuned)],
            "dare_rate": 0.5,
            "dare_seed": 7,
        }),
    );
    assert_eq!(result["isError"], false);
    let value = payload(&result);
    assert_eq!(value["dare_rate"], json!(0.5));
    assert_eq!(value["dare_seed"], json!(7));
    let tasks = [TaskVector::between(&base, &tuned).expect("task")];
    let expected = merge_tasks_dare(&base, &tasks, 0.5, 7).expect("expected");
    let artifact = value["artifact"].as_str().expect("artifact");
    assert_eq!(
        trit_wspace::from_wsp(artifact).expect("parse").state_hash(),
        expected.state_hash()
    );
}

#[test]
fn mcp_task_merge_without_dare_is_unchanged() {
    let base = full_net(-1);
    let tuned = full_net(1);
    let mut server = Mcp::new();
    let result = call(
        &mut server,
        "wsp_task_merge",
        json!({"base": to_wsp(&base), "tasks": [to_wsp(&tuned)]}),
    );
    assert_eq!(result["isError"], false);
    let value = payload(&result);
    assert!(value["dare_rate"].is_null());
    let tasks = [TaskVector::between(&base, &tuned).expect("task")];
    let expected = merge_tasks(&base, &tasks).expect("plain");
    let artifact = value["artifact"].as_str().expect("artifact");
    assert_eq!(
        trit_wspace::from_wsp(artifact).expect("parse").state_hash(),
        expected.state_hash()
    );
}

#[test]
fn merge_per_task_rate_zero_matches_plain_merge() {
    let base = xor_net();
    let left = tuned(&base, vec![set(0, 0, 0, Trit::Zero)]);
    let right = tuned(&base, vec![set(0, 0, 1, Trit::Zero)]);
    let tasks = [
        TaskVector::between(&base, &left).expect("left"),
        TaskVector::between(&base, &right).expect("right"),
    ];
    let plain = merge_tasks(&base, &tasks).expect("plain merge");
    let per = merge_tasks_per_task_dare(&base, &tasks, 0.0, 7).expect("per task");
    assert_eq!(per, plain);
}

#[test]
fn merge_per_task_drops_votes_before_voting() {
    let base = xor_net();
    let pos = tuned(&base, vec![set(0, 0, 0, Trit::Pos)]);
    let zero = tuned(&base, vec![set(0, 0, 0, Trit::Zero)]);
    let tasks = [
        TaskVector::between(&base, &pos).expect("pos"),
        TaskVector::between(&base, &zero).expect("zero"),
    ];
    let plain = merge_tasks(&base, &tasks).expect("plain");
    assert_eq!(
        plain.layer(0).expect("l0").trit(0, 0),
        Some(Trit::Pos),
        "+1 与 0 投票应取 +1"
    );
    let mut differed = false;
    for seed in 0..64 {
        let per = merge_tasks_per_task_dare(&base, &tasks, 0.5, seed).expect("per");
        if per != plain {
            differed = true;
            break;
        }
    }
    assert!(differed, "per-task 丢票必须能改变投票结果");
}

#[test]
fn merge_per_task_seed_changes_selection() {
    let base = full_net(-1);
    let tuned = full_net(1);
    let tasks = [TaskVector::between(&base, &tuned).expect("task")];
    let a = merge_tasks_per_task_dare(&base, &tasks, 0.5, 7).expect("seed 7");
    let b = merge_tasks_per_task_dare(&base, &tasks, 0.5, 8).expect("seed 8");
    assert_ne!(a, b);
}

#[test]
fn merge_per_task_rescales_changed_rows() {
    let base = full_net(-1);
    let tuned = full_net(1);
    let tasks = [TaskVector::between(&base, &tuned).expect("task")];
    let out = merge_tasks_per_task_dare(&base, &tasks, 0.5, 7).expect("per task");
    let mut changed = 0;
    for layer in 0..2 {
        let b = base.layer(layer).expect("base layer");
        let o = out.layer(layer).expect("out layer");
        for row in 0..b.rows() {
            let diff = (0..b.cols()).any(|col| b.trit(row, col) != o.trit(row, col))
                || b.bias_trit(row) != o.bias_trit(row);
            if diff {
                assert_eq!(o.scale(row), Some(2.0), "有差异的行必须重标度");
                changed += 1;
            }
        }
    }
    assert!(changed > 0);
}

#[test]
fn merge_per_task_rejects_invalid_rate() {
    let base = full_net(-1);
    let tuned = full_net(1);
    let tasks = [TaskVector::between(&base, &tuned).expect("task")];
    assert!(merge_tasks_per_task_dare(&base, &tasks, 1.0, 0).is_err());
    assert!(merge_tasks_per_task_dare(&base, &tasks, -0.5, 0).is_err());
    assert!(merge_tasks_per_task_dare(&base, &tasks, f32::NAN, 0).is_err());
}

#[test]
fn cli_task_merge_per_task_dare() {
    let dir = temp_dir("cli-per-task");
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
        "--dare-per-task",
        "0.5",
        "--dare-seed",
        "7",
        "--out",
        &path_string(&out_path),
    ]));
    assert!(output["dare_rate"].is_null());
    assert_eq!(output["dare_per_task"], json!(0.5));
    assert_eq!(output["dare_seed"], json!(7));
    let tasks = [TaskVector::between(&base, &tuned).expect("task")];
    let expected = merge_tasks_per_task_dare(&base, &tasks, 0.5, 7).expect("expected");
    let written =
        trit_wspace::from_wsp(&fs::read_to_string(&out_path).expect("read out")).expect("parse");
    assert_eq!(written.state_hash(), expected.state_hash());
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn cli_task_merge_rejects_both_dare_modes() {
    let dir = temp_dir("cli-both");
    let base_path = dir.join("base.wsp");
    let tuned_path = dir.join("tuned.wsp");
    fs::write(&base_path, to_wsp(&full_net(-1))).expect("write base");
    fs::write(&tuned_path, to_wsp(&full_net(1))).expect("write tuned");
    let output = run(&[
        "task-merge",
        &path_string(&base_path),
        &path_string(&tuned_path),
        "--dare-rate",
        "0.5",
        "--dare-per-task",
        "0.5",
    ]);
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("只能给一个"), "stderr: {stderr}");
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn mcp_task_merge_per_task_dare() {
    let base = full_net(-1);
    let tuned = full_net(1);
    let mut server = Mcp::new();
    let result = call(
        &mut server,
        "wsp_task_merge",
        json!({
            "base": to_wsp(&base),
            "tasks": [to_wsp(&tuned)],
            "dare_per_task": 0.5,
            "dare_seed": 7,
        }),
    );
    assert_eq!(result["isError"], false);
    let value = payload(&result);
    assert!(value["dare_rate"].is_null());
    assert_eq!(value["dare_per_task"], json!(0.5));
    let tasks = [TaskVector::between(&base, &tuned).expect("task")];
    let expected = merge_tasks_per_task_dare(&base, &tasks, 0.5, 7).expect("expected");
    let artifact = value["artifact"].as_str().expect("artifact");
    assert_eq!(
        trit_wspace::from_wsp(artifact).expect("parse").state_hash(),
        expected.state_hash()
    );
}

#[test]
fn merge_per_task_seeds_differ_per_task() {
    let base = tuned(&xor_net(), vec![set(0, 0, 0, Trit::Zero)]);
    let pos = tuned(&base, vec![set(0, 0, 0, Trit::Pos)]);
    let neg = tuned(&base, vec![set(0, 0, 0, Trit::Neg)]);
    let tasks = [
        TaskVector::between(&base, &pos).expect("pos"),
        TaskVector::between(&base, &neg).expect("neg"),
    ];
    let mut saw_asymmetry = false;
    for seed in 0..64 {
        let out = merge_tasks_per_task_dare(&base, &tasks, 0.5, seed).expect("per");
        let value = out.layer(0).expect("l0").trit(0, 0).expect("in range");
        if value != Trit::Zero {
            saw_asymmetry = true;
            break;
        }
    }
    assert!(
        saw_asymmetry,
        "不同任务必须用独立子种子（否则同坐标的票同步丢弃，结果恒为基准）"
    );
}
