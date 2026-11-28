//! GT 帧解码：批量评测用 RGB 帧；`--bg auto` 用边界均值。

use std::path::Path;

use trit_splat::raster::border_mean;

#[path = "gt/decode.rs"]
mod decode;

pub use decode::decode_gt_rgb;

/// GT 边界均值（`--bg auto` 用）。
pub(super) fn decode_gt_border(path: &Path) -> [f32; 3] {
    let Some(frame) = decode_gt_rgb(path) else {
        return [0.4, 0.4, 0.4];
    };
    let mean = border_mean(&frame.rgb, frame.width, frame.height, 4);
    println!("GT 边界均值: {mean:?} ({path:?})");
    mean
}
