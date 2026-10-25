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
// 共享行距是 9 word 而不是 8：32 B 行距会让 8 行落在同一 bank 组，9 是奇数、行间完全错开。
// A 行段与 B 列段进 workgroup storage——A 段被 tile 内 16 个列线程复用、
// B 段被 16 个行线程复用，把重复读砍 16×。同 bind group、同数据，只有映射不同。
const TILED: u32 = 16u;
const TILED_WORDS: u32 = 8u;

var<workgroup> tile_ap: array<u32, 144>;
var<workgroup> tile_an: array<u32, 144>;
var<workgroup> tile_bp: array<u32, 144>;
var<workgroup> tile_bn: array<u32, 144>;

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
            let slot = lid.y * 9u + lid.x;
            tile_ap[slot] = select(0u, a_pos[row * params.words + a_word], a_ok);
            tile_an[slot] = select(0u, a_neg[row * params.words + a_word], a_ok);
        }
        if (lid.y < TILED_WORDS) {
            let b_word = chunk * TILED_WORDS + lid.y;
            let b_ok = col < params.n && b_word < params.words;
            let slot = lid.x * 9u + lid.y;
            tile_bp[slot] = select(0u, b_pos[col * params.words + b_word], b_ok);
            tile_bn[slot] = select(0u, b_neg[col * params.words + b_word], b_ok);
        }
        workgroupBarrier();
        for (var w = 0u; w < TILED_WORDS; w = w + 1u) {
            let ap = tile_ap[lid.y * 9u + w];
            let an = tile_an[lid.y * 9u + w];
            let bp = tile_bp[lid.x * 9u + w];
            let bn = tile_bn[lid.x * 9u + w];
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
        tile_ap[(s1 / 8u) * 9u + s1 % 8u] = select(0u, a_pos[(row_base + ar1) * params.words + aw1], row_base + ar1 < params.m && aw1 < params.words);
        tile_an[(s1 / 8u) * 9u + s1 % 8u] = select(0u, a_neg[(row_base + ar1) * params.words + aw1], row_base + ar1 < params.m && aw1 < params.words);
        tile_ap[(s2 / 8u) * 9u + s2 % 8u] = select(0u, a_pos[(row_base + ar2) * params.words + aw2], row_base + ar2 < params.m && aw2 < params.words);
        tile_an[(s2 / 8u) * 9u + s2 % 8u] = select(0u, a_neg[(row_base + ar2) * params.words + aw2], row_base + ar2 < params.m && aw2 < params.words);
        let bc1 = col_base + (s1 / 8u);
        let bw1 = chunk * TILED_WORDS + s1 % 8u;
        let bc2 = col_base + (s2 / 8u);
        let bw2 = chunk * TILED_WORDS + s2 % 8u;
        tile_bp[(s1 / 8u) * 9u + s1 % 8u] = select(0u, b_pos[(bc1) * params.words + bw1], bc1 < params.n && bw1 < params.words);
        tile_bn[(s1 / 8u) * 9u + s1 % 8u] = select(0u, b_neg[(bc1) * params.words + bw1], bc1 < params.n && bw1 < params.words);
        tile_bp[(s2 / 8u) * 9u + s2 % 8u] = select(0u, b_pos[(bc2) * params.words + bw2], bc2 < params.n && bw2 < params.words);
        tile_bn[(s2 / 8u) * 9u + s2 % 8u] = select(0u, b_neg[(bc2) * params.words + bw2], bc2 < params.n && bw2 < params.words);
        workgroupBarrier();
        for (var w = 0u; w < TILED_WORDS; w = w + 1u) {
            let a0p = tile_ap[r0 * 9u + w];
            let a0n = tile_an[r0 * 9u + w];
            let a1p = tile_ap[(r0 + 1u) * 9u + w];
            let a1n = tile_an[(r0 + 1u) * 9u + w];
            let b0p = tile_bp[c0 * 9u + w];
            let b0n = tile_bn[c0 * 9u + w];
            let b1p = tile_bp[(c0 + 1u) * 9u + w];
            let b1n = tile_bn[(c0 + 1u) * 9u + w];
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

var<workgroup> t4_ap: array<u32, 288>;
var<workgroup> t4_an: array<u32, 288>;
var<workgroup> t4_bp: array<u32, 288>;
var<workgroup> t4_bn: array<u32, 288>;

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
            t4_ap[(slot / 8u) * 9u + slot % 8u] = select(0u, a_pos[(row_base + line) * params.words + word], ok_a);
            t4_an[(slot / 8u) * 9u + slot % 8u] = select(0u, a_neg[(row_base + line) * params.words + word], ok_a);
            t4_bp[(slot / 8u) * 9u + slot % 8u] = select(0u, b_pos[(col_base + line) * params.words + word], ok_b);
            t4_bn[(slot / 8u) * 9u + slot % 8u] = select(0u, b_neg[(col_base + line) * params.words + word], ok_b);
        }
        workgroupBarrier();
        for (var w = 0u; w < T4_WORDS; w = w + 1u) {
            let a0p = t4_ap[(r0 + 0u) * 9u + w];
            let a0n = t4_an[(r0 + 0u) * 9u + w];
            let a1p = t4_ap[(r0 + 1u) * 9u + w];
            let a1n = t4_an[(r0 + 1u) * 9u + w];
            let a2p = t4_ap[(r0 + 2u) * 9u + w];
            let a2n = t4_an[(r0 + 2u) * 9u + w];
            let a3p = t4_ap[(r0 + 3u) * 9u + w];
            let a3n = t4_an[(r0 + 3u) * 9u + w];
            let b0p = t4_bp[(c0 + 0u) * 9u + w];
            let b0n = t4_bn[(c0 + 0u) * 9u + w];
            let b1p = t4_bp[(c0 + 1u) * 9u + w];
            let b1n = t4_bn[(c0 + 1u) * 9u + w];
            let b2p = t4_bp[(c0 + 2u) * 9u + w];
            let b2n = t4_bn[(c0 + 2u) * 9u + w];
            let b3p = t4_bp[(c0 + 3u) * 9u + w];
            let b3n = t4_bn[(c0 + 3u) * 9u + w];
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

// 2×2 微块 + **pos/neg 打包成 vec2<u32>**：共享内存一次 64 位读拿两平面，
// 把每步的 ld.shared 指令数从 8 降到 4（tiled2 是 8 次 u32 读）。
// 结构仍与 main_tiled2 完全一致（16×16 tile、8×8 线程、2×2 输出、8-word 段）。
const V_TILE: u32 = 16u;
const V_WORDS: u32 = 8u;

var<workgroup> v_a: array<vec2<u32>, 128>;
var<workgroup> v_b: array<vec2<u32>, 128>;

@compute @workgroup_size(8, 8)
fn main_tiled2v(
    @builtin(workgroup_id) wg: vec3<u32>,
    @builtin(local_invocation_id) lid: vec3<u32>,
) {
    let row_base = wg.y * V_TILE;
    let col_base = wg.x * V_TILE;
    let r0 = lid.y * 2u;
    let c0 = lid.x * 2u;
    let idx = lid.y * 8u + lid.x;
    var s00 = 0;
    var s01 = 0;
    var s10 = 0;
    var s11 = 0;
    let word_tiles = (params.words + V_WORDS - 1u) / V_WORDS;
    for (var chunk = 0u; chunk < word_tiles; chunk = chunk + 1u) {
        // 64 线程 × 2 槽 = 128 = 16 行/列 × 8 word。
        for (var t = 0u; t < 2u; t = t + 1u) {
            let slot = idx + t * 64u;
            let line = slot / V_WORDS;
            let word = chunk * V_WORDS + slot % V_WORDS;
            let ok_a = row_base + line < params.m && word < params.words;
            let ok_b = col_base + line < params.n && word < params.words;
            v_a[slot] = vec2<u32>(
                select(0u, a_pos[(row_base + line) * params.words + word], ok_a),
                select(0u, a_neg[(row_base + line) * params.words + word], ok_a),
            );
            v_b[slot] = vec2<u32>(
                select(0u, b_pos[(col_base + line) * params.words + word], ok_b),
                select(0u, b_neg[(col_base + line) * params.words + word], ok_b),
            );
        }
        workgroupBarrier();
        for (var w = 0u; w < V_WORDS; w = w + 1u) {
            let a0 = v_a[r0 * V_WORDS + w];
            let a1 = v_a[(r0 + 1u) * V_WORDS + w];
            let b0 = v_b[c0 * V_WORDS + w];
            let b1 = v_b[(c0 + 1u) * V_WORDS + w];
            s00 = s00 + i32(countOneBits(a0.x & b0.x)) + i32(countOneBits(a0.y & b0.y))
                - i32(countOneBits(a0.x & b0.y)) - i32(countOneBits(a0.y & b0.x));
            s01 = s01 + i32(countOneBits(a0.x & b1.x)) + i32(countOneBits(a0.y & b1.y))
                - i32(countOneBits(a0.x & b1.y)) - i32(countOneBits(a0.y & b1.x));
            s10 = s10 + i32(countOneBits(a1.x & b0.x)) + i32(countOneBits(a1.y & b0.y))
                - i32(countOneBits(a1.x & b0.y)) - i32(countOneBits(a1.y & b0.x));
            s11 = s11 + i32(countOneBits(a1.x & b1.x)) + i32(countOneBits(a1.y & b1.y))
                - i32(countOneBits(a1.x & b1.y)) - i32(countOneBits(a1.y & b1.x));
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

// popcount 吞吐微基准：每轮 8 次 countOneBits + 4 次廉价更新（约 2/3 是 popc），
// 只碰寄存器、不碰存储（除最后一次防优化写）。测吞吐而非延迟：8 条独立累加链。
struct PopcParams {
    iters: u32,
    seed: u32,
};

@group(0) @binding(0) var<storage, read_write> sink: array<u32>;
@group(0) @binding(1) var<uniform> pp: PopcParams;

@compute @workgroup_size(64)
fn bench_popc(@builtin(global_invocation_id) gid: vec3<u32>) {
    var a = pp.seed ^ (gid.x * 747796405u);
    var b = a ^ 0x9E3779B9u;
    var c = a + 0x85EBCA6Bu;
    var d = b ^ 0xC2B2AE35u;
    var acc0 = 0u;
    var acc1 = 0u;
    var acc2 = 0u;
    var acc3 = 0u;
    var acc4 = 0u;
    var acc5 = 0u;
    var acc6 = 0u;
    var acc7 = 0u;
    for (var i = 0u; i < pp.iters; i = i + 1u) {
        acc0 = acc0 + countOneBits(a & b);
        acc1 = acc1 + countOneBits(a | b);
        acc2 = acc2 + countOneBits(a ^ b);
        acc3 = acc3 + countOneBits(~a & b);
        acc4 = acc4 + countOneBits(c & d);
        acc5 = acc5 + countOneBits(c | d);
        acc6 = acc6 + countOneBits(c ^ d);
        acc7 = acc7 + countOneBits(~c & d);
        a = a ^ b;
        b = b + i;
        c = c ^ d;
        d = d + 1u;
    }
    let total = acc0 + acc1 + acc2 + acc3 + acc4 + acc5 + acc6 + acc7;
    if (total == 0xFFFFFFFFu) {
        sink[gid.x % arrayLength(&sink)] = total;
    }
}

// split-K + 3-popc：沿 K 切 4 片并行（workgroup_id.z = 切片号），每片算一部分再 atomicAdd 汇总。
// 结构沿用 main_tiled2（16×16 tile、8×8 线程、2×2 输出），内层用 o3 的 3-popc 恒等式
// （双导轨不交叠 ⇒ popc(aₚ&bₙ)+popc(aₙ&bₚ) = popc((aₚ&bₙ)|(aₙ&bₚ))）。
// 说明：模块级绑定不能重复声明，所以这里复用 a_pos/a_neg/b_pos/b_neg/params，
// 输出走**另一个绑定（binding 6）的 atomic 视图**，指向同一个输出缓冲。
const SLICES: u32 = 4u;

@group(0) @binding(6) var<storage, read_write> out_atomic: array<atomic<i32>>;

var<workgroup> k_ap: array<u32, 144>;
var<workgroup> k_an: array<u32, 144>;
var<workgroup> k_bp: array<u32, 144>;
var<workgroup> k_bn: array<u32, 144>;

@compute @workgroup_size(8, 8)
fn main_splitk(
    @builtin(workgroup_id) wg: vec3<u32>,
    @builtin(local_invocation_id) lid: vec3<u32>,
) {
    let row_base = wg.y * 16u;
    let col_base = wg.x * 16u;
    let r0 = lid.y * 2u;
    let c0 = lid.x * 2u;
    let idx = lid.y * 8u + lid.x;
    let words_per_slice = (params.words + SLICES - 1u) / SLICES;
    let word_start = wg.z * words_per_slice;
    let word_end = min(word_start + words_per_slice, params.words);
    var s00 = 0;
    var s01 = 0;
    var s10 = 0;
    var s11 = 0;
    var first = word_start;
    while (first < word_end) {
        let chunk_end = min(first + 8u, word_end);
        for (var t = 0u; t < 2u; t = t + 1u) {
            let slot = idx + t * 64u;
            let line = slot / 8u;
            let word = first + slot % 8u;
            let ok_a = row_base + line < params.m && word < chunk_end;
            let ok_b = col_base + line < params.n && word < chunk_end;
            k_ap[(slot / 8u) * 9u + slot % 8u] = select(0u, a_pos[(row_base + line) * params.words + word], ok_a);
            k_an[(slot / 8u) * 9u + slot % 8u] = select(0u, a_neg[(row_base + line) * params.words + word], ok_a);
            k_bp[(slot / 8u) * 9u + slot % 8u] = select(0u, b_pos[(col_base + line) * params.words + word], ok_b);
            k_bn[(slot / 8u) * 9u + slot % 8u] = select(0u, b_neg[(col_base + line) * params.words + word], ok_b);
        }
        workgroupBarrier();
        for (var w = 0u; w < 8u; w = w + 1u) {
            if (first + w >= chunk_end) {
                break;
            }
            let a0p = k_ap[r0 * 9u + w];
            let a0n = k_an[r0 * 9u + w];
            let a1p = k_ap[(r0 + 1u) * 9u + w];
            let a1n = k_an[(r0 + 1u) * 9u + w];
            let b0p = k_bp[c0 * 9u + w];
            let b0n = k_bn[c0 * 9u + w];
            let b1p = k_bp[(c0 + 1u) * 9u + w];
            let b1n = k_bn[(c0 + 1u) * 9u + w];
            s00 = s00 + i32(countOneBits(a0p & b0p)) + i32(countOneBits(a0n & b0n)) - i32(countOneBits((a0p & b0n) | (a0n & b0p)));
            s01 = s01 + i32(countOneBits(a0p & b1p)) + i32(countOneBits(a0n & b1n)) - i32(countOneBits((a0p & b1n) | (a0n & b1p)));
            s10 = s10 + i32(countOneBits(a1p & b0p)) + i32(countOneBits(a1n & b0n)) - i32(countOneBits((a1p & b0n) | (a1n & b0p)));
            s11 = s11 + i32(countOneBits(a1p & b1p)) + i32(countOneBits(a1n & b1n)) - i32(countOneBits((a1p & b1n) | (a1n & b1p)));
        }
        workgroupBarrier();
        first = chunk_end;
    }
    let row0 = row_base + r0;
    let col0 = col_base + c0;
    if (row0 < params.m && col0 < params.n) {
        atomicAdd(&out_atomic[row0 * params.n + col0], s00);
    }
    if (row0 < params.m && col0 + 1u < params.n) {
        atomicAdd(&out_atomic[row0 * params.n + col0 + 1u], s01);
    }
    if (row0 + 1u < params.m && col0 < params.n) {
        atomicAdd(&out_atomic[(row0 + 1u) * params.n + col0], s10);
    }
    if (row0 + 1u < params.m && col0 + 1u < params.n) {
        atomicAdd(&out_atomic[(row0 + 1u) * params.n + col0 + 1u], s11);
    }
}

// K 段加大到 16 word：与 main_tiled2 完全同结构（16×16 tile、8×8 线程、2×2 输出），
// 只是每段的 word 数 8→16 ⇒ 屏障次数减半（128 word 从 32 次降到 16 次）。
// 共享内存 4 张平面 × 256 槽 = 4 KB（16 行 × 16 word）。
const W16_WORDS: u32 = 16u;

var<workgroup> w16_ap: array<u32, 256>;
var<workgroup> w16_an: array<u32, 256>;
var<workgroup> w16_bp: array<u32, 256>;
var<workgroup> w16_bn: array<u32, 256>;

@compute @workgroup_size(8, 8)
fn main_tiled2w16(
    @builtin(workgroup_id) wg: vec3<u32>,
    @builtin(local_invocation_id) lid: vec3<u32>,
) {
    let row_base = wg.y * 16u;
    let col_base = wg.x * 16u;
    let r0 = lid.y * 2u;
    let c0 = lid.x * 2u;
    let idx = lid.y * 8u + lid.x;
    var s00 = 0;
    var s01 = 0;
    var s10 = 0;
    var s11 = 0;
    let word_tiles = (params.words + W16_WORDS - 1u) / W16_WORDS;
    for (var chunk = 0u; chunk < word_tiles; chunk = chunk + 1u) {
        // 64 线程 × 4 槽 = 256 = 16 行/列 × 16 word。
        for (var t = 0u; t < 4u; t = t + 1u) {
            let slot = idx + t * 64u;
            let line = slot / W16_WORDS;
            let word = chunk * W16_WORDS + slot % W16_WORDS;
            let ok_a = row_base + line < params.m && word < params.words;
            let ok_b = col_base + line < params.n && word < params.words;
            w16_ap[slot] = select(0u, a_pos[(row_base + line) * params.words + word], ok_a);
            w16_an[slot] = select(0u, a_neg[(row_base + line) * params.words + word], ok_a);
            w16_bp[slot] = select(0u, b_pos[(col_base + line) * params.words + word], ok_b);
            w16_bn[slot] = select(0u, b_neg[(col_base + line) * params.words + word], ok_b);
        }
        workgroupBarrier();
        for (var w = 0u; w < W16_WORDS; w = w + 1u) {
            let a0p = w16_ap[r0 * W16_WORDS + w];
            let a0n = w16_an[r0 * W16_WORDS + w];
            let a1p = w16_ap[(r0 + 1u) * W16_WORDS + w];
            let a1n = w16_an[(r0 + 1u) * W16_WORDS + w];
            let b0p = w16_bp[c0 * W16_WORDS + w];
            let b0n = w16_bn[c0 * W16_WORDS + w];
            let b1p = w16_bp[(c0 + 1u) * W16_WORDS + w];
            let b1n = w16_bn[(c0 + 1u) * W16_WORDS + w];
            s00 = s00 + i32(countOneBits(a0p & b0p)) + i32(countOneBits(a0n & b0n)) - i32(countOneBits(a0p & b0n)) - i32(countOneBits(a0n & b0p));
            s01 = s01 + i32(countOneBits(a0p & b1p)) + i32(countOneBits(a0n & b1n)) - i32(countOneBits(a0p & b1n)) - i32(countOneBits(a0n & b1p));
            s10 = s10 + i32(countOneBits(a1p & b0p)) + i32(countOneBits(a1n & b0n)) - i32(countOneBits(a1p & b0n)) - i32(countOneBits(a1n & b0p));
            s11 = s11 + i32(countOneBits(a1p & b1p)) + i32(countOneBits(a1n & b1n)) - i32(countOneBits(a1p & b1n)) - i32(countOneBits(a1n & b1p));
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
var<workgroup> o3_ap: array<u32, 144>;
var<workgroup> o3_an: array<u32, 144>;
var<workgroup> o3_bp: array<u32, 144>;
var<workgroup> o3_bn: array<u32, 144>;
// 3-popc：与 main_tiled2 完全同结构，把 4 次 popc 压到 3 次。
//
//   popc(aₚ&bₚ) + popc(aₙ&bₙ) − popc(aₚ&bₙ) − popc(aₙ&bₚ)
// = popc(aₚ&bₚ) + popc(aₙ&bₙ) − popc((aₚ&bₙ) | (aₙ&bₚ))
//
// 等号靠双导轨的**不交叠不变量**：`aₚ&aₙ = 0`、`bₚ&bₙ = 0`（每个 trit 只落一根轨），
// 所以 `(aₚ&bₙ)&(aₙ&bₚ) = 0`，两次 popc 可并成一次 OR + popc。
// pack_rows / pack_columns 保证该不变量；测试用 4-popc 内核与 CPU 镜像逐元素对拍。
@compute @workgroup_size(8, 8)
fn main_tiled2o3(
    @builtin(workgroup_id) wg: vec3<u32>,
    @builtin(local_invocation_id) lid: vec3<u32>,
) {
    let row_base = wg.y * 16u;
    let col_base = wg.x * 16u;
    let r0 = lid.y * 2u;
    let c0 = lid.x * 2u;
    let idx = lid.y * 8u + lid.x;
    var s00 = 0;
    var s01 = 0;
    var s10 = 0;
    var s11 = 0;
    let word_tiles = (params.words + 8u - 1u) / 8u;
    for (var chunk = 0u; chunk < word_tiles; chunk = chunk + 1u) {
        for (var t = 0u; t < 2u; t = t + 1u) {
            let slot = idx + t * 64u;
            let line = slot / 8u;
            let word = chunk * 8u + slot % 8u;
            let ok_a = row_base + line < params.m && word < params.words;
            let ok_b = col_base + line < params.n && word < params.words;
            o3_ap[(slot / 8u) * 9u + slot % 8u] = select(0u, a_pos[(row_base + line) * params.words + word], ok_a);
            o3_an[(slot / 8u) * 9u + slot % 8u] = select(0u, a_neg[(row_base + line) * params.words + word], ok_a);
            o3_bp[(slot / 8u) * 9u + slot % 8u] = select(0u, b_pos[(col_base + line) * params.words + word], ok_b);
            o3_bn[(slot / 8u) * 9u + slot % 8u] = select(0u, b_neg[(col_base + line) * params.words + word], ok_b);
        }
        workgroupBarrier();
        for (var w = 0u; w < 8u; w = w + 1u) {
            let a0p = o3_ap[r0 * 9u + w];
            let a0n = o3_an[r0 * 9u + w];
            let a1p = o3_ap[(r0 + 1u) * 9u + w];
            let a1n = o3_an[(r0 + 1u) * 9u + w];
            let b0p = o3_bp[c0 * 9u + w];
            let b0n = o3_bn[c0 * 9u + w];
            let b1p = o3_bp[(c0 + 1u) * 9u + w];
            let b1n = o3_bn[(c0 + 1u) * 9u + w];
            s00 = s00 + i32(countOneBits(a0p & b0p)) + i32(countOneBits(a0n & b0n)) - i32(countOneBits((a0p & b0n) | (a0n & b0p)));
            s01 = s01 + i32(countOneBits(a0p & b1p)) + i32(countOneBits(a0n & b1n)) - i32(countOneBits((a0p & b1n) | (a0n & b1p)));
            s10 = s10 + i32(countOneBits(a1p & b0p)) + i32(countOneBits(a1n & b0n)) - i32(countOneBits((a1p & b0n) | (a1n & b0p)));
            s11 = s11 + i32(countOneBits(a1p & b1p)) + i32(countOneBits(a1n & b1n)) - i32(countOneBits((a1p & b1n) | (a1n & b1p)));
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
