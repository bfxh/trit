//! 三值一维元胞自动机（半径 1、总istic 规则）。
//! 邻域三格之和取值 `0..=6`（`F=0, X=1, T=2`），规则是一张 7 项查表；规则空间只有 `3^7 = 2187` 种。

use trit_core::value::Ternary;

/// 宽度上限：`4096` 格 × 3 值 ≈ 12 KB 一行状态。
pub const MAX_WIDTH: usize = 4096;
/// 步数上限：每步 clone 一整行，`steps × width` 是历史表。
pub const MAX_STEPS: usize = 10_000;

/// 一条总istic 三值规则：按邻域和查表。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rule(pub [Ternary; 7]);

impl Rule {
    /// 用 0..2187 的编号构造（三进制展开，低位对应 `sum = 0`）。
    pub fn from_index(index: usize) -> Option<Self> {
        if index >= 2187 {
            return None;
        }
        let mut digits = [Ternary::False; 7];
        let mut rest = index;
        for digit in &mut digits {
            *digit = match rest % 3 {
                0 => Ternary::False,
                1 => Ternary::Unknown,
                _ => Ternary::True,
            };
            rest /= 3;
        }
        Some(Self(digits))
    }
}

/// 一次演化。
#[derive(Debug, Clone, PartialEq)]
pub struct Run {
    pub cells: Vec<Ternary>,
    pub period: Option<usize>,
    pub density: f64,
    pub entropy: f64,
}

pub fn run(rule: &Rule, width: usize, steps: usize) -> Run {
    assert!(width <= MAX_WIDTH, "宽度 {width} 超过上限 {MAX_WIDTH}");
    assert!(steps <= MAX_STEPS, "步数 {steps} 超过上限 {MAX_STEPS}");
    let mut cells = vec![Ternary::False; width];
    if width > 0 {
        cells[width / 2] = Ternary::True;
    }
    let mut history = vec![cells.clone()];
    let mut period = None;
    for _ in 0..steps {
        cells = step(rule, &cells);
        if let Some(index) = history.iter().position(|old| *old == cells) {
            period = Some(history.len() - index);
            break;
        }
        history.push(cells.clone());
    }
    Run {
        density: density(&cells),
        entropy: entropy(&cells),
        cells,
        period,
    }
}

/// 走一步；边界外按 `F` 处理。
pub fn step(rule: &Rule, cells: &[Ternary]) -> Vec<Ternary> {
    let mut next = Vec::with_capacity(cells.len());
    for index in 0..cells.len() {
        let left = if index == 0 {
            Ternary::False
        } else {
            cells[index - 1]
        };
        let right = if index + 1 >= cells.len() {
            Ternary::False
        } else {
            cells[index + 1]
        };
        let sum = offset(left) + offset(cells[index]) + offset(right);
        next.push(rule.0[sum]);
    }
    next
}

fn offset(value: Ternary) -> usize {
    match value {
        Ternary::False => 0,
        Ternary::Unknown => 1,
        Ternary::True => 2,
    }
}

/// 非 `F` 占比。
pub fn density(cells: &[Ternary]) -> f64 {
    if cells.is_empty() {
        return 0.0;
    }
    let alive = cells
        .iter()
        .filter(|value| **value != Ternary::False)
        .count();
    alive as f64 / cells.len() as f64
}

/// 三符号香农熵。
pub fn entropy(cells: &[Ternary]) -> f64 {
    if cells.is_empty() {
        return 0.0;
    }
    let total = cells.len() as f64;
    [Ternary::False, Ternary::Unknown, Ternary::True]
        .into_iter()
        .map(|symbol| {
            let count = cells.iter().filter(|value| **value == symbol).count() as f64;
            if count == 0.0 {
                0.0
            } else {
                -(count / total) * (count / total).log2()
            }
        })
        .sum()
}

/// 整片规则空间的行为分布。
#[derive(Debug, Clone, PartialEq)]
pub struct Survey {
    /// 结束时全 `F`。
    pub dead: usize,
    /// 周期 1。
    pub fixed: usize,
    /// 周期 2..=50。
    pub periodic: usize,
    /// 步数内没回到旧状态。
    pub complex: usize,
    /// 末态熵最高的规则编号。
    pub max_entropy_rule: usize,
    /// 该规则的末态熵。
    pub max_entropy: f64,
}

/// 枚举全部 2187 条规则跑一遍。
pub fn survey(width: usize, steps: usize) -> Survey {
    let mut survey = Survey {
        dead: 0,
        fixed: 0,
        periodic: 0,
        complex: 0,
        max_entropy_rule: 0,
        max_entropy: -1.0,
    };
    for index in 0..2187 {
        let rule = Rule::from_index(index).expect("index < 2187");
        let run = run(&rule, width, steps);
        match run.period {
            Some(1) => {
                if run.density == 0.0 {
                    survey.dead += 1;
                } else {
                    survey.fixed += 1;
                }
            }
            Some(period) if period <= 50 => survey.periodic += 1,
            _ => survey.complex += 1,
        }
        if run.entropy > survey.max_entropy {
            survey.max_entropy = run.entropy;
            survey.max_entropy_rule = index;
        }
    }
    survey
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_false_stays_false_when_the_rule_says_so() {
        // 邻域和 0 的条目是 False：全 F 是死态。
        let rule = Rule([Ternary::False; 7]);
        assert_eq!(step(&rule, &[Ternary::False; 5]), vec![Ternary::False; 5]);
        let rule = Rule([Ternary::True; 7]);
        assert_eq!(step(&rule, &[Ternary::False; 5]), vec![Ternary::True; 5]);
    }

    #[test]
    fn single_seed_spreads_to_the_whole_line() {
        // 只有 sum=0 给 F，其余给 T：单个种子一步就铺满，之后是不动点。
        let rule = Rule([
            Ternary::False,
            Ternary::True,
            Ternary::True,
            Ternary::True,
            Ternary::True,
            Ternary::True,
            Ternary::True,
        ]);
        let result = run(&rule, 9, 5);
        assert_eq!(result.density, 1.0);
        assert_eq!(result.period, Some(1));
    }

    #[test]
    fn rule_index_round_trips() {
        let rule = Rule::from_index(1000).unwrap();
        let back: usize = rule
            .0
            .iter()
            .enumerate()
            .map(|(power, value)| offset(*value) * 3usize.pow(power as u32))
            .sum();
        assert_eq!(back, 1000);
        assert!(Rule::from_index(2187).is_none());
    }

    #[test]
    fn survey_covers_the_whole_space() {
        let survey = survey(32, 60);
        assert_eq!(
            survey.dead + survey.fixed + survey.periodic + survey.complex,
            2187
        );
        assert!(survey.max_entropy > 0.0);
    }

    #[test]
    fn entropy_is_zero_for_uniform() {
        assert_eq!(entropy(&[Ternary::True; 8]), 0.0);
        assert!(entropy(&[Ternary::False, Ternary::Unknown, Ternary::True]) > 1.5);
    }
}
