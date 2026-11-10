//! 表示层/编辑层的不变量：规范差、事务、回滚、编解码、置换。

use trit_wspace::{
    Behavior, Edit, Error, Layer, Net, Oracle, Patch, Program, Trit, align, from_wsp,
    merge_aligned, merge_vote, to_wsp,
};

fn scale_net(net: &Net, factor: f32) -> Net {
    let layers: Vec<Layer> = net
        .layers()
        .iter()
        .map(|layer| {
            let trits: Vec<i8> = layer.trits().iter().map(|t| t.to_i8()).collect();
            let bias: Vec<i8> = layer.bias().iter().map(|t| t.to_i8()).collect();
            let scales: Vec<f32> = layer.scales().iter().map(|s| s * factor).collect();
            Layer::with_scales(layer.rows(), layer.cols(), &trits, &bias, scales)
                .expect("scaled layer")
        })
        .collect();
    Net::new(layers).expect("scaled net")
}

fn flip_one_trit(text: &str) -> String {
    let mut lines: Vec<String> = text.lines().map(str::to_string).collect();
    let index = lines
        .iter()
        .position(|line| line.starts_with("row "))
        .expect("a row line");
    let mut chars: Vec<char> = lines[index].chars().collect();
    let position = chars
        .iter()
        .position(|c| matches!(c, '-' | '0' | '+'))
        .expect("a trit char");
    chars[position] = if chars[position] == '+' { '-' } else { '+' };
    lines[index] = chars.into_iter().collect();
    lines.join("\n")
}

#[test]
fn scale_gauge_is_behavioral_noop() {
    let net = Net::random(&[3, 4, 1], 11).expect("net");
    let oracle =
        Oracle::ternary(3, |input| net.forward(input).expect("forward")[0]).expect("oracle");
    let scaled = scale_net(&net, 2.5);
    assert_ne!(net.layers()[0].scales(), scaled.layers()[0].scales());
    assert_eq!(
        Behavior::of(&net, &oracle).expect("behavior"),
        Behavior::of(&scaled, &oracle).expect("behavior")
    );
    assert_eq!(net.state_hash(), scaled.state_hash());
}

#[test]
fn scale_must_be_finite_and_positive() {
    for bad in [0.0, -1.0, f32::NAN, f32::INFINITY] {
        assert!(Layer::with_scales(1, 1, &[1], &[0], vec![bad]).is_err());
    }
    assert!(Layer::with_scales(1, 1, &[1], &[0], vec![1e-30]).is_ok());
}

#[test]
fn codec_round_trip_is_exact_and_canonical() {
    for seed in 0..8 {
        let net = Net::random(&[2, 3, 2], seed).expect("net");
        let text = to_wsp(&net);
        let parsed = from_wsp(&text).expect("parse");
        assert_eq!(parsed, net);
        assert_eq!(to_wsp(&parsed), text);
        assert_eq!(trit_wspace::checksum(&parsed), trit_wspace::checksum(&net));
    }
}

#[test]
fn codec_accepts_comments_but_canonicalizes_them_away() {
    let net = Net::random(&[2, 2, 1], 3).expect("net");
    let text = to_wsp(&net);
    let mut lines: Vec<&str> = text.lines().collect();
    lines.insert(1, "# 说明：这一行不参与校验和");
    let commented = format!("{}\n", lines.join("\n"));
    let parsed = from_wsp(&commented).expect("comments are allowed");
    assert_eq!(parsed, net);
    assert!(!to_wsp(&parsed).contains('#'));
}

#[test]
fn codec_detects_any_trit_change() {
    let net = Net::random(&[2, 3, 1], 5).expect("net");
    let text = to_wsp(&net);
    let corrupted = flip_one_trit(&text);
    assert_ne!(corrupted, text);
    assert!(matches!(from_wsp(&corrupted), Err(Error::Checksum { .. })));
}

#[test]
fn codec_detects_checksum_tampering_and_truncation() {
    let net = Net::random(&[2, 2, 1], 9).expect("net");
    let text = to_wsp(&net);

    let mut tampered = text.clone();
    let mut chars: Vec<char> = tampered.chars().collect();
    let position = chars
        .iter()
        .rposition(|c| c.is_ascii_hexdigit())
        .expect("checksum digit");
    chars[position] = if chars[position] == '0' { '1' } else { '0' };
    tampered = chars.into_iter().collect();
    assert!(matches!(from_wsp(&tampered), Err(Error::Checksum { .. })));

    let truncated: String = text
        .lines()
        .filter(|line| !line.starts_with("row "))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(matches!(
        from_wsp(&truncated),
        Err(Error::MissingLine(_) | Error::Parse { .. })
    ));

    let bad_scale = text.replace("scale 1", "scale 0");
    assert!(matches!(
        from_wsp(&bad_scale),
        Err(Error::InvalidScale { .. })
    ));

    let bad_char = text.replacen("row ", "row x", 1);
    assert!(matches!(from_wsp(&bad_char), Err(Error::Parse { .. })));

    let trailing = format!("{text}extra\n");
    assert!(matches!(from_wsp(&trailing), Err(Error::Parse { .. })));
}

#[test]
fn patch_journal_restores_bit_exactly() {
    let mut net = Net::random(&[2, 3, 2], 21).expect("net");
    let before = net.clone();
    let patch = Patch::new(vec![
        Edit::SetTrit {
            layer: 0,
            row: 0,
            col: 0,
            to: Trit::Zero,
        },
        Edit::SetBias {
            layer: 0,
            row: 1,
            to: Trit::Neg,
        },
        Edit::SetScale {
            layer: 0,
            row: 2,
            to: 3.5,
        },
        Edit::SwapRows {
            layer: 0,
            a: 0,
            b: 2,
        },
        Edit::NegateRow { layer: 1, row: 1 },
    ]);
    let journal = patch.apply(&mut net).expect("apply");
    assert_ne!(net, before);
    journal.undo(&mut net);
    assert_eq!(net, before);
}

#[test]
fn patch_is_transactional() {
    let mut net = Net::random(&[2, 3, 2], 22).expect("net");
    let before = net.clone();
    let patch = Patch::new(vec![
        Edit::SetTrit {
            layer: 0,
            row: 0,
            col: 0,
            to: Trit::Pos,
        },
        Edit::SetTrit {
            layer: 9,
            row: 0,
            col: 0,
            to: Trit::Pos,
        },
    ]);
    assert!(patch.apply(&mut net).is_err());
    assert_eq!(net, before);
}

#[test]
fn diff_apply_round_trip_and_cost() {
    let from = Net::random(&[3, 3, 2], 31).expect("from");
    let to = Net::random(&[3, 3, 2], 32).expect("to");
    let patch = Patch::diff(&from, &to).expect("diff");
    let mut actual = from.clone();
    patch.apply(&mut actual).expect("apply");
    assert_eq!(actual, to);

    let cost = patch.cost(&from);
    assert_eq!(cost.trits + cost.biases + cost.scales, patch.len());
    assert_eq!(cost.row_ops, 0);

    assert!(Patch::diff(&from, &from).expect("self diff").is_empty());
    assert!(matches!(
        Patch::diff(&from, &Net::zeros(&[2, 2]).expect("other")),
        Err(Error::ArchMismatch)
    ));
}

#[test]
fn empty_patch_is_identity() {
    let mut net = Net::random(&[2, 2, 1], 41).expect("net");
    let before = net.clone();
    let patch = Patch::default();
    let journal = patch.apply(&mut net).expect("apply");
    assert_eq!(net, before);
    assert_eq!(patch.cost(&net), trit_wspace::Cost::default());
    journal.undo(&mut net);
    assert_eq!(net, before);
}

#[test]
fn program_precondition_blocks_without_mutation() {
    let mut net = and_net();
    let before = net.clone();
    let oracle = Oracle::boolean(2, |b| b[0] && b[1]).expect("oracle");
    let patch = Patch::new(vec![Edit::SetBias {
        layer: 0,
        row: 0,
        to: Trit::Neg,
    }]);
    let wrong = Behavior::of(&Net::zeros(&[2, 1]).expect("zero net"), &oracle).expect("beh");
    assert_ne!(
        wrong,
        Behavior::of(&net, &oracle).expect("and behavior"),
        "测试前提：零网络与 AND 行为不同"
    );
    let report = Program::new("blocked", patch)
        .require_before(wrong)
        .run(&mut net, &oracle)
        .expect("run");
    assert!(!report.applied);
    assert!(!report.before_ok);
    assert_eq!(net, before);
}

#[test]
fn program_rolls_back_when_postcondition_fails() {
    let mut net = and_net();
    let before = net.clone();
    let oracle = Oracle::boolean(2, |b| b[0] && b[1]).expect("oracle");
    let safe = Behavior::of(&net, &oracle).expect("behavior");

    let patch = Patch::new(vec![Edit::SetBias {
        layer: 0,
        row: 0,
        to: Trit::Zero,
    }]);
    let mut changed = net.clone();
    patch.apply(&mut changed).expect("apply");
    let changed_behavior = Behavior::of(&changed, &oracle).expect("behavior");
    assert_ne!(changed_behavior, safe, "测试前提：这条编辑必须改变行为");

    let report = Program::new("rollback", patch)
        .require_before(safe.clone())
        .require_after(safe)
        .run(&mut net, &oracle)
        .expect("run");
    assert!(report.applied);
    assert!(report.rolled_back);
    assert!(!report.after_ok);
    assert_eq!(report.changed_signature, changed_behavior.signature());
    assert_eq!(report.final_signature, report.before_signature);
    assert_eq!(net, before);
}

#[test]
fn program_applies_when_postcondition_holds() {
    let mut net = and_net();
    let oracle = Oracle::boolean(2, |b| b[0] && b[1]).expect("oracle");
    let patch = Patch::new(vec![Edit::SetBias {
        layer: 0,
        row: 0,
        to: Trit::Zero,
    }]);
    let mut expected = net.clone();
    patch.apply(&mut expected).expect("apply");
    let want = Behavior::of(&expected, &oracle).expect("behavior");

    let report = Program::new("ok", patch)
        .require_after(want)
        .run(&mut net, &oracle)
        .expect("run");
    assert!(report.applied);
    assert!(!report.rolled_back);
    assert!(report.after_ok);
    assert_eq!(net, expected);
    assert_eq!(report.final_signature, report.changed_signature);
}

#[test]
fn oracle_domains_are_ordered_and_bounded() {
    let and = Oracle::boolean(2, |b| b[0] && b[1]).expect("and");
    assert_eq!(and.len(), 4);
    assert_eq!(
        and.outputs().iter().map(|row| row[0]).collect::<Vec<_>>(),
        vec![Trit::Neg, Trit::Neg, Trit::Neg, Trit::Pos]
    );
    assert_eq!(
        and.inputs()[3],
        vec![Trit::Pos, Trit::Pos],
        "最后一个变量变化最快"
    );

    let ternary = Oracle::ternary(2, |input| input[0]).expect("ternary");
    assert_eq!(ternary.len(), 9);
    assert_eq!(ternary.inputs()[0], vec![Trit::Neg, Trit::Neg]);
    assert_eq!(ternary.inputs()[8], vec![Trit::Pos, Trit::Pos]);

    assert!(matches!(
        Oracle::boolean(0, |_| true),
        Err(Error::DomainTooLarge { .. })
    ));
    assert!(matches!(
        Oracle::boolean(21, |_| true),
        Err(Error::DomainTooLarge { .. })
    ));
    assert!(matches!(
        Oracle::ternary(11, |_| Trit::Zero),
        Err(Error::DomainTooLarge { .. })
    ));
}

#[test]
fn oracle_rejects_ragged_data() {
    assert!(matches!(
        Oracle::new(vec![], vec![]),
        Err(Error::EmptyOracle)
    ));
    assert!(matches!(
        Oracle::new(
            vec![vec![Trit::Pos], vec![Trit::Pos, Trit::Neg]],
            vec![vec![Trit::Pos], vec![Trit::Pos]]
        ),
        Err(Error::Shape { .. })
    ));
    assert!(matches!(
        Oracle::new(
            vec![vec![Trit::Pos]],
            vec![vec![Trit::Pos], vec![Trit::Neg]]
        ),
        Err(Error::Shape { .. })
    ));
}

#[test]
fn net_rejects_shape_errors() {
    assert!(matches!(Net::zeros(&[2]), Err(Error::EmptySizes)));
    assert!(Layer::new(0, 2, &[], &[]).is_err());
    let a = Layer::new(2, 2, &[0; 4], &[0; 2]).expect("a");
    let b = Layer::new(2, 3, &[0; 6], &[0; 2]).expect("b");
    assert!(matches!(Net::new(vec![a, b]), Err(Error::Shape { .. })));
    let net = Net::zeros(&[2, 2, 1]).expect("net");
    assert!(matches!(
        net.forward(&[Trit::Pos]),
        Err(Error::Shape { .. })
    ));
    assert!(matches!(net.readout(&[0.0]), Err(Error::Shape { .. })));
}

#[test]
fn permutation_is_involutive_and_behavior_preserving() {
    let net = Net::random(&[3, 3, 1], 61).expect("net");
    let perm = [2, 0, 1];
    let inverse = [1, 2, 0];
    let mapped = net.permute_rows(0, &perm).expect("permute");
    let oracle =
        Oracle::ternary(3, |input| net.forward(input).expect("forward")[0]).expect("oracle");
    assert_eq!(
        Behavior::of(&net, &oracle).expect("a"),
        Behavior::of(&mapped, &oracle).expect("b")
    );
    let restored = mapped.permute_rows(0, &inverse).expect("inverse");
    assert_eq!(restored, net);
    assert!(matches!(
        net.permute_rows(1, &[0]),
        Err(Error::OutputLayerPermutation { .. })
    ));
    assert!(matches!(
        net.permute_rows(0, &[0, 0, 1]),
        Err(Error::NotPermutation { .. })
    ));
}

#[test]
fn align_recovers_a_permuted_model_and_merge_is_identity() {
    let a = Net::new(vec![
        Layer::new(2, 2, &[1, 1, -1, 1], &[0, 0]).expect("l0"),
        Layer::new(1, 2, &[-1, 1], &[0]).expect("l1"),
    ])
    .expect("a");
    let b = a.permute_rows(0, &[1, 0]).expect("permute");

    let aligned = align(&a, &b).expect("align");
    assert_eq!(aligned, a);
    assert_eq!(merge_aligned(&a, &b).expect("merge"), a);

    let naive = merge_vote(&a, &b).expect("naive");
    assert_ne!(naive, a, "不对齐会改坏权重");
    let oracle = Oracle::boolean(2, |bits| bits[0] || bits[1]).expect("oracle");
    assert_ne!(
        Behavior::of(&naive, &oracle).expect("naive behavior"),
        Behavior::of(&a, &oracle).expect("aligned behavior"),
        "不对齐的合并会改变行为"
    );
}

#[test]
fn from_f32_uses_absmean_and_protects_zero_rows() {
    let layer = Layer::from_f32(2, 2, &[0.0, 0.0, 0.3, -0.9], &[0.0, 2.0]).expect("layer");
    assert_eq!(layer.scale(0), Some(1.0));
    assert_eq!(layer.row(0), Some(&[Trit::Zero, Trit::Zero][..]));
    assert_eq!(layer.scale(1), Some(0.6));
    assert_eq!(layer.row(1), Some(&[Trit::Pos, Trit::Neg][..]));
    assert_eq!(layer.bias_trit(1), Some(Trit::Pos));
}

#[test]
fn state_hash_ignores_scale_and_tracks_trits() {
    let net = Net::random(&[2, 2, 1], 71).expect("net");
    let scaled = scale_net(&net, 7.0);
    assert_eq!(net.state_hash(), scaled.state_hash());

    let mut changed = net.clone();
    let current = changed.layer(1).expect("layer").trit(0, 1).expect("trit");
    let to = if current == Trit::Neg {
        Trit::Pos
    } else {
        Trit::Neg
    };
    Patch::new(vec![Edit::SetTrit {
        layer: 1,
        row: 0,
        col: 1,
        to,
    }])
    .apply(&mut changed)
    .expect("apply");
    assert_ne!(net.state_hash(), changed.state_hash());
}

fn and_net() -> Net {
    // sign(x0 + x1 - 1)：只有 (+,+) 为 +，其余为 -。
    Net::new(vec![Layer::new(1, 2, &[1, 1], &[-1]).expect("layer")]).expect("net")
}

#[test]
fn readout_keeps_hidden_sign_and_final_margin() {
    let net = Net::new(vec![
        Layer::with_scales(2, 2, &[1, 1, 1, -1], &[0, 0], vec![2.0, 2.0]).expect("l0"),
        Layer::with_scales(1, 2, &[1, 0], &[0], vec![3.0]).expect("l1"),
    ])
    .expect("net");
    // 隐藏层：[sign(x0+x1), sign(x0-x1)]。
    // x=(1,1) → [+1, 0]，输出 raw = 3*1 = 3。
    assert_eq!(net.readout(&[1.0, 1.0]).expect("readout"), vec![3.0]);
    // x=(1,-1) → [0, +1]，输出 raw = 3*0 = 0。
    assert_eq!(net.readout(&[1.0, -1.0]).expect("readout"), vec![0.0]);
    // x=(-1,0) → [-1, -1]，输出 raw = 3*(-1) = -3。
    assert_eq!(net.readout(&[-1.0, 0.0]).expect("readout"), vec![-3.0]);
}
