//! CLI `project`：用基工件压缩网络的某一层（输入维度）。
//!
//! 端到端：写两个 WSP → 跑 `project` → 读回验证形状与后续层未被改动。

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use trit_wspace::{Layer, Net, from_wsp, to_wsp};

fn temp_dir(tag: &str) -> PathBuf {
    let dir =
        std::env::temp_dir().join(format!("trit-wspace-project-{tag}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).expect("temp dir");
    dir
}

fn write_net(dir: &Path, name: &str, net: &Net) -> String {
    let path = dir.join(name);
    fs::write(&path, to_wsp(net)).expect("write wsp");
    path.to_str().expect("utf-8 path").to_string()
}

#[test]
fn project_cli_compresses_first_layer_only() {
    let dir = temp_dir("compress");
    // 3×4 → 2×3 的两层网络
    let net = Net::new(vec![
        Layer::new(3, 4, &[1, 0, -1, 0, 0, 1, 0, -1, -1, 0, 1, 0], &[0, 0, 0]).expect("l0"),
        Layer::new(2, 3, &[1, 0, -1, 0, 1, 0], &[0, 0]).expect("l1"),
    ])
    .expect("net");
    let net_path = write_net(&dir, "net.wsp", &net);

    // 基：2×4（把 4 维输入压到 2 维）
    let basis = Net::new(vec![
        Layer::new(2, 4, &[1, 1, 0, 0, 0, 0, 1, 1], &[0, 0]).expect("b"),
    ])
    .expect("basis");
    let basis_path = write_net(&dir, "basis.wsp", &basis);

    let out = dir.join("out.wsp");
    let output = Command::new(env!("CARGO_BIN_EXE_trit-wspace"))
        .args([
            "project",
            &net_path,
            "--basis",
            &basis_path,
            "--out",
            out.to_str().expect("out"),
        ])
        .output()
        .expect("run cli");
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let projected = from_wsp(&fs::read_to_string(&out).expect("read out")).expect("parse wsp");
    assert_eq!(projected.depth(), 2, "层数不变");
    assert_eq!(projected.layer(0).expect("l0").cols(), 2, "输入维度 4 → 2");
    assert_eq!(projected.layer(0).expect("l0").rows(), 3, "输出维度不变");
    assert_eq!(projected.layer(1).expect("l1").cols(), 3, "第二层不受影响");
}

#[test]
fn project_cli_rejects_multi_layer_basis() {
    let dir = temp_dir("bad-basis");
    let net = Net::new(vec![
        Layer::new(2, 3, &[1, 0, -1, 0, 1, 0], &[0, 0]).expect("l"),
    ])
    .expect("net");
    let net_path = write_net(&dir, "net.wsp", &net);
    let basis = Net::new(vec![
        Layer::new(2, 3, &[1, 0, 0, 0, 1, 0], &[0, 0]).expect("b0"),
        Layer::new(1, 2, &[1, 1], &[0]).expect("b1"),
    ])
    .expect("basis");
    let basis_path = write_net(&dir, "basis.wsp", &basis);
    let out = dir.join("out.wsp");

    let output = Command::new(env!("CARGO_BIN_EXE_trit-wspace"))
        .args([
            "project",
            &net_path,
            "--basis",
            &basis_path,
            "--out",
            out.to_str().expect("out"),
        ])
        .output()
        .expect("run cli");
    assert!(!output.status.success(), "多层基必须被拒绝");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("单层"), "stderr: {stderr}");
}
