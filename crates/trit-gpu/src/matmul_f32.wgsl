// f32 基线矩阵乘：与 matmul.wgsl 完全相同的 naive 结构
// （一线程一输出、循环内积维度），唯一变量是数据表示（32 bit 浮点 vs 2 bit/trit）。
// fb 同样按转置行主序存放。

struct Params {
    m: u32,
    n: u32,
    k: u32,
};

@group(0) @binding(0) var<storage, read> fa: array<f32>;
@group(0) @binding(1) var<storage, read> fb: array<f32>;
@group(0) @binding(2) var<storage, read_write> out: array<f32>;
@group(0) @binding(3) var<uniform> params: Params;

@compute @workgroup_size(64)
fn main(@builtin(global_invocation_id) gid: vec3<u32>) {
    let index = gid.x;
    if (index >= params.m * params.n) {
        return;
    }
    let row = index / params.n;
    let col = index % params.n;
    let a_base = row * params.k;
    let b_base = col * params.k;
    var sum = 0.0;
    for (var kk = 0u; kk < params.k; kk = kk + 1u) {
        sum = sum + fa[a_base + kk] * fb[b_base + kk];
    }
    out[index] = sum;
}

// 分块版（经典共享内存 tiling）：16×16 输出 tile，A/B 的 16×16 子块协作加载进
// workgroup storage，内积维按 tile 步进——全局读流量砍 TILE 倍。
const TILE: u32 = 16u;

var<workgroup> tile_a: array<f32, 256>;
var<workgroup> tile_b: array<f32, 256>;

@compute @workgroup_size(16, 16)
fn main_tiled(
    @builtin(global_invocation_id) gid: vec3<u32>,
    @builtin(local_invocation_id) lid: vec3<u32>,
) {
    let row = gid.y;
    let col = gid.x;
    let linear = lid.y * TILE + lid.x;
    var sum = 0.0;
    let k_tiles = (params.k + TILE - 1u) / TILE;
    for (var chunk = 0u; chunk < k_tiles; chunk = chunk + 1u) {
        let a_col = chunk * TILE + lid.x;
        tile_a[linear] = select(0.0, fa[row * params.k + a_col], row < params.m && a_col < params.k);
        // B^T 行主序：fb[col * k + kk]。
        let b_row = chunk * TILE + lid.y;
        tile_b[linear] = select(0.0, fb[col * params.k + b_row], col < params.n && b_row < params.k);
        workgroupBarrier();
        for (var kk = 0u; kk < TILE; kk = kk + 1u) {
            sum = sum + tile_a[lid.y * TILE + kk] * tile_b[kk * TILE + lid.x];
        }
        workgroupBarrier();
    }
    if (row < params.m && col < params.n) {
        out[row * params.n + col] = sum;
    }
}

// 寄存器阻塞版：workgroup 8×8，每线程 2×2 四个 FMA 累加器——
// tile_a/tile_b 每次读取喂 4 个乘加，算术强度再翻倍。tile 仍 16×16。
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
    var sum00 = 0.0;
    var sum01 = 0.0;
    var sum10 = 0.0;
    var sum11 = 0.0;
    let k_tiles = (params.k + TILE - 1u) / TILE;
    for (var chunk = 0u; chunk < k_tiles; chunk = chunk + 1u) {
        // 64 线程各装 4 槽：tile_a[r][kk] / tile_b[c][kk]，r、c 覆盖 0..16。
        for (var i = 0u; i < 4u; i = i + 1u) {
            let s = idx + i * 64u;
            let r = s / 16u;
            let kk = chunk * TILE + s % 16u;
            let a_ok = row_base + r < params.m && kk < params.k;
            let b_ok = col_base + r < params.n && kk < params.k;
            tile_a[s] = select(0.0, fa[(row_base + r) * params.k + kk], a_ok);
            // B^T 行主序：tile_b[c][kk] = fb[(col_base + c) * k + kk]。
            tile_b[s] = select(0.0, fb[(col_base + r) * params.k + kk], b_ok);
        }
        workgroupBarrier();
        for (var kk = 0u; kk < TILE; kk = kk + 1u) {
            let a0 = tile_a[r0 * TILE + kk];
            let a1 = tile_a[(r0 + 1u) * TILE + kk];
            let b0 = tile_b[c0 * TILE + kk];
            let b1 = tile_b[(c0 + 1u) * TILE + kk];
            sum00 = sum00 + a0 * b0;
            sum01 = sum01 + a0 * b1;
            sum10 = sum10 + a1 * b0;
            sum11 = sum11 + a1 * b1;
        }
        workgroupBarrier();
    }
    let row0 = row_base + r0;
    let col0 = col_base + c0;
    if (row0 < params.m && col0 < params.n) {
        out[row0 * params.n + col0] = sum00;
    }
    if (row0 < params.m && col0 + 1u < params.n) {
        out[row0 * params.n + col0 + 1u] = sum01;
    }
    if (row0 + 1u < params.m && col0 < params.n) {
        out[(row0 + 1u) * params.n + col0] = sum10;
    }
    if (row0 + 1u < params.m && col0 + 1u < params.n) {
        out[(row0 + 1u) * params.n + col0 + 1u] = sum11;
    }
}

// 4×4 寄存器阻塞（与 matmul.wgsl 的 main_tiled4 同结构）：32×32 tile、8×8 线程、
// 每线程 4×4 输出，K 方向每次搬 8 个元素。
const T4: u32 = 32u;
const K_CHUNK: u32 = 8u;

var<workgroup> t4_a: array<f32, 256>;
var<workgroup> t4_b: array<f32, 256>;

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
    var s00 = 0.0; var s01 = 0.0; var s02 = 0.0; var s03 = 0.0;
    var s10 = 0.0; var s11 = 0.0; var s12 = 0.0; var s13 = 0.0;
    var s20 = 0.0; var s21 = 0.0; var s22 = 0.0; var s23 = 0.0;
    var s30 = 0.0; var s31 = 0.0; var s32 = 0.0; var s33 = 0.0;
    let k_tiles = (params.k + K_CHUNK - 1u) / K_CHUNK;
    for (var chunk = 0u; chunk < k_tiles; chunk = chunk + 1u) {
        for (var t = 0u; t < 4u; t = t + 1u) {
            let slot = idx + t * 64u;
            let line = slot / K_CHUNK;
            let kk = chunk * K_CHUNK + slot % K_CHUNK;
            let ok_a = row_base + line < params.m && kk < params.k;
            let ok_b = col_base + line < params.n && kk < params.k;
            t4_a[slot] = select(0.0, fa[(row_base + line) * params.k + kk], ok_a);
            t4_b[slot] = select(0.0, fb[(col_base + line) * params.k + kk], ok_b);
        }
        workgroupBarrier();
        for (var kk = 0u; kk < K_CHUNK; kk = kk + 1u) {
            let a0 = t4_a[(r0 + 0u) * K_CHUNK + kk];
            let a1 = t4_a[(r0 + 1u) * K_CHUNK + kk];
            let a2 = t4_a[(r0 + 2u) * K_CHUNK + kk];
            let a3 = t4_a[(r0 + 3u) * K_CHUNK + kk];
            let b0 = t4_b[(c0 + 0u) * K_CHUNK + kk];
            let b1 = t4_b[(c0 + 1u) * K_CHUNK + kk];
            let b2 = t4_b[(c0 + 2u) * K_CHUNK + kk];
            let b3 = t4_b[(c0 + 3u) * K_CHUNK + kk];
            s00 = s00 + a0 * b0; s01 = s01 + a0 * b1; s02 = s02 + a0 * b2; s03 = s03 + a0 * b3;
            s10 = s10 + a1 * b0; s11 = s11 + a1 * b1; s12 = s12 + a1 * b2; s13 = s13 + a1 * b3;
            s20 = s20 + a2 * b0; s21 = s21 + a2 * b1; s22 = s22 + a2 * b2; s23 = s23 + a2 * b3;
            s30 = s30 + a3 * b0; s31 = s31 + a3 * b1; s32 = s32 + a3 * b2; s33 = s33 + a3 * b3;
        }
        workgroupBarrier();
    }
    let row0 = row_base + r0;
    let col0 = col_base + c0;
    let acc = array<f32, 16>(s00, s01, s02, s03, s10, s11, s12, s13, s20, s21, s22, s23, s30, s31, s32, s33);
    for (var i = 0u; i < 4u; i = i + 1u) {
        for (var j = 0u; j < 4u; j = j + 1u) {
            if (row0 + i < params.m && col0 + j < params.n) {
                out[(row0 + i) * params.n + col0 + j] = acc[i * 4u + j];
            }
        }
    }
}

// 2D 版 naive（对齐 NVIDIA cuda-oxide 例子的 16×16 线程块映射）：
// 与 main 数学完全一致，只有线程→输出的映射不同（线性 1D vs 2D block）。
@compute @workgroup_size(16, 16)
fn main2d(@builtin(global_invocation_id) gid: vec3<u32>) {
    let row = gid.y;
    let col = gid.x;
    if (row >= params.m || col >= params.n) {
        return;
    }
    let a_base = row * params.k;
    let b_base = col * params.k;
    var sum = 0.0;
    for (var kk = 0u; kk < params.k; kk = kk + 1u) {
        sum = sum + fa[a_base + kk] * fb[b_base + kk];
    }
    out[row * params.n + col] = sum;
}

// 2×2 寄存器阻塞 + K 段 8（对齐 cuda-oxide 的 reg2 结构；我们的 main_tiled2 用的是 K 段 16）。
const K8: u32 = 8u;
var<workgroup> t2k8_a: array<f32, 128>;
var<workgroup> t2k8_b: array<f32, 128>;

@compute @workgroup_size(8, 8)
fn main_tiled2k8(
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
    for (var chunk = 0u; chunk < k_tiles; chunk = chunk + 1u) {
        // 64 线程 × 2 槽 = 128 = 16 行/列 × 8 元素。
        for (var i = 0u; i < 2u; i = i + 1u) {
            let s = idx + i * 64u;
            let r = s / K8;
            let kk = chunk * K8 + s % K8;
            let a_ok = row_base + r < params.m && kk < params.k;
            let b_ok = col_base + r < params.n && kk < params.k;
            t2k8_a[s] = select(0.0, fa[(row_base + r) * params.k + kk], a_ok);
            t2k8_b[s] = select(0.0, fb[(col_base + r) * params.k + kk], b_ok);
        }
        workgroupBarrier();
        for (var kk = 0u; kk < K8; kk = kk + 1u) {
            let a0 = t2k8_a[r0 * K8 + kk];
            let a1 = t2k8_a[(r0 + 1u) * K8 + kk];
            let b0 = t2k8_b[c0 * K8 + kk];
            let b1 = t2k8_b[(c0 + 1u) * K8 + kk];
            s00 = s00 + a0 * b0;
            s01 = s01 + a0 * b1;
            s10 = s10 + a1 * b0;
            s11 = s11 + a1 * b1;
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

// 4×4 寄存器阻塞 + K 段 16：32×32 tile、8×8 线程，每段 16 元素（共享 2×2KB）。
// 与 main_tiled4 同结构，只是把 K 段从 8 加到 16（屏障减半、每次装载复用更多）。
const K16: u32 = 16u;
var<workgroup> t4k16_a: array<f32, 512>;
var<workgroup> t4k16_b: array<f32, 512>;

@compute @workgroup_size(8, 8)
fn main_tiled4k16(
    @builtin(workgroup_id) wg: vec3<u32>,
    @builtin(local_invocation_id) lid: vec3<u32>,
) {
    let row_base = wg.y * 32u;
    let col_base = wg.x * 32u;
    let r0 = lid.y * 4u;
    let c0 = lid.x * 4u;
    let idx = lid.y * 8u + lid.x;
    var s00 = 0.0; var s01 = 0.0; var s02 = 0.0; var s03 = 0.0;
    var s10 = 0.0; var s11 = 0.0; var s12 = 0.0; var s13 = 0.0;
    var s20 = 0.0; var s21 = 0.0; var s22 = 0.0; var s23 = 0.0;
    var s30 = 0.0; var s31 = 0.0; var s32 = 0.0; var s33 = 0.0;
    let k_tiles = (params.k + K16 - 1u) / K16;
    for (var chunk = 0u; chunk < k_tiles; chunk = chunk + 1u) {
        // 64 线程 × 8 槽 = 512 = 32 行/列 × 16 元素。
        for (var t = 0u; t < 8u; t = t + 1u) {
            let slot = idx + t * 64u;
            let line = slot / K16;
            let kk = chunk * K16 + slot % K16;
            let ok_a = row_base + line < params.m && kk < params.k;
            let ok_b = col_base + line < params.n && kk < params.k;
            t4k16_a[slot] = select(0.0, fa[(row_base + line) * params.k + kk], ok_a);
            t4k16_b[slot] = select(0.0, fb[(col_base + line) * params.k + kk], ok_b);
        }
        workgroupBarrier();
        for (var kk = 0u; kk < K16; kk = kk + 1u) {
            let a0 = t4k16_a[(r0 + 0u) * K16 + kk];
            let a1 = t4k16_a[(r0 + 1u) * K16 + kk];
            let a2 = t4k16_a[(r0 + 2u) * K16 + kk];
            let a3 = t4k16_a[(r0 + 3u) * K16 + kk];
            let b0 = t4k16_b[(c0 + 0u) * K16 + kk];
            let b1 = t4k16_b[(c0 + 1u) * K16 + kk];
            let b2 = t4k16_b[(c0 + 2u) * K16 + kk];
            let b3 = t4k16_b[(c0 + 3u) * K16 + kk];
            s00 = s00 + a0 * b0; s01 = s01 + a0 * b1; s02 = s02 + a0 * b2; s03 = s03 + a0 * b3;
            s10 = s10 + a1 * b0; s11 = s11 + a1 * b1; s12 = s12 + a1 * b2; s13 = s13 + a1 * b3;
            s20 = s20 + a2 * b0; s21 = s21 + a2 * b1; s22 = s22 + a2 * b2; s23 = s23 + a2 * b3;
            s30 = s30 + a3 * b0; s31 = s31 + a3 * b1; s32 = s32 + a3 * b2; s33 = s33 + a3 * b3;
        }
        workgroupBarrier();
    }
    let row0 = row_base + r0;
    let col0 = col_base + c0;
    let acc = array<f32, 16>(s00, s01, s02, s03, s10, s11, s12, s13, s20, s21, s22, s23, s30, s31, s32, s33);
    for (var i = 0u; i < 4u; i = i + 1u) {
        for (var j = 0u; j < 4u; j = j + 1u) {
            if (row0 + i < params.m && col0 + j < params.n) {
                out[(row0 + i) * params.n + col0 + j] = acc[i * 4u + j];
            }
        }
    }
}

// 2×2 寄存器阻塞 + K 段 16：16×16 tile，共享 16×16（256 项/操作数）。
var<workgroup> t2k16_a: array<f32, 256>;
var<workgroup> t2k16_b: array<f32, 256>;

@compute @workgroup_size(8, 8)
fn main_tiled2k16(
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
    let k_tiles = (params.k + K16 - 1u) / K16;
    for (var chunk = 0u; chunk < k_tiles; chunk = chunk + 1u) {
        // 64 线程 × 4 槽 = 256 = 16 行/列 × 16 元素。
        for (var t = 0u; t < 4u; t = t + 1u) {
            let slot = idx + t * 64u;
            let line = slot / K16;
            let kk = chunk * K16 + slot % K16;
            let ok_a = row_base + line < params.m && kk < params.k;
            let ok_b = col_base + line < params.n && kk < params.k;
            t2k16_a[slot] = select(0.0, fa[(row_base + line) * params.k + kk], ok_a);
            t2k16_b[slot] = select(0.0, fb[(col_base + line) * params.k + kk], ok_b);
        }
        workgroupBarrier();
        for (var w = 0u; w < K16; w = w + 1u) {
            let a0 = t2k16_a[r0 * K16 + w];
            let a1 = t2k16_a[(r0 + 1u) * K16 + w];
            let b0 = t2k16_b[c0 * K16 + w];
            let b1 = t2k16_b[(c0 + 1u) * K16 + w];
            s00 = s00 + a0 * b0;
            s01 = s01 + a0 * b1;
            s10 = s10 + a1 * b0;
            s11 = s11 + a1 * b1;
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