//! 体素 → 网格：实心化 + Surface Nets 提取 + 分辨率金字塔（LOD）+ 顶点聚类简化。
//!
//! 输入是**占用栅格**（0 空、非 0 实），三个组件各自独立可测：
//! - `fill_solid`：把点云壳壳填成实体（从边界泛洪，剩下的空腔就是内部）；
//! - `surface_nets`：对实体做等值面提取，得到近水密三角网格；
//! - `downsample_occupancy` / `simplify_cluster`：高/低精度两级的产生方式。
//!
//! 这里**不做学习**；学习型占据/隐式场在后续片接进来（同一接口：栅格 → Mesh）。

/// 三角网格（顶点 + 三角面索引）。
mod build;
mod io;
mod morph;

pub use build::*;
pub use io::*;
pub use morph::*;

#[cfg(test)]
mod box_test;
#[cfg(test)]
mod tests;
