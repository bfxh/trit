//! `ttx-convert`：TTX ↔ PNG 的文件级转换 CLI（`png` feature，默认关）。
//!
//! ```text
//! cargo run -p trit-texture --features png --bin ttx-convert -- <子命令>
//!   png-to-ttx    in.png out.ttx [--v1] [--mip]
//!   ttx-to-png    in.ttx out.png
//!   ttx-upgrade   in.ttx out.ttx    # TTX1 → TTX2
//!   ttx-downgrade in.ttx out.ttx    # TTX2 → TTX1（仅单级 TEX）
//! ```
#![forbid(unsafe_code)]

use std::path::Path;
use std::process::ExitCode;

use trit_texture::convert::{
    build_mip_chain, rgba_to_ttx1, rgba_to_ttx2, rgba_to_ttx2_mips, ttx1_to_rgba, ttx1_to_ttx2,
    ttx2_to_rgba, ttx2_to_ttx1,
};
use trit_texture::pngio::{load_png_rgba, save_png_rgba};

const USAGE: &str = "用法：ttx-convert <png-to-ttx|ttx-to-png|ttx-upgrade|ttx-downgrade> <输入> <输出> [png-to-ttx 选项：--v1 --mip]";

fn main() -> ExitCode {
    match run() {
        Ok(summary) => {
            println!("{summary}");
            ExitCode::SUCCESS
        }
        Err(message) => {
            eprintln!("ttx-convert：{message}\n{USAGE}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<String, String> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let (command, rest) = args.split_first().ok_or("缺少子命令")?;
    match command.as_str() {
        "png-to-ttx" => {
            let (input, output, v1, mip) = parse_png_to_ttx(rest)?;
            let image = load_png_rgba(Path::new(&input)).map_err(|e| e.to_string())?;
            if v1 && mip {
                return Err("TTX1 没有 mip 链字段：--mip 只能与默认的 TTX2 搭配".to_string());
            }
            let bytes = if v1 {
                rgba_to_ttx1(&image.rgba, image.width, image.height)
            } else if mip {
                rgba_to_ttx2_mips(&image.rgba, image.width, image.height)
            } else {
                rgba_to_ttx2(&image.rgba, image.width, image.height)
            }
            .map_err(|e| format!("编码失败（宽须为 4 的倍数、高须为 10 的倍数）：{e}"))?;
            let levels = if mip {
                build_mip_chain(&image.rgba, image.width, image.height)
                    .map_err(|e| e.to_string())?
                    .len()
            } else {
                1
            };
            std::fs::write(Path::new(&output), &bytes).map_err(|e| e.to_string())?;
            Ok(format!(
                "{input} → {output}（{}×{}，{levels} 级，{} 字节）",
                image.width,
                image.height,
                bytes.len()
            ))
        }
        "ttx-to-png" => {
            let (input, output) = parse_two(rest)?;
            let bytes = std::fs::read(Path::new(&input)).map_err(|e| e.to_string())?;
            let image = ttx2_to_rgba(&bytes)
                .or_else(|_| ttx1_to_rgba(&bytes))
                .map_err(|_| format!("{input} 不是可解码的 TTX 纹理"))?;
            save_png_rgba(Path::new(&output), &image).map_err(|e| e.to_string())?;
            Ok(format!(
                "{input} → {output}（{}×{}）",
                image.width, image.height
            ))
        }
        "ttx-upgrade" => {
            let (input, output) = parse_two(rest)?;
            let bytes = std::fs::read(Path::new(&input)).map_err(|e| e.to_string())?;
            let upgraded = ttx1_to_ttx2(&bytes).map_err(|e| e.to_string())?;
            std::fs::write(Path::new(&output), &upgraded).map_err(|e| e.to_string())?;
            Ok(format!(
                "{input} → {output}（TTX2，{} 字节）",
                upgraded.len()
            ))
        }
        "ttx-downgrade" => {
            let (input, output) = parse_two(rest)?;
            let bytes = std::fs::read(Path::new(&input)).map_err(|e| e.to_string())?;
            let downgraded = ttx2_to_ttx1(&bytes)
                .map_err(|e| format!("降级失败（mip 链会丢级，仅单级 TEX 可降）：{e}"))?;
            std::fs::write(Path::new(&output), &downgraded).map_err(|e| e.to_string())?;
            Ok(format!(
                "{input} → {output}（TTX1，{} 字节）",
                downgraded.len()
            ))
        }
        other => Err(format!("未知子命令 {other}")),
    }
}

fn parse_two(rest: &[String]) -> Result<(String, String), String> {
    if rest.len() != 2 {
        return Err("需要恰好两个路径参数".to_string());
    }
    Ok((rest[0].clone(), rest[1].clone()))
}

fn parse_png_to_ttx(rest: &[String]) -> Result<(String, String, bool, bool), String> {
    let paths: Vec<&String> = rest.iter().filter(|arg| !arg.starts_with("--")).collect();
    if paths.len() != 2 {
        return Err("需要恰好两个路径参数".to_string());
    }
    let v1 = rest.iter().any(|arg| arg == "--v1");
    let mip = rest.iter().any(|arg| arg == "--mip");
    Ok((paths[0].clone(), paths[1].clone(), v1, mip))
}
