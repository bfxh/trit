//! splat-render 的 CLI 辅助：参数取值、SH 裁剪、背景色与 PNG 落盘。

use trit_splat::ply::RawGaussian;

#[path = "cli/args.rs"]
mod args;
#[path = "cli/background.rs"]
mod background;
#[path = "cli/gt.rs"]
mod gt;
#[path = "cli/png.rs"]
mod png;

pub use args::{flag, value};
pub use background::background;
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
