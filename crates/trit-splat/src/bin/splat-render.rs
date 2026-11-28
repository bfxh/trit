//! splat-render：读 3DGS .ply/.spz + nerfstudio transforms.json，渲染指定帧出 PNG。
//!
//! 用法：
//!   splat-render --dataset <root> --frame 16 --ply scene_30000.ply --out out.png [--bg auto]
//!   splat-render --dataset <root> --frame 16 --ply scene_30000.spz --out out.png

use std::path::PathBuf;

#[path = "splat-render/cli.rs"]
mod cli;
#[path = "splat-render/mip.rs"]
mod mip;

use trit_splat::Frame;
use trit_splat::camera::parse_transforms;
use trit_splat::ply::parse_ply;
use trit_splat::raster::render;

fn main() -> std::process::ExitCode {
    let argv: Vec<String> = std::env::args().collect();
    let Some(dataset) = cli::value(&argv, "--dataset") else {
        eprintln!(
            "用法: splat-render --dataset <root> --frame N --ply <ply|spz> --out <png> [--bg auto|r,g,b]"
        );
        return std::process::ExitCode::from(2);
    };
    let frame: usize = cli::value(&argv, "--frame")
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);
    let ply_path = cli::value(&argv, "--ply").unwrap_or_else(|| "scene_30000.ply".into());
    let out_path: PathBuf = cli::value(&argv, "--out")
        .unwrap_or_else(|| "splat.png".into())
        .into();
    let bg_mode = cli::value(&argv, "--bg").unwrap_or_else(|| "auto".into());
    // SH 阶数裁剪：0=纯 DC，3=完整 SH3（默认）。用于隔离视角色项的对错。
    let sh_degree: usize = cli::value(&argv, "--sh-degree")
        .and_then(|v| v.parse().ok())
        .unwrap_or(3);
    // Mip-Splatting 3D 滤波强度（默认 0=关：v0.3.0 导出无 SplatRenderMode 标记
    // 且实验证明 floor 劣化该数据；新版 Brush 导出可用 --min-scale-factor 0.1）。
    let min_scale_factor: f32 = cli::value(&argv, "--min-scale-factor")
        .and_then(|v| v.parse().ok())
        .unwrap_or(0.0);

    let run = || -> Result<Frame, String> {
        // .spz 资产直读（spz feature）；.ply 走解析器
        let mut gaussians = if ply_path.to_ascii_lowercase().ends_with(".spz") {
            #[cfg(feature = "spz")]
            {
                trit_splat::spz::load_spz(std::path::Path::new(&ply_path))
                    .map_err(|e| format!("SPZ 解析: {e}"))?
            }
            #[cfg(not(feature = "spz"))]
            {
                return Err("读 .spz 需要构建时启用 spz feature".into());
            }
        } else {
            let ply_bytes =
                std::fs::read(&ply_path).map_err(|e| format!("读 ply 失败 {ply_path}: {e}"))?;
            parse_ply(&ply_bytes).map_err(|e| format!("PLY 解析: {e}"))?
        };
        println!("ply: {} 高斯 (sh-degree={sh_degree})", gaussians.len());
        cli::truncate_sh(&mut gaussians, sh_degree);

        let transforms_path = PathBuf::from(&dataset).join("transforms.json");
        let text = std::fs::read_to_string(&transforms_path)
            .map_err(|e| format!("读 transforms 失败: {e}"))?;
        let transforms = parse_transforms(&text).map_err(|e| format!("transforms.json: {e}"))?;
        if frame >= transforms.frames.len() {
            return Err(format!(
                "frame {frame} 超界（共 {} 帧）",
                transforms.frames.len()
            ));
        }
        let cam = &transforms.frames[frame];
        println!(
            "frame {frame}: {} 相机位置 ({:.2},{:.2},{:.2})",
            cam.file_path,
            cam.c2w.translation().x,
            cam.c2w.translation().y,
            cam.c2w.translation().z
        );

        let background: [f32; 3] = cli::background(&bg_mode, &dataset, &cam.file_path);
        println!("背景色 (auto=GT 边界均值): {background:?}");

        // Mip-Splatting 3D 滤波：f_i = √factor · min_v(dist/focal)（Brush
        // train.rs:98 口径，factor 默认 0.1 ≈ 0.32px 下限）。
        let min_scales = mip::min_scales(&gaussians, &transforms, min_scale_factor);
        let frame_img = render(
            &gaussians,
            &cam.c2w,
            &transforms.intrinsics,
            background,
            min_scales.as_deref(),
        );
        cli::write_png(&out_path, &frame_img).map_err(|e| format!("写 PNG: {e}"))?;
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
