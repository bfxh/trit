//! `ttx-convert`：TTX ↔ PNG 的文件级转换 CLI（`png` feature，默认关）。
//!
//! ```text
//! cargo run -p trit-texture --features png --bin ttx-convert -- <子命令>
//!   png-to-ttx    in.png out.ttx [--v1] [--mip]
//!   ttx-to-png    in.ttx out.png
//!   ttx-upgrade   in.ttx out.ttx    # TTX1 → TTX2
//!   ttx-downgrade in.ttx out.ttx    # TTX2 → TTX1（仅单级 TEX）
//!   render        in.ttx out.png    # 渲染预览（采样口径/寻址/平铺可调，--split 并排对拍）
//!   stats         in.ttx [--level N]  # 逐级 mip 账单 + 第 N 级调色板跨度直方图
//!
//! `render` 的选项：`--size WxH` `--mode nearest|bilinear|trilinear|auto` `--lod F`
//! `--wrap clamp|repeat|mirror` `--tiles F` `--split` `--level N` `--blocks` `--atlas x,y,w,h`

//! ```
#![forbid(unsafe_code)]

use std::path::Path;
use std::process::ExitCode;

use trit_texture::convert::RgbaImage;
use trit_texture::convert::{
    build_mip_chain, rgba_to_ttx1, rgba_to_ttx2, rgba_to_ttx2_mips, ttx1_to_rgba, ttx1_to_ttx2,
    ttx2_to_rgba, ttx2_to_ttx1,
};
use trit_texture::pngio::{load_png_rgba, save_png_rgba};
use trit_texture::preview::{RenderOptions, SampleMode, level_stats, palette_histogram, render};
use trit_texture::sample::{Rect, Wrap};
use trit_texture::ttx2::{TtxDocument, from_ttx2};

const USAGE: &str = "用法：ttx-convert <png-to-ttx|ttx-to-png|ttx-upgrade|ttx-downgrade|render> <输入> <输出> [png-to-ttx 选项：--v1 --mip] [render 选项：--size WxH --mode nearest|bilinear|trilinear|auto --lod F --wrap clamp|repeat|mirror --tiles F --split]";

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
        "render" => {
            let (input, output, opts) = parse_render(rest)?;
            let bytes = std::fs::read(Path::new(&input)).map_err(|e| e.to_string())?;
            let doc = load_doc(&bytes, &input)?;
            let (w, h, rgba) = render(&doc, &opts).map_err(|e| format!("渲染失败：{e}"))?;
            let image = RgbaImage {
                width: w,
                height: h,
                rgba,
            };
            save_png_rgba(Path::new(&output), &image).map_err(|e| e.to_string())?;
            let levels = doc.levels().map(|l| l.len()).unwrap_or(0);
            Ok(format!(
                "{input} → {output}（{w}×{h}，{} 级，mode {:?}，wrap {:?}{}）",
                levels,
                opts.mode,
                opts.wrap,
                if opts.split { "，左右对拍" } else { "" }
            ))
        }
        "stats" => {
            let mut level = 0usize;
            let mut path: Option<&String> = None;
            let mut index = 0;
            while index < rest.len() {
                if rest[index] == "--level" {
                    index += 1;
                    let value = rest.get(index).ok_or("--level 缺少取值")?;
                    level = value
                        .parse()
                        .map_err(|_| format!("--level 不是整数：{value}"))?;
                } else {
                    path = Some(&rest[index]);
                }
                index += 1;
            }
            let path = path.ok_or("stats 需要一个路径参数：<in.ttx>")?;
            let bytes = std::fs::read(Path::new(path)).map_err(|e| e.to_string())?;
            let doc = load_doc(&bytes, path)?;
            let stats = level_stats(&doc).map_err(|e| e.to_string())?;
            let mut out = String::from(
                "级别  尺寸        块数    块流字节  RGBA字节  压缩比  载荷槽
",
            );
            for s in &stats {
                out.push_str(&format!(
                    "{:<5} {:<10} {:>6} {:>9} {:>9}  {:>6.1}×  {}/{}
",
                    s.level,
                    format!("{}×{}", s.width, s.height),
                    s.blocks,
                    s.bytes,
                    s.rgba_bytes,
                    s.rgba_bytes as f64 / s.bytes.max(1) as f64,
                    s.payload_used,
                    s.payload_slots
                ));
            }
            let total: usize = stats.iter().map(|s| s.bytes).sum();
            out.push_str(&format!("合计块流 {total} 字节（{} 级）", stats.len()));

            let hist = palette_histogram(&doc, level).map_err(|e| e.to_string())?;
            out.push_str(&format!(
                "

调色板跨度分布（第 {level} 级，{} 块）
",
                hist.blocks
            ));
            for (bin, count) in hist.bins.iter().enumerate() {
                let lo = bin * 56;
                let hi = (bin + 1) * 56;
                let share = *count as f64 * 100.0 / hist.blocks.max(1) as f64;
                let bar = "#".repeat((share / 2.0) as usize);
                out.push_str(&format!(
                    "  [{lo:>3},{hi:>3}) {count:>6} {bar:<25} {share:>5.1}%
"
                ));
            }
            out.push_str(&format!(
                "索引使用：1 色 {} 块 / 2 色 {} 块 / 3 色 {} 块",
                hist.index_usage[0], hist.index_usage[1], hist.index_usage[2]
            ));
            Ok(out)
        }
        other => Err(format!("未知子命令 {other}")),
    }
}

/// 读入 TTX（v2 走容器解析，v1 包成单级文档）。
fn load_doc(bytes: &[u8], name: &str) -> Result<TtxDocument, String> {
    if bytes.starts_with(b"TTX2") {
        from_ttx2(bytes).map_err(|e| format!("{name} 解析失败：{e}"))
    } else {
        // TTX1 没有 mip 链字段 ⇒ 单级纹理
        let texture =
            trit_texture::ttx::from_ttx(bytes).map_err(|e| format!("{name} 不是 TTX 纹理：{e}"))?;
        Ok(TtxDocument::Texture {
            levels: vec![texture],
        })
    }
}

fn parse_render(rest: &[String]) -> Result<(String, String, RenderOptions), String> {
    let mut opts = RenderOptions::default();
    let mut paths: Vec<String> = Vec::new();
    let mut index = 0;
    while index < rest.len() {
        let arg = rest[index].clone();
        // 取选项值的辅助：消费下一个参数
        macro_rules! take {
            () => {{
                index += 1;
                rest.get(index)
                    .cloned()
                    .ok_or_else(|| format!("{arg} 缺少取值"))?
            }};
        }
        match arg.as_str() {
            "--split" => opts.split = true,
            "--blocks" => opts.overlay_blocks = true,
            "--atlas" => {
                let value = take!();
                let parts: Vec<&str> = value.split(',').collect();
                if parts.len() != 4 {
                    return Err(format!("--atlas 要写成 x,y,w,h：{value}"));
                }
                let mut nums = [0usize; 4];
                for (slot, text) in nums.iter_mut().zip(&parts) {
                    *slot = text
                        .parse()
                        .map_err(|_| format!("--atlas 数值不合法：{text}"))?;
                }
                opts.atlas = Some(
                    Rect::new(nums[0], nums[1], nums[2], nums[3])
                        .ok_or("--atlas 的宽高必须 > 0")?,
                );
            }
            "--level" => {
                let value = take!();
                opts.level = value
                    .parse()
                    .map_err(|_| format!("--level 不是整数：{value}"))?;
            }
            "--size" => {
                let value = take!();
                let (w, h) = value
                    .split_once('x')
                    .ok_or_else(|| format!("--size 要写成 WxH：{value}"))?;
                opts.width = w.parse().map_err(|_| format!("宽度不是整数：{w}"))?;
                opts.height = h.parse().map_err(|_| format!("高度不是整数：{h}"))?;
            }
            "--tiles" => {
                opts.tiles = take!()
                    .parse()
                    .map_err(|_| "--tiles 不是数值".to_string())?;
                if opts.tiles <= 0.0 || !opts.tiles.is_finite() {
                    return Err("--tiles 必须 > 0".to_string());
                }
            }
            "--lod" => {
                let lod: f32 = take!().parse().map_err(|_| "--lod 不是数值".to_string())?;
                opts.mode = SampleMode::Trilinear(lod);
            }
            "--aniso" => {
                let taps: u32 = take!()
                    .parse()
                    .map_err(|_| "--aniso 不是整数".to_string())?;
                if taps == 0 {
                    return Err("--aniso 必须 ≥ 1".to_string());
                }
                opts.mode = SampleMode::Aniso(taps);
            }
            "--mode" => {
                let value = take!();
                opts.mode = match value.as_str() {
                    "nearest" => SampleMode::Nearest,
                    "bilinear" => SampleMode::Bilinear,
                    "trilinear" => SampleMode::Trilinear(0.0),
                    "auto" => SampleMode::Auto,
                    other => return Err(format!("未知 --mode {other}")),
                };
            }
            "--wrap" => {
                let value = take!();
                opts.wrap = match value.as_str() {
                    "clamp" => Wrap::Clamp,
                    "repeat" => Wrap::Repeat,
                    "mirror" => Wrap::Mirror,
                    other => return Err(format!("未知 --wrap {other}")),
                };
            }
            other if other.starts_with("--") => return Err(format!("未知选项 {other}")),
            other => paths.push(other.to_string()),
        }
        index += 1;
    }
    if paths.len() != 2 {
        return Err("render 需要恰好两个路径参数：<in.ttx> <out.png>".to_string());
    }
    Ok((paths[0].clone(), paths[1].clone(), opts))
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
