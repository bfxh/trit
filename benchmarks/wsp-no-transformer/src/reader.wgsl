// mem_assoc2 读取器的 GPU 版：一个 workgroup 处理一个问题。
//
// 输入全是 CPU 预计算好的稀疏表示（top-16、pair 行号、qb 位图、选项概念表），
// 内核只做读取与累加；这样对照的是「读取器」本身，不是 topk/编码。
//
// 二阶读取用 120 个线程并行（每线程一对），累加用定点 i32 原子（2^16 缩放），
// 避免 f32 共享内存并发写。固定 C = 4324（论文 Opus gen5 mem_assoc2 的概念数）。

struct Params {
    c: u32,
    r: u32,
    q_len: u32,
    o_len: u32,
    mix: f32,
    packed: u32,
    _pad: vec2<u32>,
};

@group(0) @binding(0) var<storage, read> a: array<f32>;
@group(0) @binding(1) var<storage, read> row_ids: array<i32>;
@group(0) @binding(2) var<storage, read> row_w: array<f32>;
@group(0) @binding(3) var<storage, read> pair_rows: array<u32>;
@group(0) @binding(4) var<storage, read> top_idx: array<u32>;
@group(0) @binding(5) var<storage, read> top_val: array<f32>;
@group(0) @binding(6) var<storage, read> qb_ids: array<u32>;
@group(0) @binding(7) var<storage, read> qb_vals: array<f32>;
@group(0) @binding(8) var<storage, read> qb_mask: array<u32>;
@group(0) @binding(9) var<storage, read> ob_ids: array<u32>;
@group(0) @binding(10) var<storage, read> ob_w: array<f32>;
@group(0) @binding(11) var<storage, read_write> out: array<f32>;
@group(0) @binding(12) var<uniform> p: Params;
@group(0) @binding(13) var<storage, read> row_pos: array<u32>;
@group(0) @binding(14) var<storage, read> row_neg: array<u32>;
@group(0) @binding(15) var<storage, read> row_scale: array<f32>;

var<workgroup> read_i: array<atomic<i32>, 4324>;
var<workgroup> acc_buf: array<f32, 4324>;

const J: u32 = 16u;
const PAIRS: u32 = 256u;
const NONE: u32 = 4294967295u;
const SCALE: f32 = 262144.0;

@compute @workgroup_size(128)
fn main(@builtin(workgroup_id) wid: vec3<u32>, @builtin(local_invocation_id) lid: vec3<u32>) {
    let n = wid.x;
    let t = lid.x;

    for (var c = t; c < p.c; c = c + 128u) {
        atomicStore(&read_i[c], 0);
    }
    workgroupBarrier();

    // 二阶读取：120 个线程各处理一对，定点原子累加。
    if (t < 120u) {
        var i = 0u;
        var j = 0u;
        var rem = t;
        for (var a = 0u; a < J; a = a + 1u) {
            let cnt = J - a - 1u;
            if (rem < cnt) {
                i = a;
                j = a + 1u + rem;
                break;
            }
            rem = rem - cnt;
        }
        let strength = top_val[n * J + i] * top_val[n * J + j];
        if (strength != 0.0) {
            let row = pair_rows[n * PAIRS + i * J + j];
            let base = row * p.r;
            if (p.packed == 1u) {
                let pos = row_pos[row];
                let neg = row_neg[row];
                let scale = row_scale[row] * SCALE;
                for (var k = 0u; k < p.r; k = k + 1u) {
                    let c = row_ids[base + k];
                    if (c >= 0) {
                        let v = f32((pos >> k) & 1u) - f32((neg >> k) & 1u);
                        atomicAdd(&read_i[c], i32(strength * v * scale));
                    }
                }
            } else {
                for (var k = 0u; k < p.r; k = k + 1u) {
                    let c = row_ids[base + k];
                    if (c >= 0) {
                        atomicAdd(&read_i[c], i32(strength * row_w[base + k] * SCALE));
                    }
                }
            }
        }
    }
    workgroupBarrier();

    // 一阶读取：每个线程负责一部分输出概念，遍历问题概念袋里的全部非零概念。
    for (var c = t; c < p.c; c = c + 128u) {
        var sum = 0.0;
        for (var i = 0u; i < p.q_len; i = i + 1u) {
            let d = qb_ids[n * p.q_len + i];
            if (d == NONE) {
                break;
            }
            sum += qb_vals[n * p.q_len + i] * a[d * p.c + c];
        }
        acc_buf[c] = sum;
    }
    workgroupBarrier();

    // mask：问题概念袋里已有的概念不参与选项打分；顺带加 mix 一阶项。
    let words = (p.c + 31u) / 32u;
    for (var c = t; c < p.c; c = c + 128u) {
        let bit = (qb_mask[n * words + c / 32u] >> (c % 32u)) & 1u;
        let val = f32(atomicLoad(&read_i[c])) / SCALE + p.mix * acc_buf[c];
        atomicStore(&read_i[c], select(i32(val * SCALE), 0, bit == 1u));
    }
    workgroupBarrier();

    // 选项得分：四个线程各算一个选项，遍历 CPU 预计算的概念表。
    if (t < 4u) {
        var score = 0.0;
        for (var i = 0u; i < p.o_len; i = i + 1u) {
            let c = ob_ids[(n * 4u + t) * p.o_len + i];
            if (c == NONE) {
                break;
            }
            score += f32(atomicLoad(&read_i[c])) / SCALE * ob_w[(n * 4u + t) * p.o_len + i];
        }
        out[n * 4u + t] = score;
    }
}
