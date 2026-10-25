// f32 基线矩阵乘：与 matmul.wgsl 对应的几档同结构内核（2D naive / tiled /
// 2×2 + K8 / 4×4 + K8 / 4×4 + K32），唯一变量是数据表示（32 bit 浮点 vs 2 bit/trit）。
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
// 同一缓冲的 vec4 视图：一次 128 位装载替 4 次 32 位（仅装载用）。
@group(0) @binding(4) var<storage, read> fa4: array<vec4<f32>>;
@group(0) @binding(5) var<storage, read> fb4: array<vec4<f32>>;

// 分块版（经典共享内存 tiling）：16×16 输出 tile。A、B 的 16×16 子块**都按 [行][k]**
// 连续存放（B 的行就是输出列），于是内层一次 128 位共享读喂 4 个乘加——
// 32 次标量共享读变成 8 次 vec4 读。
//
// 每行固定 8 个 vec4 槽（K 段只用前 4 个），槽位按 `part ^ (row & 7)` 打散：
// 一行 128 B 正好覆盖全部 32 个 bank，不打散的话同一 warp 的 16 行会全落在同一组 bank 上。
// A 侧同一 warp 只有 2 行（其余是广播），不需要打散。
const TILE: u32 = 16u;

var<workgroup> tiled_a4: array<vec4<f32>, 128>;
var<workgroup> tiled_b4: array<vec4<f32>, 128>;
var<workgroup> tail_a: array<f32, 256>;
var<workgroup> tail_b: array<f32, 256>;

@compute @workgroup_size(16, 16)
fn main_tiled(
    @builtin(global_invocation_id) gid: vec3<u32>,
    @builtin(local_invocation_id) lid: vec3<u32>,
) {
    let row = gid.y;
    let col = gid.x;
    let tile_id = lid.y * TILE + lid.x;
    let row_ok = row < params.m;
    let col_ok = col < params.n;
    let block_row = row - lid.y;
    let block_col = col - lid.x;
    var sum = 0.0;
    let full = params.k / TILE;
    let k4 = params.k / 4u;
    // k 不是 4 的倍数时 vec4 的行步长对不上，退标量装载（仍拼成 vec4 落共享内存）。
    let vector_ok = params.k % 4u == 0u;
    for (var chunk = 0u; chunk < full; chunk = chunk + 1u) {
        if (tile_id < 64u) {
            let r = tile_id / 4u;
            let part = tile_id % 4u;
            if vector_ok {
                let seg_ok = (chunk * 4u + part) * 4u < params.k;
                tiled_a4[r * 8u + part] = select(vec4<f32>(0.0), fa4[(block_row + r) * k4 + chunk * 4u + part], block_row + r < params.m && seg_ok);
                tiled_b4[r * 8u + (part ^ (r & 7u))] = select(vec4<f32>(0.0), fb4[(block_col + r) * k4 + chunk * 4u + part], block_col + r < params.n && seg_ok);
            } else {
                let k0 = chunk * TILE + part * 4u;
                let a_ok = block_row + r < params.m;
                let b_ok = block_col + r < params.n;
                tiled_a4[r * 8u + part] = vec4<f32>(
                    select(0.0, fa[(block_row + r) * params.k + k0 + 0u], a_ok && k0 + 0u < params.k),
                    select(0.0, fa[(block_row + r) * params.k + k0 + 1u], a_ok && k0 + 1u < params.k),
                    select(0.0, fa[(block_row + r) * params.k + k0 + 2u], a_ok && k0 + 2u < params.k),
                    select(0.0, fa[(block_row + r) * params.k + k0 + 3u], a_ok && k0 + 3u < params.k),
                );
                tiled_b4[r * 8u + (part ^ (r & 7u))] = vec4<f32>(
                    select(0.0, fb[(block_col + r) * params.k + k0 + 0u], b_ok && k0 + 0u < params.k),
                    select(0.0, fb[(block_col + r) * params.k + k0 + 1u], b_ok && k0 + 1u < params.k),
                    select(0.0, fb[(block_col + r) * params.k + k0 + 2u], b_ok && k0 + 2u < params.k),
                    select(0.0, fb[(block_col + r) * params.k + k0 + 3u], b_ok && k0 + 3u < params.k),
                );
            }
        }
        workgroupBarrier();
        for (var w4 = 0u; w4 < 4u; w4 = w4 + 1u) {
            let a0 = tiled_a4[lid.y * 8u + w4];
            let b0 = tiled_b4[lid.x * 8u + (w4 ^ (lid.x & 7u))];
            sum = sum + a0.x * b0.x;
            sum = sum + a0.y * b0.y;
            sum = sum + a0.z * b0.z;
            sum = sum + a0.w * b0.w;
        }
        workgroupBarrier();
    }
    // 尾段（k 不是 16 的倍数）：走标量共享区，布局同样是 [行][k]。
    if (params.k % TILE != 0u) {
        let a_k = full * TILE + lid.x;
        let b_k = full * TILE + lid.y;
        if row_ok && a_k < params.k {
            tail_a[tile_id] = fa[row * params.k + a_k];
        } else {
            tail_a[tile_id] = 0.0;
        }
        if col_ok && b_k < params.k {
            tail_b[lid.x * TILE + lid.y] = fb[col * params.k + b_k];
        } else {
            tail_b[lid.x * TILE + lid.y] = 0.0;
        }
        workgroupBarrier();
        for (var kk = 0u; kk < TILE; kk = kk + 1u) {
            if (full * TILE + kk < params.k) {
                sum = sum + tail_a[lid.y * TILE + kk] * tail_b[lid.x * TILE + kk];
            }
        }
        workgroupBarrier();
    }
    if (row_ok && col_ok) {
        out[row * params.n + col] = sum;
    }
}

// 4×4 寄存器阻塞（与 matmul.wgsl 的 main_tiled4 同结构）：32×32 tile、8×8 线程、
// 每线程 4×4 输出，K 方向每次搬 8 个元素。共享内存按 vec4 组织：
// 内层每次 128 位读喂 4 个乘加，共享读次数是标量版的 1/4。
// 每行 8 个 vec4 槽（K8 只用 2 个），槽位 `part ^ ((row >> 2) & 7)`：同一 warp 内
// 行号相距 4，用 row>>2 做 key 才能让 8 个 bank 组都被用到。
const T4: u32 = 32u;
const K_CHUNK: u32 = 8u;

var<workgroup> t4_a4: array<vec4<f32>, 256>;
var<workgroup> t4_b4: array<vec4<f32>, 256>;

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
    var s00 = 0.0; 
    var s01 = 0.0; 
    var s02 = 0.0; 
    var s03 = 0.0;  
    var s10 = 0.0; 
    var s11 = 0.0; 
    var s12 = 0.0; 
    var s13 = 0.0;  
    var s20 = 0.0; 
    var s21 = 0.0; 
    var s22 = 0.0; 
    var s23 = 0.0;  
    var s30 = 0.0; 
    var s31 = 0.0; 
    var s32 = 0.0; 
    var s33 = 0.0;  
    let k_tiles = (params.k + K_CHUNK - 1u) / K_CHUNK;
    let vector_ok = params.k % 4u == 0u;
    for (var chunk = 0u; chunk < k_tiles; chunk = chunk + 1u) {
        if vector_ok {
            // 64 个 vec4 槽（32 行 × 2 段），每线程正好一个。
            let r = idx / 2u;
            let part = idx % 2u;
            let k4 = params.k / 4u;
            let seg_ok = (chunk * 2u + part) * 4u < params.k;
            t4_a4[r * 8u + (part ^ ((r >> 2u) & 7u))] = select(vec4<f32>(0.0), fa4[(row_base + r) * k4 + chunk * 2u + part], row_base + r < params.m && seg_ok);
            t4_b4[r * 8u + (part ^ ((r >> 2u) & 7u))] = select(vec4<f32>(0.0), fb4[(col_base + r) * k4 + chunk * 2u + part], col_base + r < params.n && seg_ok);
        } else {
            let r = idx / 2u;
            let part = idx % 2u;
            let k0 = chunk * K_CHUNK + part * 4u;
            let a_ok = row_base + r < params.m;
            let b_ok = col_base + r < params.n;
            t4_a4[r * 8u + (part ^ ((r >> 2u) & 7u))] = vec4<f32>(
                select(0.0, fa[(row_base + r) * params.k + k0 + 0u], a_ok && k0 + 0u < params.k),
                select(0.0, fa[(row_base + r) * params.k + k0 + 1u], a_ok && k0 + 1u < params.k),
                select(0.0, fa[(row_base + r) * params.k + k0 + 2u], a_ok && k0 + 2u < params.k),
                select(0.0, fa[(row_base + r) * params.k + k0 + 3u], a_ok && k0 + 3u < params.k),
            );
            t4_b4[r * 8u + (part ^ ((r >> 2u) & 7u))] = vec4<f32>(
                select(0.0, fb[(col_base + r) * params.k + k0 + 0u], b_ok && k0 + 0u < params.k),
                select(0.0, fb[(col_base + r) * params.k + k0 + 1u], b_ok && k0 + 1u < params.k),
                select(0.0, fb[(col_base + r) * params.k + k0 + 2u], b_ok && k0 + 2u < params.k),
                select(0.0, fb[(col_base + r) * params.k + k0 + 3u], b_ok && k0 + 3u < params.k),
            );
        }
        workgroupBarrier();
        for (var w4 = 0u; w4 < 2u; w4 = w4 + 1u) {
            let a0 = t4_a4[(r0 + 0u) * 8u + (w4 ^ (((r0 + 0u) >> 2u) & 7u))];
            let a1 = t4_a4[(r0 + 1u) * 8u + (w4 ^ (((r0 + 1u) >> 2u) & 7u))];
            let a2 = t4_a4[(r0 + 2u) * 8u + (w4 ^ (((r0 + 2u) >> 2u) & 7u))];
            let a3 = t4_a4[(r0 + 3u) * 8u + (w4 ^ (((r0 + 3u) >> 2u) & 7u))];
            let b0 = t4_b4[(c0 + 0u) * 8u + (w4 ^ (((c0 + 0u) >> 2u) & 7u))];
            let b1 = t4_b4[(c0 + 1u) * 8u + (w4 ^ (((c0 + 1u) >> 2u) & 7u))];
            let b2 = t4_b4[(c0 + 2u) * 8u + (w4 ^ (((c0 + 2u) >> 2u) & 7u))];
            let b3 = t4_b4[(c0 + 3u) * 8u + (w4 ^ (((c0 + 3u) >> 2u) & 7u))];
            s00 = s00 + a0.x * b0.x;
            s00 = s00 + a0.y * b0.y;
            s00 = s00 + a0.z * b0.z;
            s00 = s00 + a0.w * b0.w;
            s01 = s01 + a0.x * b1.x;
            s01 = s01 + a0.y * b1.y;
            s01 = s01 + a0.z * b1.z;
            s01 = s01 + a0.w * b1.w;
            s02 = s02 + a0.x * b2.x;
            s02 = s02 + a0.y * b2.y;
            s02 = s02 + a0.z * b2.z;
            s02 = s02 + a0.w * b2.w;
            s03 = s03 + a0.x * b3.x;
            s03 = s03 + a0.y * b3.y;
            s03 = s03 + a0.z * b3.z;
            s03 = s03 + a0.w * b3.w;
            s10 = s10 + a1.x * b0.x;
            s10 = s10 + a1.y * b0.y;
            s10 = s10 + a1.z * b0.z;
            s10 = s10 + a1.w * b0.w;
            s11 = s11 + a1.x * b1.x;
            s11 = s11 + a1.y * b1.y;
            s11 = s11 + a1.z * b1.z;
            s11 = s11 + a1.w * b1.w;
            s12 = s12 + a1.x * b2.x;
            s12 = s12 + a1.y * b2.y;
            s12 = s12 + a1.z * b2.z;
            s12 = s12 + a1.w * b2.w;
            s13 = s13 + a1.x * b3.x;
            s13 = s13 + a1.y * b3.y;
            s13 = s13 + a1.z * b3.z;
            s13 = s13 + a1.w * b3.w;
            s20 = s20 + a2.x * b0.x;
            s20 = s20 + a2.y * b0.y;
            s20 = s20 + a2.z * b0.z;
            s20 = s20 + a2.w * b0.w;
            s21 = s21 + a2.x * b1.x;
            s21 = s21 + a2.y * b1.y;
            s21 = s21 + a2.z * b1.z;
            s21 = s21 + a2.w * b1.w;
            s22 = s22 + a2.x * b2.x;
            s22 = s22 + a2.y * b2.y;
            s22 = s22 + a2.z * b2.z;
            s22 = s22 + a2.w * b2.w;
            s23 = s23 + a2.x * b3.x;
            s23 = s23 + a2.y * b3.y;
            s23 = s23 + a2.z * b3.z;
            s23 = s23 + a2.w * b3.w;
            s30 = s30 + a3.x * b0.x;
            s30 = s30 + a3.y * b0.y;
            s30 = s30 + a3.z * b0.z;
            s30 = s30 + a3.w * b0.w;
            s31 = s31 + a3.x * b1.x;
            s31 = s31 + a3.y * b1.y;
            s31 = s31 + a3.z * b1.z;
            s31 = s31 + a3.w * b1.w;
            s32 = s32 + a3.x * b2.x;
            s32 = s32 + a3.y * b2.y;
            s32 = s32 + a3.z * b2.z;
            s32 = s32 + a3.w * b2.w;
            s33 = s33 + a3.x * b3.x;
            s33 = s33 + a3.y * b3.y;
            s33 = s33 + a3.z * b3.z;
            s33 = s33 + a3.w * b3.w;
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
    // 每线程沿 K 连读：一次 128 位装载替 4 次 32 位（累加次序不变）。
    if (params.k % 4u == 0u) {
        let a4 = a_base / 4u;
        let b4 = b_base / 4u;
        let n4 = params.k / 4u;
        for (var w = 0u; w < n4; w = w + 1u) {
            let av = fa4[a4 + w];
            let bv = fb4[b4 + w];
            sum = sum + av.x * bv.x;
            sum = sum + av.y * bv.y;
            sum = sum + av.z * bv.z;
            sum = sum + av.w * bv.w;
        }
    } else {
        for (var kk = 0u; kk < params.k; kk = kk + 1u) {
            sum = sum + fa[a_base + kk] * fb[b_base + kk];
        }
    }
    out[row * params.n + col] = sum;
}

// 2×2 寄存器阻塞 + K 段 8（对齐 cuda-oxide 的 reg2 结构）。共享内存按 vec4 组织：
// 每线程一次 128 位读喂 4 个乘加，共享读次数是标量版的 1/4（这是反超的关键）。
// 每行 8 个 vec4 槽（只用 2 个），槽位 `part ^ ((row >> 1) & 7)`：warp 内行号相距 2。
const K8: u32 = 8u;
var<workgroup> t2k8_a4: array<vec4<f32>, 128>;
var<workgroup> t2k8_b4: array<vec4<f32>, 128>;

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
    let vector_ok = params.k % 4u == 0u;
    for (var chunk = 0u; chunk < k_tiles; chunk = chunk + 1u) {
        if vector_ok {
            // 32 个 vec4 槽（16 行 × 2 段），前 32 线程各装一个。
            if (idx < 32u) {
                let r = idx / 2u;
                let part = idx % 2u;
                let k4 = params.k / 4u;
                let seg_ok = (chunk * 2u + part) * 4u < params.k;
                t2k8_a4[r * 8u + (part ^ ((r >> 1u) & 7u))] = select(vec4<f32>(0.0), fa4[(row_base + r) * k4 + chunk * 2u + part], row_base + r < params.m && seg_ok);
                t2k8_b4[r * 8u + (part ^ ((r >> 1u) & 7u))] = select(vec4<f32>(0.0), fb4[(col_base + r) * k4 + chunk * 2u + part], col_base + r < params.n && seg_ok);
            }
        } else {
            // 标量回退：前 32 线程各拼一个 vec4（16 行 × 2 段）。
            if (idx < 32u) {
                let r = idx / 2u;
                let part = idx % 2u;
                let k0 = chunk * K8 + part * 4u;
                let a_ok = row_base + r < params.m;
                let b_ok = col_base + r < params.n;
                t2k8_a4[r * 8u + (part ^ ((r >> 1u) & 7u))] = vec4<f32>(
                    select(0.0, fa[(row_base + r) * params.k + k0 + 0u], a_ok && k0 + 0u < params.k),
                    select(0.0, fa[(row_base + r) * params.k + k0 + 1u], a_ok && k0 + 1u < params.k),
                    select(0.0, fa[(row_base + r) * params.k + k0 + 2u], a_ok && k0 + 2u < params.k),
                    select(0.0, fa[(row_base + r) * params.k + k0 + 3u], a_ok && k0 + 3u < params.k),
                );
                t2k8_b4[r * 8u + (part ^ ((r >> 1u) & 7u))] = vec4<f32>(
                    select(0.0, fb[(col_base + r) * params.k + k0 + 0u], b_ok && k0 + 0u < params.k),
                    select(0.0, fb[(col_base + r) * params.k + k0 + 1u], b_ok && k0 + 1u < params.k),
                    select(0.0, fb[(col_base + r) * params.k + k0 + 2u], b_ok && k0 + 2u < params.k),
                    select(0.0, fb[(col_base + r) * params.k + k0 + 3u], b_ok && k0 + 3u < params.k),
                );
            }
        }
        workgroupBarrier();
        // 每次 128 位共享读喂 4 个乘加；累加次序与逐 kk 标量版一致（x,y,z,w）。
        for (var w4 = 0u; w4 < 2u; w4 = w4 + 1u) {
            let a0 = t2k8_a4[r0 * 8u + (w4 ^ ((r0 >> 1u) & 7u))];
            let a1 = t2k8_a4[(r0 + 1u) * 8u + (w4 ^ (((r0 + 1u) >> 1u) & 7u))];
            let b0 = t2k8_b4[c0 * 8u + (w4 ^ ((c0 >> 1u) & 7u))];
            let b1 = t2k8_b4[(c0 + 1u) * 8u + (w4 ^ (((c0 + 1u) >> 1u) & 7u))];
            s00 = s00 + a0.x * b0.x;
            s00 = s00 + a0.y * b0.y;
            s00 = s00 + a0.z * b0.z;
            s00 = s00 + a0.w * b0.w;
            s01 = s01 + a0.x * b1.x;
            s01 = s01 + a0.y * b1.y;
            s01 = s01 + a0.z * b1.z;
            s01 = s01 + a0.w * b1.w;
            s10 = s10 + a1.x * b0.x;
            s10 = s10 + a1.y * b0.y;
            s10 = s10 + a1.z * b0.z;
            s10 = s10 + a1.w * b0.w;
            s11 = s11 + a1.x * b1.x;
            s11 = s11 + a1.y * b1.y;
            s11 = s11 + a1.z * b1.z;
            s11 = s11 + a1.w * b1.w;
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

// 4×4 寄存器阻塞 + K 段 32：32×32 tile、8×8 线程、共享 2×4KB（8 KB 总量，
// 已是 16 KB workgroup 存储上限下的最宽段：再宽会超限）。K 段越宽屏障越少。
// 8 个 vec4 槽全用，槽位 `part ^ ((row >> 2) & 7)`（行距 128 B 会全部同 bank 组）。
const K32: u32 = 32u;
var<workgroup> t4k32_a4: array<vec4<f32>, 256>;
var<workgroup> t4k32_b4: array<vec4<f32>, 256>;

@compute @workgroup_size(8, 8)
fn main_tiled4k32(
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
    let k_tiles = (params.k + K32 - 1u) / K32;
    let vector_ok = params.k % 4u == 0u;
    for (var chunk = 0u; chunk < k_tiles; chunk = chunk + 1u) {
        if vector_ok {
            for (var t = 0u; t < 4u; t = t + 1u) {
                let g = idx + t * 64u;
                let r = g / 8u;
                let part = g % 8u;
                let k4 = params.k / 4u;
                let seg_ok = (chunk * 8u + part) * 4u < params.k;
                t4k32_a4[r * 8u + (part ^ ((r >> 2u) & 7u))] = select(vec4<f32>(0.0), fa4[(row_base + r) * k4 + chunk * 8u + part], row_base + r < params.m && seg_ok);
                t4k32_b4[r * 8u + (part ^ ((r >> 2u) & 7u))] = select(vec4<f32>(0.0), fb4[(col_base + r) * k4 + chunk * 8u + part], col_base + r < params.n && seg_ok);
            }
        } else {
            for (var t = 0u; t < 4u; t = t + 1u) {
                let g = idx + t * 64u;
                let r = g / 8u;
                let part = g % 8u;
                let k0 = chunk * K32 + part * 4u;
                let a_ok = row_base + r < params.m;
                let b_ok = col_base + r < params.n;
                t4k32_a4[r * 8u + (part ^ ((r >> 2u) & 7u))] = vec4<f32>(
                    select(0.0, fa[(row_base + r) * params.k + k0 + 0u], a_ok && k0 + 0u < params.k),
                    select(0.0, fa[(row_base + r) * params.k + k0 + 1u], a_ok && k0 + 1u < params.k),
                    select(0.0, fa[(row_base + r) * params.k + k0 + 2u], a_ok && k0 + 2u < params.k),
                    select(0.0, fa[(row_base + r) * params.k + k0 + 3u], a_ok && k0 + 3u < params.k),
                );
                t4k32_b4[r * 8u + (part ^ ((r >> 2u) & 7u))] = vec4<f32>(
                    select(0.0, fb[(col_base + r) * params.k + k0 + 0u], b_ok && k0 + 0u < params.k),
                    select(0.0, fb[(col_base + r) * params.k + k0 + 1u], b_ok && k0 + 1u < params.k),
                    select(0.0, fb[(col_base + r) * params.k + k0 + 2u], b_ok && k0 + 2u < params.k),
                    select(0.0, fb[(col_base + r) * params.k + k0 + 3u], b_ok && k0 + 3u < params.k),
                );
            }
        }
        workgroupBarrier();
        for (var w4 = 0u; w4 < 8u; w4 = w4 + 1u) {
            let a0 = t4k32_a4[(r0 + 0u) * 8u + (w4 ^ (((r0 + 0u) >> 2u) & 7u))];
            let a1 = t4k32_a4[(r0 + 1u) * 8u + (w4 ^ (((r0 + 1u) >> 2u) & 7u))];
            let a2 = t4k32_a4[(r0 + 2u) * 8u + (w4 ^ (((r0 + 2u) >> 2u) & 7u))];
            let a3 = t4k32_a4[(r0 + 3u) * 8u + (w4 ^ (((r0 + 3u) >> 2u) & 7u))];
            let b0 = t4k32_b4[(c0 + 0u) * 8u + (w4 ^ (((c0 + 0u) >> 2u) & 7u))];
            let b1 = t4k32_b4[(c0 + 1u) * 8u + (w4 ^ (((c0 + 1u) >> 2u) & 7u))];
            let b2 = t4k32_b4[(c0 + 2u) * 8u + (w4 ^ (((c0 + 2u) >> 2u) & 7u))];
            let b3 = t4k32_b4[(c0 + 3u) * 8u + (w4 ^ (((c0 + 3u) >> 2u) & 7u))];
            s00 = s00 + a0.x * b0.x;
            s00 = s00 + a0.y * b0.y;
            s00 = s00 + a0.z * b0.z;
            s00 = s00 + a0.w * b0.w;
            s01 = s01 + a0.x * b1.x;
            s01 = s01 + a0.y * b1.y;
            s01 = s01 + a0.z * b1.z;
            s01 = s01 + a0.w * b1.w;
            s02 = s02 + a0.x * b2.x;
            s02 = s02 + a0.y * b2.y;
            s02 = s02 + a0.z * b2.z;
            s02 = s02 + a0.w * b2.w;
            s03 = s03 + a0.x * b3.x;
            s03 = s03 + a0.y * b3.y;
            s03 = s03 + a0.z * b3.z;
            s03 = s03 + a0.w * b3.w;
            s10 = s10 + a1.x * b0.x;
            s10 = s10 + a1.y * b0.y;
            s10 = s10 + a1.z * b0.z;
            s10 = s10 + a1.w * b0.w;
            s11 = s11 + a1.x * b1.x;
            s11 = s11 + a1.y * b1.y;
            s11 = s11 + a1.z * b1.z;
            s11 = s11 + a1.w * b1.w;
            s12 = s12 + a1.x * b2.x;
            s12 = s12 + a1.y * b2.y;
            s12 = s12 + a1.z * b2.z;
            s12 = s12 + a1.w * b2.w;
            s13 = s13 + a1.x * b3.x;
            s13 = s13 + a1.y * b3.y;
            s13 = s13 + a1.z * b3.z;
            s13 = s13 + a1.w * b3.w;
            s20 = s20 + a2.x * b0.x;
            s20 = s20 + a2.y * b0.y;
            s20 = s20 + a2.z * b0.z;
            s20 = s20 + a2.w * b0.w;
            s21 = s21 + a2.x * b1.x;
            s21 = s21 + a2.y * b1.y;
            s21 = s21 + a2.z * b1.z;
            s21 = s21 + a2.w * b1.w;
            s22 = s22 + a2.x * b2.x;
            s22 = s22 + a2.y * b2.y;
            s22 = s22 + a2.z * b2.z;
            s22 = s22 + a2.w * b2.w;
            s23 = s23 + a2.x * b3.x;
            s23 = s23 + a2.y * b3.y;
            s23 = s23 + a2.z * b3.z;
            s23 = s23 + a2.w * b3.w;
            s30 = s30 + a3.x * b0.x;
            s30 = s30 + a3.y * b0.y;
            s30 = s30 + a3.z * b0.z;
            s30 = s30 + a3.w * b0.w;
            s31 = s31 + a3.x * b1.x;
            s31 = s31 + a3.y * b1.y;
            s31 = s31 + a3.z * b1.z;
            s31 = s31 + a3.w * b1.w;
            s32 = s32 + a3.x * b2.x;
            s32 = s32 + a3.y * b2.y;
            s32 = s32 + a3.z * b2.z;
            s32 = s32 + a3.w * b2.w;
            s33 = s33 + a3.x * b3.x;
            s33 = s33 + a3.y * b3.y;
            s33 = s33 + a3.z * b3.z;
            s33 = s33 + a3.w * b3.w;
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
