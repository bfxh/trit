//! per-token 对称 int8 激活量化。

/// per-token 对称 int8：`q = round(x / scale)`，再反量化回 f32。
pub(crate) fn quantize_int8(values: &mut [f32], width: usize) {
    for chunk in values.chunks_mut(width.max(1)) {
        let max = chunk.iter().map(|value| value.abs()).fold(0.0, f32::max);
        let scale = if max > 0.0 { max / 127.0 } else { 1.0 };
        for value in chunk {
            *value = ((*value / scale).round().clamp(-127.0, 127.0)) * scale;
        }
    }
}
