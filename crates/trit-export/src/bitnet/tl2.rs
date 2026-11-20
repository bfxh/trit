//! TL2 索引布局：`preprocess_three_weights_tl2` 的逐字节复刻（three 段）。
//!
//! 每 3 个三值合成 `v = 9a + 3b + c ∈ [-13, 13]`：4-bit 幅度进 weight 段，1-bit 符号进
//! sign 段（每 16 个符号打包成 u16 小端）。参考实现硬约束：`by = 192 / bmm` 且 `by//3 == 2`。

use super::tl1::{Tile, digit, from_digit};
use trit_core::Ternary;

/// TL2 three 段的编解码错误。
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum Tl2Error {
    #[error("TL2 tile 参数不整除：m={m} k={k} bm={bm} bk={bk} bmm={bmm}")]
    Tile {
        m: usize,
        k: usize,
        bm: usize,
        bk: usize,
        bmm: usize,
    },
    #[error("TL2 三值数 {0} 与 tile 的 m*k 不符")]
    Len(usize),
    #[error("TL2 字节数 {bytes} 与 {expected} 不符")]
    Bytes { bytes: usize, expected: usize },
    #[error("TL2 幅度越界：{0} > 13")]
    Mag(u8),
}

fn by(tile: Tile) -> usize {
    192 / tile.bmm
}

fn check(tile: Tile) -> Result<(), Tl2Error> {
    let bad = Tl2Error::Tile {
        m: tile.m,
        k: tile.k,
        bm: tile.bm,
        bk: tile.bk,
        bmm: tile.bmm,
    };
    let by = by(tile);
    let ok = tile.m != 0
        && tile.k != 0
        && tile.k.is_multiple_of(3)
        && tile.m.is_multiple_of(tile.bm)
        && tile.k.is_multiple_of(tile.bk)
        && tile.bm.is_multiple_of(tile.bmm)
        && tile.bmm.is_multiple_of(16)
        && 192 % tile.bmm == 0
        && by / 3 == 2
        && tile.bk.is_multiple_of(by * 4)
        && (tile.bk / 3).is_multiple_of(8);
    if ok { Ok(()) } else { Err(bad) }
}

/// 第 `g` 组三值的组合值 `v = 9a + 3b + c`（`-13..=13`）。
fn combo(trits: &[Ternary], m: usize, g: usize, k: usize) -> i16 {
    let i = m * k + 3 * g;
    let d = |t: Ternary| digit(t) as i16;
    9 * d(trits[i]) + 3 * d(trits[i + 1]) + d(trits[i + 2]) - 13
}

fn rows(tile: Tile, sign: bool) -> usize {
    let by = by(tile);
    let depth = if sign { by * 4 } else { by };
    tile.m * tile.k / (tile.bmm * depth)
}

/// weight 段的 `(byte, t)`：`t = 0` 占高 4 bit，`t = 1` 占低 4 bit。
fn weight_slot(tile: Tile, m: usize, g: usize) -> (usize, usize) {
    let (by, gpb) = (by(tile), tile.bk / 3);
    let (g_blk, gg) = (g / gpb, g % gpb);
    let (by_blk, t) = (gg / 2, gg % 2);
    let mb = m % tile.bm;
    let row = ((m / tile.bm * (tile.k / tile.bk) + g_blk) * (tile.bm / tile.bmm) + mb / tile.bmm)
        * (tile.bk / by)
        + by_blk;
    let col = mb % tile.bmm;
    let col = if col / 8 < 4 {
        [0, 2, 1, 3][col / 8] * 8 + col % 8
    } else {
        col
    };
    (row * tile.bmm + col, t)
}

/// sign 段的 `(byte, bit)`：16 个符号打包成 u16 小端，`b = 15 - i`。
fn sign_slot(tile: Tile, m: usize, g: usize) -> (usize, usize) {
    let (by, gpb, half) = (by(tile), tile.bk / 3, tile.bmm / 2);
    let (g_blk, gg) = (g / gpb, g % gpb);
    let (g2, g3) = (gg / 8, gg % 8);
    let row = ((m / tile.bm * (tile.k / tile.bk) + g_blk) * (tile.bm / tile.bmm)
        + (m % tile.bm) / tile.bmm)
        * (tile.bk / (by * 4))
        + g2;
    let idx16 = g3 * tile.bmm + (m % tile.bm) % tile.bmm;
    let b = 15 - idx16 / half;
    let base = row * tile.bmm + (idx16 % half) * 2;
    (base + usize::from(b >= 8), b % 8)
}

/// 三值矩阵（行主序 `m×k`）→ TL2 three 段字节（weight 段 + sign 段）。
pub fn tl2_encode(trits: &[Ternary], tile: Tile) -> Result<Vec<u8>, Tl2Error> {
    check(tile)?;
    if trits.len() != tile.m * tile.k {
        return Err(Tl2Error::Len(trits.len()));
    }
    let base = rows(tile, false) * tile.bmm;
    let mut out = vec![0u8; base + rows(tile, true) * tile.bmm];
    for m in 0..tile.m {
        for g in 0..tile.k / 3 {
            let v = combo(trits, m, g, tile.k);
            let mag = v.unsigned_abs() as u8;
            let (byte, t) = weight_slot(tile, m, g);
            out[byte] |= if t == 0 { mag << 4 } else { mag };
            if v < 0 {
                let (byte, bit) = sign_slot(tile, m, g);
                out[base + byte] |= 1 << bit;
            }
        }
    }
    Ok(out)
}

/// TL2 three 段字节 → 三值矩阵（行主序 `m×k`）。
pub fn tl2_decode(bytes: &[u8], tile: Tile) -> Result<Vec<Ternary>, Tl2Error> {
    check(tile)?;
    let base = rows(tile, false) * tile.bmm;
    let expected = base + rows(tile, true) * tile.bmm;
    if bytes.len() != expected {
        return Err(Tl2Error::Bytes {
            bytes: bytes.len(),
            expected,
        });
    }
    let mut trits = vec![Ternary::Unknown; tile.m * tile.k];
    for m in 0..tile.m {
        for g in 0..tile.k / 3 {
            let (byte, t) = weight_slot(tile, m, g);
            let mag = if t == 0 {
                bytes[byte] >> 4
            } else {
                bytes[byte] & 0xF
            };
            if mag > 13 {
                return Err(Tl2Error::Mag(mag));
            }
            let (byte, bit) = sign_slot(tile, m, g);
            let neg = (bytes[base + byte] >> bit) & 1 == 1;
            let v = if neg { -(mag as i16) } else { mag as i16 };
            let idx = (v + 13) as usize;
            let i = m * tile.k + 3 * g;
            trits[i] = from_digit(idx / 9);
            trits[i + 1] = from_digit((idx / 3) % 3);
            trits[i + 2] = from_digit(idx % 3);
        }
    }
    Ok(trits)
}
