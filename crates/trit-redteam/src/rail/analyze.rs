use std::collections::BTreeMap;

use anyhow::{Result, bail};

use super::{MAX_SIGNALS, RailDesign, class, eval_design, parse_design};

#[derive(Debug, Clone)]
pub struct RailReport {
    pub signals: usize,
    pub combinations: usize,
    pub legal_inputs: usize,
    pub illegal_inputs: usize,
    /// 合法输入下输出出非法态的行数（它本该是 0）。
    pub legal_output_invalid: usize,
    /// 非法输入下输出被"只看 pos"的消费端读成 True 的行数。
    pub illegal_output_true_by_pos: usize,
    /// 非法输入下输出 **恰好** 是有效 True（`(1,0)`）的行数——最硬的 fail-open。
    pub illegal_output_true: usize,
    /// 非法输入下输出是非法态 `(1,1)` 的行数。
    pub illegal_output_invalid: usize,
    /// 全 (1,1) 输入下的输出类别。
    pub all_illegal_output: String,
    pub examples: Vec<serde_json::Value>,
}

pub fn analyze(text: &str) -> Result<RailReport> {
    let RailDesign { inputs, assigns } = parse_design(text)?;
    if inputs.is_empty() || assigns.is_empty() {
        bail!(
            "不像 trit-build 导出的门级网表（inputs={} assigns={}）",
            inputs.len(),
            assigns.len()
        );
    }
    let signals = inputs.len() / 2;
    if signals > MAX_SIGNALS {
        bail!("信号数 {signals} 超过分析上限 {MAX_SIGNALS}（4^k 会爆）");
    }
    if !inputs.chunks(2).all(|pair| pair.len() == 2) {
        bail!("输入位线不是成对的");
    }

    let mut report = RailReport {
        signals,
        combinations: 4usize.pow(signals as u32),
        legal_inputs: 0,
        illegal_inputs: 0,
        legal_output_invalid: 0,
        illegal_output_true_by_pos: 0,
        illegal_output_true: 0,
        illegal_output_invalid: 0,
        all_illegal_output: String::new(),
        examples: Vec::new(),
    };

    for code in 0..report.combinations {
        let mut env = BTreeMap::new();
        let mut rest = code;
        let mut illegal = false;
        let mut pattern = Vec::new();
        for pair in inputs.chunks(2) {
            let state = rest % 4;
            rest /= 4;
            let (pos, neg) = match state {
                0 => (0u8, 0u8),
                1 => (0u8, 1u8),
                2 => (1u8, 0u8),
                _ => (1u8, 1u8),
            };
            illegal |= state == 3;
            env.insert(pair[0].clone(), pos);
            env.insert(pair[1].clone(), neg);
            pattern.push(format!("{}={pos}{neg}", pair[0].trim_end_matches("_p")));
        }
        eval_design(&assigns, &mut env)?;
        // 输出必须是网表声明的：缺了就报错，而不是索引 panic（台账 `L54`/`L56`）。
        let (Some(&pos), Some(&neg)) = (env.get("y_p"), env.get("y_n")) else {
            bail!(
                "网表缺少 y_p/y_n 输出（解析得到的信号：{:?}）",
                env.keys().collect::<Vec<_>>()
            );
        };
        let output = class(pos, neg);
        if illegal {
            report.illegal_inputs += 1;
            if pos == 1 {
                report.illegal_output_true_by_pos += 1;
            }
            if output == "true" {
                report.illegal_output_true += 1;
            }
            if output == "invalid" {
                report.illegal_output_invalid += 1;
            }
            if pos == 1 && report.examples.len() < 5 {
                report.examples.push(serde_json::json!({
                    "inputs": pattern.join(","),
                    "y_p": pos,
                    "y_n": neg,
                    "output": output,
                }));
            }
            if code == report.combinations - 1 {
                report.all_illegal_output = output.to_string();
            }
        } else {
            report.legal_inputs += 1;
            if output == "invalid" {
                report.legal_output_invalid += 1;
            }
        }
    }
    Ok(report)
}

/// 分析结果里是否存在需要严肃对待的发现。
pub fn has_findings(report: &RailReport) -> bool {
    report.legal_output_invalid > 0
        || report.illegal_output_true > 0
        || report.illegal_output_invalid > 0
}
