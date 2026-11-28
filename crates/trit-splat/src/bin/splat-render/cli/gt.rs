//! GT 帧边界均值（`--bg auto` 用）。

use std::path::Path;

use trit_splat::raster::border_mean;

/// GT 边界均值（png crate 解码 8bit RGB/RGBA）。
pub(super) fn decode_gt_border(path: &Path) -> [f32; 3] {
    let file = match std::fs::File::open(path) {
        Ok(f) => f,
        Err(e) => {
            eprintln!("warn: 读不到 GT（{}），背景用灰 0.4: {e}", path.display());
            return [0.4, 0.4, 0.4];
        }
    };
    let decoder = png::Decoder::new(file);
    let mut reader = match decoder.read_info() {
        Ok(r) => r,
        Err(e) => {
            eprintln!("warn: GT 头解析失败: {e}");
            return [0.4, 0.4, 0.4];
        }
    };
    let mut buf = vec![0u8; reader.output_buffer_size()];
    let info = match reader.next_frame(&mut buf) {
        Ok(info) => info,
        Err(e) => {
            eprintln!("warn: GT 解码失败: {e}");
            return [0.4, 0.4, 0.4];
        }
    };
    let (w, h) = (info.width, info.height);
    let channels = info.color_type.samples() as usize;
    let mut rgb = vec![0u8; (w * h * 3) as usize];
    for i in 0..(w * h) as usize {
        rgb[i * 3] = buf[i * channels];
        rgb[i * 3 + 1] = buf[i * channels + 1];
        rgb[i * 3 + 2] = buf[i * channels + 2];
    }
    let mean = border_mean(&rgb, w, h, 4);
    println!("GT 边界均值: {mean:?} ({path:?})");
    mean
}
