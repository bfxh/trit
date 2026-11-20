//! 给外部管线用的最小 API：一帧 RGB → 部件 id 图。
//!
//! 设计意图见 `README.md`「与生成管线的联动」：这个 crate 是**可插拔下游节点**，
//! 渲染/生成端只要能出 RGB，就能拿到 id 图；调用方只需拿到一个 `Backend`
//! （`WgpuBackend` 或 `RefCpu`）和一份 `PartList`。

use crate::model::{Backend, res_is_backend_usable};
use crate::parts::PartList;
#[cfg(feature = "gpu")]
use crate::tpm::TpmModel;

/// 跑一帧并解成 id 图（清单约束解码；预测为"无部件"的像素置 0）。
pub fn infer_ids(
    backend: &mut dyn Backend,
    rgb: &[f32],
    res: u32,
    parts: &PartList,
) -> Result<Vec<u32>, String> {
    assert!(
        res_is_backend_usable(res),
        "分辨率必须是 4 的正整数倍（收到 {res}）"
    );
    let logits = backend.forward(rgb, res)?;
    Ok(decode_ids(&logits, res, parts))
}

/// 从 `k*3+1` 通道的 NHWC logits 解出 id 图（与 `parts-cli run` 同口径）。
pub fn decode_ids(logits: &[f32], res: u32, parts: &PartList) -> Vec<u32> {
    // `res` 是外部给的分辨率，这里不经过网络，所以只要求非 0：`hw` 要拿去取模，
    // `res = 0` 会让下一行的 `logits.len() % hw` 直接除零 panic（本函数不返回 Result）。
    assert!(res > 0, "分辨率不能为 0（logits 要按 res² 分组）");
    let hw = res as usize * res as usize;
    let k = parts.k;
    let base = k * 3 + 1;
    assert_eq!(logits.len() % hw, 0, "logits 长度应为 hw 的整数倍");
    let ch = logits.len() / hw;
    assert!(ch > k * 3, "通道数少于 k*3+1（{ch} < {base}）");
    let mut ids = vec![0u32; hw];
    for p in 0..hw {
        if logits[p * ch + k * 3] <= 0.0 {
            continue;
        }
        ids[p] = parts.decode_constrained(&logits[p * ch..p * ch + k * 3]);
    }
    ids
}

/// 便捷构造：从部署包 + GPU 起一个后端（`gpu` feature 下可用）。
/// 一次前向拿到 `(logits, d1 特征)`；域外闸门与分割共用同一次推理。
#[cfg(feature = "gpu")]
pub fn forward_logits_feats(
    net: &crate::net::Net,
    gpu: &crate::gpu::Gpu,
    rgb: &[f32],
    res: u32,
) -> (Vec<f32>, Vec<f32>) {
    assert!(
        res_is_backend_usable(res),
        "分辨率必须是 4 的正整数倍（收到 {res}）"
    );
    let xb = gpu.buf(rgb, "x");
    let tr = net.forward(gpu, xb, res);
    let hw = res as usize * res as usize;
    let ch = net.head_cout() as usize;
    let logits = gpu.read(&tr.out, hw * ch);
    let zc = net.layers[crate::net::D1B].cout as usize;
    let z = gpu.read(&tr.z, hw * zc);
    (logits, z)
}

#[cfg(feature = "gpu")]
pub fn wgpu_from_tpm(m: &TpmModel) -> Result<crate::backend_wgpu::WgpuBackend, String> {
    crate::backend_wgpu::WgpuBackend::from_tpm(m)
}

#[cfg(test)]
mod tests;
