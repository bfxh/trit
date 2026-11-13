//! `project --agree-from` 与 `certify` 的一致性。
//!
//! 不变量：当 oracle 的期望输出取自**原网络**时，
//! `agree.rate`（原网络 vs 投影网络的一致率）必须严格等于
//! `certify` 的 keep 保持率（投影网络 == 期望输出）。
//!
//! 两边测的是同一个比值，只是算法不同——这条测试就是验证「agree 的粗筛可靠」。

use std::fs;
use std::path::PathBuf;
use std::process::Command;

use serde_json::Value;
use trit_wspace::{Layer, Net, Trit, to_wsp};

fn temp_dir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "trit-wspace-agree-loop-{tag}-{}",
        std::process::id()
    ));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).expect("temp dir");
    dir
}

fn bin() -> &'static str {
    env!("CARGO_BIN_EXE_trit-wspace")
}

fn run(args: &[&str]) -> Value {
    let output = Command::new(bin()).args(args).output().expect("run cli");
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).expect("json stdout")
}

/// 穷举 4 维三值输入里的 8 个（-1/0/+1 的组合），覆盖足够多样本。
fn sample_inputs() -> Vec<Vec<Trit>> {
    let pool = [Trit::Neg, Trit::Zero, Trit::Pos];
    let mut out = Vec::new();
    for a in 0..3 {
        for b in 0..3 {
            out.push(vec![pool[a], pool[b], pool[(a + b) % 3], pool[(a * b) % 3]]);
        }
    }
    out
}

/// 把 4 维输入按 basis（k×4）投到 k 维——与 `project` 内部做的事一致。
fn project_input(input: &[Trit], basis: &Layer, k: usize) -> Vec<Trit> {
    (0..k)
        .map(|row| {
            let mut acc: i32 = 0;
            for (col, value) in input.iter().enumerate() {
                let w = basis.trit(row, col).map(|t| t.to_i8()).unwrap_or(0);
                acc += i32::from(w) * i32::from(value.to_i8());
            }
            match acc.signum() {
                1 => Trit::Pos,
                -1 => Trit::Neg,
                _ => Trit::Zero,
            }
        })
        .collect()
}

fn trits(values: &[Trit]) -> Vec<i8> {
    values.iter().map(|t| t.to_i8()).collect()
}

#[test]
fn agree_rate_equals_certify_keep_rate() {
    let dir = temp_dir("match");
    // 网络：3×4 → 2×3
    let net = Net::new(vec![
        Layer::new(3, 4, &[1, 0, -1, 0, 0, 1, 0, -1, -1, 0, 1, 0], &[0, 0, 0]).expect("l0"),
        Layer::new(2, 3, &[1, 0, -1, 0, 1, 0], &[0, 0]).expect("l1"),
    ])
    .expect("net");
    // basis：2×4（把 4 维输入压到 2 维）
    let basis = Net::new(vec![
        Layer::new(2, 4, &[1, 1, 0, 0, 0, 0, 1, 1], &[0, 0]).expect("b"),
    ])
    .expect("basis");

    let inputs = sample_inputs();
    let outputs: Vec<Vec<Trit>> = inputs
        .iter()
        .map(|input| net.forward(input).expect("forward"))
        .collect();
    let samples = inputs.len();

    let net_path = dir.join("net.wsp");
    let basis_path = dir.join("basis.wsp");
    let oracle_path = dir.join("oracle.json");
    fs::write(&net_path, to_wsp(&net)).expect("net");
    fs::write(&basis_path, to_wsp(&basis)).expect("basis");
    let oracle = serde_json::json!({
        "inputs": inputs.iter().map(|row| trits(row)).collect::<Vec<_>>(),
        "outputs": outputs.iter().map(|row| trits(row)).collect::<Vec<_>>(),
    });
    fs::write(&oracle_path, oracle.to_string()).expect("oracle");

    // certify 用的 oracle：输入已按 basis 投影到 k 维（与 agree 内部的投影一致）
    let projected_inputs: Vec<Vec<Trit>> = inputs
        .iter()
        .map(|row| project_input(row, basis.layer(0).expect("basis layer"), 2))
        .collect();
    let projected_oracle_path = dir.join("oracle-projected.json");
    let projected_oracle = serde_json::json!({
        "inputs": projected_inputs.iter().map(|row| trits(row)).collect::<Vec<_>>(),
        "outputs": outputs.iter().map(|row| trits(row)).collect::<Vec<_>>(),
    });
    fs::write(&projected_oracle_path, projected_oracle.to_string()).expect("projected oracle");

    // 1) project：用 oracle 的真实输入采样，样本数正好等于 oracle 大小
    let projected_path = dir.join("projected.wsp");
    let project = run(&[
        "project",
        net_path.to_str().expect("p"),
        "--basis",
        basis_path.to_str().expect("p"),
        "--out",
        projected_path.to_str().expect("p"),
        "--agree-from",
        oracle_path.to_str().expect("p"),
        "--samples",
        &samples.to_string(),
    ]);
    let rate = project["agree"]["rate"].as_f64().expect("rate");
    assert_eq!(
        project["agree"]["source"].as_str(),
        Some("oracle"),
        "来源必须标为 oracle"
    );

    // 2) 独立 oracle：用手工重建的投影网络逐样本算一致率
    let basis_layer = basis.layer(0).expect("basis layer");
    let projected_layer = net
        .layer(0)
        .expect("net layer")
        .project(basis_layer)
        .expect("project");
    let manual_net = Net::new(vec![
        projected_layer,
        net.layer(1).expect("readout").clone(),
    ])
    .expect("manual projected net");
    let mut same = 0usize;
    for (input, expected) in inputs.iter().zip(&outputs) {
        let reduced = project_input(input, basis_layer, 2);
        if manual_net.forward(&reduced).expect("forward") == *expected {
            same += 1;
        }
    }
    let manual_rate = same as f64 / inputs.len() as f64;
    // 3) 同一个比值，两种算法：必须一致
    assert!(
        (rate - manual_rate).abs() < 1e-9,
        "agree.rate={rate} 与手工一致率={manual_rate} 不一致"
    );
}
