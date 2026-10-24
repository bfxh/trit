//! 三值前向与「确定 / 弃权」解析。
//!
//! 权重是 `{-1,0,+1}`（量化的三值），信号是逻辑三值 `{F, X, T}`。输入含 `X` 时：
//!
//! - **精确解析**：枚举所有补全，收集每个输出可能取到的符号；只有一个符号才算「确定」，
//!   否则给出两个见证（让它为 `+1` 的补全、让它为 `-1` 的补全）。
//! - **保守区间**：逐层 min/max，不看层间相关性。它给出的是「实在算不出来」的下界，
//!   实测会把绝大多数本可确定的样本判成弃权——所以它只是兜底。

use trit_core::value::Ternary;

use crate::model::{Model, ModelError};

/// 精确解析的默认枚举上限。
pub const DEFAULT_BUDGET: usize = 4096;

/// 神经元输出符号；`Zero` 表示该神经元弃权。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Sign {
    /// `-1`。
    Neg,
    /// `0`。
    Zero,
    /// `+1`。
    Pos,
}

impl Sign {
    /// 单字符表示。
    pub const fn as_char(self) -> char {
        match self {
            Self::Neg => '-',
            Self::Zero => '0',
            Self::Pos => '+',
        }
    }
}

/// 让输出取到某个符号的一组 `X` 输入补全（按 `Analysis::unknown_inputs` 的顺序）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Witness {
    /// 每个未知输入取 `False` 或 `True`。
    pub values: Vec<Ternary>,
}

/// 单个输出的判定。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// 所有补全下输出都相同。
    Definite(Sign),
    /// 不同补全会给出不同符号；`pos` / `neg` 是两组见证（可能只存在一侧）。
    Ambiguous {
        /// 让输出为 `+1` 的补全（若存在）。
        pos: Option<Witness>,
        /// 让输出为 `-1` 的补全（若存在）。
        neg: Option<Witness>,
    },
    /// 未知输入太多，超过枚举预算，没做精确解析。
    BudgetExceeded,
}

/// 一次分析的结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Analysis {
    /// 是否真的枚举了所有补全。
    pub exact: bool,
    /// 补全数；超预算时为 0。
    pub completions: usize,
    /// 枚举预算。
    pub budget: usize,
    /// 取值为 `X` 的输入下标（升序）。
    pub unknown_inputs: Vec<usize>,
    /// 每个输出的判定。
    pub outputs: Vec<Outcome>,
    /// 保守区间给出的结论（`None` = 区间跨过多个符号）。
    pub conservative: Vec<Option<Sign>>,
}

impl Analysis {
    /// 精确解析里被判为确定的输出数。
    pub fn definite_count(&self) -> usize {
        self.outputs
            .iter()
            .filter(|outcome| matches!(outcome, Outcome::Definite(_)))
            .count()
    }

    /// 保守区间给出的确定输出数（用来对比悲观程度）。
    pub fn conservative_definite_count(&self) -> usize {
        self.conservative
            .iter()
            .filter(|value| value.is_some())
            .count()
    }
}

fn as_offset(value: Ternary) -> i32 {
    match value {
        Ternary::False => -1,
        Ternary::Unknown => 0,
        Ternary::True => 1,
    }
}

/// 符号函数：`<0 → -1`、`0 → 0`、`>0 → 1`。两次比较合成，无数据分支。
fn sign(value: i32) -> i32 {
    (value > 0) as i32 - (value < 0) as i32
}

fn from_offset(value: i32) -> Sign {
    match sign(value) {
        -1 => Sign::Neg,
        0 => Sign::Zero,
        _ => Sign::Pos,
    }
}

/// 完整赋值下的前向（不允许 `X`；有 `X` 会被当作 `0`，请用 [`analyze`]）。
///
/// 字段是公开的——调用方手搓的模型**在此处强制校验**（H09：不合法形状
/// 返回结构化错误，绝不靠索引 panic）。
pub fn forward(model: &Model, inputs: &[Ternary]) -> Result<Vec<Sign>, ModelError> {
    model.validate()?;
    if inputs.len() != model.inputs() {
        return Err(ModelError::Shape {
            layer: 0,
            message: format!("输入维数应为 {}，实际 {}", model.inputs(), inputs.len()),
        });
    }
    let mut values: Vec<i32> = inputs.iter().map(|value| as_offset(*value)).collect();
    for layer in &model.layers {
        let mut next = Vec::with_capacity(layer.outputs);
        for output in 0..layer.outputs {
            let mut acc = i32::from(layer.bias[output]);
            for (input, value) in values.iter().enumerate() {
                acc += i32::from(layer.weights[output][input]) * *value;
            }
            next.push(sign(acc));
        }
        values = next;
    }
    Ok(values.into_iter().map(from_offset).collect())
}

/// 逐层 min/max 的保守区间（忽略层间相关性）。
fn conservative(model: &Model, inputs: &[Ternary]) -> Result<Vec<Option<Sign>>, ModelError> {
    let mut lows: Vec<i32> = Vec::with_capacity(inputs.len());
    let mut highs: Vec<i32> = Vec::with_capacity(inputs.len());
    for value in inputs {
        match value {
            Ternary::Unknown => {
                lows.push(-1);
                highs.push(1);
            }
            known => {
                lows.push(as_offset(*known));
                highs.push(as_offset(*known));
            }
        }
    }
    for layer in &model.layers {
        let mut next_lows = Vec::with_capacity(layer.outputs);
        let mut next_highs = Vec::with_capacity(layer.outputs);
        for output in 0..layer.outputs {
            let mut low = i32::from(layer.bias[output]);
            let mut high = i32::from(layer.bias[output]);
            for input in 0..layer.inputs {
                // `w >= 0` 时贡献区间是 `[w*lows, w*highs]`，`w < 0` 时端点翻转。
                // min/max 各取一次代替按权重符号分支——随机权重对分支预测器是噪声。
                let low_term = i32::from(layer.weights[output][input]) * lows[input];
                let high_term = i32::from(layer.weights[output][input]) * highs[input];
                low += low_term.min(high_term);
                high += low_term.max(high_term);
            }
            // 区间压缩成符号：`sign(low)` / `sign(high)`（原 neg/zero/pos 三连分支
            // 里 `zero_possible` 对 `sign` 是冗余的——`high >= low` 恒成立）。
            next_lows.push(sign(low));
            next_highs.push(sign(high));
        }
        lows = next_lows;
        highs = next_highs;
    }
    Ok(lows
        .iter()
        .zip(highs.iter())
        .map(|(low, high)| {
            if low == high {
                Some(from_offset(*low))
            } else {
                None
            }
        })
        .collect())
}

/// 解析含 `X` 输入的一次推理。
pub fn analyze(model: &Model, inputs: &[Ternary], budget: usize) -> Result<Analysis, ModelError> {
    model.validate()?;
    if inputs.len() != model.inputs() {
        return Err(ModelError::Shape {
            layer: 0,
            message: format!("输入维数应为 {}，实际 {}", model.inputs(), inputs.len()),
        });
    }
    let unknown_inputs: Vec<usize> = inputs
        .iter()
        .enumerate()
        .filter(|(_, value)| value.is_unknown())
        .map(|(index, _)| index)
        .collect();
    let conservative = conservative(model, inputs)?;

    let total = 1usize
        .checked_shl(unknown_inputs.len() as u32)
        .filter(|total| *total <= budget);
    let Some(total) = total else {
        return Ok(Analysis {
            exact: false,
            completions: 0,
            budget,
            unknown_inputs,
            outputs: vec![Outcome::BudgetExceeded; model.outputs()],
            conservative,
        });
    };

    // 每个输出的符号位图：Neg = 1，Zero = 2，Pos = 4。
    let mut seen = vec![0u8; model.outputs()];
    let mut pos_witness: Vec<Option<Witness>> = vec![None; model.outputs()];
    let mut neg_witness: Vec<Option<Witness>> = vec![None; model.outputs()];
    let mut completion = inputs.to_vec();
    for mask in 0..total {
        for (bit, index) in unknown_inputs.iter().enumerate() {
            completion[*index] = if (mask >> bit) & 1 == 1 {
                Ternary::True
            } else {
                Ternary::False
            };
        }
        let signs = forward(model, &completion)?;
        let witness = Witness {
            values: unknown_inputs
                .iter()
                .map(|index| completion[*index])
                .collect(),
        };
        for (output, sign) in signs.into_iter().enumerate() {
            let bit = match sign {
                Sign::Neg => 1u8,
                Sign::Zero => 2u8,
                Sign::Pos => 4u8,
            };
            seen[output] |= bit;
            match sign {
                Sign::Pos if pos_witness[output].is_none() => {
                    pos_witness[output] = Some(witness.clone());
                }
                Sign::Neg if neg_witness[output].is_none() => {
                    neg_witness[output] = Some(witness.clone());
                }
                _ => {}
            }
        }
    }

    let outputs = seen
        .into_iter()
        .enumerate()
        .map(|(output, bits)| match bits {
            1 => Outcome::Definite(Sign::Neg),
            2 => Outcome::Definite(Sign::Zero),
            4 => Outcome::Definite(Sign::Pos),
            _ => Outcome::Ambiguous {
                pos: pos_witness[output].clone(),
                neg: neg_witness[output].clone(),
            },
        })
        .collect();

    Ok(Analysis {
        exact: true,
        completions: total,
        budget,
        unknown_inputs,
        outputs,
        conservative,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Layer, Model};

    fn model(json: &str) -> Model {
        Model::from_json(json).unwrap()
    }

    /// 单层：out = sign(x0 + x1)。
    fn sum_model() -> Model {
        model(r#"{"layers":[{"inputs":2,"outputs":1,"weights":[[1,1]],"bias":[0]}]}"#)
    }

    #[test]
    fn binary_forward_is_sign_of_weighted_sum() {
        let model = sum_model();
        assert_eq!(
            forward(&model, &[Ternary::False, Ternary::False]).unwrap(),
            vec![Sign::Neg]
        );
        assert_eq!(
            forward(&model, &[Ternary::True, Ternary::False]).unwrap(),
            vec![Sign::Zero]
        );
        assert_eq!(
            forward(&model, &[Ternary::True, Ternary::True]).unwrap(),
            vec![Sign::Pos]
        );
    }

    #[test]
    fn hand_built_invalid_model_is_rejected_instead_of_panicking() {
        // 复审 H09：outputs=1 但 weights=[] 曾让 forward 在 weights[0] 处 panic。
        let broken = Model {
            layers: vec![Layer {
                inputs: 1,
                outputs: 1,
                weights: Vec::new(),
                bias: vec![0],
            }],
        };
        assert!(matches!(
            forward(&broken, &[Ternary::True]),
            Err(ModelError::Shape { layer: 0, .. })
        ));
        assert!(matches!(
            analyze(&broken, &[Ternary::True], 4),
            Err(ModelError::Shape { layer: 0, .. })
        ));
        // 层间不衔接同样在入口拒绝，而不是在第二层索引错位。
        let disconnected = Model {
            layers: vec![
                Layer {
                    inputs: 2,
                    outputs: 2,
                    weights: vec![vec![1, 0], vec![0, 1]],
                    bias: vec![0, 0],
                },
                Layer {
                    inputs: 3,
                    outputs: 1,
                    weights: vec![vec![1, 0, 0]],
                    bias: vec![0],
                },
            ],
        };
        assert!(matches!(
            forward(&disconnected, &[Ternary::True, Ternary::True]),
            Err(ModelError::Shape { layer: 1, .. })
        ));
    }

    #[test]
    fn ambiguous_output_carries_both_witnesses() {
        let model = sum_model();
        let analysis = analyze(&model, &[Ternary::Unknown, Ternary::Unknown], 16).unwrap();
        assert!(analysis.exact);
        assert_eq!(analysis.completions, 4);
        assert_eq!(analysis.unknown_inputs, vec![0, 1]);
        let Outcome::Ambiguous { pos, neg } = &analysis.outputs[0] else {
            panic!("期望未定，实际 {:?}", analysis.outputs[0]);
        };
        let pos = pos.as_ref().expect("应有 +1 见证");
        let neg = neg.as_ref().expect("应有 -1 见证");
        assert_eq!(pos.values, vec![Ternary::True, Ternary::True]);
        assert_eq!(neg.values, vec![Ternary::False, Ternary::False]);
        for (witness, expected) in [(pos, Sign::Pos), (neg, Sign::Neg)] {
            let mut inputs = vec![Ternary::Unknown; 2];
            for (slot, value) in inputs.iter_mut().zip(witness.values.iter()) {
                *slot = *value;
            }
            assert_eq!(forward(&model, &inputs).unwrap()[0], expected);
        }
    }

    #[test]
    fn independent_unknown_does_not_block_a_definite_output() {
        // 输出只看 x0（权重 1,0），x1 未知也不影响。
        let model = model(r#"{"layers":[{"inputs":2,"outputs":1,"weights":[[1,0]],"bias":[0]}]}"#);
        let analysis = analyze(&model, &[Ternary::True, Ternary::Unknown], 16).unwrap();
        assert_eq!(analysis.outputs[0], Outcome::Definite(Sign::Pos));
        assert_eq!(analysis.conservative[0], Some(Sign::Pos));
    }

    #[test]
    fn cancellation_is_exact_but_interval_is_blind() {
        // h0 = sign(x), h1 = sign(-x)，输出取 h0 + h1 恒为 0。
        let model = model(
            r#"{"layers":[
                {"inputs":1,"outputs":2,"weights":[[1],[-1]],"bias":[0,0]},
                {"inputs":2,"outputs":1,"weights":[[1,1]],"bias":[0]}
            ]}"#,
        );
        let analysis = analyze(&model, &[Ternary::Unknown], 16).unwrap();
        assert_eq!(analysis.outputs[0], Outcome::Definite(Sign::Zero));
        assert_eq!(
            analysis.conservative[0], None,
            "区间应看不见相关性，给出弃权"
        );
        assert_eq!(analysis.definite_count(), 1);
        assert_eq!(analysis.conservative_definite_count(), 0);
    }

    #[test]
    fn budget_is_inclusive() {
        // 4 个补全、预算正好 4：必须真的枚举，而不是差一判成超预算。
        let model = sum_model();
        let analysis = analyze(&model, &[Ternary::Unknown, Ternary::Unknown], 4).unwrap();
        assert!(analysis.exact);
        assert_eq!(analysis.completions, 4);
    }

    #[test]
    fn over_budget_reports_exceeded_without_lying() {
        let model = sum_model();
        let analysis = analyze(&model, &[Ternary::Unknown, Ternary::Unknown], 3).unwrap();
        assert!(!analysis.exact);
        assert_eq!(analysis.completions, 0);
        assert_eq!(analysis.outputs[0], Outcome::BudgetExceeded);
    }

    #[test]
    fn wrong_input_count_is_rejected() {
        let model = sum_model();
        assert!(analyze(&model, &[Ternary::True], 16).is_err());
        assert!(forward(&model, &[Ternary::True]).is_err());
    }

    #[test]
    fn model_validation_rejects_bad_weights() {
        assert!(Model::from_json(r#"{"layers":[]}"#).is_err());
        assert!(
            Model::from_json(r#"{"layers":[{"inputs":1,"outputs":1,"weights":[[2]],"bias":[0]}]}"#)
                .is_err()
        );
        assert!(
            Model::from_json(
                r#"{"layers":[
                    {"inputs":2,"outputs":1,"weights":[[1,1]],"bias":[0]},
                    {"inputs":3,"outputs":1,"weights":[[1,1,1]],"bias":[0]}
                ]}"#
            )
            .is_err()
        );
    }

    #[test]
    fn negative_weight_interval_spans_for_unknown_input() {
        // w=-1、输入 X：端点项是 (w*lows, w*highs) = (+1, -1)，区间 [-1,+1] 应弃权。
        // 下界不取端点最小值（或取错端点）会把区间折成单点。
        let model = model(r#"{"layers":[{"inputs":1,"outputs":1,"weights":[[-1]],"bias":[0]}]}"#);
        let analysis = analyze(&model, &[Ternary::Unknown], 4).unwrap();
        assert_eq!(analysis.conservative[0], None);
    }

    #[test]
    fn conservative_interval_never_contradicts_exact() {
        // 随机小网络对拍：精确枚举判「确定」的输出，保守区间要么同号、要么弃权。
        // 区间只许比真值宽——min/max 端点取反、符号压缩写错都会在这里翻车。
        struct Rng(u64);
        impl Rng {
            fn next(&mut self) -> u64 {
                self.0 ^= self.0 << 13;
                self.0 ^= self.0 >> 7;
                self.0 ^= self.0 << 17;
                self.0
            }
        }
        let layer = |rng: &mut Rng, inputs: usize, outputs: usize| Layer {
            inputs,
            outputs,
            weights: (0..outputs)
                .map(|_| (0..inputs).map(|_| (rng.next() % 3) as i8 - 1).collect())
                .collect(),
            bias: (0..outputs).map(|_| (rng.next() % 3) as i8 - 1).collect(),
        };
        for seed in 1..=40u64 {
            let mut rng = Rng(seed | 1);
            let model = Model {
                layers: vec![layer(&mut rng, 3, 3), layer(&mut rng, 3, 1)],
            };
            let values: Vec<Ternary> = (0..3)
                .map(|_| match rng.next() % 3 {
                    0 => Ternary::False,
                    1 => Ternary::True,
                    _ => Ternary::Unknown,
                })
                .collect();
            let analysis = analyze(&model, &values, 64).unwrap();
            assert!(analysis.exact, "3 个输入内必在预算内");
            for (output, outcome) in analysis.outputs.iter().enumerate() {
                if let Outcome::Definite(sign) = outcome
                    && let Some(conserved) = analysis.conservative[output]
                {
                    assert_eq!(conserved, *sign, "seed={seed} output={output}");
                }
            }
        }
    }
}
