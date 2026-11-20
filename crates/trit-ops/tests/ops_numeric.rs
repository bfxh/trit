//! 三值算子库的数值对拍：每一个快路径都必须与标量参考逐点相等（或给出误差界）。
//!
//! 这里的断言不是"跑通了"，而是"和参考实现给了同一个数"。三值算术的全部价值是
//! **乘法消失但结果不变**；只要有一格对不上，整个"无乘法"的前提就不成立。

use trit_ops::bench::{gemv_f32, pack_q4, ternary_weights};
use trit_ops::gemv::{Kernel, TernaryMatrix, TryteMatrix};
use trit_ops::pack::{Rail, TRITS_PER_BYTE, TRITS_PER_WORD, Trytes};
use trit_ops::quant::{Scheme, dequantize, quantize, quantize_with_report, scale_for};
use trit_ops::trit::Trit;

const ALL: [Trit; 3] = [Trit::NEG, Trit::ZERO, Trit::POS];

fn sample(n: usize, seed: u64) -> Vec<Trit> {
    let mut state = seed | 1;
    (0..n)
        .map(|_| {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            ALL[(state % 3) as usize]
        })
        .collect()
}

fn as_i8(trits: &[Trit]) -> Vec<i8> {
    trits.iter().map(|t| t.get()).collect()
}

// ---------- Trit 标量代数 ----------

#[test]
fn trit_multiplication_is_the_full_truth_table() {
    // 9 格全表：三值乘法的全部内容就在这里，一格错就是"无乘法"是假的。
    for a in ALL {
        for b in ALL {
            assert_eq!(
                a.get() * b.get(),
                a.mul(b).get(),
                "ORCH_OPS_MUL a={a:?} b={b:?}"
            );
        }
    }
}

#[test]
fn trit_addition_saturates_instead_of_carrying() {
    // -1 + -1 = -2 在平衡三值里必须截到 -1；若回绕成 +1，点积会整体反号。
    assert_eq!(Trit::NEG.add_sat(Trit::NEG).get(), -1, "ORCH_OPS_ADD_SAT");
    assert_eq!(Trit::POS.add_sat(Trit::POS).get(), 1);
    assert_eq!(Trit::POS.add_sat(Trit::NEG).get(), 0);
    for a in ALL {
        for b in ALL {
            let raw = a.get() + b.get();
            let expected = raw.clamp(-1, 1);
            assert_eq!(a.add_sat(b).get(), expected, "a={a:?} b={b:?}");
        }
    }
}

#[test]
fn kleene_connectives_are_min_and_max_over_the_nine_cells() {
    for a in ALL {
        for b in ALL {
            assert_eq!(a.and(b).get(), a.get().min(b.get()), "ORCH_OPS_KLEENE_AND");
            assert_eq!(a.or(b).get(), a.get().max(b.get()), "ORCH_OPS_KLEENE_OR");
            assert_eq!(a.and(b), b.and(a));
            assert_eq!(a.or(b), b.or(a));
        }
    }
    for a in ALL {
        for b in ALL {
            for c in ALL {
                assert_eq!(a.and(b).and(c), a.and(b.and(c)), "结合律");
                assert_eq!(a.or(b).or(c), a.or(b.or(c)), "结合律");
                assert_eq!(a.and(b.or(c)), a.and(b).or(a.and(c)), "分配律");
                assert_eq!(a.or(a.and(b)), a, "吸收律");
            }
        }
    }
}

#[test]
fn lukasiewicz_connectives_stay_inside_the_three_values() {
    // Ł 合取 = max(-1, a+b-1)，Ł 析取 = min(1, a+b+1)：表里只有 -1/0/+1。
    for a in ALL {
        for b in ALL {
            let and_raw = a.get() + b.get() - 1;
            let or_raw = a.get() + b.get() + 1;
            assert_eq!(a.luk_and(b).get(), and_raw.clamp(-1, 1), "ORCH_OPS_LUK_AND");
            assert_eq!(a.luk_or(b).get(), or_raw.clamp(-1, 1), "ORCH_OPS_LUK_OR");
        }
    }
    // 与 Kleene 的关键差别：Ł 下 (-1)∧(-1)=-1 一致，但 (+1)∧(0)=0，(+1)∧(-1)=-1。
    assert_eq!(Trit::POS.luk_and(Trit::ZERO).get(), 0);
    assert_eq!(Trit::POS.luk_and(Trit::NEG).get(), -1);
}

#[test]
fn sign_of_maps_real_values_and_clamps_zero() {
    assert_eq!(Trit::sign_of(0.5).get(), 1, "ORCH_OPS_SIGN");
    assert_eq!(Trit::sign_of(-0.5).get(), -1);
    assert_eq!(Trit::sign_of(0.0).get(), 0);
    assert_eq!(
        Trit::sign_of(f32::NAN).get(),
        0,
        "NaN 不落入任一方向，必须是 0"
    );
    assert_eq!(Trit::sign_of(f32::INFINITY).get(), 1);
}

#[test]
fn trit_and_ternary_roundtrip_both_ways() {
    use trit_core::value::Ternary;
    for t in [Ternary::False, Ternary::Unknown, Ternary::True] {
        assert_eq!(
            Trit::from_ternary(t).to_ternary(),
            t,
            "ORCH_OPS_TERNARY_ROUNDTRIP"
        );
    }
    for trit in ALL {
        assert_eq!(Trit::from_ternary(trit.to_ternary()), trit);
    }
}

// ---------- 双导轨位平面 ----------

#[test]
fn rail_roundtrips_at_every_length_up_to_two_words() {
    for n in 0..=130usize {
        let v = sample(n, 31 + n as u64);
        let rail = Rail::from_trits(&v);
        assert_eq!(rail.len(), n);
        assert_eq!(rail.to_trits(), v, "ORCH_OPS_RAIL_ROUNDTRIP len={n}");
        assert_eq!(rail.words(), n.div_ceil(TRITS_PER_WORD).max(1));
        assert_eq!(rail.bytes(), rail.words() * 16);
    }
}

#[test]
fn rail_dot_equals_scalar_dot_at_all_lengths() {
    for n in [1usize, 2, 5, 63, 64, 65, 127, 128, 129] {
        let a = sample(n, 7);
        let b = sample(n, 11);
        let ra = Rail::from_trits(&a);
        let rb = Rail::from_trits(&b);
        assert_eq!(ra.dot(&rb), ra.dot_scalar(&rb), "ORCH_OPS_RAIL_DOT len={n}");
        // 期望值再按定义算一遍：Σ a_i * b_i
        let expected: i64 = as_i8(&a)
            .iter()
            .zip(as_i8(&b).iter())
            .map(|(x, y)| (*x as i64) * (*y as i64))
            .sum();
        assert_eq!(ra.dot(&rb), expected, "len={n}");
    }
}

#[test]
fn rail_dot_is_exhaustively_right_at_length_two() {
    // 3^2 个向量，两两组合 81 对——全部可穷举，"4 组 AND+popcount"没有暗格。
    let vectors: Vec<Vec<Trit>> = {
        let mut out = Vec::new();
        for a in ALL {
            for b in ALL {
                out.push(vec![a, b]);
            }
        }
        out
    };
    for left in &vectors {
        for right in &vectors {
            let ra = Rail::from_trits(left);
            let rb = Rail::from_trits(right);
            let expected: i64 = as_i8(left)
                .iter()
                .zip(as_i8(right).iter())
                .map(|(x, y)| (*x as i64) * (*y as i64))
                .sum();
            assert_eq!(
                ra.dot(&rb),
                expected,
                "ORCH_OPS_RAIL_DOT_81 {left:?}·{right:?}"
            );
        }
    }
}

#[test]
fn rail_rejects_planes_that_would_double_count() {
    // 同一个 bit 同时置在 pos 与 neg：读出来会被当成 ZERO，静默丢掉一个 trit。
    assert!(
        Rail::from_planes(vec![1], vec![1], 1).is_none(),
        "ORCH_OPS_RAIL_OVERLAP"
    );
    // 尾部（超出 len 的位）必须为 0，否则 nnz 会数到不存在的 trit。
    assert!(
        Rail::from_planes(vec![1u64 << 63], vec![0], 1).is_none(),
        "ORCH_OPS_RAIL_DIRTY_TAIL"
    );
    // 字数必须与 len 匹配。
    assert!(
        Rail::from_planes(vec![0, 0], vec![0, 0], 1).is_none(),
        "ORCH_OPS_RAIL_WORDS"
    );
    assert!(
        Rail::from_planes(vec![0], vec![0, 0], 1).is_none(),
        "两导轨长度必须一致"
    );
    assert!(Rail::from_planes(vec![0], vec![0], 1).is_some());
}

#[test]
fn rail_metrics_count_zeros_not_bits() {
    let v = [Trit::ZERO, Trit::POS, Trit::ZERO, Trit::NEG, Trit::ZERO];
    let rail = Rail::from_trits(&v);
    assert_eq!(rail.nnz(), 2, "ORCH_OPS_RAIL_NNZ");
    assert!((rail.sparsity() - 0.6).abs() < 1e-12);
    assert!(!rail.is_empty());
    let empty = Rail::from_trits(&[]);
    assert!(empty.is_empty());
    assert_eq!(empty.sparsity(), 0.0, "空向量的稀疏度定义为 0，不是 NaN");
}

// ---------- tryte 打包 ----------

#[test]
fn trytes_roundtrip_at_every_length_up_to_twelve_bytes() {
    for n in 0..=60usize {
        let v = sample(n, 47 + n as u64);
        let t = Trytes::from_trits(&v);
        assert_eq!(t.len(), n);
        assert_eq!(
            t.bytes(),
            n.div_ceil(TRITS_PER_BYTE),
            "ORCH_OPS_TRYTE_BYTES len={n}"
        );
        assert_eq!(t.to_trits(), v, "ORCH_OPS_TRYTE_ROUNDTRIP len={n}");
    }
}

#[test]
fn tryte_codes_span_exactly_zero_to_two_hundred_forty_two() {
    let lo = Trytes::from_trits(&[Trit::NEG; 5]);
    let hi = Trytes::from_trits(&[Trit::POS; 5]);
    assert_eq!(lo.codes()[0], 0, "ORCH_OPS_TRYTE_LO");
    assert_eq!(hi.codes()[0], 242, "ORCH_OPS_TRYTE_HI");
    let zeros = Trytes::from_trits(&[Trit::ZERO; 5]);
    // 0 -> 1，所以 [0,0,0,0,0] = 1+3+9+27+81 = 121。
    assert_eq!(zeros.codes()[0], 121, "ORCH_OPS_TRYTE_MID");
    assert!(
        Trytes::from_codes(vec![243], 5).is_none(),
        "243 不是合法编码"
    );
    assert!(
        Trytes::from_codes(vec![0u8; 3], 5).is_none(),
        "字节数必须等于 ceil(len/5)"
    );
}

#[test]
fn tryte_encoding_is_positional_base_three() {
    // 只有第 k 位是 +1、其余全 -1 时，编码应恰好是 3^k。
    for k in 0..TRITS_PER_BYTE {
        let mut v = [Trit::NEG; TRITS_PER_BYTE];
        v[k] = Trit::POS;
        let code = Trytes::from_trits(&v).codes()[0] as usize;
        // 第 k 位 +1、其余 -1：每位贡献 (trit+1)*3^i，所以只有 k 位贡献 2*3^k。
        assert_eq!(code, 2 * 3usize.pow(k as u32), "ORCH_OPS_TRYTE_POS k={k}");
    }
}

// ---------- 双导轨矩阵 ----------

#[test]
fn matrix_rows_roundtrip_and_shape_is_exact() {
    for (rows, cols) in [(0usize, 0usize), (1, 1), (3, 5), (4, 64), (2, 65), (5, 130)] {
        let data: Vec<Vec<Trit>> = (0..rows).map(|r| sample(cols, 101 + r as u64)).collect();
        let m = TernaryMatrix::from_rows(&data, cols);
        assert_eq!(m.rows(), rows, "ORCH_OPS_MATRIX_ROWS");
        assert_eq!(m.cols(), cols);
        assert_eq!(m.blocks_per_row(), cols.div_ceil(TRITS_PER_WORD).max(1));
        assert_eq!(m.bytes(), rows * m.blocks_per_row() * 16);
        for (index, row) in data.iter().enumerate() {
            assert_eq!(&m.row_trits(index), row, "row {index} 往返失败");
        }
    }
}

#[test]
fn from_fn_and_from_rows_agree() {
    let rows = 4;
    let cols = 67;
    let data: Vec<Vec<Trit>> = (0..rows).map(|r| sample(cols, 5 + r as u64)).collect();
    let from_rows = TernaryMatrix::from_rows(&data, cols);
    let from_fn = TernaryMatrix::from_fn(rows, cols, |r, c| data[r][c]);
    for r in 0..rows {
        assert_eq!(from_rows.row_trits(r), from_fn.row_trits(r));
    }
}

#[test]
fn every_kernel_agrees_with_the_scalar_reference() {
    // 快路径（掩码 / 查表）与标量参考必须给同一个数；差异只允许来自浮点求和顺序。
    let rows = 9;
    let cols = 65;
    let data: Vec<Vec<Trit>> = (0..rows).map(|r| sample(cols, 13 + r as u64)).collect();
    let x: Vec<f32> = (0..cols).map(|i| (i as f32 * 0.37).sin()).collect();
    let m = TernaryMatrix::from_rows(&data, cols);

    let mut reference = vec![0.0f32; rows];
    m.gemv(&x, &mut reference, Kernel::Scalar);
    // 参考值再用纯 i8 定义算一遍，确认标量内核本身没跑偏。
    for (r, value) in reference.iter().enumerate() {
        let expected: f32 = as_i8(&data[r])
            .iter()
            .zip(x.iter())
            .map(|(w, xv)| *w as f32 * xv)
            .sum();
        assert!(
            (value - expected).abs() < 1e-4,
            "ORCH_OPS_GEMV_SCALAR row={r}"
        );
    }

    for kernel in [Kernel::Mask, Kernel::Auto] {
        let mut got = vec![0.0f32; rows];
        m.gemv(&x, &mut got, kernel);
        for (index, (value, expected)) in got.iter().zip(reference.iter()).enumerate() {
            let tolerance = 1e-3 * (1.0 + expected.abs());
            assert!(
                (value - expected).abs() <= tolerance,
                "ORCH_OPS_GEMV_KERNEL {kernel:?} row={index}: {value} vs {expected}"
            );
        }
    }
}

#[test]
fn row_dot_matches_the_full_gemv_per_row() {
    let rows = 4;
    let cols = 130;
    let data: Vec<Vec<Trit>> = (0..rows).map(|r| sample(cols, 71 + r as u64)).collect();
    let m = TernaryMatrix::from_rows(&data, cols);
    let pad = m.blocks_per_row() * TRITS_PER_WORD;
    let mut x = vec![0.0f32; pad];
    for (i, slot) in x.iter_mut().take(cols).enumerate() {
        *slot = (i as f32 * 0.11).cos();
    }
    let mut full = vec![0.0f32; rows];
    m.gemv(&x[..cols], &mut full, Kernel::Scalar);
    for (index, expected) in full.iter().enumerate() {
        assert!(
            (m.row_dot(index, &x) - expected).abs() < 1e-3,
            "ORCH_OPS_ROW_DOT row={index}"
        );
    }
}

// ---------- tryte 矩阵 ----------

#[test]
fn tryte_matrix_matches_rail_matrix_on_all_paths() {
    let rows = 6;
    let cols = 61;
    let data: Vec<Vec<Trit>> = (0..rows).map(|r| sample(cols, 211 + r as u64)).collect();
    let rail = TernaryMatrix::from_rows(&data, cols);
    let tryte = TryteMatrix::from_rail(&rail);
    let x: Vec<f32> = (0..cols).map(|i| ((i * 17) % 23) as f32 - 11.0).collect();

    let mut reference = vec![0.0f32; rows];
    rail.gemv(&x, &mut reference, Kernel::Scalar);

    let mut got = vec![0.0f32; rows];
    tryte.gemv(&x, &mut got);
    for r in 0..rows {
        let tolerance = 1e-3 * (1.0 + reference[r].abs());
        assert!(
            (got[r] - reference[r]).abs() <= tolerance,
            "ORCH_OPS_TRYTE_GEMV row={r}: {} vs {}",
            got[r],
            reference[r]
        );
    }
}

#[test]
fn prebuilt_and_split_tables_give_the_same_answer() {
    let rows = 5;
    let cols = 23;
    let data: Vec<Vec<Trit>> = (0..rows).map(|r| sample(cols, 301 + r as u64)).collect();
    let rail = TernaryMatrix::from_rows(&data, cols);
    let tryte = TryteMatrix::from_rail(&rail);
    let x: Vec<f32> = (0..cols).map(|i| (i as f32).sqrt()).collect();

    let mut whole = vec![0.0f32; rows];
    tryte.gemv(&x, &mut whole);

    let tables = tryte.build_tables(&x);
    assert_eq!(
        tables.len() % 243,
        0,
        "表长必须是 243 的倍数：{}",
        tables.len()
    );
    let mut from_tables = vec![0.0f32; rows];
    tryte.gemv_with_tables(&tables, &mut from_tables);

    let split = tryte.build_tables_split(&x);
    assert_eq!(split.len() % 36, 0, "拆表每字节 9+27 项：{}", split.len());
    let mut from_split = vec![0.0f32; rows];
    tryte.gemv_with_split_tables(&split, &mut from_split);

    let mut mapped = vec![0.0f32; rows];
    tryte.gemv_with_split_tables_map(&split, &mut mapped);

    for r in 0..rows {
        assert!(
            (from_tables[r] - whole[r]).abs() < 1e-3,
            "ORCH_OPS_TABLE_BUILD row={r}"
        );
        assert!(
            (from_split[r] - whole[r]).abs() < 1e-3,
            "ORCH_OPS_SPLIT row={r}"
        );
        assert!(
            (mapped[r] - from_split[r]).abs() < 1e-6,
            "映射只是省掉 %9//9，结果必须逐位一致"
        );
    }
}

// ---------- 量化 ----------

#[test]
fn every_scheme_returns_a_positive_finite_scale() {
    let values = [0.0f32, 0.0, 0.0];
    for scheme in [Scheme::AbsMean, Scheme::AbsMax, Scheme::Optimal] {
        let scale = scale_for(&values, scheme);
        assert!(
            scale.is_finite() && scale > 0.0,
            "ORCH_OPS_SCALE_DEGENERATE {scheme:?} -> {scale}"
        );
    }
    assert!(scale_for(&[], Scheme::AbsMean) > 0.0, "空输入也要给回退值");
}

#[test]
fn abs_max_scale_is_the_magnitude_bound_not_the_signed_max() {
    // 按有符号最大值取会让负侧饱和，量化整体不对称。
    let values = [0.4f32, -2.0, 1.0, 0.0];
    assert_eq!(scale_for(&values, Scheme::AbsMax), 2.0, "ORCH_OPS_ABSMAX");
    let q = quantize(&values, 2.0);
    assert_eq!(as_i8(&q), vec![0, -1, 1, 0]);
}

#[test]
fn quantize_clamps_instead_of_overflowing() {
    let q = quantize(&[0.9f32, -0.9, 0.01, 0.0, 5.0], 0.5);
    assert_eq!(as_i8(&q), vec![1, -1, 0, 0, 1], "ORCH_OPS_QUANTIZE_CLAMP");
    for t in q {
        assert!((-1..=1).contains(&t.get()));
    }
}

#[test]
fn dequantize_inverts_quantize_within_one_scale() {
    let values = [0.7f32, -1.3, 0.0, 2.4, -0.05];
    let scale = scale_for(&values, Scheme::AbsMax);
    let q = quantize(&values, scale);
    let back = dequantize(&q, scale);
    for (original, recovered) in values.iter().zip(back.iter()) {
        // 三电平量化的最大误差就是一个 scale 的一半再加余量。
        assert!(
            (original - recovered).abs() <= scale * 0.5 + 1e-6,
            "ORCH_OPS_DEQUANT {original} -> {recovered} (scale={scale})"
        );
    }
}

#[test]
fn report_matches_a_manual_recomputation() {
    let values: Vec<f32> = (0..64).map(|i| ((i % 13) as f32 - 6.0) * 0.3).collect();
    let (trits, scale, report) = quantize_with_report(&values, Scheme::AbsMean);
    assert_eq!(trits.len(), values.len());
    let mut err = 0.0f64;
    let mut norm = 0.0f64;
    let mut zeros = 0usize;
    for (v, t) in values.iter().zip(trits.iter()) {
        let d = (*v - t.get() as f32 * scale) as f64;
        err += d * d;
        norm += (*v as f64) * (*v as f64);
        if t.get() == 0 {
            zeros += 1;
        }
    }
    let expected = (err / norm).sqrt();
    assert!(
        (report.rel_err - expected).abs() < 1e-12,
        "ORCH_OPS_REPORT_REL_ERR"
    );
    assert!((report.zero_ratio - zeros as f64 / values.len() as f64).abs() < 1e-12);
    assert!(report.rel_err < 1.0, "相对误差超过 1 说明量化把信号全丢了");
}

#[test]
fn empty_input_report_is_zero_not_nan() {
    let (trits, scale, report) = quantize_with_report(&[], Scheme::Optimal);
    assert!(trits.is_empty());
    assert!(scale > 0.0);
    assert_eq!(report.rel_err, 0.0, "空输入的相对误差定义为 0，不能是 NaN");
    assert_eq!(report.zero_ratio, 0.0);
}

// ---------- 参考基线（f32 / q4） ----------

#[test]
fn f32_gemv_matches_a_naive_triple_loop() {
    let rows = 5;
    let cols = 37;
    let weights: Vec<f32> = (0..rows * cols).map(|i| (i as f32 * 0.13).sin()).collect();
    let x: Vec<f32> = (0..cols).map(|i| (i as f32 * 0.29).cos()).collect();
    let mut out = vec![0.0f32; rows];
    gemv_f32(&weights, rows, cols, &x, &mut out);
    for r in 0..rows {
        let expected: f32 = (0..cols).map(|c| weights[r * cols + c] * x[c]).sum();
        assert!(
            (out[r] - expected).abs() < 1e-3,
            "ORCH_OPS_BENCH_F32 row={r}"
        );
    }
}

#[test]
fn q4_packing_emits_eighteen_bytes_per_block() {
    let weights: Vec<f32> = (0..96).map(|i| ((i % 17) as f32 - 8.0) * 0.25).collect();
    let packed = pack_q4(&weights);
    assert_eq!(packed.len(), 3 * 18, "ORCH_OPS_PACK_Q4 96 个数应打成 3 块");
    // 全零块：scale 为 0，nibble 全 8（表示 0）。
    let zeros = pack_q4(&[0.0f32; 32]);
    assert_eq!(zeros.len(), 18);
    assert_eq!(&zeros[2..], &[0x88u8; 16], "全零块的 nibble 必须都是 8");
}

#[test]
fn deterministic_weights_reproduce_and_stay_ternary() {
    let first = ternary_weights(3, 9, 42);
    let second = ternary_weights(3, 9, 42);
    assert_eq!(first, second, "ORCH_OPS_WEIGHTS_DETERMINISTIC");
    // seed 先 `| 1`：偶数 seed 与它 +1 得到同一状态。这是既有的确定性口径，
    // 固化下来，免得有人以为"换个 seed 一定换一组权重"。
    let same = ternary_weights(3, 9, 43);
    assert_eq!(first, same, "ORCH_OPS_WEIGHTS_SEED_ODD_FOLD");
    let other = ternary_weights(3, 9, 44);
    assert_ne!(first, other, "相隔一个奇偶的 seed 必须给不同权重");
    for row in &first {
        assert_eq!(row.len(), 9);
        for t in row {
            assert!((-1..=1).contains(&t.get()));
        }
    }
}
