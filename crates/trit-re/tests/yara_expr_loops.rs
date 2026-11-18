//! 表达式语言 B 项：`for <量词> <变量> in <迭代器> : ( 体 )`——区间 / 枚举 /
//! 嵌套 / 变量作用域 / 整数体的"原值累加"语义。期望值 = 官方 yara-python
//! 4.5.4 的结论（复算：`scripts/yara_expr_probe.py`）。

#[path = "support/yara_expr_support.rs"]
mod yara_expr_support;

use trit_core::value::Ternary;
use yara_expr_support::{SAMPLE, cond, eval, rejected};

fn assert_all(table: &[&str], expected: Ternary) {
    for condition in table {
        assert_eq!(cond(condition), expected, "{condition}");
    }
}

const RANGE_HIT: &[&str] = &[
    "for any i in (0..3) : (i == 2)",
    "for all i in (0..3) : (i < 4)",
    "for none i in (0..3) : (i == 4)",
    "for 2 i in (0..3) : (i > 1)",
    "for 0 i in (0..3) : (i > 5)",
    // 两端都含：单点区间迭代一次
    "for any i in (3..3) : (i == 3)",
    "for all i in (0..filesize - 1) : (uint8(i) != 0x7a)",
    "for any i in (0..filesize - 1) : (uint8(i) == 0x77)",
    // 上端含 filesize：最后一次读越界 = 未定义 = 该轮不算真
    "for 13 i in (0..filesize) : (defined uint8(i))",
    "for any i in (0..filesize) : (not defined uint8(i))",
    "for all i in (0 - filesize..0) : (i <= 0)",
    "for 14 i in (0 - filesize..0) : (i <= 0)",
    "for (filesize - 13) i in (0..3) : (i > 5)",
    "not for any i in (0..3) : (i == 4)",
    // 零次迭代的循环是**确定的假**（不是未定义）：取反为真、defined 为真
    "not for any i in (filesize..3) : (true)",
    "not (for all i in (uint8(100)..3) : (true))",
    "defined (for all i in (uint8(100)..3) : (true))",
    // 运行期才知道的量词：未定义 / 负数都"至少 N 次"恒成立
    "for uint8(100) i in (0..3) : (i >= 0)",
    "for (filesize - 14) i in (0..3) : (false)",
    "for any i in (0..3) : (i == 1) and for any j in (0..3) : (j == 2)",
    "for any i in (0..3) : (i == 9) or true",
];

const RANGE_MISS: &[&str] = &[
    "for any i in (0..3) : (i == 4)",
    "for all i in (0..3) : (i < 3)",
    "for none i in (0..3) : (i == 3)",
    "for 3 i in (0..3) : (i > 1)",
    "for 0 i in (0..3) : (i > 2)",
    "for all i in (0..filesize) : (uint8(i) != 0x7a)",
    "for all i in (0..filesize) : (defined uint8(i))",
    "for 14 i in (0..filesize) : (defined uint8(i))",
    "for 15 i in (0 - filesize..0) : (i <= 0)",
    // 运行期才知道是空区间：零次迭代 = 假（连 all / none 也是）
    "for any i in (filesize..3) : (true)",
    "for all i in (filesize..3) : (true)",
    "for none i in (filesize..3) : (false)",
    // 端点未定义：同样是零次迭代 = 假
    "for all i in (0..uint8(100)) : (true)",
    "for none i in (0..uint8(100)) : (false)",
    "for all i in (uint8(100)..3) : (true)",
    "for (filesize - 14) i in (filesize..3) : (false)",
];

#[test]
fn range_loops_use_inclusive_bounds_and_reference_quantifier_rules() {
    assert_all(RANGE_HIT, Ternary::True);
    assert_all(RANGE_MISS, Ternary::False);
}

const ENUM_HIT: &[&str] = &[
    "for any i in (1, 2, 3) : (i == 2)",
    "for all i in (1, 2, 3) : (i > 0)",
    "for any i in (1) : (i == 1)",
    "for any i in (filesize, uint8(100), 5) : (i == 5)",
    "for 2 i in (filesize, uint8(100), 5) : (defined i)",
    "for any i in (uint8(100)) : (not defined i)",
    r#"for any s in ("a", "b") : (s == "b")"#,
    r#"for all s in ("a", "ab") : (s startswith "a")"#,
    r#"for any s in ("abc") : (s matches /b/)"#,
    r#"for any s in ("", "a") : (s)"#,
    r#"for 2 s in ("a", "b") : (s)"#,
];

const ENUM_MISS: &[&str] = &[
    "for all i in (filesize, uint8(100), 5) : (defined i)",
    "for 3 i in (filesize, uint8(100), 5) : (defined i)",
    "for all i in (uint8(100)) : (true) and false",
    r#"for all s in ("a", "") : (s)"#,
    r#"for any s in ("a", "b") : (s == "c")"#,
];

#[test]
fn enumeration_loops_bind_integers_and_strings() {
    assert_all(ENUM_HIT, Ternary::True);
    assert_all(ENUM_MISS, Ternary::False);
}

const NESTED_HIT: &[&str] = &[
    "for any i in (0..1) : (for any j in (0..1) : (i + j == 2))",
    "for all i in (0..1) : (for any j in (0..1) : (i + j == 1))",
    // 内层区间用外层变量
    "for any i in (0..1) : (for any j in (i..1) : (j == 1))",
    "for all i in (1..3) : (for i j in (1..3) : (j <= i))",
    "for any a in (0..0) : (for any b in (0..0) : (for any c in (0..0) : \
     (for any d in (0..0) : (a + b + c + d == 0))))",
    // 同名变量在并列（不嵌套）的两个循环里各用各的
    "for any i in (0..1) : (i == 1) and for any i in (5..6) : (i == 6)",
];

const NESTED_MISS: &[&str] = &[
    "for all i in (0..1) : (for all j in (0..1) : (i + j < 2))",
    "for any i in (0..1) : (for all j in (0..1) : (i + j == 1))",
];

#[test]
fn nested_loops_scope_their_variables() {
    assert_all(NESTED_HIT, Ternary::True);
    assert_all(NESTED_MISS, Ternary::False);
}

// 体是整数表达式：官方把体的**原值**加进"真"计数（不是按真假记 1）
const RAW_HIT: &[&str] = &[
    "for all i in (1..1) : (i)",
    "for any i in (0..1) : (1 - i * 2)",
    "for none i in (0..1) : (i * 4 - 2)",
    "for 3 i in (5..9) : (i)",
    "for 35 i in (5..9) : (i)",
    "for any i in (0..3) : (uint8(i) & 1)",
    "for none i in (0..0) : (i)",
    r#"for any i in (0..3) : ("a")"#,
];

const RAW_MISS: &[&str] = &[
    "for all i in (2..2) : (i)",
    "for 36 i in (5..9) : (i)",
    "for all i in (1..3) : (i)",
    "for all i in (0..1) : (i)",
    "for any i in (0..1) : (i * 2 - 1)",
    "for all i in (0..1) : (uint8(100 + i) + 2 - 2 * i)",
    "for all i in (0..3) : (uint8(i) & 1)",
    r#"for any i in (0..3) : ("")"#,
];

#[test]
fn integer_bodies_add_their_raw_value_to_the_count() {
    assert_all(RAW_HIT, Ternary::True);
    assert_all(RAW_MISS, Ternary::False);
}

const LOOP_REJECT: &[&str] = &[
    // 常量区间：下界 > 上界、下界为负，官方都是编译错误
    "for any i in (4..3) : (true)",
    "for any i in (-1..3) : (true)",
    "for any i in (0 - 1..1) : (i == -1)",
    "for -1 i in (0..3) : (true)",
    r#"for "a" i in (0..3) : (true)"#,
    r#"for any i in (1, "a") : (true)"#,
    "for any i in (true) : (true)",
    "for any i in () : (true)",
    "for any i, j in (1..2) : (true)",
    // 内层遮蔽外层同名变量
    "for any i in (0..1) : (for any i in (0..1) : (true))",
    // 变量出了循环就没了
    "for any i in (0..1) : (true) and i == 0",
    "i == 0",
    "for any i in (0..i) : (true)",
    "for any i in (0..1) : i == 1",
    "for any i (0..1) : (i == 1)",
    "for any in (0..1) : (true)",
    "for i in (0..1) : (true)",
    "for any filesize in (0..1) : (true)",
    "for any i in (0..1) : ()",
    r#"for any i in ("a".."b") : (true)"#,
    // 官方循环嵌套上限 4 层
    "for any a in (0..0) : (for any b in (0..0) : (for any c in (0..0) : \
     (for any d in (0..0) : (for any e in (0..0) : (true)))))",
];

#[test]
fn malformed_loops_reject_the_whole_rule() {
    for condition in LOOP_REJECT {
        let reason = rejected(condition);
        assert!(!reason.is_empty(), "{condition}");
    }
    assert!(rejected("for any i in (0..1) : (true) and i == 0").contains('i'));
}

#[test]
fn string_loops_mix_with_variable_loops() {
    // `l` 命中 2、3、9；`o` 命中 4、7；`zzz` 无命中
    let strings = r#"$a = "l" $b = "zzz" $c = "o""#;
    // probe: strings=$a = "l" $b = "zzz" $c = "o" | touch= and #a + #b + #c >= 0
    for (condition, expected) in [
        ("for all i in (1..#a) : (@a[i] >= 2)", Ternary::True),
        ("for any i in (1..#a) : (@a[i] == 9)", Ternary::True),
        ("for all i in (1..#a) : (@a[i] < 9)", Ternary::False),
        ("for all i in (1..#a + 1) : (defined @a[i])", Ternary::False),
        // 零命中的串：区间 (1..0) 运行期为空 = 假
        ("for all i in (1..#b) : (true)", Ternary::False),
        (
            "for any i in (1..3) : (for any of them : (@[i] == 9))",
            Ternary::True,
        ),
        (
            "for any of them : ($ and for any i in (1..#) : (@[i] == 7))",
            Ternary::True,
        ),
        (
            "for all of ($a, $c) : (for all i in (1..#) : (uint8(@[i]) > 0x60))",
            Ternary::True,
        ),
        (
            "for any i in (0..filesize) : ($a at i and $c at i + 1)",
            Ternary::True,
        ),
        (
            "for any i in (0..filesize) : ($c at i and $a at i + 1)",
            Ternary::False,
        ),
        ("for 2 i in (0..filesize) : ($c at i)", Ternary::True),
        ("for 3 i in (0..filesize) : ($c at i)", Ternary::False),
    ] {
        let full = format!("({condition}) and #a + #b + #c >= 0");
        assert_eq!(eval(strings, &full, SAMPLE), expected, "{condition}");
    }
}

#[test]
fn runaway_loops_abstain_instead_of_spinning() {
    // 跑不完的循环：预算耗尽 = 弃权（官方会真的跑完）——既不挂起也不猜
    for condition in [
        "for all i in (0..9223372036854775806) : (i >= 0)",
        "for all a in (0..1000) : (for all b in (0..1000) : (for all c in (0..1000) : \
         (a + b + c >= 0)))",
        "for none i in (0..1000000000) : (i < 0)",
    ] {
        assert_eq!(cond(condition), Ternary::Unknown, "{condition}");
    }
    // 能提前定案的不受影响：any 找到就停、all 遇假就停
    assert_eq!(
        cond("for any i in (0..9223372036854775806) : (i == 5)"),
        Ternary::True
    );
    assert_eq!(
        cond("for all i in (0..9223372036854775806) : (i < 5)"),
        Ternary::False
    );
}
