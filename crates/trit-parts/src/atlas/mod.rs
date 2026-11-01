//! 图集烘焙：按「主轴 + 连通分量」切 chart，线性投影后用货架法打包，
//! 把源贴图烘进一张单图集，并给出每个三角形三个角的 UV。
//!
//! 与 [`crate::uv::box_uvs`] 的区别：box_uvs 让 UV 直接超出 `[0,1]` 靠 wrap 平铺；
//! 这里每个 chart 独占一块图集区域，贴图只被采样一次，适合把 quilt 结果固化成单张资产。
//! 局限：chart 边界仍有接缝（盒投影的固有性质）；重叠区域（如背面被投影到同一 chart）
//! 会共享同一块 UV，不保证 bijective。

mod bake;
mod charts;
mod stats;

pub use bake::bake_atlas;
pub use charts::{Atlas, Chart, PerClass};
pub use stats::{UvStats, uv_stats};

#[cfg(test)]
mod tests;
