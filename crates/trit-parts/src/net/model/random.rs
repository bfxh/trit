//! 按宽度倍数建网（从 `net/model.rs` 下沉）。
//!
//! `width` 直接来自 `--width`，通道按它线性放大；放大后的通道数又进
//! `kk * kk * cin * cout` 决定权重个数。所以校验必须在**放大之后、分配之前**。

use super::super::layer::{Layer, U2, U3};
use super::Net;
use crate::width::{check, scaled_channels};

pub(super) fn build(seed: u64, width: f32) -> Result<Net, String> {
    // 非法宽度会让整张网静默塌成 4 通道（见 `crate::width` 的文档）。
    check(width)?;
    let mut s = seed | 1;
    let (c32, c64, c128, out) = (
        scaled_channels(32, width),
        scaled_channels(64, width),
        scaled_channels(128, width),
        16u32,
    );
    let mut mk = |ci, co, kk, relu| Layer::random(ci, co, kk, relu, &mut s);
    let layers = vec![
        mk(3, c32, 3, true),
        mk(c32, c32, 3, true),
        mk(c32, c64, 3, true),
        mk(c64, c64, 3, true),
        mk(c64, c128, 3, true),
        mk(c128, c128, 3, true),
        mk(c128, c64, 2, false),
        mk(c64 + c64, c64, 3, true), // d2a 的输入是 cat(u3|c64, b|c64)
        mk(c64, c64, 3, true),
        mk(c64, c32, 2, false),
        mk(c32 + c32, c32, 3, true), // d1a 的输入是 cat(u2|c32, a|c32)
        mk(c32, c32, 3, true),
        mk(c32, out, 1, false),
    ];
    let mut layers = layers;
    layers[U3].convt = true;
    layers[U2].convt = true;
    let mut net = Net {
        layers,
        head_lr_mult: 1.0,
        freeze_until: 0,
        freeze_head: false,
    };
    net.mark_convt();
    Ok(net)
}
