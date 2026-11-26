//! 机器入口的资源上界：超限必须在分配前返回结构化错误。

use std::process::Command;

use trit_machines::{ca, snn};

fn bin() -> std::path::PathBuf {
    if let Ok(path) = std::env::var("CARGO_BIN_EXE_trit-machines") {
        return std::path::PathBuf::from(path);
    }
    let mut dir = std::env::current_exe().expect("current exe");
    dir.pop();
    if dir.ends_with("deps") {
        dir.pop();
    }
    dir.join(format!("trit-machines{}", std::env::consts::EXE_SUFFIX))
}

fn run(args: &[&str]) -> (String, String, i32) {
    let out = Command::new(bin())
        .args(args)
        .output()
        .expect("failed to run trit-machines");
    (
        String::from_utf8_lossy(&out.stdout).to_string(),
        String::from_utf8_lossy(&out.stderr).to_string(),
        out.status.code().unwrap_or(-1),
    )
}

#[test]
fn random_network_rejects_oversized_size_without_allocating() {
    assert!(matches!(
        snn::Network::random(snn::MAX_NEURONS + 1, 7),
        Err(snn::NetworkError::Limit(_))
    ));
}

#[test]
fn ca_rejects_history_and_period_work_budgets() {
    let rule = ca::Rule::from_index(0).expect("规则编号");
    assert!(matches!(
        ca::run(&rule, ca::MAX_WIDTH + 1, 1),
        Err(ca::CaError::Limit(_))
    ));
    assert!(ca::run(&rule, 64, 10_000).is_err());
    assert!(ca::survey(40_000, 5_000).is_err());
}

#[test]
fn cli_rejects_oversized_inputs_before_allocating() {
    let (_, stderr, code) = run(&["ca", "--width", "40000", "--steps", "5000"]);
    assert_ne!(code, 0);
    assert!(stderr.contains("规模超限"), "{stderr}");

    for command in ["snn", "graded"] {
        let (_, stderr, code) = run(&[command, "--size", "4097"]);
        assert_ne!(code, 0, "{command} 应拒绝 4097 个神经元");
        assert!(stderr.contains("规模超限"), "{command}: {stderr}");
    }
}
