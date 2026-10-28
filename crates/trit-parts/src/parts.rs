//! 部件清单与解码：无约束 argmax / **清单约束 argmax** / 评估指标。
//!
//! 约束解码不用「先乱猜再投影」，而是**直接在清单上取 argmax**：
//! `id* = argmax_{id ∈ 清单} Σ_i logit[i][digit_i(id)]`
//! ⇒ 结构上不可能输出清单外 id（投影前的未知率必须是 0，否则就是实现错）。
//!
//! 评估口径与 Python `parts_head_planes.evaluate` 一致：平面准确率只在 **GT 部件像素**上统计；
//! `unknown_part_rate` 按**每帧不同 id 的个数**统计（不是像素质量），本实现两种都给。

use std::path::Path;

/// 部件清单（只取解码要用的部分：k、id、digits）。
pub struct PartList {
    pub k: usize,
    /// `(id, digits)`，按 id 升序；`digits` 是 MSB 优先的 k 位三进制。
    pub parts: Vec<(u32, Vec<u8>)>,
}

impl PartList {
    /// 从 `part_index` 的 manifest JSON 读清单（`{"k":5,"parts":[{"id":1,"digits":[0,0,0,0,0]},…]}`）。
    pub fn load(path: &Path) -> Result<Self, String> {
        let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
        Self::parse(&text)
    }

    pub fn parse(text: &str) -> Result<Self, String> {
        let k = field_num(text, "\"k\"").ok_or("清单缺 k")?;
        let mut parts: Vec<(u32, Vec<u8>)> = Vec::new();
        let mut cursor = 0usize;
        let mut last_id: Option<u32> = None;
        while let Some(pos) = text[cursor..].find('"') {
            let at = cursor + pos;
            let rest = &text[at..];
            if let Some(after) = rest.strip_prefix("\"id\":") {
                last_id = field_num(after, "").map(|v| v as u32);
                cursor = at + 5;
            } else if let Some(after) = rest.strip_prefix("\"digits\":") {
                let open = after.find('[').ok_or("digits 缺 [")?;
                let close = after.find(']').ok_or("digits 缺 ]")?;
                let digits: Vec<u8> = after[open + 1..close]
                    .split(',')
                    .filter_map(|s| s.trim().parse().ok())
                    .collect();
                let id = last_id.ok_or("digits 前没有 id")?;
                if digits.len() != k {
                    return Err(format!("id {id}: digits 长度 {} != k {k}", digits.len()));
                }
                parts.push((id, digits));
                cursor = at + 9;
            } else {
                cursor = at + 1;
            }
        }
        parts.sort_by_key(|p| p.0);
        if parts.is_empty() {
            return Err("清单里没有部件".into());
        }
        Ok(Self { k, parts })
    }

    /// 三进制数字 → id（`id = 1 + Σ dᵢ·3^(k−1−i)`，0 留给背景）。
    pub fn id_of(&self, digits: &[u8]) -> u32 {
        let mut t = 0u32;
        for &d in digits {
            t = t * 3 + d as u32;
        }
        t + 1
    }

    /// id → 三进制数字（MSB 优先）。
    pub fn digits_of(&self, id: u32) -> Vec<u8> {
        let mut t = id.saturating_sub(1);
        let mut out = vec![0u8; self.k];
        for i in (0..self.k).rev() {
            out[i] = (t % 3) as u8;
            t /= 3;
        }
        out
    }

    pub fn contains(&self, id: u32) -> bool {
        self.parts.binary_search_by_key(&id, |p| p.0).is_ok()
    }

    /// 无约束解码：每条平面各自取 argmax，再拼成 id（可能落在清单外）。
    pub fn decode_free(&self, logits: &[f32]) -> u32 {
        let mut digits = vec![0u8; self.k];
        for (i, slot) in digits.iter_mut().enumerate() {
            let z = &logits[i * 3..i * 3 + 3];
            let mut best = 0usize;
            for c in 1..3 {
                if z[c] > z[best] {
                    best = c;
                }
            }
            *slot = best as u8;
        }
        self.id_of(&digits)
    }

    /// 清单约束解码：在清单里取 `Σ logit[i][digit]` 最大的 id；同分取小 id（确定性）。
    pub fn decode_constrained(&self, logits: &[f32]) -> u32 {
        let mut best_id = self.parts[0].0;
        let mut best_score = f32::NEG_INFINITY;
        for (id, digits) in &self.parts {
            let mut s = 0f32;
            for (i, &d) in digits.iter().enumerate() {
                s += logits[i * 3 + d as usize];
            }
            if s > best_score {
                best_score = s;
                best_id = *id;
            }
        }
        best_id
    }
}

/// 一帧的评估结果。
pub struct FrameMetrics {
    pub plane_acc: Vec<f32>,
    pub id_exact: Option<f32>,
    pub part_iou: Vec<f32>,
    /// 该帧出现过的、清单外的**不同** id 个数与总不同 id 个数（投影前）。
    pub unknown_ids: (usize, usize),
    /// 清单外 id 覆盖的**像素占比**（投影前，按预测 valid 与 GT valid 的交集）。
    pub unknown_pixels: (usize, usize),
}

/// 评估一帧。`constrained = true` 时用清单约束解码（结构上不可能出现清单外 id）。
#[allow(clippy::too_many_arguments)]
pub fn evaluate_frame(
    logits: &[f32],
    planes: &[Vec<u8>],
    valid_gt: &[u8],
    manifest: &PartList,
    constrained: bool,
    use_pred_valid: bool,
) -> FrameMetrics {
    let k = manifest.k;
    let ch = k * 3 + 1;
    let hw = valid_gt.len();
    assert_eq!(logits.len(), hw * ch);

    let mut hit = vec![0usize; k];
    let mut seen = 0usize;
    let mut exact = 0usize;
    let mut masked = 0usize;
    let mut inter = std::collections::HashMap::<u32, usize>::new();
    let mut union = std::collections::HashMap::<u32, usize>::new();
    let mut gt_ids = std::collections::HashSet::<u32>::new();
    let mut pred_ids = std::collections::HashSet::<u32>::new();
    let mut unknown_px = 0usize;
    let mut valid_px = 0usize;

    for p in 0..hw {
        let base = p * ch;
        if valid_gt[p] == 1 {
            seen += 1;
            for i in 0..k {
                let z = &logits[base + i * 3..base + i * 3 + 3];
                let mut best = 0usize;
                for c in 1..3 {
                    if z[c] > z[best] {
                        best = c;
                    }
                }
                if best as u8 == planes[i][p] {
                    hit[i] += 1;
                }
            }
        }
        let v = logits[base + k * 3];
        let vpred = if use_pred_valid { (v > 0.0) as u8 } else { 1 };
        let raw = if constrained {
            manifest.decode_constrained(&logits[base..base + k * 3])
        } else {
            manifest.decode_free(&logits[base..base + k * 3])
        };
        let gt_id = {
            let digits: Vec<u8> = (0..k).map(|i| planes[i][p]).collect();
            if valid_gt[p] == 1 {
                manifest.id_of(&digits)
            } else {
                0
            }
        };
        if gt_id > 0 {
            gt_ids.insert(gt_id);
        }
        if raw > 0 && vpred == 1 {
            pred_ids.insert(raw);
        }
        if valid_gt[p] == 1 && vpred == 1 {
            valid_px += 1;
            masked += 1;
            if raw == gt_id {
                exact += 1;
            }
            if raw > 0 && !manifest.contains(raw) {
                unknown_px += 1;
            }
        }
        if gt_id > 0 {
            *union.entry(gt_id).or_default() += 1;
            if raw == gt_id && vpred == 1 {
                *inter.entry(gt_id).or_default() += 1;
            }
        }
    }

    let plane_acc = hit
        .iter()
        .map(|&h| {
            if seen > 0 {
                h as f32 / seen as f32
            } else {
                0.0
            }
        })
        .collect();
    let mut ious: Vec<f32> = Vec::new();
    for id in gt_ids {
        let u = union.get(&id).copied().unwrap_or(0);
        if u > 0 {
            ious.push(inter.get(&id).copied().unwrap_or(0) as f32 / u as f32);
        }
    }
    let unknown_total = pred_ids.len();
    let unknown_bad = pred_ids
        .iter()
        .filter(|id| !manifest.contains(**id))
        .count();
    FrameMetrics {
        plane_acc,
        id_exact: if masked > 0 {
            Some(exact as f32 / masked as f32)
        } else {
            None
        },
        part_iou: ious,
        unknown_ids: (unknown_bad, unknown_total),
        unknown_pixels: (unknown_px, valid_px),
    }
}

/// 读 `"key": 123` 的第一个整数（只用于清单解析）。
fn field_num(text: &str, key: &str) -> Option<usize> {
    let rest = if key.is_empty() {
        text
    } else {
        let i = text.find(key)? + key.len();
        &text[i..]
    };
    let rest = rest.trim_start();
    let rest = rest.strip_prefix(':').unwrap_or(rest).trim_start();
    let end = rest
        .find(|c: char| !c.is_ascii_digit())
        .unwrap_or(rest.len());
    rest[..end].parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn demo() -> PartList {
        // k=2 的小清单：id 1=[0,0]、2=[0,1]、4=[1,0]
        PartList::parse(r#"{"k":2,"parts":[{"id":1,"digits":[0,0]},{"id":2,"digits":[0,1]},{"id":4,"digits":[1,0]}]}"#)
            .unwrap()
    }

    #[test]
    fn id_digits_roundtrip() {
        let m = demo();
        for (id, digits) in &m.parts {
            assert_eq!(m.id_of(digits), *id);
            assert_eq!(&m.digits_of(*id), digits);
        }
    }

    #[test]
    fn constrained_stays_inside_manifest() {
        let m = demo();
        let logits = vec![0.1, 0.2, 3.0, 0.0, 0.5, 0.0];
        assert_eq!(m.decode_free(&logits), 8, "无约束会落到清单外的 id 8");
        let got = m.decode_constrained(&logits);
        assert!(m.contains(got));
    }

    #[test]
    fn constrained_picks_best_valid() {
        let m = demo();
        // 平面0 偏好 1（digit 1），平面1 偏好 0 → 清单里最匹配的是 id 4 [1,0]
        let logits = vec![0.0, 2.0, 0.0, 3.0, 0.0, 0.0];
        assert_eq!(m.decode_constrained(&logits), 4);
    }

    #[test]
    fn evaluate_metrics_behave() {
        let m = demo();
        let hw = 4usize;
        // 两个部件像素（id 4），两个背景像素
        let planes = vec![vec![1u8, 1, 0, 0], vec![0u8, 0, 0, 0]];
        let valid = vec![1u8, 1, 0, 0];
        let ch = 7;
        let mut logits = vec![0f32; hw * ch];
        for p in 0..2 {
            logits[p * ch] = 0.0; // plane0 ch0
            logits[p * ch + 1] = 5.0; // plane0 ch1 -> digit 1
            logits[p * ch + 3] = 5.0; // plane1 ch0 -> digit 0
            logits[p * ch + 6] = -5.0; // valid
        }
        for p in 2..hw {
            logits[p * ch] = 5.0;
            logits[p * ch + 3] = 5.0;
            logits[p * ch + 6] = -5.0;
        }
        let fm = evaluate_frame(&logits, &planes, &valid, &m, true, false);
        assert_eq!(fm.id_exact, Some(1.0));
        // 出现两种 id（部件 4、背景 [0,0]→1），都在清单里 ⇒ 清单外不同 id 数 = 0
        assert_eq!(fm.unknown_ids, (0, 2));
        assert_eq!(fm.plane_acc, vec![1.0, 1.0]);
    }
}
