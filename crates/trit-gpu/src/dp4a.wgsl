//! **int8×int8 DP4A 内核（tiled + 4×4 寄存器阻塞）**。
//!
//! naive 版每个线程都从全局内存拉 A 行 / B 行 ⇒ 512³ 只有 9.08 ps/MAC（比同卡 f32 分块基线慢 ~7×）。
//! 本版：**64×64 输出 tile、每线程 4×4 输出、K 步 32**，A/B 片段进**共享内存**（各 2 KB），
//! 一条 `dot4I8Packed` 吃 4 个 MAC。
//!
//! 布局：`a` 是 M×K（每行 `kw = ceil(K/4)` 字），`bt` 是**预转置**的 N×K（每个输出列一行），
//! 两侧都是 K 方向连续、4 个 int8/u32、小端；`c` 出整型累加的 f32 形式。

const TILE: u32 = 32u; // 输出 tile 边长（32 ⇒ 256 个 workgroup @512³，够占满 SM）
const KSTEP: u32 = 32u; // 每个 K 步吃 32 个 k（= 8 个字）
const KW_PER_STEP: u32 = 8u;

@group(0) @binding(0) var<storage, read> a: array<u32>;
@group(0) @binding(1) var<storage, read> bt: array<u32>;
@group(0) @binding(2) var<storage, read_write> c: array<f32>;
@group(0) @binding(3) var<uniform> dims: vec4<u32>; // (m, n, k_words, 0)

// 共享片段：64 行 × 8 字（扁平成 1D）
var<workgroup> ta: array<u32, 256>;
var<workgroup> tb: array<u32, 256>;

@compute @workgroup_size(16, 16, 1)
fn main(
    @builtin(workgroup_id) wid: vec3<u32>,
    @builtin(local_invocation_id) lid: vec3<u32>,
) {
    let m = dims.x;
    let n = dims.y;
    let kw = dims.z;
    // 本 workgroup 负责的输出块起点
    let m0 = wid.x * TILE;
    let n0 = wid.y * TILE;
    let tid = lid.y * 16u + lid.x; // 0..255

    // 4×4 累加器（行 dy、列 dx）
    var acc: array<i32, 16>;
    for (var i: u32 = 0u; i < 16u; i = i + 1u) {
        acc[i] = 0;
    }

    let k_steps = (kw + KW_PER_STEP - 1u) / KW_PER_STEP;
    for (var s: u32 = 0u; s < k_steps; s = s + 1u) {
        let w0 = s * KW_PER_STEP;
        // ① 把 A/B 的片段搬进共享内存：每个线程搬 2 个字
        for (var ii: u32 = 0u; ii < 1u; ii = ii + 1u) {
            // 32 行 × 8 字 = 256 个字 ⇒ 每个线程搬 1 个（ii 只为保持形状）
            let idx = tid;
            let row = idx / KW_PER_STEP;
            let w = idx % KW_PER_STEP;
            let gi_m = m0 + row;
            let gi_n = n0 + row;
            // ⚠️ 用 if 而不是 select：`select` **两路都求值** ⇒ 越界读
            var va: u32 = 0u;
            var vb: u32 = 0u;
            if gi_m < m && (w0 + w) < kw {
                va = a[gi_m * kw + w0 + w];
            }
            if gi_n < n && (w0 + w) < kw {
                vb = bt[gi_n * kw + w0 + w];
            }
            ta[idx] = va;
            tb[idx] = vb;
        }
        workgroupBarrier();
        // ② 4×4 个输出，每个输出沿本段的 8 个字累加
        for (var w: u32 = 0u; w < KW_PER_STEP; w = w + 1u) {
            var aw: array<u32, 2>;
            var bw: array<u32, 2>;
            for (var dy: u32 = 0u; dy < 2u; dy = dy + 1u) {
                aw[dy] = ta[(lid.y * 2u + dy) * KW_PER_STEP + w];
            }
            for (var dx: u32 = 0u; dx < 2u; dx = dx + 1u) {
                bw[dx] = tb[(lid.x * 2u + dx) * KW_PER_STEP + w];
            }
            for (var dy: u32 = 0u; dy < 2u; dy = dy + 1u) {
                for (var dx: u32 = 0u; dx < 2u; dx = dx + 1u) {
                    acc[dy * 2u + dx] += dot4I8Packed(aw[dy], bw[dx]);
                }
            }
        }
        workgroupBarrier();
    }

    // ③ 写回
    for (var dy: u32 = 0u; dy < 2u; dy = dy + 1u) {
        let gm = m0 + lid.y * 2u + dy;
        if gm >= m {
            continue;
        }
        for (var dx: u32 = 0u; dx < 2u; dx = dx + 1u) {
            let gn = n0 + lid.x * 2u + dx;
            if gn >= n {
                continue;
            }
            c[gm * n + gn] = f32(acc[dy * 2u + dx]);
        }
    }
}
