//! splat-render 的 CLI 辅助：参数取值、SH 裁剪、GT 边界均值与 PNG 落盘。

use std::path::{Path, PathBuf};

use trit_splat::ply::RawGaussian;
use trit_splat::raster::border_mean;
use trit_splat::Frame;

pub fn value(argv: &[String], key: &str) -> Option<String> {
    argv.iter()
        .position(|a| a == key)
        .and_then(|i| argv.get(i + 1))
        .cloned()
}

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
        return decode_gt_border(&gt);
    }
    let parts: Result<Vec<f32>, _> = bg_mode.split(',').map(|s| s.trim().parse()).collect();
    match parts {
        Ok(v) if v.len() == 3 => [v[0], v[1], v[2]],
        _ => [0.4, 0.4, 0.4],
    }
}

/// GT 边界均值（png crate 解码 8bit RGB/RGBA）。
fn decode_gt_border(path: &Path) -> [f32; 3] {
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

pub fn write_png(path: &Path, frame: &Frame) -> Result<(), String> {
    let file = std::fs::File::create(path).map_err(|e| e.to_string())?;
    let mut encoder = png::Encoder::new(file, frame.width, frame.height);
    encoder.set_color(png::ColorType::Rgb);
    encoder.set_depth(png::BitDepth::Eight);
    let mut writer = encoder.write_header().map_err(|e| e.to_string())?;
    writer.write_image_data(&frame.rgb).map_err(|e| e.to_string())
}
