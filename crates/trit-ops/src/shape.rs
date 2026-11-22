//! CLI / gemv 的形状守卫（#588、#595）。
//!
//! `gemv_*` 是 `pub fn`，入口必须在建裸切片**之前**拒绝错形状；`bench` 的
//! `--rows/--cols` 也必须在上界内，否则夹具构造会直接走到分配器 abort。
//! 这两个不变式原来各写在调用点，集中到这里后单文件行数不再增长。

/// `bench` 单维上限：1<<16 已经足够所有内置形状；再大应分块跑。
pub const MAX_DIM: usize = 1 << 16;

/// 同一形状下 `prepare` 会建 f32 / f16 / Q4_0 / 双导轨 / tryte 五份夹具。
/// 1<<23 个元素约 32 MB/份，再大就交给分块。
pub const MAX_CELLS: usize = 1 << 23;

/// `gemv_*` 的形状不变式：`rows * cols` 可表示，权重/激活/输出三者对得上。
///
/// 不满足时 panic（`assert`，release 也生效），而不是在 `unsafe` 里按谎报长度
/// 发布裸切片——后者会越界读，AVX2 路径直接 SIGSEGV。
pub(crate) fn check_gemv_shape(
    weight_len: usize,
    rows: usize,
    cols: usize,
    activation_len: usize,
    out_len: usize,
) {
    let expected = rows
        .checked_mul(cols)
        .expect("rows*cols 溢出 usize，形状非法");
    assert_eq!(out_len, rows, "输出长度必须是 rows");
    assert_eq!(weight_len, expected, "权重长度必须是 rows*cols");
    assert_eq!(activation_len, cols, "激活长度必须是 cols");
}

/// Q4_0 路径的形状不变式；返回每行字节数。
///
/// `quantize_act_q8` 按 32 元素分块，`cols` 不是 32 的倍数时尾块不会被量化，
/// 按 `cols` 谎报的裸切片会越过真实长度。
pub(crate) fn check_q4_shape(
    weight_len: usize,
    rows: usize,
    cols: usize,
    activation_len: usize,
    out_len: usize,
) -> usize {
    assert_eq!(out_len, rows, "输出长度必须是 rows");
    assert_eq!(activation_len, cols, "激活长度必须是 cols");
    assert_eq!(cols % 32, 0, "Q4_0 路径要求 cols 是 32 的倍数");
    let bpr = cols / 32 * 18;
    let expected = rows.checked_mul(bpr).expect("rows*每行字节数溢出 usize");
    assert_eq!(weight_len, expected, "Q4_0 权重长度必须是 rows*每行字节数");
    bpr
}

/// 校验 bench 的 `--rows/--cols`：单维 ≤ [`MAX_DIM`]，乘积 ≤ [`MAX_CELLS`]。
pub fn check_bench_args(args: &[String]) -> Result<(), String> {
    for name in ["--rows", "--cols"] {
        if let Some(value) = arg_usize(args, name)
            && value > MAX_DIM
        {
            return Err(format!("{name} 上限 {MAX_DIM}（本次 {value}）"));
        }
    }
    if let (Some(rows), Some(cols)) = (arg_usize(args, "--rows"), arg_usize(args, "--cols")) {
        match rows.checked_mul(cols) {
            Some(cells) if cells <= MAX_CELLS => {}
            _ => {
                return Err(format!(
                    "--rows×--cols 上限 {MAX_CELLS}（本次 {rows}×{cols}）"
                ));
            }
        }
    }
    Ok(())
}

/// 取 `--name` 的 `usize` 值。
pub fn arg_usize(args: &[String], name: &str) -> Option<usize> {
    args.iter()
        .position(|a| a == name)
        .and_then(|i| args.get(i + 1))
        .and_then(|s| s.parse::<usize>().ok())
}

/// 取 `--pair A B` 的两个值。
pub fn arg_pair<'a>(args: &'a [String], name: &str) -> Option<(&'a str, &'a str)> {
    let i = args.iter().position(|a| a == name)?;
    Some((args.get(i + 1)?.as_str(), args.get(i + 2)?.as_str()))
}
