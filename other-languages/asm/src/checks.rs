//! 正确性检查：K3 真值表、随机长度、汇编/内建路径一致性。
//!
//! 从 `main.rs` 拆出（模块预算 ≤800 行）；逻辑未改动。

use crate::{
    Plane, asm_and, asm_not, asm_or, avx2_and, avx2_available, avx2_not, avx2_or, scalar_and,
    scalar_not, scalar_or,
};
use serde_json::Value;

fn expected_and(a: (bool, bool), b: (bool, bool)) -> (bool, bool) {
    if a == (false, false) || b == (false, false) {
        (false, false)
    } else if a == (true, false) && b == (true, false) {
        (true, false)
    } else {
        (false, true)
    }
}

fn expected_or(a: (bool, bool), b: (bool, bool)) -> (bool, bool) {
    if a == (true, false) || b == (true, false) {
        (true, false)
    } else if a == (false, false) && b == (false, false) {
        (false, false)
    } else {
        (false, true)
    }
}

fn expected_not(a: (bool, bool)) -> (bool, bool) {
    match a {
        (false, false) => (true, false),
        (true, false) => (false, false),
        (false, true) => (false, true),
        _ => panic!("invalid trit"),
    }
}

fn all_bits(plane: &Plane, want: (bool, bool)) -> bool {
    let o = if want.0 { !0 } else { 0 };
    let u = if want.1 { !0 } else { 0 };
    plane.ones.iter().all(|&x| x == o) && plane.unk.iter().all(|&x| x == u)
}

pub(crate) fn check_truth_tables() {
    if !avx2_available() {
        println!("AVX2 not available; skipping AVX2/asm correctness checks");
        return;
    }
    let states = [(false, false), (false, true), (true, false)];
    let words = 64usize;
    for &a in &states {
        for &b in &states {
            let pa = Plane::filled(words, a.0, a.1);
            let pb = Plane::filled(words, b.0, b.1);

            let mut s = Plane::zeros(words);
            let mut v = Plane::zeros(words);
            let mut m = Plane::zeros(words);

            unsafe {
                scalar_and(&pa, &pb, &mut s, words);
                avx2_and(&pa, &pb, &mut v, words);
                asm_and(&pa, &pb, &mut m, words);
            }
            let e = expected_and(a, b);
            assert!(all_bits(&s, e), "scalar AND {a:?} {b:?}");
            assert_eq!(s.ones, v.ones, "avx2 AND {a:?} {b:?}");
            assert_eq!(s.unk, v.unk, "avx2 AND {a:?} {b:?}");
            assert_eq!(s.ones, m.ones, "asm AND {a:?} {b:?}");
            assert_eq!(s.unk, m.unk, "asm AND {a:?} {b:?}");

            unsafe {
                scalar_or(&pa, &pb, &mut s, words);
                avx2_or(&pa, &pb, &mut v, words);
                asm_or(&pa, &pb, &mut m, words);
            }
            let e = expected_or(a, b);
            assert!(all_bits(&s, e), "scalar OR {a:?} {b:?}");
            assert_eq!(s.ones, v.ones, "avx2 OR {a:?} {b:?}");
            assert_eq!(s.unk, v.unk, "avx2 OR {a:?} {b:?}");
            assert_eq!(s.ones, m.ones, "asm OR {a:?} {b:?}");
            assert_eq!(s.unk, m.unk, "asm OR {a:?} {b:?}");

            unsafe {
                scalar_not(&pa, &mut s, words);
                avx2_not(&pa, &mut v, words);
                asm_not(&pa, &mut m, words);
            }
            let e = expected_not(a);
            assert!(all_bits(&s, e), "scalar NOT {a:?}");
            assert_eq!(s.ones, v.ones, "avx2 NOT {a:?}");
            assert_eq!(s.unk, v.unk, "avx2 NOT {a:?}");
            assert_eq!(s.ones, m.ones, "asm NOT {a:?}");
            assert_eq!(s.unk, m.unk, "asm NOT {a:?}");
        }
    }
    println!("truth tables: 9 pairs x 3 ops ok");
}

pub(crate) fn check_lengths() {
    if !avx2_available() {
        println!("AVX2 not available; skipping AVX2/asm correctness checks");
        return;
    }
    let lengths = [
        0usize, 1, 2, 3, 4, 5, 7, 8, 9, 15, 16, 17, 31, 32, 33, 63, 64, 65, 127, 128, 129, 1000,
        4096,
    ];
    let mut state = 0x1234_5678_9abc_def0;
    for &words in &lengths {
        let a = Plane::random(words, &mut state);
        let b = Plane::random(words, &mut state);
        let mut s = Plane::zeros(words);
        let mut v = Plane::zeros(words);
        let mut m = Plane::zeros(words);

        unsafe {
            scalar_and(&a, &b, &mut s, words);
            avx2_and(&a, &b, &mut v, words);
            asm_and(&a, &b, &mut m, words);
        }
        assert_eq!(s.ones, v.ones, "AND avx2 len {words}");
        assert_eq!(s.unk, v.unk, "AND avx2 len {words}");
        assert_eq!(s.ones, m.ones, "AND asm len {words}");
        assert_eq!(s.unk, m.unk, "AND asm len {words}");

        unsafe {
            scalar_or(&a, &b, &mut s, words);
            avx2_or(&a, &b, &mut v, words);
            asm_or(&a, &b, &mut m, words);
        }
        assert_eq!(s.ones, v.ones, "OR avx2 len {words}");
        assert_eq!(s.unk, v.unk, "OR avx2 len {words}");
        assert_eq!(s.ones, m.ones, "OR asm len {words}");
        assert_eq!(s.unk, m.unk, "OR asm len {words}");

        unsafe {
            scalar_not(&a, &mut s, words);
            avx2_not(&a, &mut v, words);
            asm_not(&a, &mut m, words);
        }
        assert_eq!(s.ones, v.ones, "NOT avx2 len {words}");
        assert_eq!(s.unk, v.unk, "NOT avx2 len {words}");
        assert_eq!(s.ones, m.ones, "NOT asm len {words}");
        assert_eq!(s.unk, m.unk, "NOT asm len {words}");
    }
    println!("random lengths: {} ok", lengths.len());
}

fn k3_bits(s: &str) -> Option<(bool, bool)> {
    match s {
        "F" => Some((false, false)),
        "T" => Some((true, false)),
        "U" => Some((false, true)),
        _ => None,
    }
}

fn plane_is(p: &Plane, one: bool, unknown: bool) -> bool {
    let o = if one { !0 } else { 0 };
    let u = if unknown { !0 } else { 0 };
    p.ones.iter().all(|&x| x == o) && p.unk.iter().all(|&x| x == u)
}

pub(crate) fn k3_conformance() -> bool {
    let path =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../interop/vectors/k3.json");
    let text = match std::fs::read_to_string(&path) {
        Ok(t) => t,
        Err(e) => {
            eprintln!("k3 conformance: cannot read {}: {e}", path.display());
            return false;
        }
    };
    let root: Value = match serde_json::from_str(&text) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("k3 conformance: invalid JSON: {e}");
            return false;
        }
    };
    let vectors = match root.get("vectors").and_then(Value::as_array) {
        Some(v) => v,
        None => {
            eprintln!("k3 conformance: missing vectors");
            return false;
        }
    };

    let mut pass = 0usize;
    let mut fail = 0usize;
    for v in vectors {
        let id = v.get("id").and_then(Value::as_str).unwrap_or("<missing>");
        let op = v.get("op").and_then(Value::as_str).unwrap_or("");
        let expected = match v.get("expected").and_then(Value::as_str).and_then(k3_bits) {
            Some(x) => x,
            None => {
                fail += 1;
                eprintln!("FAIL {id}: bad expected");
                continue;
            }
        };
        let input = &v["input"];
        let mut s = Plane::zeros(4);
        let mut vp = Plane::zeros(4);
        let mut m = Plane::zeros(4);
        let ok = match op {
            "not" => {
                let a = match input.as_str().and_then(k3_bits) {
                    Some(x) => x,
                    None => {
                        fail += 1;
                        eprintln!("FAIL {id}: bad input");
                        continue;
                    }
                };
                let a = Plane::filled(4, a.0, a.1);
                scalar_not(&a, &mut s, 4);
                if avx2_available() {
                    unsafe {
                        avx2_not(&a, &mut vp, 4);
                        asm_not(&a, &mut m, 4);
                    }
                }
                plane_is(&s, expected.0, expected.1)
                    && (!avx2_available()
                        || (plane_is(&vp, expected.0, expected.1)
                            && plane_is(&m, expected.0, expected.1)))
            }
            "and" | "or" => {
                let arr = match input.as_array() {
                    Some(a) => a,
                    None => {
                        fail += 1;
                        eprintln!("FAIL {id}: bad input");
                        continue;
                    }
                };
                if arr.len() != 2 {
                    fail += 1;
                    eprintln!("FAIL {id}: input length {}", arr.len());
                    continue;
                }
                let a = match arr[0].as_str().and_then(k3_bits) {
                    Some(x) => x,
                    None => {
                        fail += 1;
                        eprintln!("FAIL {id}: bad input[0]");
                        continue;
                    }
                };
                let b = match arr[1].as_str().and_then(k3_bits) {
                    Some(x) => x,
                    None => {
                        fail += 1;
                        eprintln!("FAIL {id}: bad input[1]");
                        continue;
                    }
                };
                let a = Plane::filled(4, a.0, a.1);
                let b = Plane::filled(4, b.0, b.1);
                if op == "and" {
                    scalar_and(&a, &b, &mut s, 4);
                } else {
                    scalar_or(&a, &b, &mut s, 4);
                }
                if avx2_available() {
                    unsafe {
                        if op == "and" {
                            avx2_and(&a, &b, &mut vp, 4);
                            asm_and(&a, &b, &mut m, 4);
                        } else {
                            avx2_or(&a, &b, &mut vp, 4);
                            asm_or(&a, &b, &mut m, 4);
                        }
                    }
                }
                plane_is(&s, expected.0, expected.1)
                    && (!avx2_available()
                        || (plane_is(&vp, expected.0, expected.1)
                            && plane_is(&m, expected.0, expected.1)))
            }
            _ => false,
        };
        if ok {
            pass += 1;
        } else {
            fail += 1;
            eprintln!("FAIL {id}");
        }
    }
    println!("k3 asm conformance: {pass} pass, {fail} fail");
    fail == 0
}
