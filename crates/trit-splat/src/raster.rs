//! CPU 泼溅光栅入口：投影、排序、alpha 合成与可选深度缓冲。
//!
//! 实现在 [`render`] 子模块；`render` 保持 RGB 原接口，`render_with_depth`
//! 额外返回相机空间深度（单位同视深，未覆盖像素为 +∞）。

mod border;
#[path = "raster/render.rs"]
mod render;

pub use border::border_mean;
pub use render::{TAIL_ALPHA_CUTOFF, render, render_with_depth};
