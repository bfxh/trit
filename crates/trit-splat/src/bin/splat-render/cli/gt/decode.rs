//! 8bit RGB/RGBA PNG 解码为 RGB8 帧。

use std::path::Path;

use trit_splat::Frame;

/// 解码 GT PNG；失败打印 warn 并返回 None。
pub fn decode_gt_rgb(path: &Path) -> Option<Frame> {
    let file = match std::fs::File::open(path) {
        Ok(f) => f,
        Err(e) => {
            eprintln!("warn: 读不到 GT（{}）: {e}", path.display());
            return None;
        }
    };
    let decoder = png::Decoder::new(file);
    let mut reader = match decoder.read_info() {
        Ok(r) => r,
        Err(e) => {
            eprintln!("warn: GT 头解析失败: {e}");
            return None;
        }
    };
    let mut buf = vec![0u8; reader.output_buffer_size()];
    let info = match reader.next_frame(&mut buf) {
        Ok(info) => info,
        Err(e) => {
            eprintln!("warn: GT 解码失败: {e}");
            return None;
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
    Some(Frame {
        width: w,
        height: h,
        rgb,
    })
}
