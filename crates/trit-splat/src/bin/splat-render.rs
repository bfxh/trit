//! splat-render：读 3DGS .ply + nerfstudio transforms.json，渲染指定帧出 PNG。
//!
//! 用法：
//!   splat-render --dataset <root> --frame 16 --ply scene_30000.ply --out out.png [--bg auto]

use std::path::PathBuf;

use trit_splat::camera::parse_transforms;
use trit_splat::ply::parse_ply;
use trit_splat::raster::{border_mean, render};
use trit_splat::Frame;

fn value(argv: &[String], key: &str) -> Option<String> {
    argv.iter()
        .position(|a| a == key)
        .and_then(|i| argv.get(i + 1))
        .cloned()
}

fn main() -> std::process::ExitCode {
    let argv: Vec<String> = std::env::args().collect();
    let Some(dataset) = value(&argv, "--dataset") else {
        eprintln!("用法: splat-render --dataset <root> --frame N --ply <ply> --out <png> [--bg auto|r,g,b]");
        return std::process::ExitCode::from(2);
    };
    let frame: usize = value(&argv, "--frame").and_then(|v| v.parse().ok()).unwrap_or(0);
    let ply_path = value(&argv, "--ply").unwrap_or_else(|| "scene_30000.ply".into());
    let out_path: PathBuf = value(&argv, "--out")
        .unwrap_or_else(|| "splat.png".into())
        .into();
    let bg_mode = value(&argv, "--bg").unwrap_or_else(|| "auto".into());
    // SH 阶数裁剪：0=纯 DC，3=完整 SH3（默认）。用于隔离视角色项的对错。
    let sh_degree: usize = value(&argv, "--sh-degree").and_then(|v| v.parse().ok()).unwrap_or(3);

    let run = || -> Result<Frame, String> {
        let ply_bytes = std::fs::read(&ply_path)
            .map_err(|e| format!("读 ply 失败 {ply_path}: {e}"))?;
        let mut gaussians = parse_ply(&ply_bytes).map_err(|e| format!("PLY 解析: {e}"))?;
        println!("ply: {} 高斯 (sh-degree={sh_degree})", gaussians.len());
        if sh_degree < 3 {
            let keep = match sh_degree {
                0 => 0,
                1 => 3,
                2 => 8,
                _ => 15,
            } * 3;
            for g in &mut gaussians {
                g.f_rest.truncate(keep);
            }
        }

        let transforms_path = PathBuf::from(&dataset).join("transforms.json");
        let text = std::fs::read_to_string(&transforms_path)
            .map_err(|e| format!("读 transforms 失败: {e}"))?;
        let transforms =
            parse_transforms(&text).map_err(|e| format!("transforms.json: {e}"))?;
        if frame >= transforms.frames.len() {
            return Err(format!("frame {frame} 超界（共 {} 帧）", transforms.frames.len()));
        }
        let cam = &transforms.frames[frame];
        println!(
            "frame {frame}: {} 相机位置 ({:.2},{:.2},{:.2})",
            cam.file_path,
            cam.c2w.translation().x,
            cam.c2w.translation().y,
            cam.c2w.translation().z
        );

        let background: [f32; 3] = match bg_mode.as_str() {
            "auto" => {
                // GT 帧边界均值（对拍口径，写入日志留痕）
                let gt = PathBuf::from(&dataset).join(&cam.file_path);
                decode_gt_border(&gt)
            }
            _ => {
                let parts: Result<Vec<f32>, _> =
                    bg_mode.split(',').map(|s| s.trim().parse()).collect();
                match parts {
                    Ok(v) if v.len() == 3 => [v[0], v[1], v[2]],
                    _ => [0.4, 0.4, 0.4],
                }
            }
        };
        println!("背景色 (auto=GT 边界均值): {background:?}");

        let frame_img = render(&gaussians, &cam.c2w, &transforms.intrinsics, background);
        write_png(&out_path, &frame_img).map_err(|e| format!("写 PNG: {e}"))?;
        println!("写出 {out_path:?}");
        Ok(frame_img)
    };

    match run() {
        Ok(_) => std::process::ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("FAIL: {e}");
            std::process::ExitCode::FAILURE
        }
    }
}

/// GT 边界均值（png crate 解码 8bit RGB/RGBA）。
fn decode_gt_border(path: &PathBuf) -> [f32; 3] {
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

fn write_png(path: &PathBuf, frame: &Frame) -> Result<(), String> {
    let file = std::fs::File::create(path).map_err(|e| e.to_string())?;
    let mut encoder = png::Encoder::new(file, frame.width, frame.height);
    encoder.set_color(png::ColorType::Rgb);
    encoder.set_depth(png::BitDepth::Eight);
    let mut writer = encoder.write_header().map_err(|e| e.to_string())?;
    writer.write_image_data(&frame.rgb).map_err(|e| e.to_string())
}
