// Q12 定点串行读取器：与 CPU model_fixed.rs 逐位一致。无 f32 累加。

struct Params {
    c: u32,
    r: u32,
    q_len: u32,
    o_len: u32,
    mix: f32,
    packed: u32,
    h: u32,
    _pad: u32,
};

@group(0) @binding(0) var<storage, read> a: array<f32>;
@group(0) @binding(1) var<storage, read> row_ids: array<u32>;
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

var<workgroup> read_q: array<i32, 5120>;
var<workgroup> acc_q: array<i32, 5120>;

const J: u32 = 16u;
const PAIRS: u32 = 256u;
const NONE: u32 = 4294967295u;
const QF: f32 = 2048.0;

fn fx(x: f32) -> i32 {
    return i32(round(x * QF));
}

@compute @workgroup_size(128)
fn main(@builtin(workgroup_id) wid: vec3<u32>, @builtin(local_invocation_id) lid: vec3<u32>) {
    let n = wid.x;
    let t = lid.x;

    for (var c = t; c < p.c; c = c + 128u) {
        read_q[c] = 0;
    }
    workgroupBarrier();

    if (t == 0u) {
        for (var a = 0u; a < J; a = a + 1u) {
            for (var b = a + 1u; b < J; b = b + 1u) {
                let strength = top_val[n * J + a] * top_val[n * J + b];
                if (strength == 0.0) {
                    continue;
                }
                let sq = fx(strength);
                let row = pair_rows[n * PAIRS + a * J + b];
                let base = row * p.r;
                if (p.packed == 1u) {
                    let pos = row_pos[row];
                    let neg = row_neg[row];
                    let scale = row_scale[row];
                    for (var k = 0u; k < p.r; k = k + 1u) {
                        let idx = base + k;
                        let raw = (row_ids[idx / 2u] >> ((idx % 2u) * 16u)) & 65535u;
                        if (raw != 65535u) {
                            let v = f32((pos >> k) & 1u) - f32((neg >> k) & 1u);
                            let rq = fx(v * scale);
                            read_q[raw] = read_q[raw] + ((sq * rq) >> 11);
                        }
                    }
                } else {
                    for (var k = 0u; k < p.r; k = k + 1u) {
                        let idx = base + k;
                        let raw = (row_ids[idx / 2u] >> ((idx % 2u) * 16u)) & 65535u;
                        if (raw != 65535u) {
                            let rq = fx(row_w[base + k]);
                            read_q[raw] = read_q[raw] + ((sq * rq) >> 11);
                        }
                    }
                }
            }
        }
    }
    workgroupBarrier();

    for (var c = t; c < p.c; c = c + 128u) {
        var sum = 0;
        for (var i = 0u; i < p.q_len; i = i + 1u) {
            let d = qb_ids[n * p.q_len + i];
            if (d == NONE) {
                break;
            }
            let vq = fx(qb_vals[n * p.q_len + i]);
            let aq = fx(a[d * p.c + c]);
            sum = sum + ((vq * aq) >> 11);
        }
        acc_q[c] = sum;
    }
    workgroupBarrier();

    let words = (p.c + 31u) / 32u;
    let mixq = fx(p.mix);
    for (var c = t; c < p.c; c = c + 128u) {
        let bit = (qb_mask[n * words + c / 32u] >> (c % 32u)) & 1u;
        read_q[c] = select(read_q[c] + ((mixq * acc_q[c]) >> 11), 0, bit == 1u);
    }
    workgroupBarrier();

    if (t < 4u) {
        var score = 0.0;
        for (var i = 0u; i < p.o_len; i = i + 1u) {
            let c = ob_ids[(n * 4u + t) * p.o_len + i];
            if (c == NONE) {
                break;
            }
            let ob = fx(ob_w[(n * 4u + t) * p.o_len + i]);
            if (read_q[c] != 0 && ob != 0) {
                score += (f32(read_q[c]) / QF) * (f32(ob) / QF);
            }
        }
        out[n * 4u + t] = score;
    }
}
