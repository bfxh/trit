//! 深度与栈：条件解析器、求值器、正则编译器都是递归的。嵌套上限（括号 /
//! 一元算子 64 层，表达式树 256 层）保证它们在**小栈**上也跑得完——这里
//! 把每种能递归的形状顶到上限，放在 1 MiB 栈的线程上走完整条链路
//! （解析 → 克隆 → 求值 → 比较 → 析构）。debug 构建实测峰值 ≤ 512 KiB
//! （见 docs/YARA-EXPR.md「深度与栈」）；栈溢出是进程级 abort，所以这份
//! 测试一旦回归会直接打掉整个测试二进制，而不是报一条失败。

#[path = "support/yara_expr_support.rs"]
mod yara_expr_support;

use trit_core::value::Ternary;
use trit_re::sigscan::SigScan;
use trit_re::yaraparse::{RuleOutcome, parse_rules};
use yara_expr_support::{cond, rejected, rich_pe, rule};

const ONE: &str = r#"$a = "l""#;

/// (串区, 条件)：把 `open … close` 套 `depth` 层。
fn shape(kind: &str, depth: usize) -> (String, String) {
    let wrap = |open: &str, close: &str, core: &str| {
        format!("{}{core}{}", open.repeat(depth), close.repeat(depth))
    };
    let (strings, condition) = match kind {
        "paren" => ("", wrap("(", ")", "1 + 1 == 2")),
        "paren_legacy" => ("", wrap("(", ")", "true")),
        "or_and" => ("", wrap("(true and false or ", ")", "1 + 1 == 2")),
        "or_and_not" => ("", wrap("(false or true and not ", ")", "(1 + 1 == 3)")),
        "arith" => ("", format!("{} == 0", wrap("(1 + 2 * ", ")", "filesize"))),
        "cmp_rhs" => ("", format!("1 == {}", wrap("(1 | 2 + ", ")", "filesize"))),
        "neg" => ("", format!("{}filesize < 0", "- ".repeat(depth))),
        "bitnot" => ("", format!("{}filesize < 0", "~ ".repeat(depth))),
        "not" => ("", format!("{}(1 + 1 == 3)", "not ".repeat(depth))),
        "defined" => ("", format!("{}(1 + 1 == 3)", "defined ".repeat(depth))),
        "read" => ("", format!("{} == 0", wrap("uint8(1 + ", ")", "filesize"))),
        "index" => (
            "",
            format!(
                "{} == 0",
                wrap("pe.sections[1 + ", "].virtual_address", "filesize")
            ),
        ),
        "key" => (
            "",
            format!("{} == \"\"", wrap("pe.version_info[", "]", "\"a\"")),
        ),
        "call" => (
            "",
            format!("{} == 0", wrap("pe.rva_to_offset(1 + ", ")", "filesize")),
        ),
        "offset" => (ONE, format!("{} == 0", wrap("@a[1 + ", "]", "#a"))),
        "count_in" => (ONE, format!("{} == 0", wrap("#a in (0..1 + ", ")", "#a"))),
        "at" => (ONE, format!("$a at {}", wrap("(1 + ", ")", "#a"))),
        "of_at" => (ONE, format!("any of them at {}", wrap("(1 + ", ")", "#a"))),
        "loop_body" => (
            "",
            format!(
                "for any a in (0..1) : (for any b in (0..1) : (for any c in (0..1) : \
                 (for any e in (0..1) : ({}))))",
                wrap("(true and ", ")", "a + b + c + e >= 0")
            ),
        ),
        "chain" => ("", format!("{} > 0", vec!["filesize"; depth].join(" + "))),
        "regex" => ("", format!("\"a\" matches /{}/", wrap("(a|", ")*", "b"))),
        "regex_string" => (
            &*format!("$a = /{}/", wrap("(a|", ")*", "b")).leak(),
            "$a".to_string(),
        ),
        "legacy_not" => (ONE, format!("{}$a", "not ".repeat(depth))),
        // `for … of` 不能自己套自己（官方编译错误），深度只能叠在循环体的括号上
        "legacy_for" => (ONE, format!("for any of them : {}", wrap("(", ")", "$"))),
        other => panic!("未知形状 {other}"),
    };
    (strings.to_string(), condition)
}

/// 整条链路；返回"规则被接受了吗"。
fn pipeline(kind: &str, depth: usize) -> bool {
    let (strings, condition) = shape(kind, depth);
    let Ok(parsed) = parse_rules(&rule(&strings, &condition)) else {
        return false;
    };
    let RuleOutcome::Parsed(original) = &parsed.rules[0] else {
        return false;
    };
    let copy = original.clone();
    assert!(copy.condition_ref() == original.condition_ref(), "{kind}");
    let data = rich_pe();
    let info = trit_re::pe::parse(&data).ok();
    for rule in [original, &copy] {
        let scanner = SigScan::new(std::slice::from_ref(rule));
        scanner.scan(&data, false);
        scanner.scan_with_context(&data, false, None, info.as_ref());
    }
    true
}

/// (形状, 能被接受的最大深度)
const SHAPES: &[(&str, usize)] = &[
    ("paren", 63),
    ("paren_legacy", 64),
    ("or_and", 63),
    ("or_and_not", 31),
    ("arith", 63),
    ("cmp_rhs", 63),
    ("neg", 63),
    ("bitnot", 63),
    ("not", 62),
    ("defined", 62),
    ("read", 63),
    ("index", 63),
    ("key", 63),
    ("call", 63),
    ("offset", 63),
    ("count_in", 63),
    ("at", 63),
    ("of_at", 63),
    ("loop_body", 59),
    ("chain", 255),
    ("regex", 64),
    ("regex_string", 64),
    ("legacy_not", 64),
    ("legacy_for", 64),
];

#[test]
fn deepest_accepted_shapes_fit_in_a_one_mebibyte_stack() {
    let outcomes = std::thread::Builder::new()
        .stack_size(1024 * 1024)
        .spawn(|| {
            SHAPES
                .iter()
                .map(|(kind, depth)| (*kind, pipeline(kind, *depth), pipeline(kind, depth + 1)))
                .collect::<Vec<_>>()
        })
        .expect("起线程")
        .join()
        .expect("没有 panic");
    for (kind, at_limit, beyond) in outcomes {
        assert!(at_limit, "{kind}：上限处应被接受");
        // 串区的正则过深不拒规则（该串按"编不出来"弃权，沿用既有口径）
        assert_eq!(beyond, kind == "regex_string", "{kind}：过限一层");
    }
}

#[test]
fn absurd_depths_are_refused_without_recursing() {
    // 几万层：闸在递归之前（数 token），不靠"栈够大"
    std::thread::Builder::new()
        .stack_size(256 * 1024)
        .spawn(|| {
            for (kind, _) in SHAPES {
                let accepted = pipeline(kind, 50_000);
                assert_eq!(accepted, *kind == "regex_string", "{kind}");
            }
        })
        .expect("起线程")
        .join()
        .expect("没有 panic");
}

#[test]
fn limits_are_reported_by_name() {
    let wrap =
        |depth: usize, inner: &str| format!("{}{inner}{}", "(".repeat(depth), ")".repeat(depth));
    assert_eq!(cond(&wrap(64, "true")), Ternary::True);
    assert_eq!(cond(&wrap(63, "1 + 1 == 2")), Ternary::True);
    assert_eq!(cond(&format!("{}true", "not ".repeat(64))), Ternary::True);
    for condition in [
        wrap(65, "true"),
        wrap(65, "1 + 1 == 2"),
        format!("{}true", "not ".repeat(65)),
        format!("{}filesize < 0", "- ".repeat(65)),
        format!(
            "pe.sections{}0{}.name == \"a\"",
            "[".repeat(65),
            "]".repeat(65)
        ),
    ] {
        let reason = rejected(&condition);
        assert!(
            reason.contains("嵌套过深"),
            "{}…：{reason}",
            &condition[..24]
        );
    }
    // 不深但很长的左结合链：树深单独设限
    let chain = |terms: usize| format!("{} == {terms}", vec!["1"; terms].join(" + "));
    assert_eq!(cond(&chain(255)), Ternary::True);
    assert!(rejected(&chain(300)).contains("表达式过深"));
    // n 元 and / or 是扁平的，不占树深
    let flat = vec!["1 + 1 == 2"; 5000].join(" and ");
    assert_eq!(cond(&flat), Ternary::True);
    let deep = format!("\"a\" matches /{}a{}/", "(".repeat(65), ")".repeat(65));
    assert!(rejected(&deep).contains("嵌套过深"));
}
