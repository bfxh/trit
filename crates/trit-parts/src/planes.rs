//! 深度三值平面的级数口径：`--depth-planes` 是**位数**，级数是 `3^n - 1`。
//!
//! 单独成一层是为了不挂 `features = ["gpu"]`：`net/layer.rs` 要 `wgpu`，而 CI 的
//! gpu step 只 `cargo check` 不跑 test，校验写在那里默认 CI 一次都执行不到。

/// 位数上限。`3^12 = 531441` 级，已经细于任何 8 位深度源的量化台阶。
pub const MAX_PLANES: usize = 12;

/// 位数校验。允许 0（不建深度头）。
pub fn check(n: usize) -> Result<(), String> {
    if n > MAX_PLANES {
        return Err(format!(
            "深度平面的位数 {n} 超过 {MAX_PLANES}（3^{MAX_PLANES} 级）"
        ));
    }
    Ok(())
}

/// `3^n`。`saturating`：位数越限时不会在 `usize` 里绕回。调用方应先 [`check`]。
pub fn pow3(n: usize) -> usize {
    (0..n).fold(1usize, |a, _| a.saturating_mul(3))
}

/// `3^n - 1` 级（`n = 0` 时为 0，没有可分的档）。
pub fn levels(n: usize) -> usize {
    pow3(n).saturating_sub(1)
}

/// 深度头输出通道：`n*3` 再补到 4 的倍数。
///
/// 旧算法在 `u32` 里做这两步：`n = 1_431_655_765` 时 `n*3` 绕成 `u32::MAX`，
/// 补零那步再加 1 回到 0——深度头静默变成 0 通道，不报错不崩，只是训练出
/// 一个没有深度输出的网。这里全程 `usize` 且 `saturating`。
pub fn head_cout(n: usize) -> usize {
    let need = n.saturating_mul(3);
    need + (4 - need % 4) % 4
}

/// 先 [`check`] 再取级数（`data::depth_planes` 用）。
pub fn levels_checked(n: usize) -> Result<usize, String> {
    check(n)?;
    Ok(levels(n))
}

/// 校验后原样返回（CLI 解析处用）。
pub fn checked_usize(n: usize) -> Result<usize, String> {
    check(n)?;
    Ok(n)
}

/// 从 CLI 字符串解析位数并校验：`args.rs` 的 `--depth-planes` 入口。
pub fn parse_arg(s: &str) -> Result<usize, String> {
    let n: usize = s.parse().map_err(|_| "--depth-planes 非整数")?;
    check(n)?;
    Ok(n)
}
