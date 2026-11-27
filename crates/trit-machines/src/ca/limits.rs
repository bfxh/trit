//! 元胞演化的资源预算：在分配历史/开跑之前拒绝超限输入。
//!
//! 历史是 `width × steps` 个格子的快照，周期检测又是 `width × steps²` 量级的
//! 最坏比较；`survey` 还要把单条规则的代价乘 2187。三个维度分别设上界，
//! 避免入口把一次误参数变成 OOM 或分钟级卡死。

use std::fmt;

/// 单行格子数上限（同时也是单次历史快照的宽度上限）。
pub const MAX_WIDTH: usize = 16 * 1024 * 1024;
/// 单次演化的历史格子预算（`width × steps`）。
pub const MAX_HISTORY_CELLS: usize = 16 * 1024 * 1024;
/// 单次演化的步数上限。
pub const MAX_STEPS: usize = 10_000;
/// 单条规则周期扫描的最坏比较预算（`width × steps²`）。
pub const MAX_PERIOD_WORK: u64 = 1_000_000_000;
/// 整片规则空间（2187 条）的周期扫描总预算。
pub const MAX_SURVEY_WORK: u64 = 12_000_000_000;

/// 规则数与一条规则的扫描代价相乘，得到整片 survey 的代价。
const RULES: u64 = 2187;

/// 元胞入口的结构化错误。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CaError {
    /// 规模超出预算。
    Limit(String),
}

impl fmt::Display for CaError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Limit(message) => write!(formatter, "规模超限：{message}"),
        }
    }
}

impl std::error::Error for CaError {}

impl From<CaError> for String {
    fn from(error: CaError) -> Self {
        error.to_string()
    }
}

/// 单条规则的预算检查；必须在 `vec![...; width]` 之前调用。
pub fn check(width: usize, steps: usize) -> Result<(), CaError> {
    if width > MAX_WIDTH {
        return Err(CaError::Limit(format!(
            "width {width} 超过单行上限 {MAX_WIDTH}"
        )));
    }
    if steps > MAX_STEPS {
        return Err(CaError::Limit(format!(
            "steps {steps} 超过上限 {MAX_STEPS}"
        )));
    }
    let cells = width
        .checked_mul(steps)
        .ok_or_else(|| CaError::Limit(format!("width {width} × steps {steps} 溢出 usize")))?;
    if cells > MAX_HISTORY_CELLS {
        return Err(CaError::Limit(format!(
            "width {width} × steps {steps} = {cells} 超过历史预算 {MAX_HISTORY_CELLS}"
        )));
    }
    let work = (cells as u64) * (steps as u64);
    if work > MAX_PERIOD_WORK {
        return Err(CaError::Limit(format!(
            "width {width} × steps² {steps} = {work} 超过周期扫描预算 {MAX_PERIOD_WORK}"
        )));
    }
    Ok(())
}

/// 整片 survey 的预算检查；单条规则合法不代表 2187 条都跑得起。
pub fn check_survey(width: usize, steps: usize) -> Result<(), CaError> {
    check(width, steps)?;
    let work = (width as u64) * (steps as u64) * (steps as u64) * RULES;
    if work > MAX_SURVEY_WORK {
        return Err(CaError::Limit(format!(
            "survey width {width} × steps² {steps} × {RULES} = {work} 超过总预算 {MAX_SURVEY_WORK}"
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn budgets_reject_before_allocation() {
        assert!(check(MAX_WIDTH + 1, 1).is_err());
        assert!(check(1, MAX_STEPS + 1).is_err());
        assert!(check(64, 10_000).is_err());
        assert!(check(64, 200).is_ok());
        assert!(check_survey(64, 200).is_ok());
        assert!(check_survey(256, 200).is_err());
    }
}
