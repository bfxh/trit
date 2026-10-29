//! **int8×int8 DP4A 内核**（`dot4I8Packed`）：三值权重 + 量化激活的整型 GEMM。
//!
//! 为什么需要它：`matmul_wf32` 走"三值权重 4 个/u32 + `unpack4x8snorm`"，每 MAC 仍要展开成
//! f32 乘加；而 WGSL 有 `dot4I8Packed(a, b) -> i32`（硬件 DP4A / `SPV_KHR_integer_dot_product`）
//! ⇒ 一条指令吃 4 个 MAC。
//!
//! **布局（两侧同构，K 方向连续打包，4 个 int8/u32，小端）**：
//! - `A`：M 行 × K，每行 `kw = ceil(K/4)` 个字；
//! - `B`：**预转置**为 N 行 × K（每个输出列一行）⇒ 内层循环与 A 完全对称，不用跨行拼字；
//! - `C`：M×N 的 f32（还原 `scale_a · scale_b` 由调用方做，本内核只出整型累加）。
//!
//! 编码：三值 `-1/0/+1` 直接写 int8 的 `0xFF/0x00/0x01`；`dot4I8Packed` 按**有符号**字节解释。

@group(0) @binding(0) var<storage, read> a: array<u32>;
@group(0) @binding(1) var<storage, read> bt: array<u32>;
@group(0) @binding(2) var<storage, read_write> c: array<f32>;
@group(0) @binding(3) var<uniform> dims: vec4<u32>; // (m, n, k_words, 0)

@compute @workgroup_size(8, 8, 1)
fn main(@builtin(global_invocation_id) gid: vec3<u32>) {
    let m = gid.x;
    let n = gid.y;
    let mm = dims.x;
    let nn = dims.y;
    let kw = dims.z;
    if m >= mm || n >= nn {
        return;
    }
    var acc: i32 = 0;
    for (var w: u32 = 0u; w < kw; w = w + 1u) {
        acc = acc + dot4I8Packed(a[m * kw + w], bt[n * kw + w]);
    }
    c[m * nn + n] = f32(acc);
}
