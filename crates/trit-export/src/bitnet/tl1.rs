//! TL1 索引布局：`preprocess_weights_tl1` / `process_tl1` 的逐字节复刻。
//!
//! 来源：`microsoft/BitNet` 的 `utils/convert-hf-to-gguf-bitnet.py`。输入 M×K 三值，
//! 输出 `M*K/4` 字节：每字节 4 个三值，nibble = `3*d0 + d1`（`d ∈ {0,1,2}` 表示 `-1/0/+1`）。
//! tile 参数来自模型的 `kernel_config.ini`，`by = 256 / bmm`。

use trit_core::Ternary;

/// TL 的 tile 参数（`kernel_config.ini` 的一节）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Tile {
    pub m: usize,
    pub k: usize,
    pub bm: usize,
    pub bk: usize,
    pub bmm: usize,
}

/// TL 索引编解码错误。
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum TlError {
    #[error("TL tile 参数不整除：m={m} k={k} bm={bm} bk={bk} bmm={bmm}")]
    Tile {
        m: usize,
        k: usize,
        bm: usize,
        bk: usize,
        bmm: usize,
    },
    #[error("TL 三值数 {0} 与 tile 的 m*k 不符")]
    Len(usize),
    #[error("TL 字节数 {bytes} 与 m*k/4 = {expected} 不符")]
    Bytes { bytes: usize, expected: usize },
    #[error("TL nibble 越界：{0} > 8")]
    Nibble(u8),
}

impl Tile {
    /// 一个 `bm` 行带内的 `by`：Python 参考实现的 `by = 256 // bmm`。
    fn by(&self) -> usize {
        256 / self.bmm
    }

    fn check(&self) -> Result<(), TlError> {
        let by = self.by();
        let ok = self.m != 0
            && self.k != 0
            && self.k % 2 == 0
            && self.m % self.bm == 0
            && self.k % self.bk == 0
            && self.bm % self.bmm == 0
            && self.bmm % 16 == 0
            && by % 4 == 0
            && self.bk % by == 0;
        if ok {
            Ok(())
        } else {
            Err(TlError::Tile {
                m: self.m,
                k: self.k,
                bm: self.bm,
                bk: self.bk,
                bmm: self.bmm,
            })
        }
    }
}

pub(crate) fn digit(t: Ternary) -> usize {
    match t {
        Ternary::False => 0,
        Ternary::Unknown => 1,
        Ternary::True => 2,
    }
}

pub(crate) fn from_digit(d: usize) -> Ternary {
    match d {
        0 => Ternary::False,
        1 => Ternary::Unknown,
        _ => Ternary::True,
    }
}

/// 第 `q` 个「三值对」（第 `2q`、`2q+1` 个三值）落在哪个字节的哪一位。
///
/// 返回 `(byte, row16, t)`：`byte` 是输出字节下标，`row16 ∈ 0..16`，
/// `t = 0` 占高 4 bit、`t = 1` 占低 4 bit。
fn slot(tile: Tile, m: usize, q: usize) -> (usize, usize, usize) {
    let (by, bm, bk, bmm) = (tile.by(), tile.bm, tile.bk, tile.bmm);
    let half_by = by / 2;
    let (k_blk, qq) = (q / (bk / 2), q % (bk / 2));
    let (by_blk, qqy) = (qq / half_by, qq % half_by);
    let mb = m % bm;
    let outer = ((m / bm * (tile.k / bk) + k_blk) * (bm / bmm) + mb / bmm) * (bk / by) + by_blk;
    let byte = (outer * (bmm / 16) + (mb % bmm) / 16) * (by / 4) + qqy / 2;
    (byte, (mb % bmm) % 16, qqy % 2)
}

/// 三值矩阵（行主序 `m×k`）→ TL1 索引字节。
pub fn tl1_encode(trits: &[Ternary], tile: Tile) -> Result<Vec<u8>, TlError> {
    tile.check()?;
    if trits.len() != tile.m * tile.k {
        return Err(TlError::Len(trits.len()));
    }
    let mut out = vec![0u8; tile.m * tile.k / 4];
    for v in 0..tile.m * tile.k / 2 {
        let (m, q) = (v / (tile.k / 2), v % (tile.k / 2));
        let nib = 3 * digit(trits[m * tile.k + 2 * q]) + digit(trits[m * tile.k + 2 * q + 1]);
        let (i, j, t) = slot(tile, m, q);
        out[i * 16 + j] |= (nib as u8) << (4 * (1 - t));
    }
    Ok(out)
}

/// TL1 索引字节 → 三值矩阵（行主序 `m×k`）。
pub fn tl1_decode(bytes: &[u8], tile: Tile) -> Result<Vec<Ternary>, TlError> {
    tile.check()?;
    let expected = tile.m * tile.k / 4;
    if bytes.len() != expected {
        return Err(TlError::Bytes {
            bytes: bytes.len(),
            expected,
        });
    }
    let mut trits = vec![Ternary::Unknown; tile.m * tile.k];
    for v in 0..tile.m * tile.k / 2 {
        let (m, q) = (v / (tile.k / 2), v % (tile.k / 2));
        let (i, j, t) = slot(tile, m, q);
        let nib = (bytes[i * 16 + j] >> (4 * (1 - t))) & 0xF;
        if nib > 8 {
            return Err(TlError::Nibble(nib));
        }
        trits[m * tile.k + 2 * q] = from_digit((nib / 3) as usize);
        trits[m * tile.k + 2 * q + 1] = from_digit((nib % 3) as usize);
    }
    Ok(trits)
}
