//! 3DGS 泼溅渲染实验层（`publish = false`，不属于稳定 API）。
//!
//! 目标：把训练产物 `.ply`（3DGS 布局）按数据集相机**自己渲染出图**，
//! 不依赖 bevy_gaussian_splatting 等现成件。链路：
//!
//! 1. [`ply`]：binary_little_endian PLY 头按属性名建列映射，读出
//!    位置 / f_dc / f_rest(SH) / opacity(logit) / scale(log) / rot(xyzw)；
//! 2. [`camera`]：nerfstudio `transforms.json`（OPENCV 内参 + OpenGL
//!    camera-to-world）解析成 pinhole 相机，world→cam→screen；
//! 3. [`splat`]：scale/rot → 3D 协方差，SH3 视角色，投影成屏幕空间 2D 高斯；
//! 4. [`raster`]：按视深排序的 painter alpha 合成，出 RGB8 帧。
//!
//! 深度口径承接 depth-contract 结论（射线深度 + CDF 软截断）：本 crate 渲染
//! 纯 splat 场景用深度排序；与 mesh 混合的 CDF 截断在 C0b 落地，接口留在本层。
//!
//! 渲染核零外部依赖；PNG 落盘与 CLI 在 `png` feature 后（`splat-render` bin）。
pub mod camera;
pub mod color;
mod error;
pub mod math;
pub mod ply;
mod psnr;
pub mod raster;
pub mod splat;
#[cfg(feature = "spz")]
pub mod spz;

pub use error::SplatError;
/// 渲染结果：w×h 的 RGB8 帧（行主序，无 padding）。
#[derive(Clone)]
pub struct Frame {
    pub width: u32,
    pub height: u32,
    pub rgb: Vec<u8>,
}

impl Frame {
    pub fn new(width: u32, height: u32, background: [f32; 3]) -> Self {
        let px = (width as usize) * (height as usize) * 3;
        let mut rgb = Vec::with_capacity(px);
        for _ in 0..(width as usize) * (height as usize) {
            rgb.extend_from_slice(&[
                (background[0] * 255.0).round() as u8,
                (background[1] * 255.0).round() as u8,
                (background[2] * 255.0).round() as u8,
            ]);
        }
        Self { width, height, rgb }
    }
}
