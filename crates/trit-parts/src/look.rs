//! 「观感」测量：把一张图的画质/光影/风格特征变成可复跑的标量。
//!
//! 口径：**这些是启发式描述量，不是分类器真值**。用途是给 `docs/VISUAL-PLAN.md` 的
//! S1/S2/S6 提供基线读数（同一张图两次跑必须逐位一致），以及做 A/B（改前后参数对比）。
//! 真正的三类判别头（照片/CG/UI）要单独训练，不能用下面的 `guess` 当结论。
//!
//! 手机截图/播放器页有黑边和白色页面区，会严重污染直方图 ⇒ 先检测并裁掉「均匀边条」，
//! 所有统计都在内容区上算，`bars` 单独记录被裁掉的比例。

/// 一张图的描述量。
#[derive(Debug, Clone, PartialEq)]
pub struct Look {
    pub w: usize,
    pub h: usize,
    /// 内容区亮度均值。
    pub luma: f32,
    /// 内容区 p95 − p5（整体对比）。
    pub contrast: f32,
    /// 平均饱和度 `(max−min)/max`。
    pub sat: f32,
    /// 12 桶色相直方图的归一化熵（1 = 色相很散，0 = 单色）。
    pub hue_entropy: f32,
    /// 平均梯度幅值（Sobel，亮度）。
    pub edge: f32,
    /// 中弱梯度像素占比（0.05–0.3）：细纹理/颗粒。
    pub fine: f32,
    /// 强梯度占比（>0.3）：硬边/线稿/高反差细节。
    pub strong: f32,
    /// 8×8 块内标准差 <0.01 的占比：平坦区（天空/平涂/UI 面板）。
    pub flat: f32,
    /// 有效亮度级数（64 桶中质量 ≥0.2% 的桶数）：平涂/赛璐璐低，写实高。
    pub levels: f32,
    /// 块标准差均值 / 全局标准差：低 = 局部反差被抹平（雾/平涂）。
    pub haze: f32,
    /// 亮度 >0.75 且饱和度 >0.25 的占比：自发光/霓虹。
    pub emissive: f32,
    /// 被判定为「均匀边条」而裁掉的行列占比（平台 UI / letterbox）。
    pub bars: f32,
    /// (R−B)/亮度：暖/冷。
    pub warmth: f32,
    /// 主光方向代理：亮度梯度朝向的圆周均值（度，0..180）。真实光线方向要模型估，这是代理量。
    pub key_dir: f32,
    /// 亮部 10% 与暗部 10% 的 (R−B) 差：>0 = 暖主光/冷补光（经典电影光比）。
    pub key_fill_dwarm: f32,
    /// 高光占比：亮度 >0.9 且低饱和（镜面/灯）。
    pub highlights: f32,
}

fn luma_of(r: f32, g: f32, b: f32) -> f32 {
    0.299 * r + 0.587 * g + 0.114 * b
}

fn percentile(sorted: &[f32], q: f32) -> f32 {
    if sorted.is_empty() {
        return 0.0;
    }
    let i = ((sorted.len() - 1) as f32 * q).round() as usize;
    sorted[i]
}

/// 找内容区：去掉上下左右「近均匀且很暗/很亮」的条带。返回 `(x0,y0,x1,y1)`（右/下开区间）。
fn content_box(lum: &[f32], w: usize, h: usize) -> (usize, usize, usize, usize, f32) {
    let row_uniform = |y: usize| -> bool {
        let row = &lum[y * w..(y + 1) * w];
        let m = row.iter().sum::<f32>() / w as f32;
        let sd = (row.iter().map(|v| (v - m) * (v - m)).sum::<f32>() / w as f32).sqrt();
        sd < 0.02 && (m < 0.12 || m > 0.88)
    };
    let col_uniform = |x: usize| -> bool {
        let mut s = 0.0f32;
        for y in 0..h {
            s += lum[y * w + x];
        }
        let m = s / h as f32;
        let mut v = 0.0f32;
        for y in 0..h {
            let d = lum[y * w + x] - m;
            v += d * d;
        }
        let sd = (v / h as f32).sqrt();
        sd < 0.02 && (m < 0.12 || m > 0.88)
    };
    let mut top = 0;
    while top < h / 2 && row_uniform(top) {
        top += 1;
    }
    let mut bot = h;
    while bot > top + h / 4 && row_uniform(bot - 1) {
        bot -= 1;
    }
    let mut left = 0;
    while left < w / 2 && col_uniform(left) {
        left += 1;
    }
    let mut right = w;
    while right > left + w / 4 && col_uniform(right - 1) {
        right -= 1;
    }
    let keep = (right - left) * (bot - top);
    let total = w * h;
    if keep * 4 < total {
        // 裁得太狠（例如整张常数图）⇒ 当作没有边条
        (0, 0, w, h, 0.0)
    } else {
        let bars = 1.0 - keep as f32 / total as f32;
        (left, top, right, bot, bars)
    }
}

/// 主入口：`rgb` 是 3 通道、0..1、行优先。
pub fn analyze(rgb: &[f32], w: usize, h: usize) -> Look {
    let n = w * h;
    assert!(rgb.len() >= n * 3, "rgb 长度不足");
    let lum: Vec<f32> = (0..n)
        .map(|p| luma_of(rgb[p * 3], rgb[p * 3 + 1], rgb[p * 3 + 2]))
        .collect();
    let (x0, y0, x1, y1, bars) = content_box(&lum, w, h);
    analyze_region(rgb, &lum, w, h, x0, y0, x1, y1, bars)
}

#[allow(clippy::too_many_arguments)]
fn analyze_region(
    rgb: &[f32],
    lum: &[f32],
    w: usize,
    h: usize,
    x0: usize,
    y0: usize,
    x1: usize,
    y1: usize,
    bars: f32,
) -> Look {
    let (cw, ch) = (x1 - x0, y1 - y0);
    let n = cw * ch;
    let at = |x: usize, y: usize| (y0 + y) * w + (x0 + x);

    let mean = (0..n).map(|p| lum[at(p % cw, p / cw)]).sum::<f32>() / n as f32;
    let mut sorted: Vec<f32> = (0..n).map(|p| lum[at(p % cw, p / cw)]).collect();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let (p5, p95) = (percentile(&sorted, 0.05), percentile(&sorted, 0.95));

    let mut sat_sum = 0.0f32;
    let mut hue_hist = [0.0f32; 12];
    let mut emissive = 0usize;
    let mut warm = 0.0f32;
    for y in 0..ch {
        for x in 0..cw {
            let p = at(x, y) * 3;
            let (r, g, b) = (rgb[p], rgb[p + 1], rgb[p + 2]);
            let mx = r.max(g).max(b);
            let mn = r.min(g).min(b);
            if mx > 1e-4 {
                sat_sum += (mx - mn) / mx;
            }
            let l = luma_of(r, g, b);
            if l > 0.75 && mx - mn > 0.25 {
                emissive += 1;
            }
            warm += r - b;
            if mx - mn > 0.08 {
                let deg = if mx == r {
                    60.0 * ((g - b) / (mx - mn)).rem_euclid(6.0)
                } else if mx == g {
                    60.0 * ((b - r) / (mx - mn) + 2.0)
                } else {
                    60.0 * ((r - g) / (mx - mn) + 4.0)
                };
                hue_hist[((deg / 30.0) as usize).min(11)] += 1.0;
            }
        }
    }
    let hue_n: f32 = hue_hist.iter().sum();
    let hue_entropy = if hue_n > 0.0 {
        let e: f32 = hue_hist
            .iter()
            .map(|&c| {
                let q = c / hue_n;
                if q > 0.0 { -q * q.ln() } else { 0.0 }
            })
            .sum();
        (e / (12.0f32).ln()).clamp(0.0, 1.0)
    } else {
        0.0
    };

    // Sobel（在内容区内，边界用夹取索引）
    let mut edge_sum = 0.0f32;
    let mut fine = 0usize;
    let mut strong = 0usize;
    let mut cnt = 0usize;
    let (mut ori_sin, mut ori_cos, mut ori_w) = (0.0f32, 0.0f32, 0.0f32);
    if cw > 2 && ch > 2 {
        for y in 1..ch - 1 {
            for x in 1..cw - 1 {
                let g = |dx: isize, dy: isize| {
                    lum[at((x as isize + dx) as usize, (y as isize + dy) as usize)]
                };
                let gx =
                    -g(-1, -1) - 2.0 * g(-1, 0) - g(-1, 1) + g(1, -1) + 2.0 * g(1, 0) + g(1, 1);
                let gy =
                    -g(-1, -1) - 2.0 * g(0, -1) - g(1, -1) + g(-1, 1) + 2.0 * g(0, 1) + g(1, 1);
                let m = (gx * gx + gy * gy).sqrt() / 4.0;
                if m > 0.08 {
                    // 双向平均：朝向按 180° 周期折叠
                    let th = gy.atan2(gx);
                    ori_sin += (2.0 * th).sin() * m;
                    ori_cos += (2.0 * th).cos() * m;
                    ori_w += m;
                }
                edge_sum += m;
                if (0.05..0.3).contains(&m) {
                    fine += 1;
                } else if m >= 0.3 {
                    strong += 1;
                }
                cnt += 1;
            }
        }
    }
    let (edge, fine, strong) = if cnt > 0 {
        (
            edge_sum / cnt as f32,
            fine as f32 / cnt as f32,
            strong as f32 / cnt as f32,
        )
    } else {
        (0.0, 0.0, 0.0)
    };

    // 8×8 块统计
    let bs = 8usize;
    let mut block_std = Vec::new();
    let mut flat_blocks = 0usize;
    let mut blocks = 0usize;
    if cw >= bs && ch >= bs {
        for by in (0..=ch - bs).step_by(bs) {
            for bx in (0..=cw - bs).step_by(bs) {
                let mut s = 0.0f32;
                let mut s2 = 0.0f32;
                for dy in 0..bs {
                    for dx in 0..bs {
                        let v = lum[at(bx + dx, by + dy)];
                        s += v;
                        s2 += v * v;
                    }
                }
                let k = (bs * bs) as f32;
                let m = s / k;
                let sd = (s2 / k - m * m).max(0.0).sqrt();
                block_std.push(sd);
                if sd < 0.01 {
                    flat_blocks += 1;
                }
                blocks += 1;
            }
        }
    }
    let flat = if blocks > 0 {
        flat_blocks as f32 / blocks as f32
    } else {
        0.0
    };
    let haze = if !block_std.is_empty() {
        let local = block_std.iter().sum::<f32>() / block_std.len() as f32;
        let var = sorted
            .iter()
            .map(|v| {
                let d = v - mean;
                d * d
            })
            .sum::<f32>()
            / n as f32;
        let global = var.sqrt();
        if global > 1e-6 {
            (local / global).clamp(0.0, 1.0)
        } else {
            0.0
        }
    } else {
        0.0
    };

    // 有效亮度级数（64 桶，质量 ≥0.2%）
    let mut hist = [0usize; 64];
    for y in 0..ch {
        for x in 0..cw {
            let v = lum[at(x, y)].clamp(0.0, 1.0);
            hist[((v * 63.0) as usize).min(63)] += 1;
        }
    }
    let min_mass = (n as f32 * 0.002).ceil() as usize;
    let levels = hist.iter().filter(|&&c| c >= min_mass.max(1)).count() as f32;

    let key_dir = if ori_w > 1e-6 {
        (0.5 * ori_sin.atan2(ori_cos))
            .to_degrees()
            .rem_euclid(180.0)
    } else {
        0.0
    };

    // 高光与 key/fill 色温：按亮度排序后取两端各 10%
    let hi_i = ((n as f32 * 0.9) as usize).min(n.saturating_sub(1));
    let lo_i = ((n as f32 * 0.1) as usize).min(n.saturating_sub(1));
    let (hi_l, lo_l) = (sorted[hi_i], sorted[lo_i]);
    let mut hi_warm = 0.0f32;
    let mut lo_warm = 0.0f32;
    let (mut hi_n, mut lo_n) = (0usize, 0usize);
    let mut highlights = 0usize;
    for y in 0..ch {
        for x in 0..cw {
            let idx = at(x, y);
            let p = idx * 3;
            let (r, g, b) = (rgb[p], rgb[p + 1], rgb[p + 2]);
            let l = lum[idx];
            let mx = r.max(g).max(b);
            let mn = r.min(g).min(b);
            if l >= hi_l {
                hi_warm += r - b;
                hi_n += 1;
            } else if l <= lo_l {
                lo_warm += r - b;
                lo_n += 1;
            }
            if l > 0.9 && mx - mn < 0.15 {
                highlights += 1;
            }
        }
    }
    let key_fill_dwarm = if hi_n > 0 && lo_n > 0 {
        hi_warm / hi_n as f32 - lo_warm / lo_n as f32
    } else {
        0.0
    };

    Look {
        w,
        h,
        luma: mean,
        contrast: p95 - p5,
        sat: sat_sum / n as f32,
        hue_entropy,
        edge,
        fine,
        strong,
        flat,
        levels,
        haze,
        emissive: emissive as f32 / n as f32,
        bars,
        warmth: warm / n as f32,
        key_dir,
        key_fill_dwarm,
        highlights: highlights as f32 / n as f32,
    }
}

/// **启发式**标签，只用来给读数排座（不是训练出来的分类器）。
pub fn guess(l: &Look) -> &'static str {
    if l.bars > 0.5 {
        "平台截图/边条"
    } else if l.levels < 24.0 && l.flat > 0.35 {
        "平涂/动画"
    } else if l.fine > 0.10 && l.levels >= 30.0 {
        "写实/CG 细节"
    } else {
        "中间态"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn flat_img(v: f32, w: usize, h: usize) -> Vec<f32> {
        vec![v; w * h * 3]
    }

    #[test]
    fn constant_image_is_flat() {
        let l = analyze(&flat_img(0.5, 32, 32), 32, 32);
        assert!((l.luma - 0.5).abs() < 1e-6);
        assert!(l.edge < 1e-6, "常数图梯度应为 0");
        assert!(l.flat > 0.99, "常数图应全平");
        assert!(l.levels <= 1.0, "常数图只有 1 级");
        assert!(l.hue_entropy < 1e-6, "无彩色");
        assert!(l.bars < 1e-6, "整图常数时不该当成边条裁掉");
    }

    #[test]
    fn checker_is_high_contrast_strong_edges() {
        let (w, h) = (32usize, 32usize);
        let mut v = vec![0.0f32; w * h * 3];
        for y in 0..h {
            for x in 0..w {
                let c = if (x / 4 + y / 4) % 2 == 0 { 0.0 } else { 1.0 };
                let p = (y * w + x) * 3;
                v[p] = c;
                v[p + 1] = c;
                v[p + 2] = c;
            }
        }
        let l = analyze(&v, w, h);
        assert!(l.contrast > 0.9);
        assert!(l.strong > 0.05, "硬边应被计入 strong");
        assert!(l.flat < 0.5, "棋盘不是全平");
        assert!(l.levels <= 2.0, "棋盘只有 2 级");
    }

    #[test]
    fn saturation_matches_color() {
        let v = [1.0f32, 0.0, 0.0].repeat(64 * 64);
        let l = analyze(&v, 64, 64);
        assert!(l.sat > 0.95, "纯红饱和度应接近 1，实际 {}", l.sat);
        assert!(l.warmth > 0.5, "红应偏暖");
    }

    #[test]
    fn key_direction_follows_gradient() {
        // 左亮右暗 ⇒ 梯度沿 x 轴（0°）
        let (w, h) = (64usize, 32usize);
        let mut v = vec![0.0f32; w * h * 3];
        for y in 0..h {
            for x in 0..w {
                let c = 1.0 - x as f32 / w as f32;
                let p = (y * w + x) * 3;
                v[p] = c;
                v[p + 1] = c;
                v[p + 2] = c;
            }
        }
        let l = analyze(&v, w, h);
        let d = l.key_dir.min(180.0 - l.key_dir);
        assert!(d < 10.0, "梯度应基本沿水平，实际 {}°", l.key_dir);
    }

    #[test]
    fn dark_letterbox_is_cropped() {
        // 中间彩色、上下各 8 行纯黑边：应被识别成 bars，且亮度不被拉低
        let (w, h) = (32usize, 32usize);
        let mut v = vec![0.0f32; w * h * 3];
        for y in 8..24 {
            for x in 0..w {
                let p = (y * w + x) * 3;
                v[p] = 0.6;
                v[p + 1] = 0.5;
                v[p + 2] = 0.4;
            }
        }
        let l = analyze(&v, w, h);
        assert!(l.bars > 0.3, "上下黑边应被记为边条，实际 {}", l.bars);
        assert!(
            (l.luma - 0.53).abs() < 0.02,
            "内容区亮度应约 0.53，实际 {}",
            l.luma
        );
    }
}
