//! Q4_0 权重打包（拆出 `bench.rs` 守模块行数；第 37 轮 S1 修好后按行补齐）。
//!
//! 每行独立按 `ceil(cols/32)` 出块、末块补零，与 `gemv_q4` 的读取口径一致。

use super::to_f16;

/// 把 f32 权重按 Q4_0 打包（每块 32：f16 scale + 16 字节 nibble）。
pub fn pack_q4(w: &[f32], rows: usize, cols: usize) -> Vec<u8> {
    assert_eq!(
        w.len(),
        rows.saturating_mul(cols),
        "权重个数必须是 rows*cols"
    );
    let bpr = cols.div_ceil(32) * 18;
    let mut out = Vec::with_capacity(rows * bpr);
    for row in w.chunks(cols.max(1)) {
        for blk in row.chunks(32) {
            let amax = blk.iter().fold(0.0f32, |a, v| a.max(v.abs()));
            let d = if amax > 0.0 { amax / 7.0 } else { 0.0 };
            out.extend_from_slice(&to_f16(d).to_le_bytes());
            let mut bytes = [0u8; 16];
            for (i, slot) in bytes.iter_mut().enumerate() {
                let at = |k: usize| blk.get(k).copied().unwrap_or(0.0);
                let q0 = if d > 0.0 {
                    (at(i) / d).round().clamp(-8.0, 7.0) as i32 + 8
                } else {
                    8
                };
                let q1 = if d > 0.0 {
                    (at(i + 16) / d).round().clamp(-8.0, 7.0) as i32 + 8
                } else {
                    8
                };
                *slot = ((q0 as u8) & 0x0f) | (((q1 as u8) & 0x0f) << 4);
            }
            out.extend_from_slice(&bytes);
        }
    }
    out
}
