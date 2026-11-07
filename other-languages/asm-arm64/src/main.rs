//! AArch64 NEON verification for GF(3) add/mul.
//!
//! This crate is compile-checked with
//! `cargo check --target aarch64-unknown-linux-gnu` on an x86-64 host.
//! It is meant to run on AArch64 hardware; no ARM64 runner is available here.

#[cfg(not(target_arch = "aarch64"))]
compile_error!("trit-asm-arm64 targets aarch64");

use std::hint::black_box;
use std::time::Instant;

const ADD: [u8; 9] = [0, 1, 2, 1, 2, 0, 2, 0, 1];
const MUL: [u8; 9] = [0, 0, 0, 0, 1, 2, 0, 2, 1];

static ADD_TABLE16: [u8; 16] = [0, 1, 2, 0, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0];
static MUL_TABLE16: [u8; 16] = [0, 0, 0, 0, 1, 2, 0, 2, 1, 0, 0, 0, 0, 0, 0, 0];

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

#[target_feature(enable = "neon")]
unsafe fn neon_add(a: &[u8], b: &[u8], out: &mut [u8], n: usize) {
    use core::arch::aarch64::*;
    let table = vld1q_u8(ADD_TABLE16.as_ptr());
    let mut i = 0;
    while i + 16 <= n {
        let va = vld1q_u8(a.as_ptr().add(i));
        let vb = vld1q_u8(b.as_ptr().add(i));
        let idx = vaddq_u8(va, vb);
        let res = vqtbl1q_u8(table, idx);
        vst1q_u8(out.as_mut_ptr().add(i), res);
        i += 16;
    }
    scalar_add_lut(&a[i..], &b[i..], &mut out[i..], n - i);
}

#[target_feature(enable = "neon")]
unsafe fn neon_mul(a: &[u8], b: &[u8], out: &mut [u8], n: usize) {
    use core::arch::aarch64::*;
    let table = vld1q_u8(MUL_TABLE16.as_ptr());
    let mut i = 0;
    while i + 16 <= n {
        let va = vld1q_u8(a.as_ptr().add(i));
        let vb = vld1q_u8(b.as_ptr().add(i));
        let va3 = vaddq_u8(vaddq_u8(vaddq_u8(va, va), va), vb);
        let res = vqtbl1q_u8(table, va3);
        vst1q_u8(out.as_mut_ptr().add(i), res);
        i += 16;
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
        let mut neon = vec![0u8; n];
        scalar_add_lut(&a, &b, &mut lut, n);
        unsafe {
            neon_add(&a, &b, &mut neon, n);
        }
        assert_eq!(lut, neon, "add len {n}");
        scalar_mul_lut(&a, &b, &mut lut, n);
        unsafe {
            neon_mul(&a, &b, &mut neon, n);
        }
        assert_eq!(lut, neon, "mul len {n}");
    }
    for av in 0..3u8 {
        for bv in 0..3u8 {
            let a = vec![av; 32];
            let b = vec![bv; 32];
            let mut out = vec![0u8; 32];
            scalar_add_lut(&a, &b, &mut out, 32);
            assert!(out.iter().all(|&v| v == ADD[(av * 3 + bv) as usize]));
            scalar_mul_lut(&a, &b, &mut out, 32);
            assert!(out.iter().all(|&v| v == MUL[(av * 3 + bv) as usize]));
        }
    }
    println!("aarch64 neon correctness: 20 lengths + 9 pairs ok");
}

fn bench(n: usize, iters: usize) {
    let mut state = 0xdead_beef_cafe_f00d;
    let a = random_gf3(n, &mut state);
    let b = random_gf3(n, &mut state);
    let bytes = (n as u128) * 3;
    let mut out_lut = vec![0u8; n];
    let mut out_neon = vec![0u8; n];

    println!("\naarch64 gf3 add: {n} bytes, iters={iters}");
    let mut c_lut = Vec::with_capacity(iters);
    let mut c_neon = Vec::with_capacity(iters);
    for _ in 0..3 {
        scalar_add_lut(&a, &b, &mut out_lut, n);
        unsafe {
            neon_add(&a, &b, &mut out_neon, n);
        }
    }
    for _ in 0..iters {
        let t0 = Instant::now();
        scalar_add_lut(&a, &b, &mut out_lut, n);
        let t1 = Instant::now();
        unsafe {
            neon_add(&a, &b, &mut out_neon, n);
        }
        let t2 = Instant::now();
        c_lut.push((t1 - t0).as_nanos());
        c_neon.push((t2 - t1).as_nanos());
        black_box(out_lut[0]);
        black_box(out_neon[0]);
    }
    c_lut.sort_unstable();
    c_neon.sort_unstable();
    let lut = c_lut[c_lut.len() / 2] as f64;
    let neon = c_neon[c_neon.len() / 2] as f64;
    println!(
        "add scalar_lut {:.3} ns/B, neon {:.3} ns/B, {:.2}x",
        lut / n as f64,
        neon / n as f64,
        lut / neon
    );
    let _ = bytes;

    c_lut.clear();
    c_neon.clear();
    println!("aarch64 gf3 mul: {n} bytes, iters={iters}");
    for _ in 0..3 {
        scalar_mul_lut(&a, &b, &mut out_lut, n);
        unsafe {
            neon_mul(&a, &b, &mut out_neon, n);
        }
    }
    for _ in 0..iters {
        let t0 = Instant::now();
        scalar_mul_lut(&a, &b, &mut out_lut, n);
        let t1 = Instant::now();
        unsafe {
            neon_mul(&a, &b, &mut out_neon, n);
        }
        let t2 = Instant::now();
        c_lut.push((t1 - t0).as_nanos());
        c_neon.push((t2 - t1).as_nanos());
        black_box(out_lut[0]);
        black_box(out_neon[0]);
    }
    c_lut.sort_unstable();
    c_neon.sort_unstable();
    let lut = c_lut[c_lut.len() / 2] as f64;
    let neon = c_neon[c_neon.len() / 2] as f64;
    println!(
        "mul scalar_lut {:.3} ns/B, neon {:.3} ns/B, {:.2}x",
        lut / n as f64,
        neon / n as f64,
        lut / neon
    );
}

fn main() {
    println!("trit asm arm64 GF(3) NEON verification");
    println!(
        "arch={} os={}",
        std::env::consts::ARCH,
        std::env::consts::OS
    );
    check();
    bench(1 << 20, 20);
}
