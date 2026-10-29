//! 给外部管线用的最小 API：一帧 RGB → 部件 id 图。
//!
//! 设计意图见 `README.md`「与生成管线的联动」：这个 crate 是**可插拔下游节点**，
//! 渲染/生成端只要能出 RGB，就能拿到 id 图；调用方只需拿到一个 `Backend`
//! （`WgpuBackend` 或 `RefCpu`）和一份 `PartList`。

use crate::model::Backend;
use crate::parts::PartList;
use crate::tpm::TpmModel;

/// 跑一帧并解成 id 图（清单约束解码；预测为"无部件"的像素置 0）。
pub fn infer_ids(
    backend: &mut dyn Backend,
    rgb: &[f32],
    res: u32,
    parts: &PartList,
) -> Result<Vec<u32>, String> {
    let logits = backend.forward(rgb, res)?;
    Ok(decode_ids(&logits, res, parts))
}

/// 从 `k*3+1` 通道的 NHWC logits 解出 id 图（与 `parts-cli run` 同口径）。
pub fn decode_ids(logits: &[f32], res: u32, parts: &PartList) -> Vec<u32> {
    let hw = (res * res) as usize;
    let k = parts.k;
    let ch = k * 3 + 1;
    assert_eq!(logits.len(), hw * ch, "logits 长度应为 hw*(k*3+1)");
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
#[cfg(feature = "gpu")]
pub fn wgpu_from_tpm(m: &TpmModel) -> Result<crate::backend_wgpu::WgpuBackend, String> {
    crate::backend_wgpu::WgpuBackend::from_tpm(m)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decode_respects_manifest_and_valid() {
        let m = PartList::parse(
            r#"{"k":2,"parts":[{"id":1,"digits":[0,0]},{"id":2,"digits":[0,1]},{"id":4,"digits":[1,0]}]}"#,
        )
        .unwrap();
        // res=2 ⇒ 4 个像素；只给像素 0 有效（且指向 id 4），像素 3 的 valid 置负
        let (res, ch) = (2u32, 7usize);
        let hw = (res * res) as usize;
        let mut lg = vec![0f32; hw * ch];
        lg[1] = 5.0; // plane0 → digit 1
        lg[3] = 5.0; // plane1 → digit 0
        lg[6] = 5.0; // valid
        let p3 = 3 * ch;
        lg[p3 + 6] = -5.0;
        let ids = decode_ids(&lg, res, &m);
        assert_eq!(ids[0], 4);
        assert_eq!(ids[3], 0);
    }
}
