//! GF(3) add/mul verification on byte arrays with values 0..=2.

use crate::{measure3, report_unit};
use std::hint::black_box;

const ADD: [u8; 9] = [0, 1, 2, 1, 2, 0, 2, 0, 1];
const MUL: [u8; 9] = [0, 0, 0, 0, 1, 2, 0, 2, 1];

static GF3_ADD_TABLE: [u8; 32] = [
    0, 1, 2, 0, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1, 2, 0, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
];
static GF3_MUL_TABLE: [u8; 32] = [
    0, 0, 0, 0, 1, 2, 0, 2, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1, 2, 0, 2, 1, 0, 0, 0, 0, 0, 0, 0,
];

fn scalar_add_arith(a: &[u8], b: &[u8], out: &mut [u8], n: usize) {
    for i in 0..n {
        out[i] = ((a[i] as u16 + b[i] as u16) % 3) as u8;
    }
}

fn scalar_mul_arith(a: &[u8], b: &[u8], out: &mut [u8], n: usize) {
    for i in 0..n {
        out[i] = ((a[i] as u16 * b[i] as u16) % 3) as u8;
    }
}

fn scalar_add_lut(a: &[u8], b: &[u8], out: &mut [u8], n: usize) {
    for i in 0..n {
        out[i] = ADD[(a[i] * 3 + b[i]) as usize];
    }
}

fn scalar_mul_lut(a: &[u8], b: &[u8], out: &mut [u8], n: usize) {
    for i in 0..n {
        out[i] = MUL[(a[i] * 3 + b[i]) as usize];
    }
}

#[target_feature(enable = "avx2")]
unsafe fn avx2_add(a: &[u8], b: &[u8], out: &mut [u8], n: usize) {
    use core::arch::x86_64::*;
    let table = _mm256_setr_epi8(
        0, 1, 2, 0, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1, 2, 0, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0,
        0, 0,
    );
    let mut i = 0;
    while i + 32 <= n {
        let va = _mm256_loadu_si256(a.as_ptr().add(i) as *const __m256i);
        let vb = _mm256_loadu_si256(b.as_ptr().add(i) as *const __m256i);
        let idx = _mm256_add_epi8(va, vb);
        let res = _mm256_shuffle_epi8(table, idx);
        _mm256_storeu_si256(out.as_mut_ptr().add(i) as *mut __m256i, res);
        i += 32;
    }
    scalar_add_lut(&a[i..], &b[i..], &mut out[i..], n - i);
}

#[target_feature(enable = "avx2")]
unsafe fn avx2_mul(a: &[u8], b: &[u8], out: &mut [u8], n: usize) {
    use core::arch::x86_64::*;
    let table = _mm256_setr_epi8(
        0, 0, 0, 0, 1, 2, 0, 2, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1, 2, 0, 2, 1, 0, 0, 0, 0, 0,
        0, 0,
    );
    let mut i = 0;
    while i + 32 <= n {
        let va = _mm256_loadu_si256(a.as_ptr().add(i) as *const __m256i);
        let vb = _mm256_loadu_si256(b.as_ptr().add(i) as *const __m256i);
        let va3 = _mm256_add_epi8(va, _mm256_add_epi8(va, va));
        let idx = _mm256_add_epi8(va3, vb);
        let res = _mm256_shuffle_epi8(table, idx);
        _mm256_storeu_si256(out.as_mut_ptr().add(i) as *mut __m256i, res);
        i += 32;
    }
    scalar_mul_lut(&a[i..], &b[i..], &mut out[i..], n - i);
}

#[cfg(all(target_arch = "x86_64", target_os = "windows"))]
#[repr(C)]
struct Gf3Ctx {
    a: *const u8,
    b: *const u8,
    out: *mut u8,
    groups: usize,
    table: *const u8,
}

#[cfg(all(target_arch = "x86_64", target_os = "windows"))]
core::arch::global_asm!(
    r#"
    .text

    .p2align 4
    .globl asm_gf3_add
asm_gf3_add:
    mov rax, rcx
    mov r8,  [rax+0]
    mov r9,  [rax+8]
    mov rdx, [rax+16]
    mov r10, [rax+24]
    mov r11, [rax+32]
    test r10, r10
    jz .Lgf3add_done
    vmovdqu ymm2, [r11]
.Lgf3add_loop:
    vmovdqu ymm0, [r8]
    vmovdqu ymm1, [r9]
    vpaddb ymm0, ymm0, ymm1
    vpshufb ymm0, ymm2, ymm0
    vmovdqu [rdx], ymm0
    add r8, 32
    add r9, 32
    add rdx, 32
    dec r10
    jnz .Lgf3add_loop
.Lgf3add_done:
    vzeroupper
    ret

    .p2align 4
    .globl asm_gf3_mul
asm_gf3_mul:
    mov rax, rcx
    mov r8,  [rax+0]
    mov r9,  [rax+8]
    mov rdx, [rax+16]
    mov r10, [rax+24]
    mov r11, [rax+32]
    test r10, r10
    jz .Lgf3mul_done
    vmovdqu ymm2, [r11]
.Lgf3mul_loop:
    vmovdqu ymm0, [r8]
    vmovdqu ymm1, [r9]
    vpaddb ymm0, ymm0, ymm0
    vpaddb ymm0, ymm0, [r8]
    vpaddb ymm0, ymm0, ymm1
    vpshufb ymm0, ymm2, ymm0
    vmovdqu [rdx], ymm0
    add r8, 32
    add r9, 32
    add rdx, 32
    dec r10
    jnz .Lgf3mul_loop
.Lgf3mul_done:
    vzeroupper
    ret
    "#
);

#[cfg(all(target_arch = "x86_64", target_os = "windows"))]
extern "C" {
    fn asm_gf3_add(ctx: *const Gf3Ctx);
    fn asm_gf3_mul(ctx: *const Gf3Ctx);
}

fn avx2_available() -> bool {
    is_x86_feature_detected!("avx2")
}

unsafe fn asm_add(a: &[u8], b: &[u8], out: &mut [u8], n: usize) {
    let groups = n / 32;
    #[cfg(all(target_arch = "x86_64", target_os = "windows"))]
    if groups > 0 {
        let ctx = Gf3Ctx {
            a: a.as_ptr(),
            b: b.as_ptr(),
            out: out.as_mut_ptr(),
            groups,
            table: GF3_ADD_TABLE.as_ptr(),
        };
        asm_gf3_add(&ctx);
    }
    #[cfg(all(target_arch = "x86_64", not(target_os = "windows")))]
    if groups > 0 {
        avx2_add(a, b, out, groups * 32);
    }
    scalar_add_lut(
        &a[groups * 32..],
        &b[groups * 32..],
        &mut out[groups * 32..],
        n - groups * 32,
    );
}

unsafe fn asm_mul(a: &[u8], b: &[u8], out: &mut [u8], n: usize) {
    let groups = n / 32;
    #[cfg(all(target_arch = "x86_64", target_os = "windows"))]
    if groups > 0 {
        let ctx = Gf3Ctx {
            a: a.as_ptr(),
            b: b.as_ptr(),
            out: out.as_mut_ptr(),
            groups,
            table: GF3_MUL_TABLE.as_ptr(),
        };
        asm_gf3_mul(&ctx);
    }
    #[cfg(all(target_arch = "x86_64", not(target_os = "windows")))]
    if groups > 0 {
        avx2_mul(a, b, out, groups * 32);
    }
    scalar_mul_lut(
        &a[groups * 32..],
        &b[groups * 32..],
        &mut out[groups * 32..],
        n - groups * 32,
    );
}

fn random_gf3(n: usize, state: &mut u64) -> Vec<u8> {
    let mut out = Vec::with_capacity(n);
    for _ in 0..n {
        let mut x = *state;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        *state = x;
        out.push((x % 3) as u8);
    }
    out
}

fn check() {
    let mut state = 0x1234_5678_9abc_def0;
    let lengths = [
        0usize, 1, 2, 3, 4, 5, 7, 8, 9, 15, 16, 17, 31, 32, 33, 63, 64, 65, 127, 128, 129, 1000,
        4096,
    ];
    for &n in &lengths {
        let a = random_gf3(n, &mut state);
        let b = random_gf3(n, &mut state);
        let mut arith = vec![0u8; n];
        let mut lut = vec![0u8; n];
        let mut int = vec![0u8; n];
        let mut asm = vec![0u8; n];

        scalar_add_arith(&a, &b, &mut arith, n);
        scalar_add_lut(&a, &b, &mut lut, n);
        assert_eq!(arith, lut, "add scalar len {n}");
        if avx2_available() {
            unsafe {
                avx2_add(&a, &b, &mut int, n);
                asm_add(&a, &b, &mut asm, n);
            }
            assert_eq!(arith, int, "add avx2 len {n}");
            assert_eq!(arith, asm, "add asm len {n}");
        }

        scalar_mul_arith(&a, &b, &mut arith, n);
        scalar_mul_lut(&a, &b, &mut lut, n);
        assert_eq!(arith, lut, "mul scalar len {n}");
        if avx2_available() {
            unsafe {
                avx2_mul(&a, &b, &mut int, n);
                asm_mul(&a, &b, &mut asm, n);
            }
            assert_eq!(arith, int, "mul avx2 len {n}");
            assert_eq!(arith, asm, "mul asm len {n}");
        }
    }

    // Exhaustive 3x3 truth table, repeated to exercise 32-byte blocks.
    for av in 0..3u8 {
        for bv in 0..3u8 {
            let n = 64;
            let a = vec![av; n];
            let b = vec![bv; n];
            let mut out = vec![0u8; n];
            scalar_add_arith(&a, &b, &mut out, n);
            assert!(out.iter().all(|&v| v == ADD[(av * 3 + bv) as usize]));
            scalar_mul_arith(&a, &b, &mut out, n);
            assert!(out.iter().all(|&v| v == MUL[(av * 3 + bv) as usize]));
        }
    }
    println!("gf3 correctness: 23 lengths + 9 pairs ok");
}

fn bench(n: usize, iters: usize) {
    let mut state = 0xdead_beef_cafe_f00d;
    let a = random_gf3(n, &mut state);
    let b = random_gf3(n, &mut state);
    let bytes = (n as u128) * 3;
    let mut out_lut = vec![0u8; n];
    let mut out_int = vec![0u8; n];
    let mut out_asm = vec![0u8; n];

    println!("\ngf3 add: {n} bytes, iters={iters}");
    println!(
        "{:<14} {:>10} {:>10} {:>8} {:>7}",
        "kernel", "cyc/byte", "ns/byte", "GiB/s", "vs LUT"
    );
    let r = measure3(
        &mut || {
            scalar_add_lut(&a, &b, &mut out_lut, n);
            black_box(out_lut[0]);
        },
        &mut || unsafe {
            avx2_add(&a, &b, &mut out_int, n);
            black_box(out_int[0]);
        },
        &mut || unsafe {
            asm_add(&a, &b, &mut out_asm, n);
            black_box(out_asm[0]);
        },
        iters,
    );
    report_unit("scalar_lut", n, bytes, r[0].0, r[0].0, r[0].1, "byte");
    report_unit("avx2_int", n, bytes, r[0].0, r[1].0, r[1].1, "byte");
    report_unit("asm_vpshufb", n, bytes, r[0].0, r[2].0, r[2].1, "byte");

    println!("\ngf3 mul: {n} bytes, iters={iters}");
    println!(
        "{:<14} {:>10} {:>10} {:>8} {:>7}",
        "kernel", "cyc/byte", "ns/byte", "GiB/s", "vs LUT"
    );
    let r = measure3(
        &mut || {
            scalar_mul_lut(&a, &b, &mut out_lut, n);
            black_box(out_lut[0]);
        },
        &mut || unsafe {
            avx2_mul(&a, &b, &mut out_int, n);
            black_box(out_int[0]);
        },
        &mut || unsafe {
            asm_mul(&a, &b, &mut out_asm, n);
            black_box(out_asm[0]);
        },
        iters,
    );
    report_unit("scalar_lut", n, bytes, r[0].0, r[0].0, r[0].1, "byte");
    report_unit("avx2_int", n, bytes, r[0].0, r[1].0, r[1].1, "byte");
    report_unit("asm_vpshufb", n, bytes, r[0].0, r[2].0, r[2].1, "byte");
}

pub fn run() {
    println!("\n== gf3: GF(3) add/mul ==");
    check();
    if !avx2_available() {
        println!("AVX2 not available; skipping AVX2/asm GF(3) benchmark");
        return;
    }
    for &(n, iters) in &[(1usize << 16, 100usize), (1usize << 20, 50usize)] {
        bench(n, iters);
    }
}
