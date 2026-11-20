//! TL1 索引布局契约：与 Python 参考实现（`preprocess_weights_tl1`）逐字节对拍。
//! 夹具 `fixtures/tl1_golden.txt` 每行：`m k bm bk bmm <hex>`；输入由 `pattern` 生成。

use trit_core::Ternary;
use trit_export::bitnet::tl1::{Tile, TlError, tl1_decode, tl1_encode};

const GOLDEN: &str = include_str!("fixtures/tl1_golden.txt");

/// 夹具输入：`flat[i] = ((i*5 + 3) % 7) % 3 - 1`。
fn pattern(m: usize, k: usize) -> Vec<Ternary> {
    (0..m * k)
        .map(|i| match ((i * 5 + 3) % 7) % 3 {
            0 => Ternary::False,
            1 => Ternary::Unknown,
            _ => Ternary::True,
        })
        .collect()
}

fn tile(m: usize, k: usize, bm: usize, bk: usize, bmm: usize) -> Tile {
    Tile { m, k, bm, bk, bmm }
}

/// 解析夹具行 → `(tile, 期望字节)`。
fn parse(line: &str) -> (Tile, Vec<u8>) {
    let mut it = line.split(' ');
    let mut n = [0usize; 5];
    for slot in n.iter_mut() {
        *slot = it.next().unwrap().parse().unwrap();
    }
    let h = it.next().unwrap();
    let bytes = (0..h.len() / 2)
        .map(|i| u8::from_str_radix(&h[2 * i..2 * i + 2], 16).unwrap())
        .collect();
    (tile(n[0], n[1], n[2], n[3], n[4]), bytes)
}

/// 逐字节比对，失败只报首个位置。
fn assert_bytes(got: &[u8], want: &[u8], ctx: &str) {
    assert_eq!(got.len(), want.len(), "{ctx}: 长度");
    if let Some(i) = (0..got.len()).find(|&i| got[i] != want[i]) {
        panic!(
            "{ctx}: 第 {i} 字节不符 got={:02x} want={:02x}",
            got[i], want[i]
        );
    }
}

/// 逐个三值比对，失败只报首个位置。
fn assert_trits(got: &[Ternary], want: &[Ternary], ctx: &str) {
    assert_eq!(got.len(), want.len(), "{ctx}: 长度");
    if let Some(i) = (0..got.len()).find(|&i| got[i] != want[i]) {
        panic!(
            "{ctx}: 第 {i} 个三值不符 got={:?} want={:?}",
            got[i], want[i]
        );
    }
}

#[test]
fn tl1_matches_python_reference_byte_for_byte() {
    let mut seen = 0;
    for line in GOLDEN.lines().filter(|l| !l.trim().is_empty()) {
        let (t, want) = parse(line);
        let trits = pattern(t.m, t.k);
        assert_bytes(&tl1_encode(&trits, t).unwrap(), &want, &format!("{t:?}"));
        let back = tl1_decode(&want, t).unwrap();
        assert_trits(&back, &trits, &format!("{t:?} decode"));
        seen += 1;
    }
    assert_eq!(seen, 3, "夹具应有 3 组");
}

#[test]
fn tl1_roundtrip_is_exact() {
    for t in [
        tile(32, 48, 32, 16, 16),
        tile(16, 96, 16, 32, 16),
        tile(64, 32, 32, 32, 16),
        tile(32, 64, 32, 32, 32),
    ] {
        let trits = pattern(t.m, t.k);
        let bytes = tl1_encode(&trits, t).unwrap();
        assert_eq!(bytes.len(), t.m * t.k / 4);
        assert_trits(&tl1_decode(&bytes, t).unwrap(), &trits, &format!("{t:?}"));
    }
}

#[test]
fn tl1_rejects_bad_inputs() {
    let t = tile(32, 48, 32, 16, 16);
    assert_eq!(tl1_encode(&[Ternary::Unknown; 4], t), Err(TlError::Len(4)));
    let short = Err(TlError::Bytes {
        bytes: 3,
        expected: 384,
    });
    assert_eq!(tl1_decode(&[0u8; 3], t), short);
    let bad_bm = tile(32, 48, 24, 16, 16);
    assert!(matches!(
        tl1_encode(&pattern(32, 48), bad_bm),
        Err(TlError::Tile { .. })
    ));
    let bad_by = tile(32, 48, 32, 16, 8);
    assert!(matches!(
        tl1_encode(&pattern(32, 48), bad_by),
        Err(TlError::Tile { .. })
    ));
    let mut bytes = tl1_encode(&pattern(32, 48), t).unwrap();
    bytes[0] = 0xFF;
    assert_eq!(tl1_decode(&bytes, t), Err(TlError::Nibble(0xF)));
}
