//! `sigscan` 的单元测试（从 sigscan.rs 拆出，控制单文件行数预算）。

use super::*;
use trit_core::value::Ternary;

#[rustfmt::skip]
fn rule(name: &str, id: &str, pattern: Pattern) -> Rule { Rule::new(name).push(id, pattern) }

#[test]
fn hex_literal_and_wildcard() {
    let p = Pattern::hex("4D 5A").unwrap();
    let data = [0x00, 0x4D, 0x5A, 0x90];
    let report = SigScan::new(&[rule("r", "a", p)]).scan(&data, false);
    assert_eq!(report.rules[0].value, Ternary::True);
    assert_eq!(report.rules[0].hits, vec![("a".into(), 1)]);
    let wild = Pattern::hex("4D ?? 90").unwrap();
    let data = [0x4D, 0x11, 0x90];
    assert_eq!(
        SigScan::new(&[rule("r", "a", wild)])
            .scan(&data, false)
            .rules[0]
            .value,
        Ternary::True
    );
}

#[test]
fn hex_jump_window() {
    // 4D 5A [2-4] 50：间隔 2..=4 字节
    let p = Pattern::hex("4D 5A [2-4] 50").unwrap();
    let ok = [0x4D, 0x5A, 1, 2, 0x50]; // 间隔 2
    let ok4 = [0x4D, 0x5A, 1, 2, 3, 4, 0x50]; // 间隔 4
    let too_far = [0x4D, 0x5A, 1, 2, 3, 4, 5, 0x50]; // 间隔 5
    let scan = |d: &[u8]| SigScan::new(&[rule("r", "a", p.clone())]).scan(d, false);
    assert_eq!(scan(&ok).rules[0].value, Ternary::True);
    assert_eq!(scan(&ok4).rules[0].value, Ternary::True);
    assert_eq!(scan(&too_far).rules[0].value, Ternary::False);
    // 语法：[n] 定长
    let fixed = Pattern::hex("4D 5A [2] 50").unwrap();
    let scan = |d: &[u8]| SigScan::new(&[Rule::new("r").push("a", fixed.clone())]).scan(d, false);
    assert_eq!(scan(&ok).rules[0].value, Ternary::True);
    assert_eq!(scan(&ok4).rules[0].value, Ternary::False, "[2] 不容间隔 4");
}

#[test]
fn hex_spec_errors_fail_closed() {
    assert!(Pattern::hex("4D 5A [2-").is_err(), "跳跃未闭合");
    assert!(Pattern::hex("4D 5A [4-2] 50").is_err(), "区间反了");
    assert!(Pattern::hex("4D [2-4]").is_err(), "跳跃后必须有字节段");
    assert!(Pattern::hex("4D 5A [2-4]").is_err(), "以跳跃结尾不合法");
    assert!(Pattern::hex("4D 5A [0x10] 50").is_err(), "非法数字");
    assert!(Pattern::hex("4D ZZ").is_err(), "非法字节");
    assert!(Pattern::hex("?? ??").is_ok(), "通配开头合法");
}

#[test]
fn text_nocase_and_wide() {
    let p = Pattern::text("CreateRemoteThread", false, false);
    let data = b"xx CreateRemoteThread yy";
    assert_eq!(
        SigScan::new(&[rule("r", "a", p.clone())])
            .scan(data, false)
            .rules[0]
            .value,
        Ternary::True
    );
    let nocase = Pattern::text("createremotethread", true, false);
    assert_eq!(
        SigScan::new(&[Rule::new("r").push("a", nocase)])
            .scan(data, false)
            .rules[0]
            .value,
        Ternary::True,
        "nocase = ASCII 折叠"
    );
    // wide：仅 UTF-16LE 变体（对齐 YARA `wide` 语义：ascii 变体不再命中）
    let wide = Pattern::text("AZ", false, true);
    let wdata: Vec<u8> = "AZ".encode_utf16().flat_map(|u| u.to_le_bytes()).collect();
    assert_eq!(
        SigScan::new(&[Rule::new("r").push("a", wide.clone())])
            .scan(&wdata, false)
            .rules[0]
            .value,
        Ternary::True
    );
    assert_eq!(
        SigScan::new(&[Rule::new("r").push("a", wide)])
            .scan(b"AZ", false)
            .rules[0]
            .value,
        Ternary::False,
        "wide=true 时 ASCII 形态不命中"
    );
}

#[test]
fn conditions_all_any_count_at_in() {
    let a = Pattern::hex("01").unwrap();
    let b = Pattern::hex("02").unwrap();
    let data = [0x01, 0xFF, 0x02];
    let s = |c: Condition| {
        SigScan::new(&[Rule::new("r")
            .push("a", a.clone())
            .push("b", b.clone())
            .condition(c)])
        .scan(&data, false)
        .rules[0]
            .value
    };
    assert_eq!(s(Condition::All), Ternary::True);
    assert_eq!(s(Condition::Any), Ternary::True);
    assert_eq!(s(Condition::Count(2)), Ternary::True);
    assert_eq!(s(Condition::Count(3)), Ternary::False);
    // $a at 1（首次命中恰在偏移 1？a 首次命中在 0）→ False
    assert_eq!(
        s(Condition::At {
            index: 0,
            offset: Operand::Lit(1)
        }),
        Ternary::False
    );
    assert_eq!(
        s(Condition::At {
            index: 0,
            offset: Operand::Lit(0)
        }),
        Ternary::True
    );
    // $b in (1..=2)：b 命中在 2
    assert_eq!(
        s(Condition::In {
            index: 1,
            lo: 1,
            hi: 2
        }),
        Ternary::True
    );
    assert_eq!(
        s(Condition::In {
            index: 1,
            lo: 0,
            hi: 1
        }),
        Ternary::False
    );
}

#[test]
fn truncated_input_never_false() {
    // 验收判据的本体："未命中"只在完整扫描后给出
    let p = Pattern::hex("4D 5A 90 00").unwrap();
    let rules = [rule("r", "a", p)];
    let report = SigScan::new(&rules).scan(&[0x4D], true);
    assert_eq!(report.rules[0].value, Ternary::Unknown);
    assert_eq!(report.value, Ternary::Unknown);
    // 命中不依赖完整性：截断但有命中 → True
    let report = SigScan::new(&rules).scan(&[0x4D, 0x5A, 0x90, 0x00], true);
    assert_eq!(report.rules[0].value, Ternary::True);
    // 完整 + 未命中 → False
    let report = SigScan::new(&rules).scan(&[0x00, 0x00], false);
    assert_eq!(report.rules[0].value, Ternary::False);
}

#[test]
fn overall_is_kleene_or() {
    let hit = rule("hit", "a", Pattern::hex("01").unwrap());
    let miss = rule("miss", "b", Pattern::hex("99").unwrap());
    let rules = [hit, miss];
    // True ∨ False = True（报警语义）
    let report = SigScan::new(&rules).scan(&[0x01], false);
    assert_eq!(report.value, Ternary::True);
    // False ∨ False = False
    let report = SigScan::new(&rules).scan(&[0x00], false);
    assert_eq!(report.value, Ternary::False);
}

#[test]
fn hits_are_capped() {
    // 0x00 填满 + 模式 00 → 命中数封顶 MAX_HITS
    let data = vec![0x00u8; 70000];
    let rules = [rule("r", "a", Pattern::hex("00").unwrap())];
    let report = SigScan::new(&rules).scan(&data, false);
    assert_eq!(report.rules[0].hits.len(), MAX_HITS);
    // 命中存在 → True 不受截断影响（命中不依赖完整性）
    assert_eq!(report.rules[0].value, Ternary::True);
}

#[test]
fn hit_cap_truncates_positional_conditions_honestly() {
    // 命中表封顶截断 = 不完整：位置性条件（at）在记录集不能证明时
    // 如实弃权——截断的沉默不是清白，绝不据残表判 False
    let data = vec![0x00u8; 70000];
    // offset 69999 处确有命中，但封顶后未记录 → Unknown
    let rules = [Rule::new("r")
        .push("a", Pattern::hex("00").unwrap())
        .condition(Condition::At {
            index: 0,
            offset: Operand::Lit(69999),
        })];
    let report = SigScan::new(&rules).scan(&data, false);
    assert_eq!(report.rules[0].hits.len(), MAX_HITS, "封顶生效");
    assert_eq!(
        report.rules[0].value,
        Ternary::Unknown,
        "残表不能证伪位置性命中"
    );
    // 记录集内的命中照常证明：offset 10 在前 65536 里 → True
    let rules = [Rule::new("r")
        .push("a", Pattern::hex("00").unwrap())
        .condition(Condition::At {
            index: 0,
            offset: Operand::Lit(10),
        })];
    let report = SigScan::new(&rules).scan(&data, false);
    assert_eq!(report.rules[0].value, Ternary::True);
}

#[test]
fn pathological_jump_pattern_yields_unknown_not_hang() {
    // 全 0x90 数据 + 永不成立的跳跃链：朴素回溯 ≈ 4096³ 条路径（挂起）；
    // 预算封顶后立即返回，且耗尽 = 扫描不完整 → Unknown 而非 False
    //（"未命中只在完整扫描后给出"——预算耗尽的沉默不是清白）
    let data = vec![0x90u8; 8192];
    let p = Pattern::hex("90 [0-4096] 00 [0-4096] 00 [0-4096] 00").unwrap();
    let report = SigScan::new(&[Rule::new("r").push("a", p)]).scan(&data, false);
    assert_eq!(
        report.rules[0].value,
        Ternary::Unknown,
        "预算耗尽 = 不完整扫描 → Unknown（绝不挂起、绝不误判 False）"
    );
    assert_eq!(report.value, Ternary::Unknown);
}

#[test]
fn normal_patterns_still_false_within_budget() {
    // 小数据正常模式：预算远未耗尽 → MISS 照旧判 False（语义不漂移）
    let data = vec![0x90u8; 64];
    let p = Pattern::hex("90 [0-8] 00 [0-8] 00 [0-8] 00").unwrap();
    let report = SigScan::new(&[Rule::new("r").push("a", p)]).scan(&data, false);
    assert_eq!(report.rules[0].value, Ternary::False, "小数据完整扫描");
}

#[test]
fn entrypoint_context_resolves_at_condition() {
    // entrypoint = 入口点文件偏移（官方 YARA 语义，实测对拍判定）：
    // 有上下文可评估，缺上下文如实弃权（绝不 False）
    let rules = [Rule::new("r")
        .push("a", Pattern::hex("4D 5A 90 00").unwrap())
        .condition(Condition::At {
            index: 0,
            offset: Operand::Entrypoint,
        })];
    let data = [0x4Du8, 0x5A, 0x90, 0x00, 0xCC];
    // 命中恰在入口 → True
    let report = SigScan::new(&rules).scan_with_entrypoint(&data, false, Some(0));
    assert_eq!(report.rules[0].value, Ternary::True);
    // 入口在 1：首次命中在 0 → False
    let report = SigScan::new(&rules).scan_with_entrypoint(&data, false, Some(1));
    assert_eq!(report.rules[0].value, Ternary::False);
    // 无上下文（非可执行文件/解析失败）→ Unknown
    let report = SigScan::new(&rules).scan_with_entrypoint(&data, false, None);
    assert_eq!(
        report.rules[0].value,
        Ternary::Unknown,
        "缺上下文不假未命中"
    );
    // scan() 无上下文委托，同 Unknown
    let report = SigScan::new(&rules).scan(&data, false);
    assert_eq!(report.rules[0].value, Ternary::Unknown);
    // Cmp 侧：entrypoint == 0 → True（filesize 同通道的上下文操作数）
    let cmp = [Rule::new("c").condition(Condition::Cmp {
        lhs: Operand::Entrypoint,
        op: CmpOp::Eq,
        rhs: Operand::Lit(0),
    })];
    let report = SigScan::new(&cmp).scan_with_entrypoint(&data, false, Some(0));
    assert_eq!(report.rules[0].value, Ternary::True);
    let report = SigScan::new(&cmp).scan_with_entrypoint(&data, false, None);
    assert_eq!(report.rules[0].value, Ternary::Unknown);
}
