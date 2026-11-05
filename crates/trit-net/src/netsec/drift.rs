//! 漂移门（M1 下一步第 3 条）：校准读数的机械对账。
//!
//! M1.6 的跨日读数照实记录了「换一天就基本失效」——但不能只靠人记得这件事，
//! 漂移必须变成**会红的门**：把一份基线校准读数（`calibrate --json` 的输出）
//! 与当前读数按 band 逐条对账，三个方向任一变坏超过阈值即红：
//!
//! - **FPR 上升**超过 `max_fpr_delta`（默认 0.1）——误报面恶化；
//! - **TPR 下降**超过 `max_tpr_delta`（默认 0.1）——检出面恶化；
//! - **弃权上升**超过 `max_abstain_delta`（默认 0.2）——覆盖面塌方
//!   （弃权暴涨是另一种"变坏"：模型不敢判了）。
//!
//! fail-closed 纪律：**缺 band、多 band、坏 JSON、阈值非法一律红**——
//! 对不上账的"绿"没有任何含义（与变异门逐目标 fail-closed 同一哲学）。
//! 基线是账：基线 JSON 随仓提交，重跑校准后与本门对账；基线变更必须
//! 显式重生成并在提交说明里给出理由。
//!
//! 边界（不猜）：本门只做**读数对账**，不判定漂移的原因（数据分布漂移、
//! 攻击类型不同、还是代码回归）；原因归因是人的工作，门只负责不许悄悄变坏。

use serde::Deserialize;

/// 校准报告里本门消费的字段（`calibrate --json` 的 `bands` 数组元素）。
#[derive(Debug, Clone, Copy, PartialEq, Deserialize)]
pub struct BandReading {
    /// 弃权带宽度（对账键）。
    pub band: f64,
    /// 误报率。
    pub fpr: f64,
    /// 检出率。
    pub tpr: f64,
    /// 弃权率。
    pub abstain: f64,
}

/// 整份校准读数（只取 `bands`，其余字段忽略——向前兼容）。
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct CalibrationReport {
    /// 逐 band 读数。
    pub bands: Vec<BandReading>,
}

/// 阈值配置；三个方向各自独立，负值/NaN 一律拒绝（fail-closed）。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DriftLimits {
    /// FPR 允许的最大上升。
    pub max_fpr_delta: f64,
    /// TPR 允许的最大下降。
    pub max_tpr_delta: f64,
    /// 弃权允许的最大上升。
    pub max_abstain_delta: f64,
}

impl Default for DriftLimits {
    fn default() -> Self {
        Self {
            max_fpr_delta: 0.1,
            max_tpr_delta: 0.1,
            max_abstain_delta: 0.2,
        }
    }
}

/// 单个 band 的对账结果。
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct BandDelta {
    /// 弃权带宽度。
    pub band: f64,
    /// 当前 FPR − 基线 FPR（正 = 变坏）。
    pub dfpr: f64,
    /// 当前 TPR − 基线 TPR（负 = 变坏）。
    pub dtpr: f64,
    /// 当前弃权 − 基线弃权（正 = 变坏）。
    pub dabstain: f64,
    /// 触红的理由（空 = 该 band 绿）。
    pub reasons: Vec<String>,
}

/// 漂移门的完整判决。
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct DriftVerdict {
    /// 总判决：`green` / `red`。
    pub verdict: &'static str,
    /// 逐 band 对账（按 band 升序）。
    pub bands: Vec<BandDelta>,
    /// 结构性红（缺 band / 多 band / 解析失败 / 阈值非法）的理由。
    pub structural: Vec<String>,
}

use serde::Serialize;

/// 解析校准 JSON；只要求 `bands` 数组存在且每项带 band/fpr/tpr/abstain。
pub fn parse_report(text: &str) -> Result<CalibrationReport, String> {
    let report: CalibrationReport =
        serde_json::from_str(text).map_err(|error| format!("校准 JSON 解析失败：{error}"))?;
    for reading in &report.bands {
        if !(reading.fpr.is_finite()
            && reading.tpr.is_finite()
            && reading.abstain.is_finite()
            && reading.band.is_finite())
        {
            return Err(format!(
                "band {} 的读数含非有限值（fpr/tpr/abstain 必须是有限数）",
                reading.band
            ));
        }
    }
    Ok(report)
}

/// 对账：逐 band 比较，结构不对齐即红。
pub fn drift_check(
    baseline: &CalibrationReport,
    current: &CalibrationReport,
    limits: DriftLimits,
) -> DriftVerdict {
    let mut structural = Vec::new();
    if limits.max_fpr_delta < 0.0
        || limits.max_tpr_delta < 0.0
        || limits.max_abstain_delta < 0.0
        || !limits.max_fpr_delta.is_finite()
        || !limits.max_tpr_delta.is_finite()
        || !limits.max_abstain_delta.is_finite()
    {
        structural.push("阈值为负或非有限——漂移门不猜，拒绝运行".to_string());
    }
    // band 匹配按数值精确相等（两份报告来自同一套 band 列表字面量）。
    let find = |report: &CalibrationReport, band: f64| {
        report
            .bands
            .iter()
            .find(|reading| reading.band == band)
            .copied()
    };
    let mut bands: Vec<BandDelta> = Vec::new();
    for base in &baseline.bands {
        match find(current, base.band) {
            Some(now) => {
                let dfpr = now.fpr - base.fpr;
                let dtpr = now.tpr - base.tpr;
                let dabstain = now.abstain - base.abstain;
                let mut reasons = Vec::new();
                if dfpr > limits.max_fpr_delta {
                    reasons.push(format!("FPR 上升 {dfpr:+.4} > {}", limits.max_fpr_delta));
                }
                if dtpr < -limits.max_tpr_delta {
                    reasons.push(format!("TPR 下降 {dtpr:+.4} > {}", limits.max_tpr_delta));
                }
                if dabstain > limits.max_abstain_delta {
                    reasons.push(format!(
                        "弃权上升 {dabstain:+.4} > {}",
                        limits.max_abstain_delta
                    ));
                }
                bands.push(BandDelta {
                    band: base.band,
                    dfpr,
                    dtpr,
                    dabstain,
                    reasons,
                });
            }
            None => structural.push(format!(
                "基线 band {} 在当前读数里缺失——fail-closed",
                base.band
            )),
        }
    }
    for now in &current.bands {
        if find(baseline, now.band).is_none() {
            structural.push(format!(
                "当前读数出现基线没有的 band {}——配置漂移，须显式对齐",
                now.band
            ));
        }
    }
    let red = !structural.is_empty() || bands.iter().any(|band| !band.reasons.is_empty());
    DriftVerdict {
        verdict: if red { "red" } else { "green" },
        bands,
        structural,
    }
}
