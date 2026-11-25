//! GPTQ 的 Hessian 读取（从 `loader.rs` 拆出守住行数预算）。

pub(super) fn load(dir: Option<&str>, short: &str) -> Option<Vec<f32>> {
    let path = format!("{}/{short}.f32", dir?);
    trit_wspace::activations::read_matrix(&path)
        .ok()
        .map(|(_, _, values)| values)
}
