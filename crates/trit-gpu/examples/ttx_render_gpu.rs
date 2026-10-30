//! GPU 出图示例：读 TTX → 用 `GpuSampler` 三线性/各向异性渲染 → 写 PNG。
//!
//! 放在 example 里是因为 `trit-texture` 只能作为 **dev-dependency** 被依赖
//! （发布图门：正常 path 依赖未发布 crate 会让 `cargo package` 失败）。
//!
//! ```text
//! cargo run -p trit-gpu --features gpu --example ttx_render_gpu -- \
//!     in.ttx out.png [--size 320x200] [--tiles 1.0] [--wrap repeat] [--aniso 8]
//! ```
//!
//! `--aniso N` 走各向异性（每样点 N 个 tap）；不给则是纯三线性。UV/LOD 公式与
//! `trit_texture::preview::plan_points` 同一条，两边输出对拍过（≤1 LSB）。

use std::path::Path;

use trit_gpu::sampler::{AnisoSample, GpuSampler, MipLevel, Wrap};
use trit_texture::convert::RgbaImage;
use trit_texture::pngio::save_png_rgba;
use trit_texture::preview::{RenderOptions, SampleMode, plan_points};
use trit_texture::sample::Wrap as CpuWrap;
use trit_texture::ttx2::from_ttx2;

fn main() -> Result<(), String> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut path_in = None;
    let mut path_out = None;
    let mut opts = RenderOptions {
        mode: SampleMode::Auto,
        ..RenderOptions::default()
    };
    let mut aniso = 0u32;
    let mut index = 0;
    while index < args.len() {
        let arg = args[index].clone();
        let mut take = || -> Result<String, String> {
            index += 1;
            args.get(index)
                .cloned()
                .ok_or_else(|| format!("{arg} 缺少取值"))
        };
        match arg.as_str() {
            "--size" => {
                let v = take()?;
                let (w, h) = v.split_once('x').ok_or("--size 要写成 WxH")?;
                opts.width = w.parse().map_err(|_| "--size 宽度不是整数")?;
                opts.height = h.parse().map_err(|_| "--size 高度不是整数")?;
            }
            "--tiles" => {
                opts.tiles = take()?.parse().map_err(|_| "--tiles 不是数值")?;
            }
            "--wrap" => {
                opts.wrap = match take()?.as_str() {
                    "clamp" => CpuWrap::Clamp,
                    "repeat" => CpuWrap::Repeat,
                    "mirror" => CpuWrap::Mirror,
                    other => return Err(format!("未知 --wrap {other}")),
                };
            }
            "--aniso" => {
                aniso = take()?.parse().map_err(|_| "--aniso 不是整数")?;
            }
            other if other.starts_with("--") => return Err(format!("未知选项 {other}")),
            other => {
                if path_in.is_none() {
                    path_in = Some(other.to_string());
                } else {
                    path_out = Some(other.to_string());
                }
            }
        }
        index += 1;
    }
    let path_in = path_in.ok_or("缺少输入 .ttx")?;
    let path_out = path_out.ok_or("缺少输出 .png")?;

    let bytes = std::fs::read(&path_in).map_err(|e| e.to_string())?;
    let doc = from_ttx2(&bytes).map_err(|e| format!("{path_in} 不是 TTX2：{e}"))?;
    let levels = doc.levels().ok_or("不是纹理文档")?;
    let flat: Vec<Vec<u8>> = levels.iter().map(|t| t.blocks.concat()).collect();
    let gpu_levels: Vec<MipLevel> = levels
        .iter()
        .zip(&flat)
        .map(|(t, blocks)| MipLevel {
            blocks,
            width: t.width,
            height: t.height,
            rect: None,
        })
        .collect();
    let wrap = match opts.wrap {
        CpuWrap::Clamp => Wrap::Clamp,
        CpuWrap::Repeat => Wrap::Repeat,
        CpuWrap::Mirror => Wrap::Mirror,
    };
    let mut sampler = GpuSampler::new(&gpu_levels, wrap).ok_or("没有可用 GPU adapter")?;

    let (w, h, points) =
        plan_points(&doc, &opts).ok_or("只支持 --mode auto（三线性可表达的口径）")?;
    let rgba = if aniso > 1 {
        // 与 CPU `sample_aniso` 同一条公式：ratio 推 taps，lod 取 log2(ρ_minor)
        let base = &levels[0];
        let (bw, bh) = (base.width as f32, base.height as f32);
        let (du, dv) = (opts.tiles / w as f32, opts.tiles / h as f32);
        let major = du * bw;
        let minor = (dv * bh).max(1.0);
        let ratio = (major / minor).max(1.0);
        let taps = (ratio.ceil() as u32).clamp(1, aniso);
        let lod = minor.log2();
        let samples: Vec<AnisoSample> = points
            .iter()
            .map(|(u, v, _)| AnisoSample {
                u: *u,
                v: *v,
                lod,
                taps,
                step_u: du,
                step_v: 0.0,
            })
            .collect();
        sampler.sample_aniso(&samples)
    } else {
        sampler.sample(&points)
    }
    .ok_or("GPU 采样失败")?;

    let image = RgbaImage {
        width: w,
        height: h,
        rgba,
    };
    save_png_rgba(Path::new(&path_out), &image).map_err(|e| e.to_string())?;
    println!(
        "{path_in} → {path_out}（{w}×{h}，tiles {}，wrap {:?}，{}{}）",
        opts.tiles,
        opts.wrap,
        if aniso > 1 { "aniso" } else { "三线性" },
        if aniso > 1 {
            format!("（上限 {aniso} tap）")
        } else {
            String::new()
        }
    );
    Ok(())
}
