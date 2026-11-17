//! CPU 预处理：top-16、pair 行号、qb 位图、选项概念表。

use crate::model::{J, O_LEN, Q_LEN};
use crate::weights::Weights;

pub const PAIRS: usize = 256;
pub const NONE: u32 = u32::MAX;
const PRIME: i64 = 1_000_003;

pub struct Prepared {
    pub pair_rows: Vec<u32>,
    pub top_idx: Vec<u32>,
    pub top_val: Vec<f32>,
    pub qb_ids: Vec<u32>,
    pub qb_vals: Vec<f32>,
    pub qb_mask: Vec<u32>,
    pub ob_ids: Vec<u32>,
    pub ob_w: Vec<f32>,
}

fn top16(qb: &[f32], c: usize) -> [(usize, f32); J] {
    let mut order: Vec<usize> = (1..c).collect();
    if c > J {
        order.select_nth_unstable_by(J - 1, |&x, &y| {
            qb[y].partial_cmp(&qb[x]).expect("有限值").then(x.cmp(&y))
        });
        order.truncate(J);
    }
    let mut out = [(0usize, 0f32); J];
    for (i, &c) in order.iter().enumerate() {
        out[i] = (c, qb[c]);
    }
    out
}

pub fn prepare(w: &Weights, q: &[i64], o: &[i64], n: usize) -> Prepared {
    let words = w.c.div_ceil(32);
    let mut pair_rows = vec![0u32; n * PAIRS];
    let mut top_idx = vec![0u32; n * J];
    let mut top_val = vec![0f32; n * J];
    let mut qb_mask = vec![0u32; n * words];
    let mut qb_ids = vec![NONE; n * Q_LEN];
    let mut qb_vals = vec![0f32; n * Q_LEN];
    let mut ob_ids = vec![NONE; n * 4 * O_LEN];
    let mut ob_w = vec![0f32; n * 4 * O_LEN];
    for i in 0..n {
        let mut qb = vec![0f32; w.c];
        for &id in &q[i * Q_LEN..(i + 1) * Q_LEN] {
            let c = w.concept[id as usize];
            if c > 0 {
                let c = c as usize;
                qb[c] = w.idf[c];
                qb_mask[i * words + c / 32] |= 1u32 << (c % 32);
            }
        }
        let mut used = 0usize;
        for (c, &v) in qb.iter().enumerate().skip(1) {
            if v != 0.0 {
                qb_ids[i * Q_LEN + used] = c as u32;
                qb_vals[i * Q_LEN + used] = v;
                used += 1;
            }
        }
        let top = top16(&qb, w.c);
        for d in 0..J {
            top_idx[i * J + d] = top[d].0 as u32;
            top_val[i * J + d] = top[d].1;
        }
        for a in 0..J {
            for b in (a + 1)..J {
                let (x, y) = (top[a].0, top[b].0);
                let (lo, hi) = if x < y { (x, y) } else { (y, x) };
                let row = ((lo as i64 * PRIME + hi as i64) % w.h as i64) as u32;
                pair_rows[i * PAIRS + a * J + b] = row;
            }
        }
        for k in 0..4 {
            let mut seen: Vec<usize> = Vec::new();
            for idx in 0..O_LEN {
                let c = w.concept[o[(i * 4 + k) * O_LEN + idx] as usize];
                if c > 0 {
                    let c = c as usize;
                    let in_q = (qb_mask[i * words + c / 32] >> (c % 32)) & 1 == 1;
                    if !in_q && !seen.contains(&c) {
                        seen.push(c);
                    }
                }
            }
            for (j, &c) in seen.iter().enumerate() {
                ob_ids[(i * 4 + k) * O_LEN + j] = c as u32;
                ob_w[(i * 4 + k) * O_LEN + j] = w.idf[c];
            }
        }
    }
    Prepared {
        pair_rows,
        top_idx,
        top_val,
        qb_ids,
        qb_vals,
        qb_mask,
        ob_ids,
        ob_w,
    }
}
