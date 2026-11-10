//! 跨层单元移植：入边 + 偏置 + scale + 出边一起搬；输出层只搬行。

use std::fs;
use std::path::PathBuf;
use std::process::{Command, Output};

use serde_json::{Value, json};
use trit_wspace::mcp::Mcp;
use trit_wspace::{Error, Layer, Net, Oracle, Patch, Trit, from_wsp, to_wsp, transplant_units};

fn xor_net() -> Net {
    Net::new(vec![
        Layer::new(2, 2, &[-1, -1, -1, -1], &[-1, 1]).expect("l0"),
        Layer::new(1, 2, &[-1, 1], &[-1]).expect("l1"),
    ])
    .expect("xor net")
}

fn source_net() -> Net {
    Net::new(vec![
        Layer::new(2, 2, &[-1, 0, 1, 1], &[-1, 1]).expect("l0"),
        Layer::new(2, 2, &[1, -1, 0, 1], &[0, -1]).expect("l1"),
        Layer::new(1, 2, &[1, -1], &[1]).expect("l2"),
    ])
    .expect("source net")
}

fn dest_net() -> Net {
    Net::new(vec![
        Layer::new(2, 2, &[1, 1, -1, 0], &[1, -1]).expect("l0"),
        Layer::new(2, 2, &[-1, 1, 1, 0], &[1, 0]).expect("l1"),
        Layer::new(1, 2, &[-1, 1], &[0]).expect("l2"),
    ])
    .expect("dest net")
}

fn apply(net: &Net, patch: &Patch) -> Net {
    let mut work = net.clone();
    patch.apply(&mut work).expect("apply");
    work
}

fn temp_dir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "trit-wspace-transplant-{tag}-{}",
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
            "jsonrpc": "2.0", "id": 11, "method": "tools/call",
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
fn transplant_copies_row_bias_and_outgoing_column() {
    let source = source_net();
    let dest = dest_net();
    let patch = transplant_units(&source, &dest, 0, &[0]).expect("transplant");
    let cost = patch.cost(&dest);
    assert_eq!(cost.trits, 4);
    assert_eq!(cost.biases, 1);
    assert_eq!(cost.scales, 0);

    let work = apply(&dest, &patch);
    let src0 = source.layer(0).expect("src l0");
    let out0 = work.layer(0).expect("out l0");
    assert_eq!(out0.row(0), src0.row(0));
    assert_eq!(out0.bias_trit(0), src0.bias_trit(0));
    assert_eq!(out0.row(1), dest.layer(0).expect("dest l0").row(1));

    let src1 = source.layer(1).expect("src l1");
    let out1 = work.layer(1).expect("out l1");
    assert_eq!(out1.trit(0, 0), src1.trit(0, 0));
    assert_eq!(out1.trit(1, 0), src1.trit(1, 0));
    let dest1 = dest.layer(1).expect("dest l1");
    assert_eq!(out1.trit(0, 1), dest1.trit(0, 1));
    assert_eq!(out1.trit(1, 1), dest1.trit(1, 1));
    assert_eq!(out1.bias_trit(0), dest1.bias_trit(0));
}

#[test]
fn transplant_identical_nets_is_empty() {
    let net = source_net();
    let patch = transplant_units(&net, &net, 0, &[0, 1]).expect("transplant");
    assert!(patch.is_empty());
    let empty = transplant_units(&net, &net, 0, &[]).expect("transplant");
    assert!(empty.is_empty());
}

#[test]
fn transplant_output_layer_copies_row_only() {
    let source = source_net();
    let dest = dest_net();
    let patch = transplant_units(&source, &dest, 2, &[0]).expect("transplant");
    assert_eq!(patch.len(), 3);
    let work = apply(&dest, &patch);
    let out = work.layer(2).expect("out");
    let src = source.layer(2).expect("src");
    assert_eq!(out.row(0), src.row(0));
    assert_eq!(out.bias_trit(0), src.bias_trit(0));
    assert_eq!(out.scale(0), src.scale(0));
}

#[test]
fn transplant_copies_scale() {
    let source = Net::new(vec![
        Layer::with_scales(2, 2, &[-1, 0, 1, 1], &[-1, 1], vec![2.5, 1.0]).expect("l0"),
        Layer::new(1, 2, &[1, -1], &[0]).expect("l1"),
    ])
    .expect("source");
    let dest = Net::new(vec![
        Layer::new(2, 2, &[-1, 0, 1, 1], &[-1, 1]).expect("l0"),
        Layer::new(1, 2, &[1, -1], &[0]).expect("l1"),
    ])
    .expect("dest");
    let patch = transplant_units(&source, &dest, 0, &[0]).expect("transplant");
    assert_eq!(patch.cost(&dest).scales, 1);
    let work = apply(&dest, &patch);
    assert_eq!(work.layer(0).expect("l0").scale(0), Some(2.5));
}

#[test]
fn transplant_rejects_bad_rows_and_arch() {
    let source = source_net();
    let dest = dest_net();
    assert_eq!(
        transplant_units(&source, &dest, 0, &[1, 1]).expect_err("duplicate"),
        Error::DuplicateRow { layer: 0, row: 1 }
    );
    assert_eq!(
        transplant_units(&source, &dest, 0, &[5]).expect_err("oob"),
        Error::OutOfBounds {
            layer: 0,
            row: 5,
            col: 0
        }
    );
    let other = Net::zeros(&[3, 2, 1]).expect("other arch");
    assert_eq!(
        transplant_units(&source, &other, 0, &[0]).expect_err("arch"),
        Error::ArchMismatch
    );
}

#[test]
fn transplanting_every_unit_matches_permutation() {
    let trained = xor_net();
    let permuted = trained.permute_rows(0, &[1, 0]).expect("permute");
    let patch = transplant_units(&permuted, &trained, 0, &[0, 1]).expect("transplant");
    let work = apply(&trained, &patch);
    assert_eq!(work, permuted);
    let oracle = Oracle::boolean(2, |bits| bits[0] != bits[1]).expect("oracle");
    assert!(oracle.exact(&work).expect("verify"));
}

#[test]
fn cli_transplant_writes_artifact_and_verifies() {
    let dir = temp_dir("cli");
    let trained = xor_net();
    let permuted = trained.permute_rows(0, &[1, 0]).expect("permute");
    let dest_path = dir.join("dest.wsp");
    let source_path = dir.join("source.wsp");
    let oracle_path = dir.join("xor.json");
    let out_path = dir.join("out.wsp");
    fs::write(&dest_path, to_wsp(&trained)).expect("dest");
    fs::write(&source_path, to_wsp(&permuted)).expect("source");
    fs::write(
        &oracle_path,
        r#"{"domain":"boolean","vars":2,"table":"0110"}"#,
    )
    .expect("oracle");

    let report = stdout_json(&run(&[
        "transplant",
        &path_string(&dest_path),
        "--from",
        &path_string(&source_path),
        "--layer",
        "0",
        "--rows",
        "0,1",
        "--oracle",
        &path_string(&oracle_path),
        "--require-after",
        "--out",
        &path_string(&out_path),
    ]));
    assert_eq!(report["applied"].as_bool(), Some(true));
    assert_eq!(report["verify"]["exact"].as_bool(), Some(true));
    assert_eq!(report["cost"]["trits"].as_u64(), Some(2));
    assert_eq!(report["cost"]["biases"].as_u64(), Some(2));
    let written = from_wsp(&fs::read_to_string(&out_path).expect("out")).expect("parse out");
    assert_eq!(written, permuted);
}

#[test]
fn cli_transplant_rejects_duplicate_rows() {
    let dir = temp_dir("dup");
    let dest_path = dir.join("dest.wsp");
    let source_path = dir.join("source.wsp");
    fs::write(&dest_path, to_wsp(&dest_net())).expect("dest");
    fs::write(&source_path, to_wsp(&source_net())).expect("source");
    let output = run(&[
        "transplant",
        &path_string(&dest_path),
        "--from",
        &path_string(&source_path),
        "--layer",
        "0",
        "--rows",
        "0,0",
    ]);
    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("重复"),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn mcp_transplant_returns_patch_and_artifact() {
    let trained = xor_net();
    let permuted = trained.permute_rows(0, &[1, 0]).expect("permute");
    let mut server = Mcp::new();
    let result = call(
        &mut server,
        "wsp_transplant",
        json!({
            "artifact": to_wsp(&trained),
            "source": to_wsp(&permuted),
            "layer": 0,
            "rows": [0, 1],
            "oracle": {"domain": "boolean", "vars": 2, "table": "0110"},
            "require_after": true,
        }),
    );
    assert_eq!(result["isError"], false);
    let value = payload(&result);
    assert_eq!(value["applied"].as_bool(), Some(true));
    assert_eq!(value["verify"]["exact"].as_bool(), Some(true));
    assert_eq!(value["cost"]["trits"].as_u64(), Some(2));
    let net = from_wsp(value["artifact"].as_str().expect("artifact")).expect("parse");
    assert_eq!(net, permuted);
    assert_eq!(net.layer(1).expect("l1").trit(0, 0), Some(Trit::Pos));
}
