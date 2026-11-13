//! 求值器单测：手工拼树，覆盖解析器不会产出、但求值器必须自洽的角落
//! （与官方的对拍在 `tests/yara_expr*.rs`）。

use super::eval::{Env, FUEL, V};
use super::*;
use crate::sigscan::eval::EvalCtx;

fn int(value: i64) -> Box<Expr> {
    Box::new(Expr::Int(value))
}

fn text(bytes: &[u8]) -> Box<Expr> {
    Box::new(Expr::Str(bytes.to_vec()))
}

fn arith(op: Arith, lhs: Box<Expr>, rhs: Box<Expr>) -> Box<Expr> {
    Box::new(Expr::Arith { op, lhs, rhs })
}

fn cmp(op: Rel, lhs: Box<Expr>, rhs: Box<Expr>) -> Expr {
    Expr::Cmp { op, lhs, rhs }
}

/// 在 `data` 上求值；`occ` / `partial` = 每条串的命中表与"没扫完"标记。
fn with<T>(
    data: &[u8],
    occ: &[(String, Vec<u64>)],
    partial: &[bool],
    body: impl FnOnce(&mut Env) -> T,
) -> T {
    let tree = PeLazy::new(data, false, false);
    let ctx = EvalCtx {
        data,
        truncated: false,
        binding: None,
        entrypoint: None,
        pe: None,
        tree: &tree,
        partial,
    };
    body(&mut Env::new(occ, &ctx))
}

fn verdict(expr: &Expr) -> Ternary {
    let tree = PeLazy::new(b"hello", false, false);
    let ctx = EvalCtx {
        data: b"hello",
        truncated: false,
        binding: None,
        entrypoint: None,
        pe: None,
        tree: &tree,
        partial: &[],
    };
    run(expr, &[], &ctx)
}

#[test]
fn top_level_undefined_is_a_miss_and_unknown_stays_unknown() {
    let read = |at: i64| Expr::Read {
        size: 1,
        signed: false,
        big_endian: false,
        at: int(at),
    };
    assert_eq!(verdict(&read(0)), Ternary::True); // 0x68 非零
    assert_eq!(verdict(&read(99)), Ternary::False); // 越界 = 未定义 = 不命中
    assert_eq!(verdict(&Expr::Entrypoint), Ternary::Unknown); // 没给上下文
    assert_eq!(verdict(&Expr::Not(Box::new(read(99)))), Ternary::False);
    assert_eq!(verdict(&Expr::Defined(Box::new(read(99)))), Ternary::False);
    assert_eq!(
        verdict(&Expr::Not(Box::new(Expr::Entrypoint))),
        Ternary::Unknown
    );
}

#[test]
fn sentinel_value_reads_as_undefined() {
    // 官方用 0xFFFABADAFABADAFF 当"未定义"：算出这个数的表达式就是未定义
    let sentinel = arith(Arith::Add, int(num::UNDEFINED - 1), int(1));
    assert_eq!(
        verdict(&cmp(Rel::Eq, sentinel.clone(), sentinel.clone())),
        Ternary::False
    );
    assert_eq!(verdict(&Expr::Defined(sentinel)), Ternary::False);
}

#[test]
fn string_comparison_is_signed_bytewise() {
    for (op, a, b, expected) in [
        (Rel::Lt, &b"a"[..], &b"b"[..], true),
        (Rel::Lt, b"a", b"ab", true),
        (Rel::Lt, b"\xff", b"a", true),
        (Rel::Gt, b"\x80", b"\x7f", false),
        (Rel::Eq, b"a\0b", b"a\0b", true),
        (Rel::Ne, b"a\0b", b"a\0c", true),
        (Rel::Ge, b"", b"", true),
    ] {
        let expected = if expected {
            Ternary::True
        } else {
            Ternary::False
        };
        assert_eq!(
            verdict(&cmp(op, text(a), text(b))),
            expected,
            "{a:?} {op:?} {b:?}"
        );
    }
    // 类型对不上到不了这里（解析期拒绝）；真到了也不猜
    assert_eq!(verdict(&cmp(Rel::Eq, text(b"a"), int(1))), Ternary::Unknown);
}

#[test]
fn nocase_matcher_closes_byte_sets_over_ascii_case() {
    let upper = Matcher::compile("HELLO", true).expect("可编译");
    let matches = |re: &Matcher, hay: &[u8]| strfunc::matches(re, hay);
    assert!(matches!(matches(&upper, b"hello"), V::Bool(true)));
    assert!(matches!(matches(&upper, b"HeLLo"), V::Bool(true)));
    let exact = Matcher::compile("HELLO", false).expect("可编译");
    assert!(matches!(matches(&exact, b"hello"), V::Bool(false)));
    // 只折 ASCII 字母：`[` 与 `{` 差 0x20 但不是字母
    let bracket = Matcher::compile(r"\[", true).expect("可编译");
    assert!(matches!(matches(&bracket, b"{"), V::Bool(false)));
}

#[test]
fn fuel_runs_out_into_unknown_not_into_a_verdict() {
    let forever = Expr::ForIn {
        quant: Quant::All,
        vars: (0, None),
        iter: Iter::Range(int(0), int(i64::MAX - 1)),
        body: Box::new(Expr::Bool(true)),
        raw: false,
    };
    assert_eq!(verdict(&forever), Ternary::Unknown);
    // 能提前定案的不烧完预算
    let early = Expr::ForIn {
        quant: Quant::Any,
        vars: (0, None),
        iter: Iter::Range(int(0), int(i64::MAX - 1)),
        body: Box::new(cmp(Rel::Eq, Box::new(Expr::Var(0)), int(3))),
        raw: false,
    };
    assert_eq!(verdict(&early), Ternary::True);
    with(b"", &[], &[], |env| {
        assert_eq!(env.fuel, FUEL);
        assert!(env.burn(FUEL - 1));
        assert!(!env.burn(1));
        assert!(matches!(env.eval(&Expr::Bool(true)), V::Unknown));
    });
}

#[test]
fn regex_work_is_charged_by_steps() {
    let slow = Matcher::compile("(a*)*b", false).expect("可编译");
    let hay = vec![b'a'; 200];
    with(b"", &[], &[], |env| {
        let before = env.fuel;
        let out = env.metered(|_| strfunc::matches(&slow, &hay));
        assert!(matches!(out, V::Bool(false)));
        let spent = before - env.fuel;
        assert!(
            spent > hay.len() as u64,
            "慢正则应按步数扣费，实际只扣 {spent}"
        );
        // 余额不够：弃权，而且不会把余额"跑成负数"再继续
        env.fuel = 10;
        let out = env.metered(|_| strfunc::matches(&slow, &hay));
        assert!(matches!(out, V::Unknown));
        assert!(env.fuel <= 10);
    });
}

#[test]
fn partial_hit_tables_abstain_on_absence_only() {
    let occ = [
        ("a".to_string(), vec![4, 9]),
        ("b".to_string(), Vec::new()),
        ("c".to_string(), Vec::new()),
    ];
    // a、b 没扫完；c 扫完了
    with(b"", &occ, &[true, true, false], |env| {
        let (a, b, c) = (Sid::Named(0), Sid::Named(1), Sid::Named(2));
        assert!(matches!(env.found(a), Ok(V::Bool(true))));
        assert!(matches!(env.found(b), Ok(V::Unknown)));
        assert!(matches!(env.found(c), Ok(V::Bool(false))));
        assert!(matches!(env.found_at(a, 9), Ok(V::Bool(true))));
        assert!(matches!(env.found_at(a, 5), Ok(V::Unknown)));
        assert!(matches!(env.found_at(c, 5), Ok(V::Bool(false))));
        assert!(matches!(env.found_at(b, -1), Ok(V::Bool(false))));
        assert!(matches!(env.found_in(a, 0, 4), Ok(V::Bool(true))));
        assert!(matches!(env.found_in(a, 5, 8), Ok(V::Unknown)));
        assert!(matches!(env.found_in(a, 8, 5), Ok(V::Bool(false))));
        assert!(matches!(env.eval(&Expr::Count(a)), V::Unknown));
        assert!(matches!(env.eval(&Expr::Count(c)), V::Int(0)));
        let nth = |sid, n| Expr::Offset { sid, nth: int(n) };
        assert!(matches!(env.eval(&nth(a, 2)), V::Int(9)));
        assert!(matches!(env.eval(&nth(a, 3)), V::Unknown));
        assert!(matches!(env.eval(&nth(c, 1)), V::Undef));
        assert!(matches!(env.eval(&nth(a, 0)), V::Undef));
        // 串下标越界 / 循环外的匿名串：不猜
        assert!(matches!(env.found(Sid::Named(7)), Err(V::Unknown)));
        assert!(matches!(env.found(Sid::Current), Err(V::Unknown)));
    });
}
