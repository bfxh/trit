//! b64 相位变体单元测试（拆出守预算）——官方探针回放（2026-11-13）。

use super::{find_union, variants};

#[test]
fn probe_semantics_replay() {
    // "foobar"（6 字节，整组）：3 变体 = 标准全量 + 两相位 drop 后缀
    assert_eq!(
        variants(b"foobar", false, false),
        vec![
            b"Zm9vYmFy".to_vec(),
            b"Zvb2Jhc".to_vec(),
            b"mb29iYX".to_vec()
        ]
    );
    // "foobarX"（7 字节）：phase0 尾 drop（"Zm9vYmFyW"）；phase2 的
    // 末字符 = 'X' 低 6 位（'Y'）——官方差分 MATCH 11/DIFF 0 实证
    assert_eq!(
        variants(b"foobarX", false, false),
        vec![
            b"Zm9vYmFyW".to_vec(),
            b"Zvb2Jhcl".to_vec(),
            b"mb29iYXJY".to_vec()
        ]
    );
    // 1 字节：{Q, B}（p2 变体 "B"——官方判别探针 b'B' 命中实证）
    assert_eq!(
        variants(b"A", false, false),
        vec![b"Q".to_vec(), b"B".to_vec()]
    );
    // 2 字节：{QU, FC, BQ}（p2 变体 "BQ"——官方判别探针 b'BQ' 命中实证）
    assert_eq!(
        variants(b"AB", false, false),
        vec![b"QU".to_vec(), b"FC".to_vec(), b"BQ".to_vec()]
    );
    // base64wide：编码后逐字节 \0（\x00 避开八进制转义形）
    assert_eq!(
        variants(b"foobar", false, true),
        vec![
            b"Z\x00m\x009\x00v\x00Y\x00m\x00F\x00y\x00".to_vec(),
            b"Z\x00v\x00b\x002\x00J\x00h\x00c\x00".to_vec(),
            b"m\x00b\x002\x009\x00i\x00Y\x00X\x00".to_vec()
        ]
    );
    // C1 探针：wide base64 = base64(wide(pt))，三相位（官方差分实证）
    assert_eq!(
        variants(b"foobar", true, false),
        vec![
            b"ZgBvAG8AYgBhAHIA".to_vec(),
            b"YAbwBvAGIAYQByA".to_vec(),
            b"mAG8AbwBiAGEAcg".to_vec()
        ]
    );
    // C5 探针：base64wide wide = wide(b64(wide(pt)))，wide 两侧叠加
    assert_eq!(
        variants(b"foobar", true, true),
        vec![
            b"Z\x00g\x00B\x00v\x00A\x00G\x008\x00A\x00Y\x00g\x00B\x00h\x00A\x00H\x00I\x00A\x00"
                .to_vec(),
            b"Y\x00A\x00b\x00w\x00B\x00v\x00A\x00G\x00I\x00A\x00Y\x00Q\x00B\x00y\x00A\x00".to_vec(),
            b"m\x00A\x00G\x008\x00A\x00b\x00w\x00B\x00i\x00A\x00G\x00E\x00A\x00c\x00g\x00".to_vec()
        ]
    );
}

#[test]
fn union_counts_across_variants() {
    // U1 探针：标准 + phase1 编码同文件 → 并集 2 命中
    let tps: Vec<_> = variants(b"foobar", false, false)
        .into_iter()
        .map(super::TextPattern::literal)
        .collect();
    let (hits, complete) = find_union(b"Zm9vYmFy|Zvb2Jhc", &tps);
    assert_eq!(hits, vec![0, 9]);
    assert!(complete);
}
