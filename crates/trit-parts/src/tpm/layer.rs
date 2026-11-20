//! `TpmLayer::effective`（拆出 `tpm.rs` 守模块行数；第 22 轮 22-1a 修 `% 0` 除零）。

use super::TpmLayer;

impl TpmLayer {
    pub fn effective(&self) -> Vec<f32> {
        let slots = self.cout as usize;
        if slots == 0 {
            return vec![0.0; self.q.len()]; // `% 0` 会除零 panic（第 22 轮 22-1a）
        }
        self.q
            .iter()
            .enumerate()
            .map(|(wi, &qi)| qi as f32 * self.scale[wi % slots])
            .collect()
    }
}
