//! Assembly verification for ternary/multi-valued kernels.
//!
//! First slice: K3 bit-sliced AND/OR/NOT on dual-rail `(ones, unknowns)`.
//! Compares:
//!   - scalar u64 baseline (compiler may auto-vectorize with SSE2)
//!   - Rust AVX2 intrinsics
//!   - hand-written AVX2 assembly (Windows x64; intrinsics fallback elsewhere)
//!
//! The assembly is a *binary* ISA implementation of ternary logic: it is B-class
//! emulation, not native ternary hardware. See `other-languages/README.md`.

use std::hint::black_box;
use std::time::Instant;

mod gf3;
mod pack5;

#[cfg(not(target_arch = "x86_64"))]
compile_error!("trit-asm-verify targets x86-64");

#[derive(Clone)]
struct Plane {
    ones: Vec<u64>,
    unk: Vec<u64>,
}

impl Plane {
    fn zeros(words: usize) -> Self {
        Self {
            ones: vec![0; words],
            unk: vec![0; words],
        }
    }

    fn random(words: usize, state: &mut u64) -> Self {
        let mut ones = vec![0u64; words];
        let mut unk = vec![0u64; words];
        for i in 0..words {
            let o = next_u64(state);
            let u = next_u64(state) & !o;
            ones[i] = o;
            unk[i] = u;
        }
        Self { ones, unk }
    }

    fn filled(words: usize, one: bool, unknown: bool) -> Self {
        assert!(!(one && unknown), "invalid trit (1,1)");
        let o = if one { !0 } else { 0 };
        let u = if unknown { !0 } else { 0 };
        Self {
            ones: vec![o; words],
            unk: vec![u; words],
        }
    }
}

fn next_u64(state: &mut u64) -> u64 {
    let mut x = *state;
    x ^= x << 13;
    x ^= x >> 7;
    x ^= x << 17;
    *state = x;
    x
}

#[inline(always)]
fn k3_and_bits(ao: u64, au: u64, bo: u64, bu: u64) -> (u64, u64) {
    let ones = ao & bo;
    let false_a = !(ao | au);
    let false_b = !(bo | bu);
    let unk = !(ones | false_a | false_b);
    (ones, unk)
}

#[inline(always)]
fn k3_or_bits(ao: u64, au: u64, bo: u64, bu: u64) -> (u64, u64) {
    let ones = ao | bo;
    let false_a = !(ao | au);
    let false_b = !(bo | bu);
    let unk = !(ones | (false_a & false_b));
    (ones, unk)
}

#[inline(always)]
fn k3_not_bits(ao: u64, au: u64) -> (u64, u64) {
    let ones = !(ao | au);
    (ones, au)
}

fn scalar_and_range(a: &Plane, b: &Plane, out: &mut Plane, start: usize, end: usize) {
    for i in start..end {
        let (o, u) = k3_and_bits(a.ones[i], a.unk[i], b.ones[i], b.unk[i]);
        out.ones[i] = o;
        out.unk[i] = u;
    }
}

fn scalar_or_range(a: &Plane, b: &Plane, out: &mut Plane, start: usize, end: usize) {
    for i in start..end {
        let (o, u) = k3_or_bits(a.ones[i], a.unk[i], b.ones[i], b.unk[i]);
        out.ones[i] = o;
        out.unk[i] = u;
    }
}

fn scalar_not_range(a: &Plane, out: &mut Plane, start: usize, end: usize) {
    for i in start..end {
        let (o, u) = k3_not_bits(a.ones[i], a.unk[i]);
        out.ones[i] = o;
        out.unk[i] = u;
    }
}

#[inline(never)]
fn scalar_and(a: &Plane, b: &Plane, out: &mut Plane, words: usize) {
    scalar_and_range(a, b, out, 0, words);
}

#[inline(never)]
fn scalar_or(a: &Plane, b: &Plane, out: &mut Plane, words: usize) {
    scalar_or_range(a, b, out, 0, words);
}

#[inline(never)]
fn scalar_not(a: &Plane, out: &mut Plane, words: usize) {
    scalar_not_range(a, out, 0, words);
}

fn avx2_available() -> bool {
    is_x86_feature_detected!("avx2")
}

#[target_feature(enable = "avx2")]
#[inline(never)]
unsafe fn avx2_and(a: &Plane, b: &Plane, out: &mut Plane, words: usize) {
    use core::arch::x86_64::*;
    let full = words & !3;
    let all = _mm256_set1_epi32(-1);
    let mut i = 0;
    while i < full {
        let ao = _mm256_loadu_si256(a.ones.as_ptr().add(i) as *const __m256i);
        let au = _mm256_loadu_si256(a.unk.as_ptr().add(i) as *const __m256i);
        let bo = _mm256_loadu_si256(b.ones.as_ptr().add(i) as *const __m256i);
        let bu = _mm256_loadu_si256(b.unk.as_ptr().add(i) as *const __m256i);
        let ones = _mm256_and_si256(ao, bo);
        let false_a = _mm256_andnot_si256(_mm256_or_si256(ao, au), all);
        let false_b = _mm256_andnot_si256(_mm256_or_si256(bo, bu), all);
        let unk = _mm256_andnot_si256(
            _mm256_or_si256(ones, _mm256_or_si256(false_a, false_b)),
            all,
        );
        _mm256_storeu_si256(out.ones.as_mut_ptr().add(i) as *mut __m256i, ones);
        _mm256_storeu_si256(out.unk.as_mut_ptr().add(i) as *mut __m256i, unk);
        i += 4;
    }
    scalar_and_range(a, b, out, full, words);
}

#[target_feature(enable = "avx2")]
#[inline(never)]
unsafe fn avx2_or(a: &Plane, b: &Plane, out: &mut Plane, words: usize) {
    use core::arch::x86_64::*;
    let full = words & !3;
    let all = _mm256_set1_epi32(-1);
    let mut i = 0;
    while i < full {
        let ao = _mm256_loadu_si256(a.ones.as_ptr().add(i) as *const __m256i);
        let au = _mm256_loadu_si256(a.unk.as_ptr().add(i) as *const __m256i);
        let bo = _mm256_loadu_si256(b.ones.as_ptr().add(i) as *const __m256i);
        let bu = _mm256_loadu_si256(b.unk.as_ptr().add(i) as *const __m256i);
        let ones = _mm256_or_si256(ao, bo);
        let false_a = _mm256_andnot_si256(_mm256_or_si256(ao, au), all);
        let false_b = _mm256_andnot_si256(_mm256_or_si256(bo, bu), all);
        let unk = _mm256_andnot_si256(
            _mm256_or_si256(ones, _mm256_and_si256(false_a, false_b)),
            all,
        );
        _mm256_storeu_si256(out.ones.as_mut_ptr().add(i) as *mut __m256i, ones);
        _mm256_storeu_si256(out.unk.as_mut_ptr().add(i) as *mut __m256i, unk);
        i += 4;
    }
    scalar_or_range(a, b, out, full, words);
}

#[target_feature(enable = "avx2")]
#[inline(never)]
unsafe fn avx2_not(a: &Plane, out: &mut Plane, words: usize) {
    use core::arch::x86_64::*;
    let full = words & !3;
    let all = _mm256_set1_epi32(-1);
    let mut i = 0;
    while i < full {
        let ao = _mm256_loadu_si256(a.ones.as_ptr().add(i) as *const __m256i);
        let au = _mm256_loadu_si256(a.unk.as_ptr().add(i) as *const __m256i);
        let ones = _mm256_andnot_si256(_mm256_or_si256(ao, au), all);
        _mm256_storeu_si256(out.ones.as_mut_ptr().add(i) as *mut __m256i, ones);
        _mm256_storeu_si256(out.unk.as_mut_ptr().add(i) as *mut __m256i, au);
        i += 4;
    }
    scalar_not_range(a, out, full, words);
}

#[cfg(all(target_arch = "x86_64", target_os = "windows"))]
#[repr(C)]
struct Ctx {
    a_ones: *const u64,
    a_unk: *const u64,
    b_ones: *const u64,
    b_unk: *const u64,
    out_ones: *mut u64,
    out_unk: *mut u64,
    words: usize,
}

#[cfg(all(target_arch = "x86_64", target_os = "windows"))]
#[repr(C)]
struct Ctx1 {
    a_ones: *const u64,
    a_unk: *const u64,
    out_ones: *mut u64,
    out_unk: *mut u64,
    words: usize,
}

#[cfg(all(target_arch = "x86_64", target_os = "windows"))]
core::arch::global_asm!(
    r#"
    .text

    .p2align 4
    .globl asm_k3_and
asm_k3_and:
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
    vpcmpeqd ymm5, ymm5, ymm5
.Lk3and_loop:
    vmovdqu ymm0, [r8 + rdi*8]
    vmovdqu ymm1, [r9 + rdi*8]
    vmovdqu ymm2, [r10 + rdi*8]
    vmovdqu ymm3, [r11 + rdi*8]
    vpand   ymm4, ymm0, ymm2
    vpor    ymm0, ymm0, ymm1
    vpandn  ymm0, ymm0, ymm5
    vpor    ymm1, ymm2, ymm3
    vpandn  ymm1, ymm1, ymm5
    vpor    ymm0, ymm0, ymm1
    vpor    ymm0, ymm0, ymm4
    vpandn  ymm0, ymm0, ymm5
    vmovdqu [rdx + rdi*8], ymm4
    vmovdqu [rcx + rdi*8], ymm0
    add rdi, 4
    cmp rdi, rsi
    jb .Lk3and_loop
    pop rdi
    pop rsi
    vzeroupper
    ret

    .p2align 4
    .globl asm_k3_or
asm_k3_or:
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
    vpcmpeqd ymm5, ymm5, ymm5
.Lk3or_loop:
    vmovdqu ymm0, [r8 + rdi*8]
    vmovdqu ymm1, [r9 + rdi*8]
    vmovdqu ymm2, [r10 + rdi*8]
    vmovdqu ymm3, [r11 + rdi*8]
    vpor    ymm4, ymm0, ymm2
    vpor    ymm0, ymm0, ymm1
    vpandn  ymm0, ymm0, ymm5
    vpor    ymm1, ymm2, ymm3
    vpandn  ymm1, ymm1, ymm5
    vpand   ymm0, ymm0, ymm1
    vpor    ymm0, ymm0, ymm4
    vpandn  ymm0, ymm0, ymm5
    vmovdqu [rdx + rdi*8], ymm4
    vmovdqu [rcx + rdi*8], ymm0
    add rdi, 4
    cmp rdi, rsi
    jb .Lk3or_loop
    pop rdi
    pop rsi
    vzeroupper
    ret

    .p2align 4
    .globl asm_k3_not
asm_k3_not:
    push rsi
    push rdi
    mov rax, rcx
    mov r8,  [rax+0]
    mov r9,  [rax+8]
    mov rdx, [rax+16]
    mov rcx, [rax+24]
    mov rsi, [rax+32]
    xor edi, edi
    vpcmpeqd ymm5, ymm5, ymm5
.Lk3not_loop:
    vmovdqu ymm0, [r8 + rdi*8]
    vmovdqu ymm1, [r9 + rdi*8]
    vpor    ymm0, ymm0, ymm1
    vpandn  ymm0, ymm0, ymm5
    vmovdqu [rdx + rdi*8], ymm0
    vmovdqu [rcx + rdi*8], ymm1
    add rdi, 4
    cmp rdi, rsi
    jb .Lk3not_loop
    pop rdi
    pop rsi
    vzeroupper
    ret
    "#
);

#[cfg(all(target_arch = "x86_64", target_os = "windows"))]
extern "C" {
    fn asm_k3_and(ctx: *const Ctx);
    fn asm_k3_or(ctx: *const Ctx);
    fn asm_k3_not(ctx: *const Ctx1);
}

unsafe fn asm_and(a: &Plane, b: &Plane, out: &mut Plane, words: usize) {
    let full = words & !3;
    #[cfg(all(target_arch = "x86_64", target_os = "windows"))]
    if full > 0 {
        let ctx = Ctx {
            a_ones: a.ones.as_ptr(),
            a_unk: a.unk.as_ptr(),
            b_ones: b.ones.as_ptr(),
            b_unk: b.unk.as_ptr(),
            out_ones: out.ones.as_mut_ptr(),
            out_unk: out.unk.as_mut_ptr(),
            words: full,
        };
        asm_k3_and(&ctx);
    }
    #[cfg(all(target_arch = "x86_64", not(target_os = "windows")))]
    if full > 0 {
        avx2_and(a, b, out, full);
    }
    scalar_and_range(a, b, out, full, words);
}

unsafe fn asm_or(a: &Plane, b: &Plane, out: &mut Plane, words: usize) {
    let full = words & !3;
    #[cfg(all(target_arch = "x86_64", target_os = "windows"))]
    if full > 0 {
        let ctx = Ctx {
            a_ones: a.ones.as_ptr(),
            a_unk: a.unk.as_ptr(),
            b_ones: b.ones.as_ptr(),
            b_unk: b.unk.as_ptr(),
            out_ones: out.ones.as_mut_ptr(),
            out_unk: out.unk.as_mut_ptr(),
            words: full,
        };
        asm_k3_or(&ctx);
    }
    #[cfg(all(target_arch = "x86_64", not(target_os = "windows")))]
    if full > 0 {
        avx2_or(a, b, out, full);
    }
    scalar_or_range(a, b, out, full, words);
}

unsafe fn asm_not(a: &Plane, out: &mut Plane, words: usize) {
    let full = words & !3;
    #[cfg(all(target_arch = "x86_64", target_os = "windows"))]
    if full > 0 {
        let ctx = Ctx1 {
            a_ones: a.ones.as_ptr(),
            a_unk: a.unk.as_ptr(),
            out_ones: out.ones.as_mut_ptr(),
            out_unk: out.unk.as_mut_ptr(),
            words: full,
        };
        asm_k3_not(&ctx);
    }
    #[cfg(all(target_arch = "x86_64", not(target_os = "windows")))]
    if full > 0 {
        avx2_not(a, out, full);
    }
    scalar_not_range(a, out, full, words);
}

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

fn check_truth_tables() {
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

fn check_lengths() {
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

fn run_once(f: &mut impl FnMut()) -> (u64, u128) {
    let c0 = unsafe { core::arch::x86_64::_rdtsc() };
    let t0 = Instant::now();
    f();
    let t1 = Instant::now();
    let c1 = unsafe { core::arch::x86_64::_rdtsc() };
    (c1.wrapping_sub(c0), (t1 - t0).as_nanos())
}

pub(crate) fn measure3(
    f1: &mut impl FnMut(),
    f2: &mut impl FnMut(),
    f3: &mut impl FnMut(),
    iters: usize,
) -> [(u64, u128); 3] {
    for _ in 0..3 {
        f1();
        f2();
        f3();
    }
    let mut cycles = [
        Vec::with_capacity(iters),
        Vec::with_capacity(iters),
        Vec::with_capacity(iters),
    ];
    let mut nanos = [
        Vec::with_capacity(iters),
        Vec::with_capacity(iters),
        Vec::with_capacity(iters),
    ];
    for _ in 0..iters {
        let a = run_once(f1);
        let b = run_once(f2);
        let c = run_once(f3);
        cycles[0].push(a.0);
        nanos[0].push(a.1);
        cycles[1].push(b.0);
        nanos[1].push(b.1);
        cycles[2].push(c.0);
        nanos[2].push(c.1);
    }
    let mut out = [(0u64, 0u128); 3];
    for i in 0..3 {
        cycles[i].sort_unstable();
        nanos[i].sort_unstable();
        out[i] = (cycles[i][cycles[i].len() / 2], nanos[i][nanos[i].len() / 2]);
    }
    out
}

pub(crate) fn measure2(
    f1: &mut impl FnMut(),
    f2: &mut impl FnMut(),
    iters: usize,
) -> [(u64, u128); 2] {
    for _ in 0..3 {
        f1();
        f2();
    }
    let mut cycles = [Vec::with_capacity(iters), Vec::with_capacity(iters)];
    let mut nanos = [Vec::with_capacity(iters), Vec::with_capacity(iters)];
    for _ in 0..iters {
        let a = run_once(f1);
        let b = run_once(f2);
        cycles[0].push(a.0);
        nanos[0].push(a.1);
        cycles[1].push(b.0);
        nanos[1].push(b.1);
    }
    let mut out = [(0u64, 0u128); 2];
    for i in 0..2 {
        cycles[i].sort_unstable();
        nanos[i].sort_unstable();
        out[i] = (cycles[i][cycles[i].len() / 2], nanos[i][nanos[i].len() / 2]);
    }
    out
}

fn report(name: &str, words: usize, bytes: u128, scalar_cycles: u64, cycles: u64, ns: u128) {
    let cycles_per_word = cycles as f64 / words as f64;
    let ns_per_word = ns as f64 / words as f64;
    let gib_per_s = bytes as f64 / ns as f64 / 1.073_741_824;
    let speedup = scalar_cycles as f64 / cycles as f64;
    println!(
        "{name:<14} {cycles_per_word:>10.3} {ns_per_word:>10.3} {gib_per_s:>8.2} {speedup:>7.2}x"
    );
}

pub(crate) fn report_unit(
    name: &str,
    units: usize,
    bytes: u128,
    base_cycles: u64,
    cycles: u64,
    ns: u128,
    unit: &str,
) {
    let c = cycles as f64 / units as f64;
    let n = ns as f64 / units as f64;
    let gib = bytes as f64 / ns as f64 / 1.073_741_824;
    let speedup = base_cycles as f64 / cycles as f64;
    println!("{name:<14} {c:>10.3} {n:>10.3} {gib:>8.2} {speedup:>7.2}x  [{unit}]");
}

fn bench_size(words: usize, iters: usize) {
    let mut state = 0xdead_beef_cafe_f00d;
    let a = Plane::random(words, &mut state);
    let b = Plane::random(words, &mut state);
    let bytes3 = (words as u128) * 8 * 6;
    let bytes1 = (words as u128) * 8 * 4;

    println!(
        "\nsize: {words} words ({:.1} KiB/plane), iters={iters}",
        (words * 8) as f64 / 1024.0
    );
    println!(
        "{:<14} {:>10} {:>10} {:>8} {:>7}",
        "kernel", "cyc/word", "ns/word", "GiB/s", "vs scalar"
    );

    let mut out_s = Plane::zeros(words);
    let mut out_v = Plane::zeros(words);
    let mut out_m = Plane::zeros(words);

    let r = measure3(
        &mut || {
            scalar_and(&a, &b, &mut out_s, words);
            black_box(out_s.ones[0]);
        },
        &mut || unsafe {
            avx2_and(&a, &b, &mut out_v, words);
            black_box(out_v.ones[0]);
        },
        &mut || unsafe {
            asm_and(&a, &b, &mut out_m, words);
            black_box(out_m.ones[0]);
        },
        iters,
    );
    report("scalar_and", words, bytes3, r[0].0, r[0].0, r[0].1);
    report("avx2_int_and", words, bytes3, r[0].0, r[1].0, r[1].1);
    report("asm_and", words, bytes3, r[0].0, r[2].0, r[2].1);

    let r = measure3(
        &mut || {
            scalar_or(&a, &b, &mut out_s, words);
            black_box(out_s.ones[0]);
        },
        &mut || unsafe {
            avx2_or(&a, &b, &mut out_v, words);
            black_box(out_v.ones[0]);
        },
        &mut || unsafe {
            asm_or(&a, &b, &mut out_m, words);
            black_box(out_m.ones[0]);
        },
        iters,
    );
    report("scalar_or", words, bytes3, r[0].0, r[0].0, r[0].1);
    report("avx2_int_or", words, bytes3, r[0].0, r[1].0, r[1].1);
    report("asm_or", words, bytes3, r[0].0, r[2].0, r[2].1);

    let r = measure3(
        &mut || {
            scalar_not(&a, &mut out_s, words);
            black_box(out_s.ones[0]);
        },
        &mut || unsafe {
            avx2_not(&a, &mut out_v, words);
            black_box(out_v.ones[0]);
        },
        &mut || unsafe {
            asm_not(&a, &mut out_m, words);
            black_box(out_m.ones[0]);
        },
        iters,
    );
    report("scalar_not", words, bytes1, r[0].0, r[0].0, r[0].1);
    report("avx2_int_not", words, bytes1, r[0].0, r[1].0, r[1].1);
    report("asm_not", words, bytes1, r[0].0, r[2].0, r[2].1);
}

#[cfg(target_os = "windows")]
fn pin_to_cpu(cpu: usize) {
    use std::ffi::c_void;
    #[link(name = "kernel32")]
    extern "system" {
        fn GetCurrentThread() -> *mut c_void;
        fn SetThreadAffinityMask(h_thread: *mut c_void, mask: usize) -> usize;
    }
    unsafe {
        let mask = 1usize << cpu;
        let prev = SetThreadAffinityMask(GetCurrentThread(), mask);
        if prev == 0 {
            println!("warning: SetThreadAffinityMask failed");
        }
    }
}

#[cfg(not(target_os = "windows"))]
fn pin_to_cpu(_cpu: usize) {}

fn main() {
    println!("trit asm verification — K3 bit-sliced AND/OR/NOT");
    println!(
        "arch={} os={}",
        std::env::consts::ARCH,
        std::env::consts::OS
    );
    println!(
        "avx2={} avx512f={} bmi2={}",
        is_x86_feature_detected!("avx2"),
        is_x86_feature_detected!("avx512f"),
        is_x86_feature_detected!("bmi2")
    );
    #[cfg(target_os = "windows")]
    println!("asm path: hand-written AVX2 (Windows x64)");
    #[cfg(not(target_os = "windows"))]
    println!("asm path: AVX2 intrinsics fallback (hand-written asm only on Windows)");
    pin_to_cpu(0);
    println!("pinned to CPU 0 (best effort)");

    check_truth_tables();
    check_lengths();

    if !avx2_available() {
        println!("AVX2 not available; SIMD benchmark skipped");
        return;
    }

    for &(words, iters) in &[(1usize << 14, 100usize), (1usize << 20, 50usize)] {
        bench_size(words, iters);
    }

    pack5::run();
    gf3::run();
}
