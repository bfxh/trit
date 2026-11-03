//! 三值等价的 BDD（canonical）判定。
//!
//! 每个 trit 仍然是双导轨 `(t, x)`：`(0,0)=False`、`(1,0)=True`、`(0,1)=Unknown`。
//! 与 SAT 路径不同，这里把导轨直接建成 BDD：
//!
//! - 纯二值变量只建一根 `t` 导轨（`x ≡ false`），不付 unknown 的变量成本；
//! - 三值变量建 `t` / `x` 两根导轨，并在根上检查 `¬(t ∧ x)`；
//! - 门按代数真值表逐行构造（把每个输入组合的条件 OR 起来），K3/Ł3/B3 共用；
//! - 等价 = `valid ∧ ¬(a.t ↔ b.t ∧ a.x ↔ b.x)` 为 false（BDD 是 canonical 的）；
//! - 不等价时从差集 BDD 取一个满足赋值，还原成反例编号。
//!
//! BDD 的规模依赖变量顺序与函数结构；结构化公式通常很小，但某些函数会指数爆炸。
//! 默认变量顺序是表达式首次出现顺序。

#![forbid(unsafe_code)]

use trit_eval::EquivalenceResult;

mod count;
mod encode;
mod equiv;
mod order;
mod quantify;

pub use count::count_models_bdd;
pub use equiv::{
    PremiseCheck, equivalent_bdd, equivalent_bdd_best, equivalent_bdd_best_with_limits,
    equivalent_bdd_with, equivalent_bdd_with_limits, equivalent_bdd_with_order,
    premise_counterexample_bdd, premise_counterexample_bdd_with,
    premise_counterexample_bdd_with_order, satisfiable_bdd, satisfiable_bdd_with,
    satisfiable_bdd_with_order,
};
pub use order::{VarOrder, order_variables};
pub use quantify::{
    Quantified, Quantifier, quantify_bdd, quantify_bdd_best, quantify_bdd_best_with_limits,
    quantify_bdd_with, quantify_bdd_with_order,
};

/// BDD 判定错误。
#[derive(Debug, thiserror::Error)]
pub enum BddError {
    /// BDD 变量数超过 `u16` 上限。
    #[error("BDD 变量数 {0} 超过 u16 上限")]
    TooManyVariables(usize),
    /// BDD 说可满足但取不到赋值（不应发生）。
    #[error("BDD 缺少反例赋值")]
    MissingWitness,
    /// 被量化的变量不在表达式里。
    #[error("表达式里没有变量 {0}")]
    UnknownVariable(String),
    /// 表达式超出资源预算。
    #[error("表达式超出预算: {0}")]
    Limit(#[from] trit_core::limits::LimitError),
    /// 单个 BDD 的节点数超出预算。
    #[error("BDD 节点数超过上限 {limit}")]
    BudgetExceeded {
        /// 上限。
        limit: usize,
    },
    /// 模型数超出 `u128` 可表示范围。
    #[error("模型数超过 u128 上限")]
    CountTooLarge,
}

/// BDD 节点预算。
///
/// **这是软限额**：每次运算返回后检查 `Bdd::size()`，因此单次 `apply`／量化
/// 在内部产生的峰值仍可能超过 `max_nodes`。需要硬内存/CPU 上界时，应把求解
/// 放进有资源限制的进程或 worker，而不是依赖本结构。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BddLimits {
    /// 单个 BDD 允许的最大节点数（`Bdd::size`）。
    pub max_nodes: usize,
}

impl Default for BddLimits {
    fn default() -> Self {
        Self {
            max_nodes: 1_000_000,
        }
    }
}

impl BddLimits {
    /// 不设上限。
    pub const UNLIMITED: Self = Self {
        max_nodes: usize::MAX,
    };
}

/// 一次 BDD 计算的配置：变量顺序（`None` = 首次出现）与节点预算。
#[derive(Debug, Clone, Copy, Default)]
pub struct BddRequest<'a> {
    /// 变量顺序；`None` 使用首次出现顺序。
    pub order: Option<&'a [String]>,
    /// 节点预算。
    pub limits: BddLimits,
}

/// BDD 判定结果，附带差集 BDD 的节点数。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BddVerdict {
    /// 与其它引擎同形的结论。
    pub result: EquivalenceResult,
    /// 差集 BDD 的节点数（等价时为 0 或常量大小）。
    pub nodes: usize,
}
