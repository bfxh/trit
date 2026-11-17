//! Q12 定点 forward：与 GPU `serial_fixed.wgsl` 逐位一致。

use crate::model::{J, O_LEN};
use crate::topk::top16;
use crate::weights::Weights;

const PRIME: i64 = 1_000_003;
const Q: f32 = 2048.0;

fn fx(x: f32) -> i32 {
    (x * Q).round() as i32
}

pub fn forward_fixed(w: &Weights, qids: &[i64], oids: &[i64]) -> [f32; 4] {
    let mut qb = vec![0f32; w.c];
    for &id in qids {
        let c = w.concept[id as usize];
        if c > 0 {
            qb[c as usize] = w.idf[c as usize];
        }
    }
    let top = top16(&qb, w.c);

    let mut read = vec![0i32; w.c];
    for i in 0..J {
        for j in (i + 1)..J {
            let strength = qb[top[i].0] * qb[top[j].0];
            if strength == 0.0 {
                continue;
            }
            let sq = fx(strength) as i64;
            let (x, y) = (top[i].0, top[j].0);
            let (lo, hi) = if x < y { (x, y) } else { (y, x) };
            let row = ((lo as i64 * PRIME + hi as i64) % w.h as i64) as usize;
            let base = row * w.r;
            for k in 0..w.r {
                let c = w.row_ids[base + k];
                if c >= 0 {
                    let rq = fx(w.row_w[base + k]) as i64;
                    read[c as usize] = read[c as usize].wrapping_add(((sq * rq) >> 11) as i32);
                }
            }
        }
    }

    let mut acc = vec![0i32; w.c];
    for (d, &v) in qb.iter().enumerate().skip(1) {
        if v == 0.0 {
            continue;
        }
        let vq = fx(v) as i64;
        let row = &w.a[d * w.c..(d + 1) * w.c];
        for c in 0..w.c {
            let aq = fx(row[c]) as i64;
            acc[c] = acc[c].wrapping_add(((vq * aq) >> 11) as i32);
        }
    }

    let mixq = fx(w.mix) as i64;
    for c in 0..w.c {
        read[c] = if qb[c] == 0.0 {
            read[c].wrapping_add(((mixq * acc[c] as i64) >> 11) as i32)
        } else {
            0
        };
    }

    let mut logits = [0f32; 4];
    for k in 0..4 {
        let mut ob = vec![0i32; w.c];
        for &id in &oids[k * O_LEN..(k + 1) * O_LEN] {
            let c = w.concept[id as usize];
            if c > 0 && qb[c as usize] == 0.0 {
                ob[c as usize] = fx(w.idf[c as usize]);
            }
        }
        let mut score = 0f32;
        for c in 0..w.c {
            if read[c] != 0 && ob[c] != 0 {
                score += (read[c] as f32 / Q) * (ob[c] as f32 / Q);
            }
        }
        logits[k] = score;
    }
    logits
}

/// 按 `WSP_FIXED` 选 CPU forward：定点版或 f32 版。
pub fn cpu_forward(w: &Weights, qids: &[i64], oids: &[i64]) -> [f32; 4] {
    if std::env::var("WSP_FIXED").is_ok_and(|v| v == "1") {
        forward_fixed(w, qids, oids)
    } else {
        crate::model::forward(w, qids, oids)
    }
}
