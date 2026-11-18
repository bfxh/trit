//! 新旧条件引擎的分流（`yaraparse/xparse.rs::route`）：旧解析器真能求值的
//! 规则判据一字不动；它接不住 / 只能记名弃权 / 会把未定义取反的，才交给
//! 表达式引擎。这里逐条钉住"谁接了哪条规则"和接手之后的结论。

#[path = "support/yara_expr_support.rs"]
mod yara_expr_support;

use trit_core::value::Ternary;
use trit_re::Condition;
use trit_re::sigscan::SigScan;
use yara_expr_support::{SAMPLE, parse, rejected, rich_pe};

const ONE: &str = r#"$a = "l""#;

/// (走了表达式引擎吗, 无上下文裁决, 合成 PE 上的裁决)
fn routed(strings: &str, condition: &str) -> (bool, Ternary, Ternary) {
    let rule = parse(strings, condition).unwrap_or_else(|why| panic!("{condition}：{why}"));
    let scanner = SigScan::new(std::slice::from_ref(&rule));
    let data = rich_pe();
    let info = trit_re::pe::parse(&data).ok();
    let entry = info
        .as_ref()
        .and_then(|pe| pe.rva_to_offset(pe.entry_rva))
        .map(|offset| offset as u64);
    (
        matches!(rule.condition_ref(), Condition::Expr(_)),
        scanner.scan(SAMPLE, false).rules[0].value,
        scanner
            .scan_with_context(&data, false, entry, info.as_ref())
            .rules[0]
            .value,
    )
}

#[test]
fn legacy_subset_stays_on_the_legacy_engine() {
    // 规则 2：旧解析器能求值的形状一条不动（既有 194 条测试的判据面）
    for (strings, condition, plain) in [
        ("", "true", Ternary::True),
        ("", "filesize == 13", Ternary::True),
        ("", "filesize > 1KB", Ternary::False),
        ("", "uint8(0) == 0x68", Ternary::True),
        ("", "uint16(0) == 0x5A4D", Ternary::False),
        ("", "uint8(100) == 1", Ternary::False),
        (ONE, "$a", Ternary::True),
        (ONE, "not $a", Ternary::False),
        (ONE, "#a == 3", Ternary::True),
        (ONE, "#a > 1 and @a == 2", Ternary::True),
        (ONE, "@a[1] == 2", Ternary::True),
        (ONE, "$a at 2", Ternary::True),
        (ONE, "$a in (0..5)", Ternary::True),
        (ONE, "any of them", Ternary::True),
        (ONE, "all of ($a*)", Ternary::True),
        (ONE, "1 of them", Ternary::True),
        (ONE, "for any of them : ($)", Ternary::True),
        (ONE, "for all of them : (# > 1)", Ternary::True),
        (ONE, "for any of them : (@ > 1)", Ternary::True),
    ] {
        let (expr, verdict, _) = routed(strings, condition);
        assert!(!expr, "{condition} 应留在旧引擎");
        assert_eq!(verdict, plain, "{condition}");
    }
}

#[test]
fn legacy_pe_paths_resolve_and_stay_put() {
    // `xparse/legacy.rs` 的清单：这些路径旧求值器有实现，带 PE 上下文时
    // 给得出确定结论（值或确定的未定义），所以不转交
    // probe: sample=rich
    for (condition, on_pe) in [
        ("pe.machine == 0x8664", Ternary::True),
        ("pe.number_of_sections == 3", Ternary::True),
        ("pe.timestamp > 0", Ternary::True),
        ("pe.characteristics == 0x2022", Ternary::True),
        ("pe.entry_point == 0x410", Ternary::True),
        ("pe.overlay.size == 16", Ternary::True),
        ("pe.overlay.offset == 0xE00", Ternary::True),
        ("pe.is_dll()", Ternary::True),
        ("pe.is_dll() == 8192", Ternary::True),
        ("pe.is_32bit()", Ternary::False),
        ("entrypoint == 0x410", Ternary::True),
        (r#"pe.sections[0].name == ".text""#, Ternary::True),
        ("pe.sections[0].virtual_size == 0x1F0", Ternary::True),
        ("pe.sections[2].virtual_address == 0x3000", Ternary::True),
        (
            "pe.sections[0].characteristics == 0x60000020",
            Ternary::True,
        ),
        (r#"pe.sections[9].name == ".text""#, Ternary::False),
        // 合成 PE 没有 Rich 头：确定的未定义
        ("pe.rich_signature.key == 0", Ternary::False),
        ("pe.rich_signature.length == 0", Ternary::False),
        ("pe.rich_signature.offset == 0", Ternary::False),
    ] {
        let (expr, plain, verdict) = routed("", condition);
        assert!(!expr, "{condition} 应留在旧引擎");
        assert_eq!(plain, Ternary::Unknown, "{condition}：无 PE 上下文 = 弃权");
        assert_eq!(verdict, on_pe, "{condition}");
    }
    // 旧引擎自己的字段别名 / 宽松处（官方：invalid field name / wrong usage）。
    // 既有规则在用，判据不动；表达式引擎不认这些名字（见 YARA-EXPR.md）。
    let lenient = [
        "pe.sections[0].raw_size == 0x200",
        "pe.sections[0].raw_ptr == 0x400",
        "pe.rich_signature.version == 0",
    ];
    for (condition, on_pe) in lenient
        .iter()
        .zip([Ternary::True, Ternary::True, Ternary::False])
    {
        let (expr, _, verdict) = routed("", condition);
        assert!(!expr, "{condition} 应留在旧引擎");
        assert_eq!(verdict, on_pe, "{condition}");
    }
}

#[test]
fn expression_markers_always_take_the_new_engine() {
    // 规则 1：运算符 / 字符串函数 / defined / for…in
    for condition in [
        "1 + 1 == 2",
        "filesize \\ 2 == 6",
        "filesize % 2 == 1",
        "filesize & 1 == 1",
        "filesize | 1 == 13",
        "filesize ^ 1 == 12",
        "~filesize < 0",
        "filesize << 1 == 26",
        "filesize >> 1 == 6",
        r#""a" contains "a""#,
        r#""a" matches /a/"#,
        "defined filesize",
        "for any i in (0..1) : (i == 1)",
    ] {
        let (expr, verdict, _) = routed("", condition);
        assert!(expr, "{condition}");
        assert_eq!(verdict, Ternary::True, "{condition}");
    }
    // 新引擎拒绝就是拒绝：不退回旧解析器，也只报新引擎的原因
    let reason = rejected("filesize + 1 ==");
    assert!(reason.starts_with("表达式不支持："), "{reason}");
}

#[test]
fn conditions_the_legacy_parser_rejects_get_a_second_chance() {
    // 规则 3
    for (strings, condition, plain) in [
        ("", "filesize == -1", Ternary::False),
        ("", "int8(0) == 0x68", Ternary::True),
        ("", "uint32be(0) == 0x68656c6c", Ternary::True),
        (ONE, "none of them", Ternary::False),
        (ONE, "any of them at 2", Ternary::True),
        (ONE, "any of them in (0..5)", Ternary::True),
        (ONE, "#a in (0..5) == 2", Ternary::True),
        (ONE, "50% of them", Ternary::True),
        (ONE, "2 of ($a, $a)", Ternary::True),
    ] {
        let (expr, verdict, _) = routed(strings, condition);
        assert!(expr, "{condition}");
        assert_eq!(verdict, plain, "{condition}");
    }
    // 两边都不收：两个原因一并上报
    for condition in ["filesize < 1 < 2", "filesize ==", "1.5 > 1"] {
        let reason = rejected(condition);
        assert!(reason.contains("；表达式引擎："), "{condition}：{reason}");
    }
    assert!(rejected("1.5 > 1").contains("浮点"));
}

#[test]
fn named_abstentions_are_handed_over_when_the_new_engine_can_do_better() {
    // 规则 4：旧解析器只能记名弃权的路径，新引擎能解析就接手
    // probe: sample=rich
    for (condition, on_pe) in [
        ("pe.number_of_imports == 2", Ternary::True),
        (
            r#"pe.imports("KERNEL32.dll", "ExitProcess")"#,
            Ternary::True,
        ),
        ("pe.sections[0].raw_data_size == 0x200", Ternary::True),
        ("pe.machine == pe.MACHINE_AMD64", Ternary::True),
        ("pe.is_64bit()", Ternary::True),
        ("pe.is_pe", Ternary::True),
        (
            r#"pe.sections[pe.number_of_sections - 1].name == ".rsrc""#,
            Ternary::True,
        ),
        // 旧求值器只会字符串的 == / !=；两个路径比大小它恒弃权
        ("pe.sections[1].name < pe.sections[0].name", Ternary::True),
        ("pe.sections[0].name <= pe.sections[1].name", Ternary::False),
        // 接手了也还是弃权（子集外字段）：证据面不变
        ("pe.number_of_signatures == 0", Ternary::Unknown),
    ] {
        let (expr, plain, verdict) = routed("", condition);
        assert!(expr, "{condition}");
        assert_eq!(plain, Ternary::Unknown, "{condition}");
        assert_eq!(verdict, on_pe, "{condition}");
    }
    // 新引擎也不认的：维持旧的记名弃权（官方是编译错误——宽松处，见文档）
    for condition in [
        "pe.nope == 1",
        "pe.machine.x == 1",
        "math.entropy(0, 10) > 1",
        "uint64(0) == 1",
        "pe.sections[0].name == 5",
    ] {
        let (expr, plain, verdict) = routed("", condition);
        assert!(!expr, "{condition}");
        assert_eq!(
            (plain, verdict),
            (Ternary::Unknown, Ternary::Unknown),
            "{condition}"
        );
    }
}

#[test]
fn negated_possibly_undefined_comparisons_follow_the_reference() {
    // 规则 5：`not 未定义` 仍是未定义（官方 miss）；旧求值器会得出真
    for (strings, condition, plain, on_pe) in [
        ("", "not (uint8(100) == 1)", Ternary::False, Ternary::True),
        ("", "not (uint8(0) == 1)", Ternary::True, Ternary::True),
        (ONE, "not (@a[9] > 0)", Ternary::False, Ternary::False),
        (
            "",
            r#"not (pe.sections[9].name == ".text")"#,
            Ternary::Unknown,
            Ternary::False,
        ),
        ("", "not pe.is_dll()", Ternary::Unknown, Ternary::False),
    ] {
        let (expr, verdict, pe) = routed(strings, condition);
        assert!(expr, "{condition}");
        assert_eq!((verdict, pe), (plain, on_pe), "{condition}");
    }
    // 不带 not 的同一比较不受影响
    assert!(!routed("", "uint8(100) == 1").0);
    assert!(!routed(ONE, "not $a").0);
}

#[test]
fn escaped_string_literals_go_to_the_lexer_that_reads_them_right() {
    // 规则 6：旧词法层把 `"\x2etext"` 读成 `x2etext`
    let (expr, _, on_pe) = routed("", r#"pe.sections[0].name == "\x2etext""#);
    assert!(expr);
    assert_eq!(on_pe, Ternary::True);
    let (expr, _, on_pe) = routed("", r#"pe.sections[0].name == ".text""#);
    assert!(!expr);
    assert_eq!(on_pe, Ternary::True);
}

// probe: strings=$a = "l" | touch=
const NESTED_STRING_LOOPS_REJECT: &[&str] = &[
    "for any of them : ( for any of them : ( $ ) )",
    "for any of ($a*) : ( for all of ($a*) : ( # > 0 ) )",
    "$a and for all of them : ( not ( for any of them : ( $ at 2 ) ) )",
];

// probe: strings=$a = "l" | touch=
const MIXED_LOOPS_HIT: &[&str] = &[
    "for any of them : ( for any i in (0..3) : ( i == 2 and $ at i ) )",
    "for any i in (0..3) : ( for any of them : ( $ at i ) )",
];

#[test]
fn nested_string_loops_are_rejected_like_the_reference() {
    // 旧解析器收下 `for … of` 套 `for … of`（并报 HIT）；官方是编译错误
    for condition in NESTED_STRING_LOOPS_REJECT {
        let reason = parse(ONE, condition).expect_err(condition);
        assert!(reason.contains("不能嵌套"), "{condition}：{reason}");
    }
    // 两种循环互相套是合法的
    for condition in MIXED_LOOPS_HIT {
        let (expr, plain, _) = routed(ONE, condition);
        assert!(expr, "{condition}");
        assert_eq!(plain, Ternary::True, "{condition}");
    }
}
