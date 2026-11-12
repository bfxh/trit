//! gray 的单元测试（从 src/gray.rs 拆出，控制单文件行数预算）。

use super::*;

fn chars(word: &[Ternary]) -> String {
    word.iter().map(|t| t.as_char()).collect()
}

#[test]
fn n_zero_only_allows_index_zero() {
    assert_eq!(encode(0, 0).unwrap(), Vec::<Ternary>::new());
    assert!(matches!(
        encode(1, 0),
        Err(CodecError::GrayIndexOutOfRange { index: 1, n: 0 })
    ));
    assert_eq!(decode(&[]), 0);
}

#[test]
fn out_of_range_index_is_rejected() {
    // n=2 → 合法 index 0..=8。
    assert!(encode(9, 2).is_err());
    assert!(encode(0, 2).is_ok());
}

#[test]
fn hand_known_n2_table() {
    // 手算 n=2：k[0]=LSB, k[1]=MSB, k[2]=0。
    // idx 0: k=(0,0,0) → g=(0-0,0-0)=(0,0) → "00"
    // idx 1: k=(1,0,0) → g=(1-0,0-0)=(1,0) → "X0"
    // idx 2: k=(2,0,0) → g=(2-0,0-0)=(2,0) → "10"
    // idx 3: k=(0,1,0) → g=(0-1,1-0)=(2,1) → "1X"
    let cases = [(0usize, "00"), (1, "X0"), (2, "10"), (3, "1X")];
    for (idx, expect) in cases {
        let w = encode(idx, 2).unwrap();
        assert_eq!(chars(&w), expect, "idx={idx}");
        assert_eq!(decode(&w), idx);
    }
}

#[test]
fn exhaustive_n2_adjacent_distance_is_one() {
    let n = 2;
    let size = 3usize.pow(n as u32);
    let words: Vec<Vec<Ternary>> = (0..size).map(|i| encode(i, n).unwrap()).collect();
    for k in 0..size - 1 {
        let dist = words[k]
            .iter()
            .zip(words[k + 1].iter())
            .filter(|(a, b)| a != b)
            .count();
        assert_eq!(
            dist,
            1,
            "idx {k} vs {} words {} {}",
            k + 1,
            chars(&words[k]),
            chars(&words[k + 1])
        );
        // round-trip
        assert_eq!(decode(&words[k]), k);
    }
}

#[test]
fn exhaustive_n3_adjacent_distance_is_one() {
    let n = 3;
    let size = 3usize.pow(n as u32);
    let words: Vec<Vec<Ternary>> = (0..size).map(|i| encode(i, n).unwrap()).collect();
    for k in 0..size - 1 {
        let dist = words[k]
            .iter()
            .zip(words[k + 1].iter())
            .filter(|(a, b)| a != b)
            .count();
        assert_eq!(dist, 1, "idx {k} vs {}", k + 1);
        assert_eq!(decode(&words[k]), k);
    }
    // 最后一个码字应该也是合法的
    assert_eq!(decode(&words[size - 1]), size - 1);
}

#[test]
fn word_length_matches_n() {
    for n in 1..=6 {
        let w = encode(0, n).unwrap();
        assert_eq!(w.len(), n);
    }
}

#[test]
fn n_one_exhaustive() {
    // n=1：3 个状态 0/X/1，相邻两两汉明距离 1，往返恒等。
    let words: Vec<Vec<Ternary>> = (0..3).map(|i| encode(i, 1).unwrap()).collect();
    assert_eq!(
        words.iter().map(|w| chars(w)).collect::<Vec<_>>(),
        ["0", "X", "1"]
    );
    for k in 0..2 {
        let dist = words[k]
            .iter()
            .zip(words[k + 1].iter())
            .filter(|(a, b)| a != b)
            .count();
        assert_eq!(dist, 1, "idx {k} vs {}", k + 1);
        assert_eq!(decode(&words[k]), k);
    }
    assert_eq!(decode(&words[2]), 2);
}

#[test]
fn zero_index_is_all_false_anchor() {
    // k=0 的三进制位全 0 ⇒ g[i]=(0-0)=0 ⇒ 码字全 False（"000…0"），这是格雷码的锚点。
    for n in 0..=6usize {
        let w = encode(0, n).unwrap();
        assert!(
            w.iter().all(|t| *t == Ternary::False),
            "n={n} -> {}",
            chars(&w)
        );
        assert_eq!(decode(&w), 0, "n={n}");
    }
}

#[test]
fn overlong_word_saturates_without_panicking() {
    // 60 位码字还原出的 index 远超 usize::MAX（32/64 位都溢出）。
    // 实验层铁律：绝不 panic——必须饱和到 usize::MAX 而非 debug 下的 multiply overflow。
    let word = vec![Ternary::True; 60];
    assert_eq!(decode(&word), usize::MAX);
    // 空字仍得 0（不受饱和影响）。
    assert_eq!(decode(&[]), 0);
}

#[test]
fn huge_n_is_rejected_not_truncated_into_giant_alloc() {
    // M3：n > u32::MAX 时 `n as u32` 截断会绕过 3^n 检查、再 `with_capacity(n)` 巨量分配；
    // 现在 `u32::try_from(n)` 失败即按越界拒绝，不分配。
    assert!(encode(0, u32::MAX as usize + 1).is_err());
    assert!(encode(0, 1usize << 32).is_err());
}
