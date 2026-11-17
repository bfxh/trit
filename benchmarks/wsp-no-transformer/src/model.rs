//! mem_assoc2 的 forward（论文 `arch.py` 的 Rust 镜像）。

use crate::weights::Weights;

pub const J: usize = 16;
const PRIME: i64 = 1_000_003;
pub const Q_LEN: usize = 128;
pub const O_LEN: usize = 48;

/// 一题的 forward：返回四个选项的分数。
pub fn forward(w: &Weights, q: &[i64], o: &[i64]) -> [f32; 4] {
    // 问题概念袋：一个概念出现一次即为 idf（scatter_ 置 1，不累加）。
    let mut qb = vec![0f32; w.c];
    for &id in q {
        let c = w.concept[id as usize];
        if c > 0 {
            qb[c as usize] = w.idf[c as usize];
        }
    }

    let top = crate::topk::top16(&qb, w.c);

    // 二阶读取：120 个概念对各自读一行。
    let mut read = vec![0f32; w.c];
    for i in 0..J {
        for j in (i + 1)..J {
            let (x, y) = (top[i].0, top[j].0);
            let strength = qb[x] * qb[y];
            if strength == 0.0 {
                continue;
            }
            let (lo, hi) = if x < y { (x, y) } else { (y, x) };
            let row = ((lo as i64 * PRIME + hi as i64) % w.h as i64) as usize;
            let base = row * w.r;
            for k in 0..w.r {
                let c = w.row_ids[base + k];
                if c >= 0 {
                    read[c as usize] += strength * w.row_w[base + k];
                }
            }
        }
    }

    // 一阶读取：只对 qb 非零的概念求和（等价于 qb @ A）。
    let mut acc = vec![0f32; w.c];
    for (d, &v) in qb.iter().enumerate().skip(1) {
        if v == 0.0 {
            continue;
        }
        let row = &w.a[d * w.c..(d + 1) * w.c];
        for c in 0..w.c {
            acc[c] += v * row[c];
        }
    }
    for c in 0..w.c {
        let masked = if qb[c] == 0.0 {
            read[c] + w.mix * acc[c]
        } else {
            0.0
        };
        read[c] = masked;
    }

    // 选项得分：read 覆盖了选项里多少概念（选项只保留问题没有的概念）。
    let mut logits = [0f32; 4];
    for (k, logit) in logits.iter_mut().enumerate() {
        let mut ob = vec![0f32; w.c];
        for &id in &o[k * O_LEN..(k + 1) * O_LEN] {
            let c = w.concept[id as usize];
            if c > 0 && qb[c as usize] == 0.0 {
                ob[c as usize] = w.idf[c as usize];
            }
        }
        *logit = read.iter().zip(ob.iter()).map(|(x, y)| x * y).sum::<f32>();
    }
    logits
}

pub fn argmax(scores: &[f32]) -> usize {
    let mut best = 0;
    for (i, v) in scores.iter().enumerate() {
        if *v > scores[best] {
            best = i;
        }
    }
    best
}
