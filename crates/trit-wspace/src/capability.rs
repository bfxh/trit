//! 能力命名：把 `lost` 样本集合命名成输入模式或外部标签。
//!
//! 能力本身由「改哪些参数会失去这些样本」定义；命名只是给它一个人能读的标签：
//! - 有 `labels` 时用 `lost` 集合里的众数标签；
//! - 否则找 `lost` 样本在输入上的**共同坐标值**（`pattern: x0=+1,x1=-1`）；
//! - 都没有时退化为 `samples:N`。

use crate::attribution::{AttributeReport, ParamRef};
use crate::oracle::Oracle;
use crate::trit::Trit;

/// 命名后的能力。
#[derive(Clone, Debug, PartialEq)]
pub struct NamedCapability {
    /// 该能力对应的样本集合。
    pub lost: Vec<usize>,
    /// 改这些参数会失去该能力。
    pub support: Vec<ParamRef>,
    /// 人名可读的名字。
    pub name: String,
    /// 在 `lost` 样本里取值恒定的输入坐标。
    pub features: Vec<(usize, Trit)>,
    /// 外部标签众数（没有 `labels` 时为 `None`）。
    pub label: Option<String>,
}

/// 给报告里的每种能力命名。
#[must_use]
pub fn name_capabilities(
    oracle: &Oracle,
    report: &AttributeReport,
    labels: Option<&[String]>,
) -> Vec<NamedCapability> {
    report
        .capabilities
        .iter()
        .map(|cap| {
            let features = common_features(oracle, &cap.lost);
            let label = labels.and_then(|labels| majority(labels, &cap.lost));
            let name = if let Some(label) = &label {
                format!("label:{label}")
            } else if !features.is_empty() {
                let body = features
                    .iter()
                    .map(|(index, value)| format!("x{index}={}", value.to_i8()))
                    .collect::<Vec<_>>()
                    .join(",");
                format!("pattern:{body}")
            } else {
                format!("samples:{}", cap.lost.len())
            };
            NamedCapability {
                lost: cap.lost.clone(),
                support: cap.support.clone(),
                name,
                features,
                label,
            }
        })
        .collect()
}

fn common_features(oracle: &Oracle, lost: &[usize]) -> Vec<(usize, Trit)> {
    let inputs = oracle.inputs();
    let Some(&first) = lost.first() else {
        return Vec::new();
    };
    let width = inputs[first].len();
    (0..width)
        .filter_map(|index| {
            let value = inputs[first][index];
            lost.iter()
                .all(|&sample| inputs[sample][index] == value)
                .then_some((index, value))
        })
        .collect()
}

fn majority(labels: &[String], lost: &[usize]) -> Option<String> {
    let mut counts: std::collections::BTreeMap<&str, usize> = std::collections::BTreeMap::new();
    for &sample in lost {
        if let Some(label) = labels.get(sample) {
            *counts.entry(label.as_str()).or_default() += 1;
        }
    }
    counts
        .into_iter()
        .max_by(|(left_label, left), (right_label, right)| {
            left.cmp(right).then_with(|| right_label.cmp(left_label))
        })
        .map(|(label, _)| label.to_string())
}
