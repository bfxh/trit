// WF32 层间布局搬运：上一层的 out（行主序 m×n）直接写进下一层的 b（列主序、每列补齐到 32）。
//
// 约定与 matmul_wf32 一致：B 的第 c 列是「第 c 个像素的 k 个通道值」，
// 长度补齐到 words*32，尾部补 0。上一层的 out[r][c] = 第 r 个输出通道、第 c 个像素。
// 所以链式（1×1 conv / 逐通道线性）时：
//
//   b_next[c][r] = out_prev[r][c]，r < k_next(= m_prev)，其余补 0
//
// 走 device→device，中间结果不出显存：省掉「回读上一层 + host 上传下一层」两笔搬运。

struct Params {
    rows: u32, // = 上一层的 m = 本层 k
    cols: u32, // = 上一层的 n = 本层 n
    words: u32, // = ceil(rows / 32)
    total: u32, // = cols * words * 32
};

@group(0) @binding(0) var<storage, read> src: array<f32>;
@group(0) @binding(1) var<storage, read_write> dst: array<f32>;
@group(0) @binding(2) var<uniform> params: Params;

@compute @workgroup_size(64)
fn main(@builtin(global_invocation_id) gid: vec3<u32>) {
    let idx = gid.x;
    if idx >= params.total {
        return;
    }
    let stride = params.words * 32u;
    let c = idx / stride;
    let r = idx % stride;
    var v = 0.0;
    if r < params.rows {
        v = src[r * params.cols + c];
    }
    dst[idx] = v;
}
