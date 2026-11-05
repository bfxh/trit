// 三值权重 × f32 激活：16×16 共享内存 tile。
//
// C[m][n] = Σ_k A[m][k] · B[n][k]
// A 是双导轨三值位平面；B 是 f32（列主序，每列补齐到 32 的倍数）。
//
// 一个 workgroup 算 16 行 × 16 列：A 位只在行方向取 16 次，B 的 16 列 × 32 个 k
// 先装进 workgroup 共享内存，再被 16 行复用 —— 把 B 的全局读从“每行一遍”降到
// “每个 16 行 tile 一遍”。

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

var<workgroup> apos: array<u32, 16>;
var<workgroup> aneg: array<u32, 16>;
// 16 列 × 8 个 vec4（32 个 f32）
var<workgroup> btile: array<vec4<f32>, 144>; // 16 列 × 9（填充 1 个 vec4，避开 bank 冲突）

fn bit4(word: u32, base: u32) -> vec4<bool> {
    return vec4<bool>(
        (word & (1u << base)) != 0u,
        (word & (1u << (base + 1u))) != 0u,
        (word & (1u << (base + 2u))) != 0u,
        (word & (1u << (base + 3u))) != 0u,
    );
}

@compute @workgroup_size(256)
fn main(
    @builtin(workgroup_id) wid: vec3<u32>,
    @builtin(local_invocation_index) lid: u32,
) {
    let tile_m = wid.y;
    let tile_n = wid.x;
    let row = lid / 16u;
    let col = lid % 16u;
    let stride = params.words * 8u; // 每列 words*32 f32 = words*8 个 vec4

    var acc = vec4<f32>(0.0);

    for (var w = 0u; w < params.words; w = w + 1u) {
        // A：16 行 × 当前 word 的位
        if (lid < 16u) {
            let global_m = tile_m * 16u + lid;
            if (global_m < params.m) {
                apos[lid] = a_pos[global_m * params.words + w];
                aneg[lid] = a_neg[global_m * params.words + w];
            } else {
                apos[lid] = 0u;
                aneg[lid] = 0u;
            }
        }
        // B：16 列 × 8 个 vec4（32 个 k）
        if (lid < 128u) {
            let c = lid / 8u;
            let j = lid % 8u;
            let global_n = tile_n * 16u + c;
            if (global_n < params.n) {
                btile[c * 9u + j] = b[global_n * stride + w * 8u + j];
            } else {
                btile[c * 9u + j] = vec4<f32>(0.0);
            }
        }
        workgroupBarrier();

        let ap = apos[row];
        let an = aneg[row];
        for (var j = 0u; j < 8u; j = j + 1u) {
            let pp = bit4(ap, j * 4u);
            let nn = bit4(an, j * 4u);
            let v = btile[col * 9u + j];
            acc += select(vec4<f32>(0.0), v, pp) - select(vec4<f32>(0.0), v, nn);
        }
        workgroupBarrier();
    }

    let global_m = tile_m * 16u + row;
    let global_n = tile_n * 16u + col;
    if (global_m < params.m && global_n < params.n) {
        out[global_m * params.n + global_n] = acc.x + acc.y + acc.z + acc.w;
    }
}
