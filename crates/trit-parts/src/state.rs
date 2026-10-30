//! 断点（进程存档）读写：模型 + Adam + step + RNG。
//!
//! 独立成模块是为了守住「单文件 ≤800 行」的门。格式：
//! `magic(8) | 层数(u32) | step(u64) | rng(u64) | 每层参数…`，**原子写**（先 `.tmp` 再 rename）。

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
    let mut layers = Vec::with_capacity(n_layers);
    for _ in 0..n_layers {
        let cin = take_u32(&b, &mut at)?;
        let cout = take_u32(&b, &mut at)?;
        let kk = take_u32(&b, &mut at)?;
        let relu = take_u32(&b, &mut at)? != 0;
        let n = take_u32(&b, &mut at)? as usize;
        let slots = take_u32(&b, &mut at)? as usize;
        let mut latent = vec![0.0f32; n];
        for v in &mut latent {
            *v = f32::from_bits(take_u32(&b, &mut at)?);
        }
        let mut scale = vec![0.0f32; slots];
        for v in &mut scale {
            *v = f32::from_bits(take_u32(&b, &mut at)?);
        }
        let mut q = vec![0i32; n];
        for v in &mut q {
            *v = take_u32(&b, &mut at)? as i32;
        }
        let mut bias = vec![0.0f32; slots];
        for v in &mut bias {
            *v = f32::from_bits(take_u32(&b, &mut at)?);
        }
        let take_f = |count: usize, b: &[u8], at: &mut usize| -> Result<Vec<f32>, String> {
            let mut v = vec![0.0f32; count];
            for x in &mut v {
                *x = f32::from_bits(take_u32(b, at)?);
            }
            Ok(v)
        };
        let m_w = take_f(n, &b, &mut at)?;
        let v_w = take_f(n, &b, &mut at)?;
        let m_s = take_f(slots, &b, &mut at)?;
        let v_s = take_f(slots, &b, &mut at)?;
        let m_b = take_f(slots, &b, &mut at)?;
        let v_b = take_f(slots, &b, &mut at)?;
        let (mut gamma, mut beta) = (vec![1.0f32; slots], vec![0.0f32; slots]);
        let (mut m_g, mut v_g) = (vec![0.0f32; slots], vec![0.0f32; slots]);
        let (mut m_be, mut v_be) = (vec![0.0f32; slots], vec![0.0f32; slots]);
        if v2 {
            for arr in [
                &mut gamma, &mut beta, &mut m_g, &mut v_g, &mut m_be, &mut v_be,
            ] {
                for v in arr.iter_mut() {
                    *v = f32::from_bits(take_u32(&b, &mut at)?);
                }
            }
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
    };
    net.mark_convt();
    Ok((net, step, rng))
}

fn take_u32(b: &[u8], at: &mut usize) -> Result<u32, String> {
    let s = b.get(*at..*at + 4).ok_or("断点文件截断")?;
    *at += 4;
    Ok(u32::from_le_bytes([s[0], s[1], s[2], s[3]]))
}

fn take_u64(b: &[u8], at: &mut usize) -> Result<u64, String> {
    let s = b.get(*at..*at + 8).ok_or("断点文件截断")?;
    *at += 8;
    let mut a = [0u8; 8];
    a.copy_from_slice(s);
    Ok(u64::from_le_bytes(a))
}
