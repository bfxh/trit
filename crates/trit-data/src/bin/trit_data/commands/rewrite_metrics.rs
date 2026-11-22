//! `rewrite.report.json` 的聚合读数：只有 counts 看不出“重写值不值得”。
//!
//! 这里补四类：保真通过率、失败原因分桶、回门后决策分布、长度变化分位数。
//! 全部由运行期样本直接算出，不做估计，也不把 gate `reasons` 和失败原因混在一起。

use std::collections::BTreeMap;

use trit_data::script::trajectory_text;
use trit_data::schema::Trajectory;
use trit_learn::json::Json;

use super::Counts;

#[derive(Default)]
pub struct Metrics {
    failure_reasons: BTreeMap<String, usize>,
    /// 每条成功重写轨迹的 `重写后字符数 / 原字符数`。
    ratios: Vec<f64>,
}

impl Metrics {
    pub fn fail(&mut self, reason: &str) {
        *self.failure_reasons.entry(reason.to_string()).or_insert(0) += 1;
    }

    pub fn observe(&mut self, before: &Trajectory, after: &Trajectory) {
        let a = trajectory_text(before).chars().count();
        let b = trajectory_text(after).chars().count();
        if a > 0 {
            self.ratios.push(b as f64 / a as f64);
        }
    }

    pub fn print(
        &self,
        counts: &Counts,
        gate_reasons: &BTreeMap<&'static str, usize>,
        path: Option<&std::path::Path>,
    ) {
        println!(
            "重写 {}：成功 {}（accept {} / defer {} / reject {}）/ 保真失败 {} / 调用失败 {}",
            counts.records,
            counts.rewritten,
            counts.accepted,
            counts.deferred,
            counts.rejected,
            counts.fidelity_failed,
            counts.failed
        );
        let attempted = counts.rewritten + counts.fidelity_failed;
        let rate = ratio(counts.rewritten, attempted);
        println!("保真通过率 {rate}");
        if !self.failure_reasons.is_empty() {
            let reasons = bucket(&self.failure_reasons);
            println!("失败原因：{reasons}");
        }
        if let Some(p50) = percentile(&mut self.sorted(), 0.5) {
            println!(
                "长度变化 p10={:.4} p50={:.4} p90={:.4}",
                percentile(&mut self.sorted(), 0.1).unwrap_or(p50),
                p50,
                percentile(&mut self.sorted(), 0.9).unwrap_or(p50)
            );
        }
        if !gate_reasons.is_empty() {
            let reasons: Vec<String> = gate_reasons
                .iter()
                .map(|(k, v)| format!("{k}={v}"))
                .collect();
            println!("回门原因：{}", reasons.join(", "));
        }
        if let Some(path) = path {
            println!("报告 {}", path.display());
        }
    }

    pub fn json(&self, counts: &Counts) -> Json {
        let attempted = counts.rewritten + counts.fidelity_failed;
        let gate_total = counts.accepted + counts.deferred + counts.rejected;
        let mut sorted = self.sorted();
        let p50 = percentile(&mut sorted, 0.5);
        let p10 = percentile(&mut sorted, 0.1);
        let p90 = percentile(&mut sorted, 0.9);
        let mut failures = Vec::new();
        for (reason, count) in &self.failure_reasons {
            failures.push(
                Json::obj()
                    .put("reason", reason.clone())
                    .put("count", *count as u64),
            );
        }
        Json::obj()
            .put(
                "fidelity",
                Json::obj()
                    .put("attempted", attempted as u64)
                    .put("passed", counts.rewritten as u64)
                    .put("rate", num_s(frac(counts.rewritten, attempted))),
            )
            .put(
                "gate_decisions",
                Json::obj()
                    .put("accept", counts.accepted as u64)
                    .put("defer", counts.deferred as u64)
                    .put("reject", counts.rejected as u64)
                    .put("accept_share", num_s(frac(counts.accepted, gate_total)))
                    .put("defer_share", num_s(frac(counts.deferred, gate_total))),
            )
            .put(
                "length_ratio",
                Json::obj()
                    .put("p10", p10.map(num_f).unwrap_or(Json::Null))
                    .put("p50", p50.map(num_f).unwrap_or(Json::Null))
                    .put("p90", p90.map(num_f).unwrap_or(Json::Null))
                    .put("samples", sorted.len() as u64),
            )
            .put("failure_reasons", Json::Arr(failures))
    }

    fn sorted(&self) -> Vec<f64> {
        let mut sorted = self.ratios.clone();
        sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        sorted
    }
}

fn percentile(sorted: &mut [f64], q: f64) -> Option<f64> {
    if sorted.is_empty() {
        return None;
    }
    let pos = q * (sorted.len() - 1) as f64;
    let lower = pos.floor() as usize;
    let upper = pos.ceil() as usize;
    if lower == upper {
        return Some(sorted[lower]);
    }
    let weight = pos - lower as f64;
    Some(sorted[lower] * (1.0 - weight) + sorted[upper] * weight)
}

/// 比例转百分比字符串，固定三位小数；`0/0` 记 0。
fn ratio(part: usize, total: usize) -> String {
    if total == 0 {
        "0.000%".to_string()
    } else {
        format!("{:.3}%", part as f64 / total as f64 * 100.0)
    }
}

fn num_s(raw: String) -> Json {
    Json::Num(raw)
}

fn num_f(value: f64) -> Json {
    Json::Num(format!("{value:.6}"))
}

/// 通过率用小数值（0..1），固定六位；`0/0` 记 0。
fn frac(part: usize, total: usize) -> String {
    if total == 0 {
        "0.000000".to_string()
    } else {
        format!("{:.6}", part as f64 / total as f64)
    }
}

fn bucket(reasons: &BTreeMap<String, usize>) -> String {
    reasons
        .iter()
        .map(|(k, v)| format!("{k}={v}"))
        .collect::<Vec<_>>()
        .join(", ")
}
