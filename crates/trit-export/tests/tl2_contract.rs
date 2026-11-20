//! TL2 three 段索引布局契约：与 Python 参考实现（`preprocess_three_weights_tl2`）逐字节对拍。
//! 夹具 `fixtures/tl2_golden.txt` 每行：`m k bm bk bmm <hex>`；输入由 `pattern` 生成。

use trit_core::Ternary;
use trit_export::bitnet::tl1::Tile;
use trit_export::bitnet::tl2::{Tl2Error, tl2_decode, tl2_encode};

const GOLDEN: &str = include_str!("fixtures/tl2_golden.txt");

/// 夹具输入：与 Python 侧同一 LCG（`s = s*1664525 + 1013904223`，`(s>>16)%3 - 1`）。
fn pattern(m: usize, k: usize) -> Vec<Ternary> {
    let mut s: u32 = 0x1234_5678;
    (0..m * k)
        .map(|_| {
            s = s.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            match (s >> 16) % 3 {
                0 => Ternary::False,
                1 => Ternary::Unknown,
                _ => Ternary::True,
            }
        })
        .collect()
}

fn tile(m: usize, k: usize, bm: usize, bk: usize) -> Tile {
    Tile {
        m,
        k,
        bm,
        bk,
        bmm: 32,
    }
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
    (tile(n[0], n[1], n[2], n[3]), bytes)
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
fn tl2_matches_python_reference_byte_for_byte() {
    let mut seen = 0;
    for line in GOLDEN.lines().filter(|l| !l.trim().is_empty()) {
        let (t, want) = parse(line);
        let trits = pattern(t.m, t.k);
        assert_bytes(&tl2_encode(&trits, t).unwrap(), &want, &format!("{t:?}"));
        assert_trits(&tl2_decode(&want, t).unwrap(), &trits, &format!("{t:?}"));
        seen += 1;
    }
    assert_eq!(seen, 3, "夹具应有 3 组");
}

#[test]
fn tl2_roundtrip_is_exact() {
    for t in [
        tile(32, 48, 32, 48),
        tile(64, 96, 32, 48),
        tile(32, 96, 32, 96),
        tile(64, 96, 64, 96),
    ] {
        let trits = pattern(t.m, t.k);
        let bytes = tl2_encode(&trits, t).unwrap();
        // three 段总体积：每 3 个三值 5 bit → m*k*5/(4*by)
        assert_eq!(bytes.len(), t.m * t.k * 5 / (4 * 6));
        assert_trits(&tl2_decode(&bytes, t).unwrap(), &trits, &format!("{t:?}"));
    }
}

#[test]
fn tl2_rejects_bad_inputs() {
    let t = tile(32, 48, 32, 48);
    assert_eq!(tl2_encode(&[Ternary::Unknown; 5], t), Err(Tl2Error::Len(5)));
    let want = tl2_encode(&pattern(32, 48), t).unwrap();
    let short = Err(Tl2Error::Bytes {
        bytes: 4,
        expected: want.len(),
    });
    assert_eq!(tl2_decode(&[0u8; 4], t), short);
    // bmm=16 → by=12、by/3=4，参考实现只支持 by/3==2
    let bad = tile(32, 48, 32, 96);
    let bad = Tile { bmm: 16, ..bad };
    assert!(matches!(
        tl2_encode(&pattern(32, 48), bad),
        Err(Tl2Error::Tile { .. })
    ));
    // 幅度 nibble 上限是 13；0xE 必须被拒
    let mut bytes = want.clone();
    bytes[0] |= 0xE0;
    assert_eq!(tl2_decode(&bytes, t), Err(Tl2Error::Mag(0xE)));
}
