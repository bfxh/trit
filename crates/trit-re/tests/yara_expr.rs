//! 表达式语言 A 项：算术 / 位运算 / 一元负号 / 括号 / 官方优先级，以及
//! 未定义值与三值纪律。每张表里的期望值都是官方 yara-python 4.5.4 在同一段
//! 13 字节样本上给的结论（`docs/YARA-EXPR.md`「对拍」一节记了复算办法）。

#[path = "support/yara_expr_support.rs"]
mod yara_expr_support;

use trit_core::value::Ternary;
use yara_expr_support::{SAMPLE, cond, eval, parse, rejected};

fn assert_all(table: &[&str], expected: Ternary) {
    for condition in table {
        assert_eq!(cond(condition), expected, "{condition}");
    }
}

const ARITH_HIT: &[&str] = &[
    "1 + 2 == 3",
    "filesize + 1 == 14",
    "filesize - 14 == -1",
    "filesize * 2 == 26",
    r"filesize \ 2 == 6",
    "filesize % 5 == 3",
    r"7 \ 2 == 3",
    r"(0 - 7) \ 2 == 0 - 3",
    "(0 - 7) % 2 == 0 - 1",
    "7 % (0 - 2) == 1",
    "- - 1 == 1",
    "-(-(1)) == 1",
    "~0 == -1",
    "~~5 == 5",
    "-~1 == 2",
    "~-1 == 0",
    "6 & 3 == 2",
    "6 | 3 == 7",
    "6 ^ 3 == 5",
    "1 << 4 == 16",
    "256 >> 4 == 16",
    "(0 - 8) >> 1 == 0 - 4",
    "0x7fffffffffffffff + 0 > 0",
    "1KB == 1024",
    "1MB == 1048576",
    "0o17 == 15",
    "uint8(0) + uint8(1) == 0x68 + 0x65",
    "uint16be(0) == (uint8(0) << 8 | uint8(1))",
    "uint32(0) == (uint8(0) | uint8(1) << 8 | uint8(2) << 16 | uint8(3) << 24)",
    "int8(filesize - 1) == -1",
    "uint8(uint8(12) - 255) == uint8(0)",
];

const ARITH_MISS: &[&str] = &[
    "1 + 2 == 4",
    "filesize + 1 == 13",
    r"filesize \ 2 == 7",
    "~0 == 0",
    "1 << 4 == 8",
    "(0 - 1) >> 1 > 0",
    "int8(filesize - 1) == 255",
];

#[test]
fn arithmetic_and_bitwise_operators_evaluate() {
    assert_all(ARITH_HIT, Ternary::True);
    assert_all(ARITH_MISS, Ternary::False);
}

// 优先级（低 → 高）：or、and、not、== !=、< <= > >=、|、^、&、<< >>、+ -、* \ %、~ 一元负
const PRECEDENCE_HIT: &[&str] = &[
    "2 + 3 * 4 == 14",
    "(2 + 3) * 4 == 20",
    "1 | 2 ^ 3 & 4 == 3",
    "1 + 2 << 1 == 6",
    "7 & 3 + 1 == 4",
    "8 >> 1 + 1 == 2",
    "10 - 4 - 3 == 3",
    r"100 \ 10 \ 5 == 2",
    "2 * 3 % 4 == 2",
    "7 % 4 * 2 == 6",
    "-2 * 3 == -6",
    "~1 & 3 == 2",
    "1 == 1 and 2 == 2 or 1 == 2",
    "1 == 2 or 1 == 1 and 2 == 2",
    "not 1 == 2",
    "not 1 == 2 and true",
    "not false or false",
];

const PRECEDENCE_MISS: &[&str] = &[
    "2 + 3 * 4 == 20",
    "1 + 2 << 1 == 5",
    "10 - 4 - 3 == 9",
    "not 1 == 1 or false",
    "not (true or true) and true",
];

#[test]
fn operator_precedence_and_associativity_follow_the_reference_grammar() {
    assert_all(PRECEDENCE_HIT, Ternary::True);
    assert_all(PRECEDENCE_MISS, Ternary::False);
}

// 64 位补码回绕（常量折叠里的溢出官方是编译错误——见下面的 REJECT 表）
const WRAP_HIT: &[&str] = &[
    "9223372036854775807 + filesize - filesize + 1 < 0",
    "(filesize - filesize + 9223372036854775807) * 2 == 0 - 2",
    "(filesize - filesize - 9223372036854775807) - 2 > 0",
    "1 << 63 < 0",
    "1 << 62 > 0",
    "1 << (filesize - filesize + 64) == 0",
    "1 << (filesize - filesize + 65) == 0",
    "1 >> (filesize - filesize + 64) == 0",
    "(0 - 1) >> (filesize - filesize + 1) < 0",
    "1 << 64 == 0",
    "1 >> 64 == 0",
    "filesize * filesize >= filesize",
];

#[test]
fn runtime_overflow_wraps_and_oversized_shifts_give_zero() {
    assert_all(WRAP_HIT, Ternary::True);
}

// 官方是编译错误的写法：整条规则 Unsupported（带原因），不是求值期的假
const ARITH_REJECT: &[&str] = &[
    r"1 \ 0 == 1",
    "1 % 0 == 1",
    r"1 \ (1 - 1) == 1",
    "1 % (2 - 2) == 1",
    "1 << -1 == 0",
    "1 >> -1 == 0",
    "9223372036854775807 + 1 < 0",
    "9223372036854775807 * 2 == 0 - 2",
    "0 - 9223372036854775807 - 2 > 0",
    "1 +",
    "1 + == 2",
    r#""a" + 1 == 2"#,
    r#"1 + "a" == 2"#,
    r#"-"a" == 1"#,
    r#"~"a" == 1"#,
    r#"1 == "a""#,
    r#"uint8("a") + 0 == 1"#,
    "1 < 2 < 3",
    "1 < 2 == true",
    "true == 1 < 2",
    // 十六进制字面量超出 int64 正半轴：官方词法 / 语法错误
    "0xffffffffffffffff == -1",
    "true + 1 == 2",
    "nosuchname + 1 == 2",
    "$missing and 1 + 1 == 2",
];

// 官方能编译、本实现如实 Unsupported 的子集边界（浮点不在子集内）
const ARITH_GAP: &[&str] = &["1.5 + 1 > 2", "filesize + 0.5 > 1", "1.0 == 1"];

#[test]
fn floating_point_is_outside_the_subset_and_says_so() {
    for condition in ARITH_GAP {
        assert!(rejected(condition).contains("浮点"), "{condition}");
    }
}

#[test]
fn bare_integer_and_string_conditions_go_by_truthiness() {
    assert_all(
        &[
            "1 + 2",
            "filesize - 12",
            "0 - 1",
            r#""a""#,
            "uint8(0) & 0x08",
        ],
        Ternary::True,
    );
    assert_all(
        &[
            "1 - 1",
            "filesize - 13",
            r#""""#,
            "uint8(0) & 0x01",
            "uint8(100) + 1",
        ],
        Ternary::False,
    );
}

#[test]
fn compile_time_errors_reject_the_whole_rule_with_a_reason() {
    for condition in ARITH_REJECT {
        let reason = rejected(condition);
        assert!(!reason.is_empty(), "{condition}");
    }
    // 原因要说人话：除零 / 移位 / 溢出各有各的说法
    assert!(rejected(r"1 \ 0 == 1").contains("除"), "除零");
    assert!(rejected("1 << -1 == 0").contains("移位"), "负移位");
    assert!(
        rejected("9223372036854775807 + 1 < 0").contains("溢出"),
        "溢出"
    );
    // 多出来的 `)`：规则切分层就拒绝了（整份文件 Err），同样不会静默通过
    let unbalanced = yara_expr_support::rule("", "1 + 2 == 3)");
    assert!(trit_re::yaraparse::parse_rules(&unbalanced).is_err());
}

#[test]
fn division_by_zero_at_runtime_is_undefined_not_an_error() {
    // 除数只有运行期才知道是 0：官方得到未定义，比较为假
    assert_all(
        &[
            r"1 \ (filesize - filesize) == 1",
            "1 % (filesize - filesize) == 1",
            r"defined (1 \ (filesize - filesize))",
            "1 << (filesize - filesize - 1) == 0",
            "defined (1 << (filesize - filesize - 1))",
            "1 >> (filesize - filesize - 1) == 0",
            // INT64_MIN \ -1：官方结果未定义（这里不能溢出 panic）
            r"(0 - 9223372036854775807 - 1) \ (filesize - filesize - 1) == 0",
            "defined ((0 - 9223372036854775807 - 1) % (filesize - filesize - 1))",
        ],
        Ternary::False,
    );
    assert_eq!(
        cond("not defined (1 % (filesize - filesize))"),
        Ternary::True
    );
}

// 未定义（越界读）一路传上去：算术 / 比较都未定义，`not 未定义` 仍未定义；
// `and` / `or` 把未定义当假；`defined` 把它变回布尔
const UNDEFINED_HIT: &[&str] = &[
    "not defined uint8(100)",
    "not defined (uint8(100) + 1)",
    "not defined (uint8(100) == 1)",
    "not defined (not (uint8(100) == 1))",
    "uint8(100) == 1 or true",
    "not (uint8(100) == 1 or false)",
    "not (uint8(100) == 1 and true)",
    "defined (uint8(100) == 1 or false)",
    "defined uint8(filesize - 1)",
    "not defined uint8(filesize)",
    "not defined uint16(filesize - 1)",
    "not defined uint8(0 - 1)",
    "not defined uint8(9223372036854775807)",
    "defined filesize",
    "defined 0",
    "defined true",
    "defined false",
];

const UNDEFINED_MISS: &[&str] = &[
    "uint8(100) == 1",
    "uint8(100) != 1",
    "uint8(100) + 1 > 0",
    "uint8(100) * 0 == 0",
    "uint8(100) & 0 == 0",
    "not (uint8(100) == 1)",
    "not not (uint8(100) == 1)",
    "not not not (uint8(100) == 1)",
    "not (uint8(100) != 1)",
    "defined uint8(100)",
    "uint8(100) == uint8(100)",
    "not (uint8(100) == 1) or false",
];

#[test]
fn undefined_propagates_like_the_reference_engine() {
    assert_all(UNDEFINED_HIT, Ternary::True);
    assert_all(UNDEFINED_MISS, Ternary::False);
}

#[test]
fn the_undefined_sentinel_value_is_undefined_even_when_computed() {
    // 官方用 0xFFFABADAFABADAFF 这个整数表示"未定义"：算出它 = 算出未定义
    assert_all(
        &[
            "filesize - filesize - 0x5452505452501 == 0 - 0x5452505452501",
            "defined (filesize - filesize - 0x5452505452501)",
            "-0x5452505452501 < 0",
            "defined -0x5452505452501",
        ],
        Ternary::False,
    );
    assert_all(
        &[
            "filesize - filesize - 0x5452505452500 == 0 - 0x5452505452500",
            "defined (filesize - filesize - 0x5452505452500)",
        ],
        Ternary::True,
    );
    // 值为哨兵的常量除数：官方不报除零（它当未定义，停止折叠），规则照常编译
    let sentinel_divisor = r"1 \ (-0x5452505452501 + 0x5452505452501) == 1";
    assert_eq!(cond(sentinel_divisor), Ternary::False);
    assert!(rejected(r"1 \ (-0x5452505452500 + 0x5452505452500) == 1").contains("除"));
}

#[test]
fn truncated_input_abstains_instead_of_guessing() {
    // 输入被截断：filesize 与读数都不可信 → Unknown，绝不折成假
    let scan = |condition: &str| {
        let rule = parse("", condition).expect("可解析");
        trit_re::sigscan::SigScan::new(std::slice::from_ref(&rule))
            .scan(SAMPLE, true)
            .rules[0]
            .value
    };
    for condition in [
        "filesize + 1 == 14",
        "filesize + 1 == 15",
        "uint8(0) + 1 == 0x69",
        "uint8(100) + 1 == 1",
        "not defined uint8(100)",
        "defined (filesize + 0)",
    ] {
        assert_eq!(scan(condition), Ternary::Unknown, "{condition}");
    }
    // 不依赖输入的部分照常下结论；Kleene 短路仍然成立
    assert_eq!(scan("1 + 1 == 2"), Ternary::True);
    assert_eq!(scan("1 + 1 == 2 or filesize + 0 == 0"), Ternary::True);
    assert_eq!(scan("1 + 1 == 3 and filesize + 0 == 0"), Ternary::False);
    assert_eq!(scan("1 + 1 == 3"), Ternary::False);
    assert_eq!(scan("1 + 1 == 2 and filesize + 0 == 0"), Ternary::Unknown);
}

#[test]
fn incomplete_string_scans_never_prove_absence() {
    // 大于扫描预算的样本：只在尾部出现一次的串扫不到（命中表不完整）。
    // 官方在这份样本上：`$a` 命中、`not $a` / `#a == 0` 不命中。
    let mut data = vec![0u8; trit_re::sigscan::SCAN_BUDGET as usize + 4096];
    let at = data.len() - 16;
    data[at..at + 6].copy_from_slice(b"needle");
    data[..2].copy_from_slice(b"MZ");
    let scan = |strings: &str, condition: &str| eval(strings, condition, &data);
    let late = r#"$a = "needle""#;
    let early = r#"$a = "MZ""#;
    for (strings, condition, expected) in [
        // 没扫到 ≠ 没有：两个引擎都弃权（旧口径曾把后三条报成 HIT）
        (late, "$a", Ternary::Unknown),
        (late, "not $a", Ternary::Unknown),
        (late, "#a == 0", Ternary::Unknown),
        (late, "for all of them : (# == 0)", Ternary::Unknown),
        (late, "#a < 1", Ternary::Unknown),
        (late, "#a + 0 == 0", Ternary::Unknown),
        (late, "none of them", Ternary::Unknown),
        (late, "not $a and 1 + 1 == 2", Ternary::Unknown),
        (late, "not defined @a[1]", Ternary::Unknown),
        // 已经找到的就是找到了；再多命中也推翻不了的结论照常落账
        (early, "$a", Ternary::True),
        (early, "$a at 0", Ternary::True),
        (early, "#a > 0", Ternary::True),
        (early, "@a[1] + 0 == 0", Ternary::True),
        (early, "not $a or 1 + 1 == 3", Ternary::False),
        (early, "$a and 1 + 1 == 3", Ternary::False),
        (early, "none of them or 1 + 1 == 3", Ternary::False),
        // 旧求值器的既有口径更保守：不完整扫描上的假一律弃权（不放宽）
        (early, "not $a", Ternary::Unknown),
        (early, "#a < 1", Ternary::Unknown),
        // 找到了一次，但总数还不知道
        (early, "#a == 1", Ternary::Unknown),
        (early, "#a + 0 == 1", Ternary::Unknown),
        (early, "not defined @a[2]", Ternary::Unknown),
    ] {
        assert_eq!(
            scan(strings, condition),
            expected,
            "{strings} / {condition}"
        );
    }
}

#[test]
fn unknown_never_folds_to_false_through_boolean_operators() {
    // entrypoint 没有上下文 = 证据缺失（不是未定义）
    for (condition, expected) in [
        ("entrypoint + 0 == 0", Ternary::Unknown),
        ("not (entrypoint + 0 == 0)", Ternary::Unknown),
        ("defined (entrypoint + 0)", Ternary::Unknown),
        ("entrypoint + 0 == 0 and false", Ternary::False),
        ("entrypoint + 0 == 0 and true", Ternary::Unknown),
        ("entrypoint + 0 == 0 or true", Ternary::True),
        ("entrypoint + 0 == 0 or false", Ternary::Unknown),
        ("entrypoint * 0 == 0", Ternary::Unknown),
        // 未定义压过弃权：越界读让整个算术确定地未定义
        ("entrypoint + uint8(100) == 0", Ternary::False),
        (
            "for any i in (0..1) : (entrypoint + i == 0)",
            Ternary::Unknown,
        ),
        (
            "for all i in (0..1) : (entrypoint + i == 0 and i > 5)",
            Ternary::False,
        ),
    ] {
        assert_eq!(cond(condition), expected, "{condition}");
    }
}

#[test]
fn string_counts_and_offsets_take_part_in_arithmetic() {
    // `l` 命中 2、3、9；`o` 命中 4、7；`zzz` 无命中
    let strings = r#"$a = "l" $b = "zzz" $c = "o""#;
    let touch = " and #a + #b + #c >= 0";
    // probe: strings=$a = "l" $b = "zzz" $c = "o" | touch= and #a + #b + #c >= 0
    for (condition, expected) in [
        ("#a + #c == 5", Ternary::True),
        ("#a * 2 - #c == 4", Ternary::True),
        ("@a[1] + @a[2] == 5", Ternary::True),
        ("@a[#a] == 9", Ternary::True),
        ("@a[#a + 1] == 9", Ternary::False),
        ("not defined @a[#a + 1]", Ternary::True),
        ("not defined @b", Ternary::True),
        ("@b + 1 == 1", Ternary::False),
        ("#a in (0..3) == 2", Ternary::True),
        (
            "#a in (3..filesize) + #c in (5..filesize) == 3",
            Ternary::True,
        ),
        ("$a at @a[2]", Ternary::True),
        ("$a at @c - 1", Ternary::True),
        ("$a in (@c..@c + 5)", Ternary::True),
        ("$b at 0 + 0", Ternary::False),
        ("@a[0] == 2", Ternary::False),
        ("not defined @a[0]", Ternary::True),
        ("not defined @a[0 - 1]", Ternary::True),
    ] {
        let full = format!("({condition}){touch}");
        assert_eq!(eval(strings, &full, SAMPLE), expected, "{condition}");
    }
}
