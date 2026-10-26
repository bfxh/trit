// 三值权重 × f32 激活的矩阵乘。
//
// C[m][n] = Σ_k A[m][k] · B[n][k]
// A 是 {-1,0,+1} 权重（双导轨位平面），B 是 f32 激活（按列主序，每列补齐到 64 的倍数）。
// 每个输出：+1 加激活、-1 减激活、0 跳过，没有浮点乘法。
//
// 线程映射：index = row * n + col，同一 warp 内 row 相同（n 是 64 的倍数），
// 所以权重位在整个 warp 上一致，分支是 warp-uniform 的，不会被 ±1/0 随机打散。

struct Params {
    m: u32,
    n: u32,
    k: u32,
    words: u32,
};

@group(0) @binding(0) var<storage, read> a_pos: array<u32>;
@group(0) @binding(1) var<storage, read> a_neg: array<u32>;
@group(0) @binding(2) var<storage, read> b: array<f32>;
@group(0) @binding(3) var<storage, read_write> out: array<f32>;
@group(0) @binding(4) var<uniform> params: Params;

@compute @workgroup_size(64)
fn main(@builtin(global_invocation_id) gid: vec3<u32>) {
    let index = gid.x;
    if (index >= params.m * params.n) {
        return;
    }
    let row = index / params.n;
    let col = index % params.n;
    let a_base = row * params.words;
    let b_base = col * params.words * 32u;
    var sum = 0.0;
    for (var w = 0u; w < params.words; w = w + 1u) {
        let ap = a_pos[a_base + w];
        let an = a_neg[a_base + w];
        let base = w * 32u;
        for (var i = 0u; i < 32u; i = i + 1u) {
            let kk = base + i;
            if (kk >= params.k) {
                break;
            }
            let bit = 1u << i;
            if ((ap & bit) != 0u) {
                sum = sum + b[b_base + kk];
            } else if ((an & bit) != 0u) {
                sum = sum - b[b_base + kk];
            }
        }
    }
    out[index] = sum;
}
