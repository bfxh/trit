//! 双导轨的一阶侧信道模型：**汉明重量直接把 Unknown 暴露出来**。
//!
//! 双导轨编码里 `True=(1,0)`、`False=(0,1)`、`Unknown=(0,0)`，所以一根三值信号贡献的
//! 汉明重量只可能是 0（Unknown）或 1（已知）。任何只测"总线上有多少根线为 1"的观测者，
//! 不用解逻辑就能把 Unknown 挑出来。
//!
//! 这里给的是**静态一阶模型**：对合法输入穷举，算
//! `H(输出类别)` 与 `H(输出类别 | 输出重量)` 的差（bit）作为泄漏量。
//! 这不是功耗/时序的物理模型，也不含噪声——只说明"编码本身泄漏多少"，不给攻击成功率。

use std::collections::BTreeMap;

use anyhow::{Result, bail};
use serde_json::{Value, json};

use crate::rail::{eval_expr, parse_design};

fn class_of(pos: u8, neg: u8) -> &'static str {
    match (pos, neg) {
        (1, 0) => "true",
        (0, 1) => "false",
        (0, 0) => "unknown",
        _ => "invalid",
    }
}

fn entropy(counts: &BTreeMap<String, usize>) -> f64 {
    let total: usize = counts.values().sum();
    if total == 0 {
        return 0.0;
    }
    counts
        .values()
        .filter(|count| **count > 0)
        .map(|count| {
            let p = *count as f64 / total as f64;
            -p * p.log2()
        })
        .sum()
}

#[derive(Debug)]
pub struct Report {
    pub signals: usize,
    pub rows: usize,
    pub h_output: f64,
    pub h_output_given_weight: f64,
    pub leakage_bits: f64,
    pub weight_histogram: BTreeMap<u8, BTreeMap<String, usize>>,
    pub unknown_case: String,
}

pub fn analyze(text: &str) -> Result<Report> {
    let (inputs, assigns) = parse_design(text);
    if inputs.is_empty() || inputs.len() % 2 != 0 || assigns.is_empty() {
        bail!(
            "不像 trit-build 导出的门级网表（inputs={} assigns={}）",
            inputs.len(),
            assigns.len()
        );
    }
    let signals = inputs.len() / 2;
    let rows = 3usize.pow(signals as u32);

    let mut outputs: BTreeMap<String, usize> = BTreeMap::new();
    let mut by_weight: BTreeMap<u8, BTreeMap<String, usize>> = BTreeMap::new();
    let mut unknown_case = String::new();

    for code in 0..rows {
        let mut env = BTreeMap::new();
        let mut rest = code;
        for pair in inputs.chunks(2) {
            let state = rest % 3;
            rest /= 3;
            let (pos, neg) = match state {
                0 => (0u8, 0u8),
                1 => (0u8, 1u8),
                _ => (1u8, 0u8),
            };
            env.insert(pair[0].clone(), pos);
            env.insert(pair[1].clone(), neg);
        }
        for (name, rhs) in &assigns {
            let value = eval_expr(rhs, &env);
            env.insert(name.clone(), value);
        }
        let (pos, neg) = (env["y_p"], env["y_n"]);
        let class = class_of(pos, neg);
        *outputs.entry(class.to_string()).or_default() += 1;
        *by_weight
            .entry(pos + neg)
            .or_default()
            .entry(class.to_string())
            .or_default() += 1;
        if class == "unknown" && unknown_case.is_empty() {
            let pattern: Vec<String> = inputs
                .chunks(2)
                .map(|pair| {
                    format!(
                        "{}={}{}",
                        pair[0].trim_end_matches("_p"),
                        env[&pair[0]],
                        env[&pair[1]]
                    )
                })
                .collect();
            unknown_case = pattern.join(",");
        }
    }

    let h_output = entropy(&outputs);
    let h_conditional: f64 = by_weight
        .values()
        .map(|bucket| {
            let total: usize = bucket.values().sum();
            entropy(bucket) * (total as f64 / rows as f64)
        })
        .sum();
    Ok(Report {
        signals,
        rows,
        h_output,
        h_output_given_weight: h_conditional,
        leakage_bits: (h_output - h_conditional).max(0.0),
        weight_histogram: by_weight,
        unknown_case,
    })
}

pub fn to_json(report: &Report) -> Value {
    let histogram: Vec<Value> = report
        .weight_histogram
        .iter()
        .map(|(weight, buckets)| {
            let total: usize = buckets.values().sum();
            json!({
                "output_weight": weight,
                "rows": total,
                "classes": buckets,
            })
        })
        .collect();
    json!({
        "tool": "trit-redteam",
        "check": "dual-rail-sidechannel",
        "signals": report.signals,
        "legal_inputs": report.rows,
        "entropy_output": round(report.h_output),
        "entropy_output_given_weight": round(report.h_output_given_weight),
        "leakage_bits": round(report.leakage_bits),
        "weight_histogram": histogram,
        "sample_unknown_input": report.unknown_case,
        "note": "一阶静态模型：True/False 各贡献 1 根拉高，Unknown 贡献 0 根；只测总拉高线数即可区分 Unknown。不含噪声与动态功耗。",
    })
}

fn round(value: f64) -> f64 {
    (value * 1000.0).round() / 1000.0
}

/// 泄漏是否构成发现：输出重量能区分类别（H 的下降 > 0）。
pub fn has_findings(report: &Report) -> bool {
    report.leakage_bits > 0.0
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str = include_str!("../fixtures/three_rail.v");

    #[test]
    fn weight_directly_reveals_unknowns() {
        let report = analyze(FIXTURE).unwrap();
        assert_eq!(report.signals, 3);
        assert_eq!(report.rows, 27);
        let bucket0 = report.weight_histogram.get(&0).expect("应有重量 0 的行");
        assert_eq!(
            bucket0.keys().collect::<Vec<_>>(),
            vec!["unknown"],
            "重量 0 必须只对应 Unknown：{bucket0:?}"
        );
        assert_eq!(
            report.leakage_bits,
            report.h_output - report.h_output_given_weight
        );
        assert!(report.leakage_bits > 0.0, "未知值应当泄漏：{report:?}");
    }

    #[test]
    fn a_design_without_unknown_outputs_has_no_leak() {
        // 常量输出没有 Unknown ⇒ 每行重量恒为 1 ⇒ 重量不携带信息。
        let design = "
module m(
  input wire a_p, a_n,
  output wire y_p, y_n
);
  assign y_p = 1'b1;
  assign y_n = 1'b0;
endmodule
";
        let report = analyze(design).unwrap();
        assert_eq!(report.h_output, 0.0, "输出类别恒定");
        assert_eq!(report.leakage_bits, 0.0, "没有信息可泄漏：{report:?}");
        assert!(!has_findings(&report));
    }

    #[test]
    fn unknown_density_matches_the_truth_table() {
        // `a & b`：K3 下 U∧U、U∧T、T∧U 三行是 Unknown（F∧U = F），其余 6 行有确定值。
        let design = "
module m(
  input wire a_p, a_n,
  input wire b_p, b_n,
  output wire y_p, y_n
);
  assign y_p = (a_p & b_p);
  assign y_n = (a_n | b_n);
endmodule
";
        let report = analyze(design).unwrap();
        assert_eq!(report.rows, 9);
        let zeros = report
            .weight_histogram
            .get(&0)
            .map(|b| *b.get("unknown").unwrap_or(&0))
            .unwrap_or(0);
        assert_eq!(zeros, 3, "U∧U / U∧T / T∧U 三行输出 Unknown：{report:?}");
        assert!(report.leakage_bits > 0.0);
    }
}
