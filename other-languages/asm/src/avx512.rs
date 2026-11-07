//! AVX-512 VBMI path for GF(3) add/mul.
//!
//! Compile-only on this host: the CPU has no AVX-512, so `run()` checks the
//! feature bits and skips execution. The code is compiled to keep the path
//! honest; run it on AVX-512 hardware later.

use crate::{measure2, report_unit};
use std::hint::black_box;

const ADD: [u8; 9] = [0, 1, 2, 1, 2, 0, 2, 0, 1];
const MUL: [u8; 9] = [0, 0, 0, 0, 1, 2, 0, 2, 1];

static ADD512: [u8; 64] = [
    0, 1, 2, 0, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
];
static MUL512: [u8; 64] = [
    0, 0, 0, 0, 1, 2, 0, 2, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
];

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

fn avx512_available() -> bool {
    is_x86_feature_detected!("avx512f")
        && is_x86_feature_detected!("avx512bw")
        && is_x86_feature_detected!("avx512vbmi")
}

#[target_feature(enable = "avx512f,avx512bw,avx512vbmi")]
unsafe fn avx512_add(a: &[u8], b: &[u8], out: &mut [u8], n: usize) {
    use core::arch::x86_64::*;
    let table = _mm512_loadu_si512(ADD512.as_ptr() as *const __m512i);
    let mut i = 0;
    while i + 64 <= n {
        let va = _mm512_loadu_si512(a.as_ptr().add(i) as *const __m512i);
        let vb = _mm512_loadu_si512(b.as_ptr().add(i) as *const __m512i);
        let idx = _mm512_add_epi8(va, vb);
        let res = _mm512_permutexvar_epi8(idx, table);
        _mm512_storeu_si512(out.as_mut_ptr().add(i) as *mut __m512i, res);
        i += 64;
    }
    scalar_add_lut(&a[i..], &b[i..], &mut out[i..], n - i);
}

#[target_feature(enable = "avx512f,avx512bw,avx512vbmi")]
unsafe fn avx512_mul(a: &[u8], b: &[u8], out: &mut [u8], n: usize) {
    use core::arch::x86_64::*;
    let table = _mm512_loadu_si512(MUL512.as_ptr() as *const __m512i);
    let mut i = 0;
    while i + 64 <= n {
        let va = _mm512_loadu_si512(a.as_ptr().add(i) as *const __m512i);
        let vb = _mm512_loadu_si512(b.as_ptr().add(i) as *const __m512i);
        let va3 = _mm512_add_epi8(va, _mm512_add_epi8(va, va));
        let idx = _mm512_add_epi8(va3, vb);
        let res = _mm512_permutexvar_epi8(idx, table);
        _mm512_storeu_si512(out.as_mut_ptr().add(i) as *mut __m512i, res);
        i += 64;
    }
    scalar_mul_lut(&a[i..], &b[i..], &mut out[i..], n - i);
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
    let lengths = [
        0usize, 1, 2, 3, 4, 5, 7, 8, 9, 15, 16, 17, 31, 32, 33, 63, 64, 65, 1000, 4096,
    ];
    let mut state = 0x1234_5678_9abc_def0;
    for &n in &lengths {
        let a = random_gf3(n, &mut state);
        let b = random_gf3(n, &mut state);
        let mut lut = vec![0u8; n];
        let mut avx = vec![0u8; n];
        scalar_add_lut(&a, &b, &mut lut, n);
        unsafe {
            avx512_add(&a, &b, &mut avx, n);
        }
        assert_eq!(lut, avx, "AVX-512 add len {n}");
        scalar_mul_lut(&a, &b, &mut lut, n);
        unsafe {
            avx512_mul(&a, &b, &mut avx, n);
        }
        assert_eq!(lut, avx, "AVX-512 mul len {n}");
    }
    for av in 0..3u8 {
        for bv in 0..3u8 {
            let a = vec![av; 64];
            let b = vec![bv; 64];
            let mut out = vec![0u8; 64];
            scalar_add_lut(&a, &b, &mut out, 64);
            assert!(out.iter().all(|&v| v == ADD[(av * 3 + bv) as usize]));
            scalar_mul_lut(&a, &b, &mut out, 64);
            assert!(out.iter().all(|&v| v == MUL[(av * 3 + bv) as usize]));
        }
    }
    println!("avx512 correctness: 20 lengths + 9 pairs ok");
}

fn bench(n: usize, iters: usize) {
    let mut state = 0xdead_beef_cafe_f00d;
    let a = random_gf3(n, &mut state);
    let b = random_gf3(n, &mut state);
    let bytes = (n as u128) * 3;
    let mut out_lut = vec![0u8; n];
    let mut out_avx = vec![0u8; n];

    println!("\navx512 gf3 add: {n} bytes, iters={iters}");
    println!(
        "{:<14} {:>10} {:>10} {:>8} {:>7}",
        "kernel", "cyc/byte", "ns/byte", "GiB/s", "vs LUT"
    );
    let r = measure2(
        &mut || {
            scalar_add_lut(&a, &b, &mut out_lut, n);
            black_box(out_lut[0]);
        },
        &mut || unsafe {
            avx512_add(&a, &b, &mut out_avx, n);
            black_box(out_avx[0]);
        },
        iters,
    );
    report_unit("scalar_lut", n, bytes, r[0].0, r[0].0, r[0].1, "byte");
    report_unit("avx512_vbmi", n, bytes, r[0].0, r[1].0, r[1].1, "byte");

    println!("\navx512 gf3 mul: {n} bytes, iters={iters}");
    println!(
        "{:<14} {:>10} {:>10} {:>8} {:>7}",
        "kernel", "cyc/byte", "ns/byte", "GiB/s", "vs LUT"
    );
    let r = measure2(
        &mut || {
            scalar_mul_lut(&a, &b, &mut out_lut, n);
            black_box(out_lut[0]);
        },
        &mut || unsafe {
            avx512_mul(&a, &b, &mut out_avx, n);
            black_box(out_avx[0]);
        },
        iters,
    );
    report_unit("scalar_lut", n, bytes, r[0].0, r[0].0, r[0].1, "byte");
    report_unit("avx512_vbmi", n, bytes, r[0].0, r[1].0, r[1].1, "byte");
}

pub fn run() {
    println!("\n== avx512: GF(3) VBMI path ==");
    if !avx512_available() {
        println!("AVX-512 F/BW/VBMI not available; code compiled but not executed");
        return;
    }
    check();
    bench(1 << 20, 50);
}
