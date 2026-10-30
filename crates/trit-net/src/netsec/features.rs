//! 特征 → 三值输入。
//!
//! 每条规则给一个**死区**：落进死区的值判 `Unknown`，交给 [`crate::analyze`]
//! 枚举两种补全再下结论——这正是「证据不足就别假装知道」的落点。
//!
//! 🔴 [`DEFAULT_RULES`] 是**未校准的启发式占位**，只用来把管道跑通和让弃权语义
//! 可测。它们不是检测结论；任何精度声明都必须等 M1 用公开数据集校准后再说。

use trit_core::value::Ternary;

use super::flow::N;

/// 规则方向。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Bias {
    /// 值越大越可疑：`>= high` 判 `True`，`<= low` 判 `False`。
    High,
    /// 值越小越可疑：`<= low` 判 `True`，`>= high` 判 `False`。
    Low,
}

/// 一条阈值规则。`low < high` 必须成立，中间是死区。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rule {
    /// 特征下标，对应 [`NAMES`]。
    pub feature: usize,
    /// 下界。
    pub low: f64,
    /// 上界。
    pub high: f64,
    /// 方向。
    pub bias: Bias,
}

impl Rule {
    fn apply(&self, value: f64) -> Ternary {
        // 带宽为 0 时低上界相等，退化成硬阈值；否则等号会同时落进两侧。
        if self.high <= self.low {
            return match self.bias {
                Bias::High => {
                    if value >= self.high {
                        Ternary::True
                    } else {
                        Ternary::False
                    }
                }
                Bias::Low => {
                    if value <= self.low {
                        Ternary::True
                    } else {
                        Ternary::False
                    }
                }
            };
        }
        let (suspicious, benign) = match self.bias {
            Bias::High => (value >= self.high, value <= self.low),
            Bias::Low => (value <= self.low, value >= self.high),
        };
        if suspicious {
            Ternary::True
        } else if benign {
            Ternary::False
        } else {
            Ternary::Unknown
        }
    }
}

/// 未校准的默认规则集。见模块文档的红线。
pub const DEFAULT_RULES: [Rule; 8] = [
    Rule {
        feature: 1,
        low: 10.0,
        high: 1000.0,
        bias: Bias::High,
    },
    Rule {
        feature: 2,
        low: 1.0,
        high: 5.0,
        bias: Bias::Low,
    },
    Rule {
        feature: 17,
        low: 10.0,
        high: 1000.0,
        bias: Bias::High,
    },
    Rule {
        feature: 18,
        low: 10_000.0,
        high: 10_000_000.0,
        bias: Bias::High,
    },
    Rule {
        feature: 21,
        low: 0.0,
        high: 3.0,
        bias: Bias::High,
    },
    Rule {
        feature: 20,
        low: 1.0,
        high: 20.0,
        bias: Bias::High,
    },
    Rule {
        feature: 14,
        low: 1.0,
        high: 1000.0,
        bias: Bias::High,
    },
    Rule {
        feature: 19,
        low: 0.9,
        high: 1.0,
        bias: Bias::High,
    },
];

/// 把特征值按规则转成三值输入。
///
/// 没有被任何规则覆盖的维度固定为 `False`——这是**显式选择**：
/// 模型只该看被声明过的证据，没声明的维度不参与判断。
pub fn ternarize(values: &[f64; N], rules: &[Rule]) -> Vec<Ternary> {
    let mut inputs = vec![Ternary::False; N];
    for rule in rules {
        inputs[rule.feature] = rule.apply(values[rule.feature]);
    }
    inputs
}

/// 三值输入的单字符视图（`T`/`F`/`X`），用于台账可读化。
pub fn encode(inputs: &[Ternary]) -> String {
    inputs
        .iter()
        .map(|value| match value {
            Ternary::True => 'T',
            Ternary::False => 'F',
            Ternary::Unknown => 'X',
        })
        .collect()
}
