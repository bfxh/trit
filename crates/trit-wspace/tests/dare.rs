//! DARE 三值丢弃：确定性选择、重标度只进 scale（行为中性）、CLI/MCP 闭环。

use std::fs;
use std::path::PathBuf;
use std::process::{Command, Output};

use serde_json::{Value, json};
use std::collections::BTreeSet;
use trit_wspace::mcp::Mcp;

use trit_wspace::{
    Behavior, Edit, Error, Layer, Net, Oracle, Patch, Trit, dare_drop, dare_drop_blocks,
    dare_drop_rows, from_wsp, to_wsp,
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

#[test]
fn row_drop_keeps_or_drops_whole_rows() {
    let base = base_net();
    let tuned = tuned_net();
    let outcome = dare_drop_rows(&base, &tuned, 0.5, 7).expect("drop rows");
    assert_eq!(outcome.kept, 25);
    assert_eq!(outcome.dropped, 15);
    assert_eq!(outcome.rescaled_rows, 5);
    let mut per_row: std::collections::BTreeMap<(usize, usize), usize> =
        std::collections::BTreeMap::new();
    for edit in outcome.patch.edits() {
        match edit {
            Edit::SetTrit { layer, row, .. } | Edit::SetBias { layer, row, .. } => {
                let key = (*layer, *row);
                *per_row.entry(key).or_default() += 1;
            }
            Edit::SetScale { .. } => {}
            other => panic!("意外编辑 {other:?}"),
        }
    }
    let kept_rows: Vec<(usize, usize)> = per_row.keys().copied().collect();
    assert_eq!(kept_rows, vec![(0, 0), (0, 2), (1, 0), (1, 2), (1, 3)]);
    assert!(
        per_row.values().all(|count| *count == 5),
        "每行必须整行保留（4 trit + 1 bias）：{per_row:?}"
    );
}

#[test]
fn row_drop_applies_whole_row_deltas() {
    let base = base_net();
    let tuned = tuned_net();
    let outcome = dare_drop_rows(&base, &tuned, 0.5, 7).expect("drop rows");
    let work = apply(&base, &outcome.patch);
    let kept = [(0usize, 0usize), (0, 2), (1, 0), (1, 2), (1, 3)];
    for layer in 0..2 {
        let b = base.layer(layer).expect("base layer");
        let t = tuned.layer(layer).expect("tuned layer");
        let w = work.layer(layer).expect("work layer");
        for row in 0..b.rows() {
            if kept.contains(&(layer, row)) {
                assert_eq!(w.row(row), t.row(row), "kept row ({layer},{row})");
                assert_eq!(w.bias_trit(row), t.bias_trit(row));
            } else {
                assert_eq!(w.row(row), b.row(row), "dropped row ({layer},{row})");
                assert_eq!(w.bias_trit(row), b.bias_trit(row));
            }
        }
    }
}

#[test]
fn row_drop_selection_is_pinned_and_seed_sensitive() {
    let base = base_net();
    let tuned = tuned_net();
    let first = dare_drop_rows(&base, &tuned, 0.5, 7).expect("rows");
    let again = dare_drop_rows(&base, &tuned, 0.5, 7).expect("rows");
    assert_eq!(first, again);
    let other = dare_drop_rows(&base, &tuned, 0.5, 8).expect("rows");
    assert_ne!(first.patch.edits(), other.patch.edits());
}

#[test]
fn row_drop_differs_from_coordinate_drop() {
    let base = base_net();
    let tuned = tuned_net();
    let rows = dare_drop_rows(&base, &tuned, 0.5, 7).expect("rows");
    let coords = dare_drop(&base, &tuned, 0.5, 7).expect("coords");
    assert_ne!(rows.patch.edits(), coords.patch.edits());
    assert_ne!(rows.kept, coords.kept);
}

#[test]
fn row_drop_rate_zero_is_full_delta() {
    let base = base_net();
    let tuned = tuned_net();
    let outcome = dare_drop_rows(&base, &tuned, 0.0, 7).expect("rows");
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
fn row_drop_rejects_bad_rate_and_arch() {
    let base = base_net();
    let tuned = tuned_net();
    for rate in [1.0_f32, -0.1, f32::INFINITY] {
        assert_eq!(
            dare_drop_rows(&base, &tuned, rate, 0),
            Err(Error::InvalidRate { rate })
        );
    }
    assert!(matches!(
        dare_drop_rows(&base, &tuned, f32::NAN, 0),
        Err(Error::InvalidRate { .. })
    ));
    assert_eq!(
        dare_drop_rows(&base, &xor_net(), 0.5, 0),
        Err(Error::ArchMismatch)
    );
}

#[test]
fn cli_dare_drop_by_row_reports_and_applies() {
    let dir = temp_dir("cli-row");
    let base = base_net();
    let tuned = tuned_net();
    let base_path = dir.join("base.wsp");
    let tuned_path = dir.join("tuned.wsp");
    let out_path = dir.join("out.wsp");
    fs::write(&base_path, to_wsp(&base)).expect("write base");
    fs::write(&tuned_path, to_wsp(&tuned)).expect("write tuned");
    let output = stdout_json(&run(&[
        "dare-drop",
        &path_string(&base_path),
        "--tuned",
        &path_string(&tuned_path),
        "--rate",
        "0.5",
        "--seed",
        "7",
        "--by-row",
        "--out",
        &path_string(&out_path),
    ]));
    assert_eq!(output["by_row"], json!(true));
    assert_eq!(output["kept"], json!(25));
    assert_eq!(output["dropped"], json!(15));
    assert_eq!(output["rescaled_rows"], json!(5));
    let expected = dare_drop_rows(&base, &tuned, 0.5, 7).expect("rows");
    let work = apply(&base, &expected.patch);
    let written = from_wsp(&fs::read_to_string(&out_path).expect("read out")).expect("parse");
    assert_eq!(written.state_hash(), work.state_hash());
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn mcp_dare_drop_by_row_round_trip() {
    let mut server = Mcp::new();
    let result = call(
        &mut server,
        "wsp_dare_drop",
        json!({
            "artifact": to_wsp(&base_net()),
            "tuned": to_wsp(&tuned_net()),
            "rate": 0.5,
            "seed": 7,
            "by_row": true,
        }),
    );
    assert_eq!(result["isError"], false);
    let value = payload(&result);
    assert_eq!(value["by_row"], json!(true));
    assert_eq!(value["kept"], json!(25));
    assert_eq!(value["dropped"], json!(15));
    assert_eq!(value["rescaled_rows"], json!(5));
    let expected = dare_drop_rows(&base_net(), &tuned_net(), 0.5, 7).expect("rows");
    let work = apply(&base_net(), &expected.patch);
    let artifact = value["artifact"].as_str().expect("artifact");
    assert_eq!(
        from_wsp(artifact).expect("parse").state_hash(),
        work.state_hash()
    );
}

#[test]
fn block_drop_keeps_blocks_whole() {
    let base = base_net();
    let tuned = tuned_net();
    let outcome = dare_drop_blocks(&base, &tuned, 0.5, 7, 2).expect("blocks");
    let mut present: BTreeSet<(usize, usize, usize)> = BTreeSet::new();
    for edit in outcome.patch.edits() {
        if let Edit::SetTrit {
            layer, row, col, ..
        } = edit
        {
            present.insert((*layer, *row, *col));
        }
    }
    for layer in 0..2 {
        for row in 0..4 {
            for block in 0..2 {
                let first = present.contains(&(layer, row, block * 2));
                for offset in 0..2 {
                    let col = block * 2 + offset;
                    assert_eq!(
                        present.contains(&(layer, row, col)),
                        first,
                        "块 ({layer},{row},{block}) 必须整块保留或整块丢弃"
                    );
                }
            }
        }
    }
}

#[test]
fn block_one_matches_coord() {
    let base = base_net();
    let tuned = tuned_net();
    let coord = dare_drop(&base, &tuned, 0.5, 7).expect("coord");
    let block = dare_drop_blocks(&base, &tuned, 0.5, 7, 1).expect("block");
    assert_eq!(block.kept, coord.kept);
    assert_eq!(block.dropped, coord.dropped);
    assert_eq!(block.rescaled_rows, coord.rescaled_rows);
    assert_eq!(block.patch.edits(), coord.patch.edits());
}

#[test]
fn block_zero_rejected() {
    let base = base_net();
    let tuned = tuned_net();
    assert!(matches!(
        dare_drop_blocks(&base, &tuned, 0.5, 7, 0),
        Err(Error::InvalidBlock { block: 0 })
    ));
}

#[test]
fn cli_dare_drop_by_block_round_trip() {
    let dir = temp_dir("cli-block");
    let base = base_net();
    let tuned = tuned_net();
    let base_path = dir.join("base.wsp");
    let tuned_path = dir.join("tuned.wsp");
    let out_path = dir.join("out.wsp");
    fs::write(&base_path, to_wsp(&base)).expect("write base");
    fs::write(&tuned_path, to_wsp(&tuned)).expect("write tuned");
    let output = stdout_json(&run(&[
        "dare-drop",
        &path_string(&base_path),
        "--tuned",
        &path_string(&tuned_path),
        "--rate",
        "0.5",
        "--seed",
        "7",
        "--block",
        "2",
        "--out",
        &path_string(&out_path),
    ]));
    assert_eq!(output["by_row"], json!(false));
    assert_eq!(output["block"], json!(2));
    let expected = dare_drop_blocks(&base, &tuned, 0.5, 7, 2).expect("blocks");
    let work = apply(&base, &expected.patch);
    let written = from_wsp(&fs::read_to_string(&out_path).expect("read out")).expect("parse");
    assert_eq!(written.state_hash(), work.state_hash());
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn mcp_dare_drop_by_block_round_trip() {
    let mut server = Mcp::new();
    let result = call(
        &mut server,
        "wsp_dare_drop",
        json!({
            "artifact": to_wsp(&base_net()),
            "tuned": to_wsp(&tuned_net()),
            "rate": 0.5,
            "seed": 7,
            "block": 2,
        }),
    );
    assert_eq!(result["isError"], false);
    let value = payload(&result);
    assert_eq!(value["block"], json!(2));
    let expected = dare_drop_blocks(&base_net(), &tuned_net(), 0.5, 7, 2).expect("blocks");
    let work = apply(&base_net(), &expected.patch);
    let artifact = value["artifact"].as_str().expect("artifact");
    assert_eq!(
        from_wsp(artifact).expect("parse").state_hash(),
        work.state_hash()
    );
}
