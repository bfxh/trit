//! MOD 模型前向端到端：trit-texture 的 `to_dual_rail()` 缓冲直上 GPU naive
//! matmul 内核，与标量逐元素对拍（含跨字边界 inputs=33）。没有可用 adapter
//! 时跳过（与 `gpu::run` 同约定）。

#![cfg(feature = "gpu")]

use trit_gpu::{TRITS_PER_WORD, forward::matmul_packed, pack_columns};
use trit_texture::ttx2::{TritModel, TritModelLayer, TtxDocument};

#[test]
fn dual_rail_from_ttx_layer_feeds_the_gpu_kernel() {
    // inputs=33（跨 32-trit 字边界）、outputs=7：位布局若与内核约定不符，
    // 这里就会逐元素失配。
    let mut state = 0x9E37_79B9_7F4A_7C15u64;
    let mut next = || {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        (state % 3) as i32 - 1
    };
    let inputs = 33usize;
    let outputs = 7usize;
    let rows: Vec<Vec<i8>> = (0..outputs)
        .map(|_| (0..inputs).map(|_| next() as i8).collect())
        .collect();
    let bias: Vec<i8> = (0..outputs).map(|_| next() as i8).collect();
    let layer = TritModelLayer::new(&rows, &bias).unwrap();
    // 这就是要证明的路径：文件里的权重（基 3）→ to_dual_rail → 内核 A 侧。
    let (a_pos, a_neg) = layer.to_dual_rail();

    // 输入向量 x（n=1 列），用 trit-gpu 自己的 pack_columns 打包 B 侧。
    let x: Vec<i32> = (0..inputs).map(|_| next()).collect();
    let (b_pos, b_neg) = pack_columns(&x, inputs, 1);

    let words = inputs.div_ceil(TRITS_PER_WORD);
    let Some(gpu) = matmul_packed(outputs, 1, words, &a_pos, &a_neg, &b_pos, &b_neg) else {
        eprintln!("没有可用 GPU adapter，端到端对拍跳过");
        return;
    };
    for (row, preactivation) in gpu.iter().enumerate() {
        let expected: i32 = rows[row]
            .iter()
            .zip(&x)
            .map(|(weight, input)| i32::from(*weight) * input)
            .sum();
        assert_eq!(
            *preactivation, expected,
            "第 {row} 行：to_dual_rail 布局与内核点积不一致"
        );
    }
    // 前向语义收尾（CPU 侧）：preactivation + bias → sign。
    let forward: Vec<i8> = gpu
        .iter()
        .zip(&bias)
        .map(
            |(activation, bias)| match (activation + i32::from(*bias)).signum() {
                sign if sign > 0 => 1,
                sign if sign < 0 => -1,
                _ => 0,
            },
        )
        .collect();
    assert_eq!(forward.len(), outputs);
    // 往返完整性：容器字节重载后同一布局再跑一遍，结果必须一致。
    let model = TritModel::new(vec![TritModelLayer::new(&rows, &bias).unwrap()]).unwrap();
    let bytes = trit_texture::ttx2::to_ttx2(&TtxDocument::Model(model)).unwrap();
    let TtxDocument::Model(reloaded) = trit_texture::ttx2::from_ttx2(&bytes).unwrap() else {
        panic!("kind 漂移");
    };
    let (r_pos, r_neg) = reloaded.layers[0].to_dual_rail();
    let Some(again) = matmul_packed(outputs, 1, words, &r_pos, &r_neg, &b_pos, &b_neg) else {
        eprintln!("没有可用 GPU adapter，重载对拍跳过");
        return;
    };
    assert_eq!(again, gpu, "容器往返后双轨布局漂移");
}
