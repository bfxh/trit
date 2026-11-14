//! 分组判等证书：L0–L4 分级、反例落在 `inconclusive`、充分性自检、成本上限。

use std::time::Instant;

use trit_wspace::grouped::GroupedLayer;
use trit_wspace::grouped_certify::{MAX_VARS, Witness};

fn layer(
    rows: usize,
    cols: usize,
    trits: &[i8],
    groups: &[u32],
    scales: &[f32],
    bias: &[f32],
) -> GroupedLayer {
    GroupedLayer::from_trits(rows, cols, trits, groups, scales, bias).expect("layer")
}

/// 2×2、每行自成一组，trits `[1, -1, 1, 1]`。
fn row_grouped(scales: [f32; 2]) -> GroupedLayer {
    layer(2, 2, &[1, -1, 1, 1], &[0, 0, 1, 1], &scales, &[0.0, 0.0])
}

#[test]
fn identical_layer_is_l0() {
    let a = row_grouped([1.0, 2.0]);
    let certificate = a.certify_equivalence(&a, 2).expect("certify");
    assert_eq!(certificate.witness, Witness::Identical);
    assert!(certificate.equivalent());
    assert_eq!(certificate.implies(), "all_inputs");
}

#[test]
fn global_scale_is_l1() {
    let a = row_grouped([1.0, 2.0]);
    let b = row_grouped([3.0, 6.0]);
    let certificate = a.certify_equivalence(&b, 2).expect("certify");
    assert_eq!(certificate.witness, Witness::Canonical);
    let ratio = certificate.scale_ratio.expect("ratio");
    assert!((ratio - 3.0).abs() < 1e-6, "比例应为 3，实际 {ratio}");
    assert_eq!(certificate.implies(), "all_inputs");
}

#[test]
fn per_row_scale_is_l2() {
    let a = row_grouped([1.0, 1.0]);
    let b = row_grouped([2.0, 5.0]);
    let certificate = a.certify_equivalence(&b, 2).expect("certify");
    assert_eq!(certificate.witness, Witness::RowProportional);
    assert_eq!(certificate.row_ratios.len(), 2);
    assert!((certificate.row_ratios[0] - 2.0).abs() < 1e-6);
    assert!((certificate.row_ratios[1] - 5.0).abs() < 1e-6);
    assert_eq!(certificate.implies(), "all_inputs");
}

#[test]
fn bounded_domain_is_l3() {
    // 第三列不在 `vars = 2` 的行为域里（输入恒为 0），所以改了它：
    // 有效权重不成比例（L2 失败），但域内行为相同（L3 命中）。
    let a = layer(
        2,
        3,
        &[1, 0, 0, 0, 1, 0],
        &[0, 0, 0, 1, 1, 1],
        &[1.0, 1.0],
        &[0.0, 0.0],
    );
    let b = layer(
        2,
        3,
        &[1, 0, 1, 0, 1, 0],
        &[0, 0, 0, 1, 1, 1],
        &[1.0, 1.0],
        &[0.0, 0.0],
    );
    assert!(
        a.row_proportional(&b).is_none(),
        "第三列不同 ⇒ 不应逐行成比例"
    );
    let certificate = a.certify_equivalence(&b, 2).expect("certify");
    assert_eq!(certificate.witness, Witness::Behavior);
    let proof = certificate.behavior.expect("behavior proof");
    assert_eq!((proof.vars, proof.samples, proof.rows), (2, 9, 2));
    assert_eq!(certificate.implies(), "bounded_trit_domain");
}

#[test]
fn scale_swap_is_inconclusive_not_different() {
    // `docs/WSP-SUMMARY.md` §7.1 的反例：同 trits、不同分组 ⇒ 行为不同。
    let a = layer(
        2,
        2,
        &[1, 1, 1, -1],
        &[0, 0, 1, 1],
        &[0.1, 10.0],
        &[0.0, 0.0],
    );
    let b = layer(
        2,
        2,
        &[1, 1, 1, -1],
        &[0, 1, 0, 1],
        &[0.1, 10.0],
        &[0.0, 0.0],
    );
    let certificate = a.certify_equivalence(&b, 2).expect("certify");
    assert_eq!(
        certificate.witness,
        Witness::Inconclusive,
        "没证出来只能写 inconclusive，不能冒充 different"
    );
    assert!(!certificate.equivalent());
    assert_eq!(certificate.implies(), "none");
}

#[test]
fn shape_mismatch_is_not_equivalent() {
    let a = row_grouped([1.0, 1.0]);
    let b = layer(
        2,
        3,
        &[1, -1, 0, 1, 1, 0],
        &[0, 0, 0, 1, 1, 1],
        &[1.0, 1.0],
        &[0.0, 0.0],
    );
    let certificate = a.certify_equivalence(&b, 2).expect("certify");
    assert_eq!(certificate.witness, Witness::ShapeMismatch);
    assert!(!certificate.equivalent());
}

#[test]
fn vars_beyond_limit_is_an_error() {
    let a = row_grouped([1.0, 1.0]);
    let error = a
        .certify_equivalence(&a, MAX_VARS + 1)
        .expect_err("超域必须报错");
    assert!(
        matches!(error, trit_wspace::Error::DomainTooLarge { .. }),
        "{error}"
    );
}

#[test]
fn row_proportional_implies_same_behavior() {
    // 充分性自检：凡是 L2 成立（逐行正比例），行为签名必须相同。
    // 一半样本由随机层逐行缩放生成（保证有成比例的），一半独立随机（大多不成比例）。
    let mut state = 0x9e37_79b9_7f4a_7c15u64;
    let mut next = || {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        state
    };
    let mut proportional = 0usize;
    for _ in 0..100 {
        let trits: Vec<i8> = (0..6).map(|_| (next() % 3) as i8 - 1).collect();
        let groups: Vec<u32> = (0..6).map(|_| (next() % 2) as u32).collect();
        let scales = [1.0f32 + (next() % 3) as f32, 1.0 + (next() % 3) as f32];
        let a = layer(2, 3, &trits, &groups, &scales, &[0.0, 0.0]);
        let b = if next() % 2 == 0 {
            // 逐行缩放：L2 必然成立
            let k = [1.0 + (next() % 4) as f32, 1.0 + (next() % 4) as f32];
            let scaled: Vec<f32> = scales
                .iter()
                .enumerate()
                .map(|(index, scale)| scale * k[index / 3])
                .collect();
            layer(2, 3, &trits, &groups, &scaled, &[0.0, 0.0])
        } else {
            let trits_b: Vec<i8> = (0..6).map(|_| (next() % 3) as i8 - 1).collect();
            let groups_b: Vec<u32> = (0..6).map(|_| (next() % 2) as u32).collect();
            let scales_b = [1.0f32 + (next() % 3) as f32, 1.0 + (next() % 3) as f32];
            layer(2, 3, &trits_b, &groups_b, &scales_b, &[0.0, 0.0])
        };
        if a.row_proportional(&b).is_some() {
            proportional += 1;
            assert!(
                a.equivalent(&b, 2).expect("equivalent"),
                "L2 成立但行为不同 ⇒ 充分条件被证伪"
            );
        }
    }
    assert!(proportional > 20, "样本里成比例的对太少（{proportional}）");
}

#[test]
fn certificate_json_records_scope_and_evidence() {
    let a = row_grouped([1.0, 1.0]);
    let b = row_grouped([2.0, 3.0]);
    let json = a.certify_equivalence(&b, 2).expect("certify").to_json();
    assert_eq!(json["schema"], "trit-wspace.grouped-certificate.v1");
    assert_eq!(json["scope"], "structural");
    assert_eq!(json["witness"], "row_proportional");
    assert_eq!(json["implies"], "all_inputs");
    assert_eq!(json["equivalent"], true);
    assert_eq!(json["shape"]["rows"], 2);
    assert_eq!(json["shape"]["cols"], 2);
    assert_eq!(json["row_ratios"].as_array().expect("ratios").len(), 2);
}

#[test]
fn full_size_certificate_is_fast_enough() {
    // 384×384、vars=4：3^4 = 81 次前向 ≈ 11.9M 次乘加（每层）。
    // 这里只卡上限，不做精确计时；超时说明实现退化成了 O(n²) 之类。
    let trits: Vec<i8> = (0..384 * 384).map(|i| (i % 3) as i8 - 1).collect();
    let groups: Vec<u32> = (0..384 * 384).map(|i| (i / 16) as u32).collect();
    let scales: Vec<f32> = (0..384 * 384 / 16)
        .map(|i| 0.5 + (i % 7) as f32 * 0.1)
        .collect();
    let a = layer(384, 384, &trits, &groups, &scales, &vec![0.0; 384]);
    let mut trits_b = trits.clone();
    // 改最后一列（不在 vars=4 的域里）：强制走到 L3 的穷举
    for row in 0..384 {
        trits_b[row * 384 + 383] = -trits_b[row * 384 + 383];
    }
    let b = layer(384, 384, &trits_b, &groups, &scales, &vec![0.0; 384]);
    let start = Instant::now();
    let certificate = a.certify_equivalence(&b, 4).expect("certify");
    let elapsed = start.elapsed();
    assert_eq!(certificate.witness, Witness::Behavior);
    assert!(
        elapsed.as_millis() < 5_000,
        "384×384、vars=4 的证书耗时 {} ms",
        elapsed.as_millis()
    );
}
