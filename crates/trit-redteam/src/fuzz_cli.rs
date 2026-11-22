//! `--iterations` 的 CLI 上界（#596）。
//!
//! `mutants` 按迭代数预分配；无上界时 2^31-1 会直接触发分配器 abort。
//! 拆成单独模块只是为了守住 `main.rs` 的行数预算。

/// 一轮 fuzz 最多多少变异用例。
pub const MAX_ITERATIONS: usize = 1_048_576;

/// 校验 `--iterations`，越界返回结构化错误。
pub fn checked(iterations: usize) -> anyhow::Result<usize> {
    anyhow::ensure!(
        iterations > 0 && iterations <= MAX_ITERATIONS,
        "--iterations 必须在 1..={MAX_ITERATIONS}（本次 {iterations}）"
    );
    Ok(iterations)
}
