//! 非参数 patch 合成(`tex-quilt` 的正本):Efros–Freeman 随机候选 + 重叠 SSD 选优。
//!
//! 接缝与可平铺是台账点名的两个缺口(§6.47「规范做法是 graph-cut 缝合或周期包装」):
//! - `cut`(DP 最小误差切割,`seam.rs`):只沿最小误差路径切,替代整片线性羽化,
//!   细纹不再被混平(§6.75 实测羽化 hf1 −37%);
//! - `tile`(周期包装):在**环面**上合成——网格每轴补一个跨边界的收尾 patch,
//!   回绕侧与内缝走同一套接缝机制 ⇒ 输出**严格可平铺**(边界比 ≈1)。
//!
//! 预算:原 `quilt.rs` 282 行顶格,新旋钮进本文件;`quilt` 保留旧签名
//! (行为与拆分前逐位一致),新能力走 [`quilt_ex`] + [`QuiltSpec`]。

use super::super::util::{fit_plane, luma_of, sm64};
use super::seam::{horizontal_cut, vertical_cut};

/// `quilt_ex` 的参数面。12 个位置参数之后旋钮还在涨,收成结构体。
#[derive(Debug, Clone, Copy)]
pub struct QuiltSpec {
    pub out_w: usize,
    pub out_h: usize,
    pub patch: usize,
    pub overlap: usize,
    pub seed: u64,
    pub tone_match: bool,
    pub detrend: usize,
    pub relight: bool,
    pub relight_plane: bool,
    /// DP 最小误差切割缝合(false = 线性羽化)
    pub cut: bool,
    /// 环面合成 ⇒ 输出严格可平铺
    pub tile: bool,
}

/// 旧签名(线性羽化、不可平铺),行为与拆分前逐位一致;新旋钮用 [`quilt_ex`]。
#[allow(clippy::too_many_arguments)]
pub fn quilt(
    src: &[f32],
    sw: usize,
    sh: usize,
    out_w: usize,
    out_h: usize,
    patch: usize,
    overlap: usize,
    seed: u64,
    tone_match: bool,
    detrend: usize,
    relight: bool,
    relight_plane: bool,
) -> Vec<f32> {
    quilt_ex(
        src,
        sw,
        sh,
        &QuiltSpec {
            out_w,
            out_h,
            patch,
            overlap,
            seed,
            tone_match,
            detrend,
            relight,
            relight_plane,
            cut: false,
            tile: false,
        },
    )
}

/// 非参数 patch 合成的全参数入口。
///
/// `src` 为交错 RGB(0..1,`sw×sh`);输出 `out_w×out_h` 同格式。
/// 每个 patch 只随机试 64 个候选,选与已落区域 SSD 最小的那个 ⇒ 确定性(同 seed 逐位一致)。
pub fn quilt_ex(src: &[f32], sw: usize, sh: usize, spec: &QuiltSpec) -> Vec<f32> {
    let QuiltSpec {
        out_w,
        out_h,
        patch,
        overlap,
        seed,
        tone_match,
        detrend,
        relight,
        relight_plane,
        cut,
        tile,
    } = *spec;
    let expect = sw
        .checked_mul(sh)
        .and_then(|a| a.checked_mul(3))
        .expect("源图尺寸溢出");
    assert!(src.len() == expect, "源图长度与 sw×sh×3 不符");
    assert!(patch >= 2 && patch <= sw.min(sh), "patch 必须 ≤ 源图短边");
    assert!(out_w >= patch && out_h >= patch, "输出不能小于一个 patch");
    let cells = out_w
        .checked_mul(out_h)
        .and_then(|c| c.checked_mul(3))
        .expect("输出尺寸溢出");
    // 环面模式:重叠必须落在 [1, patch/2]——ov ≤ step(= patch − ov)保证
    // 左带与回绕右带不重叠,回绕收尾 patch 的位置也才合法
    let ov = if tile {
        overlap.max(1).min(patch / 2)
    } else {
        overlap.min(patch - 1)
    };
    let step = patch - ov;
    let mut out = vec![0.0f32; cells];
    let mut placed = vec![false; cells / 3];
    // 网格:常规铺法到最后一个整 patch;tile 再补一个**跨边界收尾** patch
    // (位置 dim−step,覆盖 [dim−step, dim) ∪ [0, ov)),回绕接缝由此走同一套机制
    let grid = |dim: usize| -> Vec<usize> {
        let mut v = Vec::new();
        let mut x = 0;
        while x + patch < dim {
            v.push(x);
            x += step;
        }
        v.push(dim - patch);
        if tile {
            let w = dim - step;
            if !v.contains(&w) {
                v.push(w);
            }
        }
        v
    };
    let xs = grid(out_w);
    let ys = grid(out_h);
    // 去低频(光照梯度):拟合并扣除线性光照,拼完再回贴 —— 拼接口会抹平跨区域光照,
    // 这是对比度 p95−p5 大幅缩水的主因(Planks008 实测 −78%)。

    let src_orig: &[f32] = src;
    // 平面回贴(旧行为):只在 `--relight-plane` 时用;分布回贴不需要平面。
    let plane = if relight_plane {
        fit_plane(src, sw, sh)
    } else {
        [[0.0f32; 3]; 3]
    };
    let work: Vec<f32> = if detrend > 0 {
        let lf = box_blur_rgb(src, sw, sh, detrend);
        src.iter().zip(&lf).map(|(a, b)| a - b).collect()
    } else {
        src.to_vec()
    };
    let src: &[f32] = &work;
    let mut state = seed;
    let tries = 64u64;
    // 源图逐通道均值/方差:tone-match 时把每个 patch 拉到同一均值/方差,
    // 抵消「不同区域的光照梯度在拼接时被抹平」(实测 contrast p95−p5 可掉 65–78%)
    let gmean = if tone_match {
        let n = (sw * sh) as f32;
        let mut m = [0.0f32; 3];
        for p in src.chunks(3) {
            for c in 0..3 {
                m[c] += p[c];
            }
        }
        for v in m.iter_mut() {
            *v /= n;
        }
        Some(m)
    } else {
        None
    };
    let mut src_std = [0.0f32; 3];
    if let Some(gm) = gmean {
        let n = (sw * sh) as f32;
        let mut v = [0.0f32; 3];
        for p in src.chunks(3) {
            for c in 0..3 {
                v[c] += (p[c] - gm[c]).powi(2);
            }
        }
        for c in 0..3 {
            src_std[c] = (v[c] / n).sqrt().max(1e-4);
        }
    }
    for &oy in &ys {
        for &ox in &xs {
            let mut best = (f32::INFINITY, 0usize, 0usize);
            for _ in 0..tries {
                let cx = (sm64(&mut state) % (sw - patch + 1) as u64) as usize;
                let cy = (sm64(&mut state) % (sh - patch + 1) as u64) as usize;
                let mut cost = 0.0f32;
                for dy in 0..patch {
                    let gy = if tile { (oy + dy) % out_h } else { oy + dy };
                    for dx in 0..patch {
                        let gx = if tile { (ox + dx) % out_w } else { ox + dx };
                        if !placed[gy * out_w + gx] {
                            continue;
                        }
                        let s = ((cy + dy) * sw + cx + dx) * 3;
                        let o = (gy * out_w + gx) * 3;
                        for c in 0..3 {
                            let d = src[s + c] - out[o + c];
                            cost += d * d;
                        }
                    }
                }
                if cost < best.0 {
                    best = (cost, cx, cy);
                }
            }
            let (_, cx, cy) = best;
            // 选定 patch 的逐通道均值/方差(tone-match 用)
            let pstat = if tone_match {
                let n = (patch * patch) as f32;
                let mut m = [0.0f32; 3];
                for dy in 0..patch {
                    for dx in 0..patch {
                        let s0 = ((cy + dy) * sw + cx + dx) * 3;
                        for c in 0..3 {
                            m[c] += src[s0 + c];
                        }
                    }
                }
                for v in m.iter_mut() {
                    *v /= n;
                }
                let mut vv = [0.0f32; 3];
                for dy in 0..patch {
                    for dx in 0..patch {
                        let s0 = ((cy + dy) * sw + cx + dx) * 3;
                        for c in 0..3 {
                            vv[c] += (src[s0 + c] - m[c]).powi(2);
                        }
                    }
                }
                let mut sd = [0.0f32; 3];
                for c in 0..3 {
                    sd[c] = (vv[c] / n).sqrt().max(1e-4);
                }
                Some((m, sd))
            } else {
                None
            };
            // 接缝切割(cut 模式):对每条重叠带算误差场并求最小误差路径。
            // 带的存在性看**已落像素**:首 patch 无带;环面收尾 patch 的回绕侧
            // (右/下带)与常规左/上带同权。竖带 w=ov、h=patch;横带 w=patch、h=ov。
            let band_v = |lo: usize| -> Option<Vec<usize>> {
                let mut any = false;
                let mut err = vec![0.0f32; ov * patch];
                for dy in 0..patch {
                    let gy = if tile { (oy + dy) % out_h } else { oy + dy };
                    for b in 0..ov {
                        let gx = if tile {
                            (ox + lo + b) % out_w
                        } else {
                            ox + lo + b
                        };
                        if !placed[gy * out_w + gx] {
                            continue;
                        }
                        any = true;
                        let s = ((cy + dy) * sw + cx + lo + b) * 3;
                        let o = (gy * out_w + gx) * 3;
                        let mut e = 0.0f32;
                        for c in 0..3 {
                            let d = src[s + c] - out[o + c];
                            e += d * d;
                        }
                        err[dy * ov + b] = e;
                    }
                }
                if any {
                    Some(vertical_cut(&err, ov, patch))
                } else {
                    None
                }
            };
            let band_h = |lo: usize| -> Option<Vec<usize>> {
                let mut any = false;
                let mut err = vec![0.0f32; ov * patch];
                for b in 0..ov {
                    let gy = if tile {
                        (oy + lo + b) % out_h
                    } else {
                        oy + lo + b
                    };
                    for dx in 0..patch {
                        let gx = if tile { (ox + dx) % out_w } else { ox + dx };
                        if !placed[gy * out_w + gx] {
                            continue;
                        }
                        any = true;
                        let s = ((cy + lo + b) * sw + cx + dx) * 3;
                        let o = (gy * out_w + gx) * 3;
                        let mut e = 0.0f32;
                        for c in 0..3 {
                            let d = src[s + c] - out[o + c];
                            e += d * d;
                        }
                        err[b * patch + dx] = e;
                    }
                }
                if any {
                    Some(horizontal_cut(&err, patch, ov))
                } else {
                    None
                }
            };
            let (vcut, hcut, rcut, bcut) = if cut {
                (
                    band_v(0),
                    band_h(0),
                    if tile { band_v(step) } else { None },
                    if tile { band_h(step) } else { None },
                )
            } else {
                (None, None, None, None)
            };
            let ramp = |i: usize| (i + 1) as f32 / (ov + 1) as f32;
            for dy in 0..patch {
                let gy = if tile { (oy + dy) % out_h } else { oy + dy };
                for dx in 0..patch {
                    let gx = if tile { (ox + dx) % out_w } else { ox + dx };
                    let s = ((cy + dy) * sw + cx + dx) * 3;
                    let o = (gy * out_w + gx) * 3;
                    let was = placed[gy * out_w + gx];
                    // cut 模式:未落 ⇒ 取新;已落 ⇒ 默认取新,但每条 applicable 带
                    // 都能把它否回旧内容(AND——角部两带同时命中,保两侧接缝连续)。
                    // 环面右/下带是镜像:新内容贴着回绕缝(与缝另一侧的收尾 patch 同源连续)。
                    let take = if !cut || !was {
                        true
                    } else {
                        let mut t = true;
                        if let Some(v) = &vcut
                            && dx < ov
                        {
                            t &= dx >= v[dy];
                        }
                        if let Some(h) = &hcut
                            && dy < ov
                        {
                            t &= dy >= h[dx];
                        }
                        if let Some(r) = &rcut
                            && dx >= step
                        {
                            t &= dx - step < r[dy];
                        }
                        if let Some(b) = &bcut
                            && dy >= step
                        {
                            t &= dy - step < b[dx];
                        }
                        t
                    };
                    let mut a = 1.0f32;
                    if tile && dx >= step && was {
                        a = a.min(1.0 - ramp(dx - step));
                    }
                    if tile && dy >= step && was {
                        a = a.min(1.0 - ramp(dy - step));
                    }
                    if dy < ov {
                        a = a.min(ramp(dy));
                    }
                    if dx < ov {
                        a = a.min(ramp(dx));
                    }
                    if !was {
                        a = 1.0;
                    }
                    for c in 0..3 {
                        let mut v = src[s + c];
                        if let (Some((pm, ps)), Some(gm)) = (pstat, gmean) {
                            // 放大倍数 clamp 到 [0.5, 2]：否则低方差 patch 会把噪声放大成"结构"（实测 coherence 0.865→0.326）
                            let k = (src_std[c] / ps[c]).clamp(0.5, 2.0);
                            v = (v - pm[c]) * k + gm[c];
                        }
                        if cut {
                            if take {
                                out[o + c] = v;
                            }
                        } else {
                            out[o + c] = out[o + c] * (1.0 - a) + v * a;
                        }
                    }
                    placed[gy * out_w + gx] = true;
                }
            }
        }
    }
    if relight_plane {
        // **旧行为**：回贴源图拟合的线性平面。结构保得住，但对比度只回一小部分（§6.80/§6.81）。
        for y in 0..out_h {
            let v = y as f32 / out_h.max(1) as f32;
            for x in 0..out_w {
                let u = x as f32 / out_w.max(1) as f32;
                let o = (y * out_w + x) * 3;
                for c in 0..3 {
                    let g = plane[c][0] + plane[c][1] * u + plane[c][2] * v;
                    out[o + c] = (out[o + c] + g).clamp(0.0, 1.0);
                }
            }
        }
    } else if relight {
        // **亮度分位匹配（加性）**：把输出亮度分位对到源亮度分布，位移量加到 RGB 上。
        // 逐通道独立映射也能补回对比度，但会改色相与梯度（实测 coh 0.865→0.470、hf1 +94%）⇒ 用加性保结构。
        let mut src_luma: Vec<f32> = src_orig.chunks(3).map(luma_of).collect();
        src_luma.sort_by(f32::total_cmp);
        let mut out_sorted: Vec<f32> = out.chunks(3).map(luma_of).collect();
        out_sorted.sort_by(f32::total_cmp);
        let (ns, no) = (src_luma.len(), out_sorted.len());
        if ns > 1 && no > 1 {
            for i in 0..out.len() / 3 {
                let o = i * 3;
                let y0 = luma_of(&out[o..o + 3]);
                let rank = out_sorted.partition_point(|v| *v < y0).min(no - 1);
                let q = rank as f32 / (no - 1) as f32;
                let t = (q * (ns - 1) as f32).round() as usize;
                let delta = src_luma[t.min(ns - 1)] - y0;
                for c in 0..3 {
                    out[o + c] = (out[o + c] + delta).clamp(0.0, 1.0);
                }
            }
        }
    }
    out
}

/// 逐通道盒式模糊（去光照用的低频）。
pub(crate) fn box_blur_rgb(src: &[f32], w: usize, h: usize, r: usize) -> Vec<f32> {
    if r == 0 {
        return src.to_vec();
    }
    let mut tmp = vec![0.0f32; src.len()];
    let n = (2 * r + 1) as f32;
    for y in 0..h {
        for x in 0..w {
            for c in 0..3 {
                let mut sum = 0.0f32;
                for k in 0..=2 * r {
                    let xx = (x + k).saturating_sub(r).min(w - 1);
                    sum += src[(y * w + xx) * 3 + c];
                }
                tmp[(y * w + x) * 3 + c] = sum / n;
            }
        }
    }
    let mut out = vec![0.0f32; src.len()];
    for y in 0..h {
        for x in 0..w {
            for c in 0..3 {
                let mut sum = 0.0f32;
                for k in 0..=2 * r {
                    let yy = (y + k).saturating_sub(r).min(h - 1);
                    sum += tmp[(yy * w + x) * 3 + c];
                }
                out[(y * w + x) * 3 + c] = sum / n;
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stripes(n: usize, period: usize) -> Vec<f32> {
        (0..n * n)
            .flat_map(|i| {
                let v = 0.1 + 0.8 * (((i % n) / period).is_multiple_of(2) as u8 as f32);
                [v; 3]
            })
            .collect()
    }

    /// 源图长度与 sw×sh×3 不符时必须在分配前拒绝 —— 否则 `chunks(3)` 末尾短块会越界。
    #[test]
    #[should_panic(expected = "源图长度与 sw×sh×3 不符")]
    fn short_source_is_rejected_before_allocating() {
        let src = vec![0.0f32; 32 * 32 * 3 - 1];
        quilt(&src, 32, 32, 64, 64, 8, 4, 1, false, 0, false, false);
    }

    /// 输出尺寸连乘溢出必须在分配前拒绝，不能让 `out_w * out_h * 3` 绕成小值。
    #[test]
    #[should_panic(expected = "输出尺寸溢出")]
    fn output_size_overflow_is_rejected() {
        let src = vec![0.0f32; 32 * 32 * 3];
        quilt(&src, 32, 32, usize::MAX, 8, 8, 4, 1, false, 0, false, false);
    }

    /// 旧签名必须与全默认 `quilt_ex` 逐位一致——拆分不许偷改行为。
    #[test]
    fn legacy_quilt_matches_default_quilt_ex() {
        let src = stripes(32, 8);
        let a = quilt(&src, 32, 32, 64, 64, 8, 4, 7, true, 4, true, false);
        let b = quilt_ex(
            &src,
            32,
            32,
            &QuiltSpec {
                out_w: 64,
                out_h: 64,
                patch: 8,
                overlap: 4,
                seed: 7,
                tone_match: true,
                detrend: 4,
                relight: true,
                relight_plane: false,
                cut: false,
                tile: false,
            },
        );
        assert_eq!(a, b);
    }

    /// cut 模式确定性 + 尺寸/值域 + 与羽化是两条不同路径。
    #[test]
    fn cut_mode_is_deterministic_and_differs_from_feathering() {
        let src = stripes(48, 6);
        let spec = |cut: bool| QuiltSpec {
            out_w: 64,
            out_h: 64,
            patch: 12,
            overlap: 4,
            seed: 9,
            tone_match: false,
            detrend: 0,
            relight: false,
            relight_plane: false,
            cut,
            tile: false,
        };
        let a = quilt_ex(&src, 48, 48, &spec(true));
        let b = quilt_ex(&src, 48, 48, &spec(true));
        assert_eq!(a, b, "同 seed 逐位一致");
        assert_eq!(a.len(), 64 * 64 * 3);
        assert!(a.iter().all(|v| (0.0..=1.0).contains(v)));
        let f = quilt_ex(&src, 48, 48, &spec(false));
        assert_ne!(a, f, "切割缝合应与线性羽化给出不同拼图");
    }

    /// tile 模式:输出严格可平铺——环绕边界比必须压到 ≈1,且不差于不可平铺版。
    /// 源用周期构造的 wood 贴图(`tiles::tile` 周期 = 边长),接缝质量才有可比性。
    #[test]
    fn tile_mode_wraps_seamlessly() {
        let n = 48usize;
        let raw = crate::tex::tile("wood", n, 3);
        let src: Vec<f32> = raw
            .iter()
            .flat_map(|p| p.iter().map(|c| *c as f32 / 255.0))
            .collect();
        let spec = |tile: bool, cut: bool| QuiltSpec {
            out_w: 96,
            out_h: 96,
            patch: 16,
            overlap: 4,
            seed: 11,
            tone_match: false,
            detrend: 0,
            relight: false,
            relight_plane: false,
            cut,
            tile,
        };
        let feather = quilt_ex(&src, n, n, &spec(false, false));
        let tiled = quilt_ex(&src, n, n, &spec(true, false));
        let (bf, bt) = (
            crate::look::analyze(&feather, 96, 96).border,
            crate::look::analyze(&tiled, 96, 96).border,
        );
        assert!(
            bt < 1.3,
            "tile 模式环绕比应 ≈1(可平铺),实际 {bt:.3}(羽化 {bf:.3})"
        );
        assert!(
            bt <= bf,
            "tile 模式的环绕缝不应差于不可平铺版:{bt:.3} vs {bf:.3}"
        );
    }

    /// tile × cut 组合:确定性 + 环绕比同样 ≈1。
    #[test]
    fn tile_with_cut_is_deterministic_and_seamless() {
        let n = 48usize;
        let raw = crate::tex::tile("marble", n, 5);
        let src: Vec<f32> = raw
            .iter()
            .flat_map(|p| p.iter().map(|c| *c as f32 / 255.0))
            .collect();
        let spec = QuiltSpec {
            out_w: 96,
            out_h: 96,
            patch: 16,
            overlap: 4,
            seed: 13,
            tone_match: false,
            detrend: 0,
            relight: false,
            relight_plane: false,
            cut: true,
            tile: true,
        };
        let a = quilt_ex(&src, n, n, &spec);
        let b = quilt_ex(&src, n, n, &spec);
        assert_eq!(a, b);
        let bt = crate::look::analyze(&a, 96, 96).border;
        assert!(bt < 1.3, "tile+cut 环绕比应 ≈1,实际 {bt:.3}");
    }

    /// tile 模式把 overlap 钳到 [1, patch/2]:0 与超配额都不 panic、不越界。
    #[test]
    fn tile_mode_clamps_overlap_into_legal_range() {
        let src = stripes(32, 8);
        for ov in [0usize, 7, 16] {
            let out = quilt_ex(
                &src,
                32,
                32,
                &QuiltSpec {
                    out_w: 64,
                    out_h: 64,
                    patch: 12,
                    overlap: ov,
                    seed: 3,
                    tone_match: false,
                    detrend: 0,
                    relight: false,
                    relight_plane: false,
                    cut: true,
                    tile: true,
                },
            );
            assert_eq!(out.len(), 64 * 64 * 3);
            assert!(out.iter().all(|v| (0.0..=1.0).contains(v)));
        }
    }
}
