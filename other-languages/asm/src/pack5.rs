//! 5 trits/byte pack/unpack verification.
//!
//! Input layout for pack: one `i8` per trit (-1/0/+1).
//! Output layout: one byte per 5 trits; tail padded with trit 0.
//! The fast paths assume valid pack5 bytes (0..=242) on unpack.

use crate::{measure2, measure3, report_unit};
use std::hint::black_box;

const PACK_LUT_LEN: usize = 1024;
const TRITS_PER_BYTE: usize = 5;

fn map_field(field: usize) -> Option<u32> {
    match field {
        3 => Some(0), // raw low bits of -1
        0 => Some(1), // raw low bits of 0
        1 => Some(2), // raw low bits of +1
        _ => None,    // 2 is not a valid trit byte
    }
}

fn build_pack_lut() -> Box<[u8; PACK_LUT_LEN]> {
    let mut lut = Box::new([0u8; PACK_LUT_LEN]);
    for (idx, slot) in lut.iter_mut().enumerate() {
        let mut value = 0u32;
        let mut pow = 1u32;
        let mut ok = true;
        for j in 0..TRITS_PER_BYTE {
            let field = (idx >> (2 * j)) & 3;
            match map_field(field) {
                Some(d) => {
                    value += d * pow;
                    pow *= 3;
                }
                None => {
                    ok = false;
                    break;
                }
            }
        }
        *slot = if ok { value as u8 } else { 0 };
    }
    lut
}

fn build_unpack_lut() -> Box<[u64; 243]> {
    let mut lut = Box::new([0u64; 243]);
    for (value, slot) in lut.iter_mut().enumerate() {
        let mut v = value as u32;
        let mut word = 0u64;
        for j in 0..TRITS_PER_BYTE {
            let trit = (v % 3) as i8 - 1;
            word |= ((trit as u8) as u64) << (8 * j);
            v /= 3;
        }
        *slot = word;
    }
    lut
}

fn pack_group_arith(group: &[i8]) -> u8 {
    let mut value = 0u32;
    let mut pow = 1u32;
    for j in 0..TRITS_PER_BYTE {
        let digit = if j < group.len() {
            (group[j] as i32 + 1) as u32
        } else {
            1 // pad with trit 0
        };
        value += digit * pow;
        pow *= 3;
    }
    value as u8
}

fn scalar_pack_arith(input: &[i8], out: &mut [u8], n: usize) {
    let groups = n.div_ceil(TRITS_PER_BYTE);
    for g in 0..groups {
        let start = g * TRITS_PER_BYTE;
        let end = (start + TRITS_PER_BYTE).min(n);
        out[g] = pack_group_arith(&input[start..end]);
    }
}

#[inline(always)]
fn pack_one_lut(input: &[i8], n: usize, lut: &[u8; PACK_LUT_LEN], g: usize) -> u8 {
    let start = g * TRITS_PER_BYTE;
    let mut idx = 0usize;
    for j in 0..TRITS_PER_BYTE {
        let field = if start + j < n {
            (input[start + j] as u8 & 3) as usize
        } else {
            0 // missing -> trit 0
        };
        idx |= field << (2 * j);
    }
    lut[idx]
}

fn scalar_pack_lut_from(
    input: &[i8],
    out: &mut [u8],
    n: usize,
    lut: &[u8; PACK_LUT_LEN],
    start_group: usize,
) {
    let groups = n.div_ceil(TRITS_PER_BYTE);
    for g in start_group..groups {
        out[g] = pack_one_lut(input, n, lut, g);
    }
}

fn scalar_pack_lut(input: &[i8], out: &mut [u8], n: usize, lut: &[u8; PACK_LUT_LEN]) {
    scalar_pack_lut_from(input, out, n, lut, 0);
}

#[target_feature(enable = "bmi2")]
unsafe fn pack_pext_intrinsic(input: &[i8], out: &mut [u8], n: usize, lut: &[u8; PACK_LUT_LEN]) {
    let full_groups = n / TRITS_PER_BYTE;
    let groups = if n >= 8 {
        ((n - 8) / TRITS_PER_BYTE + 1).min(full_groups)
    } else {
        0
    };
    for g in 0..groups {
        let word = std::ptr::read_unaligned(input.as_ptr().add(g * TRITS_PER_BYTE) as *const u64);
        let idx = core::arch::x86_64::_pext_u64(word, 0x03_0303_0303) as usize;
        out[g] = lut[idx];
    }
    scalar_pack_lut_from(input, out, n, lut, groups);
}

fn scalar_unpack_arith(input: &[u8], out: &mut [i8], n: usize) -> Result<(), ()> {
    for t in 0..n {
        let b = input[t / TRITS_PER_BYTE];
        if b >= 243 {
            return Err(());
        }
        let mut v = b as u32;
        for _ in 0..(t % TRITS_PER_BYTE) {
            v /= 3;
        }
        out[t] = (v % 3) as i8 - 1;
    }
    Ok(())
}

#[inline(always)]
fn unpack_bytes(word: u64) -> [i8; TRITS_PER_BYTE] {
    let raw = word.to_le_bytes();
    let mut out = [0i8; TRITS_PER_BYTE];
    for i in 0..TRITS_PER_BYTE {
        out[i] = raw[i] as i8;
    }
    out
}

fn unpack_lut_from(
    input: &[u8],
    out: &mut [i8],
    n: usize,
    lut: &[u64; 243],
    start_group: usize,
) -> Result<(), ()> {
    let full_groups = n / TRITS_PER_BYTE;
    for g in start_group..full_groups {
        let b = input[g] as usize;
        if b >= 243 {
            return Err(());
        }
        out[g * TRITS_PER_BYTE..g * TRITS_PER_BYTE + TRITS_PER_BYTE]
            .copy_from_slice(&unpack_bytes(lut[b]));
    }
    if n % TRITS_PER_BYTE != 0 {
        let b = input[full_groups] as usize;
        if b >= 243 {
            return Err(());
        }
        let tail = n - full_groups * TRITS_PER_BYTE;
        out[full_groups * TRITS_PER_BYTE..n].copy_from_slice(&unpack_bytes(lut[b])[..tail]);
    }
    Ok(())
}

fn scalar_unpack_lut(input: &[u8], out: &mut [i8], n: usize, lut: &[u64; 243]) -> Result<(), ()> {
    unpack_lut_from(input, out, n, lut, 0)
}

#[cfg(all(target_arch = "x86_64", target_os = "windows"))]
#[repr(C)]
struct PackCtx {
    input: *const i8,
    output: *mut u8,
    groups: usize,
    lut: *const u8,
}

#[cfg(all(target_arch = "x86_64", target_os = "windows"))]
#[repr(C)]
struct UnpackCtx {
    input: *const u8,
    output: *mut i8,
    groups: usize,
    lut: *const u64,
}

#[cfg(all(target_arch = "x86_64", target_os = "windows"))]
core::arch::global_asm!(
    r#"
    .text

    .p2align 4
    .globl asm_pack5_pext
asm_pack5_pext:
    mov rax, rcx
    mov r8,  [rax+0]
    mov r9,  [rax+8]
    mov r10, [rax+16]
    mov r11, [rax+24]
    test r10, r10
    jz .Lpack5_done
.Lpack5_loop:
    mov rax, [r8]
    mov rcx, 0x0303030303
    pext rdx, rax, rcx
    movzx ecx, byte ptr [r11 + rdx]
    mov [r9], cl
    add r8, 5
    inc r9
    dec r10
    jnz .Lpack5_loop
.Lpack5_done:
    ret

    .p2align 4
    .globl asm_unpack5_lut
asm_unpack5_lut:
    mov rax, rcx
    mov r8,  [rax+0]
    mov r9,  [rax+8]
    mov r10, [rax+16]
    mov r11, [rax+24]
    test r10, r10
    jz .Lunpack5_done
.Lunpack5_loop:
    movzx eax, byte ptr [r8]
    mov rax, [r11 + rax*8]
    mov [r9], rax
    inc r8
    add r9, 5
    dec r10
    jnz .Lunpack5_loop
.Lunpack5_done:
    ret
    "#
);

#[cfg(all(target_arch = "x86_64", target_os = "windows"))]
extern "C" {
    fn asm_pack5_pext(ctx: *const PackCtx);
    fn asm_unpack5_lut(ctx: *const UnpackCtx);
}

unsafe fn asm_pack(input: &[i8], out: &mut [u8], n: usize, lut: &[u8; PACK_LUT_LEN]) {
    let full_groups = n / TRITS_PER_BYTE;
    let groups = if n >= 8 {
        ((n - 8) / TRITS_PER_BYTE + 1).min(full_groups)
    } else {
        0
    };
    #[cfg(all(target_arch = "x86_64", target_os = "windows"))]
    if groups > 0 {
        let ctx = PackCtx {
            input: input.as_ptr(),
            output: out.as_mut_ptr(),
            groups,
            lut: lut.as_ptr(),
        };
        asm_pack5_pext(&ctx);
    }
    #[cfg(all(target_arch = "x86_64", not(target_os = "windows")))]
    {
        let _ = groups;
        pack_pext_intrinsic(input, out, n, lut);
        return;
    }
    #[cfg(all(target_arch = "x86_64", target_os = "windows"))]
    scalar_pack_lut_from(input, out, n, lut, groups);
}

unsafe fn asm_unpack(input: &[u8], out: &mut [i8], n: usize, lut: &[u64; 243]) -> Result<(), ()> {
    let full_groups = n / TRITS_PER_BYTE;
    let groups = full_groups.saturating_sub(1);
    #[cfg(all(target_arch = "x86_64", target_os = "windows"))]
    if groups > 0 {
        let ctx = UnpackCtx {
            input: input.as_ptr(),
            output: out.as_mut_ptr(),
            groups,
            lut: lut.as_ptr(),
        };
        asm_unpack5_lut(&ctx);
    }
    unpack_lut_from(input, out, n, lut, groups)
}

fn bmi2_available() -> bool {
    is_x86_feature_detected!("bmi2")
}

fn random_trits(n: usize, state: &mut u64) -> Vec<i8> {
    let mut out = Vec::with_capacity(n);
    for _ in 0..n {
        let mut x = *state;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        *state = x;
        out.push((x % 3) as i8 - 1);
    }
    out
}

fn check(pack_lut: &[u8; PACK_LUT_LEN], unpack_lut: &[u64; 243]) {
    // All 243 combinations of 5 trits, repeated enough for the 8-byte-load paths.
    let mut source = Vec::with_capacity(243 * TRITS_PER_BYTE);
    for value in 0..243u32 {
        let mut v = value;
        for _ in 0..TRITS_PER_BYTE {
            source.push((v % 3) as i8 - 1);
            v /= 3;
        }
    }
    let n = source.len();
    let bytes = n.div_ceil(TRITS_PER_BYTE);

    let mut arith = vec![0u8; bytes];
    let mut lut = vec![0u8; bytes];
    let mut int = vec![0u8; bytes];
    let mut asm = vec![0u8; bytes];
    scalar_pack_arith(&source, &mut arith, n);
    scalar_pack_lut(&source, &mut lut, n, pack_lut);
    assert_eq!(arith, lut, "pack scalar arith vs LUT");
    if bmi2_available() {
        unsafe {
            pack_pext_intrinsic(&source, &mut int, n, pack_lut);
            asm_pack(&source, &mut asm, n, pack_lut);
        }
        assert_eq!(arith, int, "pack scalar vs pext intrinsic");
        assert_eq!(arith, asm, "pack scalar vs asm");
    }

    let mut out_arith = vec![0i8; n];
    let mut out_lut = vec![0i8; n];
    let mut out_asm = vec![0i8; n];
    scalar_unpack_arith(&arith, &mut out_arith, n).unwrap();
    scalar_unpack_lut(&arith, &mut out_lut, n, unpack_lut).unwrap();
    assert_eq!(source, out_arith, "unpack arith");
    assert_eq!(source, out_lut, "unpack LUT");
    unsafe {
        asm_unpack(&arith, &mut out_asm, n, unpack_lut).unwrap();
    }
    assert_eq!(source, out_asm, "unpack asm");
    assert_eq!(arith, lut, "pack LUT");

    // Random lengths, including tails.
    let lengths = [
        0usize, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 15, 16, 17, 31, 32, 33, 63, 64, 65, 127, 128, 129,
        1000, 4096,
    ];
    let mut state = 0x1234_5678_9abc_def0;
    for &len in &lengths {
        let input = random_trits(len, &mut state);
        let out_len = len.div_ceil(TRITS_PER_BYTE);
        let mut a = vec![0u8; out_len];
        let mut b = vec![0u8; out_len];
        let mut c = vec![0u8; out_len];
        scalar_pack_arith(&input, &mut a, len);
        scalar_pack_lut(&input, &mut b, len, pack_lut);
        assert_eq!(a, b, "random pack len {len}");
        if bmi2_available() {
            unsafe {
                pack_pext_intrinsic(&input, &mut c, len, pack_lut);
            }
            assert_eq!(a, c, "random pack pext len {len}");
            unsafe {
                asm_pack(&input, &mut c, len, pack_lut);
            }
            assert_eq!(a, c, "random pack asm len {len}");
        }

        let mut round = vec![0i8; len];
        scalar_unpack_lut(&a, &mut round, len, unpack_lut).unwrap();
        assert_eq!(input, round, "round trip len {len}");
        unsafe {
            asm_unpack(&a, &mut round, len, unpack_lut).unwrap();
        }
        assert_eq!(input, round, "round trip asm len {len}");
    }

    let mut tiny = [0i8; 5];
    assert!(scalar_unpack_lut(&[243], &mut tiny, 5, unpack_lut).is_err());
    println!("pack5 correctness: 243 combinations + 25 lengths ok");
}

fn bench_pack(n: usize, iters: usize, lut: &[u8; PACK_LUT_LEN]) {
    let mut state = 0xdead_beef_cafe_f00d;
    let input = random_trits(n, &mut state);
    let out_len = n.div_ceil(TRITS_PER_BYTE);
    let bytes = (n + out_len) as u128;
    let mut out_lut = vec![0u8; out_len];
    let mut out_int = vec![0u8; out_len];
    let mut out_asm = vec![0u8; out_len];

    println!("\npack5: {n} trits -> {out_len} bytes, iters={iters}");
    println!(
        "{:<14} {:>10} {:>10} {:>8} {:>7}",
        "kernel", "cyc/trit", "ns/trit", "GiB/s", "vs LUT"
    );
    let r = measure3(
        &mut || {
            scalar_pack_lut(&input, &mut out_lut, n, lut);
            black_box(out_lut[0]);
        },
        &mut || unsafe {
            pack_pext_intrinsic(&input, &mut out_int, n, lut);
            black_box(out_int[0]);
        },
        &mut || unsafe {
            asm_pack(&input, &mut out_asm, n, lut);
            black_box(out_asm[0]);
        },
        iters,
    );
    report_unit("scalar_lut", n, bytes, r[0].0, r[0].0, r[0].1, "trit");
    report_unit("pext_int", n, bytes, r[0].0, r[1].0, r[1].1, "trit");
    report_unit("asm_pext", n, bytes, r[0].0, r[2].0, r[2].1, "trit");
}

fn bench_unpack(n: usize, iters: usize, pack_lut: &[u8; PACK_LUT_LEN], lut: &[u64; 243]) {
    let mut state = 0x0bad_f00d_dead_beef;
    let input_trits = random_trits(n, &mut state);
    let bytes_in = n.div_ceil(TRITS_PER_BYTE);
    let mut packed = vec![0u8; bytes_in];
    scalar_pack_lut(&input_trits, &mut packed, n, pack_lut);
    let bytes = (n + bytes_in) as u128;
    let mut out_lut = vec![0i8; n];
    let mut out_asm = vec![0i8; n];

    println!("\nunpack5: {bytes_in} bytes -> {n} trits, iters={iters}");
    println!(
        "{:<14} {:>10} {:>10} {:>8} {:>7}",
        "kernel", "cyc/trit", "ns/trit", "GiB/s", "vs LUT"
    );
    let r = measure2(
        &mut || {
            scalar_unpack_lut(&packed, &mut out_lut, n, lut).unwrap();
            black_box(out_lut[0]);
        },
        &mut || unsafe {
            asm_unpack(&packed, &mut out_asm, n, lut).unwrap();
            black_box(out_asm[0]);
        },
        iters,
    );
    report_unit("scalar_lut", n, bytes, r[0].0, r[0].0, r[0].1, "trit");
    report_unit("asm_lut", n, bytes, r[0].0, r[1].0, r[1].1, "trit");
}

pub fn run() {
    println!("\n== pack5: 5 trits/byte ==");
    let pack_lut = build_pack_lut();
    let unpack_lut = build_unpack_lut();
    check(&pack_lut, &unpack_lut);
    if !bmi2_available() {
        println!("BMI2 not available; skipping pext/asm pack benchmark");
        return;
    }
    for &(n, iters) in &[(1usize << 16, 100usize), (1usize << 20, 50usize)] {
        bench_pack(n, iters, &pack_lut);
        bench_unpack(n, iters, &pack_lut, &unpack_lut);
    }
}
