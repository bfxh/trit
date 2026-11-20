//! 给外部管线用的最小 API：一帧 RGB → 部件 id 图。

use crate::model::{Backend, res_is_backend_usable};
use crate::parts::PartList;
#[cfg(feature = "gpu")]
use crate::tpm::TpmModel;

pub const MAX_RES: u32 = 8192;

pub fn infer_ids(
    backend: &mut dyn Backend,
    rgb: &[f32],
    res: u32,
    parts: &PartList,
) -> Result<Vec<u32>, String> {
    if !res_is_backend_usable(res) {
        return Err(format!("分辨率必须是 4 的正整数倍（收到 {res}）"));
    }
    let logits = backend.forward(rgb, res)?;
    decode_ids(&logits, res, parts)
}

/// 从 `k*3+1` 通道的 NHWC logits 解出 id 图（与 `parts-cli run` 同口径）。
pub fn decode_ids(logits: &[f32], res: u32, parts: &PartList) -> Result<Vec<u32>, String> {
    if res == 0 || res > MAX_RES {
        return Err(format!("分辨率必须在 1..={MAX_RES}（收到 {res}）"));
    }
    let hw = res as usize * res as usize;
    let k = parts.k;
    if logits.len() % hw != 0 {
        return Err(format!(
            "logits 长度 {} 应为 hw={hw} 的整数倍",
            logits.len()
        ));
    }
    let ch = logits.len() / hw;
    if ch <= k * 3 {
        return Err(format!("通道数 {ch} 少于 k*3+1={}", k * 3 + 1));
    }
    let mut ids = vec![0u32; hw];
    for p in 0..hw {
        if logits[p * ch + k * 3] <= 0.0 {
            continue;
        }
        ids[p] = parts.decode_constrained(&logits[p * ch..p * ch + k * 3]);
    }
    Ok(ids)
}

/// 一次前向拿到 `(logits, d1 特征)`；域外闸门与分割共用同一次推理。
#[cfg(feature = "gpu")]
pub fn forward_logits_feats(
    net: &crate::net::Net,
    gpu: &crate::gpu::Gpu,
    rgb: &[f32],
    res: u32,
) -> Result<(Vec<f32>, Vec<f32>), String> {
    if !res_is_backend_usable(res) {
        return Err(format!("分辨率必须是 4 的正整数倍（收到 {res}）"));
    }
    let xb = gpu.buf(rgb, "x");
    let tr = net.forward(gpu, xb, res);
    let hw = res as usize * res as usize;
    let ch = net.head_cout() as usize;
    let logits = gpu.read(&tr.out, hw * ch);
    let zc = net.layers[crate::net::D1B].cout as usize;
    let z = gpu.read(&tr.z, hw * zc);
    Ok((logits, z))
}

#[cfg(feature = "gpu")]
pub fn wgpu_from_tpm(m: &TpmModel) -> Result<crate::backend_wgpu::WgpuBackend, String> {
    crate::backend_wgpu::WgpuBackend::from_tpm(m)
}

#[cfg(test)]
mod tests;
