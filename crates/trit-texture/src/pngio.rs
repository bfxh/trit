//! PNG 文件 I/O（`png` feature，默认关）。
//!
//! 只做 RGBA8 ↔ PNG：RGB8 自动补 alpha=255，其余颜色类型拒绝。像素/容器
//! 之间的纯转换在 [`crate::convert`]（默认构建就有）；本模块只补文件这一层。

use std::fs::File;
use std::io::{BufReader, BufWriter};
use std::path::Path;

use crate::convert::RgbaImage;

/// PNG 模块的错误（文件 I/O 层，不走 `TextureError`——那套是格式语义错误）。
#[derive(Debug, thiserror::Error)]
pub enum PngError {
    /// PNG 解码/编码失败（透传 `png` crate 的错误文本）。
    #[error("PNG I/O 失败：{0}")]
    Io(String),
}

impl From<std::io::Error> for PngError {
    fn from(error: std::io::Error) -> Self {
        Self::Io(error.to_string())
    }
}

impl From<png::DecodingError> for PngError {
    fn from(error: png::DecodingError) -> Self {
        Self::Io(error.to_string())
    }
}

impl From<png::EncodingError> for PngError {
    fn from(error: png::EncodingError) -> Self {
        Self::Io(error.to_string())
    }
}

/// 读 PNG 文件成 RGBA8。
pub fn load_png_rgba(path: &Path) -> Result<RgbaImage, PngError> {
    let decoder = png::Decoder::new(BufReader::new(File::open(path)?));
    let mut reader = decoder.read_info()?;
    let mut buffer = vec![0u8; reader.output_buffer_size()];
    let frame = reader.next_frame(&mut buffer)?;
    let (width, height) = (frame.width as usize, frame.height as usize);
    buffer.truncate(frame.buffer_size());
    match frame.color_type.samples() {
        4 => Ok(RgbaImage {
            width,
            height,
            rgba: buffer,
        }),
        3 => {
            let mut rgba = Vec::with_capacity(width * height * 4);
            for pixel in buffer.chunks_exact(3) {
                rgba.extend_from_slice(pixel);
                rgba.push(255);
            }
            Ok(RgbaImage {
                width,
                height,
                rgba,
            })
        }
        other => Err(PngError::Io(format!(
            "颜色类型 {other} 字节/像素不支持：请先转成 RGB/RGBA"
        ))),
    }
}

/// 把 RGBA8 写成 PNG 文件。
pub fn save_png_rgba(path: &Path, image: &RgbaImage) -> Result<(), PngError> {
    let file = File::create(path)?;
    let mut encoder = png::Encoder::new(
        BufWriter::new(file),
        image.width as u32,
        image.height as u32,
    );
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    let mut writer = encoder.write_header()?;
    writer.write_image_data(&image.rgba)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::convert::{rgba_to_ttx2, ttx2_to_rgba};

    #[test]
    fn png_ttx_png_roundtrip_through_real_files() {
        let dir = std::env::temp_dir().join("trit-texture-png-test");
        std::fs::create_dir_all(&dir).unwrap();
        let image = RgbaImage {
            width: 8,
            height: 20,
            rgba: (0..8 * 20 * 4).map(|i| (i % 251) as u8).collect(),
        };
        let png_path = dir.join("src.png");
        save_png_rgba(&png_path, &image).unwrap();
        assert_eq!(load_png_rgba(&png_path).unwrap(), image);

        let ttx_path = dir.join("out.ttx");
        std::fs::write(&ttx_path, rgba_to_ttx2(&image.rgba, 8, 20).unwrap()).unwrap();
        let back = ttx2_to_rgba(&std::fs::read(&ttx_path).unwrap()).unwrap();
        let back_png = dir.join("back.png");
        save_png_rgba(&back_png, &back).unwrap();
        // TTX 解码结果二次写读 PNG 无损。
        assert_eq!(load_png_rgba(&back_png).unwrap(), back);
    }
}
