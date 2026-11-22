//! `trit-dot` 的 CLI 参数解析与上界（#592）。
//!
//! 拆出 `main.rs` 以守住行数预算；`bench` 一次会建 5 份 `len` 长的 i32 缓冲
//! （a/b/sparse_a/sparse_b/block_sparse），再加位平面。

/// `bench --len` 上限：1<<24 ≈ 16.7M ≈ 335 MB；再大走分配器 abort。
pub const MAX_LEN: usize = 1 << 24;

/// 取 `--name` 的 `usize` 值。
pub fn number(args: &[String], name: &str, default: usize) -> Result<usize, String> {
    match args.iter().position(|arg| arg == name) {
        Some(index) => args
            .get(index + 1)
            .ok_or_else(|| format!("{name} 缺少参数"))?
            .parse()
            .map_err(|_| format!("{name} 不是整数")),
        None => Ok(default),
    }
}

/// 校验 `bench --len`，越界返回结构化错误而不是分配器 abort。
pub fn check_len(len: usize) -> Result<(), String> {
    if len > MAX_LEN {
        return Err(format!(
            "--len 上限 {MAX_LEN}（本次 {len}；5 份 i32 缓冲约 {} MB）",
            len.saturating_mul(4 * 5) / 1_000_000
        ));
    }
    Ok(())
}
