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
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct BandReading {
    /// 弃权带宽度（对账键）。
    pub band: f64,
    /// 误报率。
    pub fpr: f64,
    /// 检出率。
    pub tpr: f64,
    /// 弃权率。
    pub abstain: f64,
    /// 逐特征三值分布（`[False, Unknown, True]` 占比）。旧基线缺这个字段时
    /// 逐特征归因不可用，只做 band 级读数对账（见 [`DriftVerdict::feature_attribution`]）。
    #[serde(default)]
    pub feature_hist: Option<Vec<[f64; 3]>>,
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
    /// 单特征三值占比允许的最大变化（逐特征归因阈值）。
    pub max_feature_delta: f64,
    /// 是否要求基线必须带逐特征读数（`true` 时缺失即红；默认 `false` 仅标记不可用）。
    pub require_feature_hist: bool,
}

impl Default for DriftLimits {
    fn default() -> Self {
        Self {
            max_fpr_delta: 0.1,
            max_tpr_delta: 0.1,
            max_abstain_delta: 0.2,
            max_feature_delta: 0.1,
            require_feature_hist: false,
        }
    }
}

/// 单个特征的漂移条目（逐特征归因的输出）。
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct FeatureShift {
    /// 特征下标（对应 `flow::NAMES`）。
    pub feature: usize,
    /// 三值状态：`F` / `X` / `T`。
    pub state: &'static str,
    /// 当前占比 − 基线占比（绝对值超阈值才记录）。
    pub delta: f64,
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
    /// 逐特征超阈值的漂移（空 = 无特征级漂移；基线缺读数时也为空）。
    pub feature_shifts: Vec<FeatureShift>,
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
    /// 逐特征归因可用性：`ok`（基线与当前都带逐特征读数）/
    /// `unavailable`（基线或当前缺 `feature_hist`，只做 band 级对账）。
    pub feature_attribution: &'static str,
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
        || limits.max_feature_delta < 0.0
        || !limits.max_fpr_delta.is_finite()
        || !limits.max_tpr_delta.is_finite()
        || !limits.max_abstain_delta.is_finite()
        || !limits.max_feature_delta.is_finite()
    {
        structural.push("阈值为负或非有限——漂移门不猜，拒绝运行".to_string());
    }
    // band 匹配按数值精确相等（两份报告来自同一套 band 列表字面量）。
    let find = |report: &CalibrationReport, band: f64| {
        report
            .bands
            .iter()
            .find(|reading| reading.band == band)
            .cloned()
    };
    let mut attribution_available = true;
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
                // 逐特征归因：三值占比的绝对变化超阈值即记录并判红。
                let mut feature_shifts = Vec::new();
                match (&base.feature_hist, &now.feature_hist) {
                    (Some(b), Some(c)) if b.len() != c.len() => {
                        structural.push(format!(
                            "band {} 的逐特征维数不一致（基线 {} vs 当前 {}）——fail-closed",
                            base.band,
                            b.len(),
                            c.len()
                        ));
                        attribution_available = false;
                    }
                    (Some(b), Some(c)) => {
                        const STATES: [&str; 3] = ["F", "X", "T"];
                        for (feature, (before, after)) in b.iter().zip(c).enumerate() {
                            for k in 0..3 {
                                let delta = after[k] - before[k];
                                if delta.abs() > limits.max_feature_delta {
                                    feature_shifts.push(FeatureShift {
                                        feature,
                                        state: STATES[k],
                                        delta,
                                    });
                                }
                            }
                        }
                        for shift in &feature_shifts {
                            reasons.push(format!(
                                "特征 #{} 的 {} 态占比漂移 {:+.3} > {}",
                                shift.feature, shift.state, shift.delta, limits.max_feature_delta
                            ));
                        }
                    }
                    _ => {
                        attribution_available = false;
                        if limits.require_feature_hist {
                            structural.push(format!(
                                "band {} 缺逐特征读数而 --require-feature-hist 已开启——fail-closed",
                                base.band
                            ));
                        }
                    }
                }
                bands.push(BandDelta {
                    band: base.band,
                    dfpr,
                    dtpr,
                    dabstain,
                    reasons,
                    feature_shifts,
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
        feature_attribution: if attribution_available {
            "ok"
        } else {
            "unavailable"
        },
    }
}
