//! 闭式岭回归（含截距），用于小样本分类/回归头。
//!
//! `w = (XᵀX + λI)⁻¹ Xᵀy`，I 不含截距维（截距不惩罚）。矩阵小（特征数 ~20），直接高斯消元即可，
//! 全程 f64、确定性、无随机初始化。

/// 拟合岭回归，返回 `[bias, w0, w1, …]`（长度 = 特征数 + 1）。
pub fn fit_ridge(x: &[Vec<f32>], y: &[f32], lambda: f32) -> Vec<f32> {
    assert_eq!(x.len(), y.len());
    if x.is_empty() {
        return vec![0.0];
    }
    let d = x[0].len() + 1;
    let mut a = vec![vec![0.0f64; d]; d];
    let mut b = vec![0.0f64; d];
    for (row, &yy) in x.iter().zip(y) {
        let mut v = vec![1.0f64];
        v.extend(row.iter().map(|&f| f as f64));
        for i in 0..d {
            b[i] += v[i] * yy as f64;
            for j in 0..d {
                a[i][j] += v[i] * v[j];
            }
        }
    }
    for (i, row) in a.iter_mut().enumerate().skip(1) {
        row[i] += lambda as f64;
    }
    solve(a, b)
}

/// 预测（`w` 来自 [`fit_ridge`]）。
pub fn predict(w: &[f32], x: &[f32]) -> f32 {
    let mut s = w.first().copied().unwrap_or(0.0);
    for (wi, xi) in w.iter().skip(1).zip(x) {
        s += wi * xi;
    }
    s
}

/// 高斯消元（列主元），奇异时退化为 0 解。
fn solve(mut a: Vec<Vec<f64>>, mut b: Vec<f64>) -> Vec<f32> {
    let n = b.len();
    for col in 0..n {
        let mut piv = col;
        for r in col + 1..n {
            if a[r][col].abs() > a[piv][col].abs() {
                piv = r;
            }
        }
        if a[piv][col].abs() < 1e-12 {
            continue;
        }
        a.swap(col, piv);
        b.swap(col, piv);
        let d = a[col][col];
        for v in a[col].iter_mut().skip(col) {
            *v /= d;
        }
        b[col] /= d;
        for r in 0..n {
            if r == col {
                continue;
            }
            let f = a[r][col];
            if f == 0.0 {
                continue;
            }
            // r < col 时 split_at_mut 拿不到第 col 行 ⇒ 直接克隆源行（矩阵很小）
            let src: Vec<f64> = a[col].clone();
            for (v, s) in a[r].iter_mut().skip(col).zip(src.iter().skip(col)) {
                *v -= f * s;
            }
            b[r] -= f * b[col];
        }
    }
    b.into_iter().map(|v| v as f32).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recovers_a_linear_relation() {
        // y = 2 + 3x0 − 1.5x1（无噪声）⇒ 权重应几乎精确
        let x: Vec<Vec<f32>> = (0..20)
            .map(|i| {
                let t = i as f32 * 0.1;
                vec![t, (i % 5) as f32]
            })
            .collect();
        let y: Vec<f32> = x.iter().map(|v| 2.0 + 3.0 * v[0] - 1.5 * v[1]).collect();
        let w = fit_ridge(&x, &y, 1e-6);
        assert!((w[0] - 2.0).abs() < 0.01, "截距 {} ≈ 2", w[0]);
        assert!((w[1] - 3.0).abs() < 0.01, "w0 {} ≈ 3", w[1]);
        assert!((w[2] + 1.5).abs() < 0.01, "w1 {} ≈ −1.5", w[2]);
    }

    #[test]
    fn lambda_shrinks_weights_towards_zero() {
        let x: Vec<Vec<f32>> = (0..30).map(|i| vec![(i % 7) as f32 * 0.3]).collect();
        let y: Vec<f32> = x.iter().map(|v| v[0] * 0.5).collect();
        let loose = fit_ridge(&x, &y, 0.0);
        let tight = fit_ridge(&x, &y, 1e3);
        assert!(
            tight[1].abs() < loose[1].abs(),
            "大 λ 应把权重压小：{} vs {}",
            tight[1],
            loose[1]
        );
    }

    #[test]
    fn is_deterministic_and_handles_empty() {
        let x: Vec<Vec<f32>> = vec![vec![1.0], vec![2.0]];
        let y = vec![1.0f32, 2.0];
        assert_eq!(fit_ridge(&x, &y, 0.1), fit_ridge(&x, &y, 0.1));
        assert_eq!(fit_ridge(&[], &[], 0.1), vec![0.0]);
    }

    #[test]
    fn predicts_the_training_point_reasonably() {
        let x: Vec<Vec<f32>> = (0..12).map(|i| vec![i as f32]).collect();
        let y: Vec<f32> = x
            .iter()
            .map(|v| if v[0] > 5.0 { 1.0 } else { -1.0 })
            .collect();
        let w = fit_ridge(&x, &y, 0.01);
        let s = predict(&w, &[9.0]);
        assert!(s > 0.5, "x=9 应判正，实际 {s}");
        let s = predict(&w, &[1.0]);
        assert!(s < -0.5, "x=1 应判负，实际 {s}");
    }
}
