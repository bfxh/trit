//! 端到端断言（`tests/` 级，不是同文件内联单点）。
//!
//! 立项目的：`docs/STATUS.md` §2 把 `DepthUNet` 与 `TriModel` 判成「已实现未测」——
//! 断言全在实现文件的 `mod tests` 里，`crates/trit-parts/tests/` 一度**一个文件都没有**。
//! 这里补的是「训练→存档→推理后端」这条链上的可复算判据，不是把内联测试抄一遍。

use trit_parts::depthunet::DepthUNet;
use trit_parts::model::{Backend, RefCpu};

fn synthetic_input(cin: usize, h: usize, w: usize) -> Vec<Vec<f32>> {
    (0..cin)
        .map(|c| {
            (0..h * w)
                .map(|i| ((i * 7 + c * 13) % 11) as f32 / 5.0 - 1.0)
                .collect()
        })
        .collect()
}

/// 过拟合一块两块的目标：300 步后 MSE 显著下降，且**左右均值分开**
/// （只看 MSE 会放过「塌成常数」的退化解，所以再加一条区域判据）。
#[test]
fn depth_unet_learns_a_two_region_target() {
    let (h, w) = (16, 16);
    let mut net = DepthUNet::new(3, 4, 7, true);
    let x = synthetic_input(3, h, w);
    let target: Vec<f32> = (0..h * w)
        .map(|i| if i % w < w / 2 { -0.6 } else { 0.6 })
        .collect();
    let mask = vec![true; h * w];
    let first = net.train_step(&x, w, h, &target, &mask, 0.02);
    for _ in 0..300 {
        net.train_step(&x, w, h, &target, &mask, 0.02);
    }
    let pred = net.forward(&x, w, h);
    let mse = pred
        .iter()
        .zip(&target)
        .map(|(a, b)| (a - b) * (a - b))
        .sum::<f32>()
        / (h * w) as f32;
    assert!(first > 0.1, "起始误差 {first} 太小，夹具没区分度");
    assert!(mse < first * 0.35, "300 步后 {mse} 应显著低于 {first}");

    let mean = |left: bool| -> f32 {
        let vals: Vec<f32> = (0..h * w)
            .filter(|&i| (i % w < w / 2) == left)
            .map(|i| pred[i])
            .collect();
        vals.iter().sum::<f32>() / vals.len() as f32
    };
    let (left, right) = (mean(true), mean(false));
    assert!(
        left < -0.2 && right > 0.2,
        "左半均值 {left} / 右半 {right}：应分别压向 −0.6 / +0.6，塌成常数就过不了这条"
    );
}

/// 全 false 掩码 = 一步空转：权重与输出必须**逐位不变**（不是"差不多"）。
#[test]
fn depth_unet_train_step_with_no_valid_pixels_is_a_no_op() {
    let (h, w) = (8, 8);
    let net = DepthUNet::new(3, 2, 11, true);
    let mut same = DepthUNet::new(3, 2, 11, true);
    let x = synthetic_input(3, h, w);
    let target = vec![0.5f32; h * w];
    let mask = vec![false; h * w];
    let loss = same.train_step(&x, w, h, &target, &mask, 0.05);
    assert_eq!(
        loss, 0.0,
        "损失按有效像素求和，全掩码时应为 0（不是按目标算）"
    );
    let before = net.forward(&x, w, h);
    let after = same.forward(&x, w, h);
    assert_eq!(
        before, after,
        "没有有效像素时不该动任何权重（逐位比较，容差不算）"
    );
}

/// 存 → 读 → 前向逐位一致；坏头要报错而不是 panic。
#[test]
fn depth_unet_save_load_roundtrip_is_bitwise() {
    let (h, w) = (8, 8);
    let mut net = DepthUNet::new(3, 2, 5, true);
    let x = synthetic_input(3, h, w);
    let target: Vec<f32> = (0..h * w).map(|i| (i % 5) as f32 * 0.2 - 0.4).collect();
    for _ in 0..30 {
        net.train_step(&x, w, h, &target, &vec![true; h * w], 0.02);
    }
    let dir = std::env::temp_dir().join("trit-parts-e2e-unet");
    std::fs::create_dir_all(&dir).unwrap();
    let p = dir.join("u.net");
    net.save(&p).unwrap();
    let back = DepthUNet::load(&p).unwrap();
    assert_eq!(net.forward(&x, w, h), back.forward(&x, w, h));

    std::fs::write(&p, "X1 0 1 3 2 0\n").unwrap();
    let err = match DepthUNet::load(&p) {
        Ok(_) => panic!("坏头应该报错，不该读出网络"),
        Err(e) => e,
    };
    assert!(err.contains("不是 U1"), "坏头应点名格式：{err}");
    std::fs::remove_file(&p).ok();
}

fn nhwc(res: u32) -> Vec<f32> {
    (0..res * res * 3)
        .map(|i| ((i * 37) % 17) as f32 / 8.0 - 1.0)
        .collect()
}

/// 随仓的部署包（`models/parts_id8118.tpm`，13 层 3→…→16）。
/// 用它而不是 `Net::random` 是有原因的：`net`/`tpm::from_net` 在 `gpu` feature 后面，
/// CI 默认特性下跑不到那条链，而加载 → 推理这一段是**任何芯片都要过的门**。
fn shipped() -> trit_parts::tpm::TpmModel {
    let p = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("models/parts_id8118.tpm");
    trit_parts::tpm::TpmModel::load(&p).unwrap_or_else(|e| panic!("{}: {e}", p.display()))
}

/// 训练态导出的包必须能被 CPU 参考后端跑通，且逐位可复算。
#[test]
fn tri_model_chains_from_tpm_to_refcpu_and_is_deterministic() {
    let tpm = shipped();
    assert_eq!(tpm.layers.len(), 13, "RefCpu 的层序常量按 13 层写死");
    assert_eq!((tpm.layers[0].cin, tpm.layers[0].cout), (3, 32));
    assert_eq!((tpm.layers[12].cin, tpm.layers[12].cout), (32, 16));

    let mut cpu = RefCpu::new(tpm.to_tri());
    let x = nhwc(8);
    let out = cpu.forward(&x, 8).expect("参考后端必须能独立跑");
    assert_eq!(out.len(), 8 * 8 * 16, "输出是 NHWC 的 res×res×16");
    assert!(out.iter().all(|v| v.is_finite()), "出现 NaN/Inf");
    let distinct: std::collections::BTreeSet<i32> = out.iter().map(|v| (v * 1e4) as i32).collect();
    assert!(distinct.len() > 1, "输出全同值，说明前向退化成常数");

    let mut again = RefCpu::new(shipped().to_tri());
    assert_eq!(
        out,
        again.forward(&x, 8).unwrap(),
        "重新加载后前向应逐位相同"
    );
}

/// 三值不变量：`E = q·scale`，q ∈ {−1,0,+1} ⇒ 每个 |E| 只能是 0 或该输出通道的 scale；
/// 并且 `to_tri()` 搬的就是这份 E（不是重新量化）。
#[test]
fn tri_model_effective_weights_are_ternary() {
    let tpm = shipped();
    let tri = tpm.to_tri();
    assert_eq!(tri.layers.len(), tpm.layers.len());
    for (i, (src, dst)) in tpm.layers.iter().zip(&tri.layers).enumerate() {
        assert_eq!(
            dst.e,
            src.effective(),
            "层 {i}：to_tri 之后 E 变了，说明中间混进了第二次量化"
        );
        assert_eq!(
            dst.e.len() % src.cout as usize,
            0,
            "层 {i} 的 E 不按 cout 对齐"
        );
        for (wi, &e) in dst.e.iter().enumerate() {
            let q = src.q[wi];
            assert!(
                q == -1 || q == 0 || q == 1,
                "层 {i} 第 {wi} 个 q={q} 不是三值"
            );
            let scale = src.scale[wi % src.cout as usize].abs();
            let mag = e.abs();
            assert!(
                mag == 0.0 || (mag - scale).abs() <= scale * 1e-6,
                "层 {i} 第 {wi} 个权重 |E|={mag} 既不是 0 也不是 scale={scale}"
            );
        }
    }
}

/// 裁 head 只删通道：留下的平面必须逐位不变（1×1 head 逐通道独立）。
#[test]
fn truncate_head_keeps_the_surviving_planes_bitwise() {
    let tpm = shipped();
    let x = nhwc(8);
    let mut cpu = RefCpu::new(tpm.to_tri());
    let full = cpu.forward(&x, 8).unwrap();
    let cut = tpm.truncate_head(4);
    let mut cpu2 = RefCpu::new(cut.to_tri());
    let small = cpu2.forward(&x, 8).unwrap();
    assert_eq!(small.len(), 8 * 8 * 4);
    for p in 0..8 * 8 {
        for c in 0..4 {
            assert_eq!(
                full[p * 16 + c],
                small[p * 4 + c],
                "像素 {p} 平面 {c} 被裁剪改动了 —— head 是 1×1，逐通道必须独立"
            );
        }
    }
}

/// 后端能力表不是摆设：`refcpu` 必须自称无 int8 / 无 LUT。
#[test]
fn refcpu_caps_are_stated_not_defaulted() {
    let cpu = RefCpu::new(shipped().to_tri());
    let caps = Backend::caps(&cpu);
    assert_eq!(caps.name, "refcpu");
    assert!(!caps.int8 && !caps.lut, "参考实现是纯 f32 标量");
    assert_eq!(caps.max_buffer, u64::MAX);
}

/// 对拍尺子本身要自证：同一份输出 ⇒ 一致率 1.0 / 最大差 0；
/// 只翻一个三元组的 argmax ⇒ 一致率必须掉下来（否则"所有后端与参考一致"这条门是空转的）。
#[test]
fn the_cross_backend_ruler_is_decisive() {
    let tpm = shipped();
    let x = nhwc(8);
    let mut cpu = RefCpu::new(tpm.to_tri());
    let a = cpu.forward(&x, 8).unwrap();
    let (rate, maxd) = trit_parts::model::compare_nhwc(&a, &a, 8);
    assert_eq!((rate, maxd), (1.0, 0.0), "尺子对自己必须报满一致");

    let mut b = a.clone();
    // 像素 0、三元组 0（平面 0/1/2）：把当前 argmax **换到另一个通道**，
    // 单纯抬高某个值有可能抬的就是本来就是最大值的那个通道（第一版就是这么漏掉的）。
    let cur = (0..3usize)
        .max_by(|x, y| b[*x].partial_cmp(&b[*y]).unwrap())
        .unwrap();
    b[(cur + 1) % 3] = b[cur] + 1.0;
    let (rate2, maxd2) = trit_parts::model::compare_nhwc(&a, &b, 8);
    assert!(maxd2 > 0.0, "改了值却报零差");
    assert!(
        rate2 < 1.0,
        "翻了 argmax 却报一致：{rate2}（应至少掉 1/(5·64)≈0.0031）"
    );
    assert!(rate2 > 0.99, "只改了一个像素却把整帧判成不一致：{rate2}");
}

fn fnv1a64(vals: &[f32]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for v in vals {
        for byte in v.to_le_bytes() {
            h ^= byte as u64;
            h = h.wrapping_mul(0x1000_0000_01b3);
        }
    }
    h
}

/// 数值地板：随仓权重 + 固定输入 ⇒ 固定指纹（FNV-1a 64，逐 f32 的位）。
/// 结构不变量拦不住"算子写错但形状对"，这一条拦得住：池化 max→avg、仿射丢 γ、
/// convt 布局反了都会打红。换权重文件必须重新登记，并同时重跑跨后端对拍。
#[test]
fn refcpu_output_matches_the_recorded_fingerprint() {
    for (res, want) in [(8u32, 0xf6de046f0b41c1f6u64), (16, 0x05f25dc40107c04a)] {
        let mut cpu = RefCpu::new(shipped().to_tri());
        let out = cpu.forward(&nhwc(res), res).unwrap();
        let got = fnv1a64(&out);
        assert_eq!(
            got, want,
            "res={res} 的前向指纹从 {want:016x} 变成 {got:016x}：\
             要么改了参考后端的算子，要么换了 `models/parts_id8118.tpm`——两者都要显式重登记"
        );
    }
}
