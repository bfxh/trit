//! WF32 持久缓冲计划：换权重/换激活/合批三条路径都要和「每次新建缓冲」的旧口径同解。
//! 没有可用 adapter 时跳过（与其余 GPU 测试同约定）。

#![cfg(feature = "gpu")]

use trit_gpu::matmul_wf32::Wf32Gpu;
use trit_gpu::matmul_wf32_plan::{Wf32Kernel, Wf32Plan, run_batch, run_chain};
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
        assert_close(&plan.run(None), &cpu_wf32(m, n, k, &a, &b), "换激活");
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
/// 把上一层的行主序 `m×n` 输出重排成下一层的列主序 `b`（尾部补 0）——
/// 与 `matmul_wf32_transpose.wgsl` 同一套约定的 CPU 参照。
fn transpose_into_b(prev: &[f32], prev_m: usize, prev_n: usize) -> Vec<f32> {
    let words = prev_m.div_ceil(32);
    let mut b = vec![0f32; prev_n * words * 32];
    for c in 0..prev_n {
        for r in 0..prev_m {
            b[c * words * 32 + r] = prev[r * prev_n + c];
        }
    }
    b
}

/// 通道数递减、像素数不变的逐层链：40→24→8 通道 × 12 像素，每层 k = 上一层的 m。
/// 40 不是 32 的倍数，正好压住「补齐尾部必须是 0」。
const CHAIN: [(usize, usize, usize); 3] = [(40, 12, 32), (24, 12, 40), (8, 12, 24)];

/// 建一条链：非首层的 `b` 故意灌成垃圾，只有层间搬运真跑了结果才可能对上。
fn poisoned_chain(gpu: &Wf32Gpu) -> (Vec<Wf32Plan<'_>>, Vec<f32>) {
    let mut expected: Vec<f32> = Vec::new();
    let mut plans = Vec::new();
    for (idx, (m, n, k)) in CHAIN.into_iter().enumerate() {
        let (a, ap, an) = trit_rows(m, k, idx + 1);
        let words = k.div_ceil(32);
        let right = if idx == 0 {
            activations(n, k, 0)
        } else {
            transpose_into_b(&expected, CHAIN[idx - 1].0, n)
        };
        assert_eq!(right.len(), n * words * 32, "第 {idx} 层激活长度");
        expected = cpu_wf32(m, n, k, &a, &right);
        let poison = if idx == 0 {
            right
        } else {
            vec![3.25f32; n * words * 32]
        };
        plans.push(gpu.plan(m, n, k, &ap, &an, &poison));
    }
    (plans, expected)
}

#[test]
fn chain_consumes_previous_layer_output_on_device() {
    let Some(gpu) = Wf32Gpu::new() else {
        eprintln!("无 GPU adapter，跳过");
        return;
    };
    let (plans, expected) = poisoned_chain(&gpu);
    let refs: Vec<&Wf32Plan<'_>> = plans.iter().collect();
    assert_close(
        &run_chain(&refs, Some(Wf32Kernel::Scalar)),
        &expected,
        "链式 vs CPU 参照",
    );
    assert_close(
        &run_chain(&refs, Some(Wf32Kernel::Tiled)),
        &expected,
        "链式 tiled",
    );
    assert_close(&run_chain(&refs, None), &expected, "链式自动选核");
}

#[test]
fn chain_matches_layer_by_layer_roundtrip() {
    let Some(gpu) = Wf32Gpu::new() else {
        eprintln!("无 GPU adapter，跳过");
        return;
    };
    // 逐层口径：每层跑完回读、在 host 侧重排、再上传给下一层。
    let mut b_in = activations(CHAIN[0].1, CHAIN[0].2, 0);
    let mut per_layer: Vec<Vec<f32>> = Vec::new();
    for (idx, (m, n, k)) in CHAIN.into_iter().enumerate() {
        let (a, ap, an) = trit_rows(m, k, idx + 1);
        let plan = gpu.plan(m, n, k, &ap, &an, &b_in);
        let out = plan.run(Some(Wf32Kernel::Scalar));
        assert_close(&out, &cpu_wf32(m, n, k, &a, &b_in), "逐层第 {idx} 层");
        per_layer.push(out.clone());
        b_in = transpose_into_b(&out, m, n);
    }
    let (plans, expected) = poisoned_chain(&gpu);
    let refs: Vec<&Wf32Plan<'_>> = plans.iter().collect();
    assert_close(&per_layer[2], &expected, "逐层参照 vs CPU 参照");
    assert_close(
        &run_chain(&refs, Some(Wf32Kernel::Scalar)),
        &per_layer[2],
        "链式 vs 逐层回读",
    );
}

#[test]
fn chain_rejects_shape_that_does_not_connect() {
    let Some(gpu) = Wf32Gpu::new() else {
        eprintln!("无 GPU adapter，跳过");
        return;
    };
    let (_a, ap, an) = trit_rows(40, 32, 1);
    let prev = gpu.plan(40, 12, 32, &ap, &an, &activations(12, 32, 0));
    // 通道接不上：下一层 k=33 != 上一层 m=40
    let (_a2, ap2, an2) = trit_rows(24, 33, 2);
    let bad_k = gpu.plan(24, 12, 33, &ap2, &an2, &activations(12, 33, 0));
    // 像素数变了：n=11 != 上一层 n=12
    let (_a3, ap3, an3) = trit_rows(24, 40, 3);
    let bad_n = gpu.plan(24, 11, 40, &ap3, &an3, &activations(11, 40, 0));

    for (label, bad) in [("k 接不上", &bad_k), ("像素数变了", &bad_n)] {
        let refs = vec![&prev, bad];
        let caught = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            run_chain(&refs, Some(Wf32Kernel::Scalar))
        }));
        assert!(caught.is_err(), "{label} 时必须判红，不能悄悄算出个错结果");
    }
}
