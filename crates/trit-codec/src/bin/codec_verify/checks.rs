//! 逐项复核：每条检查自带独立 oracle，不复用被测函数的解码路径。
//!
//! 口径见 `docs/research/TERNARY-AUDIT.md`：往返（encode→decode 逐位还原）只是底线，
//! 真正的复核是「另写一份最小实现，与库输出逐位比对」，再加上纠错/检错与边界拒绝。

use trit_codec::{balanced, checksum, delta, gray, hamming, interleave, pack, plan, rle, sparse};
use trit_core::value::Ternary;

use super::corpus::Sample;
use super::report::{Check, Report};

/// 三条合法 trit，按 wire format 的枚举顺序：False=0 / Unknown=1 / True=2。
const TRITS: [Ternary; 3] = [Ternary::False, Ternary::Unknown, Ternary::True];

pub fn run(report: &mut Report, samples: &[Sample]) {
    pack_roundtrip(report, samples);
    pack_oracle(report, samples);
    pack_errors(report);
    balanced_checks(report);
    hamming_checks(report);
    interleave_checks(report);
    rle_checks(report, samples);
    delta_checks(report, samples);
    gray_checks(report);
    sparse_checks(report, samples);
    checksum_checks(report, samples);
    plan_checks(report, samples);
}

// ---------------------------------------------------------------- pack

/// 独立打包器：`byte = Σ trit[i]·3^i`，每 5 个 trit 成一个字节。
fn oracle_pack(values: &[Ternary]) -> Vec<u8> {
    let mut out = Vec::new();
    for chunk in values.chunks(5) {
        let mut byte = 0u32;
        let mut weight = 1u32;
        for value in chunk {
            byte += value.index() as u32 * weight;
            weight *= 3;
        }
        out.push(byte as u8);
    }
    out
}

/// 独立解包器：按 3 取余逐位还原，低位在前。
fn oracle_unpack(bytes: &[u8], len: usize) -> Vec<Ternary> {
    let mut out = Vec::new();
    'outer: for &byte in bytes {
        let mut rest = byte as u32;
        for _ in 0..5 {
            if out.len() == len {
                break 'outer;
            }
            out.push(Ternary::from_index((rest % 3) as usize));
            rest /= 3;
        }
    }
    out
}

fn pack_roundtrip(report: &mut Report, samples: &[Sample]) {
    let mut check = Check::new("pack.roundtrip");
    for sample in samples {
        let bytes = pack::pack(&sample.values);
        check.case(bytes.len() == sample.values.len().div_ceil(5), || {
            format!("{}: 字节数不符", sample.name)
        });
        match pack::unpack(&bytes, sample.values.len()) {
            Ok(back) => check.case(back == sample.values, || {
                format!("{}: 宽松解码不等", sample.name)
            }),
            Err(error) => check.case(false, || format!("{}: 宽松解码报错 {error}", sample.name)),
        }
        match pack::unpack_canonical(&bytes, sample.values.len()) {
            Ok(back) => check.case(back == sample.values, || {
                format!("{}: 严格解码不等", sample.name)
            }),
            Err(error) => check.case(false, || format!("{}: 严格解码报错 {error}", sample.name)),
        }
    }
    report.finish(check);
}

fn pack_oracle(report: &mut Report, samples: &[Sample]) {
    let mut check = Check::new("pack.independent_oracle");
    for sample in samples {
        let bytes = pack::pack(&sample.values);
        check.case(bytes == oracle_pack(&sample.values), || {
            format!("{}: 独立打包字节不同", sample.name)
        });
        check.case(
            oracle_unpack(&bytes, sample.values.len()) == sample.values,
            || format!("{}: 独立解包不等", sample.name),
        );
        // 契约：0..=242 合法；未用的尾部槽位必须是 False。
        for &byte in &bytes {
            check.case(byte <= 242, || {
                format!("{}: 产出非法字节 {byte}", sample.name)
            });
        }
        let used = sample.values.len() % 5;
        if used != 0
            && let Some(&last) = bytes.last()
        {
            let mut rest = last as u32;
            for slot in 0..5 {
                if slot >= used {
                    check.case(rest.is_multiple_of(3), || {
                        format!("{}: 尾部槽位 {slot} 非 False", sample.name)
                    });
                }
                rest /= 3;
            }
        }
    }
    report.finish(check);
}

fn pack_errors(report: &mut Report) {
    let mut check = Check::new("pack.rejects_illegal");
    for byte in 243u8..=255 {
        check.case(pack::unpack(&[byte], 1).is_err(), || {
            format!("非法字节 {byte} 未拒绝")
        });
    }
    check.case(pack::unpack(&[], 1).is_err(), || {
        "空字节却要 1 个 trit".to_string()
    });
    check.case(pack::unpack(&[0, 0], 11).is_err(), || {
        "2 字节容量 10，要 11 个 trit 应报长度不足".to_string()
    });
    check.case(pack::unpack_canonical(&[0, 0], 5).is_err(), || {
        "严格路径要求 1 字节，给了 2 字节".to_string()
    });
    check.case(pack::unpack_canonical(&[3], 1).is_err(), || {
        "非规范尾部填充（第 2 槽位 Unknown）未拒绝".to_string()
    });
    check.case(pack::unpack_canonical(&[1], 1).is_ok(), || {
        "规范尾部填充被误拒".to_string()
    });
    report.finish(check);
}

// ---------------------------------------------------------------- balanced

fn pow3(n: u32) -> i128 {
    let mut value = 1i128;
    for _ in 0..n {
        value *= 3;
    }
    value
}

/// 独立解析：`+`=+1 / `0`=0 / `-`=-1，高位在前；不做溢出折叠。
pub(crate) fn oracle_balanced(text: &str) -> Option<i128> {
    let trimmed = text.trim_start_matches('0');
    if trimmed.is_empty() {
        return Some(0);
    }
    let mut value = 0i128;
    for ch in trimmed.chars() {
        let digit = match ch {
            '+' => 1i128,
            '0' => 0,
            '-' => -1,
            _ => return None,
        };
        value = value.checked_mul(3)?.checked_add(digit)?;
    }
    Some(value)
}

fn balanced_values() -> Vec<i64> {
    let mut values = vec![
        0,
        1,
        -1,
        2,
        -2,
        3,
        -3,
        4,
        -4,
        5,
        -5,
        9,
        -9,
        27,
        -27,
        81,
        -81,
        242,
        -242,
        243,
        -243,
        i64::MAX,
        i64::MIN,
        i64::MAX - 1,
        i64::MIN + 1,
    ];
    for n in 1..=40u32 {
        let bound = (pow3(n) - 1) / 2;
        if bound <= i64::MAX as i128 {
            let value = bound as i64;
            values.extend([value, -value, value - 1, 1 - value]);
        }
    }
    values
}

fn balanced_checks(report: &mut Report) {
    let mut check = Check::new("balanced.roundtrip");
    let mut oracle = Check::new("balanced.independent_oracle");
    for n in 0..=40u32 {
        let bound = (pow3(n) - 1) / 2;
        oracle.case(balanced::range(n) == bound, || {
            format!(
                "range({n}) = {} 与独立算的 {bound} 不符",
                balanced::range(n)
            )
        });
    }
    for value in balanced_values() {
        let text = balanced::encode(value);
        match balanced::decode(&text) {
            Ok(back) => check.case(back == value, || {
                format!("{value} 编成 {text} 却解回 {back}")
            }),
            Err(error) => check.case(false, || format!("{value} 编成 {text} 解码报错 {error}")),
        }
        oracle.case(oracle_balanced(&text) == Some(i128::from(value)), || {
            format!("{value} 编成 {text}，独立解析不同")
        });
    }
    // 明确拒绝的边界：空串、非法字符、42 位、前导零等价。
    check.case(balanced::decode("").is_err(), || "空串未拒绝".to_string());
    check.case(balanced::decode("2").is_err(), || {
        "非法字符未拒绝".to_string()
    });
    check.case(balanced::decode(&"+".repeat(42)).is_err(), || {
        "42 位应判溢出".to_string()
    });
    check.case(balanced::decode("00+0") == balanced::decode("+0"), || {
        "前导零与等价写法不一致".to_string()
    });
    report.finish(check);
    report.finish(oracle);
}

// ---------------------------------------------------------------- hamming

fn shift(value: Ternary, delta: u8) -> Ternary {
    Ternary::from_index((value.index() as u8 + delta).rem_euclid(3) as usize)
}

fn hamming_checks(report: &mut Report) {
    let mut check = Check::new("hamming.single_symbol_error");
    for &a in &TRITS {
        for &b in &TRITS {
            let word = hamming::encode(a, b);
            let clean = hamming::decode(word);
            check.case(clean.message == [a, b] && clean.corrected.is_none(), || {
                format!("({a:?},{b:?}) 无错却被改")
            });
            check.case(hamming::syndrome(&word) == (0, 0), || {
                format!("({a:?},{b:?}) 校验子非零")
            });
            for position in 0..word.len() {
                for delta in 1..=2u8 {
                    let mut bad = word;
                    bad[position] = shift(bad[position], delta);
                    let fixed = hamming::decode(bad);
                    check.case(
                        fixed.message == [a, b] && fixed.corrected == Some((position, delta)),
                        || {
                            format!(
                                "({a:?},{b:?}) 位置 {position} 错 +{delta}：解出 {:?} / {:?}",
                                fixed.message, fixed.corrected
                            )
                        },
                    );
                }
            }
        }
    }
    report.finish(check);
}

// ---------------------------------------------------------------- interleave

fn interleave_checks(report: &mut Report) {
    let mut check = Check::new("interleave.roundtrip");
    for block in 1..=8usize {
        for len in 1..=17usize {
            let values: Vec<Ternary> = (0..len)
                .map(|index| Ternary::from_index((index * 7 + block) % 3))
                .collect();
            match interleave::interleave(&values, block) {
                Ok(shuffled) => match interleave::deinterleave(&shuffled, block, len) {
                    Ok(back) => check.case(back == values, || {
                        format!("block={block} len={len}: 解交织不等")
                    }),
                    Err(error) => check.case(false, || {
                        format!("block={block} len={len}: 解交织报错 {error}")
                    }),
                },
                Err(error) => check.case(false, || {
                    format!("block={block} len={len}: 交织报错 {error}")
                }),
            }
        }
    }
    check.case(interleave::interleave(&TRITS, 0).is_err(), || {
        "block=0 未拒绝".to_string()
    });
    check.case(interleave::deinterleave(&TRITS, 2, 4).is_err(), || {
        "长度不符未拒绝".to_string()
    });
    report.finish(check);
}

// ---------------------------------------------------------------- rle / delta

fn rle_checks(report: &mut Report, samples: &[Sample]) {
    let mut check = Check::new("rle.roundtrip");
    let mut ratio = Check::new("rle.metrics_consistent");
    for sample in samples {
        let runs = rle::encode(&sample.values);
        match rle::decode(&runs, sample.values.len()) {
            Ok(back) => check.case(back == sample.values, || {
                format!("{}: 解码不等", sample.name)
            }),
            Err(error) => check.case(false, || format!("{}: 解码报错 {error}", sample.name)),
        }
        let len = sample.values.len();
        let expected = if len == 0 {
            0.0
        } else {
            1.0 - runs.len() as f64 / len as f64
        };
        ratio.case(
            (rle::compression_ratio(&runs, len) - expected).abs() < 1e-12,
            || format!("{}: 压缩比口径不一致", sample.name),
        );
        let average = if runs.is_empty() {
            0.0
        } else {
            len as f64 / runs.len() as f64
        };
        ratio.case(
            (rle::average_run_len(&runs) - average).abs() < 1e-12,
            || format!("{}: 平均游程口径不一致", sample.name),
        );
        // 游程计数必须都非零、且总长等于原长。
        let total: u32 = runs.iter().map(|(_, count)| *count).sum();
        check.case(total as usize == len, || {
            format!("{}: 游程总长不符", sample.name)
        });
    }
    check.case(rle::decode(&[(Ternary::False, 0)], 0).is_err(), || {
        "零长度游程未拒绝".to_string()
    });
    report.finish(check);
    report.finish(ratio);
}

fn oracle_delta_encode(values: &[Ternary]) -> Vec<Ternary> {
    let mut out = Vec::new();
    for (index, &value) in values.iter().enumerate() {
        let encoded = if index == 0 {
            value
        } else {
            let diff = (value.index() as i8 - values[index - 1].index() as i8).rem_euclid(3);
            Ternary::from_index(diff as usize)
        };
        out.push(encoded);
    }
    out
}

fn oracle_delta_decode(deltas: &[Ternary]) -> Vec<Ternary> {
    let mut out = Vec::new();
    let mut acc = 0usize;
    for (index, &value) in deltas.iter().enumerate() {
        if index == 0 {
            acc = value.index();
            out.push(value);
        } else {
            acc = (acc + value.index()) % 3;
            out.push(Ternary::from_index(acc));
        }
    }
    out
}

fn delta_checks(report: &mut Report, samples: &[Sample]) {
    let mut check = Check::new("delta.roundtrip");
    let mut oracle = Check::new("delta.independent_oracle");
    for sample in samples {
        let deltas = delta::encode(&sample.values);
        check.case(delta::decode(&deltas) == sample.values, || {
            format!("{}: 解码不等", sample.name)
        });
        oracle.case(deltas == oracle_delta_encode(&sample.values), || {
            format!("{}: 独立编码不同", sample.name)
        });
        oracle.case(oracle_delta_decode(&deltas) == sample.values, || {
            format!("{}: 独立解码不等", sample.name)
        });
    }
    report.finish(check);
    report.finish(oracle);
}

// ---------------------------------------------------------------- gray

fn gray_checks(report: &mut Report) {
    let mut check = Check::new("gray.bijection");
    let mut adjacent = Check::new("gray.adjacent_distance_one");
    for n in 0..=8u32 {
        let size = 3usize.pow(n);
        for index in 0..size {
            let word = gray::encode(index, n as usize).expect("index 在 3^n 内");
            check.case(gray::decode(&word) == index, || {
                format!("n={n} index={index}: 解回 {}", gray::decode(&word))
            });
            if index > 0 {
                let previous = gray::encode(index - 1, n as usize).expect("index 在 3^n 内");
                let distance = previous
                    .iter()
                    .zip(&word)
                    .filter(|(left, right)| left != right)
                    .count();
                adjacent.case(distance == 1, || {
                    format!("n={n} index={index}: 汉明距离 {distance}")
                });
            }
        }
    }
    check.case(gray::encode(3, 1).is_err(), || {
        "index=3, n=1 应越界".to_string()
    });
    check.case(gray::encode(0, 0).is_ok(), || {
        "n=0 index=0 应合法".to_string()
    });
    check.case(gray::encode(1, 0).is_err(), || {
        "n=0 index=1 应越界".to_string()
    });
    report.finish(check);
    report.finish(adjacent);
}

// ---------------------------------------------------------------- sparse

fn sparse_checks(report: &mut Report, samples: &[Sample]) {
    let mut check = Check::new("sparse.roundtrip");
    let mut oracle = Check::new("sparse.independent_oracle");
    for sample in samples {
        let entries = sparse::encode(&sample.values);
        match sparse::decode(&entries, sample.values.len()) {
            Ok(back) => check.case(back == sample.values, || {
                format!("{}: 解码不等", sample.name)
            }),
            Err(error) => check.case(false, || format!("{}: 解码报错 {error}", sample.name)),
        }
        let known = sample
            .values
            .iter()
            .filter(|value| value.is_known())
            .count();
        check.case(entries.len() == known, || {
            format!(
                "{}: 条目数 {} != 已知值 {known}",
                sample.name,
                entries.len()
            )
        });
        let sorted = entries.windows(2).all(|pair| pair[0].0 < pair[1].0);
        check.case(sorted, || format!("{}: 下标非严格递增", sample.name));
        check.case(entries.iter().all(|(_, value)| value.is_known()), || {
            format!("{}: 条目里混进了 Unknown", sample.name)
        });
        // 独立重建：未列出的位置一律 Unknown。
        let mut rebuilt = vec![Ternary::Unknown; sample.values.len()];
        for &(index, value) in &entries {
            rebuilt[index as usize] = value;
        }
        oracle.case(rebuilt == sample.values, || {
            format!("{}: 独立重建不等", sample.name)
        });
    }
    check.case(
        sparse::decode(&[(1, Ternary::True), (1, Ternary::False)], 3).is_err(),
        || "重复下标未拒绝".to_string(),
    );
    check.case(
        sparse::decode(&[(2, Ternary::True), (1, Ternary::False)], 3).is_err(),
        || "逆序下标未拒绝".to_string(),
    );
    check.case(sparse::decode(&[(3, Ternary::True)], 3).is_err(), || {
        "越界下标未拒绝".to_string()
    });
    report.finish(check);
    report.finish(oracle);
}

// ---------------------------------------------------------------- checksum

fn checksum_checks(report: &mut Report, samples: &[Sample]) {
    let mut check = Check::new("checksum.detects_single_symbol");
    for sample in samples.iter().filter(|sample| sample.values.len() <= 300) {
        let codeword = checksum::append(&sample.values);
        check.case(checksum::verify(&codeword), || {
            format!("{}: 干净码字被拒", sample.name)
        });
        check.case(
            codeword.len() == sample.values.len() + checksum::CHECKSUM_LEN,
            || format!("{}: 校验位长度不符", sample.name),
        );
        for position in 0..codeword.len() {
            for delta in 1..=2u8 {
                let mut bad = codeword.clone();
                bad[position] = shift(bad[position], delta);
                check.case(!checksum::verify(&bad), || {
                    format!("{}: 位置 {position} +{delta} 未检出", sample.name)
                });
            }
        }
    }
    // 大样本抽 16 个位置，避免 O(len²) 打满。
    for sample in samples.iter().filter(|sample| sample.values.len() > 300) {
        let codeword = checksum::append(&sample.values);
        check.case(checksum::verify(&codeword), || {
            format!("{}: 干净码字被拒", sample.name)
        });
        for spot in 0..16usize {
            let position = (spot * 97 + 13) % codeword.len();
            for delta in 1..=2u8 {
                let mut bad = codeword.clone();
                bad[position] = shift(bad[position], delta);
                check.case(!checksum::verify(&bad), || {
                    format!("{}: 抽样位置 {position} +{delta} 未检出", sample.name)
                });
            }
        }
    }
    check.case(!checksum::verify(&[]), || "空码字被当作合法".to_string());
    check.case(!checksum::verify(&[Ternary::False, Ternary::False]), || {
        "短码字被当作合法".to_string()
    });
    report.finish(check);
}

// ---------------------------------------------------------------- plan

fn plan_checks(report: &mut Report, samples: &[Sample]) {
    let mut check = Check::new("plan.roundtrip");
    for sample in samples {
        let (encoding, bytes) = plan::encode(&sample.values);
        match plan::decode(encoding, &bytes, sample.values.len()) {
            Ok(back) => check.case(back == sample.values, || {
                format!("{}: {encoding:?} 解码不等", sample.name)
            }),
            Err(error) => check.case(false, || {
                format!("{}: {encoding:?} 解码报错 {error}", sample.name)
            }),
        }
        match plan::View::new(encoding, &bytes, sample.values.len()) {
            Ok(view) => {
                let collected: Vec<Ternary> = view.iter().collect();
                check.case(collected == sample.values, || {
                    format!("{}: View 顺序不对", sample.name)
                });
                if !sample.values.is_empty() {
                    check.case(view.get(0) == Some(sample.values[0]), || {
                        format!("{}: View[0] 不对", sample.name)
                    });
                    let last = sample.values.len() - 1;
                    check.case(view.get(last) == Some(sample.values[last]), || {
                        format!("{}: View[last] 不对", sample.name)
                    });
                }
            }
            Err(error) => check.case(false, || format!("{}: View 构造报错 {error}", sample.name)),
        }
        let sizes = plan::sizes(&sample.values);
        check.case(sizes.iter().any(Option::is_some), || {
            format!("{}: 没有任何可用编码", sample.name)
        });
    }
    report.finish(check);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn oracle_pack_matches_known_wire_vectors() {
        assert_eq!(oracle_pack(&[Ternary::False; 5]), vec![0]);
        assert_eq!(oracle_pack(&[Ternary::True; 5]), vec![242]);
        assert_eq!(oracle_pack(&[Ternary::Unknown; 5]), vec![121]);
        assert_eq!(oracle_pack(&[Ternary::True]), vec![2]);
        assert_eq!(oracle_pack(&[]), Vec::<u8>::new());
    }

    #[test]
    fn oracle_roundtrip_is_self_consistent() {
        let values = [
            Ternary::True,
            Ternary::Unknown,
            Ternary::False,
            Ternary::False,
            Ternary::Unknown,
            Ternary::True,
            Ternary::False,
        ];
        let bytes = oracle_pack(&values);
        assert_eq!(bytes.len(), 2);
        assert_eq!(oracle_unpack(&bytes, values.len()), values.to_vec());
    }

    #[test]
    fn oracle_balanced_reads_signs_by_position() {
        assert_eq!(oracle_balanced("+0"), Some(3));
        assert_eq!(oracle_balanced("-0"), Some(-3));
        assert_eq!(oracle_balanced("0"), Some(0));
        assert_eq!(oracle_balanced("00-"), Some(-1));
        assert_eq!(oracle_balanced("2"), None);
        assert_eq!(oracle_balanced(""), Some(0));
    }

    #[test]
    fn oracle_delta_inverts_mod_three() {
        let values = [
            Ternary::True,
            Ternary::False,
            Ternary::False,
            Ternary::Unknown,
            Ternary::True,
        ];
        let deltas = oracle_delta_encode(&values);
        assert_eq!(oracle_delta_decode(&deltas), values.to_vec());
    }
}
