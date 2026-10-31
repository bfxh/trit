//! 类无关实例评测：前景连通域、标签 IoU 贪心匹配、实例 mIoU。
//!
//! 标签图口径：0 = 背景，1..=n 为实例编号。匹配**与编号无关**（pred 的 7 号可以匹配
//! GT 的 2 号），只按 IoU；每个 pred / GT 标签最多用一次，按 IoU 从大到小贪心。

use std::collections::{HashMap, HashSet, VecDeque};

/// 4 邻域连通域：前景 = `labels[i] != 0`，返回 `(新标签图, 实例数)`，新标签 1..=n。
pub fn components(labels: &[u32], w: usize, h: usize) -> (Vec<u32>, usize) {
    assert_eq!(labels.len(), w * h, "标签图与形状不一致");
    let mut out = vec![0u32; labels.len()];
    let mut stack: Vec<usize> = Vec::new();
    let mut n = 0u32;
    for start in 0..labels.len() {
        if labels[start] == 0 || out[start] != 0 {
            continue;
        }
        n += 1;
        out[start] = n;
        stack.push(start);
        while let Some(p) = stack.pop() {
            let (x, y) = (p % w, p / w);
            let push = |q: usize, out: &mut Vec<u32>, stack: &mut Vec<usize>| {
                if labels[q] != 0 && out[q] == 0 {
                    out[q] = n;
                    stack.push(q);
                }
            };
            if x > 0 {
                push(p - 1, &mut out, &mut stack);
            }
            if x + 1 < w {
                push(p + 1, &mut out, &mut stack);
            }
            if y > 0 {
                push(p - w, &mut out, &mut stack);
            }
            if y + 1 < h {
                push(p + w, &mut out, &mut stack);
            }
        }
    }
    (out, n as usize)
}

/// 前景（label != 0）的像素 IoU。
pub fn fg_iou(pred: &[u32], gt: &[u32]) -> f64 {
    let mut inter = 0usize;
    let mut union = 0usize;
    for (p, g) in pred.iter().zip(gt) {
        let (p, g) = (*p != 0, *g != 0);
        inter += (p && g) as usize;
        union += (p || g) as usize;
    }
    if union == 0 {
        return 1.0;
    }
    inter as f64 / union as f64
}

/// 匹配结果。
pub struct Match {
    /// GT 侧平均 IoU（未匹配的 GT 实例按 0 计）。
    pub miou: f64,
    /// 每个 GT 实例（1..=n_gt）的 IoU。
    pub per_gt: Vec<f64>,
    /// 匹配上的 GT 数。
    pub matched: usize,
    /// pred 侧的标签数（含没匹配上的）。
    pub n_pred: usize,
}

/// 按 IoU 贪心匹配 pred / GT 标签，返回实例 mIoU。
pub fn match_labels(pred: &[u32], gt: &[u32], n_gt: usize) -> Match {
    assert_eq!(pred.len(), gt.len(), "两张标签图长度不一致");
    let mut area_pred: HashMap<u32, usize> = HashMap::new();
    let mut area_gt = vec![0usize; n_gt + 1];
    let mut pairs: HashMap<(u32, u32), usize> = HashMap::new();
    for (&p, &g) in pred.iter().zip(gt) {
        if p != 0 {
            *area_pred.entry(p).or_insert(0) += 1;
        }
        if g != 0 && g as usize <= n_gt {
            area_gt[g as usize] += 1;
            if p != 0 {
                *pairs.entry((p, g)).or_insert(0) += 1;
            }
        }
    }
    let mut cand: Vec<(f64, u32, u32)> = pairs
        .into_iter()
        .map(|((p, g), inter)| {
            let union = area_pred[&p] + area_gt[g as usize] - inter;
            (inter as f64 / union as f64, p, g)
        })
        .collect();
    cand.sort_by(|a, b| b.0.total_cmp(&a.0));
    let mut used_p: HashSet<u32> = HashSet::new();
    let mut per_gt = vec![0.0f64; n_gt + 1];
    let mut matched = 0usize;
    for (iou, p, g) in cand {
        if used_p.contains(&p) || per_gt[g as usize] > 0.0 {
            continue;
        }
        used_p.insert(p);
        per_gt[g as usize] = iou;
        matched += 1;
    }
    let sum: f64 = per_gt.iter().skip(1).sum();
    Match {
        miou: if n_gt == 0 { 1.0 } else { sum / n_gt as f64 },
        per_gt,
        matched,
        n_pred: area_pred.len(),
    }
}

/// 二值前景闭运算（先膨胀 r 再腐蚀 r，方形结构元），用来跨过同一物体部件之间的小缝。
/// 只对 0/非 0 生效，返回 0/1 图；`r = 0` 原样返回。
pub fn close(mask: &[u32], w: usize, h: usize, r: usize) -> Vec<u32> {
    assert_eq!(mask.len(), w * h, "掩码与形状不一致");
    if r == 0 {
        return mask.iter().map(|&v| (v != 0) as u32).collect();
    }
    // 边缘复制填充 r，闭运算才不会在贴边处被削/被胀满
    let (pw, ph) = (w + 2 * r, h + 2 * r);
    let mut a = vec![0u32; pw * ph];
    for y in 0..ph {
        let sy = (y as isize - r as isize).clamp(0, h as isize - 1) as usize;
        for x in 0..pw {
            let sx = (x as isize - r as isize).clamp(0, w as isize - 1) as usize;
            a[y * pw + x] = (mask[sy * w + sx] != 0) as u32;
        }
    }
    dilate(&mut a, pw, ph, r);
    erode(&mut a, pw, ph, r);
    let mut out = vec![0u32; w * h];
    for y in 0..h {
        for x in 0..w {
            out[y * w + x] = a[(y + r) * pw + (x + r)];
        }
    }
    out
}

/// 可分离膨胀（行、列各一次滑窗最大值）。
fn dilate(a: &mut [u32], w: usize, h: usize, r: usize) {
    let mut t = vec![0u32; a.len()];
    for y in 0..h {
        for x in 0..w {
            let lo = x.saturating_sub(r);
            let hi = (x + r).min(w - 1);
            t[y * w + x] = (lo..=hi).map(|i| a[y * w + i]).max().unwrap_or(0);
        }
    }
    for x in 0..w {
        for y in 0..h {
            let lo = y.saturating_sub(r);
            let hi = (y + r).min(h - 1);
            a[y * w + x] = (lo..=hi).map(|j| t[j * w + x]).max().unwrap_or(0);
        }
    }
}

/// 可分离腐蚀；窗口夹在图像内（等价于图外视为前景，边界不会被啃掉）。
fn erode(a: &mut [u32], w: usize, h: usize, r: usize) {
    let mut t = vec![0u32; a.len()];
    for y in 0..h {
        for x in 0..w {
            let lo = x.saturating_sub(r);
            let hi = (x + r).min(w - 1);
            t[y * w + x] = (lo..=hi).map(|i| a[y * w + i]).min().unwrap_or(0);
        }
    }
    for x in 0..w {
        for y in 0..h {
            let lo = y.saturating_sub(r);
            let hi = (y + r).min(h - 1);
            a[y * w + x] = (lo..=hi).map(|j| t[j * w + x]).min().unwrap_or(0);
        }
    }
}

/// 用边界把前景切成实例：先对「前景且非边界」做 4 邻域连通域，再把边界像素
/// 按多源 BFS 归给最近的分量（等距先到先得）。没有边界时退化为连通域。
pub fn split_by_boundary(fg: &[u32], boundary: &[u32], w: usize, h: usize) -> Vec<u32> {
    assert_eq!(fg.len(), w * h, "前景与形状不一致");
    assert_eq!(boundary.len(), w * h, "边界与形状不一致");
    let interior: Vec<u32> = fg
        .iter()
        .zip(boundary)
        .map(|(&f, &b)| (f != 0 && b == 0) as u32)
        .collect();
    let (mut lab, _n) = components(&interior, w, h);
    let mut q: std::collections::VecDeque<usize> =
        (0..lab.len()).filter(|&p| lab[p] != 0).collect();
    while let Some(p) = q.pop_front() {
        let (x, y) = (p % w, p / w);
        let push = |r: usize, lab: &mut Vec<u32>, q: &mut std::collections::VecDeque<usize>| {
            if fg[r] != 0 && lab[r] == 0 {
                lab[r] = lab[p];
                q.push_back(r);
            }
        };
        if x > 0 {
            push(p - 1, &mut lab, &mut q);
        }
        if x + 1 < w {
            push(p + 1, &mut lab, &mut q);
        }
        if y > 0 {
            push(p - w, &mut lab, &mut q);
        }
        if y + 1 < h {
            push(p + w, &mut lab, &mut q);
        }
    }
    lab
}

/// `split_by_boundary` + 小碎片合并：面积 < `min_size` 的连通块不保留，
/// 其像素按多源 BFS 归给最近的大块（`min_size = 0` 时与上面的函数等价）。
pub fn split_by_boundary_min(
    fg: &[u32],
    boundary: &[u32],
    w: usize,
    h: usize,
    min_size: usize,
) -> Vec<u32> {
    let mut lab = split_by_boundary(fg, boundary, w, h);
    if min_size == 0 {
        return lab;
    }
    let mut area: HashMap<u32, usize> = HashMap::new();
    for &v in lab.iter().filter(|&&v| v != 0) {
        *area.entry(v).or_insert(0) += 1;
    }
    let mut q: std::collections::VecDeque<usize> = VecDeque::new();
    for (p, v) in lab.iter_mut().enumerate() {
        if *v != 0 && area[v] < min_size {
            *v = 0;
        } else if *v != 0 {
            q.push_back(p);
        }
    }
    while let Some(p) = q.pop_front() {
        let (x, y) = (p % w, p / w);
        let push = |r: usize, lab: &mut Vec<u32>, q: &mut std::collections::VecDeque<usize>| {
            if fg[r] != 0 && lab[r] == 0 {
                lab[r] = lab[p];
                q.push_back(r);
            }
        };
        if x > 0 {
            push(p - 1, &mut lab, &mut q);
        }
        if x + 1 < w {
            push(p + 1, &mut lab, &mut q);
        }
        if y > 0 {
            push(p - w, &mut lab, &mut q);
        }
        if y + 1 < h {
            push(p + w, &mut lab, &mut q);
        }
    }
    lab
}

/// 实例边界标签：`0` 背景、`1` 实例内部、`2` **实例之间的边界像素**
/// （前景且 `radius` 邻域里存在不同实例号的像素；`radius≥1` 加粗，便于下采样网络学到）。
/// 物体外沿不算边界。
pub fn boundary_of(inst: &[u32], w: usize, h: usize, radius: usize) -> Vec<u32> {
    assert_eq!(inst.len(), w * h, "实例图与形状不一致");
    let mut out = vec![0u32; inst.len()];
    for y in 0..h {
        for x in 0..w {
            let p = y * w + x;
            let id = inst[p];
            if id == 0 {
                continue;
            }
            let mut edge = false;
            'outer: for dy in -(radius as i32)..=(radius as i32) {
                for dx in -(radius as i32)..=(radius as i32) {
                    let (xx, yy) = (x as i32 + dx, y as i32 + dy);
                    if xx < 0 || yy < 0 || xx >= w as i32 || yy >= h as i32 {
                        continue;
                    }
                    let q = yy as usize * w + xx as usize;
                    if inst[q] != 0 && inst[q] != id {
                        edge = true;
                        break 'outer;
                    }
                }
            }
            out[p] = if edge { 2 } else { 1 };
        }
    }
    out
}

/// 部件 id → 实例 id：按**每帧可见实例的像素质心 x 升序**重编号（1..=m）。
/// 世界坐标的实例序号单帧推不出来；按图内左右排序是单帧可学的规范序。
/// 该帧完全不可见的实例不占号（m = 可见实例数）。
pub fn order_by_x(ids: &[i32], ranges: &[(u32, u32)], w: usize) -> Vec<u32> {
    let mut obj_of = HashMap::new();
    for (j, &(f, l)) in ranges.iter().enumerate() {
        for id in f..=l {
            obj_of.insert(id as i32, j);
        }
    }
    let mut sum_x = vec![0f64; ranges.len()];
    let mut cnt = vec![0usize; ranges.len()];
    for (p, &id) in ids.iter().enumerate() {
        if let Some(&j) = obj_of.get(&id) {
            sum_x[j] += (p % w) as f64;
            cnt[j] += 1;
        }
    }
    let mut order: Vec<usize> = (0..ranges.len()).filter(|&j| cnt[j] > 0).collect();
    order.sort_by(|&a, &b| {
        (sum_x[a] / cnt[a] as f64)
            .total_cmp(&(sum_x[b] / cnt[b] as f64))
            .then(a.cmp(&b))
    });
    let mut new_id = HashMap::new();
    for (rank, &j) in order.iter().enumerate() {
        new_id.insert(j, rank as u32 + 1);
    }
    ids.iter()
        .map(|i| {
            obj_of
                .get(i)
                .and_then(|j| new_id.get(j))
                .copied()
                .unwrap_or(0)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn paint(w: usize, blocks: &[(usize, usize, usize, usize, u32)]) -> Vec<u32> {
        let mut v = vec![0u32; w * 8];
        for &(x0, y0, x1, y1, id) in blocks {
            for y in y0..y1 {
                for x in x0..x1 {
                    v[y * w + x] = id;
                }
            }
        }
        v
    }

    #[test]
    fn components_two_blocks() {
        let v = paint(8, &[(0, 0, 2, 2, 5), (5, 5, 7, 7, 9)]);
        let (lab, n) = components(&v, 8, 8);
        assert_eq!(n, 2);
        assert_eq!(lab[0], lab[8 + 1]);
        assert_ne!(lab[0], lab[5 * 8 + 5]);
    }

    #[test]
    fn components_diagonal_is_two() {
        let v = paint(8, &[(1, 1, 3, 3, 1), (3, 3, 5, 5, 1)]);
        let (_lab, n) = components(&v, 8, 8);
        assert_eq!(n, 2, "角接触不算连通（4 邻域）");
    }

    #[test]
    fn match_is_label_invariant() {
        let pred = paint(8, &[(0, 0, 2, 2, 7), (5, 5, 7, 7, 3)]);
        let gt = paint(8, &[(0, 0, 2, 2, 1), (5, 5, 7, 7, 2)]);
        let m = match_labels(&pred, &gt, 2);
        assert!((m.miou - 1.0).abs() < 1e-12);
        assert_eq!(m.matched, 2);
        assert_eq!(m.n_pred, 2);
    }

    #[test]
    fn match_merged_penalized() {
        let pred = paint(8, &[(0, 0, 4, 2, 1)]);
        let gt = paint(8, &[(0, 0, 2, 2, 1), (2, 0, 4, 2, 2)]);
        let m = match_labels(&pred, &gt, 2);
        // 两个 GT 只匹配上一个 ⇒ 平均按 2 个 GT 算：0.5/2
        assert!((m.miou - 0.25).abs() < 1e-12, "miou={}", m.miou);
        assert_eq!(m.matched, 1);
        assert_eq!(m.n_pred, 1);
    }

    #[test]
    fn match_split_penalized() {
        let pred = paint(8, &[(0, 0, 2, 2, 1), (2, 0, 4, 2, 2)]);
        let gt = paint(8, &[(0, 0, 4, 2, 1)]);
        let m = match_labels(&pred, &gt, 1);
        assert!((m.miou - 0.5).abs() < 1e-12, "miou={}", m.miou);
        assert_eq!(m.n_pred, 2, "多出的 pred 实例要被计入 pred 数");
    }

    #[test]
    fn fg_iou_basic() {
        let pred = paint(8, &[(0, 0, 2, 2, 1)]);
        let gt = paint(8, &[(1, 1, 3, 3, 1)]);
        let v = fg_iou(&pred, &gt);
        assert!((v - 1.0 / 7.0).abs() < 1e-12, "v={v}");
    }

    #[test]
    fn close_bridges_small_gap_only() {
        // 两块 2×2，中间 2 px 缝：r=1 连通，r=0 不连通
        let small = paint(8, &[(1, 1, 3, 3, 1), (5, 1, 7, 3, 1)]);
        assert_eq!(components(&small, 8, 8).1, 2, "原始是两块");
        assert_eq!(
            components(&close(&small, 8, 8, 1), 8, 8).1,
            1,
            "r=1 应跨过 2px 缝"
        );
        // 大缝（4 px）r=1 不该被连起来
        let big = paint(8, &[(0, 1, 2, 3, 1), (6, 1, 8, 3, 1)]);
        assert_eq!(components(&close(&big, 8, 8, 1), 8, 8).1, 2, "大缝不该被填");
        // 实心块的形状在闭运算后保持不变（角不被磨圆）
        let solid = paint(8, &[(2, 2, 6, 6, 1)]);
        assert_eq!(close(&solid, 8, 8, 2), solid, "实心块闭运算应幂等");
    }

    #[test]
    fn split_by_boundary_separates_touching() {
        // 8×4 的前景左 3 列与右 5 列相接；第 3 列是边界 ⇒ 应切成 2 个实例
        let w = 8;
        let fg: Vec<u32> = (0..32).map(|p| ((p % w) < 6) as u32).collect();
        let mut bnd = vec![0u32; 32];
        for y in 0..4 {
            bnd[y * w + 3] = 1;
        }
        let lab = split_by_boundary(&fg, &bnd, w, 4);
        let left = lab[0];
        let right = lab[4];
        assert!(left != 0 && right != 0);
        assert_ne!(left, right, "边界应把相接的两块切开");
        assert_eq!(
            lab.iter().filter(|&&v| v > 0).count(),
            24,
            "所有前景像素都要有实例号"
        );
        // 没有边界时退化为连通域：1 个实例
        let none = vec![0u32; 32];
        assert_eq!(
            split_by_boundary(&fg, &none, w, 4).iter().max().copied(),
            Some(1)
        );
    }

    #[test]
    fn min_size_merges_confetti_only() {
        // 两行 12 px 前景，边界落在 x=5、x=9 ⇒ 三段；最右段（4 px）小于 min_size ⇒ 被并掉。
        let (w, h) = (12usize, 2usize);
        let fg = vec![1u32; w * h];
        let mut bnd = vec![0u32; w * h];
        for y in 0..h {
            bnd[y * w + 5] = 1;
            bnd[y * w + 9] = 1;
        }
        let raw = split_by_boundary(&fg, &bnd, w, h);
        assert_eq!(
            raw.iter()
                .filter(|&&v| v != 0)
                .collect::<HashSet<_>>()
                .len(),
            3
        );
        let merged = split_by_boundary_min(&fg, &bnd, w, h, 5);
        let distinct: HashSet<u32> = merged.iter().copied().filter(|&v| v != 0).collect();
        assert_eq!(
            distinct.len(),
            2,
            "小段被并入邻块：raw={raw:?} merged={merged:?}"
        );
        assert!(merged.iter().all(|&v| v != 0), "前景像素不能丢");
    }

    #[test]
    fn boundary_marks_only_between_instances() {
        // 两个实例相邻：交界两侧各一列边界；背景/外沿不标
        let mut inst = paint(8, &[(0, 2, 4, 5, 1), (4, 2, 7, 5, 2)]);
        inst[0] = 0;
        let b = boundary_of(&inst, 8, 8, 1);
        assert_eq!(b[2 * 8 + 3], 2, "左实例贴近边界的列");
        assert_eq!(b[2 * 8 + 4], 2, "右实例贴近边界的列");
        assert_eq!(b[2 * 8 + 1], 1, "内部像素");
        assert_eq!(b[0], 0, "背景");
    }

    #[test]
    fn order_by_x_left_first_and_skips_invisible() {
        // 对象 A = 部件 1..2（右侧），B = 部件 3..4（左侧），C = 部件 5..6（不可见）
        let ids: Vec<i32> = paint(
            8,
            &[
                (5, 0, 7, 2, 1),
                (5, 3, 7, 5, 2),
                (0, 0, 2, 2, 3),
                (0, 3, 2, 5, 4),
            ],
        )
        .into_iter()
        .map(|v| v as i32)
        .collect();
        let out = order_by_x(&ids, &[(1, 2), (3, 4), (5, 6)], 8);
        assert_eq!(out[0], 1, "左侧对象（部件 3/4）排 1");
        assert_eq!(out[5], 2, "右侧对象（部件 1/2）排 2");
        assert_eq!(out.iter().filter(|&&v| v > 0).count(), 16);
        assert!(out.iter().all(|&v| v <= 2), "不可见对象不占号");
    }
}
