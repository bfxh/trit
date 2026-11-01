// 光栅化入口：顶点原样透传（裁剪空间），片元自己从 `dpdx`/`dpdy` 选 LOD 再三线性采样。
//
// 与 CPU 参考的差别只有一处：GPU 的导数是 2×2 quad 的有限差分，CPU 是解析式。
// 所以 `force_lod` 模式下两边应当逐字节一致；`Auto` 模式只做有界对拍。
//
// 解码与三线性在 `sampler_common.wgsl`，两个入口点共用。

struct VertexIn {
    @location(0) clip: vec4<f32>,
    @location(1) uv: vec2<f32>,
};

struct VertexOut {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

struct RenderParams {
    // 0 = 由 dpdx/dpdy 自动选 LOD，1 = 用 force_lod
    use_force_lod: u32,
    pad0: u32,
    pad1: u32,
    pad2: u32,
    force_lod: f32,
    pad3: f32,
    pad4: f32,
    pad5: f32,
};

@group(0) @binding(3) var<uniform> render: RenderParams;

@vertex
fn vs_main(in: VertexIn) -> VertexOut {
    var out: VertexOut;
    // 顶点直接给裁剪空间：光栅化器自己做透视除法与视口映射
    out.position = in.clip;
    out.uv = in.uv;
    return out;
}

@fragment
fn fs_main(in: VertexOut) -> @location(0) vec4<f32> {
    var lod = 0.0;
    if (render.use_force_lod != 0u) {
        lod = render.force_lod;
    } else {
        // ρ = max(|∂/∂x|, |∂/∂y|)，单位是**基准级纹素**（与 CPU `lod_from_derivatives` 同式）
        let base = levels[0];
        let size = vec2<f32>(f32(base.width), f32(base.height));
        let dx = length(dpdx(in.uv) * size);
        let dy = length(dpdy(in.uv) * size);
        let rho = max(dx, dy);
        // 放大不改 LOD（ρ ≤ 1 时取 0）
        if (rho > 1.0) {
            lod = log2(rho);
        }
    }
    let last = f32(params.level_count - 1u);
    if (lod != lod) {
        lod = 0.0;
    }
    lod = clamp(lod, 0.0, last);
    let base_level = floor(lod);
    let l0 = u32(base_level);
    let l1 = min(l0 + 1u, params.level_count - 1u);
    let t = lod - base_level;
    // 先逐级回 8 位再融合，与 CPU 的量化时机一致
    let a = quantize(bilinear(l0, in.uv.x, in.uv.y));
    var c = a;
    if (l1 != l0 && t > 0.0) {
        let b = quantize(bilinear(l1, in.uv.x, in.uv.y));
        c = fma(a, vec3<f32>(1.0 - t), b * t);
    }
    let q = clamp(
        vec3<f32>(round(c.x), round(c.y), round(c.z)),
        vec3<f32>(0.0),
        vec3<f32>(255.0),
    );
    return vec4<f32>(q / 255.0, 1.0);
}
