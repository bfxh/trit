//! 线性光 tone mapping。

use super::ToneMap;

pub(super) fn map_channel(v: f32, mode: ToneMap) -> f32 {
    let v = v.max(0.0);
    match mode {
        ToneMap::None => v,
        ToneMap::Reinhard => v / (1.0 + v),
        ToneMap::Aces => (v * (2.51 * v + 0.03)) / (v * (2.43 * v + 0.59) + 0.14),
    }
}
