//! Adam 基准的固定输入：确定性权重、`x` 与 `target`。

use trit_train::layer::TernaryLinear;
use trit_train::quant::Ste;

/// 每轮重新构造同一层，避免 Adam 状态跨轮污染。
pub(super) fn build(rows: usize, cols: usize) -> TernaryLinear {
    let mut state = 0xdead_beef_0bad_f00du64;
    let mut layer = TernaryLinear::new(
        rows,
        cols,
        |_, _| {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            ((state % 100) as f32 - 50.0) * 0.001
        },
        Ste::Soft,
        0.01,
        0.5,
    );
    layer.set_prefetch(false);
    layer
}

/// 单样本输入与目标；两条路径共用同一组数据。
pub(super) fn inputs(rows: usize, cols: usize) -> (Vec<f32>, Vec<f32>) {
    let mut data = 0x0123_4567_89ab_cdefu64;
    let x: Vec<f32> = (0..cols)
        .map(|_| {
            data ^= data << 13;
            data ^= data >> 7;
            data ^= data << 17;
            ((data % 2001) as f32 - 1000.0) / 1000.0
        })
        .collect();
    let target: Vec<f32> = (0..rows).map(|r| ((r % 13) as f32 - 6.0) * 0.2).collect();
    (x, target)
}
