//! 断点（进程存档）读写：模型 + Adam + step + RNG。
//!
//! 独立成模块是为了守住「单文件 ≤800 行」的门。格式：
//! `magic(8) | 层数(u32) | step(u64) | rng(u64) | 每层参数…`，**原子写**（先 `.tmp` 再 rename）。

mod tpn;
use tpn::{need, take_f, take_u32, take_u64};

use std::path::Path;

use crate::net::{Layer, Net};

/// 存断点。`TPNT3` 起带 affine 与 RNG；老 `TPNT1/TPNT2` 仍可读。
pub fn save(net: &Net, path: &Path, step: u64, rng: u64) -> std::io::Result<()> {
    let mut b: Vec<u8> = Vec::new();
    b.extend_from_slice(b"TPNT3\0\0\0");
    b.extend_from_slice(&(net.layers.len() as u32).to_le_bytes());
    b.extend_from_slice(&step.to_le_bytes());
    b.extend_from_slice(&rng.to_le_bytes());
    for l in &net.layers {
        for v in [
            l.cin,
            l.cout,
            l.kk,
            l.relu as u32,
            l.latent.len() as u32,
            l.scale.len() as u32,
        ] {
            b.extend_from_slice(&v.to_le_bytes());
        }
        for arr in [&l.latent, &l.scale] {
            for v in arr {
                b.extend_from_slice(&v.to_le_bytes());
            }
        }
        for v in &l.q {
            b.extend_from_slice(&v.to_le_bytes());
        }
        for arr in [
            &l.bias, &l.m_w, &l.v_w, &l.m_s, &l.v_s, &l.m_b, &l.v_b, &l.gamma, &l.beta, &l.m_g,
            &l.v_g, &l.m_be, &l.v_be,
        ] {
            for v in arr {
                b.extend_from_slice(&v.to_le_bytes());
            }
        }
        b.extend_from_slice(&l.t.to_le_bytes());
    }
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, b)?;
    std::fs::rename(&tmp, path)
}

/// 读断点。返回 `(网, step, RNG)`；老存档没有 RNG 时返回 `None`。
pub fn load(path: &Path) -> Result<(Net, u64, Option<u64>), String> {
    let b = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
    if b.len() < 8 {
        return Err("不是 trit-parts 断点文件".into());
    }
    let v3 = &b[0..8] == b"TPNT3\0\0\0";
    let v2 = v3 || &b[0..8] == b"TPNT2\0\0\0";
    if !v2 && &b[0..8] != b"TPNT1\0\0\0" {
        return Err("不是 trit-parts 断点文件".into());
    }
    let mut at = 8usize;
    let n_layers = take_u32(&b, &mut at)? as usize;
    let step = take_u64(&b, &mut at)?;
    let rng = if v3 {
        Some(take_u64(&b, &mut at)?)
    } else {
        None
    };
    let mut layers = Vec::with_capacity(n_layers.min(64));
    for _ in 0..n_layers {
        let cin = take_u32(&b, &mut at)?;
        let cout = take_u32(&b, &mut at)?;
        let kk = take_u32(&b, &mut at)?;
        let relu = take_u32(&b, &mut at)? != 0;
        let n = take_u32(&b, &mut at)? as usize;
        let slots = take_u32(&b, &mut at)? as usize;
        let latent = take_f(&b, &mut at, n)?;
        let scale = take_f(&b, &mut at, slots)?;
        need(&b, at, n)?;
        let mut q = vec![0i32; n];
        for v in &mut q {
            *v = take_u32(&b, &mut at)? as i32;
        }
        let bias = take_f(&b, &mut at, slots)?;
        let m_w = take_f(&b, &mut at, n)?;
        let v_w = take_f(&b, &mut at, n)?;
        let m_s = take_f(&b, &mut at, slots)?;
        let v_s = take_f(&b, &mut at, slots)?;
        let m_b = take_f(&b, &mut at, slots)?;
        let v_b = take_f(&b, &mut at, slots)?;
        let (mut gamma, mut beta) = (vec![1.0f32; slots], vec![0.0f32; slots]);
        let (mut m_g, mut v_g) = (vec![0.0f32; slots], vec![0.0f32; slots]);
        let (mut m_be, mut v_be) = (vec![0.0f32; slots], vec![0.0f32; slots]);
        if v2 {
            gamma = take_f(&b, &mut at, slots)?;
            beta = take_f(&b, &mut at, slots)?;
            m_g = take_f(&b, &mut at, slots)?;
            v_g = take_f(&b, &mut at, slots)?;
            m_be = take_f(&b, &mut at, slots)?;
            v_be = take_f(&b, &mut at, slots)?;
        }
        let t = take_u32(&b, &mut at)?;
        layers.push(Layer {
            latent,
            scale,
            q,
            bias,
            cin,
            cout,
            kk,
            relu,
            grad_e: vec![0.0; n],
            grad_b: vec![0.0; slots],
            m_w,
            v_w,
            m_s,
            v_s,
            m_b,
            v_b,
            t,
            dev: None,
            acc_e: None,
            acc_b: None,
            gamma,
            beta,
            grad_gamma: vec![0.0; slots],
            grad_beta: vec![0.0; slots],
            m_g,
            v_g,
            m_be,
            v_be,
            dev_gb: None,
            acc_gb: None,
            dev_t: None,
            convt: false,
            residual: false,
        });
    }
    let mut net = Net {
        layers,
        head_lr_mult: 1.0,
        freeze_until: 0,
        freeze_head: false,
    };
    net.mark_convt();
    Ok((net, step, rng))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `n` / `slots` 来自文件，分配前必须能判出剩余字节不够。
    /// 修复前这里会按 u32::MAX 先分配 16GB 再读第一项 —— 进程直接 abort。
    #[test]
    fn checkpoint_counts_are_rejected_before_allocating() {
        let mut v = b"TPNT1\0\0\0".to_vec();
        v.extend(1u32.to_le_bytes());
        v.extend(0u64.to_le_bytes());
        for _ in 0..4 {
            v.extend(1u32.to_le_bytes());
        }
        v.extend(u32::MAX.to_le_bytes());
        v.extend(1u32.to_le_bytes());
        let p = std::env::temp_dir().join("trit-state-count-bound.tpn");
        std::fs::write(&p, &v).unwrap();
        match load(&p) {
            Err(e) => assert!(e.contains("截断"), "{e}"),
            Ok(_) => panic!("u32::MAX 的 n 应被拒绝"),
        }
        let _ = std::fs::remove_file(&p);
    }
}
