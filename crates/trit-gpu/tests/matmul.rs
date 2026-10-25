//! 三值矩阵乘：GPU 内核与 CPU 位并行镜像（`matmul_words`）逐元素对拍。
//! 没有可用 adapter 时跳过（与 `gpu::run` 同约定）。

#![cfg(feature = "gpu")]

use trit_gpu::{matmul, matmul_words, pack_columns, pack_rows};

#[test]
fn gpu_reg_matmul_tiny_case() {
    // 最小用例：m=n=k=1。A=[[+1]]、B=[[+1]] ⇒ C=[[1]]。
    // reg 内核若在装载/累加映射上有错，1 个 MAC 就能暴露。
    let a = vec![1i32];
    let b = vec![1i32];
    let (a_pos, a_neg) = pack_rows(&a, 1);
    let (b_pos, b_neg) = pack_columns(&b, 1, 1);
    let expected = matmul_words(1, 1, &a_pos, &a_neg, &b_pos, &b_neg);
    assert_eq!(expected, vec![1]);
    let report = matmul::run(1, 1, 1);
    let Some(report) = report else {
        eprintln!("没有可用 GPU adapter，对拍跳过");
        return;
    };
    assert!(report.reg_matches, "1×1×1 reg 不一致：{report:?}");
    assert!(report.reg4_matches, "1×1×1 reg4 不一致：{report:?}");
    assert!(report.matches);
    assert!(report.tiled_matches);
}

#[test]
fn gpu_matmul_matches_cpu_mirror() {
    // 非整字边界（k=100 → 4 字 + 28 位零尾）+ 非方阵，边界与转置一起钉。
    let m = 33usize;
    let k = 100usize;
    let n = 17usize;
    let mut state = 0x9E37_79B9_7F4A_7C15u64;
    let mut next = || {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        (state % 3) as i32 - 1
    };
    let a: Vec<i32> = (0..m * k).map(|_| next()).collect();
    let b: Vec<i32> = (0..k * n).map(|_| next()).collect();
    let (a_pos, a_neg) = pack_rows(&a, k);
    let (b_pos, b_neg) = pack_columns(&b, k, n);
    let expected = matmul_words(m, n, &a_pos, &a_neg, &b_pos, &b_neg);
    // 域检查：每个输出是 k 个 ±1 乘积之和，|值| ≤ k（runner 内部逐元素对拍
    // 依赖 matmul_words，这里先钉住它自己的取值域，防「两边一起错」）。
    assert_eq!(expected.len(), m * n);
    assert!(expected.iter().all(|value| value.abs() <= k as i64));

    let report = matmul::run(m, k, n);
    let Some(report) = report else {
        eprintln!("没有可用 GPU adapter，对拍跳过");
        return;
    };
    assert!(report.matches, "GPU 与 CPU 镜像不一致：{report:?}");
    assert!(
        report.tiled_matches,
        "分块内核与 CPU 镜像不一致：{report:?}"
    );
    assert!(
        report.reg_matches,
        "寄存器阻塞内核与 CPU 镜像不一致：{report:?}"
    );
    assert_eq!(report.m, m);
    assert_eq!(report.n, n);
    assert_eq!(report.k, k);
}
