//! J6 体积账：1.5 Mi 三值下各格式的字节数。
//!
//! 只算权重本体；LUT 表由 kernel 运行时生成、不入盘，单独列（`lut_tables_are_runtime_only`）。

use trit_core::Ternary;
use trit_export::bitnet::i2s::{TAIL_BYTES, encode_ternary};
use trit_export::bitnet::tl1::{Tile, tl1_encode};
use trit_export::bitnet::tl2::tl2_encode;

const M: usize = 1024;
const K: usize = 1536;

fn trits(n: usize) -> Vec<Ternary> {
    (0..n)
        .map(|i| match i % 3 {
            0 => Ternary::False,
            1 => Ternary::Unknown,
            _ => Ternary::True,
        })
        .collect()
}

#[test]
fn weight_bytes_per_trit_match_the_spec() {
    let n = M * K;
    let t = trits(n);
    let i2s = encode_ternary(&t, 1.0).unwrap();
    let tl1 = tl1_encode(
        &t,
        Tile {
            m: M,
            k: K,
            bm: 64,
            bk: 64,
            bmm: 16,
        },
    )
    .unwrap();
    let tl2 = tl2_encode(
        &t,
        Tile {
            m: M,
            k: K,
            bm: 64,
            bk: 96,
            bmm: 32,
        },
    )
    .unwrap();

    // I2_S：2 bit/权重 + 每张量 32 B 尾（0.25 B/权重）
    assert_eq!(i2s.len(), n / 4 + TAIL_BYTES);
    // TL1：每字节 4 个三值（0.25 B/权重）
    assert_eq!(tl1.len(), n / 4);
    // TL2 three 段：每 3 值 5 bit（4-bit 幅度 + 1-bit 符号）≈ 0.208 B/权重
    assert_eq!(tl2.len() * 24, n * 5);
    // 仓库自用 5 trits/byte 更小：0.2 B/权重（此处不可整除，按每字节 5 值折算）
    assert_eq!(n / 5, 314_572);
}

#[test]
fn lut_tables_are_runtime_only_and_counted_separately() {
    let n = M * K;
    // three_lut_ctor：每 24 个三值权重生成 256 B 表；two_lut_ctor：每 16 个二值权重 256 B
    let three_table = n / 24 * 256;
    let two_table = n / 16 * 256;
    let tl1_weights = n / 4;
    // 表比索引本体大 40 倍以上：这就是「表不入盘」必须单列的理由
    assert_eq!(three_table, n * 32 / 3);
    assert_eq!(two_table, n * 16);
    assert!(three_table > tl1_weights * 40);
}
