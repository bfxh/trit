//! splat-render 的 CLI 辅助：参数取值、SH 裁剪、背景色与 PNG 落盘。

use std::path::PathBuf;

use trit_splat::ply::RawGaussian;

#[path = "cli/args.rs"]
mod args;
#[path = "cli/gt.rs"]
mod gt;
#[path = "cli/png.rs"]
mod png;

pub use args::{flag, value};
pub use gt::decode_gt_rgb;
pub use png::write_png;

/// 按 `--sh-degree` 截断每点的 SH 系数（0/1/2/3 阶）。
pub fn truncate_sh(gaussians: &mut [RawGaussian], sh_degree: usize) {
    if sh_degree >= 3 {
        return;
    }
    let keep = match sh_degree {
        0 => 0,
        1 => 3,
        2 => 8,
        _ => 15,
    } * 3;
    for g in gaussians {
        g.f_rest.truncate(keep);
    }
}

/// 背景色：`auto` = GT 帧边界均值；否则按 `r,g,b` 解析，失败退回灰 0.4。
pub fn background(bg_mode: &str, dataset: &str, cam_file: &str) -> [f32; 3] {
    if bg_mode == "auto" {
        let gt = PathBuf::from(dataset).join(cam_file);
        return gt::decode_gt_border(&gt);
    }
    let parts: Result<Vec<f32>, _> = bg_mode.split(',').map(|s| s.trim().parse()).collect();
    match parts {
        Ok(v) if v.len() == 3 => [v[0], v[1], v[2]],
        _ => [0.4, 0.4, 0.4],
    }
}
