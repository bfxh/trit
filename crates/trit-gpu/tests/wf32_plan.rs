//! WF32 持久缓冲计划：换权重/换激活/合批三条路径都要和「每次新建缓冲」的旧口径同解。
//! 没有可用 adapter 时跳过（与其余 GPU 测试同约定）。

#![cfg(feature = "gpu")]

use trit_gpu::matmul_wf32::Wf32Gpu;
use trit_gpu::matmul_wf32_plan::{Wf32Kernel, run_batch};
use trit_gpu::pack_rows;

/// `C[r][c] = Σ_k a[r][k] · b[c][k]`，`b` 按列主序、每列补齐到 `words*32`。
fn cpu_wf32(m: usize, n: usize, k: usize, a: &[i32], b: &[f32]) -> Vec<f32> {
    let words = k.div_ceil(32);
    let mut out: Vec<f32> = Vec::with_capacity(m * n);
    for r in 0..m {
        for c in 0..n {
            let base = c * words * 32;
            let acc: f32 = (0..k).map(|kk| a[r * k + kk] as f32 * b[base + kk]).sum();
            out.push(acc);
        }
    }
    out
}

fn activations(n: usize, k: usize, seed: usize) -> Vec<f32> {
    let words = k.div_ceil(32);
    (0..n * words * 32)
        .map(|i| ((i * 13 + seed) % 17) as f32 / 17.0 - 0.5)
        .collect()
}

fn trit_rows(m: usize, k: usize, seed: usize) -> (Vec<i32>, Vec<u32>, Vec<u32>) {
    let a: Vec<i32> = (0..m * k)
        .map(|i| ((i * 7 + seed) % 3) as i32 - 1)
        .collect();
    let (pos, neg) = pack_rows(&a, k);
    (a, pos, neg)
}

fn assert_close(got: &[f32], want: &[f32], ctx: &str) {
    assert_eq!(got.len(), want.len(), "{ctx} 长度");
    for (i, (g, w)) in got.iter().zip(want).enumerate() {
        assert!((g - w).abs() < 1e-4, "{ctx} i={i} got={g} want={w}");
    }
}

#[test]
fn kernel_pick_covers_three_shapes() {
    assert_eq!(Wf32Kernel::pick(16, 16), Wf32Kernel::Tiled);
    assert_eq!(Wf32Kernel::pick(15, 64), Wf32Kernel::Reg4);
    assert_eq!(Wf32Kernel::pick(64, 4), Wf32Kernel::Reg4);
    assert_eq!(Wf32Kernel::pick(64, 3), Wf32Kernel::Scalar);
    assert_eq!(Wf32Kernel::pick(7, 3), Wf32Kernel::Scalar);
}

#[test]
fn plan_matches_cpu_on_all_kernels() {
    let Some(gpu) = Wf32Gpu::new() else {
        eprintln!("无 GPU adapter，跳过");
        return;
    };
    let (m, n, k) = (7usize, 9usize, 100usize);
    let (a, ap, an) = trit_rows(m, k, 1);
    let b = activations(n, k, 0);
    let want = cpu_wf32(m, n, k, &a, &b);
    let plan = gpu.plan(m, n, k, &ap, &an, &b);
    for kernel in [Wf32Kernel::Scalar, Wf32Kernel::Reg4, Wf32Kernel::Tiled] {
        assert_close(&plan.run(Some(kernel)), &want, &format!("{kernel:?}"));
    }
    assert_close(&plan.run(None), &want, "auto");
}

#[test]
fn reuse_plan_after_new_activations() {
    let Some(gpu) = Wf32Gpu::new() else {
        eprintln!("无 GPU adapter，跳过");
        return;
    };
    let (m, n, k) = (5usize, 8usize, 37usize);
    let (a, ap, an) = trit_rows(m, k, 1);
    let plan = gpu.plan(m, n, k, &ap, &an, &activations(n, k, 0));

    // 同一份数据连跑两次：持久 out/staging 必须每轮都被覆写，不能读到上一轮残留。
    let first = plan.run(None);
    assert_close(&plan.run(None), &first, "第二次同数据");

    for seed in [31usize, 57, 91] {
        let b = activations(n, k, seed);
        plan.set_activations(&b);
        assert_close(&plan.run(None), &cpu_wf32(m, n, k, &a, &b), &"换激活");
    }
}

#[test]
fn set_weights_replaces_the_bound_plane() {
    let Some(gpu) = Wf32Gpu::new() else {
        eprintln!("无 GPU adapter，跳过");
        return;
    };
    let (m, n, k) = (6usize, 5usize, 40usize);
    let (a0, ap0, an0) = trit_rows(m, k, 1);
    let (a1, ap1, an1) = trit_rows(m, k, 2);
    let b = activations(n, k, 0);
    assert_ne!(a0, a1, "两份权重应不同，否则本测试无意义");

    let plan = gpu.plan(m, n, k, &ap0, &an0, &b);
    assert_close(&plan.run(None), &cpu_wf32(m, n, k, &a0, &b), "原权重");
    plan.set_weights(&ap1, &an1);
    assert_close(&plan.run(None), &cpu_wf32(m, n, k, &a1, &b), "换权重");
}

#[test]
fn batch_of_layers_equals_individual_runs() {
    let Some(gpu) = Wf32Gpu::new() else {
        eprintln!("无 GPU adapter，跳过");
        return;
    };
    let shapes = [(8usize, 5usize, 70usize), (17, 33, 45), (20, 16, 128)];
    let mut plans = Vec::new();
    let mut want = Vec::new();
    for (idx, (m, n, k)) in shapes.into_iter().enumerate() {
        let (a, ap, an) = trit_rows(m, k, idx + 1);
        let b = activations(n, k, idx * 7);
        want.push(cpu_wf32(m, n, k, &a, &b));
        plans.push(gpu.plan(m, n, k, &ap, &an, &b));
    }
    let refs: Vec<&_> = plans.iter().collect();

    let batched = run_batch(&refs, None);
    assert_eq!(batched.len(), refs.len(), "每层都要有一条结果");
    for (idx, (got, exp)) in batched.iter().zip(&want).enumerate() {
        assert_close(got, exp, &format!("合批第 {idx} 层"));
    }

    // 合批后再单独跑，读写状态必须还能继续复用。
    for (idx, plan) in plans.iter().enumerate() {
        assert_close(
            &plan.run(None),
            &want[idx],
            &format!("合批后单跑第 {idx} 层"),
        );
    }
}

#[test]
fn timed_phases_agree_with_plain_run() {
    let Some(gpu) = Wf32Gpu::new() else {
        eprintln!("无 GPU adapter，跳过");
        return;
    };
    let (m, n, k) = (64usize, 64usize, 96usize);
    let (a, ap, an) = trit_rows(m, k, 1);
    let b = activations(n, k, 0);
    let plan = gpu.plan(m, n, k, &ap, &an, &b);
    let (timed, phases) = plan.run_timed(None);
    assert_close(&timed, &cpu_wf32(m, n, k, &a, &b), "分段计时");
    assert_close(&timed, &plan.run(None), "分段 vs 单 submit");
    assert!(phases.compute_ms > 0.0, "compute 段应耗时");
    assert!(phases.readback_ms > 0.0, "回读段应耗时");
}
