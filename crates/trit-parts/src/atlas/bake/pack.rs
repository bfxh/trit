/// 24 次减半重排，`rects` 是 `[x, y, w, h]`；放不下返回 `None`。
///
/// 每处像素边长都夹到 `atlas_px + 1`：单个 chart 的边长不可能超过整张图集，
/// 超过就是「放不下」。不夹的话 `f32 as usize` 会饱和成 `usize::MAX`，
/// `cx + w + pad` 绕回成一个小值，反而让「放不下」判断通过，最后写 `img[巨大]`。
pub(super) fn pack(
    bounds: &[([f32; 2], [f32; 2])],
    ng: usize,
    mut scale: f32,
    atlas_px: usize,
) -> Option<(Vec<[usize; 4]>, f32)> {
    let pad = 2usize;
    let cap = atlas_px + 1;
    let mut rects = vec![[0, 0, 0, 0]; ng];
    for _ in 0..24 {
        let mut order: Vec<usize> = (0..bounds.len()).collect();
        order.sort_by(|&a, &b| {
            let ha = (((bounds[a].1[1] - bounds[a].0[1]) * scale).ceil() as usize).min(cap);
            let hb = (((bounds[b].1[1] - bounds[b].0[1]) * scale).ceil() as usize).min(cap);
            hb.cmp(&ha)
        });
        rects = vec![[0, 0, 0, 0]; ng];
        let (mut cx, mut cy, mut shelf_h, mut ok) = (0usize, 0usize, 0usize, true);
        for &gi in &order {
            let (lo, hi) = bounds[gi];
            let w = (((hi[0] - lo[0]) * scale).ceil() as usize).max(1).min(cap);
            let h = (((hi[1] - lo[1]) * scale).ceil() as usize).max(1).min(cap);
            if cx + w + pad > atlas_px {
                cx = 0;
                cy += shelf_h + pad;
                shelf_h = 0;
            }
            if cy + h + pad > atlas_px {
                ok = false;
                break;
            }
            rects[gi] = [cx, cy, w, h];
            cx += w + pad;
            shelf_h = shelf_h.max(h);
        }
        if ok {
            return Some((rects, scale));
        }
        scale *= 0.5;
    }
    None
}
