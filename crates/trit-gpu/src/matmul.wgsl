// 三值矩阵乘内核（双导轨 + 四组 POPC）与同结构 f32 基线。
//
// C[m][n] = Σ_k A[m][k]·B[k][n]。B 按**转置**行主序存放（b_pos[n][w] 是 B 的第 n 列），
// 于是每个输出就是两次行向量的三值点积——与 dot 内核同一公式：
//   Σ popcount(ap&bp) + popcount(an&bn) − popcount(ap&bn) − popcount(an&bp)
// 两侧都是 naive「一线程一输出、循环内积维度」，唯一变量是数据表示（2 bit/trit vs 32 bit）。

struct Params {
    m: u32,
    n: u32,
    words: u32,
};

@group(0) @binding(0) var<storage, read> a_pos: array<u32>;
@group(0) @binding(1) var<storage, read> a_neg: array<u32>;
@group(0) @binding(2) var<storage, read> b_pos: array<u32>;
@group(0) @binding(3) var<storage, read> b_neg: array<u32>;
@group(0) @binding(4) var<storage, read_write> out: array<i32>;
@group(0) @binding(5) var<uniform> params: Params;

@compute @workgroup_size(64)
fn main(@builtin(global_invocation_id) gid: vec3<u32>) {
    let index = gid.x;
    if (index >= params.m * params.n) {
        return;
    }
    let row = index / params.n;
    let col = index % params.n;
    let a_base = row * params.words;
    let b_base = col * params.words;
    var sum = 0;
    for (var w = 0u; w < params.words; w = w + 1u) {
        let ap = a_pos[a_base + w];
        let an = a_neg[a_base + w];
        let bp = b_pos[b_base + w];
        let bn = b_neg[b_base + w];
        sum = sum
            + i32(countOneBits(ap & bp))
            + i32(countOneBits(an & bn))
            - i32(countOneBits(ap & bn))
            - i32(countOneBits(an & bp));
    }
    out[index] = sum;
}

// 分块版（共享内存）：16×16 的输出 tile，一次协作加载 8-word（256 trit）的
// A 行段与 B 列段进 workgroup storage——A 段被 tile 内 16 个列线程复用、
// B 段被 16 个行线程复用，把重复读砍 16×。同 bind group、同数据，只有映射不同。
const TILED: u32 = 16u;
const TILED_WORDS: u32 = 8u;

var<workgroup> tile_ap: array<u32, 512>;
var<workgroup> tile_an: array<u32, 512>;
var<workgroup> tile_bp: array<u32, 512>;
var<workgroup> tile_bn: array<u32, 512>;

@compute @workgroup_size(16, 16)
fn main_tiled(
    @builtin(global_invocation_id) gid: vec3<u32>,
    @builtin(local_invocation_id) lid: vec3<u32>,
) {
    let row = gid.y;
    let col = gid.x;
    let linear = lid.y * TILED + lid.x;
    var sum = 0;
    let word_tiles = (params.words + TILED_WORDS - 1u) / TILED_WORDS;
    for (var chunk = 0u; chunk < word_tiles; chunk = chunk + 1u) {
        // 每段 8 字、tile 有 16×16 线程：A 由 lid.x<8 的线程装入 lid.y 行的 8 个槽，
        // B 由 lid.y<8 的线程装入 lid.x 列的 8 个槽——槽位布局必须与下方累加读取
        // 一致（否则就是「加载 16 宽、累加 8 宽」的错位，读进相邻 chunk 的字）。
        if (lid.x < TILED_WORDS) {
            let a_word = chunk * TILED_WORDS + lid.x;
            let a_ok = row < params.m && a_word < params.words;
            let slot = lid.y * TILED_WORDS + lid.x;
            tile_ap[slot] = select(0u, a_pos[row * params.words + a_word], a_ok);
            tile_an[slot] = select(0u, a_neg[row * params.words + a_word], a_ok);
        }
        if (lid.y < TILED_WORDS) {
            let b_word = chunk * TILED_WORDS + lid.y;
            let b_ok = col < params.n && b_word < params.words;
            let slot = lid.x * TILED_WORDS + lid.y;
            tile_bp[slot] = select(0u, b_pos[col * params.words + b_word], b_ok);
            tile_bn[slot] = select(0u, b_neg[col * params.words + b_word], b_ok);
        }
        workgroupBarrier();
        for (var w = 0u; w < TILED_WORDS; w = w + 1u) {
            let ap = tile_ap[lid.y * TILED_WORDS + w];
            let an = tile_an[lid.y * TILED_WORDS + w];
            let bp = tile_bp[lid.x * TILED_WORDS + w];
            let bn = tile_bn[lid.x * TILED_WORDS + w];
            sum = sum
                + i32(countOneBits(ap & bp))
                + i32(countOneBits(an & bn))
                - i32(countOneBits(ap & bn))
                - i32(countOneBits(an & bp));
        }
        workgroupBarrier();
    }
    if (row < params.m && col < params.n) {
        out[row * params.n + col] = sum;
    }
}

// 寄存器阻塞版：workgroup 8×8，每线程算 2×2 共 4 个输出——共享内存里的
// A/B 行段每次读取喂 4 个累加器（算术强度再翻倍）。tile 仍是 16×16、
// 每段 8-word，装载由 64 线程各担 2 槽完成。
@compute @workgroup_size(8, 8)
fn main_tiled2(
    @builtin(workgroup_id) wg: vec3<u32>,
    @builtin(local_invocation_id) lid: vec3<u32>,
) {
    // 每 workgroup 覆盖一个 16×16 输出 tile：基址必须来自 workgroup 号，
    // 不能用 global_invocation_id（那是线程号，乘 16 会让相邻组互相覆盖输出）。
    let row_base = wg.y * 16u;
    let col_base = wg.x * 16u;
    let r0 = lid.y * 2u;
    let c0 = lid.x * 2u;
    let idx = lid.y * 8u + lid.x;
    var s00 = 0;
    var s01 = 0;
    var s10 = 0;
    var s11 = 0;
    let word_tiles = (params.words + TILED_WORDS - 1u) / TILED_WORDS;
    for (var chunk = 0u; chunk < word_tiles; chunk = chunk + 1u) {
        // 64 线程各装 2 槽：槽 = r*8+w（A 按行、B 按列）。
        let s1 = idx;
        let s2 = idx + 64u;
        let ar1 = s1 / 8u;
        let aw1 = chunk * TILED_WORDS + s1 % 8u;
        let ar2 = s2 / 8u;
        let aw2 = chunk * TILED_WORDS + s2 % 8u;
        tile_ap[s1] = select(0u, a_pos[(row_base + ar1) * params.words + aw1], row_base + ar1 < params.m && aw1 < params.words);
        tile_an[s1] = select(0u, a_neg[(row_base + ar1) * params.words + aw1], row_base + ar1 < params.m && aw1 < params.words);
        tile_ap[s2] = select(0u, a_pos[(row_base + ar2) * params.words + aw2], row_base + ar2 < params.m && aw2 < params.words);
        tile_an[s2] = select(0u, a_neg[(row_base + ar2) * params.words + aw2], row_base + ar2 < params.m && aw2 < params.words);
        let bc1 = col_base + (s1 / 8u);
        let bw1 = chunk * TILED_WORDS + s1 % 8u;
        let bc2 = col_base + (s2 / 8u);
        let bw2 = chunk * TILED_WORDS + s2 % 8u;
        tile_bp[s1] = select(0u, b_pos[(bc1) * params.words + bw1], bc1 < params.n && bw1 < params.words);
        tile_bn[s1] = select(0u, b_neg[(bc1) * params.words + bw1], bc1 < params.n && bw1 < params.words);
        tile_bp[s2] = select(0u, b_pos[(bc2) * params.words + bw2], bc2 < params.n && bw2 < params.words);
        tile_bn[s2] = select(0u, b_neg[(bc2) * params.words + bw2], bc2 < params.n && bw2 < params.words);
        workgroupBarrier();
        for (var w = 0u; w < TILED_WORDS; w = w + 1u) {
            let a0p = tile_ap[r0 * TILED_WORDS + w];
            let a0n = tile_an[r0 * TILED_WORDS + w];
            let a1p = tile_ap[(r0 + 1u) * TILED_WORDS + w];
            let a1n = tile_an[(r0 + 1u) * TILED_WORDS + w];
            let b0p = tile_bp[c0 * TILED_WORDS + w];
            let b0n = tile_bn[c0 * TILED_WORDS + w];
            let b1p = tile_bp[(c0 + 1u) * TILED_WORDS + w];
            let b1n = tile_bn[(c0 + 1u) * TILED_WORDS + w];
            s00 = s00 + i32(countOneBits(a0p & b0p)) + i32(countOneBits(a0n & b0n))
                - i32(countOneBits(a0p & b0n)) - i32(countOneBits(a0n & b0p));
            s01 = s01 + i32(countOneBits(a0p & b1p)) + i32(countOneBits(a0n & b1n))
                - i32(countOneBits(a0p & b1n)) - i32(countOneBits(a0n & b1p));
            s10 = s10 + i32(countOneBits(a1p & b0p)) + i32(countOneBits(a1n & b0n))
                - i32(countOneBits(a1p & b0n)) - i32(countOneBits(a1n & b0p));
            s11 = s11 + i32(countOneBits(a1p & b1p)) + i32(countOneBits(a1n & b1n))
                - i32(countOneBits(a1p & b1n)) - i32(countOneBits(a1n & b1p));
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

// 4×4 寄存器阻塞：32×32 输出 tile、8×8 线程、每线程 4×4 个输出。
// 每个 chunk 只搬 (32 行 + 32 列) × 8 word 进共享内存，喂满 32×32 个输出；
// 相对 2×2 版，共享内存读取次数/MAC 再降一半。
const T4: u32 = 32u;
const T4_WORDS: u32 = 8u;

var<workgroup> t4_ap: array<u32, 256>;
var<workgroup> t4_an: array<u32, 256>;
var<workgroup> t4_bp: array<u32, 256>;
var<workgroup> t4_bn: array<u32, 256>;

@compute @workgroup_size(8, 8)
fn main_tiled4(
    @builtin(workgroup_id) wg: vec3<u32>,
    @builtin(local_invocation_id) lid: vec3<u32>,
) {
    let row_base = wg.y * T4;
    let col_base = wg.x * T4;
    let r0 = lid.y * 4u;
    let c0 = lid.x * 4u;
    let idx = lid.y * 8u + lid.x;
    var s00 = 0; var s01 = 0; var s02 = 0; var s03 = 0;
    var s10 = 0; var s11 = 0; var s12 = 0; var s13 = 0;
    var s20 = 0; var s21 = 0; var s22 = 0; var s23 = 0;
    var s30 = 0; var s31 = 0; var s32 = 0; var s33 = 0;
    let word_tiles = (params.words + T4_WORDS - 1u) / T4_WORDS;
    for (var chunk = 0u; chunk < word_tiles; chunk = chunk + 1u) {
        // 64 线程 × 4 槽 = 256 = 32 行/列 × 8 word。
        for (var t = 0u; t < 4u; t = t + 1u) {
            let slot = idx + t * 64u;
            let line = slot / T4_WORDS;
            let word = chunk * T4_WORDS + slot % T4_WORDS;
            let ok_a = row_base + line < params.m && word < params.words;
            let ok_b = col_base + line < params.n && word < params.words;
            t4_ap[slot] = select(0u, a_pos[(row_base + line) * params.words + word], ok_a);
            t4_an[slot] = select(0u, a_neg[(row_base + line) * params.words + word], ok_a);
            t4_bp[slot] = select(0u, b_pos[(col_base + line) * params.words + word], ok_b);
            t4_bn[slot] = select(0u, b_neg[(col_base + line) * params.words + word], ok_b);
        }
        workgroupBarrier();
        for (var w = 0u; w < T4_WORDS; w = w + 1u) {
            let a0p = t4_ap[(r0 + 0u) * T4_WORDS + w];
            let a0n = t4_an[(r0 + 0u) * T4_WORDS + w];
            let a1p = t4_ap[(r0 + 1u) * T4_WORDS + w];
            let a1n = t4_an[(r0 + 1u) * T4_WORDS + w];
            let a2p = t4_ap[(r0 + 2u) * T4_WORDS + w];
            let a2n = t4_an[(r0 + 2u) * T4_WORDS + w];
            let a3p = t4_ap[(r0 + 3u) * T4_WORDS + w];
            let a3n = t4_an[(r0 + 3u) * T4_WORDS + w];
            let b0p = t4_bp[(c0 + 0u) * T4_WORDS + w];
            let b0n = t4_bn[(c0 + 0u) * T4_WORDS + w];
            let b1p = t4_bp[(c0 + 1u) * T4_WORDS + w];
            let b1n = t4_bn[(c0 + 1u) * T4_WORDS + w];
            let b2p = t4_bp[(c0 + 2u) * T4_WORDS + w];
            let b2n = t4_bn[(c0 + 2u) * T4_WORDS + w];
            let b3p = t4_bp[(c0 + 3u) * T4_WORDS + w];
            let b3n = t4_bn[(c0 + 3u) * T4_WORDS + w];
            s00 = s00 + i32(countOneBits(a0p & b0p)) + i32(countOneBits(a0n & b0n)) - i32(countOneBits(a0p & b0n)) - i32(countOneBits(a0n & b0p));
            s01 = s01 + i32(countOneBits(a0p & b1p)) + i32(countOneBits(a0n & b1n)) - i32(countOneBits(a0p & b1n)) - i32(countOneBits(a0n & b1p));
            s02 = s02 + i32(countOneBits(a0p & b2p)) + i32(countOneBits(a0n & b2n)) - i32(countOneBits(a0p & b2n)) - i32(countOneBits(a0n & b2p));
            s03 = s03 + i32(countOneBits(a0p & b3p)) + i32(countOneBits(a0n & b3n)) - i32(countOneBits(a0p & b3n)) - i32(countOneBits(a0n & b3p));
            s10 = s10 + i32(countOneBits(a1p & b0p)) + i32(countOneBits(a1n & b0n)) - i32(countOneBits(a1p & b0n)) - i32(countOneBits(a1n & b0p));
            s11 = s11 + i32(countOneBits(a1p & b1p)) + i32(countOneBits(a1n & b1n)) - i32(countOneBits(a1p & b1n)) - i32(countOneBits(a1n & b1p));
            s12 = s12 + i32(countOneBits(a1p & b2p)) + i32(countOneBits(a1n & b2n)) - i32(countOneBits(a1p & b2n)) - i32(countOneBits(a1n & b2p));
            s13 = s13 + i32(countOneBits(a1p & b3p)) + i32(countOneBits(a1n & b3n)) - i32(countOneBits(a1p & b3n)) - i32(countOneBits(a1n & b3p));
            s20 = s20 + i32(countOneBits(a2p & b0p)) + i32(countOneBits(a2n & b0n)) - i32(countOneBits(a2p & b0n)) - i32(countOneBits(a2n & b0p));
            s21 = s21 + i32(countOneBits(a2p & b1p)) + i32(countOneBits(a2n & b1n)) - i32(countOneBits(a2p & b1n)) - i32(countOneBits(a2n & b1p));
            s22 = s22 + i32(countOneBits(a2p & b2p)) + i32(countOneBits(a2n & b2n)) - i32(countOneBits(a2p & b2n)) - i32(countOneBits(a2n & b2p));
            s23 = s23 + i32(countOneBits(a2p & b3p)) + i32(countOneBits(a2n & b3n)) - i32(countOneBits(a2p & b3n)) - i32(countOneBits(a2n & b3p));
            s30 = s30 + i32(countOneBits(a3p & b0p)) + i32(countOneBits(a3n & b0n)) - i32(countOneBits(a3p & b0n)) - i32(countOneBits(a3n & b0p));
            s31 = s31 + i32(countOneBits(a3p & b1p)) + i32(countOneBits(a3n & b1n)) - i32(countOneBits(a3p & b1n)) - i32(countOneBits(a3n & b1p));
            s32 = s32 + i32(countOneBits(a3p & b2p)) + i32(countOneBits(a3n & b2n)) - i32(countOneBits(a3p & b2n)) - i32(countOneBits(a3n & b2p));
            s33 = s33 + i32(countOneBits(a3p & b3p)) + i32(countOneBits(a3n & b3n)) - i32(countOneBits(a3p & b3n)) - i32(countOneBits(a3n & b3p));
        }
        workgroupBarrier();
    }
    let row0 = row_base + r0;
    let col0 = col_base + c0;
    let acc = array<i32, 16>(s00, s01, s02, s03, s10, s11, s12, s13, s20, s21, s22, s23, s30, s31, s32, s33);
    for (var i = 0u; i < 4u; i = i + 1u) {
        for (var j = 0u; j < 4u; j = j + 1u) {
            if (row0 + i < params.m && col0 + j < params.n) {
                out[(row0 + i) * params.n + col0 + j] = acc[i * 4u + j];
            }
        }
    }
}
