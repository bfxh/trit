//! 调色板与端点选择——TT3 的**画质**全部集中在这里。
//!
//! 一个块的可选色只有 3 个：`{e0, mid, e1}`，其中 `mid` 由两端点取平均决定。
//! 所以「块内画质」只剩两个可调项：
//!
//! 1. **端点选择**（本模块）：min/max 是起点，最小二乘精修是增益来源；
//! 2. **索引选择**：`nearest_index`，与进制无关（D 类）。
//!
//! 端点精修是纯 D 类（跟基数无关的编码质量）改进：字节数一个不变。本仓实测
//! （真实照片，块结构/调色板/码率全同，唯一变量是端点选择）**+1.3 ~ +2.0 dB**。
//!
//! ## 为什么 `mid` 不能自由优化（格式硬约束）
//!
//! 解码器手里只有 17 字节：4 字节端点对 + 13 字节 trit 索引。中间色**必须**是
//! `(e0, e1)` 的纯函数，否则解码器算不出来。于是「最优 3 色」里那宝贵的一席
//! 永远被钉在两端点的连线上——实测这条约束值 **+0.9 ~ +3.2 dB**
//! （见 `docs/` 的 3 色天花板测量），而它是格式决定的，不是编码器偷懒。
//!
//! 能动的只有：**选哪一对端点**，以及**怎么把端点落回 RGB565 格点**。

/// 3 级调色板：`[e0, mid, e1]`，8 位 RGB。
pub(crate) type Palette = [(u8, u8, u8); 3];

/// 端点最小二乘精修的轮数。3 轮在本仓实测中已收敛（第 4 轮 PSNR 变化 <0.01 dB），
/// 再多次只加编码耗时；编码是离线步骤，不进 GPU 采样热路径。
pub(crate) const REFINE_PASSES: u32 = 3;

/// 格点爬山的上限轮数。实测 2 轮内即无改进；封顶是为了给编码耗时兜底。
const HILL_CLIMB_PASSES: u32 = 2;

/// 由两个 RGB565 端点展开 3 级调色板：`{e0, mid, e1}`，`mid` 为逐通道平均。
pub(crate) fn palette_from_endpoints(e0: u16, e1: u16) -> Palette {
    let (r0, g0, b0) = crate::unpack_rgb565(e0);
    let (r1, g1, b1) = crate::unpack_rgb565(e1);
    let mid = |a: u8, b: u8| ((u16::from(a) + u16::from(b)) / 2) as u8;
    [
        (r0, g0, b0),
        (mid(r0, r1), mid(g0, g1), mid(b0, b1)),
        (r1, g1, b1),
    ]
}

/// 最近调色板色；并列取**较小下标**（确定性）。
pub(crate) fn nearest_index(palette: &Palette, r: u8, g: u8, b: u8) -> usize {
    let mut best = 0usize;
    let mut best_dist = u32::MAX;
    for (index, (pr, pg, pb)) in palette.iter().enumerate() {
        let dr = i32::from(r) - i32::from(*pr);
        let dg = i32::from(g) - i32::from(*pg);
        let db = i32::from(b) - i32::from(*pb);
        let dist = (dr * dr + dg * dg + db * db) as u32;
        if dist < best_dist {
            best = index;
            best_dist = dist;
        }
    }
    best
}

/// 找块的 min/max 端点（逐通道 min/max），量化成 RGB565。
pub(crate) fn block_endpoints(pixels: &[[u8; 4]]) -> (u16, u16) {
    let mut min = (255u8, 255u8, 255u8);
    let mut max = (0u8, 0u8, 0u8);
    for pixel in pixels {
        min = (
            min.0.min(pixel[0]),
            min.1.min(pixel[1]),
            min.2.min(pixel[2]),
        );
        max = (
            max.0.max(pixel[0]),
            max.1.max(pixel[1]),
            max.2.max(pixel[2]),
        );
    }
    (
        crate::pack_rgb565(min.0, min.1, min.2),
        crate::pack_rgb565(max.0, max.1, max.2),
    )
}

/// 一对端点在 `pixels` 上的**精确块内误差**（逐纹素到最近调色板色的平方和）。
///
/// 精修里的每一次「试一下」都必须用它打分，而不是用组质心残差近似——后者正是
/// 旧实现会走上下坡的原因。
fn pair_error(pixels: &[[u8; 4]], e0: u16, e1: u16) -> u64 {
    let palette = palette_from_endpoints(e0, e1);
    let mut total = 0u64;
    for pixel in pixels {
        let (r, g, b) = palette[nearest_index(&palette, pixel[0], pixel[1], pixel[2])];
        let dr = i64::from(pixel[0]) - i64::from(r);
        let dg = i64::from(pixel[1]) - i64::from(g);
        let db = i64::from(pixel[2]) - i64::from(b);
        total += (dr * dr + dg * dg + db * db) as u64;
    }
    total
}

/// RGB565 各通道的位宽（R/G/B）。
const CHANNEL_BITS: [u8; 3] = [5, 6, 5];

/// 取出 `word` 第 `channel` 个通道的原始 5/6/5 域值。
fn channel_field(word: u16, channel: usize) -> u8 {
    match channel {
        0 => (word >> 11) as u8 & 0x1f,
        1 => (word >> 5) as u8 & 0x3f,
        _ => word as u8 & 0x1f,
    }
}

/// 把 `word` 第 `channel` 个通道替换成 `value`（越界即饱和）。
fn with_channel(word: u16, channel: usize, value: u8) -> u16 {
    let max = (1u16 << CHANNEL_BITS[channel]) - 1;
    let value = u16::from(value).min(max);
    match channel {
        0 => (word & 0x07ff) | (value << 11),
        1 => (word & 0xf81f) | (value << 5),
        _ => (word & 0xffe0) | value,
    }
}

/// 在 RGB565 格点上做**贪心爬山**：逐个端点、逐个通道试 ±1 级，只收严格变好的步。
///
/// 这是「量化感知」的全部内容：Lloyd 用整数除法把质心砸回 565 时会引入亚格点
/// 偏差（截断方向恒定偏暗），爬山把它擦掉。真实照片实测 **+0.1 ~ +0.9 dB**，
/// 字节数一个不变——纯编码器算力换画质。
///
/// 每一步都以[`pair_error`]的精确误差为准，所以整体严格单调不升；
/// 端点塌缩（`e0 == e1`，调色板退化成 1 色）直接被拒。
fn hill_climb(pixels: &[[u8; 4]], mut e0: u16, mut e1: u16) -> (u16, u16) {
    let mut best = pair_error(pixels, e0, e1);
    for _ in 0..HILL_CLIMB_PASSES {
        let mut improved = false;
        for which_low in [true, false] {
            for (channel, bits) in CHANNEL_BITS.iter().enumerate() {
                let current = if which_low { e0 } else { e1 };
                let field = channel_field(current, channel);
                for delta in [1i8, -1] {
                    let moved = i16::from(field) + i16::from(delta);
                    let max = (1i16 << bits) - 1;
                    if moved < 0 || moved > max {
                        continue;
                    }
                    let candidate = with_channel(current, channel, moved as u8);
                    let (c0, c1) = if which_low {
                        (candidate, e1)
                    } else {
                        (e0, candidate)
                    };
                    if c0 == c1 {
                        continue;
                    }
                    let error = pair_error(pixels, c0, c1);
                    if error < best {
                        best = error;
                        e0 = c0;
                        e1 = c1;
                        improved = true;
                    }
                }
            }
        }
        if !improved {
            break;
        }
    }
    (e0, e1)
}

/// 最小二乘端点精修：把 min/max 端点当初值，交替「按最近色分组 → 用各组质心
/// 重估端点」，做 [`REFINE_PASSES`] 轮，最后在 RGB565 格点上爬山消除量化偏差。
///
/// > 教训：min/max 端点只在端点色恰好落在块的极值角上时才最优；真实纹理的色簇
/// > 通常沿一条斜线分布，逐通道 min/max 会同时抬高两端与中点的误差。本仓实测
/// > （真实照片，块结构/调色板/码率全不变，唯一变量是端点选择）**+1.3 ~ +2.0 dB**，
/// > 字节数一个不变。
///
/// 中点槽 `mid` 由端点决定，所以只重估 `e0`/`e1`；`mid` 组不参与（它被约束在两端
/// 中间，单独拉动端点会破坏几何一致性）。端点写回 RGB565，与定码率对齐。
///
/// 单调性：本函数**不得**让块内量化误差上升——因此每一轮都用[`pair_error`]
/// 打分并**保留历史最优**（Lloyd 迭代本身不是单调的：质心重估 + 565 再量化
/// 可能走过头，实测在平坦块上会让误差反升 0.8 dB）。空组/单点组不重估——
/// 没有统计意义，也避免端点塌缩。
pub(crate) fn refine_endpoints(pixels: &[[u8; 4]], e0: u16, e1: u16) -> (u16, u16) {
    let mut best_pair = (e0, e1);
    let mut best_error = pair_error(pixels, e0, e1);
    let mut low = e0;
    let mut high = e1;
    for _ in 0..REFINE_PASSES {
        let palette = palette_from_endpoints(low, high);
        let mut sum = [(0u32, 0u32, 0u32); 3];
        let mut count = [0u32; 3];
        for pixel in pixels {
            let slot = nearest_index(&palette, pixel[0], pixel[1], pixel[2]);
            sum[slot].0 += u32::from(pixel[0]);
            sum[slot].1 += u32::from(pixel[1]);
            sum[slot].2 += u32::from(pixel[2]);
            count[slot] += 1;
        }
        let mut changed = false;
        if count[0] >= 2 {
            let next = (
                (sum[0].0 / count[0]) as u8,
                (sum[0].1 / count[0]) as u8,
                (sum[0].2 / count[0]) as u8,
            );
            let packed = crate::pack_rgb565(next.0, next.1, next.2);
            changed |= packed != low;
            low = packed;
        }
        if count[2] >= 2 {
            let next = (
                (sum[2].0 / count[2]) as u8,
                (sum[2].1 / count[2]) as u8,
                (sum[2].2 / count[2]) as u8,
            );
            let packed = crate::pack_rgb565(next.0, next.1, next.2);
            changed |= packed != high;
            high = packed;
        }
        if low != high {
            let error = pair_error(pixels, low, high);
            if error < best_error {
                best_error = error;
                best_pair = (low, high);
            }
        }
        if !changed {
            break;
        }
    }
    let (e0, e1) = hill_climb(pixels, best_pair.0, best_pair.1);
    debug_assert!(pair_error(pixels, e0, e1) <= best_error);
    (e0, e1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nearest_color_breaks_ties_toward_the_lower_index() {
        // (5,5,5) 到 e0 与 mid 距离相等：必须选下标 0（严格小于才换）。
        let palette = [(0u8, 0u8, 0u8), (10, 10, 10), (20, 20, 20)];
        assert_eq!(nearest_index(&palette, 5, 5, 5), 0);
        assert_eq!(nearest_index(&palette, 4, 4, 4), 0);
        assert_eq!(nearest_index(&palette, 6, 6, 6), 1);
        assert_eq!(nearest_index(&palette, 19, 19, 19), 2);
    }

    #[test]
    fn palette_midpoint_is_the_channel_average() {
        // 565 可表示的两色 → 端点与中点都必须按 (a+b)/2 精确落位。
        let e0 = crate::pack_rgb565(0, 0, 0);
        let e1 = crate::pack_rgb565(200, 100, 50);
        let palette = palette_from_endpoints(e0, e1);
        let (r1, g1, b1) = crate::unpack_rgb565(e1);
        assert_eq!(palette[0], (0, 0, 0));
        assert_eq!(palette[2], (r1, g1, b1));
        assert_eq!(palette[1], ((r1 / 2), (g1 / 2), (b1 / 2)));
    }

    #[test]
    fn refinement_is_monotone_and_keeps_endpoints_distinct() {
        // 精修是单调不升的：任何块上，精修后的调色板量化误差都不得大于 min/max。
        // 同时两端点不得塌缩成同一色。
        let mut state = 0x1234_5678_9ABC_DEF0u64;
        let mut next = || {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            state
        };
        let error = |palette: &Palette, texels: &[[u8; 4]]| -> u64 {
            texels
                .iter()
                .map(|pixel| {
                    let (r, g, b) = palette[nearest_index(palette, pixel[0], pixel[1], pixel[2])];
                    let dr = i64::from(pixel[0]) - i64::from(r);
                    let dg = i64::from(pixel[1]) - i64::from(g);
                    let db = i64::from(pixel[2]) - i64::from(b);
                    (dr * dr + dg * dg + db * db) as u64
                })
                .sum()
        };
        for case in 0..64 {
            let mut texels = [[0u8; 4]; crate::BLOCK_PIXELS];
            for texel in texels.iter_mut() {
                // 沿一条斜轴分布的色簇 + 噪声：正是 min/max 次优、精修有效的场景。
                let t = (next() % 256) as u8;
                *texel = [t, t.saturating_sub(8), 255 - t, 255];
            }
            let (e0, e1) = block_endpoints(&texels);
            let (r0, r1) = refine_endpoints(&texels, e0, e1);
            assert!(
                error(&palette_from_endpoints(r0, r1), &texels)
                    <= error(&palette_from_endpoints(e0, e1), &texels),
                "case {case}: 精修后误差反升"
            );
            // 非纯色块的端点不应被拖成同一色（塌缩 = 调色板退化成 1 色）。
            if texels.iter().any(|p| p[..3] != texels[0][..3]) {
                assert_ne!(r0, r1, "case {case}: 端点塌缩");
            }
        }
    }

    #[test]
    fn hill_climb_never_makes_a_block_worse_and_keeps_endpoints_apart() {
        // 爬山只收严格改进；且「塌缩成同一色」的候选必须在入口就被拒。
        let mut state = 0xDEAD_BEEF_CAFE_1234u64;
        let mut next = || {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            state
        };
        for case in 0..32 {
            let mut texels = [[0u8; 4]; crate::BLOCK_PIXELS];
            for texel in texels.iter_mut() {
                let t = (next() % 256) as u8;
                *texel = [t, 255 - t, t / 2, 255];
            }
            let (e0, e1) = block_endpoints(&texels);
            let (c0, c1) = hill_climb(&texels, e0, e1);
            assert!(
                pair_error(&texels, c0, c1) <= pair_error(&texels, e0, e1),
                "case {case}: 爬山让误差反升"
            );
            assert_ne!(c0, c1, "case {case}: 爬山把端点塌缩了");
        }
    }

    #[test]
    fn channel_field_roundtrips_every_565_level() {
        // with_channel 的位掩码写错会静默串通道；全量 5/6/5 扫一遍。
        for r in 0..32u16 {
            for g in 0..64u16 {
                for b in 0..32u16 {
                    let word = (r << 11) | (g << 5) | b;
                    assert_eq!(u16::from(channel_field(word, 0)), r);
                    assert_eq!(u16::from(channel_field(word, 1)), g);
                    assert_eq!(u16::from(channel_field(word, 2)), b);
                    for (channel, value) in [(0usize, r), (1, g), (2, b)] {
                        assert_eq!(with_channel(word, channel, value as u8), word);
                    }
                }
            }
        }
    }
}
