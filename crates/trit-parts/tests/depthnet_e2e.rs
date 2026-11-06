//! `DepthNet` 的 `tests/` 级端到端断言。
//!
//! 分工：同文件 `mod tests` 已经有"学斜坡 / 学常数 / mv 门 / save-load / 确定性"这些局部性质，
//! 这里**不抄一遍**，只测那些必须跨模块才看得见的东西：三值量化的行为不变量、
//! 膨胀率与感受野、留出集上的泛化、以及部署时输入构成不能被拿错。

use trit_parts::depthnet::DepthNet;

const W: usize = 16;
const H: usize = 16;

fn planes(cin: usize, w: usize, h: usize, f: &dyn Fn(usize, usize, usize) -> f32) -> Vec<Vec<f32>> {
    (0..cin)
        .map(|c| {
            (0..h)
                .flat_map(move |y| (0..w).map(move |x| f(c, x, y)))
                .collect()
        })
        .collect()
}

/// 把第 0 个 latent 权重换成**同一量化格内**的另一个值。
/// 三值下 q(w)=round(w).clamp(-1,1)，所以格内换值不该改变任何输出；
/// f32 档（`ternary=false`）必须改变 —— 这一对同时钉住"标志位真的生效"。
#[test]
fn ternary_quantization_cell_leaves_output_bitwise_unchanged() {
    let mut tri = DepthNet::with_ternary(4, 7, true);
    let mut f32_ = DepthNet::with_ternary(4, 7, false);
    let x = planes(3, W, H, &|c, px, py| {
        ((px * 3 + py * 5 + c) % 9) as f32 / 4.0 - 1.0
    });
    let target: Vec<f32> = (0..W * H).map(|i| (i % W) as f32 / 8.0 - 1.0).collect();
    let mask = vec![true; W * H];
    for _ in 0..60 {
        tri.train_step(&x, W, H, &target, &mask, 0.02);
        f32_.train_step(&x, W, H, &target, &mask, 0.02);
    }

    let cell_value = |w0: f32| -> f32 {
        if w0.abs() >= 1.0 {
            w0 * 2.0 // clamp 到 ±1 的那一侧，翻倍仍是同一个 q
        } else if w0.abs() >= 0.5 {
            w0.signum() // q=±1，取格心的精确 ±1
        } else {
            w0 * 0.5 // q=0，缩一半还在 0 的格里
        }
    };
    let probe = |net: &mut DepthNet| -> (Vec<f32>, Vec<f32>) {
        let before = net.forward(&x, W, H);
        let w0 = net.c1.w[0];
        net.c1.w[0] = cell_value(w0);
        assert_ne!(net.c1.w[0], w0, "扰动本身得真的动一下，否则这条测试是空转");
        (before, net.forward(&x, W, H))
    };

    let (b, a) = probe(&mut tri);
    assert_eq!(
        b, a,
        "三值档：格内换 latent 后前向必须逐位不变（q(w) 没变）"
    );
    let (b2, a2) = probe(&mut f32_);
    assert_ne!(
        b2, a2,
        "f32 档：同一个扰动应当改变输出 —— ternary 标志位没被忽略才算数"
    );
}

/// 感受野靠膨胀长出来：`dil=1` 时远处像素**根本不依赖**那个冲击；`dil=3` 时必须依赖。
/// 判据用"改输入后远处输出动不动"，不用"输出值大概会变"。
#[test]
fn dilation_actually_extends_the_receptive_field() {
    let mut near = DepthNet::new(4, 13);
    near.set_dilation(1);
    let mut far = DepthNet::new(4, 13);
    far.set_dilation(3);

    let base = planes(3, W, H, &|_, _, _| 0.0);
    let impulse = |d: f32| {
        let mut p = base.clone();
        for plane in p.iter_mut() {
            plane[0] = d; // (0,0) 处的冲击
        }
        p
    };
    // 实测：膨胀核的抽头在 ±dil 的**整数倍**上，所以孤立冲击的可达集落在 dil 格点上
    // （dil=1 依赖 16 个像素 = {0,1,2,3}²，dil=3 = {0,3,6,9}²，最远 (9,9)）。
    // (6,6) 是 dil=3 的格点、却超出 dil=1 的半径 3 ⇒ 同一个像素在两种档下依赖性相反。
    // 第一版选了 (15,15)，理由是"角落够远"，但 15 不在 3 的格点上 ⇒ 报红的是夹具不是实现。
    let distant = 6 * W + 6;

    let b1 = near.forward(&impulse(0.0), W, H);
    let a1 = near.forward(&impulse(4.0), W, H);
    assert_eq!(
        b1[distant], a1[distant],
        "dil=1 的半径只有 3，(6,6) 不该被 (0,0) 的冲击影响"
    );

    let b3 = far.forward(&impulse(0.0), W, H);
    let a3 = far.forward(&impulse(4.0), W, H);
    assert_ne!(
        b3[distant], a3[distant],
        "(6,6) 在 dil=3 的格点上却不受影响 ⇒ 膨胀只是存了个字段"
    );
}

/// 留出集：3 张图训练、第 4 张（不同方向）从没出现过 ⇒ 误差要真的降下来。
/// 只看训练图会被"背下来"糊过去，这条挡住它。
#[test]
fn depth_net_generalises_to_a_held_out_orientation() {
    // 每族一张：水平 / 垂直 / 对角为训练，反对角为留出。
    let mk = |k: usize| -> (Vec<Vec<f32>>, Vec<f32>) {
        let g = move |c: usize, px: usize, py: usize| match k {
            0 => (px as f32 - 8.0) / 8.0 + c as f32 * 0.1,
            1 => (py as f32 - 8.0) / 8.0 + c as f32 * 0.1,
            2 => (px as f32 + py as f32 - 15.0) / 15.0 + c as f32 * 0.1,
            _ => (px as f32 - py as f32) / 15.0 + c as f32 * 0.1,
        };
        let x = planes(3, W, H, &g);
        let y: Vec<f32> = (0..H)
            .flat_map(|py| {
                (0..W).map(move |px| match k {
                    0 => (px as f32 - 8.0) / 8.0,
                    1 => (py as f32 - 8.0) / 8.0,
                    2 => (px + py) as f32 / 15.0 - 1.0,
                    _ => (px as f32 - py as f32) / 15.0,
                })
            })
            .collect();
        (x, y)
    };
    let train = [mk(0), mk(1), mk(2)];
    let (hx, hy) = mk(3);
    let mask = vec![true; W * H];

    let mut net = DepthNet::new(8, 21);
    let mse = |net: &DepthNet| -> f32 {
        let p = net.forward(&hx, W, H);
        p.iter()
            .zip(&hy)
            .map(|(a, b)| (a - b) * (a - b))
            .sum::<f32>()
            / (W * H) as f32
    };
    let before = mse(&net);
    for step in 0..1200 {
        let (x, y) = &train[step % 3];
        net.train_step(x, W, H, y, &mask, 0.02);
    }
    let after = mse(&net);
    assert!(
        after < before,
        "留出集 MSE 从 {before} 变成 {after}：训练 1200 步后必须下降"
    );
}

/// 输入构成（cin / pyr / ppyr）与量化档、膨胀率、归一化统计都要跟着存档走：
/// 拿错构成的模型在推理时是**通道数都对不上**的。
#[test]
fn deployment_composition_survives_the_archive() {
    let mut net = DepthNet::with_inputs(6, 4, 5, true);
    net.pyr = 1;
    net.ppyr = 0;
    net.set_dilation(2);
    net.med = -0.75;
    net.iqr = 1.25;
    net.out_b = 0.3;

    let dir = std::env::temp_dir().join("trit-parts-e2e-dcn");
    std::fs::create_dir_all(&dir).unwrap();
    let p = dir.join("d.dcn");
    net.save(&p).unwrap();
    let back = DepthNet::load(&p).unwrap();

    assert_eq!(
        (back.c1.cin, back.pyr, back.ppyr),
        (6, 1, 0),
        "输入构成丢了"
    );
    assert_eq!((back.c1.dil, back.c2.dil, back.c3.dil), (2, 2, 2));
    assert_eq!((back.med, back.iqr, back.out_b), (-0.75, 1.25, 0.3));
    assert!(
        back.c1.ternary && back.c2.ternary && !back.c3.ternary,
        "量化档被改写"
    );
    // 反归一化按存档统计走：部署侧少了这一步，深度就退回"相对值"。
    assert_eq!(back.denorm(1.0), 1.0 * 1.25 + -0.75);

    let x = planes(6, W, H, &|c, px, py| {
        ((px + py * 3 + c) % 7) as f32 / 3.0 - 1.0
    });
    assert_eq!(
        net.forward(&x, W, H),
        back.forward(&x, W, H),
        "存档往返后前向应逐位一致"
    );

    std::fs::write(&p, "DCN9 0 1 1 0 1 0 0\n").unwrap();
    match DepthNet::load(&p) {
        Ok(_) => panic!("未知 magic 不该被读成模型"),
        Err(e) => assert!(
            e.contains("不是 DCN1/DCN2/DCN3/DCN4"),
            "报错要点名格式：{e}"
        ),
    }
    std::fs::remove_file(&p).ok();
}
