//! 脚本组合：原子序列、条件分支、线格式往返。

use trit_wspace::wire;
use trit_wspace::{
    Behavior, Edit, Layer, Net, Oracle, Patch, Program, Script, ScriptReport, Trit, checksum,
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

fn break_patch() -> Patch {
    Patch::new(vec![Edit::SetTrit {
        layer: 0,
        row: 0,
        col: 0,
        to: Trit::Zero,
    }])
}

fn fix_patch() -> Patch {
    Patch::new(vec![Edit::SetTrit {
        layer: 0,
        row: 0,
        col: 0,
        to: Trit::Neg,
    }])
}

fn zero_behavior() -> Behavior {
    Behavior::from_outputs(vec![vec![Trit::Zero]; 4])
}

fn broken_net() -> Net {
    let mut net = xor_net();
    break_patch().apply(&mut net).expect("break");
    net
}

#[test]
fn atomic_sequence_rolls_back_every_step() {
    let oracle = xor_oracle();
    let mut net = xor_net();
    let script = Script::sequence(
        "all-or-nothing",
        vec![
            Script::step(Program::new("break", break_patch())),
            Script::step(Program::new("bad-fix", fix_patch()).require_after(zero_behavior())),
        ],
    );
    let report = script.run(&mut net, &oracle).expect("run");
    let ScriptReport::Sequence {
        steps,
        rolled_back,
        final_signature,
        ..
    } = &report
    else {
        panic!("sequence report");
    };
    assert_eq!(steps.len(), 2);
    assert!(rolled_back);
    assert!(report.failed());
    assert_eq!(*final_signature, xor_oracle().behavior().signature());
    assert_eq!(net, xor_net(), "两步都被回滚");
    assert_eq!(checksum(&net), checksum(&xor_net()));
}

#[test]
fn lax_sequence_keeps_earlier_steps() {
    let oracle = xor_oracle();
    let mut net = xor_net();
    let script = Script::sequence(
        "best-effort",
        vec![
            Script::step(Program::new("break", break_patch())),
            Script::step(Program::new("bad-fix", fix_patch()).require_after(zero_behavior())),
        ],
    )
    .atomic(false);
    let report = script.run(&mut net, &oracle).expect("run");
    let ScriptReport::Sequence { rolled_back, .. } = &report else {
        panic!("sequence report");
    };
    assert!(!rolled_back);
    assert_eq!(net, broken_net(), "第一步保留");
    assert_eq!(
        report.final_signature(),
        Behavior::of(&broken_net(), &oracle)
            .expect("behavior")
            .signature()
    );
}

#[test]
fn when_selects_then_or_otherwise() {
    let oracle = xor_oracle();
    let expected_broken = Behavior::of(&broken_net(), &oracle).expect("behavior");
    let then = Script::step(Program::new("fix", fix_patch()));
    let otherwise = Script::step(Program::new("break", break_patch()));

    let mut net = broken_net();
    let script = Script::when(
        "repair-if-broken",
        expected_broken.clone(),
        then.clone(),
        Some(otherwise.clone()),
    );
    let report = script.run(&mut net, &oracle).expect("run");
    let ScriptReport::When {
        matched, chosen, ..
    } = &report
    else {
        panic!("when report");
    };
    assert!(matched);
    assert!(chosen.as_ref().is_some_and(|report| !report.failed()));
    assert_eq!(net, xor_net(), "走了 then 分支");

    let mut net = xor_net();
    let script = Script::when("repair-if-broken", expected_broken, then, Some(otherwise));
    let report = script.run(&mut net, &oracle).expect("run");
    let ScriptReport::When {
        matched, chosen, ..
    } = &report
    else {
        panic!("when report");
    };
    assert!(!matched);
    assert!(chosen.as_ref().is_some_and(|report| !report.failed()));
    assert_eq!(net, broken_net(), "走了 otherwise 分支");

    let mut net = broken_net();
    let script = Script::when(
        "no-else",
        behavior_of(&xor_net(), &oracle),
        Script::step(Program::new("fix", fix_patch())),
        None,
    );
    let report = script.run(&mut net, &oracle).expect("run");
    let ScriptReport::When {
        matched, chosen, ..
    } = &report
    else {
        panic!("when report");
    };
    assert!(!matched);
    assert!(chosen.is_none());
    assert_eq!(net, broken_net(), "没有否则分支时不动");
}

fn behavior_of(net: &Net, oracle: &Oracle) -> Behavior {
    Behavior::of(net, oracle).expect("behavior")
}

#[test]
fn script_wire_round_trip() {
    let oracle = xor_oracle();
    let script = Script::sequence(
        "root",
        vec![
            Script::step(Program::new("break", break_patch())),
            Script::when(
                "branch",
                behavior_of(&broken_net(), &oracle),
                Script::step(
                    Program::new("fix", fix_patch())
                        .require_after(behavior_of(&xor_net(), &oracle)),
                ),
                Some(Script::step(Program::new("noop", break_patch()))),
            ),
        ],
    );
    let value = wire::script_to_value(&script);
    let parsed = wire::script_from_value(&value).expect("parse script");
    assert_eq!(parsed, script);

    let mut left = xor_net();
    let mut right = xor_net();
    let left_report = script.run(&mut left, &oracle).expect("left");
    let right_report = parsed.run(&mut right, &oracle).expect("right");
    assert_eq!(left, right);
    assert_eq!(
        wire::script_report_to_value(&left_report),
        wire::script_report_to_value(&right_report)
    );
    assert!(oracle.exact(&left).expect("exact"));
}

#[test]
fn unknown_script_type_is_rejected() {
    let value = serde_json::json!({"type": "nope"});
    assert!(wire::script_from_value(&value).is_err());
    let value = serde_json::json!({"type": "sequence"});
    assert!(wire::script_from_value(&value).is_err());
    let value = serde_json::json!({"type": "when", "when": {"outputs": ["0"]}});
    assert!(wire::script_from_value(&value).is_err());
}
