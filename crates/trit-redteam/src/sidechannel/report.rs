use std::collections::BTreeMap;

/// 侧信道分析结果：各类别的熵、条件熵与泄漏量（bit）。
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
