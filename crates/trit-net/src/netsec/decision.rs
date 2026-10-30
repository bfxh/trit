//! 三值判决：`analyze` 的结果映射成 `malicious / benign / undetermined`。

use trit_core::value::Ternary;

use crate::{Model, ModelError, Outcome, Sign, analyze};

/// 一条流的判决。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    /// 所有补全下输出都是 `+1`。
    Malicious,
    /// 所有补全下输出都是 `-1`。
    Benign,
    /// 证据不足、符号冲突，或输出神经元本身弃权。
    Undetermined,
}

impl Verdict {
    /// 稳定字符串。
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Malicious => "malicious",
            Self::Benign => "benign",
            Self::Undetermined => "undetermined",
        }
    }
}

/// 判决 + 为什么。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Decision {
    /// 结论。
    pub verdict: Verdict,
    /// 判定依据（稳定字符串，进 JSON）。
    pub reason: &'static str,
    /// 是否真的枚举了全部补全。
    pub exact: bool,
    /// 补全数；`BudgetExceeded` 时为 0。
    pub completions: usize,
    /// 取值为 `X` 的输入下标。
    pub unknown_inputs: Vec<usize>,
}

/// 单输出模型的三值判决。
///
/// 预算不足时**不降级成猜测**：结论保持 `Undetermined`，由 `reason` 说明原因。
pub fn decide(model: &Model, inputs: &[Ternary], budget: usize) -> Result<Decision, ModelError> {
    if model.outputs() != 1 {
        return Err(ModelError::Shape {
            layer: model.layers.len().saturating_sub(1),
            message: format!("判决模型必须恰好 1 个输出，实际 {}", model.outputs()),
        });
    }
    let analysis = analyze(model, inputs, budget)?;
    let (verdict, reason) = match analysis.outputs.first() {
        Some(Outcome::Definite(Sign::Pos)) => (Verdict::Malicious, "definite-positive"),
        Some(Outcome::Definite(Sign::Neg)) => (Verdict::Benign, "definite-negative"),
        Some(Outcome::Definite(Sign::Zero)) => (Verdict::Undetermined, "output-zero"),
        Some(Outcome::Ambiguous { .. }) => (Verdict::Undetermined, "ambiguous"),
        Some(Outcome::BudgetExceeded) => (Verdict::Undetermined, "budget-exceeded"),
        None => (Verdict::Undetermined, "no-output"),
    };
    Ok(Decision {
        verdict,
        reason,
        exact: analysis.exact,
        completions: analysis.completions,
        unknown_inputs: analysis.unknown_inputs,
    })
}
