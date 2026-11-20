//! 非参数 patch 合成(`tex-quilt`)的门面:实现在子模块里。
//!
//! 历史预算注记:本文件曾 282 行顶格;2026-11-20 收台账缺口(graph-cut 缝合 +
//! 周期包装可平铺)时把实现迁入 `quilt/synth.rs`、DP 切割迁入 `quilt/seam.rs`,
//! 本文件只留声明与再导出,公开面(`quilt` / `box_blur_rgb`)不变。

mod seam;
mod synth;

pub(crate) use synth::box_blur_rgb;
pub use synth::{QuiltSpec, quilt, quilt_ex};
