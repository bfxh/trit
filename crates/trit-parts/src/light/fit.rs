/// 由「法线 + 亮度」线性求解光照：`I ≈ a + b·n`，`b = k·l`。
/// 返回 `(l 单位向量, k 强度, a 环境项)`；y 方向沿用图像坐标（v 向下）。
///
/// 前提：法线图与亮度同一相机坐标系；只用 `mask` 为真的像素。
pub fn fit_from_normals(
    lum: &[f32],
    n: &[f32],
    mask: &[bool],
    w: usize,
    h: usize,
) -> Option<([f32; 3], f32, f32)> {
    // 长度契约由本函数自己守：`lum`/`mask` 要 w*h，`n` 要 w*h*3。调用者拿到的是
    // 从文件解出来的缓冲，尺寸与 w/h 不一致时这里必须返回 None，而不是让下面
    // 的 `mask[i]` / `n[i*3+2]` 直接越界。`px*3` 也走 checked，32 位目标上
    // 超大 w*h 会先溢出再算出一个偏小的上界。
    let px = w.checked_mul(h)?;
    let px3 = px.checked_mul(3)?;
    if lum.len() < px || mask.len() < px || n.len() < px3 {
        return None;
    }
    // 法方程 4×4：未知 [a, bx, by, bz]
    let mut ata = [[0.0f64; 4]; 4];
    let mut aty = [0.0f64; 4];
    let mut cnt = 0usize;
    for i in 0..w * h {
        if !mask[i] {
            continue;
        }
        let row = [
            1.0,
            n[i * 3] as f64,
            n[i * 3 + 1] as f64,
            n[i * 3 + 2] as f64,
        ];
        let y = lum[i] as f64;
        for a in 0..4 {
            aty[a] += row[a] * y;
            for b in 0..4 {
                ata[a][b] += row[a] * row[b];
            }
        }
        cnt += 1;
    }
    if cnt < 64 {
        return None;
    }
    let mut m = ata;
    let mut b = aty;
    let n4 = 4;
    for col in 0..n4 {
        let mut piv = col;
        for r in col + 1..n4 {
            if m[r][col].abs() > m[piv][col].abs() {
                piv = r;
            }
        }
        if m[piv][col].abs() < 1e-12 {
            return None;
        }
        m.swap(col, piv);
        b.swap(col, piv);
        let d = m[col][col];
        for v in m[col].iter_mut().skip(col) {
            *v /= d;
        }
        b[col] /= d;
        for r in 0..n4 {
            if r == col {
                continue;
            }
            let f = m[r][col];
            let src: [f64; 4] = m[col];
            for (v, s0) in m[r].iter_mut().skip(col).zip(src.iter().skip(col)) {
                *v -= f * s0;
            }
            b[r] -= f * b[col];
        }
    }
    let (a, bx, by, bz) = (b[0], b[1], b[2], b[3]);
    let k = (bx * bx + by * by + bz * bz).sqrt();
    if k < 1e-9 {
        return None;
    }
    Some((
        [(bx / k) as f32, (by / k) as f32, (bz / k) as f32],
        k as f32,
        a as f32,
    ))
}

/// 基于法线的重打光：`L' = a + k·max(0, n·l')`，按 `I' = I · L'/L` 缩放（保色相）。
///
/// 长度契约同 [`fit_from_normals`]：`rgb` 与 `n` 都必须是 `w*h*3`，不足时返回
/// `None`。早先这里直接 `rgb.to_vec()` 再裸索引，缓冲短一截就是越界 panic。
pub fn relight_normals(
    rgb: &[f32],
    n: &[f32],
    w: usize,
    h: usize,
    l: [f32; 3],
    k: f32,
    a: f32,
) -> Option<Vec<f32>> {
    let px = w.checked_mul(h)?;
    let px3 = px.checked_mul(3)?;
    if rgb.len() < px3 || n.len() < px3 {
        return None;
    }
    let mut out = rgb.to_vec();
    for i in 0..w * h {
        let ndl = n[i * 3] * l[0] + n[i * 3 + 1] * l[1] + n[i * 3 + 2] * l[2];
        let l_new = (a + k * ndl.max(0.0)).clamp(0.02, 2.0);
        let l_old =
            (0.299 * rgb[i * 3] + 0.587 * rgb[i * 3 + 1] + 0.114 * rgb[i * 3 + 2]).clamp(0.02, 1.0);
        let s = (l_new / l_old).clamp(0.1, 6.0);
        for c in 0..3 {
            out[i * 3 + c] = (rgb[i * 3 + c] * s).clamp(0.0, 1.0);
        }
    }
    Some(out)
}
