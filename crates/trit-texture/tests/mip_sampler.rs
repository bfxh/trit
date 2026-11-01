//! `MipSampler` 与既有自由函数的对拍：口径必须逐字节一致，差别只允许是「多了缓存/矩形」。

use trit_texture::convert::rgba_to_ttx2_mips;
use trit_texture::mip::MipSampler;
use trit_texture::sample::{Rect, Wrap, sample_aniso};
use trit_texture::ttx2::{TtxDocument, from_ttx2};

/// 64×40（4×10 块对齐）竖条纹 + 逐块偏色：块边界两侧差异大，错块必被抓到。
fn stripes(width: usize, height: usize) -> Vec<u8> {
    let mut px = vec![0u8; width * height * 4];
    for y in 0..height {
        for x in 0..width {
            let o = (y * width + x) * 4;
            let stripe = if (x / 2) % 2 == 0 { 220 } else { 30 };
            px[o] = stripe;
            px[o + 1] = stripe.wrapping_add((y % 5) as u8 * 6);
            px[o + 2] = stripe.wrapping_sub((x % 3) as u8 * 9);
            px[o + 3] = 255;
        }
    }
    px
}

fn doc() -> TtxDocument {
    let (w, h) = (64usize, 40usize);
    let bytes = rgba_to_ttx2_mips(&stripes(w, h), w, h).expect("encode");
    from_ttx2(&bytes).expect("parse")
}

/// 确定性 LCG：别用 rand，判据要能复算。
fn lcg(state: &mut u64) -> f32 {
    *state = state
        .wrapping_mul(6_364_136_223_846_793_005)
        .wrapping_add(1_442_695_040_888_963_407);
    ((*state >> 40) as f32) / ((1u64 << 24) as f32)
}

fn grid(n: usize) -> Vec<(f32, f32)> {
    let mut state = 0x5eed_1234_5678_9abc_u64;
    (0..n)
        .map(|_| {
            let u = lcg(&mut state) * 2.0 - 0.5;
            let v = lcg(&mut state) * 2.0 - 0.5;
            (u, v)
        })
        .collect()
}

#[test]
fn trilinear_matches_document() {
    let d = doc();
    let levels = d.levels().expect("levels");
    for wrap in [Wrap::Clamp, Wrap::Repeat, Wrap::Mirror] {
        let mut sampler = MipSampler::with_wrap(levels, wrap);
        for (i, &(u, v)) in grid(256).iter().enumerate() {
            let lod = (i % 40) as f32 * 0.137;
            let want = d.sample_trilinear_with(u, v, lod, wrap).expect("doc");
            let got = sampler.sample_trilinear(u, v, lod).expect("mip");
            assert_eq!(got, want, "u={u} v={v} lod={lod} wrap={wrap:?}");
        }
    }
}

#[test]
fn aniso_matches_free_function() {
    let d = doc();
    let levels = d.levels().expect("levels");
    let mut sampler = MipSampler::with_wrap(levels, Wrap::Repeat);
    let scales = [0.0f32, 0.25, 1.0, 3.7, 16.0, 64.0];
    for &sx in &scales {
        for &sy in &scales {
            for &(u, v) in grid(12).iter() {
                for taps in [1u32, 2, 3, 8] {
                    let (du_x, dv_x) = (sx / 64.0, sy / 64.0 * 0.25);
                    let (du_y, dv_y) = (sx / 40.0 * 0.1, sy / 40.0);
                    let want = sample_aniso(&d, u, v, du_x, dv_x, du_y, dv_y, taps, Wrap::Repeat)
                        .expect("free");
                    let got = sampler
                        .sample_aniso(u, v, du_x, dv_x, du_y, dv_y, taps)
                        .expect("mip");
                    assert_eq!(
                        got, want,
                        "u={u} v={v} du=({du_x},{dv_x}) dv=({du_y},{dv_y}) taps={taps}"
                    );
                }
            }
        }
    }
}

#[test]
fn trilinear_at_integer_lod_equals_level_bilinear() {
    let d = doc();
    let levels = d.levels().expect("levels");
    let mut sampler = MipSampler::with_wrap(levels, Wrap::Clamp);
    let mut single = trit_texture::sample::Sampler::new(&levels[0]);
    for &(u, v) in grid(64).iter() {
        assert_eq!(
            sampler.sample_trilinear(u, v, 0.0).expect("mip"),
            single.sample_bilinear(u, v),
            "lod=0 必须与一级双线性逐字节相同"
        );
    }
}

/// 图集矩形：矩形外是纯蓝，矩形内是灰度；强各向异性打横条时必须一个蓝都不出。
#[test]
fn rect_aniso_never_leaks_outside() {
    let (w, h) = (64usize, 40usize);
    let rect = Rect::new(16, 10, 32, 20).expect("rect");
    let mut px = vec![0u8; w * h * 4];
    for y in 0..h {
        for x in 0..w {
            let o = (y * w + x) * 4;
            let inside =
                x >= rect.x && x < rect.x + rect.width && y >= rect.y && y < rect.y + rect.height;
            let (r, g, b) = if inside {
                let c = if (x / 2) % 2 == 0 { 230 } else { 25 };
                (c, c, c)
            } else {
                (0, 0, 255)
            };
            px[o] = r;
            px[o + 1] = g;
            px[o + 2] = b;
            px[o + 3] = 255;
        }
    }
    let bytes = rgba_to_ttx2_mips(&px, w, h).expect("encode");
    let d = from_ttx2(&bytes).expect("parse");
    let levels = d.levels().expect("levels");

    // 长轴 8 纹素、短轴 0.5 纹素 ⇒ taps=8 且 lod=0，只碰级别 0 的原纹素
    let (du_x, dv_x, du_y, dv_y) = (8.0 / w as f32, 0.0, 0.0, 0.5 / h as f32);

    let mut atlas = MipSampler::new(levels);
    atlas.set_rect(Some(rect));
    let mut full = MipSampler::new(levels);

    let mut leaked_outside = 0;
    for step in 0..64 {
        let u = 0.02 + step as f32 * 0.015; // 覆盖矩形左缘附近
        let v = 0.5;
        let a = atlas
            .sample_aniso(u, v, du_x, dv_x, du_y, dv_y, 8)
            .expect("atlas");
        // 矩形内是灰度：任何蓝渗入都会让 b 超过 r/g
        assert_eq!(a[0], a[1], "u={u} 灰度被破坏：{a:?}");
        assert_eq!(a[1], a[2], "u={u} 矩形内采样出现了蓝：{a:?}");
        let f = full
            .sample_aniso(u, v, du_x, dv_x, du_y, dv_y, 8)
            .expect("full");
        if f[2] > f[0] {
            leaked_outside += 1;
        }
    }
    assert!(
        leaked_outside > 0,
        "对照组一个蓝都没采到 ⇒ 这个用例没有区分力"
    );
}

#[test]
fn repeat_is_periodic() {
    let d = doc();
    let levels = d.levels().expect("levels");
    let mut sampler = MipSampler::with_wrap(levels, Wrap::Repeat);
    for &(u, v) in grid(32).iter() {
        let a = sampler.sample_trilinear(u, v, 1.5).expect("a");
        let b = sampler.sample_trilinear(u + 1.0, v - 2.0, 1.5).expect("b");
        assert_eq!(a, b, "repeat 下 u={u} v={v} 不平移不变");
    }
}
