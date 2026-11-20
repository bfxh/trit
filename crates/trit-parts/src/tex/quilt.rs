//! 非参数 patch 合成(`tex-quilt`)的门面:实现在子模块里(后处理在 `post.rs`)。
//!
//! 历史预算注记:本文件曾 282 行顶格;2026-11-20 收台账缺口(graph-cut 缝合 +
//! 周期包装可平铺)后实现迁入 synth/seam,后处理迁入 post;公开面(`quilt`/`box_blur_rgb`)不变。

mod post;
mod seam;
mod synth;

pub(crate) use post::box_blur_rgb;
pub use synth::{QuiltSpec, quilt, quilt_ex};
