//! DARE 三值丢弃：确定性选择、重标度只进 scale（行为中性）、CLI/MCP 闭环。

use std::fs;
use std::path::PathBuf;
use std::process::{Command, Output};

use serde_json::{Value, json};
use trit_wspace::mcp::Mcp;
use trit_wspace::{
    Behavior, Edit, Error, Layer, Net, Oracle, Patch, Trit, dare_drop, from_wsp, to_wsp,
};

fn flat_net(rows: usize, cols: usize, layers: usize, value: i8) -> Net {
    let mut built = Vec::new();
    let mut width = cols;
    for _ in 0..layers {
        built.push(
            Layer::new(rows, width, &vec![value; rows * width], &vec![value; rows]).expect("layer"),
        );
        width = rows;
    }
    Net::new(built).expect("net")
}

fn base_net() -> Net {
    flat_net(4, 4, 2, -1)
}

fn tuned_net() -> Net {
    flat_net(4, 4, 2, 1)
}

fn xor_net() -> Net {
    Net::new(vec![
        Layer::new(2, 2, &[-1, -1, -1, -1], &[-1, 1]).expect("l0"),
        Layer::new(1, 2, &[-1, 1], &[-1]).expect("l1"),
    ])
    .expect("xor net")
}

fn apply(net: &Net, patch: &Patch) -> Net {
    let mut work = net.clone();
    patch.apply(&mut work).expect("apply");
    work
}

fn temp_dir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("trit-wspace-dare-{tag}-{}", std::process::id()));
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
            "jsonrpc": "2.0", "id": 13, "method": "tools/call",
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
fn rate_zero_keeps_full_delta_without_rescale() {
    let base = base_net();
    let tuned = tuned_net();
    let outcome = dare_drop(&base, &tuned, 0.0, 7).expect("drop");
    assert_eq!(outcome.kept, 40);
    assert_eq!(outcome.dropped, 0);
    assert_eq!(outcome.rescaled_rows, 0);
    let full = Patch::diff(&base, &tuned).expect("diff");
    assert_eq!(outcome.patch.edits(), full.edits());
    assert_eq!(
        apply(&base, &outcome.patch).state_hash(),
        tuned.state_hash()
    );
}

#[test]
fn invalid_rate_is_error() {
    let base = base_net();
    let tuned = tuned_net();
    for rate in [1.0_f32, -0.1, f32::INFINITY] {
        assert_eq!(
            dare_drop(&base, &tuned, rate, 0),
            Err(Error::InvalidRate { rate })
        );
    }
    assert!(matches!(
        dare_drop(&base, &tuned, f32::NAN, 0),
        Err(Error::InvalidRate { .. })
    ));
}

#[test]
fn arch_mismatch_is_error() {
    let base = base_net();
    let other = xor_net();
    assert_eq!(dare_drop(&base, &other, 0.5, 0), Err(Error::ArchMismatch));
}

#[test]
fn identical_nets_is_empty() {
    let net = base_net();
    let outcome = dare_drop(&net, &net, 0.5, 7).expect("drop");
    assert!(outcome.patch.is_empty());
    assert_eq!(outcome.kept, 0);
    assert_eq!(outcome.dropped, 0);
    assert_eq!(outcome.rescaled_rows, 0);
}

#[test]
fn same_seed_is_deterministic() {
    let base = base_net();
    let tuned = tuned_net();
    let first = dare_drop(&base, &tuned, 0.5, 7).expect("drop");
    let second = dare_drop(&base, &tuned, 0.5, 7).expect("drop");
    assert_eq!(first, second);
}

#[test]
fn different_seed_changes_selection() {
    let base = base_net();
    let tuned = tuned_net();
    let first = dare_drop(&base, &tuned, 0.5, 7).expect("drop");
    let second = dare_drop(&base, &tuned, 0.5, 8).expect("drop");
    assert_ne!(first.patch.edits(), second.patch.edits());
}

#[test]
fn drop_selection_is_pinned_for_seed_seven() {
    let base = base_net();
    let tuned = tuned_net();
    let outcome = dare_drop(&base, &tuned, 0.5, 7).expect("drop");
    assert_eq!(outcome.kept, 21);
    assert_eq!(outcome.dropped, 19);
    assert_eq!(outcome.rescaled_rows, 8);
    let mut kept_coords = Vec::new();
    for edit in outcome.patch.edits() {
        match edit {
            Edit::SetTrit {
                layer,
                row,
                col,
                to,
            } => {
                assert_eq!(*to, Trit::Pos);
                kept_coords.push((*layer, *row, *col));
            }
            Edit::SetBias { layer, row, to } => {
                assert_eq!(*to, Trit::Pos);
                kept_coords.push((*layer, *row, usize::MAX));
            }
            Edit::SetScale { .. } => {}
            other => panic!("意外编辑 {other:?}"),
        }
    }
    assert_eq!(
        kept_coords,
        vec![
            (0, 0, 2),
            (0, 0, usize::MAX),
            (0, 1, 1),
            (0, 1, 2),
            (0, 1, 3),
            (0, 2, 3),
            (0, 2, usize::MAX),
            (0, 3, 0),
            (0, 3, 1),
            (0, 3, 2),
            (1, 0, 0),
            (1, 0, 1),
            (1, 0, 3),
            (1, 0, usize::MAX),
            (1, 1, 2),
            (1, 1, usize::MAX),
            (1, 2, 2),
            (1, 2, 3),
            (1, 3, 2),
            (1, 3, 3),
            (1, 3, usize::MAX),
        ]
    );
    let work = apply(&base, &outcome.patch);
    let layer = work.layer(0).expect("l0");
    assert_eq!(layer.trit(0, 0), Some(Trit::Neg));
    assert_eq!(layer.trit(0, 2), Some(Trit::Pos));
    assert_eq!(layer.bias_trit(0), Some(Trit::Pos));
    assert_eq!(layer.bias_trit(1), Some(Trit::Neg));
}

#[test]
fn rescale_only_rows_with_kept_deltas() {
    let base = base_net();
    let tuned = tuned_net();
    let outcome = dare_drop(&base, &tuned, 0.5, 18).expect("drop");
    assert_eq!(outcome.kept, 23);
    assert_eq!(outcome.dropped, 17);
    assert_eq!(outcome.rescaled_rows, 7);
    let work = apply(&base, &outcome.patch);
    let layer = work.layer(0).expect("l0");
    for row in 0..3 {
        assert_eq!(layer.scale(row), Some(2.0), "row {row}");
    }
    assert_eq!(layer.scale(3), Some(1.0));
    let layer1 = work.layer(1).expect("l1");
    for row in 0..4 {
        assert_eq!(layer1.scale(row), Some(2.0), "l1 row {row}");
    }
    assert!(outcome.patch.edits().iter().all(|edit| !matches!(
        edit,
        Edit::SetScale {
            layer: 0,
            row: 3,
            ..
        }
    )));
}

#[test]
fn scale_rescale_is_behavior_neutral() {
    let base = xor_net();
    let mut tuned = base.clone();
    Patch::new(vec![
        Edit::SetTrit {
            layer: 0,
            row: 0,
            col: 0,
            to: Trit::Zero,
        },
        Edit::SetBias {
            layer: 1,
            row: 0,
            to: Trit::Pos,
        },
    ])
    .apply(&mut tuned)
    .expect("tune");
    let oracle = Oracle::boolean(2, |b| b[0] != b[1]).expect("oracle");
    let outcome = dare_drop(&base, &tuned, 0.5, 0).expect("drop");
    assert_eq!(outcome.kept, 2);
    assert_eq!(outcome.rescaled_rows, 2);
    let work = apply(&base, &outcome.patch);
    let bare_edits: Vec<Edit> = outcome
        .patch
        .edits()
        .iter()
        .filter(|edit| !matches!(edit, Edit::SetScale { .. }))
        .cloned()
        .collect();
    let bare = apply(&base, &Patch::new(bare_edits));
    assert_eq!(
        Behavior::of(&work, &oracle).expect("behavior").signature(),
        Behavior::of(&bare, &oracle).expect("behavior").signature()
    );
    assert_eq!(work.layer(0).expect("l0").scale(0), Some(2.0));
    assert_eq!(bare.layer(0).expect("l0").scale(0), Some(1.0));
}

#[test]
fn output_layer_deltas_are_dropped_too() {
    let base = Net::new(vec![
        Layer::new(2, 2, &[-1, -1, -1, -1], &[-1, -1]).expect("l0"),
        Layer::new(1, 2, &[-1, -1], &[-1]).expect("l1"),
    ])
    .expect("base");
    let tuned = Net::new(vec![
        Layer::new(2, 2, &[-1, -1, -1, -1], &[-1, -1]).expect("l0"),
        Layer::new(1, 2, &[1, 1], &[1]).expect("l1"),
    ])
    .expect("tuned");
    let outcome = dare_drop(&base, &tuned, 0.5, 0).expect("drop");
    assert_eq!(outcome.kept, 1);
    assert_eq!(outcome.dropped, 2);
    assert_eq!(outcome.rescaled_rows, 1);
    let work = apply(&base, &outcome.patch);
    assert_eq!(
        work.layer(0).expect("l0").row(0),
        base.layer(0).expect("l0").row(0)
    );
    let out = work.layer(1).expect("l1");
    assert_eq!(out.trit(0, 0), Some(Trit::Neg));
    assert_eq!(out.trit(0, 1), Some(Trit::Neg));
    assert_eq!(out.bias_trit(0), Some(Trit::Pos));
    assert_eq!(out.scale(0), Some(2.0));
}

#[test]
fn cli_dare_drop_writes_artifact_and_verifies() {
    let dir = temp_dir("cli");
    let base = xor_net();
    let mut tuned = base.clone();
    Patch::new(vec![Edit::SetTrit {
        layer: 0,
        row: 0,
        col: 0,
        to: Trit::Zero,
    }])
    .apply(&mut tuned)
    .expect("tune");
    let base_path = dir.join("base.wsp");
    let tuned_path = dir.join("tuned.wsp");
    let out_path = dir.join("out.wsp");
    let oracle_path = dir.join("oracle.json");
    fs::write(&base_path, to_wsp(&base)).expect("write base");
    fs::write(&tuned_path, to_wsp(&tuned)).expect("write tuned");
    fs::write(
        &oracle_path,
        json!({"domain": "boolean", "vars": 2, "table": "0110"}).to_string(),
    )
    .expect("write oracle");
    let output = stdout_json(&run(&[
        "dare-drop",
        &path_string(&base_path),
        "--tuned",
        &path_string(&tuned_path),
        "--rate",
        "0",
        "--oracle",
        &path_string(&oracle_path),
        "--require-before",
        "--out",
        &path_string(&out_path),
    ]));
    assert_eq!(output["applied"], json!(true));
    assert_eq!(output["rolled_back"], json!(false));
    assert_eq!(output["kept"], json!(1));
    assert_eq!(output["dropped"], json!(0));
    assert_eq!(output["seed"], json!(0));
    assert_eq!(output["verify"]["exact"], json!(false));
    let written = from_wsp(&fs::read_to_string(&out_path).expect("read out")).expect("parse");
    assert_eq!(written.state_hash(), tuned.state_hash());
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn cli_dare_drop_bad_rate_fails() {
    let dir = temp_dir("bad-rate");
    let base_path = dir.join("base.wsp");
    let tuned_path = dir.join("tuned.wsp");
    fs::write(&base_path, to_wsp(&base_net())).expect("write base");
    fs::write(&tuned_path, to_wsp(&tuned_net())).expect("write tuned");
    for (rate, needle) in [("1", "丢弃率"), ("abc", "需要数字")] {
        let output = run(&[
            "dare-drop",
            &path_string(&base_path),
            "--tuned",
            &path_string(&tuned_path),
            "--rate",
            rate,
        ]);
        assert!(!output.status.success(), "rate={rate} 应当失败");
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(stderr.contains(needle), "stderr: {stderr}");
    }
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn mcp_dare_drop_round_trip() {
    let mut server = Mcp::new();
    let result = call(
        &mut server,
        "wsp_dare_drop",
        json!({
            "artifact": to_wsp(&base_net()),
            "tuned": to_wsp(&tuned_net()),
            "rate": 0.5,
            "seed": 18,
        }),
    );
    assert_eq!(result["isError"], false);
    let value = payload(&result);
    assert_eq!(value["kept"], json!(23));
    assert_eq!(value["dropped"], json!(17));
    assert_eq!(value["rescaled_rows"], json!(7));
    assert_eq!(value["applied"], json!(true));
    let expected = dare_drop(&base_net(), &tuned_net(), 0.5, 18).expect("drop");
    let work = apply(&base_net(), &expected.patch);
    let artifact = value["artifact"].as_str().expect("artifact");
    assert_eq!(
        from_wsp(artifact).expect("parse").state_hash(),
        work.state_hash()
    );
}

#[test]
fn mcp_dare_drop_missing_rate_is_error() {
    let mut server = Mcp::new();
    let result = call(
        &mut server,
        "wsp_dare_drop",
        json!({"artifact": to_wsp(&base_net()), "tuned": to_wsp(&tuned_net())}),
    );
    assert_eq!(result["isError"], true);
    let text = result["content"][0]["text"].as_str().expect("text");
    assert!(text.contains("rate"), "text: {text}");
}
