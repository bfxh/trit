//! `diagnose` 的三种结论：ok / repairable / not-repairable，以及 CLI 端到端。

use std::fs;
use std::process::Command;

use serde_json::Value;
use trit_wspace::diagnose::{DiagnoseOptions, Verdict, diagnose};
use trit_wspace::{Edit, Layer, Net, Oracle, Patch, Trit, to_wsp, wire};

fn xor_net() -> Net {
    Net::new(vec![
        Layer::new(2, 2, &[-1, -1, -1, -1], &[-1, 1]).expect("l0"),
        Layer::new(1, 2, &[-1, 1], &[-1]).expect("l1"),
    ])
    .expect("net")
}

fn xor_oracle() -> Oracle {
    Oracle::boolean(2, |b| b[0] != b[1]).expect("oracle")
}

fn break_one(net: &mut Net) {
    Patch::new(vec![Edit::SetTrit {
        layer: 0,
        row: 0,
        col: 0,
        to: Trit::Zero,
    }])
    .apply(net)
    .expect("apply");
}

#[test]
fn exact_network_is_ok() {
    let d = diagnose(&xor_net(), &xor_oracle(), &DiagnoseOptions::default()).expect("diagnose");
    assert_eq!(d.verdict, Verdict::Ok);
    assert!(d.exact);
    assert_eq!(d.loss, 0);
    assert!(!d.rows.is_empty(), "ok 也要报脆弱行");
}

#[test]
fn one_break_is_repairable() {
    let mut broken = xor_net();
    break_one(&mut broken);
    let d = diagnose(&broken, &xor_oracle(), &DiagnoseOptions::default()).expect("diagnose");
    assert_eq!(d.verdict, Verdict::Repairable);
    assert!(d.repair.found);
    assert_eq!(d.repair.patch_len, 1);
    assert!(!d.edits.is_empty());
    assert!(d.edits[0].gain > 0, "最佳单点编辑应有正收益");
}

#[test]
fn zero_budget_is_not_repairable() {
    let mut broken = xor_net();
    break_one(&mut broken);
    let options = DiagnoseOptions {
        max_edits: 0,
        ..DiagnoseOptions::default()
    };
    let d = diagnose(&broken, &xor_oracle(), &options).expect("diagnose");
    assert_eq!(d.verdict, Verdict::NotRepairable);
    assert!(!d.repair.found);
}

#[test]
fn cli_reports_ok() {
    let dir = std::env::temp_dir().join(format!("trit-wspace-diag-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).expect("dir");
    let net_path = dir.join("net.wsp");
    let oracle_path = dir.join("oracle.json");
    fs::write(&net_path, to_wsp(&xor_net())).expect("net");
    let oracle = serde_json::to_string(&wire::oracle_to_value(&xor_oracle())).expect("oracle json");
    fs::write(&oracle_path, oracle).expect("oracle");
    let output = Command::new(env!("CARGO_BIN_EXE_trit-wspace"))
        .args([
            "diagnose",
            net_path.to_str().expect("path"),
            "--oracle",
            oracle_path.to_str().expect("path"),
        ])
        .output()
        .expect("run");
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let value: Value = serde_json::from_slice(&output.stdout).expect("json");
    assert_eq!(value["verdict"], "ok");
    assert_eq!(value["loss"], 0);
    assert!(
        value["rows"]
            .as_array()
            .is_some_and(|rows| !rows.is_empty())
    );
    assert!(
        value["suggestion"]
            .as_str()
            .is_some_and(|text| text.contains("已满足"))
    );
}
