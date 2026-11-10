//! 三方补丁重放：干净应用 / 已应用跳过 / 冲突整体放弃 / 行操作与 CLI、MCP 入口。

use std::fs;
use std::path::PathBuf;
use std::process::{Command, Output};

use serde_json::{Value, json};
use trit_wspace::mcp::Mcp;
use trit_wspace::rebase::{Conflict, rebase};
use trit_wspace::{Edit, Error, Layer, Net, Oracle, Patch, Trit, from_wsp, to_wsp};

fn xor_net() -> Net {
    Net::new(vec![
        Layer::new(2, 2, &[-1, -1, -1, -1], &[-1, 1]).expect("l0"),
        Layer::new(1, 2, &[-1, 1], &[-1]).expect("l1"),
    ])
    .expect("xor net")
}

fn rowed_net() -> Net {
    Net::new(vec![
        Layer::new(2, 2, &[-1, -1, 1, 1], &[-1, 1]).expect("l0"),
        Layer::new(1, 2, &[-1, 1], &[-1]).expect("l1"),
    ])
    .expect("rowed net")
}

fn apply(net: &Net, edits: Vec<Edit>) -> Net {
    let mut work = net.clone();
    Patch::new(edits).apply(&mut work).expect("apply");
    work
}

fn temp_dir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("trit-wspace-rebase-{tag}-{}", std::process::id()));
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
            "jsonrpc": "2.0", "id": 9, "method": "tools/call",
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
fn clean_rebase_applies_onto_advanced_target() {
    let base = xor_net();
    let edit = Edit::SetTrit {
        layer: 1,
        row: 0,
        col: 1,
        to: Trit::Zero,
    };
    let other = Edit::SetTrit {
        layer: 0,
        row: 1,
        col: 1,
        to: Trit::Zero,
    };
    let target = apply(&base, vec![other.clone()]);
    let report = rebase(&base, &target, &Patch::new(vec![edit.clone()]), false).expect("rebase");
    assert!(report.conflicts.is_empty());
    assert_eq!(report.already_applied, 0);
    assert!(!report.rejected);
    assert_eq!(report.clean.len(), 1);

    let mut merged = target.clone();
    report.clean.apply(&mut merged).expect("apply clean");
    assert_eq!(merged, apply(&base, vec![edit, other]));
    assert_eq!(
        target,
        apply(
            &base,
            vec![Edit::SetTrit {
                layer: 0,
                row: 1,
                col: 1,
                to: Trit::Zero
            }]
        )
    );
}

#[test]
fn conflict_reports_both_sides_and_keeps_target() {
    let base = xor_net();
    let mine = Edit::SetTrit {
        layer: 0,
        row: 0,
        col: 0,
        to: Trit::Zero,
    };
    let theirs = Edit::SetTrit {
        layer: 0,
        row: 0,
        col: 0,
        to: Trit::Pos,
    };
    let target = apply(&base, vec![theirs]);
    let report = rebase(&base, &target, &Patch::new(vec![mine]), false).expect("rebase");
    assert!(report.rejected);
    assert!(report.clean.is_empty());
    assert_eq!(
        report.conflicts,
        vec![Conflict {
            edit: Edit::SetTrit {
                layer: 0,
                row: 0,
                col: 0,
                to: Trit::Zero,
            },
            base: "-".into(),
            target: "+".into(),
        }]
    );
    assert_eq!(target.layer(0).expect("l0").trit(0, 0), Some(Trit::Pos));
}

#[test]
fn already_applied_edit_is_skipped() {
    let base = xor_net();
    let edit = Edit::SetTrit {
        layer: 0,
        row: 0,
        col: 0,
        to: Trit::Zero,
    };
    let target = apply(&base, vec![edit.clone()]);
    let report = rebase(&base, &target, &Patch::new(vec![edit]), false).expect("rebase");
    assert!(!report.rejected);
    assert_eq!(report.already_applied, 1);
    assert!(report.clean.is_empty());
    assert!(report.conflicts.is_empty());
}

#[test]
fn skip_conflicts_applies_only_clean_edits() {
    let base = xor_net();
    let blocked = Edit::SetTrit {
        layer: 0,
        row: 0,
        col: 0,
        to: Trit::Zero,
    };
    let clean = Edit::SetTrit {
        layer: 1,
        row: 0,
        col: 1,
        to: Trit::Zero,
    };
    let target = apply(
        &base,
        vec![Edit::SetTrit {
            layer: 0,
            row: 0,
            col: 0,
            to: Trit::Pos,
        }],
    );
    let report = rebase(
        &base,
        &target,
        &Patch::new(vec![blocked, clean.clone()]),
        true,
    )
    .expect("rebase");
    assert!(!report.rejected);
    assert_eq!(report.conflicts.len(), 1);
    assert_eq!(report.clean.edits(), std::slice::from_ref(&clean));

    let mut merged = target.clone();
    report.clean.apply(&mut merged).expect("apply clean");
    assert_eq!(merged.layer(1).expect("l1").trit(0, 1), Some(Trit::Zero));
    assert_eq!(merged.layer(0).expect("l0").trit(0, 0), Some(Trit::Pos));
}

#[test]
fn swap_rows_already_applied_is_skipped() {
    let base = rowed_net();
    let swap = Edit::SwapRows {
        layer: 0,
        a: 0,
        b: 1,
    };
    let target = apply(&base, vec![swap.clone()]);
    let report = rebase(&base, &target, &Patch::new(vec![swap]), false).expect("rebase");
    assert_eq!(report.already_applied, 1);
    assert!(report.clean.is_empty());
    assert!(report.conflicts.is_empty());
    assert_eq!(
        target.layer(0).expect("l0").row(0),
        base.layer(0).expect("l0").row(1)
    );
}

#[test]
fn swap_rows_conflicts_when_a_row_changed() {
    let base = rowed_net();
    let target = apply(
        &base,
        vec![Edit::SetTrit {
            layer: 0,
            row: 0,
            col: 0,
            to: Trit::Zero,
        }],
    );
    let report = rebase(
        &base,
        &target,
        &Patch::new(vec![Edit::SwapRows {
            layer: 0,
            a: 0,
            b: 1,
        }]),
        false,
    )
    .expect("rebase");
    assert!(report.rejected);
    assert_eq!(report.conflicts.len(), 1);
    assert_ne!(report.conflicts[0].base, report.conflicts[0].target);
}

#[test]
fn negate_row_already_applied_is_skipped() {
    let base = rowed_net();
    let negate = Edit::NegateRow { layer: 0, row: 0 };
    let target = apply(&base, vec![negate.clone()]);
    let report = rebase(&base, &target, &Patch::new(vec![negate]), false).expect("rebase");
    assert_eq!(report.already_applied, 1);
    assert!(report.clean.is_empty());
    assert!(report.conflicts.is_empty());
}

#[test]
fn negate_row_conflicts_when_a_row_changed() {
    let base = rowed_net();
    let target = apply(
        &base,
        vec![Edit::SetTrit {
            layer: 0,
            row: 0,
            col: 0,
            to: Trit::Zero,
        }],
    );
    let report = rebase(
        &base,
        &target,
        &Patch::new(vec![Edit::NegateRow { layer: 0, row: 0 }]),
        false,
    )
    .expect("rebase");
    assert!(report.rejected);
    assert_eq!(report.conflicts.len(), 1);
}

#[test]
fn scale_edit_detects_conflict_and_already_applied() {
    let base = xor_net();
    let target = apply(
        &base,
        vec![Edit::SetScale {
            layer: 0,
            row: 0,
            to: 2.0,
        }],
    );
    let same = rebase(
        &base,
        &target,
        &Patch::new(vec![Edit::SetScale {
            layer: 0,
            row: 0,
            to: 2.0,
        }]),
        false,
    )
    .expect("rebase");
    assert_eq!(same.already_applied, 1);

    let conflict = rebase(
        &base,
        &target,
        &Patch::new(vec![Edit::SetScale {
            layer: 0,
            row: 0,
            to: 3.0,
        }]),
        false,
    )
    .expect("rebase");
    assert!(conflict.rejected);
    assert_eq!(conflict.conflicts[0].base, "1");
    assert_eq!(conflict.conflicts[0].target, "2");
}

#[test]
fn arch_mismatch_is_rejected() {
    let base = xor_net();
    let target = Net::zeros(&[3, 2, 1]).expect("other arch");
    let error = rebase(&base, &target, &Patch::new(Vec::new()), false).expect_err("mismatch");
    assert_eq!(error, Error::ArchMismatch);
}

#[test]
fn cli_rebase_reports_conflict_and_leaves_target() {
    let dir = temp_dir("conflict");
    let base = xor_net();
    let target = apply(
        &base,
        vec![Edit::SetTrit {
            layer: 0,
            row: 0,
            col: 0,
            to: Trit::Pos,
        }],
    );
    let base_path = dir.join("base.wsp");
    let target_path = dir.join("target.wsp");
    let patch_path = dir.join("patch.json");
    fs::write(&base_path, to_wsp(&base)).expect("base");
    fs::write(&target_path, to_wsp(&target)).expect("target");
    // 第二条是干净编辑：整体放弃时它也不能落盘。
    fs::write(
        &patch_path,
        r#"{"version":1,"edits":[
            {"op":"set_trit","layer":0,"row":0,"col":0,"to":"0"},
            {"op":"set_trit","layer":1,"row":0,"col":1,"to":"0"}
        ]}"#,
    )
    .expect("patch");

    let report = stdout_json(&run(&[
        "rebase",
        &path_string(&target_path),
        "--base",
        &path_string(&base_path),
        "--patch",
        &path_string(&patch_path),
    ]));
    assert_eq!(report["rejected"].as_bool(), Some(true));
    assert_eq!(report["conflicts"].as_array().expect("conflicts").len(), 1);
    assert_eq!(report["conflicts"][0]["base"].as_str(), Some("-"));
    assert_eq!(report["conflicts"][0]["target"].as_str(), Some("+"));
    assert!(report.get("applied").is_none(), "放弃时不应报告已应用");
    let kept = from_wsp(report["artifact"].as_str().expect("artifact")).expect("parse");
    assert_eq!(kept, target);
}

#[test]
fn cli_rebase_skip_conflicts_writes_partial_and_verifies() {
    let dir = temp_dir("skip");
    let base = xor_net();
    let broken = apply(
        &base,
        vec![Edit::SetTrit {
            layer: 0,
            row: 0,
            col: 0,
            to: Trit::Zero,
        }],
    );
    // target 在别的坐标上前进了（scale 不改行为），同一补丁的 scale 编辑与它冲突。
    let target = apply(
        &broken,
        vec![Edit::SetScale {
            layer: 0,
            row: 0,
            to: 3.0,
        }],
    );
    let base_path = dir.join("base.wsp");
    let target_path = dir.join("target.wsp");
    let patch_path = dir.join("patch.json");
    let oracle_path = dir.join("xor.json");
    let out_path = dir.join("out.wsp");
    fs::write(&base_path, to_wsp(&broken)).expect("base");
    fs::write(&target_path, to_wsp(&target)).expect("target");
    fs::write(
        &patch_path,
        r#"{"version":1,"edits":[
            {"op":"set_trit","layer":0,"row":0,"col":0,"to":"-"},
            {"op":"set_scale","layer":0,"row":0,"to":2.0}
        ]}"#,
    )
    .expect("patch");
    fs::write(
        &oracle_path,
        r#"{"domain":"boolean","vars":2,"table":"0110"}"#,
    )
    .expect("oracle");

    let report = stdout_json(&run(&[
        "rebase",
        &path_string(&target_path),
        "--base",
        &path_string(&base_path),
        "--patch",
        &path_string(&patch_path),
        "--skip-conflicts",
        "--oracle",
        &path_string(&oracle_path),
        "--require-after",
        "--out",
        &path_string(&out_path),
    ]));
    assert_eq!(report["rejected"].as_bool(), Some(false));
    assert_eq!(report["applied"].as_bool(), Some(true));
    assert_eq!(report["verify"]["exact"].as_bool(), Some(true));
    let written = from_wsp(&fs::read_to_string(&out_path).expect("out")).expect("parse out");
    let oracle = Oracle::boolean(2, |bits| bits[0] != bits[1]).expect("oracle");
    assert!(oracle.exact(&written).expect("verify"));
}

#[test]
fn mcp_rebase_returns_clean_artifact_and_conflicts() {
    let base = xor_net();
    let target = apply(
        &base,
        vec![Edit::SetTrit {
            layer: 0,
            row: 0,
            col: 0,
            to: Trit::Pos,
        }],
    );
    let mut server = Mcp::new();
    let rejected = payload(&call(
        &mut server,
        "wsp_rebase",
        json!({
            "artifact": to_wsp(&target),
            "base": to_wsp(&base),
            "patch": {"version": 1, "edits": [
                {"op": "set_trit", "layer": 0, "row": 0, "col": 0, "to": "0"},
                {"op": "set_trit", "layer": 1, "row": 0, "col": 1, "to": "0"}
            ]},
        }),
    ));
    assert_eq!(rejected["rejected"].as_bool(), Some(true));
    assert_eq!(
        rejected["conflicts"].as_array().expect("conflicts").len(),
        1
    );
    assert!(rejected.get("applied").is_none(), "放弃时不应报告已应用");
    let kept = from_wsp(rejected["artifact"].as_str().expect("artifact")).expect("parse");
    assert_eq!(kept, target);

    let accepted = payload(&call(
        &mut server,
        "wsp_rebase",
        json!({
            "artifact": to_wsp(&target),
            "base": to_wsp(&base),
            "patch": {"version": 1, "edits": [
                {"op": "set_trit", "layer": 1, "row": 0, "col": 1, "to": "0"}
            ]},
        }),
    ));
    assert_eq!(accepted["rejected"].as_bool(), Some(false));
    assert_eq!(accepted["applied"].as_bool(), Some(true));
    let net = from_wsp(accepted["artifact"].as_str().expect("artifact")).expect("parse");
    assert_eq!(net.layer(1).expect("l1").trit(0, 1), Some(Trit::Zero));
}
