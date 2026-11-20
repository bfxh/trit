//! TL1/TL2 的查表数学核心（表由权重现场生成，不入盘）。
//!
//! `three_lut_ctor` / `two_lut_ctor` 的 kernel 内布局依赖寄存器 shuffle；本模块只复算
//! “每个三值/二值组合对应的线性组合值”这一可移植层。索引的 byte 布局见 `tl1`（TL1）
//! 与 `tl2`（TL2）。

use trit_core::Ternary;

/// 三值组合 → 索引：`idx = 9*a + 3*b + c`，`a/b/c ∈ {0,1,2}` 对应 `-1/0/+1`。
pub fn three_index(trits: [Ternary; 3]) -> usize {
    let digit = |t: Ternary| match t {
        Ternary::False => 0,
        Ternary::Unknown => 1,
        Ternary::True => 2,
    };
    9 * digit(trits[0]) + 3 * digit(trits[1]) + digit(trits[2])
}

/// 二值组合 → 索引：`idx = 2*a + b`，`false/true` 对应 `-1/+1`。
pub fn two_index(bits: [bool; 2]) -> usize {
    let digit = |b: bool| usize::from(b);
    2 * digit(bits[0]) + digit(bits[1])
}

/// 三值表：27 项，`table[idx] = round(scale * Σ w_i q_i)`。
pub fn build_three_table(weights: [f32; 3], scale: f32) -> [i8; 27] {
    let mut table = [0i8; 27];
    for (idx, slot) in table.iter_mut().enumerate() {
        let a = (idx / 9) as f32 - 1.0;
        let b = ((idx / 3) % 3) as f32 - 1.0;
        let c = (idx % 3) as f32 - 1.0;
        let sum = scale * (weights[0] * a + weights[1] * b + weights[2] * c);
        *slot = sum.round().clamp(-128.0, 127.0) as i8;
    }
    table
}

/// 二值表：4 项，`table[idx] = round(scale * Σ w_i q_i)`。
pub fn build_two_table(weights: [f32; 2], scale: f32) -> [i8; 4] {
    let mut table = [0i8; 4];
    for (idx, slot) in table.iter_mut().enumerate() {
        let a = if idx / 2 == 0 { -1.0 } else { 1.0 };
        let b = if idx % 2 == 0 { -1.0 } else { 1.0 };
        let sum = scale * (weights[0] * a + weights[1] * b);
        *slot = sum.round().clamp(-128.0, 127.0) as i8;
    }
    table
}
