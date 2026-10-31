//! 类无关实例评测：前景连通域、标签 IoU 贪心匹配、实例 mIoU。
//!
//! 标签图口径：0 = 背景，1..=n 为实例编号。匹配**与编号无关**（pred 的 7 号可以匹配
//! GT 的 2 号），只按 IoU；每个 pred / GT 标签最多用一次，按 IoU 从大到小贪心。

use std::collections::{HashMap, HashSet};

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
            };            if x > 0 {
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
        .map(|i| obj_of.get(i).and_then(|j| new_id.get(j)).copied().unwrap_or(0))
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
        assert_eq!(lab[0], lab[1 * 8 + 1]);
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
    fn order_by_x_left_first_and_skips_invisible() {
        // 对象 A = 部件 1..2（右侧），B = 部件 3..4（左侧），C = 部件 5..6（不可见）
        let ids: Vec<i32> = paint(8, &[(5, 0, 7, 2, 1), (5, 3, 7, 5, 2), (0, 0, 2, 2, 3), (0, 3, 2, 5, 4)])
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
