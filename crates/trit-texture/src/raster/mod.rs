//! CPU 参考光栅化器：把压缩域采样接到真正的三角形光栅化上。
//!
//! 定位是**参考实现 / GPU 对拍基线**，不是性能路径——和 [`crate::sample`]
//! 一样，压缩域采样在 CPU 上比「整图解码成 RGBA 再过滤」慢（见该 crate 的基准）。
//!
//! 口径：
//! - 顶点位置直接给**裁剪空间**（自己乘 [`math::Mat4::perspective`]），光栅化器只做
//!   透视除法，不做近/远平面裁剪；
//! - 覆盖判定用 f64 边函数 + top-left 规则 ⇒ 共享边上的像素**恰好着色一次**；
//! - UV 透视校正（`u/w`、`v/w`、`1/w` 三平面），导数用解析式而不是 2×2 差分；
//! - 深度是屏幕空间线性的 NDC z，`[0, 1]`，越小越近；
//! - 两条三角形绕向都接受，不做背面剔除。
//!
//! ```no_run
//! use trit_texture::raster::{Mesh, RenderOptions, SampleMode, Vertex, render_textured};
//! # fn demo(doc: &trit_texture::ttx2::TtxDocument) {
//! let mesh = Mesh {
//!     vertices: vec![
//!         Vertex { clip: [-1.0, -1.0, 0.5, 1.0], uv: [0.0, 1.0] },
//!         Vertex { clip: [1.0, -1.0, 0.5, 1.0], uv: [1.0, 1.0] },
//!         Vertex { clip: [1.0, 1.0, 0.5, 1.0], uv: [1.0, 0.0] },
//!     ],
//!     indices: vec![[0, 1, 2]],
//! };
//! let opts = RenderOptions { width: 64, height: 64, mode: SampleMode::Auto, ..Default::default() };
//! let (image, stats) = render_textured(&mesh, doc, &opts).expect("render");
//! assert_eq!(image.pixels().len(), 64 * 64);
//! assert!(stats.shaded > 0);
//! # }
//! ```

pub mod core;
pub mod math;
pub mod mesh;
mod shade;

use std::fmt;

pub use crate::preview::SampleMode;
pub use crate::sample::{Rect, Wrap};
pub use core::{Derivative, Fragment, Shaded, Stats, Target, rasterize};
pub use mesh::{Mesh, Vertex};

use crate::ttx2::TtxDocument;

/// 渲染参数。
#[derive(Clone, Copy, Debug)]
pub struct RenderOptions {
    /// 输出宽（像素）。
    pub width: usize,
    /// 输出高（像素）。
    pub height: usize,
    /// 采样口径。
    pub mode: SampleMode,
    /// 越界寻址。
    pub wrap: Wrap,
    /// 图集子矩形（第 0 级纹素坐标）；`None` = 整张。
    pub rect: Option<Rect>,
    /// 背景色。
    pub background: [u8; 3],
}

impl Default for RenderOptions {
    fn default() -> Self {
        Self {
            width: 256,
            height: 256,
            mode: SampleMode::Auto,
            wrap: Wrap::Clamp,
            rect: None,
            background: [0, 0, 0],
        }
    }
}

/// 渲染结果（RGB）。
#[derive(Clone, Debug)]
pub struct Image {
    width: usize,
    height: usize,
    rgb: Vec<[u8; 3]>,
}

impl Image {
    /// 宽。
    pub fn width(&self) -> usize {
        self.width
    }

    /// 高。
    pub fn height(&self) -> usize {
        self.height
    }

    /// 行优先的 RGB 像素。
    pub fn pixels(&self) -> &[[u8; 3]] {
        &self.rgb
    }

    /// 单像素。
    pub fn pixel(&self, x: usize, y: usize) -> [u8; 3] {
        self.rgb[y * self.width + x]
    }

    /// 转成 RGBA8（给 PNG 编码用）。
    pub fn to_rgba(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(self.rgb.len() * 4);
        for px in &self.rgb {
            out.extend_from_slice(&[px[0], px[1], px[2], 255]);
        }
        out
    }
}

/// 渲染失败。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RenderError {
    /// 文档不是纹理（`kind=MOD`），没有 mip 链。
    NotATexture,
    /// 纹理没有级。
    EmptyLevels,
}

impl fmt::Display for RenderError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotATexture => write!(f, "文档不是纹理，没有可采样的 mip 链"),
            Self::EmptyLevels => write!(f, "纹理没有级"),
        }
    }
}

impl std::error::Error for RenderError {}

/// 用纹理着色渲染一个网格。
pub fn render_textured(
    mesh: &Mesh,
    doc: &TtxDocument,
    opts: &RenderOptions,
) -> Result<(Image, Stats), RenderError> {
    let levels = doc.levels().ok_or(RenderError::NotATexture)?;
    if levels.is_empty() {
        return Err(RenderError::EmptyLevels);
    }
    let mut target = Target::new(opts.width, opts.height, opts.background);
    let mut shader = shade::Textured::new(levels, opts.mode, opts.wrap, opts.rect);
    let stats = rasterize(mesh, &mut target, |f| shader.shade(f));
    Ok((
        Image {
            width: target.width,
            height: target.height,
            rgb: target.color,
        },
        stats,
    ))
}
