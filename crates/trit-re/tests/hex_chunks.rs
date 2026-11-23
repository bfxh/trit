//! hex 段链匹配的集成测试（千段命中 / 跳跃回溯 / 前缀重叠与通配抖动）。

use trit_core::value::Ternary;
use trit_re::sigscan::{Pattern, Rule, SigScan};

fn hex_rule(id: &str, pattern: Pattern) -> Rule {
    Rule::new("r").push(id, pattern)
}

/// 拼 `chunks` 段的定长跳跃 hex 串：首段 `4D`，后续每段 `01`（`[0]` 衔接）。
fn repeated_hex_spec(chunks: usize) -> String {
    let mut spec = String::from("4D");
    for _ in 1..chunks {
        spec.push_str(" [0] 01");
    }
    spec
}

#[test]
fn hex_thousand_chunks_still_hits() {
    // 对照规则口径：1000 段链在完整样本上照常命中（迭代化不得改变既有命中）。
    let chunks = 1000usize;
    let p = Pattern::hex(&repeated_hex_spec(chunks)).expect("千段合法");
    let mut data = Vec::with_capacity(chunks);
    data.push(0x4D);
    data.extend(vec![0x01; chunks - 1]);
    let report = SigScan::new(&[hex_rule("a", p)]).scan(&data, false);
    assert_eq!(report.rules[0].value, Ternary::True);
    assert_eq!(report.rules[0].hits, vec![("a".into(), 0)]);
    // 截断样本：缺末段 → 完整扫描后 False（不假命中）。
    let report =
        SigScan::new(
            &[Rule::new("r").push("a", Pattern::hex(&repeated_hex_spec(chunks)).unwrap())],
        )
        .scan(&data[..chunks - 1], false);
    assert_eq!(report.rules[0].value, Ternary::False);
}

#[test]
fn hex_backtracks_across_jump_candidates() {
    // 跳跃窗口回溯：前段候选先尝试近距，不匹配再退到远距；跨段抖动照常命中。
    let p = Pattern::hex("4D [1-2] 50").unwrap();
    let scan = |d: &[u8]| {
        SigScan::new(&[hex_rule("a", p.clone())])
            .scan(d, false)
            .rules[0]
            .value
    };
    assert_eq!(scan(&[0x4D, 0x00, 0x50]), Ternary::True, "间隔 1 命中");
    assert_eq!(
        scan(&[0x4D, 0x00, 0x00, 0x50]),
        Ternary::True,
        "间隔 2 命中"
    );
    assert_eq!(scan(&[0x4D, 0x50]), Ternary::False, "间隔 0 不在窗口");
    assert_eq!(
        scan(&[0x4D, 0x00, 0x00, 0x00, 0x50]),
        Ternary::False,
        "间隔 3 出窗"
    );
}

#[test]
fn hex_prefix_overlap_and_wildcard_jitter() {
    // 前缀重叠：命中可落在样本中段；跨段通配 + 抖动位不影响链上其余段。
    let p = Pattern::hex("01 02 [0] 03").unwrap();
    let scan = |d: &[u8]| SigScan::new(&[hex_rule("a", p.clone())]).scan(d, false);
    let report = scan(&[0x00, 0x01, 0x02, 0x03]);
    assert_eq!(report.rules[0].value, Ternary::True);
    assert_eq!(report.rules[0].hits, vec![("a".into(), 1)]);
    let p = Pattern::hex("4D [0-1] ?? 5A").unwrap();
    let scan = |d: &[u8]| {
        SigScan::new(&[hex_rule("a", p.clone())])
            .scan(d, false)
            .rules[0]
            .value
    };
    assert_eq!(scan(&[0x4D, 0x77, 0x5A]), Ternary::True, "跳跃 0 + 通配");
    assert_eq!(
        scan(&[0x4D, 0x00, 0x77, 0x5A]),
        Ternary::True,
        "跳跃 1 + 通配"
    );
    // 5A 落在跳跃窗口可达范围之外 → False（回溯后仍无落位）
    assert_eq!(
        scan(&[0x4D, 0x77, 0x00, 0x00, 0x5A]),
        Ternary::False,
        "通配位后对位失败"
    );
}

#[test]
fn hex_parser_rejects_overlong_chunks_directly() {
    // 解析期直查：4096 段合法，4097 段结构化报错（不进入匹配、不 abort）。
    let mut ok = String::from("4D");
    for _ in 0..4095 {
        ok.push_str(" [0] 01");
    }
    assert!(Pattern::hex(&ok).is_ok(), "4096 段应在上限内");

    let mut over = String::from("4D");
    for _ in 0..4096 {
        over.push_str(" [0] 01");
    }
    let Err(e) = Pattern::hex(&over) else {
        panic!("超上限应拒绝");
    };
    assert!(e.contains("段数"), "拒绝原因应说明段数上限：{e}");
}
