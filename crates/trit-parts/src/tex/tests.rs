use super::quilt::{luma_of, sm64};
use super::*;
#[test]
fn tiles_are_seamless_and_distinct() {
    for kind in ["wood", "marble", "rust", "grid"] {
        let t = tile(kind, 128, 7);
        assert_eq!(t.len(), 128 * 128);
        // 周期函数 ⇒ 边界差与内部差同量级（无硬接缝）
        let r = seam_ratio(&t, 128);
        assert!(r < 2.0, "{kind} 接缝比 {r:.2}");
    }
    let a = tile("wood", 64, 1);
    let b = tile("marble", 64, 1);
    let d: u32 = a
        .iter()
        .zip(&b)
        .map(|(x, y)| {
            (0..3)
                .map(|k| (x[k] as i32 - y[k] as i32).unsigned_abs())
                .sum::<u32>()
        })
        .sum();
    assert!(d > 64 * 64 * 30, "两种材质应有明显差异（总差 {d}）");
}

fn stripes(n: usize, period: usize) -> Vec<f32> {
    let mut v = Vec::with_capacity(n * n * 3);
    for _y in 0..n {
        for x in 0..n {
            let c = if (x / (period / 2)).is_multiple_of(2) {
                0.85
            } else {
                0.15
            };
            v.extend_from_slice(&[c, c, c]);
        }
    }
    v
}

fn sample_img(n: usize) -> Vec<f32> {
    // 有方向性 + 多色阶的合成图（亮条纹 + 噪声），便于测因子
    let mut st = 12345u64;
    let mut v = Vec::with_capacity(n * n * 3);
    for y in 0..n {
        for x in 0..n {
            let stripe = if (x / 6) % 2 == 0 { 0.75 } else { 0.35 };
            let g = sm64(&mut st) % 1000;
            let noise = (g as f32 / 1000.0 - 0.5) * 0.08;
            let base = (stripe + noise).clamp(0.0, 1.0);
            v.extend_from_slice(&[base, base * 0.9 + 0.05, base * 0.75 + 0.1]);
        }
        let _ = y;
    }
    v
}

#[test]
fn preset_zero_strength_is_identity() {
    let img = sample_img(32);
    for p in PRESETS {
        assert_eq!(
            p.style(0.0),
            Style::default(),
            "{}: t=0 应为默认参数",
            p.name
        );
        assert_eq!(
            stylize(&img, 32, 32, &p.style(0.0)),
            img,
            "{}: t=0 必须逐位不变",
            p.name
        );
    }
}

#[test]
fn preset_names_are_unique() {
    let mut names: Vec<&str> = PRESETS.iter().map(|p| p.name).collect();
    names.sort_unstable();
    let n = names.len();
    names.dedup();
    assert_eq!(names.len(), n, "预设名不能重复");
    assert!(Preset::by_name("cel").is_some());
    assert!(Preset::by_name("nope").is_none());
}

#[test]
fn preset_strength_is_monotone_in_its_defining_factors() {
    let (w, h) = (64usize, 64usize);
    // 细纹理彩色底：stroke 不撞上限，色阶/对比度也都有空间
    let mut st = 4242u64;
    // 中调区噪声（0.25–0.75）：对比度拉到 1.35 也不会削顶，否则裁剪会让 contrast 指标假跌
    let img: Vec<f32> = (0..w * h * 3)
        .map(|i| {
            let base = 0.25 + 0.5 * ((sm64(&mut st) >> 40) as f32 / (1u64 << 24) as f32);
            base * [1.0, 0.9, 0.8][i % 3]
        })
        .collect();
    let ts = [0.0f32, 0.25, 0.5, 0.75, 1.0];
    for p in PRESETS {
        let v: Vec<crate::look::Look> = ts
            .iter()
            .map(|&t| crate::look::analyze(&stylize(&img, w, h, &p.style(t)), w, h))
            .collect();
        match p.name {
            "cel" => {
                // look.levels 是 luma 直方图档数，逐通道量化后 luma 会组合出更多值；
                // 验收「posterize」要看**通道实际离散值数** ≤ 请求档数，并随强度单调减少。
                let distinct: Vec<usize> = ts
                    .iter()
                    .map(|&t| {
                        let o = stylize(&img, w, h, &p.style(t));
                        let mut set: Vec<u32> = o.iter().map(|v| (v * 1e6) as u32).collect();
                        set.sort_unstable();
                        set.dedup();
                        set.len()
                    })
                    .collect();
                let mono = distinct.windows(2).all(|x| x[1] <= x[0]);
                assert!(mono, "cel 量化档数应随强度单调减少，实际 {distinct:?}");
                for (i, &d) in distinct.iter().enumerate() {
                    let req = p.style(ts[i]).levels as usize;
                    let want = if req == 0 { usize::MAX } else { req };
                    assert!(d <= want, "t={}: 离散值 {d} 应 ≤ 请求档数 {want}", ts[i]);
                }
                let contrast_up = v.windows(2).all(|x| x[1].contrast >= x[0].contrast - 1e-3);
                assert!(
                    contrast_up,
                    "cel 对比度应单调上升，实际 {:?}",
                    v.iter().map(|l| l.contrast).collect::<Vec<_>>()
                );
            }
            "oil" => {
                let s_up = v.windows(2).all(|x| x[1].stroke >= x[0].stroke - 1e-3);
                assert!(
                    s_up,
                    "oil 笔触尺度应单调上升，实际 {:?}",
                    v.iter().map(|l| l.stroke).collect::<Vec<_>>()
                );
                let w_up = v.windows(2).all(|x| x[1].warmth >= x[0].warmth - 1e-3);
                assert!(
                    w_up,
                    "oil 色温应单调上升，实际 {:?}",
                    v.iter().map(|l| l.warmth).collect::<Vec<_>>()
                );
            }
            "epic" => {
                let c_up = v.windows(2).all(|x| x[1].contrast >= x[0].contrast - 1e-3);
                assert!(
                    c_up,
                    "epic 对比度应单调上升，实际 {:?}",
                    v.iter().map(|l| l.contrast).collect::<Vec<_>>()
                );
                let w_up = v.windows(2).all(|x| x[1].warmth >= x[0].warmth - 1e-3);
                assert!(
                    w_up,
                    "epic 色温应单调上升，实际 {:?}",
                    v.iter().map(|l| l.warmth).collect::<Vec<_>>()
                );
            }
            _ => unreachable!(),
        }
    }
}

#[test]
fn stylize_identity_is_bit_exact() {
    let img = sample_img(32);
    let out = stylize(&img, 32, 32, &Style::default());
    assert_eq!(out, img, "默认参数必须逐位不变");
}

#[test]
fn stylize_knobs_move_their_own_factor_monotonically() {
    let (n, w, h) = (48usize, 48usize, 48usize);
    let img = sample_img(n);
    let look = |s: &Style| {
        let o = stylize(&img, w, h, s);
        crate::look::analyze(&o, w, h)
    };
    let base = look(&Style::default());

    let lo = look(&Style {
        levels: 4,
        ..Default::default()
    });
    let hi = look(&Style {
        levels: 32,
        ..Default::default()
    });
    assert!(
        hi.levels > lo.levels,
        "色阶：32 档应比 4 档测出更多级数（{} vs {}）",
        hi.levels,
        lo.levels
    );

    let c1 = look(&Style {
        contrast: 0.6,
        ..Default::default()
    });
    let c2 = look(&Style {
        contrast: 1.8,
        ..Default::default()
    });
    assert!(
        c2.contrast > c1.contrast,
        "对比度应单调（{} vs {}）",
        c2.contrast,
        c1.contrast
    );

    let warm = look(&Style {
        warmth: 0.25,
        ..Default::default()
    });
    assert!(
        warm.warmth > base.warmth,
        "色温应上升（{} vs {}）",
        warm.warmth,
        base.warmth
    );

    let sat = look(&Style {
        sat: 1.6,
        ..Default::default()
    });
    assert!(
        sat.sat > base.sat,
        "饱和度应上升（{} vs {}）",
        sat.sat,
        base.sat
    );

    // 笔触尺度要看「相关长度」：条纹图的相关长度被条纹周期主导，改用纯噪声图
    let mut st = 999u64;
    let noise: Vec<f32> = (0..n * n * 3)
        .map(|_| (sm64(&mut st) >> 40) as f32 / (1u64 << 24) as f32)
        .collect();
    let s0 = crate::look::analyze(&noise, w, h).stroke;
    let s2 = crate::look::analyze(
        &stylize(
            &noise,
            w,
            h,
            &Style {
                stroke: 3.0,
                ..Default::default()
            },
        ),
        w,
        h,
    )
    .stroke;
    let s_sharp = crate::look::analyze(
        &stylize(
            &noise,
            w,
            h,
            &Style {
                stroke: -1.0,
                ..Default::default()
            },
        ),
        w,
        h,
    )
    .stroke;
    assert!(s2 > s0, "模糊应拉长相关长度（{} vs {}）", s2, s0);
    assert!(
        s_sharp <= s0 + 1e-3,
        "锐化不该拉长相关长度（{} vs {}）",
        s_sharp,
        s0
    );
}

#[test]
fn stylize_is_deterministic() {
    let img = sample_img(24);
    let s = Style {
        contrast: 1.3,
        levels: 12,
        warmth: 0.1,
        sat: 1.2,
        stroke: 1.0,
    };
    assert_eq!(stylize(&img, 24, 24, &s), stylize(&img, 24, 24, &s));
}

fn ramp_with_noise(n: usize) -> Vec<f32> {
    let mut st = 77u64;
    let mut v = Vec::with_capacity(n * n * 3);
    for _y in 0..n {
        for x in 0..n {
            let ramp = 0.15 + 0.7 * (x as f32 / n as f32);
            let noise = ((sm64(&mut st) >> 40) as f32 / (1u64 << 24) as f32 - 0.5) * 0.06;
            let c = (ramp + noise).clamp(0.0, 1.0);
            v.extend_from_slice(&[c, c, c]);
        }
    }
    v
}

fn lr_diff(img: &[f32], n: usize) -> f32 {
    let q = n / 4;
    let mut left = 0.0f32;
    let mut right = 0.0f32;
    let mut cnt = 0.0f32;
    for y in 0..n {
        for x in 0..q {
            left += img[(y * n + x) * 3];
            right += img[(y * n + (n - 1 - x)) * 3];
            cnt += 1.0;
        }
    }
    (right - left) / cnt
}

#[test]
fn detrend_removes_and_relight_restores_the_global_gradient() {
    let n = 96usize;
    let src = ramp_with_noise(n);
    let d_src = lr_diff(&src, n);
    assert!(d_src > 0.3, "测试源本身要有明显梯度，实际 {d_src}");
    // 只去梯度不回贴：输出应基本平坦
    let flat = quilt(&src, n, n, n, n, 16, 4, 5, false, 16, false, false);
    let d_flat = lr_diff(&flat, n).abs();
    assert!(
        d_flat < d_src * 0.35,
        "detrend 后梯度应大幅减小：{d_flat:.3} vs 源 {d_src:.3}"
    );
    // 去梯度 + 分位回贴：对比度（p95−p5）应回到源的 80% 以上
    let lit = quilt(&src, n, n, n, n, 16, 4, 5, false, 16, true, false);
    let p = |img: &[f32]| -> f32 {
        let mut v: Vec<f32> = img.chunks(3).map(luma_of).collect();
        v.sort_by(f32::total_cmp);
        let q = |f: f32| v[((v.len() as f32 - 1.0) * f).round() as usize];
        q(0.95) - q(0.05)
    };
    let (cs, cl) = (p(&src), p(&lit));
    assert!(
        cl >= cs * 0.8,
        "分位回贴后对比度应接近源：{cl:.3} vs {cs:.3}"
    );
}

#[test]
fn relight_plane_restores_a_gentle_ramp_but_not_full_contrast() {
    // 平面回贴：左右亮度差应恢复（≥70% 源），但整体对比度不应达到源（这是它和分布回贴的区别）
    let n = 96usize;
    let src = ramp_with_noise(n);
    let d_src = lr_diff(&src, n);
    let lit = quilt(&src, n, n, n, n, 16, 4, 5, false, 16, false, true);
    let d_lit = lr_diff(&lit, n);
    assert!(
        d_lit >= d_src * 0.7,
        "平面回贴后亮度梯度应恢复：{d_lit:.3} vs 源 {d_src:.3}"
    );
    let p = |img: &[f32]| -> f32 {
        let mut v: Vec<f32> = img.chunks(3).map(luma_of).collect();
        v.sort_by(f32::total_cmp);
        let q = |f: f32| v[((v.len() as f32 - 1.0) * f).round() as usize];
        q(0.95) - q(0.05)
    };
    assert!(
        p(&lit) < p(&src),
        "平面回贴不应把对比度拉满（分布回贴才会）"
    );
}

#[test]
fn quilt_outputs_the_requested_size_and_stays_in_range() {
    let src = stripes(32, 8);
    let out = quilt(&src, 32, 32, 64, 48, 8, 4, 7, false, 0, false, false);
    assert_eq!(out.len(), 64 * 48 * 3);
    assert!(out.iter().all(|v| (0.0..=1.0).contains(v)));
    // 源只有两档灰度 ⇒ 输出也只应落在两档附近（羽化区允许中间值）
    let mid = out.iter().filter(|v| (0.3..0.7).contains(*v)).count();
    assert!(
        mid < out.len() / 4,
        "羽化区外的值应保持源图两档，中间值 {mid}"
    );
}

#[test]
fn quilt_is_deterministic_and_seed_sensitive() {
    let src = stripes(32, 8);
    let a = quilt(&src, 32, 32, 64, 64, 8, 4, 1, false, 0, false, false);
    let b = quilt(&src, 32, 32, 64, 64, 8, 4, 1, false, 0, false, false);
    assert_eq!(a, b, "同 seed 必须逐位一致");
    let c = quilt(&src, 32, 32, 64, 64, 8, 4, 2, false, 0, false, false);
    assert_ne!(a, c, "不同 seed 应给出不同拼法");
}

#[test]
fn quilt_preserves_directionality_where_diffusion_does_not() {
    // 竖条纹源：合成图必须仍是大幅各向异性；随机噪声源作对照必须是各向同性
    let src = stripes(48, 8);
    let out = quilt(&src, 48, 48, 64, 64, 12, 6, 3, false, 0, false, false);
    let l = crate::look::analyze(&out, 64, 64);
    assert!(
        l.coherence > 0.6,
        "竖条纹应保住方向性，实际 {:.3}",
        l.coherence
    );
    assert!(
        l.stroke > 2.0,
        "笔触尺度应接近源的条纹宽度，实际 {:.2}",
        l.stroke
    );

    let mut st = 99u64;
    let noise: Vec<f32> = (0..48 * 48 * 3)
        .map(|_| (sm64(&mut st) >> 40) as f32 / (1u64 << 24) as f32)
        .collect();
    let l2 = crate::look::analyze(&noise, 48, 48);
    assert!(
        l2.coherence < 0.4,
        "对照：噪声应各向同性，实际 {:.3}",
        l2.coherence
    );
}
