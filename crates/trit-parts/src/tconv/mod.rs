//! 小型三值卷积栈（CPU）：3×3、same padding、隐藏层 ReLU、输出 sigmoid + BCE。
//! 用途：证明「扩感受野要靠加深 + 共享权重」而不是把 patch 摊平进 MLP。
//! 权重三值（{-1,0,+1}，STE 直通），bias 与输出层保持 f32；第 0 通道是图像，后面拼 one-hot 条件。

mod bn;
mod cap;
mod conv;
mod diffusion;
mod io;
mod net;

pub use bn::{AuxCls, Bn, Film};
pub use cap::{MAX_CH, check_ch};
pub use conv::{Conv, Sample};
pub use diffusion::{EpsDen, VDen, ddim_sample_eps};
pub use net::Net;

#[cfg(test)]
mod tests;
