//! NVIDIA cuda-oxide（Rust → PTX）f32 矩阵乘对照，形状/布局/计时口径与 trit-gpu 对齐：
//!   C[m][n] = Σ_k A[m][k]·Bᵀ[n][k]（B 转置存放），m=512 k=4096 n=512；
//!   warmup 10；5 组 ×20 次取中位数；读数 ps/MAC。
//!
//! 四个内核与 trit-gpu 的 WGSL 一一对应：
//!   sgemm_naive ↔ main         sgemm_tiled ↔ main_tiled
//!   sgemm_reg2  ↔ main_tiled2  sgemm_reg4  ↔ main_tiled4

use cuda_core::{CudaContext, DeviceBuffer, LaunchConfig2D};
use cuda_device::{
    DisjointSlice, RuntimeRowMajorTiles, SharedArray, cuda_module, kernel, launch_bounds,
    launch_contract, thread,
};

const M: usize = 512;
const K: usize = 4096;
const N: usize = 512;

#[cuda_module]
mod kernels {
    use super::*;

    #[kernel(launch_context = lc)]
    #[launch_bounds(256)]
    #[launch_contract(
        domain = 2,
        coordinates = u32,
        block = (16, 16, 1),
        requires = (a.len() >= m * k, bt.len() >= n * k, c.len() >= m * n)
    )]
    pub fn sgemm_naive(
        m: u32,
        n: u32,
        k: u32,
        a: &[f32],
        bt: &[f32],
        mut c: DisjointSlice<f32, RuntimeRowMajorTiles<1, 1>>,
    ) {
        let coord = thread::coord_2d_u32(lc);
        let row = coord.row() as usize;
        let col = coord.col() as usize;
        let (m, n, k) = (m as usize, n as usize, k as usize);
        let mut sum = 0.0f32;
        if row < m && col < n {
            let mut kk = 0usize;
            while kk < k {
                sum += a[row * k + kk] * bt[col * k + kk];
                kk += 1;
            }
            if let Some(mut cell) = c.tile_2d32_rt(coord) {
                cell.at_const::<0, 0>().write(sum);
            }
        }
    }

    #[kernel(launch_context = lc)]
    #[launch_bounds(256)]
    #[launch_contract(
        domain = 2,
        coordinates = u32,
        block = (16, 16, 1),
        requires = (a.len() >= m * k, bt.len() >= n * k, c.len() >= m * n)
    )]
    pub fn sgemm_tiled(
        m: u32,
        n: u32,
        k: u32,
        a: &[f32],
        bt: &[f32],
        mut c: DisjointSlice<f32, RuntimeRowMajorTiles<1, 1>>,
    ) {
        static mut TILE_A: SharedArray<f32, 256> = SharedArray::UNINIT;
        static mut TILE_B: SharedArray<f32, 256> = SharedArray::UNINIT;
        let ta = unsafe { SharedArray::as_raw_mut_ptr(&raw mut TILE_A) };
        let tb = unsafe { SharedArray::as_raw_mut_ptr(&raw mut TILE_B) };

        let coord = thread::coord_2d_u32(lc);
        let row = coord.row() as usize;
        let col = coord.col() as usize;
        let (m, n, k) = (m as usize, n as usize, k as usize);
        let tx = thread::threadIdx_x() as usize;
        let ty = thread::threadIdx_y() as usize;
        let mut sum = 0.0f32;
        let mut base = 0usize;
        while base < k {
            unsafe {
                let a_k = base + tx;
                ta.add(ty * 16 + tx).write(if row < m && a_k < k {
                    a[row * k + a_k]
                } else {
                    0.0
                });
                let b_k = base + ty;
                tb.add(tx * 16 + ty).write(if col < n && b_k < k {
                    bt[col * k + b_k]
                } else {
                    0.0
                });
            }
            thread::sync_threads();
            unsafe {
                let mut i = 0usize;
                while i < 16 {
                    sum += ta.add(ty * 16 + i).read() * tb.add(tx * 16 + i).read();
                    i += 1;
                }
            }
            thread::sync_threads();
            base += 16;
        }
        if row < m
            && col < n
            && let Some(mut cell) = c.tile_2d32_rt(coord)
        {
            cell.at_const::<0, 0>().write(sum);
        }
    }

    /// 8×8 线程、每线程 2×2、16×16 tile、K 段 8（↔ WGSL main_tiled2）。
    #[kernel(launch_context = lc)]
    #[launch_bounds(64)]
    #[launch_contract(
        domain = 2,
        coordinates = u32,
        block = (8, 8, 1),
        requires = (a.len() >= m * k, bt.len() >= n * k, c.len() >= m * n)
    )]
    pub fn sgemm_reg2(
        m: u32,
        n: u32,
        k: u32,
        a: &[f32],
        bt: &[f32],
        mut c: DisjointSlice<f32, RuntimeRowMajorTiles<2, 2>>,
    ) {
        static mut TILE_A: SharedArray<f32, 128> = SharedArray::UNINIT;
        static mut TILE_B: SharedArray<f32, 128> = SharedArray::UNINIT;
        let ta = unsafe { SharedArray::as_raw_mut_ptr(&raw mut TILE_A) };
        let tb = unsafe { SharedArray::as_raw_mut_ptr(&raw mut TILE_B) };

        let coord = thread::coord_2d_u32(lc);
        let (m, n, k) = (m as usize, n as usize, k as usize);
        let br = thread::blockIdx_y() as usize * 16;
        let bc = thread::blockIdx_x() as usize * 16;
        let tx = thread::threadIdx_x() as usize;
        let ty = thread::threadIdx_y() as usize;
        let tid = ty * 8 + tx;
        let r0 = ty * 2;
        let c0 = tx * 2;
        let mut s00 = 0.0f32;
        let mut s01 = 0.0f32;
        let mut s10 = 0.0f32;
        let mut s11 = 0.0f32;
        let mut base = 0usize;
        while base < k {
            let mut t = 0usize;
            while t < 2 {
                let slot = tid + t * 64;
                let line = slot / 8;
                let elem = base + slot % 8;
                unsafe {
                    ta.add(slot).write(if br + line < m && elem < k {
                        a[(br + line) * k + elem]
                    } else {
                        0.0
                    });
                    tb.add(slot).write(if bc + line < n && elem < k {
                        bt[(bc + line) * k + elem]
                    } else {
                        0.0
                    });
                }
                t += 1;
            }
            thread::sync_threads();
            unsafe {
                let mut w = 0usize;
                while w < 8 {
                    if base + w < k {
                        let a0 = ta.add(r0 * 8 + w).read();
                        let a1 = ta.add((r0 + 1) * 8 + w).read();
                        let b0 = tb.add(c0 * 8 + w).read();
                        let b1 = tb.add((c0 + 1) * 8 + w).read();
                        s00 += a0 * b0;
                        s01 += a0 * b1;
                        s10 += a1 * b0;
                        s11 += a1 * b1;
                    }
                    w += 1;
                }
            }
            thread::sync_threads();
            base += 8;
        }
        if let Some(mut cell) = c.tile_2d32_rt(coord) {
            cell.at_const::<0, 0>().write(s00);
            cell.at_const::<0, 1>().write(s01);
            cell.at_const::<1, 0>().write(s10);
            cell.at_const::<1, 1>().write(s11);
        }
    }

    /// 8×8 线程、每线程 4×4、32×32 tile、K 段 8（↔ WGSL main_tiled4）。
    #[kernel(launch_context = lc)]
    #[launch_bounds(64)]
    #[launch_contract(
        domain = 2,
        coordinates = u32,
        block = (8, 8, 1),
        requires = (a.len() >= m * k, bt.len() >= n * k, c.len() >= m * n)
    )]
    pub fn sgemm_reg4(
        m: u32,
        n: u32,
        k: u32,
        a: &[f32],
        bt: &[f32],
        mut c: DisjointSlice<f32, RuntimeRowMajorTiles<4, 4>>,
    ) {
        static mut TILE_A: SharedArray<f32, 256> = SharedArray::UNINIT;
        static mut TILE_B: SharedArray<f32, 256> = SharedArray::UNINIT;
        let ta = unsafe { SharedArray::as_raw_mut_ptr(&raw mut TILE_A) };
        let tb = unsafe { SharedArray::as_raw_mut_ptr(&raw mut TILE_B) };

        let coord = thread::coord_2d_u32(lc);
        let (m, n, k) = (m as usize, n as usize, k as usize);
        let br = thread::blockIdx_y() as usize * 32;
        let bc = thread::blockIdx_x() as usize * 32;
        let tx = thread::threadIdx_x() as usize;
        let ty = thread::threadIdx_y() as usize;
        let tid = ty * 8 + tx;
        let r0 = ty * 4;
        let c0 = tx * 4;
        #[allow(clippy::needless_range_loop)]
        let mut acc = [[0.0f32; 4]; 4];
        let mut base = 0usize;
        while base < k {
            let mut t = 0usize;
            while t < 4 {
                let slot = tid + t * 64;
                let line = slot / 8;
                let elem = base + slot % 8;
                unsafe {
                    ta.add(slot).write(if br + line < m && elem < k {
                        a[(br + line) * k + elem]
                    } else {
                        0.0
                    });
                    tb.add(slot).write(if bc + line < n && elem < k {
                        bt[(bc + line) * k + elem]
                    } else {
                        0.0
                    });
                }
                t += 1;
            }
            thread::sync_threads();
            unsafe {
                let mut w = 0usize;
                while w < 8 {
                    if base + w < k {
                        let a0 = ta.add(r0 * 8 + w).read();
                        let a1 = ta.add((r0 + 1) * 8 + w).read();
                        let a2 = ta.add((r0 + 2) * 8 + w).read();
                        let a3 = ta.add((r0 + 3) * 8 + w).read();
                        let b0 = tb.add(c0 * 8 + w).read();
                        let b1 = tb.add((c0 + 1) * 8 + w).read();
                        let b2 = tb.add((c0 + 2) * 8 + w).read();
                        let b3 = tb.add((c0 + 3) * 8 + w).read();
                        acc[0][0] += a0 * b0;
                        acc[0][1] += a0 * b1;
                        acc[0][2] += a0 * b2;
                        acc[0][3] += a0 * b3;
                        acc[1][0] += a1 * b0;
                        acc[1][1] += a1 * b1;
                        acc[1][2] += a1 * b2;
                        acc[1][3] += a1 * b3;
                        acc[2][0] += a2 * b0;
                        acc[2][1] += a2 * b1;
                        acc[2][2] += a2 * b2;
                        acc[2][3] += a2 * b3;
                        acc[3][0] += a3 * b0;
                        acc[3][1] += a3 * b1;
                        acc[3][2] += a3 * b2;
                        acc[3][3] += a3 * b3;
                    }
                    w += 1;
                }
            }
            thread::sync_threads();
            base += 8;
        }
        if let Some(mut cell) = c.tile_2d32_rt(coord) {
            cell.at_const::<0, 0>().write(acc[0][0]);
            cell.at_const::<0, 1>().write(acc[0][1]);
            cell.at_const::<0, 2>().write(acc[0][2]);
            cell.at_const::<0, 3>().write(acc[0][3]);
            cell.at_const::<1, 0>().write(acc[1][0]);
            cell.at_const::<1, 1>().write(acc[1][1]);
            cell.at_const::<1, 2>().write(acc[1][2]);
            cell.at_const::<1, 3>().write(acc[1][3]);
            cell.at_const::<2, 0>().write(acc[2][0]);
            cell.at_const::<2, 1>().write(acc[2][1]);
            cell.at_const::<2, 2>().write(acc[2][2]);
            cell.at_const::<2, 3>().write(acc[2][3]);
            cell.at_const::<3, 0>().write(acc[3][0]);
            cell.at_const::<3, 1>().write(acc[3][1]);
            cell.at_const::<3, 2>().write(acc[3][2]);
            cell.at_const::<3, 3>().write(acc[3][3]);
        }
    }
}

fn fill(values: &mut [f32], seed: u64) {
    let mut state = seed | 1;
    for value in values.iter_mut() {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        *value = ((state % 2001) as f32 - 1000.0) / 1000.0;
    }
}

fn median(mut samples: Vec<f64>) -> f64 {
    samples.sort_by(|a, b| a.partial_cmp(b).unwrap());
    samples[samples.len() / 2]
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let ctx = CudaContext::new(0)?;
    let stream = ctx.default_stream();
    println!("cuda-oxide f32 matmul compare (M={M} K={K} N={N})");

    let mut a = vec![0.0f32; M * K];
    let mut bt = vec![0.0f32; N * K];
    fill(&mut a, 0x9E37_79B9_7F4A_7C15);
    fill(&mut bt, 0xD1B5_4A32_D192_ED03);

    let a_dev = DeviceBuffer::from_host(&stream, &a)?;
    let bt_dev = DeviceBuffer::from_host(&stream, &bt)?;
    let mut c_dev = DeviceBuffer::<f32>::zeroed(&stream, M * N)?;

    // SAFETY: this package owns the embedded device bundle produced for the kernels module.
    let module = unsafe { kernels::load(&ctx)? };
    let (m, n, k) = (M as u32, N as u32, K as u32);
    let macs = (M * N * K) as f64;

    let cfg_16 = LaunchConfig2D::new(((N as u32).div_ceil(16), (M as u32).div_ceil(16)), (16, 16), 0);
    let cfg_2 = LaunchConfig2D::new(((N as u32).div_ceil(16), (M as u32).div_ceil(16)), (8, 8), 0);
    let cfg_4 = LaunchConfig2D::new(((N as u32).div_ceil(32), (M as u32).div_ceil(32)), (8, 8), 0);

    let naive = module.prepare_sgemm_naive(cfg_16)?;
    let tiled = module.prepare_sgemm_tiled(cfg_16)?;
    let reg2 = module.prepare_sgemm_reg2(cfg_2)?;
    let reg4 = module.prepare_sgemm_reg4(cfg_4)?;

    macro_rules! bench {
        ($name:literal, $launch:expr, $which:ident) => {{
            for _ in 0..10 {
                module.$which(
                    &stream,
                    &$launch,
                    m,
                    n,
                    k,
                    &a_dev,
                    &bt_dev,
                    cuda_host::RowWidth::new(&mut c_dev, n),
                )?;
            }
            stream.synchronize()?;
            let mut samples = Vec::with_capacity(5);
            for _ in 0..5 {
                let start = std::time::Instant::now();
                for _ in 0..20 {
                    module.$which(
                        &stream,
                        &$launch,
                        m,
                        n,
                        k,
                        &a_dev,
                        &bt_dev,
                        cuda_host::RowWidth::new(&mut c_dev, n),
                    )?;
                }
                stream.synchronize()?;
                samples.push(start.elapsed().as_secs_f64() / 20.0);
            }
            let seconds = median(samples);
            let ps = seconds * 1e12 / macs;
            println!(
                "{:>12}: {:.3} ms  {:.2} GFLOPS  {:.5} ps/MAC",
                $name,
                seconds * 1e3,
                2.0 * macs / seconds / 1e9,
                ps
            );
            ps
        }};
    }

    let naive_ps = bench!("sgemm_naive", naive, sgemm_naive);
    let tiled_ps = bench!("sgemm_tiled", tiled, sgemm_tiled);
    let reg2_ps = bench!("sgemm_reg2", reg2, sgemm_reg2);
    let reg4_ps = bench!("sgemm_reg4", reg4, sgemm_reg4);

    let out = c_dev.to_host_vec(&stream)?;
    let mut max_error = 0.0f32;
    for sample in 0..64usize {
        let index = sample * (M * N) / 64;
        let (row, col) = (index / N, index % N);
        let mut expected = 0.0f32;
        for kk in 0..K {
            expected += a[row * K + kk] * bt[col * K + kk];
        }
        max_error = max_error.max((out[index] - expected).abs());
    }
    println!("max_abs_error = {max_error:.3e}");
    println!(
        "{{\"naive\":{naive_ps:.6},\"tiled\":{tiled_ps:.6},\"reg2\":{reg2_ps:.6},\"reg4\":{reg4_ps:.6},\"max_abs_error\":{max_error:.3e}}}"
    );
    assert!(max_error < 5e-2, "结果对不上 CPU：{max_error}");
    Ok(())
}
