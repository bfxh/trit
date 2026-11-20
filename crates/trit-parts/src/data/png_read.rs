//! PNG → f32 NHWC（拆出 `data.rs` 守模块行数；第 29 轮 29-1 加解码字节上限）。

use std::fs::File;
use std::io::BufReader;
use std::path::Path;

pub(crate) fn read_png_rgb(path: &Path) -> Result<(usize, usize, Vec<f32>), String> {
    let file = File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut decoder = png::Decoder::new(BufReader::new(file));
    decoder.set_transformations(png::Transformations::EXPAND);
    let mut reader = decoder.read_info().map_err(|e| e.to_string())?;
    // IHDR 声明的尺寸直接决定分配：先按上限截断再读，1e6×1e6 不再 3 TB（第 29 轮 29-1）
    let mut buf = vec![0u8; reader.output_buffer_size().min(1 << 28)];
    let info = reader.next_frame(&mut buf).map_err(|e| e.to_string())?;
    let (w, h) = (info.width as usize, info.height as usize);
    let channels = match info.color_type {
        png::ColorType::Rgb => 3,
        png::ColorType::Rgba => 4,
        png::ColorType::Grayscale => 1,
        png::ColorType::GrayscaleAlpha => 2,
        png::ColorType::Indexed => return Err("索引色 PNG 未展开".into()),
    };
    let mut out = Vec::with_capacity(w * h * 3);
    for p in 0..w * h {
        let base = p * channels;
        let (r, g, b) = match channels {
            1 => (buf[base], buf[base], buf[base]),
            2 => (buf[base], buf[base], buf[base]),
            4 => (buf[base], buf[base + 1], buf[base + 2]),
            _ => (buf[base], buf[base + 1], buf[base + 2]),
        };
        out.push(f32::from(r) / 255.0);
        out.push(f32::from(g) / 255.0);
        out.push(f32::from(b) / 255.0);
    }
    Ok((w, h, out))
}
