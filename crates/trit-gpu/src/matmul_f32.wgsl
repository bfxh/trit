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
// 同一缓冲的 vec4 视图：一次 128 位装载替 4 次 32 位（仅装载用）。
@group(0) @binding(4) var<storage, read> fa4: array<vec4<f32>>;
@group(0) @binding(5) var<storage, read> fb4: array<vec4<f32>>;

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
    let tile_id = lid.y * TILE + lid.x;
    let row_ok = row < params.m;
    let col_ok = col < params.n;
    let block_row = row - lid.y;
    let block_col = col - lid.x;
    var sum = 0.0;
    let full = params.k / TILE;
    // k 不是 4 的倍数时 vec4 的行步长对不上（k/4 会截断），走标量装载。
    let vector_ok = params.k % 4u == 0u;
    for (var chunk = 0u; chunk < full; chunk = chunk + 1u) {
        if vector_ok {
            // 向量化装载：前 64 个线程各装 1 个 vec4（16 行 × 4 段）。
            if (tile_id < 64u) {
                let r = tile_id / 4u;
                let part = tile_id % 4u;
                let a_index = (block_row + r) * (params.k / 4u) + chunk * 4u + part;
                let av = select(vec4<f32>(0.0), fa4[a_index], block_row + r < params.m);
                tile_a[r * TILE + part * 4u + 0u] = av.x;
                tile_a[r * TILE + part * 4u + 1u] = av.y;
                tile_a[r * TILE + part * 4u + 2u] = av.z;
                tile_a[r * TILE + part * 4u + 3u] = av.w;
                let b_index = (block_col + r) * (params.k / 4u) + chunk * 4u + part;
                let bv = select(vec4<f32>(0.0), fb4[b_index], block_col + r < params.n);
                // 累加按 tile_b[k][c] 读，所以 B 段要按 [k][列] 落位。
                tile_b[(part * 4u + 0u) * TILE + r] = bv.x;
                tile_b[(part * 4u + 1u) * TILE + r] = bv.y;
                tile_b[(part * 4u + 2u) * TILE + r] = bv.z;
                tile_b[(part * 4u + 3u) * TILE + r] = bv.w;
            }
        } else {
            let a_col = chunk * TILE + lid.x;
            tile_a[tile_id] = select(0.0, fa[row * params.k + a_col], row_ok && a_col < params.k);
            let b_row = chunk * TILE + lid.y;
            tile_b[tile_id] = select(0.0, fb[col * params.k + b_row], col_ok && b_row < params.k);
        }
        workgroupBarrier();
        for (var kk = 0u; kk < TILE; kk = kk + 1u) {
            sum = sum + tile_a[lid.y * TILE + kk] * tile_b[kk * TILE + lid.x];
        }
        workgroupBarrier();
    }
    // 尾段（k 不是 16 的倍数）：标量装载 + 守卫。
    if (params.k % TILE != 0u) {
        let a_k = full * TILE + lid.x;
        let b_k = full * TILE + lid.y;
        if row_ok && a_k < params.k {
            tile_a[tile_id] = fa[row * params.k + a_k];
        } else {
            tile_a[tile_id] = 0.0;
        }
        if col_ok && b_k < params.k {
            tile_b[tile_id] = fb[col * params.k + b_k];
        } else {
            tile_b[tile_id] = 0.0;
        }
        workgroupBarrier();
        for (var kk = 0u; kk < TILE; kk = kk + 1u) {
            if (full * TILE + kk < params.k) {
                sum = sum + tile_a[lid.y * TILE + kk] * tile_b[kk * TILE + lid.x];
            }
        }
        workgroupBarrier();
    }
    if (row_ok && col_ok) {
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
// 每线程 4×4 输出，K 方向每次搬 8 个元素。共享内存按 vec4 组织：
// 内层每次 128 位读喂 4 个乘加，共享读次数是标量版的 1/4。
const T4: u32 = 32u;
const K_CHUNK: u32 = 8u;

var<workgroup> t4_a4: array<vec4<f32>, 64>;
var<workgroup> t4_b4: array<vec4<f32>, 64>;

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
            t4_a4[idx] = select(vec4<f32>(0.0), fa4[(row_base + r) * k4 + chunk * 2u + part], row_base + r < params.m && seg_ok);
            t4_b4[idx] = select(vec4<f32>(0.0), fb4[(col_base + r) * k4 + chunk * 2u + part], col_base + r < params.n && seg_ok);
        } else {
            let r = idx / 2u;
            let part = idx % 2u;
            let k0 = chunk * K_CHUNK + part * 4u;
            let a_ok = row_base + r < params.m;
            let b_ok = col_base + r < params.n;
            t4_a4[idx] = vec4<f32>(
                select(0.0, fa[(row_base + r) * params.k + k0 + 0u], a_ok && k0 + 0u < params.k),
                select(0.0, fa[(row_base + r) * params.k + k0 + 1u], a_ok && k0 + 1u < params.k),
                select(0.0, fa[(row_base + r) * params.k + k0 + 2u], a_ok && k0 + 2u < params.k),
                select(0.0, fa[(row_base + r) * params.k + k0 + 3u], a_ok && k0 + 3u < params.k),
            );
            t4_b4[idx] = vec4<f32>(
                select(0.0, fb[(col_base + r) * params.k + k0 + 0u], b_ok && k0 + 0u < params.k),
                select(0.0, fb[(col_base + r) * params.k + k0 + 1u], b_ok && k0 + 1u < params.k),
                select(0.0, fb[(col_base + r) * params.k + k0 + 2u], b_ok && k0 + 2u < params.k),
                select(0.0, fb[(col_base + r) * params.k + k0 + 3u], b_ok && k0 + 3u < params.k),
            );
        }
        workgroupBarrier();
        for (var w4 = 0u; w4 < 2u; w4 = w4 + 1u) {
            let a0 = t4_a4[(r0 + 0u) * 2u + w4];
            let a1 = t4_a4[(r0 + 1u) * 2u + w4];
            let a2 = t4_a4[(r0 + 2u) * 2u + w4];
            let a3 = t4_a4[(r0 + 3u) * 2u + w4];
            let b0 = t4_b4[(c0 + 0u) * 2u + w4];
            let b1 = t4_b4[(c0 + 1u) * 2u + w4];
            let b2 = t4_b4[(c0 + 2u) * 2u + w4];
            let b3 = t4_b4[(c0 + 3u) * 2u + w4];
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
const K8: u32 = 8u;
var<workgroup> t2k8_a4: array<vec4<f32>, 32>;
var<workgroup> t2k8_b4: array<vec4<f32>, 32>;

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
                t2k8_a4[idx] = select(vec4<f32>(0.0), fa4[(row_base + r) * k4 + chunk * 2u + part], row_base + r < params.m && seg_ok);
                t2k8_b4[idx] = select(vec4<f32>(0.0), fb4[(col_base + r) * k4 + chunk * 2u + part], col_base + r < params.n && seg_ok);
            }
        } else {
            // 标量回退：前 32 线程各拼一个 vec4（16 行 × 2 段）。
            if (idx < 32u) {
                let r = idx / 2u;
                let part = idx % 2u;
                let k0 = chunk * K8 + part * 4u;
                let a_ok = row_base + r < params.m;
                let b_ok = col_base + r < params.n;
                t2k8_a4[idx] = vec4<f32>(
                    select(0.0, fa[(row_base + r) * params.k + k0 + 0u], a_ok && k0 + 0u < params.k),
                    select(0.0, fa[(row_base + r) * params.k + k0 + 1u], a_ok && k0 + 1u < params.k),
                    select(0.0, fa[(row_base + r) * params.k + k0 + 2u], a_ok && k0 + 2u < params.k),
                    select(0.0, fa[(row_base + r) * params.k + k0 + 3u], a_ok && k0 + 3u < params.k),
                );
                t2k8_b4[idx] = vec4<f32>(
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
            let a0 = t2k8_a4[r0 * 2u + w4];
            let a1 = t2k8_a4[(r0 + 1u) * 2u + w4];
            let b0 = t2k8_b4[c0 * 2u + w4];
            let b1 = t2k8_b4[(c0 + 1u) * 2u + w4];
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

// 4×4 寄存器阻塞 + K 段 16：32×32 tile、8×8 线程，每段 16 元素（共享 4×1KB）。
// 与 main_tiled4 同结构，只是把 K 段从 8 加到 16（屏障减半、每次装载复用更多）。
const K16: u32 = 16u;
var<workgroup> t4k16_a4: array<vec4<f32>, 128>;
var<workgroup> t4k16_b4: array<vec4<f32>, 128>;

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
    let k_tiles = (params.k + K16 - 1u) / K16;
    let vector_ok = params.k % 4u == 0u;
    for (var chunk = 0u; chunk < k_tiles; chunk = chunk + 1u) {
        if vector_ok {
            // 128 个 vec4 槽（32 行 × 4 段），每线程装两个。
            for (var t = 0u; t < 2u; t = t + 1u) {
                let g = idx + t * 64u;
                let r = g / 4u;
                let part = g % 4u;
                let k4 = params.k / 4u;
                let seg_ok = (chunk * 4u + part) * 4u < params.k;
                t4k16_a4[g] = select(vec4<f32>(0.0), fa4[(row_base + r) * k4 + chunk * 4u + part], row_base + r < params.m && seg_ok);
                t4k16_b4[g] = select(vec4<f32>(0.0), fb4[(col_base + r) * k4 + chunk * 4u + part], col_base + r < params.n && seg_ok);
            }
        } else {
            for (var t = 0u; t < 2u; t = t + 1u) {
                let g = idx + t * 64u;
                let r = g / 4u;
                let part = g % 4u;
                let k0 = chunk * K16 + part * 4u;
                let a_ok = row_base + r < params.m;
                let b_ok = col_base + r < params.n;
                t4k16_a4[g] = vec4<f32>(
                    select(0.0, fa[(row_base + r) * params.k + k0 + 0u], a_ok && k0 + 0u < params.k),
                    select(0.0, fa[(row_base + r) * params.k + k0 + 1u], a_ok && k0 + 1u < params.k),
                    select(0.0, fa[(row_base + r) * params.k + k0 + 2u], a_ok && k0 + 2u < params.k),
                    select(0.0, fa[(row_base + r) * params.k + k0 + 3u], a_ok && k0 + 3u < params.k),
                );
                t4k16_b4[g] = vec4<f32>(
                    select(0.0, fb[(col_base + r) * params.k + k0 + 0u], b_ok && k0 + 0u < params.k),
                    select(0.0, fb[(col_base + r) * params.k + k0 + 1u], b_ok && k0 + 1u < params.k),
                    select(0.0, fb[(col_base + r) * params.k + k0 + 2u], b_ok && k0 + 2u < params.k),
                    select(0.0, fb[(col_base + r) * params.k + k0 + 3u], b_ok && k0 + 3u < params.k),
                );
            }
        }
        workgroupBarrier();
        for (var w4 = 0u; w4 < 4u; w4 = w4 + 1u) {
            let a0 = t4k16_a4[(r0 + 0u) * 4u + w4];
            let a1 = t4k16_a4[(r0 + 1u) * 4u + w4];
            let a2 = t4k16_a4[(r0 + 2u) * 4u + w4];
            let a3 = t4k16_a4[(r0 + 3u) * 4u + w4];
            let b0 = t4k16_b4[(c0 + 0u) * 4u + w4];
            let b1 = t4k16_b4[(c0 + 1u) * 4u + w4];
            let b2 = t4k16_b4[(c0 + 2u) * 4u + w4];
            let b3 = t4k16_b4[(c0 + 3u) * 4u + w4];
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
