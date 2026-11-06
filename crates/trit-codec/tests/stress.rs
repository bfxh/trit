//! 编解码高压不变量：随机长度、全部字节值、随机 i64、随机单符号错误。

use trit_codec::{CodecError, balanced, hamming, pack};
use trit_core::value::Ternary;

// RNG 与三值取样都改吃 trit-fuzz（同一条 xorshift：13/7/17）。
// 注意取值顺序：本地那份是 0→False, 1→Unknown, 2→True，
// `corpus::trit` 的口径是 0→False, 1→True, 2→Unknown —— 两者都是均匀三值，
// 这些测试断言的是往返/纠错性质，不钉具体序列。
use trit_fuzz::SeedRng;
use trit_fuzz::corpus::trit;

#[test]
fn pack_round_trips_every_short_length() {
    let mut rng = SeedRng::new(0x9E37_79B9_7F4A_7C15);
    for len in 0..=256usize {
        let values: Vec<Ternary> = (0..len).map(|_| trit(&mut rng)).collect();
        let bytes = pack::pack(&values);
        assert_eq!(bytes.len(), len.div_ceil(pack::TRITS_PER_BYTE), "len={len}");
        assert_eq!(pack::unpack(&bytes, len).unwrap(), values, "len={len}");
    }
}

#[test]
fn pack_round_trips_large_payload() {
    let mut rng = SeedRng::new(7);
    let values: Vec<Ternary> = (0..100_000).map(|_| trit(&mut rng)).collect();
    let bytes = pack::pack(&values);
    assert_eq!(pack::unpack(&bytes, values.len()).unwrap(), values);
}

#[test]
fn every_byte_value_is_classified() {
    for byte in 0..=255u8 {
        let result = pack::unpack(&[byte], 5);
        if u16::from(byte) >= pack::COMBINATIONS {
            assert!(matches!(result, Err(CodecError::InvalidByte(_))), "{byte}");
        } else {
            assert!(result.is_ok(), "{byte}");
        }
    }
}

#[test]
fn balanced_round_trips_random_i64() {
    let mut rng = SeedRng::new(0xBEEF);
    let mut values: Vec<i64> = (0..2000).map(|_| rng.next_u64() as i64).collect();
    values.extend([i64::MIN, i64::MAX, 0, 1, -1]);
    for value in values {
        let text = balanced::encode(value);
        assert_eq!(balanced::decode(&text).unwrap(), value, "{text}");
        assert!(text.chars().count() <= 41, "{value} -> {text}");
    }
}

#[test]
fn hamming_corrects_random_single_errors() {
    let mut rng = SeedRng::new(0x1234_5678);
    let trits = [Ternary::False, Ternary::Unknown, Ternary::True];
    for _ in 0..200 {
        let message = [
            trits[(rng.next_u64() % 3) as usize],
            trits[(rng.next_u64() % 3) as usize],
        ];
        let word = hamming::encode(message[0], message[1]);
        for position in 0..hamming::CODEWORD {
            for magnitude in 1..=2u8 {
                let mut corrupted = word;
                let digit = match corrupted[position] {
                    Ternary::False => 0,
                    Ternary::Unknown => 1,
                    Ternary::True => 2,
                };
                corrupted[position] = match (digit + i32::from(magnitude)) % 3 {
                    0 => Ternary::False,
                    1 => Ternary::Unknown,
                    _ => Ternary::True,
                };
                let decoded = hamming::decode(corrupted);
                assert_eq!(decoded.message, message, "位置 {position} 幅度 {magnitude}");
                assert_eq!(decoded.corrected, Some((position, magnitude)));
            }
        }
    }
}
