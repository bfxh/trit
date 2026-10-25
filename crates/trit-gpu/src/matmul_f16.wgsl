enable f16;

// fp16 存储变体（单独成文件）：`enable f16;` 是**模块级**开关，放在共享的
// matmul_f32.wgsl 里会让不支持 SHADER_F16 的 adapter（如 CI 的软件后端）
// 连 f32 基线都建不出 shader。这里只保留本变体需要的声明。

struct Params {
    m: u32,
    n: u32,
    k: u32,
};

@group(0) @binding(2) var<storage, read_write> out: array<f32>;
@group(0) @binding(3) var<uniform> params: Params;
@group(0) @binding(6) var<storage, read> ha4: array<vec4<f16>>;
@group(0) @binding(7) var<storage, read> hb4: array<vec4<f16>>;

// 与 matmul_f32.wgsl 的 main_tiled2k8 同一个 K 段宽度。
const K8: u32 = 8u;

// fp16 存储变体：与 main_tiled2k8 完全同结构（16×16 tile、8×8 线程、2×2 输出、K 段 8），
// 只把 A/B 的**存储与共享内存**换成 vec4<f16>：全局带宽与共享带宽各减半，累加仍用 f32。
// 目的：量化「f32 基线里有多少是带宽账」。

var<workgroup> t2k8h_a4: array<vec4<f16>, 128>;
var<workgroup> t2k8h_b4: array<vec4<f16>, 128>;

@compute @workgroup_size(8, 8)
fn main_tiled2k8_f16(
    @builtin(workgroup_id) wg: vec3<u32>,
    @builtin(local_invocation_id) lid: vec3<u32>,
) {
    let row_base = wg.y * 16u;
    let col_base = wg.x * 16u;
    let r0 = lid.y * 2u;
    let c0 = lid.x * 2u;
    let idx = lid.y * 8u + lid.x;
    var s00 = 0.0;
    var s01 = 0.0;
    var s10 = 0.0;
    var s11 = 0.0;
    let k_tiles = (params.k + K8 - 1u) / K8;
    let k4 = params.k / 4u;
    let vector_ok = params.k % 4u == 0u;
    for (var chunk = 0u; chunk < k_tiles; chunk = chunk + 1u) {
        if (idx < 32u) {
            let r = idx / 2u;
            let part = idx % 2u;
            let slot = r * 8u + (part ^ ((r >> 1u) & 7u));
            if (vector_ok) {
                let seg_ok = (chunk * 2u + part) * 4u < params.k;
                t2k8h_a4[slot] = select(vec4<f16>(0.0h), ha4[(row_base + r) * k4 + chunk * 2u + part], row_base + r < params.m && seg_ok);
                t2k8h_b4[slot] = select(vec4<f16>(0.0h), hb4[(col_base + r) * k4 + chunk * 2u + part], col_base + r < params.n && seg_ok);
            } else {
                // 标量回退：k0 是 4 的倍数，正好落在一个 vec4<f16> 上。
                let k0 = chunk * K8 + part * 4u;
                let a_ok = row_base + r < params.m;
                let b_ok = col_base + r < params.n;
                let av = select(vec4<f16>(0.0h), ha4[(row_base + r) * k4 + k0 / 4u], a_ok);
                let bv = select(vec4<f16>(0.0h), hb4[(col_base + r) * k4 + k0 / 4u], b_ok);
                t2k8h_a4[slot] = vec4<f16>(
                    select(0.0h, av.x, k0 + 0u < params.k),
                    select(0.0h, av.y, k0 + 1u < params.k),
                    select(0.0h, av.z, k0 + 2u < params.k),
                    select(0.0h, av.w, k0 + 3u < params.k),
                );
                t2k8h_b4[slot] = vec4<f16>(
                    select(0.0h, bv.x, k0 + 0u < params.k),
                    select(0.0h, bv.y, k0 + 1u < params.k),
                    select(0.0h, bv.z, k0 + 2u < params.k),
                    select(0.0h, bv.w, k0 + 3u < params.k),
                );
            }
        }
        workgroupBarrier();
        for (var w4 = 0u; w4 < 2u; w4 = w4 + 1u) {
            let a0 = t2k8h_a4[r0 * 8u + (w4 ^ ((r0 >> 1u) & 7u))];
            let a1 = t2k8h_a4[(r0 + 1u) * 8u + (w4 ^ (((r0 + 1u) >> 1u) & 7u))];
            let b0 = t2k8h_b4[c0 * 8u + (w4 ^ ((c0 >> 1u) & 7u))];
            let b1 = t2k8h_b4[(c0 + 1u) * 8u + (w4 ^ (((c0 + 1u) >> 1u) & 7u))];
            s00 = s00 + f32(a0.x) * f32(b0.x);
            s00 = s00 + f32(a0.y) * f32(b0.y);
            s00 = s00 + f32(a0.z) * f32(b0.z);
            s00 = s00 + f32(a0.w) * f32(b0.w);
            s01 = s01 + f32(a0.x) * f32(b1.x);
            s01 = s01 + f32(a0.y) * f32(b1.y);
            s01 = s01 + f32(a0.z) * f32(b1.z);
            s01 = s01 + f32(a0.w) * f32(b1.w);
            s10 = s10 + f32(a1.x) * f32(b0.x);
            s10 = s10 + f32(a1.y) * f32(b0.y);
            s10 = s10 + f32(a1.z) * f32(b0.z);
            s10 = s10 + f32(a1.w) * f32(b0.w);
            s11 = s11 + f32(a1.x) * f32(b1.x);
            s11 = s11 + f32(a1.y) * f32(b1.y);
            s11 = s11 + f32(a1.z) * f32(b1.z);
            s11 = s11 + f32(a1.w) * f32(b1.w);
        }
        workgroupBarrier();
    }
    let row0 = row_base + r0;
    let col0 = col_base + c0;
    if (row0 < params.m && col0 < params.n) {
        out[row0 * params.n + col0] = s00;
    }
    if (row0 < params.m && col0 + 1u < params.n) {
        out[row0 * params.n + col0 + 1u] = s01;
    }
    if (row0 + 1u < params.m && col0 < params.n) {
        out[(row0 + 1u) * params.n + col0] = s10;
    }
    if (row0 + 1u < params.m && col0 + 1u < params.n) {
        out[(row0 + 1u) * params.n + col0 + 1u] = s11;
    }
}
