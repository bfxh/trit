//! Belnap FOUR bitplane verification.
//!
//! Encoding: N=(0,0), T=(1,0), F=(0,1), B=(1,1).
//! Meet/join use the truth order:
//!   AND: truth = ta & tb, falsity = fa | fb
//!   OR:  truth = ta | tb, falsity = fa & fb
//!   NOT: swap truth/falsity.

use crate::{measure3, report_unit};
use serde_json::Value;
use std::hint::black_box;

#[derive(Clone)]
struct Plane4 {
    truth: Vec<u64>,
    falsity: Vec<u64>,
}

impl Plane4 {
    fn zeros(words: usize) -> Self {
        Self {
            truth: vec![0; words],
            falsity: vec![0; words],
        }
    }

    fn random(words: usize, state: &mut u64) -> Self {
        let mut truth = vec![0u64; words];
        let mut falsity = vec![0u64; words];
        for i in 0..words {
            truth[i] = next(state);
            falsity[i] = next(state);
        }
        Self { truth, falsity }
    }

    fn filled(words: usize, truth: bool, falsity: bool) -> Self {
        Self {
            truth: vec![if truth { !0 } else { 0 }; words],
            falsity: vec![if falsity { !0 } else { 0 }; words],
        }
    }
}

fn next(state: &mut u64) -> u64 {
    let mut x = *state;
    x ^= x << 13;
    x ^= x >> 7;
    x ^= x << 17;
    *state = x;
    x
}

fn scalar_and(a: &Plane4, b: &Plane4, out: &mut Plane4, start: usize, end: usize) {
    for i in start..end {
        out.truth[i] = a.truth[i] & b.truth[i];
        out.falsity[i] = a.falsity[i] | b.falsity[i];
    }
}

fn scalar_or(a: &Plane4, b: &Plane4, out: &mut Plane4, start: usize, end: usize) {
    for i in start..end {
        out.truth[i] = a.truth[i] | b.truth[i];
        out.falsity[i] = a.falsity[i] & b.falsity[i];
    }
}

fn scalar_not(a: &Plane4, out: &mut Plane4, start: usize, end: usize) {
    for i in start..end {
        out.truth[i] = a.falsity[i];
        out.falsity[i] = a.truth[i];
    }
}

fn avx2_available() -> bool {
    is_x86_feature_detected!("avx2")
}

#[target_feature(enable = "avx2")]
unsafe fn avx2_and(a: &Plane4, b: &Plane4, out: &mut Plane4, words: usize) {
    use core::arch::x86_64::*;
    let full = words & !3;
    let mut i = 0;
    while i < full {
        let at = _mm256_loadu_si256(a.truth.as_ptr().add(i) as *const __m256i);
        let af = _mm256_loadu_si256(a.falsity.as_ptr().add(i) as *const __m256i);
        let bt = _mm256_loadu_si256(b.truth.as_ptr().add(i) as *const __m256i);
        let bf = _mm256_loadu_si256(b.falsity.as_ptr().add(i) as *const __m256i);
        let t = _mm256_and_si256(at, bt);
        let f = _mm256_or_si256(af, bf);
        _mm256_storeu_si256(out.truth.as_mut_ptr().add(i) as *mut __m256i, t);
        _mm256_storeu_si256(out.falsity.as_mut_ptr().add(i) as *mut __m256i, f);
        i += 4;
    }
    scalar_and(a, b, out, full, words);
}

#[target_feature(enable = "avx2")]
unsafe fn avx2_or(a: &Plane4, b: &Plane4, out: &mut Plane4, words: usize) {
    use core::arch::x86_64::*;
    let full = words & !3;
    let mut i = 0;
    while i < full {
        let at = _mm256_loadu_si256(a.truth.as_ptr().add(i) as *const __m256i);
        let af = _mm256_loadu_si256(a.falsity.as_ptr().add(i) as *const __m256i);
        let bt = _mm256_loadu_si256(b.truth.as_ptr().add(i) as *const __m256i);
        let bf = _mm256_loadu_si256(b.falsity.as_ptr().add(i) as *const __m256i);
        let t = _mm256_or_si256(at, bt);
        let f = _mm256_and_si256(af, bf);
        _mm256_storeu_si256(out.truth.as_mut_ptr().add(i) as *mut __m256i, t);
        _mm256_storeu_si256(out.falsity.as_mut_ptr().add(i) as *mut __m256i, f);
        i += 4;
    }
    scalar_or(a, b, out, full, words);
}

#[target_feature(enable = "avx2")]
unsafe fn avx2_not(a: &Plane4, out: &mut Plane4, words: usize) {
    use core::arch::x86_64::*;
    let full = words & !3;
    let mut i = 0;
    while i < full {
        let at = _mm256_loadu_si256(a.truth.as_ptr().add(i) as *const __m256i);
        let af = _mm256_loadu_si256(a.falsity.as_ptr().add(i) as *const __m256i);
        _mm256_storeu_si256(out.truth.as_mut_ptr().add(i) as *mut __m256i, af);
        _mm256_storeu_si256(out.falsity.as_mut_ptr().add(i) as *mut __m256i, at);
        i += 4;
    }
    scalar_not(a, out, full, words);
}

#[cfg(all(target_arch = "x86_64", target_os = "windows"))]
#[repr(C)]
struct Ctx4 {
    a_t: *const u64,
    a_f: *const u64,
    b_t: *const u64,
    b_f: *const u64,
    out_t: *mut u64,
    out_f: *mut u64,
    words: usize,
}

#[cfg(all(target_arch = "x86_64", target_os = "windows"))]
#[repr(C)]
struct Ctx4Not {
    a_t: *const u64,
    a_f: *const u64,
    out_t: *mut u64,
    out_f: *mut u64,
    words: usize,
}

#[cfg(all(target_arch = "x86_64", target_os = "windows"))]
core::arch::global_asm!(
    r#"
    .text

    .p2align 4
    .globl asm_b4_and
asm_b4_and:
    push rsi
    push rdi
    mov rax, rcx
    mov r8,  [rax+0]
    mov r9,  [rax+8]
    mov r10, [rax+16]
    mov r11, [rax+24]
    mov rdx, [rax+32]
    mov rcx, [rax+40]
    mov rsi, [rax+48]
    xor edi, edi
.Lb4and_loop:
    vmovdqu ymm0, [r8 + rdi*8]
    vmovdqu ymm1, [r9 + rdi*8]
    vmovdqu ymm2, [r10 + rdi*8]
    vmovdqu ymm3, [r11 + rdi*8]
    vpand   ymm4, ymm0, ymm2
    vpor    ymm0, ymm1, ymm3
    vmovdqu [rdx + rdi*8], ymm4
    vmovdqu [rcx + rdi*8], ymm0
    add rdi, 4
    cmp rdi, rsi
    jb .Lb4and_loop
    pop rdi
    pop rsi
    vzeroupper
    ret

    .p2align 4
    .globl asm_b4_or
asm_b4_or:
    push rsi
    push rdi
    mov rax, rcx
    mov r8,  [rax+0]
    mov r9,  [rax+8]
    mov r10, [rax+16]
    mov r11, [rax+24]
    mov rdx, [rax+32]
    mov rcx, [rax+40]
    mov rsi, [rax+48]
    xor edi, edi
.Lb4or_loop:
    vmovdqu ymm0, [r8 + rdi*8]
    vmovdqu ymm1, [r9 + rdi*8]
    vmovdqu ymm2, [r10 + rdi*8]
    vmovdqu ymm3, [r11 + rdi*8]
    vpor    ymm4, ymm0, ymm2
    vpand   ymm0, ymm1, ymm3
    vmovdqu [rdx + rdi*8], ymm4
    vmovdqu [rcx + rdi*8], ymm0
    add rdi, 4
    cmp rdi, rsi
    jb .Lb4or_loop
    pop rdi
    pop rsi
    vzeroupper
    ret

    .p2align 4
    .globl asm_b4_not
asm_b4_not:
    push rsi
    push rdi
    mov rax, rcx
    mov r8,  [rax+0]
    mov r9,  [rax+8]
    mov rdx, [rax+16]
    mov rcx, [rax+24]
    mov rsi, [rax+32]
    xor edi, edi
.Lb4not_loop:
    vmovdqu ymm0, [r8 + rdi*8]
    vmovdqu ymm1, [r9 + rdi*8]
    vmovdqu [rdx + rdi*8], ymm1
    vmovdqu [rcx + rdi*8], ymm0
    add rdi, 4
    cmp rdi, rsi
    jb .Lb4not_loop
    pop rdi
    pop rsi
    vzeroupper
    ret
    "#
);

#[cfg(all(target_arch = "x86_64", target_os = "windows"))]
extern "C" {
    fn asm_b4_and(ctx: *const Ctx4);
    fn asm_b4_or(ctx: *const Ctx4);
    fn asm_b4_not(ctx: *const Ctx4Not);
}

unsafe fn asm_and(a: &Plane4, b: &Plane4, out: &mut Plane4, words: usize) {
    let full = words & !3;
    #[cfg(all(target_arch = "x86_64", target_os = "windows"))]
    if full > 0 {
        let ctx = Ctx4 {
            a_t: a.truth.as_ptr(),
            a_f: a.falsity.as_ptr(),
            b_t: b.truth.as_ptr(),
            b_f: b.falsity.as_ptr(),
            out_t: out.truth.as_mut_ptr(),
            out_f: out.falsity.as_mut_ptr(),
            words: full,
        };
        asm_b4_and(&ctx);
    }
    #[cfg(all(target_arch = "x86_64", not(target_os = "windows")))]
    if full > 0 {
        avx2_and(a, b, out, full);
    }
    scalar_and(a, b, out, full, words);
}

unsafe fn asm_or(a: &Plane4, b: &Plane4, out: &mut Plane4, words: usize) {
    let full = words & !3;
    #[cfg(all(target_arch = "x86_64", target_os = "windows"))]
    if full > 0 {
        let ctx = Ctx4 {
            a_t: a.truth.as_ptr(),
            a_f: a.falsity.as_ptr(),
            b_t: b.truth.as_ptr(),
            b_f: b.falsity.as_ptr(),
            out_t: out.truth.as_mut_ptr(),
            out_f: out.falsity.as_mut_ptr(),
            words: full,
        };
        asm_b4_or(&ctx);
    }
    #[cfg(all(target_arch = "x86_64", not(target_os = "windows")))]
    if full > 0 {
        avx2_or(a, b, out, full);
    }
    scalar_or(a, b, out, full, words);
}

unsafe fn asm_not(a: &Plane4, out: &mut Plane4, words: usize) {
    let full = words & !3;
    #[cfg(all(target_arch = "x86_64", target_os = "windows"))]
    if full > 0 {
        let ctx = Ctx4Not {
            a_t: a.truth.as_ptr(),
            a_f: a.falsity.as_ptr(),
            out_t: out.truth.as_mut_ptr(),
            out_f: out.falsity.as_mut_ptr(),
            words: full,
        };
        asm_b4_not(&ctx);
    }
    #[cfg(all(target_arch = "x86_64", not(target_os = "windows")))]
    if full > 0 {
        avx2_not(a, out, full);
    }
    scalar_not(a, out, full, words);
}

fn check() {
    let values = [(false, false), (true, false), (false, true), (true, true)];
    let words = 64usize;
    for &a in &values {
        for &b in &values {
            let pa = Plane4::filled(words, a.0, a.1);
            let pb = Plane4::filled(words, b.0, b.1);
            let mut s = Plane4::zeros(words);
            let mut v = Plane4::zeros(words);
            let mut m = Plane4::zeros(words);

            unsafe {
                avx2_and(&pa, &pb, &mut v, words);
                asm_and(&pa, &pb, &mut m, words);
            }
            scalar_and(&pa, &pb, &mut s, 0, words);
            assert_eq!(s.truth, v.truth, "AND avx2 {a:?} {b:?}");
            assert_eq!(s.falsity, v.falsity, "AND avx2 {a:?} {b:?}");
            assert_eq!(s.truth, m.truth, "AND asm {a:?} {b:?}");
            assert_eq!(s.falsity, m.falsity, "AND asm {a:?} {b:?}");

            unsafe {
                avx2_or(&pa, &pb, &mut v, words);
                asm_or(&pa, &pb, &mut m, words);
            }
            scalar_or(&pa, &pb, &mut s, 0, words);
            assert_eq!(s.truth, v.truth, "OR avx2 {a:?} {b:?}");
            assert_eq!(s.falsity, v.falsity, "OR avx2 {a:?} {b:?}");
            assert_eq!(s.truth, m.truth, "OR asm {a:?} {b:?}");
            assert_eq!(s.falsity, m.falsity, "OR asm {a:?} {b:?}");

            unsafe {
                avx2_not(&pa, &mut v, words);
                asm_not(&pa, &mut m, words);
            }
            scalar_not(&pa, &mut s, 0, words);
            assert_eq!(s.truth, v.truth, "NOT avx2 {a:?}");
            assert_eq!(s.falsity, v.falsity, "NOT avx2 {a:?}");
            assert_eq!(s.truth, m.truth, "NOT asm {a:?}");
            assert_eq!(s.falsity, m.falsity, "NOT asm {a:?}");
        }
    }

    let lengths = [
        0usize, 1, 2, 3, 4, 5, 7, 8, 9, 15, 16, 17, 31, 32, 33, 63, 64, 65, 127, 128, 129, 1000,
        4096,
    ];
    let mut state = 0x1234_5678_9abc_def0;
    for &n in &lengths {
        let a = Plane4::random(n, &mut state);
        let b = Plane4::random(n, &mut state);
        let mut s = Plane4::zeros(n);
        let mut v = Plane4::zeros(n);
        let mut m = Plane4::zeros(n);
        unsafe {
            avx2_and(&a, &b, &mut v, n);
            asm_and(&a, &b, &mut m, n);
        }
        scalar_and(&a, &b, &mut s, 0, n);
        assert_eq!(s.truth, v.truth, "AND avx2 len {n}");
        assert_eq!(s.falsity, v.falsity, "AND avx2 len {n}");
        assert_eq!(s.truth, m.truth, "AND asm len {n}");
        assert_eq!(s.falsity, m.falsity, "AND asm len {n}");

        unsafe {
            avx2_or(&a, &b, &mut v, n);
            asm_or(&a, &b, &mut m, n);
        }
        scalar_or(&a, &b, &mut s, 0, n);
        assert_eq!(s.truth, v.truth, "OR avx2 len {n}");
        assert_eq!(s.falsity, v.falsity, "OR avx2 len {n}");
        assert_eq!(s.truth, m.truth, "OR asm len {n}");
        assert_eq!(s.falsity, m.falsity, "OR asm len {n}");

        unsafe {
            avx2_not(&a, &mut v, n);
            asm_not(&a, &mut m, n);
        }
        scalar_not(&a, &mut s, 0, n);
        assert_eq!(s.truth, v.truth, "NOT avx2 len {n}");
        assert_eq!(s.falsity, v.falsity, "NOT avx2 len {n}");
        assert_eq!(s.truth, m.truth, "NOT asm len {n}");
        assert_eq!(s.falsity, m.falsity, "NOT asm len {n}");
    }
    println!("belnap4 correctness: 16 pairs + 23 lengths ok");
}

fn bench_size(words: usize, iters: usize) {
    let mut state = 0xdead_beef_cafe_f00d;
    let a = Plane4::random(words, &mut state);
    let b = Plane4::random(words, &mut state);
    let bytes = (words as u128) * 8 * 6;
    let mut out_s = Plane4::zeros(words);
    let mut out_v = Plane4::zeros(words);
    let mut out_m = Plane4::zeros(words);

    println!(
        "\nbelnap4: {words} words ({:.1} KiB/plane), iters={iters}",
        (words * 8) as f64 / 1024.0
    );
    println!(
        "{:<14} {:>10} {:>10} {:>8} {:>7}",
        "kernel", "cyc/word", "ns/word", "GiB/s", "vs scalar"
    );

    let r = measure3(
        &mut || {
            scalar_and(&a, &b, &mut out_s, 0, words);
            black_box(out_s.truth[0]);
        },
        &mut || unsafe {
            avx2_and(&a, &b, &mut out_v, words);
            black_box(out_v.truth[0]);
        },
        &mut || unsafe {
            asm_and(&a, &b, &mut out_m, words);
            black_box(out_m.truth[0]);
        },
        iters,
    );
    report_unit("scalar_and", words, bytes, r[0].0, r[0].0, r[0].1, "word");
    report_unit("avx2_int_and", words, bytes, r[0].0, r[1].0, r[1].1, "word");
    report_unit("asm_and", words, bytes, r[0].0, r[2].0, r[2].1, "word");

    let r = measure3(
        &mut || {
            scalar_or(&a, &b, &mut out_s, 0, words);
            black_box(out_s.truth[0]);
        },
        &mut || unsafe {
            avx2_or(&a, &b, &mut out_v, words);
            black_box(out_v.truth[0]);
        },
        &mut || unsafe {
            asm_or(&a, &b, &mut out_m, words);
            black_box(out_m.truth[0]);
        },
        iters,
    );
    report_unit("scalar_or", words, bytes, r[0].0, r[0].0, r[0].1, "word");
    report_unit("avx2_int_or", words, bytes, r[0].0, r[1].0, r[1].1, "word");
    report_unit("asm_or", words, bytes, r[0].0, r[2].0, r[2].1, "word");

    let r = measure3(
        &mut || {
            scalar_not(&a, &mut out_s, 0, words);
            black_box(out_s.truth[0]);
        },
        &mut || unsafe {
            avx2_not(&a, &mut out_v, words);
            black_box(out_v.truth[0]);
        },
        &mut || unsafe {
            asm_not(&a, &mut out_m, words);
            black_box(out_m.truth[0]);
        },
        iters,
    );
    report_unit("scalar_not", words, bytes, r[0].0, r[0].0, r[0].1, "word");
    report_unit("avx2_int_not", words, bytes, r[0].0, r[1].0, r[1].1, "word");
    report_unit("asm_not", words, bytes, r[0].0, r[2].0, r[2].1, "word");
}

pub fn run() {
    println!("\n== belnap4: 4-valued bitplanes ==");
    if !avx2_available() {
        println!("AVX2 not available; skipping Belnap4 checks");
        return;
    }
    check();
    for &(words, iters) in &[(1usize << 14, 100usize), (1usize << 20, 50usize)] {
        bench_size(words, iters);
    }
}

fn belnap_bits(s: &str) -> Option<(bool, bool)> {
    match s {
        "N" => Some((false, false)),
        "T" => Some((true, false)),
        "F" => Some((false, true)),
        "B" => Some((true, true)),
        _ => None,
    }
}

fn plane4_is(p: &Plane4, truth: bool, falsity: bool) -> bool {
    let t = if truth { !0 } else { 0 };
    let f = if falsity { !0 } else { 0 };
    p.truth.iter().all(|&x| x == t) && p.falsity.iter().all(|&x| x == f)
}

pub(crate) fn conformance() -> bool {
    let path =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../interop/vectors/belnap4.json");
    let text = match std::fs::read_to_string(&path) {
        Ok(t) => t,
        Err(e) => {
            eprintln!("belnap4 conformance: cannot read {}: {e}", path.display());
            return false;
        }
    };
    let root: Value = match serde_json::from_str(&text) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("belnap4 conformance: invalid JSON: {e}");
            return false;
        }
    };
    let vectors = match root.get("vectors").and_then(Value::as_array) {
        Some(v) => v,
        None => {
            eprintln!("belnap4 conformance: missing vectors");
            return false;
        }
    };

    let mut pass = 0usize;
    let mut fail = 0usize;
    for v in vectors {
        let id = v.get("id").and_then(Value::as_str).unwrap_or("<missing>");
        let op = v.get("op").and_then(Value::as_str).unwrap_or("");
        let expected = match v
            .get("expected")
            .and_then(Value::as_str)
            .and_then(belnap_bits)
        {
            Some(x) => x,
            None => {
                fail += 1;
                eprintln!("FAIL {id}: bad expected");
                continue;
            }
        };
        let input = &v["input"];
        let mut s = Plane4::zeros(4);
        let mut vp = Plane4::zeros(4);
        let mut m = Plane4::zeros(4);
        let ok = match op {
            "not" => {
                let a = match input.as_str().and_then(belnap_bits) {
                    Some(x) => x,
                    None => {
                        fail += 1;
                        eprintln!("FAIL {id}: bad input");
                        continue;
                    }
                };
                let a = Plane4::filled(4, a.0, a.1);
                scalar_not(&a, &mut s, 0, 4);
                if avx2_available() {
                    unsafe {
                        avx2_not(&a, &mut vp, 4);
                        asm_not(&a, &mut m, 4);
                    }
                }
                plane4_is(&s, expected.0, expected.1)
                    && (!avx2_available()
                        || (plane4_is(&vp, expected.0, expected.1)
                            && plane4_is(&m, expected.0, expected.1)))
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
                let a = match arr[0].as_str().and_then(belnap_bits) {
                    Some(x) => x,
                    None => {
                        fail += 1;
                        eprintln!("FAIL {id}: bad input[0]");
                        continue;
                    }
                };
                let b = match arr[1].as_str().and_then(belnap_bits) {
                    Some(x) => x,
                    None => {
                        fail += 1;
                        eprintln!("FAIL {id}: bad input[1]");
                        continue;
                    }
                };
                let a = Plane4::filled(4, a.0, a.1);
                let b = Plane4::filled(4, b.0, b.1);
                if op == "and" {
                    scalar_and(&a, &b, &mut s, 0, 4);
                } else {
                    scalar_or(&a, &b, &mut s, 0, 4);
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
                plane4_is(&s, expected.0, expected.1)
                    && (!avx2_available()
                        || (plane4_is(&vp, expected.0, expected.1)
                            && plane4_is(&m, expected.0, expected.1)))
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
    println!("belnap4 asm conformance: {pass} pass, {fail} fail");
    fail == 0
}
