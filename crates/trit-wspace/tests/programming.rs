//! 编程语义：可达性、最小修补、预算与不可达目标。

use trit_wspace::{
    Edit, Layer, Net, Oracle, Patch, Program, RepairOptions, Trit, from_wsp, repair, to_wsp,
};

fn xor_net() -> Net {
    Net::new(vec![
        Layer::new(2, 2, &[-1, -1, -1, -1], &[-1, 1]).expect("l0"),
        Layer::new(1, 2, &[-1, 1], &[-1]).expect("l1"),
    ])
    .expect("xor net")
}

fn xor_oracle() -> Oracle {
    Oracle::boolean(2, |bits| bits[0] != bits[1]).expect("oracle")
}

fn ternary_code(outputs: &[Vec<Trit>]) -> u32 {
    outputs.iter().fold(0u32, |code, row| {
        code * 3 + u32::from((row[0].to_i8() + 1) as u8)
    })
}

#[test]
fn every_boolean_function_is_reachable_in_2_2_1() {
    let oracles: Vec<Oracle> = (0u16..16)
        .map(|mask| {
            Oracle::boolean(2, move |bits| {
                let index = (usize::from(bits[0]) << 1) | usize::from(bits[1]);
                (mask >> (3 - index)) & 1 == 1
            })
            .expect("oracle")
        })
        .collect();
    let expected: Vec<u32> = oracles.iter().map(|o| ternary_code(o.outputs())).collect();

    let mut reached = 0u16;
    for config in 0..3usize.pow(9) {
        let mut rest = config;
        let mut values = [0i8; 9];
        for value in &mut values {
            *value = Trit::ALL[rest % 3].to_i8();
            rest /= 3;
        }
        let net = Net::new(vec![
            Layer::new(2, 2, &values[0..4], &values[4..6]).expect("l0"),
            Layer::new(1, 2, &values[6..8], &values[8..9]).expect("l1"),
        ])
        .expect("net");
        let outputs = oracles[0]
            .inputs()
            .iter()
            .map(|input| net.forward(input).expect("forward"))
            .collect::<Vec<_>>();
        let code = ternary_code(&outputs);
        if let Some(index) = expected.iter().position(|&want| want == code) {
            reached |= 1 << index;
        }
    }
    assert_eq!(
        reached, 0xFFFF,
        "2-2-1（带偏置）必须覆盖全部 16 个二元布尔函数"
    );
}

#[test]
fn single_trit_corruption_is_repairable_by_one_edit() {
    let net = xor_net();
    let oracle = xor_oracle();
    let mut cases = 0;
    for layer in 0..net.depth() {
        let shape = net.layer(layer).expect("layer");
        for row in 0..shape.rows() {
            for col in 0..shape.cols() {
                let current = shape.trit(row, col).expect("trit");
                for &to in &Trit::ALL {
                    if to == current {
                        continue;
                    }
                    let edit = Edit::SetTrit {
                        layer,
                        row,
                        col,
                        to,
                    };
                    cases += corrupt_and_repair(&net, &oracle, edit);
                }
            }
            let current = shape.bias_trit(row).expect("bias");
            for &to in &Trit::ALL {
                if to == current {
                    continue;
                }
                let edit = Edit::SetBias { layer, row, to };
                cases += corrupt_and_repair(&net, &oracle, edit);
            }
        }
    }
    assert!(cases > 0, "至少要有一个破坏行为的单点编辑");
}

fn corrupt_and_repair(net: &Net, oracle: &Oracle, edit: Edit) -> usize {
    let mut broken = net.clone();
    Patch::new(vec![edit]).apply(&mut broken).expect("corrupt");
    if oracle.exact(&broken).expect("loss") {
        return 0;
    }
    let options = RepairOptions {
        max_edits: 1,
        ..RepairOptions::default()
    };
    let found = repair(&broken, oracle, &options)
        .expect("repair")
        .expect("single-trit corruption must have a one-edit fix");
    assert_eq!(found.patch.len(), 1, "最小性：单点破坏只需要一条编辑");
    assert_eq!(found.patch.cost(&broken).scales, 0, "修补不碰 scale");
    let mut fixed = broken.clone();
    found.patch.apply(&mut fixed).expect("apply");
    assert!(oracle.exact(&fixed).expect("loss"));
    1
}

#[test]
fn two_trit_corruption_is_repaired_within_two_edits() {
    let net = xor_net();
    let oracle = xor_oracle();
    let mut broken = net.clone();
    Patch::new(vec![
        Edit::SetTrit {
            layer: 0,
            row: 0,
            col: 0,
            to: Trit::Zero,
        },
        Edit::SetTrit {
            layer: 1,
            row: 0,
            col: 0,
            to: Trit::Pos,
        },
    ])
    .apply(&mut broken)
    .expect("corrupt");
    assert!(!oracle.exact(&broken).expect("loss"));

    let options = RepairOptions {
        max_edits: 2,
        ..RepairOptions::default()
    };
    let found = repair(&broken, &oracle, &options)
        .expect("repair")
        .expect("two-edit fix");
    assert!(found.patch.len() <= 2);
    let mut fixed = broken.clone();
    found.patch.apply(&mut fixed).expect("apply");
    assert!(oracle.exact(&fixed).expect("loss"));
}

#[test]
fn repair_from_zeros_reaches_xor_at_the_far_corner() {
    // 零网络到 XOR 的汉明距离恰好是 9（2-2-1 的全部参数都非零，穷举可证），
    // 也就是三值立方体的最远角：WSP 的真正优势是“改已有工件”，不是从零训练。
    let net = Net::zeros(&[2, 2, 1]).expect("zero net");
    let oracle = xor_oracle();
    let options = RepairOptions {
        max_edits: 9,
        beam_width: 128,
        exhaustive_pairs: 20_000,
        ..RepairOptions::default()
    };
    let found = repair(&net, &oracle, &options)
        .expect("repair")
        .expect("BFS should find an exact program");
    let mut programmed = net.clone();
    found.patch.apply(&mut programmed).expect("apply");
    assert!(oracle.exact(&programmed).expect("loss"));
    assert_eq!(found.patch.cost(&net).touched, 9, "最小编辑数是 9");
}

#[test]
fn repair_respects_the_edit_budget() {
    let broken = {
        let mut net = xor_net();
        Patch::new(vec![Edit::SetTrit {
            layer: 0,
            row: 0,
            col: 0,
            to: Trit::Zero,
        }])
        .apply(&mut net)
        .expect("corrupt");
        net
    };
    let oracle = xor_oracle();
    assert!(
        repair(
            &broken,
            &oracle,
            &RepairOptions {
                max_edits: 0,
                ..Default::default()
            }
        )
        .expect("repair")
        .is_none()
    );
    let zeros = Net::zeros(&[2, 2, 1]).expect("zeros");
    assert!(
        repair(
            &zeros,
            &oracle,
            &RepairOptions {
                max_edits: 1,
                ..Default::default()
            }
        )
        .expect("repair")
        .is_none()
    );
}

#[test]
fn repair_reports_none_for_an_unreachable_target() {
    // 2→1 单层只能实现半空间，XOR 不可达；搜索不能“差不多对”就算成功。
    let net = Net::zeros(&[2, 1]).expect("net");
    let oracle = xor_oracle();
    let options = RepairOptions {
        max_edits: 4,
        beam_width: 32,
        exhaustive_pairs: 20_000,
        ..RepairOptions::default()
    };
    assert!(repair(&net, &oracle, &options).expect("repair").is_none());
}

#[test]
fn repair_is_deterministic() {
    let mut broken = xor_net();
    Patch::new(vec![Edit::SetBias {
        layer: 0,
        row: 0,
        to: Trit::Zero,
    }])
    .apply(&mut broken)
    .expect("corrupt");
    let oracle = xor_oracle();
    let options = RepairOptions::default();
    let first = repair(&broken, &oracle, &options)
        .expect("a")
        .expect("found");
    let second = repair(&broken, &oracle, &options)
        .expect("b")
        .expect("found");
    assert_eq!(first.patch, second.patch);
    assert_eq!(first.evaluated, second.evaluated);
}

#[test]
fn repaired_artifact_survives_the_codec_and_a_program() {
    let mut broken = xor_net();
    Patch::new(vec![Edit::SetTrit {
        layer: 0,
        row: 0,
        col: 0,
        to: Trit::Zero,
    }])
    .apply(&mut broken)
    .expect("corrupt");
    let oracle = xor_oracle();
    let want = xor_want_behavior(&oracle);
    let found = repair(&broken, &oracle, &RepairOptions::default())
        .expect("repair")
        .expect("fix");

    let mut fixed = broken.clone();
    let report = Program::new("repair-xor", found.patch)
        .require_after(want)
        .run(&mut fixed, &oracle)
        .expect("run");
    assert!(report.applied);
    assert!(!report.rolled_back);

    let text = to_wsp(&fixed);
    let parsed = from_wsp(&text).expect("parse");
    assert_eq!(parsed.state_hash(), fixed.state_hash());
    assert!(oracle.exact(&parsed).expect("loss"));
}

fn xor_want_behavior(oracle: &Oracle) -> trit_wspace::Behavior {
    let net = xor_net();
    trit_wspace::Behavior::of(&net, oracle).expect("behavior")
}
