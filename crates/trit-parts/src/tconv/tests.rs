use super::conv::{coh_loss_grad, sobel, sobel_adjoint};
use super::*;
#[test]
fn conv_forward_shape_and_zero_pad() {
    let c = Conv::new(1, 2, 1);
    let x = vec![vec![1.0f32; 16]];
    let y = c.forward(&x, 4, 4);
    assert_eq!(y.len(), 2);
    assert_eq!(y[0].len(), 16);
}

#[test]
fn learns_constant_target() {
    // 最小诊断：输入常数 0.9、目标常数 0.9，单层网络应把输出推到 0.9
    let (w, h) = (6usize, 6usize);
    let img = vec![0.9f32; w * h];
    let tgt = vec![0.9f32; w * h];
    let mut net = Net::new(1, &[], 3);
    for _ in 0..3000 {
        net.train_step(&img, &tgt, None, w, h, 0.5);
    }
    let p = net.forward(&img, None, w, h);
    let mean = p.iter().sum::<f32>() / p.len() as f32;
    assert!(mean > 0.85, "应学成 0.9，实际 {mean:.3}");
}

#[test]
fn net_save_load_roundtrip() {
    let mut net = Net::new(2, &[4, 3], 11);
    net.set_bn(true);
    net.set_dilation(2);
    let (w, h) = (5usize, 5usize);
    let x = vec![vec![0.4f32; w * h], vec![0.25f32; w * h]];
    let a = net.forward_x(&x, w, h);
    let p = std::env::temp_dir().join("tconv_roundtrip.tpc");
    net.save(&p).unwrap();
    let back = Net::load(&p).unwrap();
    assert_eq!(back.layers[0].dil, 2, "膨胀率必须存进文件");
    let b = back.forward_x(&x, w, h);
    for (p, q) in a.iter().zip(&b) {
        assert!((p - q).abs() < 1e-5, "存读应逐位一致 {p} vs {q}");
    }
    assert_eq!(back.cond, 0);
    assert!(!back.linear_out);
    let _ = std::fs::remove_file(p);
}

#[test]
fn dilation_widens_the_tap_spacing() {
    // 单通道、单输出：输入中心放一个脉冲，dil=2 时只有 ±2 的抽头看得到它
    let mut c = Conv::new(1, 1, 5);
    c.ternary = false;
    c.w.fill(1.0);
    c.b.fill(0.0);
    c.scale.fill(1.0);
    let (w, h) = (7usize, 7usize);
    let mut x = vec![vec![0.0f32; w * h]];
    x[0][3 * w + 3] = 1.0;
    let mut hit = |dil: usize, dx: i32, dy: i32| {
        c.dil = dil;
        let o = c.forward(&x, w, h);
        o[0][((3 + dy) as usize) * w + (3 + dx) as usize]
    };
    assert_eq!(hit(1, 1, 0), 1.0, "dil=1 时 ±1 的抽头应命中");
    assert_eq!(hit(2, 1, 0), 0.0, "dil=2 时 ±1 间距上不应有抽头");
    assert_eq!(hit(2, 2, 0), 1.0, "dil=2 时 ±2 的抽头应命中");
    assert_eq!(hit(2, 2, 2), 1.0, "对角线 ±2 也应命中");
}

#[test]
fn eps_denoiser_maps_noise_to_x0() {
    use crate::diffuse::Denoiser;
    // 网络输出恒 0 的 ε ⇒ x0 应等于 x_t/√ᾱ
    let (w, h) = (4usize, 4usize);
    let mut net = Net::new(2, &[], 3);
    net.linear_out = true;
    for l in net.layers.iter_mut() {
        l.b.fill(0.0);
        l.w.fill(0.0);
        l.scale.fill(0.0);
    }
    let abar = vec![1.0f32, 0.5, 0.0];
    let d = EpsDen {
        net,
        abar: abar.clone(),
        eps_scale: 1.0,
    };
    let xt = vec![0.4f32; w * h];
    let x0 = d.predict_x0(&xt, 1.0, None, w, h); // t_frac=1 ⇒ ᾱ=0 ⇒ 走 clamp
    assert_eq!(x0.len(), w * h);
    let x0b = d.predict_x0(&xt, 0.5, None, w, h); // ᾱ=0.5 ⇒ 0.4/√0.5≈0.566
    assert!(
        (x0b[0] - 0.4 / 0.5f32.sqrt()).abs() < 1e-4,
        "实际 {}",
        x0b[0]
    );
}

#[test]
fn highpass_term_penalizes_missing_detail() {
    let (w, h) = (8usize, 8usize);
    let mut tgt = vec![0.0f32; w * h];
    for y in 0..h {
        for x in 0..w {
            tgt[y * w + x] = if (x + y) % 2 == 0 { 1.0 } else { -1.0 };
        }
    }
    let maps = vec![vec![0.0f32; w * h]];
    let mk = |hp: f32| -> f32 {
        let mut n = Net::new(1, &[], 3);
        n.linear_out = true;
        n.highpass = hp;
        for l in n.layers.iter_mut() {
            l.w.fill(0.0);
            l.b.fill(0.0);
            l.scale.fill(1.0);
        }
        n.train_batch_x(&[(maps.clone(), tgt.clone())], w, h, 0.0, false)
    };
    let a = mk(0.0);
    let b = mk(1.0);
    assert!(b > a * 1.5, "高通项应显著加大损失：{a:.3} -> {b:.3}");
}

#[test]
fn v_denoiser_maps_v_to_x0() {
    use crate::diffuse::Denoiser;
    // 网络输出恒 0 的 v ⇒ x̂0 = √ᾱ·x_t
    let (w, h) = (4usize, 4usize);
    let mut net = Net::new(2, &[], 3);
    net.linear_out = true;
    for l in net.layers.iter_mut() {
        l.b.fill(0.0);
        l.w.fill(0.0);
        l.scale.fill(0.0);
    }
    let abar = vec![1.0f32, 0.25, 0.0];
    let d = VDen {
        net,
        abar: abar.clone(),
        extra: Vec::new(),
        clamp: true,
        class: None,
    };
    let xt = vec![0.8f32; w * h];
    let x0 = d.predict_x0(&xt, 0.5, None, w, h); // ᾱ=0.25 ⇒ 0.5*0.8=0.4
    assert!((x0[0] - 0.4).abs() < 1e-4, "实际 {}", x0[0]);
    let x0b = d.predict_x0(&xt, 0.0, None, w, h); // ᾱ=1 ⇒ x_t
    assert!((x0b[0] - 0.8).abs() < 1e-4, "实际 {}", x0b[0]);
}

#[test]
fn film_modulates_by_class() {
    let (w, h) = (6usize, 6usize);
    let mut net = Net::new(1, &[4], 9);
    net.set_film(2);
    // 手工把两类的 γ 设成不同值
    {
        let f = net.films[0].as_mut().unwrap();
        for v in f.g[0..4].iter_mut() {
            *v = 2.0;
        }
        for v in f.g[4..8].iter_mut() {
            *v = 0.5;
        }
    }
    let x = vec![vec![0.7f32; w * h]];
    let a = net.forward_xc(&x, Some(0), w, h);
    let b = net.forward_xc(&x, Some(1), w, h);
    let c = net.forward_xc(&x, None, w, h); // 空类 γ=1
    assert!((a[0] - b[0]).abs() > 1e-4, "不同类别应给出不同输出");
    assert!((c[0] - a[0]).abs() > 1e-4, "空类应与类 0 不同");
}

#[test]
fn film_learns_class_offsets() {
    let (w, h) = (6usize, 6usize);
    let mut net = Net::new(1, &[4], 11);
    net.set_film(2);
    net.linear_out = true;
    let maps = vec![vec![0.5f32; w * h]];
    // 类 0 目标 0.2、类 1 目标 0.8
    let t0 = vec![0.2f32; w * h];
    let t1 = vec![0.8f32; w * h];
    let batch = vec![
        (maps.clone(), t0.clone(), Some(0usize)),
        (maps.clone(), t1.clone(), Some(1usize)),
    ];
    for _ in 0..300 {
        net.train_batch_xc(&batch, w, h, 0.05, true);
    }
    let a = net.forward_xc(&maps, Some(0), w, h)[0];
    let b = net.forward_xc(&maps, Some(1), w, h)[0];
    assert!(b - a > 0.2, "两类应被区分：{a:.3} vs {b:.3}");
}

#[test]
fn coh_loss_grad_matches_finite_difference() {
    let (w, h) = (9usize, 8usize);
    let mut s = 11u64;
    let mut rnd = || {
        s ^= s << 13;
        s ^= s >> 7;
        s ^= s << 17;
        (s >> 11) as f32 / (1u64 << 53) as f32 - 0.5
    };
    let x0: Vec<f32> = (0..w * h).map(|_| rnd()).collect();
    let (cstar, tstar, lam, wt) = (0.573f32, 0.0333f32, 1.0f32, 0.1f32);
    let (_, grad) = coh_loss_grad(&x0, w, h, cstar, tstar, lam, wt);
    let eps = 1e-3f32;
    for &i in &[w + 2, 3 * w + 4, 5 * w + 5] {
        let mut up = x0.clone();
        up[i] += eps;
        let mut dn = x0.clone();
        dn[i] -= eps;
        let fu = coh_loss_grad(&up, w, h, cstar, tstar, lam, wt).0;
        let fd = coh_loss_grad(&dn, w, h, cstar, tstar, lam, wt).0;
        let num = (fu - fd) / (2.0 * eps);
        let ana = grad[i];
        assert!(
            (num - ana).abs() < 0.05 * num.abs().max(0.05),
            "像素 {i}: 数值 {num:.4} vs 解析 {ana:.4}"
        );
    }
}

#[test]
fn sobel_adjoint_is_transpose() {
    let (w, h) = (7usize, 6usize);
    let mut s = 7u64;
    let mut rnd = || {
        s ^= s << 13;
        s ^= s >> 7;
        s ^= s << 17;
        (s >> 11) as f32 / (1u64 << 53) as f32 - 0.5
    };
    let x: Vec<f32> = (0..w * h).map(|_| rnd()).collect();
    let cx: Vec<f32> = (0..w * h).map(|_| rnd()).collect();
    let cy: Vec<f32> = (0..w * h).map(|_| rnd()).collect();
    let (gx, gy) = sobel(&x, w, h);
    let lhs: f32 = (0..w * h).map(|i| gx[i] * cx[i] + gy[i] * cy[i]).sum();
    let adj = sobel_adjoint(&cx, &cy, w, h);
    let rhs: f32 = (0..w * h).map(|i| x[i] * adj[i]).sum();
    assert!(
        (lhs - rhs).abs() < 1e-3 * lhs.abs().max(1.0),
        "adjoint 不是转置：{lhs} vs {rhs}"
    );
}

#[test]
fn denoiser_api_feeds_t_channel() {
    use crate::diffuse::Denoiser;
    let (w, h) = (4usize, 4usize);
    let mut net = Net::new(2, &[4], 5);
    net.set_bn(false);
    let xt = vec![0.3f32; w * h];
    let p = net.predict_x0(&xt, 0.5, None, w, h);
    assert_eq!(p.len(), w * h);
    assert!(p.iter().all(|v| (0.0..=1.0).contains(v)));
    assert_eq!(Denoiser::input_dim(&net), 2);
}

#[test]
fn bn_backward_matches_numeric() {
    // 纯 BN：y = γ·x̂+β，loss = Σ dout·y，检查 dx 与有限差分一致
    let n = 6usize;
    let x0 = vec![0.2f32, 0.9, 0.4, 0.7, 0.1, 0.5];
    let dout = vec![0.3f32, -0.2, 0.5, 0.1, -0.4, 0.2];
    let loss = |x: &[f32]| -> f64 {
        let mut bn = Bn::new(1);
        bn.gamma[0] = 1.3;
        bn.beta[0] = -0.2;
        let (y, _) = bn.forward(&[x.to_vec()], n, true);
        y[0].iter().zip(&dout).map(|(a, b)| f64::from(a * b)).sum()
    };
    let mut bn = Bn::new(1);
    bn.gamma[0] = 1.3;
    bn.beta[0] = -0.2;
    let (_, xh) = bn.forward(std::slice::from_ref(&x0), n, true);
    let (dx, _dg, _db) = bn.backward(&xh, std::slice::from_ref(&dout), n);
    let eps = 1e-3f32;
    for i in 0..n {
        let mut up = x0.clone();
        up[i] += eps;
        let mut dn = x0.clone();
        dn[i] -= eps;
        let num = (loss(&up) - loss(&dn)) / (2.0 * f64::from(eps));
        let ana = f64::from(dx[0][i]);
        assert!(
            (num - ana).abs() < 0.05 * ana.abs().max(1e-2),
            "i={i}: 数值 {num:.4} vs 解析 {ana:.4}"
        );
    }
}

#[test]
fn gradient_matches_finite_difference() {
    // 小网络：1→1 单层 + sigmoid，检查 dL/dw 与数值差分一致
    let w = 4;
    let h = 4;
    let img: Vec<f32> = (0..16).map(|i| ((i * 37) % 11) as f32 / 11.0).collect();
    let target: Vec<f32> = (0..16).map(|i| ((i * 53) % 13) as f32 / 13.0).collect();
    let mut net = Net::new(1, &[], 7);
    net.layers[0].ternary = false;
    let loss_at = |net: &Net| -> f64 {
        let p = net.forward(&img, None, w, h);
        let mut l = 0.0f64;
        for i in 0..w * h {
            let q = p[i].clamp(1e-6, 1.0 - 1e-6);
            l -= f64::from(target[i] * q.ln() + (1.0 - target[i]) * (1.0 - q).ln());
        }
        l
    };
    // 用一次 train_step 的梯度方向估计：解析梯度 = -(w_new - w_old) * n / lr
    let before = net.layers[0].w.clone();
    let lr = 1e-2;
    net.train_step(&img, &target, None, w, h, lr);
    let after = net.layers[0].w.clone();
    let mut checked = 0;
    for k in 0..before.len() {
        let ana = -((after[k] - before[k]) / lr) as f64 * (w * h) as f64; // 更新用的是均值损失
        if ana.abs() < 1e-3 {
            continue;
        }
        let eps = 1e-2f32;
        let mut up = Net::new(1, &[], 7);
        up.layers[0].ternary = false;
        up.layers[0].w = before.clone();
        up.layers[0].w[k] += eps;
        let mut dn = Net::new(1, &[], 7);
        dn.layers[0].ternary = false;
        dn.layers[0].w = before.clone();
        dn.layers[0].w[k] -= eps;
        let num = (loss_at(&up) - loss_at(&dn)) / (2.0 * f64::from(eps));
        assert!(
            (num - ana).abs() < 0.1 * ana.abs().max(1e-2),
            "k={k}: 数值 {num:.4} vs 解析 {ana:.4}"
        );
        checked += 1;
    }
    assert!(checked > 3, "应至少校验 3 个权重");
}
