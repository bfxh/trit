//! 预览拼图：`res×res×3` 的若干帧拼成 `cols` 列的联系表（行优先）。
//!
//! 尺寸判定单独成层是因为 `cols`/`res` 都来自外部（CLI `--cols`、清单 JSON 的
//! `cols`/`res`），而 `sheet_dims` 不分配——上限能被断言直接钉住。

/// 拼图像素上限：64M 像素（f32 RGB 时 768 MB）。再大就不是「联系表」了。
pub const MAX_SHEET_PIXELS: usize = 64 << 20;

fn mul(a: usize, b: usize, what: &str) -> Result<usize, String> {
    a.checked_mul(b)
        .ok_or_else(|| format!("{what} {a}×{b} 溢出 usize"))
}

/// 只算拼图宽高，不分配。
pub fn sheet_dims(frames_len: usize, res: usize, cols: usize) -> Result<(usize, usize), String> {
    if res == 0 {
        return Err("拼图边 res 不能是 0（会得到一张空表）".into());
    }
    let cols = cols.max(1);
    let rows = frames_len.div_ceil(cols).max(1);
    let w = mul(cols, res, "拼图宽")?;
    let h = mul(rows, res, "拼图高")?;
    let px = mul(w, h, "拼图像素数")?;
    if px > MAX_SHEET_PIXELS {
        return Err(format!(
            "拼图 {w}×{h} = {px} 像素超过 {MAX_SHEET_PIXELS} 上限"
        ));
    }
    Ok((w, h))
}

/// 把若干张 `res×res×3` 的图拼成 `cols` 列的联系表（行优先）。
///
/// 宽高与像素数一律 `checked` 后再分配；长度不符的帧跳过（不静默截断像素）。
pub fn contact_sheet(
    frames: &[Vec<f32>],
    res: usize,
    cols: usize,
) -> Result<(usize, usize, Vec<f32>), String> {
    let (w, h) = sheet_dims(frames.len(), res, cols)?;
    let need = mul(mul(w, h, "拼图像素数")?, 3, "拼图 f32 个数")?;
    let mut sheet = vec![0.0f32; need];
    let cols = cols.max(1);
    for (fi, f) in frames.iter().enumerate() {
        if f.len() != res.saturating_mul(res).saturating_mul(3) {
            continue;
        }
        let (cx, cy) = (fi % cols, fi / cols);
        for y in 0..res {
            let src = y * res * 3;
            let dst = ((cy * res + y) * w + cx * res) * 3;
            sheet[dst..dst + res * 3].copy_from_slice(&f[src..src + res * 3]);
        }
    }
    Ok((w, h, sheet))
}
