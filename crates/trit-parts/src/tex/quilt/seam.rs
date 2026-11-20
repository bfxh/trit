//! 接缝的最小误差切割(Efros & Freeman 2001 的 DP boundary),给 `synth.rs` 的
//! `cut`/`tile` 模式用:线性羽化把重叠区整片混色,细纹被磨平(§6.75 hf1 −37%);
//! DP 切割只沿**一条最小误差路径**切,路径两侧各取新旧内容,结构保得住。
//!
//! 竖切:自上而下每行取一个切列,行间步进 ≤1;横切:自左而右每列取一个切行。
//! 代价 = 重叠区内新旧像素的逐通道平方差之和;并列一律取最小下标(确定性,
//! 同输入两次运行逐位一致)。

/// 竖切:`err[y * w + x]`(w = 带宽,通常是重叠宽度;h = patch 高),
/// 返回每行的切列 `cut[y] ∈ [0, w)`。路径左侧保留旧内容,右侧取新内容。
pub(crate) fn vertical_cut(err: &[f32], w: usize, h: usize) -> Vec<usize> {
    assert!(w > 0 && h > 0, "切割带不能为空");
    assert!(err.len() == w * h, "误差场尺寸与 w×h 不符");
    let mut cost = vec![0.0f32; w * h];
    cost[..w].copy_from_slice(&err[..w]);
    for y in 1..h {
        for x in 0..w {
            let mut m = cost[(y - 1) * w + x];
            if x > 0 {
                m = m.min(cost[(y - 1) * w + x - 1]);
            }
            if x + 1 < w {
                m = m.min(cost[(y - 1) * w + x + 1]);
            }
            cost[y * w + x] = err[y * w + x] + m;
        }
    }
    let mut cut = vec![0usize; h];
    let (mut best, mut x) = (f32::INFINITY, 0usize);
    for i in 0..w {
        let c = cost[(h - 1) * w + i];
        if c < best {
            best = c;
            x = i;
        }
    }
    cut[h - 1] = x;
    for y in (1..h).rev() {
        let lo = x.saturating_sub(1);
        let hi = (x + 1).min(w - 1);
        let mut m = f32::INFINITY;
        let mut bx = x;
        for i in lo..=hi {
            let c = cost[(y - 1) * w + i];
            if c < m {
                m = c;
                bx = i;
            }
        }
        cut[y - 1] = bx;
        x = bx;
    }
    cut
}

/// 横切:`err[y * w + x]`(w = patch 宽,h = 带高,通常是重叠高度),
/// 返回每列的切行 `cut[x] ∈ [0, h)`。路径上方保留旧内容,下方取新内容。
pub(crate) fn horizontal_cut(err: &[f32], w: usize, h: usize) -> Vec<usize> {
    assert!(w > 0 && h > 0, "切割带不能为空");
    assert!(err.len() == w * h, "误差场尺寸与 w×h 不符");
    let mut cost = vec![0.0f32; w * h];
    for y in 0..h {
        cost[y * w] = err[y * w];
    }
    for x in 1..w {
        for y in 0..h {
            let mut m = cost[y * w + x - 1];
            if y > 0 {
                m = m.min(cost[(y - 1) * w + x - 1]);
            }
            if y + 1 < h {
                m = m.min(cost[(y + 1) * w + x - 1]);
            }
            cost[y * w + x] = err[y * w + x] + m;
        }
    }
    let mut cut = vec![0usize; w];
    let (mut best, mut y) = (f32::INFINITY, 0usize);
    for i in 0..h {
        let c = cost[i * w + w - 1];
        if c < best {
            best = c;
            y = i;
        }
    }
    cut[w - 1] = y;
    for x in (1..w).rev() {
        let lo = y.saturating_sub(1);
        let hi = (y + 1).min(h - 1);
        let mut m = f32::INFINITY;
        let mut by = y;
        for i in lo..=hi {
            let c = cost[i * w + x - 1];
            if c < m {
                m = c;
                by = i;
            }
        }
        cut[x - 1] = by;
        y = by;
    }
    cut
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 整列零代价 ⇒ 最优路径贴着这一列走。
    #[test]
    fn vertical_path_hugs_the_zero_cost_column() {
        let (w, h) = (5usize, 6usize);
        let mut err = vec![1.0f32; w * h];
        for y in 0..h {
            err[y * w + 2] = 0.0;
        }
        assert_eq!(vertical_cut(&err, w, h), vec![2; h]);
    }

    /// 整行零代价 ⇒ 横切贴着这一行走。
    #[test]
    fn horizontal_path_hugs_the_zero_cost_row() {
        let (w, h) = (6usize, 5usize);
        let mut err = vec![1.0f32; w * h];
        for x in 0..w {
            err[3 * w + x] = 0.0;
        }
        assert_eq!(horizontal_cut(&err, w, h), vec![3; w]);
    }

    /// 斜坡场:零列从 x=0 逐行移到 x=w-1,路径应跟着斜过去(步进 ≤1 足够)。
    #[test]
    fn vertical_path_follows_a_gentle_slope() {
        let (w, h) = (6usize, 6usize);
        let mut err = vec![1.0f32; w * h];
        for y in 0..h {
            err[y * w + y.min(w - 1)] = 0.0;
        }
        let cut = vertical_cut(&err, w, h);
        for (y, c) in cut.iter().enumerate() {
            assert_eq!(*c, y.min(w - 1), "第 {y} 行应切在零代价列");
        }
    }

    /// 全零场:并列取最小下标 ⇒ 全部切在 0(确定性)。
    #[test]
    fn all_zero_field_is_deterministic_at_the_first_index() {
        let err = vec![0.0f32; 4 * 7];
        assert_eq!(vertical_cut(&err, 4, 7), vec![0; 7]);
        assert_eq!(horizontal_cut(&err, 7, 4), vec![0; 7]);
    }

    /// 切割路径的行间步进必须 ≤1(否则两行内容错位,接缝反而更硬)。
    #[test]
    fn vertical_path_never_jumps_more_than_one_column() {
        let (w, h) = (8usize, 9usize);
        // 每行的零代价列交错摆动:0 与 7 交替,DP 无法同时命中,只会取折中
        let mut err = vec![1.0f32; w * h];
        for y in 0..h {
            err[y * w + if y % 2 == 0 { 0 } else { 7 }] = 0.0;
        }
        let cut = vertical_cut(&err, w, h);
        for w2 in cut.windows(2) {
            assert!(
                w2[0].abs_diff(w2[1]) <= 1,
                "相邻行切点步进 ≤1，实际 {} → {}",
                w2[0],
                w2[1]
            );
        }
    }
}
