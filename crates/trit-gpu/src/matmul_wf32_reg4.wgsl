// 三值权重 × f32 激活的矩阵乘：每线程 4 列，A 位只取一次，B 用 vec4 装载。
//
// C[m][n] = Σ_k A[m][k] · B[n][k]
// A 是 {-1,0,+1} 权重（双导轨位平面），B 是 f32 激活（列主序，每列补齐到 32 的倍数）。
//
// 映射：index = row * ceil(n/4) + col_group；一个线程算 4 列。
// 这样每个 k word 的 A 位只取一次、给 4 列复用；B 每 4 个 k 用一次 vec4 装载。

struct Params {
    m: u32,
    n: u32,
    k: u32,
    words: u32,
};

@group(0) @binding(0) var<storage, read> a_pos: array<u32>;
@group(0) @binding(1) var<storage, read> a_neg: array<u32>;
@group(0) @binding(2) var<storage, read> b: array<vec4<f32>>;
@group(0) @binding(3) var<storage, read_write> out: array<f32>;
@group(0) @binding(4) var<uniform> params: Params;

fn bit4(word: u32, base: u32) -> vec4<bool> {
    return vec4<bool>(
        (word & (1u << base)) != 0u,
        (word & (1u << (base + 1u))) != 0u,
        (word & (1u << (base + 2u))) != 0u,
        (word & (1u << (base + 3u))) != 0u,
    );
}

@compute @workgroup_size(64)
fn main(@builtin(global_invocation_id) gid: vec3<u32>) {
    let groups = (params.n + 3u) / 4u;
    if (gid.x >= params.m * groups) {
        return;
    }
    let row = gid.x / groups;
    let col_group = gid.x % groups;
    let c0 = col_group * 4u;
    // 尾部列钳到最后一列：多算但不越界；写出时再按 n 裁掉。
    let c1 = min(c0 + 1u, params.n - 1u);
    let c2 = min(c0 + 2u, params.n - 1u);
    let c3 = min(c0 + 3u, params.n - 1u);

    let a_base = row * params.words;
    let stride = params.words * 8u; // 每列 words*32 个 f32 = words*8 个 vec4
    let b0 = c0 * stride;
    let b1 = c1 * stride;
    let b2 = c2 * stride;
    let b3 = c3 * stride;

    var acc0 = vec4<f32>(0.0);
    var acc1 = vec4<f32>(0.0);
    var acc2 = vec4<f32>(0.0);
    var acc3 = vec4<f32>(0.0);

    for (var w = 0u; w < params.words; w = w + 1u) {
        let ap = a_pos[a_base + w];
        let an = a_neg[a_base + w];
        let chunk = w * 8u;
        for (var i = 0u; i < 8u; i = i + 1u) {
            let bit = i * 4u;
            let pp = bit4(ap, bit);
            let nn = bit4(an, bit);
            let v0 = b[b0 + chunk + i];
            let v1 = b[b1 + chunk + i];
            let v2 = b[b2 + chunk + i];
            let v3 = b[b3 + chunk + i];
            acc0 += select(vec4<f32>(0.0), v0, pp) - select(vec4<f32>(0.0), v0, nn);
            acc1 += select(vec4<f32>(0.0), v1, pp) - select(vec4<f32>(0.0), v1, nn);
            acc2 += select(vec4<f32>(0.0), v2, pp) - select(vec4<f32>(0.0), v2, nn);
            acc3 += select(vec4<f32>(0.0), v3, pp) - select(vec4<f32>(0.0), v3, nn);
        }
    }

    let s0 = acc0.x + acc0.y + acc0.z + acc0.w;
    let s1 = acc1.x + acc1.y + acc1.z + acc1.w;
    let s2 = acc2.x + acc2.y + acc2.z + acc2.w;
    let s3 = acc3.x + acc3.y + acc3.z + acc3.w;
    let out_base = row * params.n + c0;
    if (c0 < params.n) {
        out[out_base] = s0;
    }
    if (c0 + 1u < params.n) {
        out[out_base + 1u] = s1;
    }
    if (c0 + 2u < params.n) {
        out[out_base + 2u] = s2;
    }
    if (c0 + 3u < params.n) {
        out[out_base + 3u] = s3;
    }
}
