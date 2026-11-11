//! 有限 MV-代数（Łukasiewicz 链 Ł_{n+1}）与 t-norm 族。
//!
//! 口径：
//! - 链上取值为整数层 `0..=n`（`n = levels`），全部运算**整数精确**（无浮点误差）；
//! - t-norm 只收**在有限链上精确成立**的四种：Minimum / Łukasiewicz / Drastic /
//!   NilpotentMinimum。Product 不在其中——离散化后的乘法不满足结合律，收了就是假代数；
//! - residuum 只对左连续 t-norm 有定义（Drastic 不是左连续 ⇒ 返回结构化错误）。
//!
//! 三值对应：`levels = 2`（0/1/2）时，弱与/弱或/否定与 `trit-core` 的 Kleene 语义逐点一致
//! （见 `tests/kleene_parity.rs` 的穷举对拍）。

#![forbid(unsafe_code)]

use thiserror::Error;

/// 层数上限（`x + y` 用 u64 中间量，这个上限保证不会溢出，也不会让穷举测试爆掉）。
pub const MAX_LEVELS: u32 = 1_000_000;

/// 有限 Łukasiewicz 链：取值 `0..=levels`，`levels` 是真值个数减一。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Chain {
    levels: u32,
}

/// t-norm 族（有限链上可精确成立的四类）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TNorm {
    /// 最小（Gödel）：`min(x, y)`。
    Minimum,
    /// Łukasiewicz：`max(0, x + y − n)`。
    Lukasiewicz,
    /// Drastic：只有 `top` 是单位元，其余组合归零。
    Drastic,
    /// 幂零最小：`x + y > n` 取 `min`，否则归零。
    NilpotentMinimum,
}

impl TNorm {
    /// 全部族成员。
    pub const ALL: [Self; 4] = [
        Self::Minimum,
        Self::Lukasiewicz,
        Self::Drastic,
        Self::NilpotentMinimum,
    ];

    /// 稳定短名。
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Minimum => "minimum",
            Self::Lukasiewicz => "lukasiewicz",
            Self::Drastic => "drastic",
            Self::NilpotentMinimum => "nilpotent-minimum",
        }
    }
}

impl Chain {
    /// 建链；`levels = n` 表示 `n + 1` 个真值。
    pub fn new(levels: u32) -> Result<Self, MvError> {
        if levels == 0 {
            return Err(MvError::TooFewLevels);
        }
        if levels > MAX_LEVELS {
            return Err(MvError::TooManyLevels {
                levels,
                limit: MAX_LEVELS,
            });
        }
        Ok(Self { levels })
    }

    /// 层数 `n`。
    #[must_use]
    pub const fn levels(&self) -> u32 {
        self.levels
    }

    /// 最大值（`n`）。
    #[must_use]
    pub const fn top(&self) -> u32 {
        self.levels
    }

    /// 最小值（`0`）。
    #[must_use]
    pub const fn bottom(&self) -> u32 {
        0
    }

    /// 全部取值 `0..=n`。
    pub fn values(&self) -> impl Iterator<Item = u32> + '_ {
        0..=self.levels
    }

    /// 把层换算成 `[0, 1]` 上的比例（展示用；运算一律走整数）。
    #[must_use]
    pub fn ratio(&self, value: u32) -> f64 {
        f64::from(value) / f64::from(self.levels)
    }

    /// MV 否定：`¬x = n − x`。
    pub fn neg(&self, x: u32) -> Result<u32, MvError> {
        self.check(x)?;
        Ok(self.levels - x)
    }

    /// 弱合取（格序 min）。
    pub fn weak_and(&self, x: u32, y: u32) -> Result<u32, MvError> {
        self.check(x)?;
        self.check(y)?;
        Ok(x.min(y))
    }

    /// 弱析取（格序 max）。
    pub fn weak_or(&self, x: u32, y: u32) -> Result<u32, MvError> {
        self.check(x)?;
        self.check(y)?;
        Ok(x.max(y))
    }

    /// t-norm（强合取）。
    pub fn t_norm(&self, kind: TNorm, x: u32, y: u32) -> Result<u32, MvError> {
        self.check(x)?;
        self.check(y)?;
        let levels = u64::from(self.levels);
        let xb = u64::from(x);
        let yb = u64::from(y);
        Ok(match kind {
            TNorm::Minimum => x.min(y),
            TNorm::Lukasiewicz => u32::try_from((xb + yb).saturating_sub(levels)).unwrap_or(0),
            TNorm::Drastic => {
                if y == self.levels {
                    x
                } else if x == self.levels {
                    y
                } else {
                    0
                }
            }
            TNorm::NilpotentMinimum => {
                if xb + yb > levels {
                    x.min(y)
                } else {
                    0
                }
            }
        })
    }

    /// t-conorm（强析取）：`¬(¬x ⊗ ¬y)`。
    pub fn t_conorm(&self, kind: TNorm, x: u32, y: u32) -> Result<u32, MvError> {
        let nx = self.neg(x)?;
        let ny = self.neg(y)?;
        self.neg(self.t_norm(kind, nx, ny)?)
    }

    /// residuum（蕴含）：左连续 t-norm 才有定义。
    pub fn residuum(&self, kind: TNorm, x: u32, y: u32) -> Result<u32, MvError> {
        self.check(x)?;
        self.check(y)?;
        Ok(match kind {
            TNorm::Minimum => {
                if x <= y {
                    self.levels
                } else {
                    y
                }
            }
            TNorm::Lukasiewicz => {
                u32::try_from(u64::from(self.levels) + u64::from(y) - u64::from(x)).unwrap_or(0)
            }
            TNorm::NilpotentMinimum => {
                if x <= y {
                    self.levels
                } else {
                    (self.levels - x).max(y)
                }
            }
            TNorm::Drastic => {
                return Err(MvError::NoResiduum {
                    kind: kind.as_str(),
                });
            }
        })
    }

    fn check(&self, value: u32) -> Result<(), MvError> {
        if value > self.levels {
            return Err(MvError::OutOfRange {
                value,
                levels: self.levels,
            });
        }
        Ok(())
    }
}

/// 代数错误。
#[derive(Debug, Error)]
pub enum MvError {
    /// 层数为 0。
    #[error("层数必须 ≥ 1（至少两个真值）")]
    TooFewLevels,
    /// 层数超上限。
    #[error("层数 {levels} 超过上限 {limit}")]
    TooManyLevels {
        /// 请求的层数。
        levels: u32,
        /// 上限。
        limit: u32,
    },
    /// 取值越界。
    #[error("取值 {value} 超出链 0..={levels}")]
    OutOfRange {
        /// 请求的取值。
        value: u32,
        /// 链层数。
        levels: u32,
    },
    /// 该 t-norm 没有 residuum。
    #[error("t-norm `{kind}` 不是左连续，没有 residuum")]
    NoResiduum {
        /// t-norm 短名。
        kind: &'static str,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chains() -> Vec<Chain> {
        (1..=5).map(|n| Chain::new(n).unwrap()).collect()
    }

    #[test]
    fn tnorm_axioms_hold_on_small_chains() {
        for chain in chains() {
            for kind in TNorm::ALL {
                for x in chain.values() {
                    for y in chain.values() {
                        // 交换律
                        assert_eq!(
                            chain.t_norm(kind, x, y).unwrap(),
                            chain.t_norm(kind, y, x).unwrap(),
                            "{kind:?} 交换律失败 x={x} y={y}"
                        );
                        // 单位元：top
                        assert_eq!(
                            chain.t_norm(kind, chain.top(), x).unwrap(),
                            x,
                            "{kind:?} 单位元失败 x={x}"
                        );
                        // 单调性
                        for x2 in x..=chain.top() {
                            assert!(
                                chain.t_norm(kind, x, y).unwrap()
                                    <= chain.t_norm(kind, x2, y).unwrap(),
                                "{kind:?} 单调性失败 x={x} x2={x2} y={y}"
                            );
                        }
                        // 结合律
                        for z in chain.values() {
                            let left = chain
                                .t_norm(kind, chain.t_norm(kind, x, y).unwrap(), z)
                                .unwrap();
                            let right = chain
                                .t_norm(kind, x, chain.t_norm(kind, y, z).unwrap())
                                .unwrap();
                            assert_eq!(left, right, "{kind:?} 结合律失败 x={x} y={y} z={z}");
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn conorm_is_dual_of_norm() {
        for chain in chains() {
            for kind in TNorm::ALL {
                for x in chain.values() {
                    for y in chain.values() {
                        let dual = chain
                            .neg(
                                chain
                                    .t_norm(kind, chain.neg(x).unwrap(), chain.neg(y).unwrap())
                                    .unwrap(),
                            )
                            .unwrap();
                        assert_eq!(chain.t_conorm(kind, x, y).unwrap(), dual);
                        assert_eq!(
                            chain.t_conorm(kind, chain.bottom(), x).unwrap(),
                            x,
                            "conorm 单位元失败"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn residuum_adjunction_holds_for_left_continuous_norms() {
        for chain in chains() {
            for kind in [TNorm::Minimum, TNorm::Lukasiewicz, TNorm::NilpotentMinimum] {
                for x in chain.values() {
                    for y in chain.values() {
                        for z in chain.values() {
                            let product = chain.t_norm(kind, x, y).unwrap();
                            let implication = chain.residuum(kind, x, z).unwrap();
                            assert_eq!(
                                product <= z,
                                y <= implication,
                                "{kind:?} 伴随失败 x={x} y={y} z={z}"
                            );
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn drastic_has_no_residuum() {
        let chain = Chain::new(3).unwrap();
        let err = chain.residuum(TNorm::Drastic, 1, 2).unwrap_err();
        assert!(matches!(err, MvError::NoResiduum { .. }), "{err}");
    }

    #[test]
    fn negation_and_weak_ops_are_exact() {
        let chain = Chain::new(4).unwrap();
        for x in chain.values() {
            assert_eq!(chain.neg(chain.neg(x).unwrap()).unwrap(), x);
            assert_eq!(chain.weak_and(x, chain.top()).unwrap(), x);
            assert_eq!(chain.weak_or(x, chain.bottom()).unwrap(), x);
        }
    }

    #[test]
    fn guards_and_bounds_are_structured() {
        assert!(matches!(Chain::new(0), Err(MvError::TooFewLevels)));
        assert!(matches!(
            Chain::new(MAX_LEVELS + 1),
            Err(MvError::TooManyLevels { .. })
        ));
        let chain = Chain::new(2).unwrap();
        assert!(matches!(chain.neg(3), Err(MvError::OutOfRange { .. })));
        assert!(matches!(
            chain.t_norm(TNorm::Minimum, 1, 3),
            Err(MvError::OutOfRange { .. })
        ));
    }

    #[test]
    fn ratios_cover_endpoints() {
        let chain = Chain::new(10).unwrap();
        assert!((chain.ratio(0) - 0.0).abs() < f64::EPSILON);
        assert!((chain.ratio(10) - 1.0).abs() < f64::EPSILON);
        assert!((chain.ratio(5) - 0.5).abs() < f64::EPSILON);
    }
}
