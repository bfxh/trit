// 计算入口：逐采样点三线性/各向异性。解码逻辑在 `sampler_common.wgsl`。
//
// 采样点布局（每点 32 字节）：(u, v, lod, taps) + (step_u, step_v, _, _)。
// taps = 1 时 step 无意义（退化为单次三线性）；taps > 1 即各向异性多点平均。

struct Sample {
    uv_lod_taps: vec4<f32>,
    step: vec4<f32>,
};

@group(0) @binding(3) var<storage, read> points: array<Sample>;
@group(0) @binding(4) var<storage, read_write> out: array<u32>;

@compute @workgroup_size(64)
fn main(@builtin(global_invocation_id) gid: vec3<u32>) {
    let i = gid.x;
    if (i >= params.count) {
        return;
    }
    let sample = points[i];
    let last = f32(params.level_count - 1u);
    var lod = sample.uv_lod_taps.z;
    // NaN 归零（与 CPU 同口径）
    if (lod != lod) {
        lod = 0.0;
    }
    lod = clamp(lod, 0.0, last);
    let base_level = floor(lod);
    let l0 = u32(base_level);
    let l1 = min(l0 + 1u, params.level_count - 1u);
    let t = lod - base_level;
    // 每级先回 8 位再融合——与 CPU 参考的量化时机一致（先量化、后混、再量化）
    // 各向异性：沿长轴在半个像素跨度内均匀取 taps 个三线性点，**每点先回 8 位再平均**
    //（与 CPU `sample_aniso` 的量化时机一致）
    let taps = max(1u, u32(sample.uv_lod_taps.w));
    var acc = vec3<f32>(0.0);
    for (var k = 0u; k < taps; k = k + 1u) {
        let off = (f32(k) + 0.5) / f32(taps) - 0.5;
        let su = sample.uv_lod_taps.x + sample.step.x * off;
        let sv = sample.uv_lod_taps.y + sample.step.y * off;
        let a = quantize(bilinear(l0, su, sv));
        var tap_color = a;
        if (l1 != l0) {
            let b = quantize(bilinear(l1, su, sv));
            tap_color = fma(a, vec3<f32>(1.0 - t), b * t);
        }
        acc = acc + clamp(
            vec3<f32>(round(tap_color.x), round(tap_color.y), round(tap_color.z)),
            vec3<f32>(0.0),
            vec3<f32>(255.0),
        );
    }
    let c = acc / f32(taps);
    let r = u32(clamp(round(c.x), 0.0, 255.0));
    let g = u32(clamp(round(c.y), 0.0, 255.0));
    let b = u32(clamp(round(c.z), 0.0, 255.0));
    out[i] = r | (g << 8u) | (b << 16u) | 0xff000000u;
}
