//! 表达式语言 D 项：字符串函数 `contains icontains startswith istartswith
//! endswith iendswith iequals matches` 与字符串比较。期望值 = 官方
//! yara-python 4.5.4 的结论（复算：`scripts/yara_expr_probe.py`）。

#[path = "support/yara_expr_support.rs"]
mod yara_expr_support;

use trit_core::value::Ternary;
use yara_expr_support::{cond, rejected};

fn assert_all(table: &[&str], expected: Ternary) {
    for condition in table {
        assert_eq!(cond(condition), expected, "{condition}");
    }
}

const FUNC_HIT: &[&str] = &[
    r#""hello" contains "ell""#,
    r#""hello" icontains "ELL""#,
    // 空针恒真，连空串对空针也是
    r#""hello" contains """#,
    r#""" contains """#,
    r#""hello" startswith "he""#,
    r#""hello" istartswith "HE""#,
    r#""hello" startswith """#,
    r#""hello" endswith "lo""#,
    r#""hello" iendswith "LO""#,
    r#""hello" endswith """#,
    r#""hello" iequals "HELLO""#,
    r#""" iequals """#,
    // 按字节比：内嵌 NUL 不截断
    r#""a\x00b" contains "\x00b""#,
    r#""a\x00b" endswith "\x00b""#,
    r#""a\x00b" == "a\x00b""#,
    r#""\xc4\x80" contains "\x80""#,
    r#""a\tb" contains "\t""#,
    r#""a\"b" contains "\"""#,
    r#""a\\b" contains "\\""#,
    r#""\x41" == "A""#,
    r#"not "a" contains "b""#,
    r#""abc" startswith "a" and "abc" endswith "c""#,
    r#"("abc") contains ("b")"#,
];

const FUNC_MISS: &[&str] = &[
    r#""hello" contains "ELL""#,
    r#""" contains "a""#,
    r#""a" contains "aa""#,
    r#""hello" startswith "HE""#,
    r#""hello" startswith "hello!""#,
    r#""hello" endswith "LO""#,
    r#""hello" endswith "xhello""#,
    r#""hello" iequals "HELL""#,
    r#""a\x00b" contains "\x00c""#,
    // 大小写折叠只管 ASCII：Latin-1 的 Ä / ä 不相等
    r#""\xc4" iequals "\xe4""#,
    r#""\xc4" icontains "\xe4""#,
    // `[` 与 `{`、`@` 与 `` ` `` 只差 0x20，但不是字母
    r#""[" iequals "{""#,
    r#""@" iequals "`""#,
];

#[test]
fn string_functions_compare_bytes_and_fold_ascii_only() {
    assert_all(FUNC_HIT, Ternary::True);
    assert_all(FUNC_MISS, Ternary::False);
}

// 字符串大小比较：官方按**有符号**字节比（高位字节排在 ASCII 之前）
const ORDER_HIT: &[&str] = &[
    r#""a" < "b""#,
    r#""a" < "ab""#,
    r#""" < "a""#,
    r#""a" <= "a""#,
    r#""b" > "a""#,
    r#""B" < "a""#,
    r#""\xff" < "a""#,
    r#""\x80" < "\x7f""#,
    r#""a\x00" > "a""#,
    r#""hello" == "hello""#,
    r#""hello" != "Hello""#,
];

const ORDER_MISS: &[&str] = &[
    r#""ab" < "a""#,
    r#""a" < "\xff""#,
    r#""\x80" > "\x7f""#,
    r#""hello" != "hello""#,
    r#""hello" == "Hello""#,
    r#""a\x00b" == "a\x00c""#,
];

#[test]
fn string_ordering_uses_signed_bytes_like_the_reference_build() {
    assert_all(ORDER_HIT, Ternary::True);
    assert_all(ORDER_MISS, Ternary::False);
}

const MATCHES_HIT: &[&str] = &[
    r#""hello" matches /ell/"#,
    r#""hello" matches /^hell/"#,
    r#""hello" matches /llo$/"#,
    r#""hello" matches /^hello$/"#,
    r#""hello" matches /HELLO/i"#,
    r#""hello" matches /h.llo/"#,
    r#""hello" matches /^/"#,
    r#""" matches /^$/"#,
    r#""" matches /a*/"#,
    r#""" matches /$/"#,
    r#""abc" matches /(a|z)c/ or "abc" matches /a.?c/"#,
    r#""abc" matches /[a-b]+c/"#,
    r#""abc" matches /a{1,2}b/"#,
    r#""abc" matches /\x61\w+/"#,
    r#""a/c" matches /a\/c/"#,
    r#""hello" matches /l{2}/"#,
    r#""hello" matches /h[^a-d]l/"#,
    r#""hello\x00world" matches /o\x00w/"#,
    r#""\xff\xfe" matches /[\x80-\xff]{2}/"#,
    r#""a b" matches /a\sb/"#,
    r#""he\nlo" matches /he\nlo/"#,
];

const MATCHES_MISS: &[&str] = &[
    r#""hello" matches /^ell/"#,
    r#""hello" matches /ll$/"#,
    r#""hello" matches /HELLO/"#,
    // 非空串上 `$` 不会在串尾之后再试一次
    r#""hello" matches /$/"#,
    r#""" matches /a/"#,
    r#""abc" matches /\d/"#,
    r#""hello" matches /l{3}/"#,
    // `.` 不吃换行
    r#""he\nlo" matches /he.lo/"#,
];

#[test]
fn matches_reuses_the_regex_engine_unanchored() {
    assert_all(MATCHES_HIT, Ternary::True);
    assert_all(MATCHES_MISS, Ternary::False);
}

const STR_REJECT: &[&str] = &[
    // 操作数类型：字符串函数两侧都得是字符串；matches 右侧得是正则
    r#"1 matches /a/"#,
    r#""abc" contains 1"#,
    r#"1 contains "a""#,
    r#""abc" contains /b/"#,
    r#""abc" matches "abc""#,
    r#""abc" matches (/b/)"#,
    r#""a" contains "a" contains "a""#,
    r#""a" == "a" == "a""#,
    r#""a" == 1"#,
    r#""a" < 1"#,
    // 官方不认的转义
    r#""a\qb" contains "q""#,
    r#""a\x4" contains "a""#,
    // 正则语法错
    r#""hello" matches /(/"#,
    r#""hello" matches /[z-a]/"#,
    r#""hello" matches /l{2,1}/"#,
];

// 官方能编译、本实现如实 Unsupported 的正则子集边界
const STR_GAP: &[&str] = &[r#""he\nlo" matches /he.lo/s"#, r#""ab" matches /a\Bb/"#];

#[test]
fn type_errors_and_unsupported_regex_reject_the_whole_rule() {
    for condition in STR_REJECT.iter().chain(STR_GAP) {
        let reason = rejected(condition);
        assert!(!reason.is_empty(), "{condition}");
    }
    assert!(rejected(r#""he\nlo" matches /he.lo/s"#).contains("/s"));
}

#[test]
fn string_functions_propagate_undefined_and_unknown() {
    // 这里没有 PE 上下文：pe.* 字符串 = 证据缺失 → 弃权，不折成假
    for (condition, expected) in [
        (r#"pe.sections[0].name contains "text""#, Ternary::Unknown),
        (
            r#"not (pe.sections[0].name contains "text")"#,
            Ternary::Unknown,
        ),
        (r#"pe.sections[0].name matches /text/"#, Ternary::Unknown),
        (
            r#"pe.sections[0].name contains "text" or true"#,
            Ternary::True,
        ),
        (
            r#"pe.sections[0].name contains "text" and false"#,
            Ternary::False,
        ),
        (
            r#"pe.version_info["CompanyName"] icontains "x""#,
            Ternary::Unknown,
        ),
    ] {
        assert_eq!(cond(condition), expected, "{condition}");
    }
}

#[test]
fn hostile_regexes_and_long_haystacks_stay_bounded() {
    let haystack = "a".repeat(4000);
    // 经典回溯炸弹：Pike VM 线性推进；超预算则弃权——不挂起
    for body in ["(a*)*b", "(a|aa)+b", "(a+)+$x", "a*a*a*a*a*a*b"] {
        let verdict = cond(&format!(r#""{haystack}" matches /{body}/"#));
        assert_ne!(verdict, Ternary::True, "{body}");
    }
    assert_eq!(
        cond(&format!(r#""{haystack}" matches /a+$/"#)),
        Ternary::True
    );
    // 循环里反复跑慢正则：预算按实际步数扣，耗尽即弃权
    let looped = format!(r#"for all i in (0..100000) : ("{haystack}" matches /(a*)*b/ or i >= 0)"#);
    assert_ne!(cond(&looped), Ternary::False);
    // 分组叠几万层：守卫拒绝（以前会让解析器爆栈）
    let deep = format!(
        r#""a" matches /{}a{}/"#,
        "(".repeat(50_000),
        ")".repeat(50_000)
    );
    assert!(rejected(&deep).contains("嵌套过深"));
    // 超大正则程序：编译期如实拒绝
    assert!(!rejected(&format!(r#""a" matches /{}/"#, "a?".repeat(5000))).is_empty());
}
