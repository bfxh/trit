//! 超限抽样：精确计数算不动时，按**显式预算**抽样并给出 Wilson 95% 区间。
//!
//! 原则：抽样结果永远带 `Sampled` 标记与样本数，绝不冒充精确值；
//! 没给预算（`samples == 0`）直接报错。随机源是固定种子的 SplitMix64，
//! 同一输入 + 同一种子 ⇒ 同一结果（可复算）。

use crate::dsl::Rule;
use crate::eval::evaluate;
use crate::repair::{Completion, Fill, RepairError, completions};
use crate::value::Table;

/// 抽样预算。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SampleBudget {
    /// 抽样次数（必须 > 0）。
    pub samples: u64,
    /// 随机种子（固定种子保证可复算）。
    pub seed: u64,
}

impl Default for SampleBudget {
    fn default() -> Self {
        Self {
            samples: 100_000,
            seed: 0x5EED_CAFE_F00D_1234,
        }
    }
}

/// 抽样结果（显式标记，不是精确值）。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Sampled {
    /// 一致比例的点估计。
    pub ratio: f64,
    /// 实际抽样次数。
    pub samples: u64,
    /// Wilson 95% 置信区间（下界, 上界）。
    pub ci95: (f64, f64),
}

/// 完成计数的结果：要么精确，要么抽样（永不静默降级）。
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum CompletionOutcome {
    /// 精确计数（DPLL）。
    Exact(Completion),
    /// 抽样估计（原问题超出精确计数上限）。
    Sampled(Sampled),
}

/// 先试精确计数；只有“变量数超上限”这一种情况退化为抽样，其余错误照常上报。
pub fn completions_or_sample(
    table: &Table,
    rules: &[Rule],
    fills: &[Fill],
    budget: SampleBudget,
) -> Result<CompletionOutcome, RepairError> {
    if budget.samples == 0 {
        return Err(RepairError::InvalidBudget);
    }
    match completions(table, rules, fills) {
        Ok(exact) => Ok(CompletionOutcome::Exact(exact)),
        Err(RepairError::TooManyVariables { .. }) => Ok(CompletionOutcome::Sampled(sample_ratio(
            table, rules, fills, budget,
        ))),
        Err(other) => Err(other),
    }
}

fn sample_ratio(table: &Table, rules: &[Rule], fills: &[Fill], budget: SampleBudget) -> Sampled {
    let mut rng = SplitMix64::new(budget.seed);
    let mut working = table.clone();
    let mut hits = 0u64;
    for _ in 0..budget.samples {
        for fill in fills {
            let pick = rng.below(fill.domain.len());
            working.rows[fill.row][fill.col] = fill.domain[pick].clone();
        }
        if !evaluate(&working, rules).has_violation() {
            hits += 1;
        }
    }
    let ratio = hits as f64 / budget.samples as f64;
    Sampled {
        ratio,
        samples: budget.samples,
        ci95: wilson(hits, budget.samples),
    }
}

/// 固定种子的 SplitMix64（可复算；不引入依赖）。
struct SplitMix64 {
    state: u64,
}

impl SplitMix64 {
    fn new(seed: u64) -> Self {
        Self { state: seed }
    }

    fn next_u64(&mut self) -> u64 {
        self.state = self.state.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.state;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// `[0, bound)` 的无偏取样（拒绝落在不完整区间的尾部）。
    fn below(&mut self, bound: usize) -> usize {
        let bound = bound as u64;
        let limit = u64::MAX - (u64::MAX % bound);
        loop {
            let value = self.next_u64();
            if value < limit {
                return (value % bound) as usize;
            }
        }
    }
}

/// Wilson 95% 区间（z = 1.959963984540054）。
fn wilson(hits: u64, samples: u64) -> (f64, f64) {
    const Z: f64 = 1.959_963_984_540_054;
    let n = samples as f64;
    let p = hits as f64 / n;
    let z2 = Z * Z;
    let denominator = 1.0 + z2 / n;
    let center = (p + z2 / (2.0 * n)) / denominator;
    let half = Z * ((p * (1.0 - p) / n + z2 / (4.0 * n * n)).sqrt()) / denominator;
    ((center - half).max(0.0), (center + half).min(1.0))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dsl::CmpOp;
    use crate::value::{Column, Ty, Value};

    fn wide_table(columns: usize) -> (Table, Vec<Fill>, Vec<Rule>) {
        let cols: Vec<Column> = (0..columns)
            .map(|i| Column::new(&format!("c{i}"), Ty::Int))
            .collect();
        let mut table = Table::new(cols);
        table.push_row(vec![Value::Null; columns]).unwrap();
        let domain: Vec<Value> = (0..10).map(Value::Int).collect();
        let fills: Vec<Fill> = (0..columns)
            .map(|col| Fill::new(0, col, domain.clone()))
            .collect();
        let rules = vec![Rule::Check {
            col: 0,
            op: CmpOp::Lt,
            lit: Value::Int(2),
        }];
        (table, fills, rules)
    }

    #[test]
    fn exact_path_is_used_when_within_budget() {
        let (table, fills, rules) = wide_table(2);
        let outcome =
            completions_or_sample(&table, &rules, &fills, SampleBudget::default()).unwrap();
        assert!(
            matches!(outcome, CompletionOutcome::Exact(_)),
            "{outcome:?}"
        );
    }

    #[test]
    fn over_limit_falls_back_to_sampling_with_marker() {
        let (table, fills, rules) = wide_table(13); // 13 × 10 = 130 变量 > 120
        let outcome = completions_or_sample(
            &table,
            &rules,
            &fills,
            SampleBudget {
                samples: 20_000,
                seed: 7,
            },
        )
        .unwrap();
        let CompletionOutcome::Sampled(sampled) = outcome else {
            panic!("应当抽样，实际 {outcome:?}");
        };
        assert_eq!(sampled.samples, 20_000);
        // 精确比例 = 2/10（c0 ∈ {0,1}），用一个宽区间容差防抖。
        assert!(
            (sampled.ratio - 0.2).abs() < 0.02,
            "ratio={}",
            sampled.ratio
        );
        assert!(
            sampled.ci95.0 <= 0.2 && 0.2 <= sampled.ci95.1,
            "ci={:?}",
            sampled.ci95
        );
    }

    #[test]
    fn sampling_is_deterministic_for_the_same_seed() {
        let (table, fills, rules) = wide_table(13);
        let budget = SampleBudget {
            samples: 5_000,
            seed: 42,
        };
        let a = completions_or_sample(&table, &rules, &fills, budget).unwrap();
        let b = completions_or_sample(&table, &rules, &fills, budget).unwrap();
        assert_eq!(a, b);
    }

    #[test]
    fn zero_budget_is_an_error() {
        let (table, fills, rules) = wide_table(2);
        let err = completions_or_sample(
            &table,
            &rules,
            &fills,
            SampleBudget {
                samples: 0,
                seed: 1,
            },
        )
        .unwrap_err();
        assert!(matches!(err, RepairError::InvalidBudget), "{err}");
    }

    #[test]
    fn wilson_interval_covers_edges() {
        let (low, high) = wilson(0, 100);
        assert!((low - 0.0).abs() < 1e-12 && high > 0.0 && high < 0.05);
        let (low, high) = wilson(100, 100);
        assert!(low < 1.0 && (high - 1.0).abs() < 1e-12);
    }
}
