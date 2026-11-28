//! 帧级指标：8bit RGB 的 PSNR（峰值信噪比）。
//!
//! 只吃内存帧，不引入图像编码依赖；CLI 的批量评测复用这里。

use crate::Frame;

impl Frame {
    /// 与 `other` 的 PSNR（dB）。尺寸不一致返回 `None`；逐像素完全一致返回
    /// `Some(f64::INFINITY)`。
    ///
    /// 口径：MSE 按 8bit 码值归一化到 0..1 后计算，峰值 = 1.0。
    pub fn psnr_db(&self, other: &Frame) -> Option<f64> {
        if self.width != other.width
            || self.height != other.height
            || self.rgb.len() != other.rgb.len()
        {
            return None;
        }
        if self.rgb.is_empty() {
            return None;
        }
        let mse = self
            .rgb
            .iter()
            .zip(other.rgb.iter())
            .map(|(a, b)| {
                let d = (*a as f64 - *b as f64) / 255.0;
                d * d
            })
            .sum::<f64>()
            / self.rgb.len() as f64;
        if mse == 0.0 {
            return Some(f64::INFINITY);
        }
        Some(10.0 * (1.0 / mse).log10())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(width: u32, height: u32, rgb: Vec<u8>) -> Frame {
        Frame { width, height, rgb }
    }

    #[test]
    fn identical_frames_are_infinite() {
        let a = frame(2, 2, vec![0, 10, 20, 30, 40, 50, 60, 70, 80, 90, 100, 110]);
        let db = a.psnr_db(&a).unwrap();
        assert!(db.is_infinite() && db.is_sign_positive(), "同帧应 +∞: {db}");
    }

    #[test]
    fn one_code_difference_across_all_channels_is_48_13_db() {
        let a = frame(1, 1, vec![0, 0, 0]);
        let b = frame(1, 1, vec![1, 1, 1]);
        let db = a.psnr_db(&b).unwrap();
        assert!((db - 48.1308).abs() < 0.01, "1/255 MSE 应 48.13 dB: {db}");
    }

    #[test]
    fn size_mismatch_is_none() {
        let a = frame(1, 1, vec![0, 0, 0]);
        let b = frame(2, 1, vec![0, 0, 0, 0, 0, 0]);
        assert!(a.psnr_db(&b).is_none());
    }
}
