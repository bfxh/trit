//! CICIDS2017（`MachineLearningCVE` 的逐日 CSV）读取与特征对齐。
//!
//! 数据集列名带前导空格，数值里混有 `Infinity`/`NaN`；两者都必须在解析阶段处理掉，
//! 否则 NaN 会一路穿透到阈值分位数，把整列排序搞坏。

use super::flow::N;

/// 缺失的必需列。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DatasetError(pub String);

impl std::fmt::Display for DatasetError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "数据集解析失败：{}", self.0)
    }
}

impl std::error::Error for DatasetError {}

/// 样本标签。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Label {
    /// `BENIGN`。
    Benign,
    /// 除 `BENIGN` 以外的所有标签（按「攻击 / 非攻击」二分类）。
    Malicious,
}

/// 一条对齐后的样本。
#[derive(Debug, Clone, Copy)]
pub struct Sample {
    /// 顺序与 [`super::flow::NAMES`] 一致。
    pub features: [f64; N],
    /// 标签。
    pub label: Label,
}

/// 解析结果。
#[derive(Debug, Clone)]
pub struct Dataset {
    /// 样本。
    pub samples: Vec<Sample>,
    /// 读到的数据行数（不含表头）。
    pub rows: usize,
    /// 因非有限值 / 缺列 / 长度不足被丢弃的行数。
    pub skipped: usize,
    /// 攻击样本数。
    pub attacks: usize,
}

/// 列名别名 → 特征下标（下标含义见 [`super::flow::NAMES`]）。
///
/// CICIDS2017（`MachineLearningCSV`）与 CSE-CIC-IDS2018（`TrafficForML_CICFlowMeter`）
/// 是同一套 CICFlowMeter 特征的两个命名版本，列名全称/缩写不同，值相同。
const MAPPING: [(&[&str], usize); 23] = [
    (&["Flow Duration"], 0),
    (&["Total Fwd Packets", "Tot Fwd Pkts"], 1),
    (&["Total Backward Packets", "Tot Bwd Pkts"], 2),
    (&["Total Length of Fwd Packets", "TotLen Fwd Pkts"], 3),
    (&["Total Length of Bwd Packets", "TotLen Bwd Pkts"], 4),
    (&["Fwd Packet Length Mean", "Fwd Pkt Len Mean"], 5),
    (&["Fwd Packet Length Std", "Fwd Pkt Len Std"], 6),
    (&["Fwd Packet Length Min", "Fwd Pkt Len Min"], 7),
    (&["Fwd Packet Length Max", "Fwd Pkt Len Max"], 8),
    (&["Bwd Packet Length Mean", "Bwd Pkt Len Mean"], 9),
    (&["Bwd Packet Length Std", "Bwd Pkt Len Std"], 10),
    (&["Bwd Packet Length Min", "Bwd Pkt Len Min"], 11),
    (&["Bwd Packet Length Max", "Bwd Pkt Len Max"], 12),
    (&["Flow IAT Mean"], 13),
    (&["Flow IAT Std"], 14),
    (&["Flow IAT Min"], 15),
    (&["Flow IAT Max"], 16),
    (&["Flow Packets/s", "Flow Pkts/s"], 17),
    (&["Flow Bytes/s", "Flow Byts/s"], 18),
    (&["SYN Flag Count", "SYN Flag Cnt"], 20),
    (&["RST Flag Count", "RST Flag Cnt"], 21),
    (&["FIN Flag Count", "FIN Flag Cnt"], 22),
    (&["PSH Flag Count", "PSH Flag Cnt"], 23),
];

/// 标签列。
const LABEL_COLUMN: &str = "Label";

/// 解析 CSV 文本；`limit` 是读入的数据行上限（0 表示不限）。
pub fn parse(text: &str, limit: usize) -> Result<Dataset, DatasetError> {
    let mut lines = text.lines();
    let header = lines
        .find(|line| !line.trim().is_empty())
        .ok_or_else(|| DatasetError("空文件".to_string()))?;
    let names: Vec<&str> = split_fields(header).into_iter().map(str::trim).collect();
    let find = |aliases: &[&str]| -> Result<usize, DatasetError> {
        aliases
            .iter()
            .find_map(|alias| names.iter().position(|candidate| candidate == alias))
            .ok_or_else(|| DatasetError(format!("缺少列 {aliases:?}")))
    };
    let columns = MAPPING
        .iter()
        .map(|(aliases, feature)| Ok((find(aliases)?, *feature)))
        .collect::<Result<Vec<_>, DatasetError>>()?;
    let label = find(&[LABEL_COLUMN])?;

    let mut samples = Vec::new();
    let mut rows = 0usize;
    let mut skipped = 0usize;
    let mut attacks = 0usize;
    let lines: Vec<&str> = lines.filter(|line| !line.trim().is_empty()).collect();
    // 超上限时按等距抽样，而不是截前 N 行：逐日 CSV 是按时间写的，
    // 截前缀会让样本全落在当天早段。
    let stride = if limit != 0 && lines.len() > limit {
        lines.len().div_ceil(limit)
    } else {
        1
    };
    for (position, line) in lines.iter().enumerate() {
        rows += 1;
        if limit != 0 && samples.len() >= limit {
            continue;
        }
        if position % stride != 0 {
            continue;
        }
        let Some(sample) = row(&split_fields(line), &columns, label) else {
            skipped += 1;
            continue;
        };
        if sample.label == Label::Malicious {
            attacks += 1;
        }
        samples.push(sample);
    }
    Ok(Dataset {
        samples,
        rows,
        skipped,
        attacks,
    })
}

fn row(fields: &[&str], columns: &[(usize, usize)], label: usize) -> Option<Sample> {
    let value = |index: usize| -> Option<f64> {
        let raw = fields.get(index)?.trim();
        let parsed: f64 = raw.parse().ok()?;
        parsed.is_finite().then_some(parsed)
    };
    let mut features = [0.0f64; N];
    for (column, feature) in columns {
        features[*feature] = value(*column)?;
    }
    let raw_label = fields.get(label)?.trim();
    let label = if raw_label.eq_ignore_ascii_case("BENIGN") {
        Label::Benign
    } else {
        Label::Malicious
    };
    // 单位换算：CICIDS 的时长与 IAT 是微秒。
    features[0] /= 1e6;
    for feature in &mut features[13..17] {
        *feature /= 1e3;
    }
    let total = features[1] + features[2];
    features[19] = if total > 0.0 {
        features[1] / total
    } else {
        0.0
    };
    Some(Sample { features, label })
}

fn split_fields(line: &str) -> Vec<&str> {
    let mut fields = Vec::new();
    let bytes = line.as_bytes();
    let mut start = 0usize;
    let mut index = 0usize;
    let mut quoted = false;
    while index < bytes.len() {
        match bytes[index] {
            b'"' => quoted = !quoted,
            b',' if !quoted => {
                fields.push(&line[start..index]);
                start = index + 1;
            }
            _ => {}
        }
        index += 1;
    }
    fields.push(&line[start..]);
    fields
}

/// 排序后取分位数（线性插值）。
pub fn quantile(sorted: &[f64], q: f64) -> f64 {
    if sorted.is_empty() {
        return 0.0;
    }
    let q = q.clamp(0.0, 1.0);
    let position = q * (sorted.len() - 1) as f64;
    let lower = position.floor() as usize;
    let upper = position.ceil() as usize;
    if lower == upper {
        sorted[lower]
    } else {
        let weight = position - lower as f64;
        sorted[lower] * (1.0 - weight) + sorted[upper] * weight
    }
}

/// 按特征收集某一侧（训练集）的取值并排序，供分位数使用。
pub fn sorted_by_feature(samples: &[Sample], feature: usize) -> Vec<f64> {
    let mut values: Vec<f64> = samples
        .iter()
        .map(|sample| sample.features[feature])
        .collect();
    values.sort_by(f64::total_cmp);
    values
}
