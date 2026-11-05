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

use crate::mesh::Mesh;

/// 纹素颜色由调用方提供的图集烘焙：`shade(三角形, 世界坐标)`，
/// 返回 `None` 的纹素保留底色 `fill`（用于「没有视角看到」）。平面投影 chart（不做 LSCM）。
pub fn bake_atlas_with<F>(
    m: &Mesh,
    atlas_px: usize,
    texels_per_unit: f32,
    normal_deg: f32,
    fill: [f32; 3],
    mut shade: F,
) -> Result<Atlas, String>
where
    F: FnMut(usize, [f32; 3]) -> Option<[f32; 3]>,
{
    let mut sh = |fi: usize, _uv: [f32; 2], p: [f32; 3]| shade(fi, p);
    bake::bake_shaded(
        m,
        atlas_px,
        texels_per_unit,
        normal_deg,
        false,
        fill,
        &mut sh,
    )
}

#[cfg(test)]
mod tests;
