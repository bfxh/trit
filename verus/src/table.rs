//! K3（强 Kleene）三值真值表的**单一来源**。
//!
//! 编码与 `trit-core::value::Ternary` 的 `index()` 一致：`F = 0`、`U = 1`、`T = 2`。
//! 普通 Rust 测试（`tests/bridge.rs`）和 Verus 规格（`src/k3.rs`）都读这张表，
//! 因此“参照实现”与“被证明的规格”不会各自漂移。

/// 假。
pub const F: u8 = 0;
/// 未知。
pub const U: u8 = 1;
/// 真。
pub const T: u8 = 2;

/// 三值的全部编码。
pub const ALL: [u8; 3] = [F, U, T];

/// K3 否定：F <-> T，U 不变。
pub const NOT: [u8; 3] = [T, U, F];

/// K3 合取：`F ∧ x = F`，`T ∧ x = x`，其余 U。
pub const AND: [[u8; 3]; 3] = [[F, F, F], [F, U, U], [F, U, T]];

/// K3 析取：`T ∨ x = T`，`F ∨ x = x`，其余 U。
pub const OR: [[u8; 3]; 3] = [[F, U, T], [U, U, T], [T, T, T]];

/// K3 相等：任一 U 即 U；两个已知值相同为 T，不同为 F。
pub const EQ: [[u8; 3]; 3] = [[T, U, F], [U, U, U], [F, U, T]];

/// 经典异或（`trit-core` 的 `BitXor`）：任一 U 即 U，其余同经典异或。
/// 在 K3 中等价于 `NOT ∘ EQ`。
pub const XOR: [[u8; 3]; 3] = [[F, U, T], [U, U, U], [T, U, F]];

/// K3 否定。
pub fn not(a: u8) -> u8 {
    NOT[a as usize]
}

/// K3 合取。
pub fn and(a: u8, b: u8) -> u8 {
    AND[a as usize][b as usize]
}

/// K3 析取。
pub fn or(a: u8, b: u8) -> u8 {
    OR[a as usize][b as usize]
}

/// K3 相等。
pub fn eq(a: u8, b: u8) -> u8 {
    EQ[a as usize][b as usize]
}

/// K3 不等：`eq` 的 K3 否定。
pub fn ne(a: u8, b: u8) -> u8 {
    not(eq(a, b))
}

/// 经典异或。
pub fn xor(a: u8, b: u8) -> u8 {
    XOR[a as usize][b as usize]
}
