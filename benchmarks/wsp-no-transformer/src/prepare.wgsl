// CPU 预处理的 GPU 版：一个 workgroup 处理一个问题。
//
// 从 q/o 的 word id 直接构建 qb、top-16、pair 行号、qb 位图与选项概念表，
// 写进主读取器要用的 buffer。全部在 GPU 上做，省掉 CPU 预处理。

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

@group(0) @binding(0) var<storage, read> q: array<i32>;
@group(0) @binding(1) var<storage, read> o: array<i32>;
@group(0) @binding(2) var<storage, read> concepts: array<i32>;
@group(0) @binding(3) var<storage, read> idf: array<f32>;
@group(0) @binding(4) var<storage, read_write> top_idx: array<u32>;
@group(0) @binding(5) var<storage, read_write> top_val: array<f32>;
@group(0) @binding(6) var<storage, read_write> pair_rows: array<u32>;
@group(0) @binding(7) var<storage, read_write> qb_mask: array<u32>;
@group(0) @binding(8) var<storage, read_write> ob_ids: array<u32>;
@group(0) @binding(9) var<storage, read_write> ob_w: array<f32>;
@group(0) @binding(10) var<uniform> p: Params;
@group(0) @binding(11) var<storage, read_write> qb_ids: array<u32>;
@group(0) @binding(12) var<storage, read_write> qb_vals: array<f32>;

var<workgroup> qb: array<f32, 5120>;

const J: u32 = 16u;
const PAIRS: u32 = 256u;
const NONE: u32 = 4294967295u;

// (a*b) mod 2^22，用 32 步移位加避免 u32 溢出（b=1000003 需要 20 位）。
fn mulmod(a: u32, b: u32, m: u32) -> u32 {
    var r = 0u;
    var x = a % m;
    var y = b;
    for (var i = 0u; i < 20u; i = i + 1u) {
        if ((y & 1u) == 1u) {
            r = (r + x) % m;
        }
        x = (x << 1u) % m;
        y = y >> 1u;
    }
    return r;
}

@compute @workgroup_size(128)
fn main(@builtin(workgroup_id) wid: vec3<u32>, @builtin(local_invocation_id) lid: vec3<u32>) {
    let n = wid.x;
    let t = lid.x;
    let words = (p.c + 31u) / 32u;

    if (t == 0u) {
        for (var c = 0u; c < p.c; c = c + 1u) {
            qb[c] = 0.0;
        }
        for (var i = 0u; i < words; i = i + 1u) {
            qb_mask[n * words + i] = 0u;
        }
        for (var i = 0u; i < p.q_len; i = i + 1u) {
            let id = q[n * p.q_len + i];
            if (id == 0) {
                break;
            }
            let c = concepts[id];
            if (c > 0) {
                let cu = u32(c);
                qb[cu] = idf[cu];
                qb_mask[n * words + cu / 32] |= 1u << (cu % 32);
            }
        }

        // top-16：线性扫描 + 插入排序（值降序，相等时保持先遇到的）。
        var bi: array<u32, 16>;
        var bv: array<f32, 16>;
        for (var k = 0u; k < J; k = k + 1u) {
            bv[k] = -1e30;
            bi[k] = 0u;
        }
        for (var c = 1u; c < p.c; c = c + 1u) {
            let v = qb[c];
            if (v > bv[15u]) {
                var pos = 15u;
                loop {
                    if (pos == 0u || bv[pos - 1u] >= v) {
                        break;
                    }
                    bv[pos] = bv[pos - 1u];
                    bi[pos] = bi[pos - 1u];
                    pos = pos - 1u;
                }
                bv[pos] = v;
                bi[pos] = c;
            }
        }
        for (var k = 0u; k < J; k = k + 1u) {
            top_idx[n * J + k] = bi[k];
            top_val[n * J + k] = bv[k];
        }

        // pair 行号：(lo*1000003 + hi) mod 2^22。
        for (var a = 0u; a < J; a = a + 1u) {
            for (var b = a + 1u; b < J; b = b + 1u) {
                let x = bi[a];
                let y = bi[b];
                let lo = min(x, y);
                let hi = max(x, y);
                pair_rows[n * PAIRS + a * J + b] = (mulmod(lo, 1000003u, p.h) + hi) % p.h;
            }
        }
        // 一阶读取要用的非零概念列表（按 id 顺序），尾部写 NONE 终止。
        var used_q = 0u;
        for (var c = 1u; c < p.c; c = c + 1u) {
            if (qb[c] != 0.0 && used_q < p.q_len) {
                qb_ids[n * p.q_len + used_q] = c;
                qb_vals[n * p.q_len + used_q] = qb[c];
                used_q = used_q + 1u;
            }
        }
        if (used_q < p.q_len) {
            qb_ids[n * p.q_len + used_q] = NONE;
        }
    }
    workgroupBarrier();

    if (t < 4u) {
        let base = (n * 4u + t) * p.o_len;
        for (var i = 0u; i < p.o_len; i = i + 1u) {
            ob_ids[base + i] = NONE;
        }
        var used = 0u;
        for (var i = 0u; i < p.o_len; i = i + 1u) {
            let id = o[base + i];
            if (id == 0) {
                break;
            }
            let c = concepts[id];
            if (c <= 0) {
                continue;
            }
            let cu = u32(c);
            let in_q = (qb_mask[n * words + cu / 32] >> (cu % 32)) & 1u;
            if (in_q == 1u) {
                continue;
            }
            var dup = false;
            for (var j = 0u; j < used; j = j + 1u) {
                if (ob_ids[base + j] == cu) {
                    dup = true;
                }
            }
            if (!dup) {
                ob_ids[base + used] = cu;
                ob_w[base + used] = idf[cu];
                used = used + 1u;
            }
        }
    }
}
