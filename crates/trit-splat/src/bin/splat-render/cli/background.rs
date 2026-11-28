//! `--bg` 解析：默认纯黑；`auto` = GT 边界均值（兼容旧评测）；也接受 `r,g,b`。

use std::path::PathBuf;

/// 背景色：`black`/缺省 = 纯黑；`auto` = GT 帧边界均值；否则按 `r,g,b`
/// 解析，失败退回灰 0.4（保持旧 CLI 的容错口径）。
pub fn background(bg_mode: &str, dataset: &str, cam_file: &str) -> [f32; 3] {
    match bg_mode {
        "black" => [0.0, 0.0, 0.0],
        "auto" => {
            let gt = PathBuf::from(dataset).join(cam_file);
            super::gt::decode_gt_border(&gt)
        }
        _ => {
            let parts: Result<Vec<f32>, _> = bg_mode.split(',').map(|s| s.trim().parse()).collect();
            match parts {
                Ok(v) if v.len() == 3 => [v[0], v[1], v[2]],
                _ => [0.4, 0.4, 0.4],
            }
        }
    }
}
