//! 简化 ARAP：局部旋转（3×3 SVD）+ 全局 Laplacian/CG，约束到 GT 最近点。
//!
//! 能量：Σ_edges ||(v_i-v_j) - ½(R_i+R_j)(v_i⁰-v_j⁰)||² + λ Σ_i ||v_i-p_i||²
//! 每轮：查询 GT 最近点 → 求每顶点旋转 R_i → 解 (L+λI)v=b（Jacobi 预条件 CG）。
//!
//! 运行：`cargo run --release -q -p trit-3d --example arap -- <gt.obj> <recon.obj> <out.obj> [iters] [lambda] [cg]`

use std::collections::HashMap;

use trit_3d::mesh::TriMesh;

fn sub(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}
fn add(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}
fn scale(a: [f32; 3], s: f32) -> [f32; 3] {
    [a[0] * s, a[1] * s, a[2] * s]
}
fn dot(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}
fn mul3(m: [[f32; 3]; 3], v: [f32; 3]) -> [f32; 3] {
    [
        m[0][0] * v[0] + m[0][1] * v[1] + m[0][2] * v[2],
        m[1][0] * v[0] + m[1][1] * v[1] + m[1][2] * v[2],
        m[2][0] * v[0] + m[2][1] * v[1] + m[2][2] * v[2],
    ]
}

/// 3×3 对称矩阵 Jacobi 特征分解（列向量）。
fn eig3(mut a: [[f32; 3]; 3]) -> ([f32; 3], [[f32; 3]; 3]) {
    let mut v = [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]];
    for _ in 0..24 {
        let mut off = 0.0;
        for i in 0..3 {
            for j in (i + 1)..3 {
                off += a[i][j].abs();
            }
        }
        if off < 1e-12 {
            break;
        }
        for p in 0..3 {
            for q in (p + 1)..3 {
                if a[p][q].abs() < 1e-14 {
                    continue;
                }
                let theta = (a[q][q] - a[p][p]) / (2.0 * a[p][q]);
                let t = theta.signum() / (theta.abs() + (theta * theta + 1.0).sqrt());
                let c = 1.0 / (t * t + 1.0).sqrt();
                let s = t * c;
                for k in 0..3 {
                    let akp = a[k][p];
                    let akq = a[k][q];
                    a[k][p] = c * akp - s * akq;
                    a[k][q] = s * akp + c * akq;
                }
                for k in 0..3 {
                    let apk = a[p][k];
                    let aqk = a[q][k];
                    a[p][k] = c * apk - s * aqk;
                    a[q][k] = s * apk + c * aqk;
                }
                for k in 0..3 {
                    let vkp = v[k][p];
                    let vkq = v[k][q];
                    v[k][p] = c * vkp - s * vkq;
                    v[k][q] = s * vkp + c * vkq;
                }
            }
        }
    }
    let mut vals = [0.0f32; 3];
    for i in 0..3 {
        vals[i] = a[i][i];
    }
    (vals, v)
}

/// 3×3 SVD 后的极旋转 R；退化时返回单位阵。
fn polar_rotation(s: [[f32; 3]; 3]) -> [[f32; 3]; 3] {
    // S^T S
    let mut sts = [[0.0f32; 3]; 3];
    for i in 0..3 {
        for j in 0..3 {
            let mut acc = 0.0;
            for k in 0..3 {
                acc += s[k][i] * s[k][j];
            }
            sts[i][j] = acc;
        }
    }
    let (vals, v) = eig3(sts);
    // U = S V / sigma
    let mut u = [[0.0f32; 3]; 3];
    for c in 0..3 {
        let sig = vals[c].max(0.0).sqrt();
        let vc = [v[0][c], v[1][c], v[2][c]];
        let sv = mul3(s, vc);
        if sig > 1e-8 {
            for r in 0..3 {
                u[r][c] = sv[r] / sig;
            }
        } else {
            // 退化：用任意正交补
            let e = if c == 0 { [1.0, 0.0, 0.0] } else if c == 1 { [0.0, 1.0, 0.0] } else { [0.0, 0.0, 1.0] };
            for r in 0..3 {
                u[r][c] = e[r];
            }
        }
    }
    let mut r = [[0.0f32; 3]; 3];
    for i in 0..3 {
        for j in 0..3 {
            r[i][j] = u[i][0] * v[j][0] + u[i][1] * v[j][1] + u[i][2] * v[j][2];
        }
    }
    // det<0 时翻转最后一列
    let det = r[0][0] * (r[1][1] * r[2][2] - r[1][2] * r[2][1])
        - r[0][1] * (r[1][0] * r[2][2] - r[1][2] * r[2][0])
        + r[0][2] * (r[1][0] * r[2][1] - r[1][1] * r[2][0]);
    if det < 0.0 {
        for i in 0..3 {
            r[i][2] = -r[i][2];
        }
    }
    r
}

fn closest_on_tri(p: [f32; 3], a: [f32; 3], b: [f32; 3], c: [f32; 3]) -> [f32; 3] {
    let ab = sub(b, a);
    let ac = sub(c, a);
    let ap = sub(p, a);
    let d1 = dot(ab, ap);
    let d2 = dot(ac, ap);
    if d1 <= 0.0 && d2 <= 0.0 {
        return a;
    }
    let bp = sub(p, b);
    let d3 = dot(ab, bp);
    let d4 = dot(ac, bp);
    if d3 >= 0.0 && d4 <= d3 {
        return b;
    }
    let vc = d1 * d4 - d3 * d2;
    if vc <= 0.0 && d1 >= 0.0 && d3 <= 0.0 {
        return add(a, scale(ab, d1 / (d1 - d3)));
    }
    let cp = sub(p, c);
    let d5 = dot(ab, cp);
    let d6 = dot(ac, cp);
    if d6 >= 0.0 && d5 <= d6 {
        return c;
    }
    let vb = d5 * d2 - d1 * d6;
    if vb <= 0.0 && d2 >= 0.0 && d6 <= 0.0 {
        return add(a, scale(ac, d2 / (d2 - d6)));
    }
    let va = d3 * d6 - d5 * d4;
    if va <= 0.0 && (d4 - d3) >= 0.0 && (d5 - d6) >= 0.0 {
        let w = (d4 - d3) / ((d4 - d3) + (d5 - d6));
        return add(b, scale(sub(c, b), w));
    }
    let denom = 1.0 / (va + vb + vc);
    let v = vb * denom;
    let w = vc * denom;
    add(a, add(scale(ab, v), scale(ac, w)))
}

fn key(p: [f32; 3], cell: f32) -> (i32, i32, i32) {
    ((p[0] / cell).floor() as i32, (p[1] / cell).floor() as i32, (p[2] / cell).floor() as i32)
}

/// 每个三角形当前法线与初始法线的点积（<0 表示翻转）。
fn tri_flip_metric(a: [f32; 3], b: [f32; 3], c: [f32; 3], a0: [f32; 3], b0: [f32; 3], c0: [f32; 3]) -> f32 {
    let n = cross(sub(b, a), sub(c, a));
    let n0 = cross(sub(b0, a0), sub(c0, a0));
    dot(n, n0)
}

fn cross(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

/// 有向体积与 ∂V/∂v_i（V = 1/6 Σ a·(b×c)）。
fn volume_and_grad(verts: &[[f32; 3]], tris: &[[u32; 3]]) -> (f32, Vec<[f32; 3]>) {
    let mut vol = 0.0f32;
    let mut g = vec![[0.0f32; 3]; verts.len()];
    for t in tris {
        let a = verts[t[0] as usize];
        let b = verts[t[1] as usize];
        let c = verts[t[2] as usize];
        let bc = cross(b, c);
        let ca = cross(c, a);
        let ab = cross(a, b);
        vol += dot(a, bc) / 6.0;
        for k in 0..3 {
            g[t[0] as usize][k] += bc[k] / 6.0;
            g[t[1] as usize][k] += ca[k] / 6.0;
            g[t[2] as usize][k] += ab[k] / 6.0;
        }
    }
    (vol, g)
}

fn matvec(nbrs: &[Vec<u32>], lambda: f32, extra: &[f32], x: &[[f32; 3]]) -> Vec<[f32; 3]> {
    let mut y = Vec::with_capacity(x.len());
    for i in 0..x.len() {
        let mut acc = [0.0f32; 3];
        for &j in &nbrs[i] {
            acc = add(acc, x[j as usize]);
        }
        let d = nbrs[i].len() as f32 + lambda + extra[i];
        y.push([
            d * x[i][0] - acc[0],
            d * x[i][1] - acc[1],
            d * x[i][2] - acc[2],
        ]);
    }
    y
}

fn cg(nbrs: &[Vec<u32>], b: &[[f32; 3]], lambda: f32, extra: &[f32], x0: Vec<[f32; 3]>, iters: usize) -> Vec<[f32; 3]> {
    let mut x = x0;
    let ax = matvec(nbrs, lambda, extra, &x);
    let mut r: Vec<[f32; 3]> = b.iter().zip(&ax).map(|(&bi, &ai)| sub(bi, ai)).collect();
    let mut p = r.clone();
    let mut rs = r.iter().map(|v| dot(*v, *v)).sum::<f32>();
    for _ in 0..iters {
        let ap = matvec(nbrs, lambda, extra, &p);
        let denom: f32 = p.iter().zip(&ap).map(|(&a, &c)| dot(a, c)).sum();
        if denom.abs() < 1e-20 {
            break;
        }
        let alpha = rs / denom;
        for i in 0..x.len() {
            x[i] = add(x[i], scale(p[i], alpha));
            r[i] = sub(r[i], scale(ap[i], alpha));
        }
        let rs_new: f32 = r.iter().map(|v| dot(*v, *v)).sum();
        if rs_new < 1e-12 {
            break;
        }
        let beta = rs_new / rs;
        for i in 0..p.len() {
            p[i] = add(r[i], scale(p[i], beta));
        }
        rs = rs_new;
    }
    x
}

fn main() {
    let gt_path = std::env::args().nth(1).expect("用法: arap <gt.obj> <recon.obj> <out.obj> [iters] [lambda] [cg]");
    let rc_path = std::env::args().nth(2).expect("用法: arap <gt.obj> <recon.obj> <out.obj> [iters] [lambda] [cg]");
    let out_path = std::env::args().nth(3).expect("用法: arap <gt.obj> <recon.obj> <out.obj> [iters] [lambda] [cg]");
    let iters: usize = std::env::args().nth(4).unwrap_or_else(|| "5".to_string()).parse().expect("iters");
    let lambda: f32 = std::env::args().nth(5).unwrap_or_else(|| "1.0".to_string()).parse().expect("lambda");
    let cg_iters: usize = std::env::args().nth(6).unwrap_or_else(|| "120".to_string()).parse().expect("cg");
    let fold_w: f32 = std::env::args().nth(7).unwrap_or_else(|| "0".to_string()).parse().expect("fold");
    let vol_w: f32 = std::env::args().nth(8).unwrap_or_else(|| "0".to_string()).parse().expect("vol");
    let vol_target: f32 = std::env::args().nth(9).unwrap_or_else(|| "-1".to_string()).parse().expect("vtarget");
    let repair_iters: usize = std::env::args().nth(10).unwrap_or_else(|| "0".to_string()).parse().expect("repair");
    let repair_w: f32 = std::env::args().nth(11).unwrap_or_else(|| "0.5".to_string()).parse().expect("repair_w");

    let gt = TriMesh::from_obj(std::path::Path::new(&gt_path)).expect("读 GT");
    let rc = TriMesh::from_obj(std::path::Path::new(&rc_path)).expect("读重建");
    let n = rc.verts.len();
    let v0 = rc.verts.clone();
    let mut v = rc.verts.clone();
    let v0_vol = rc.signed_volume();
    let vol_ratio = if vol_target > 0.0 { vol_target } else { 1.0 };
    let target_vol = vol_ratio * v0_vol;

    // 顶点邻接（去重）。
    let mut sets: Vec<Vec<u32>> = vec![Vec::new(); n];
    for t in &rc.tris {
        for k in 0..3 {
            let a = t[k];
            let b = t[(k + 1) % 3];
            sets[a as usize].push(b);
            sets[b as usize].push(a);
        }
    }
    let nbrs: Vec<Vec<u32>> = sets
        .into_iter()
        .map(|mut v| {
            v.sort_unstable();
            v.dedup();
            v
        })
        .collect();
    let mut edges: Vec<(u32, u32)> = Vec::new();
    for i in 0..n {
        for &j in &nbrs[i] {
            if (i as u32) < j {
                edges.push((i as u32, j));
            }
        }
    }

    // GT 质心哈希。
    let cell = 0.05f32;
    let mut grid: HashMap<(i32, i32, i32), Vec<usize>> = HashMap::new();
    let mut cent: Vec<[f32; 3]> = Vec::with_capacity(gt.tris.len());
    for (i, t) in gt.tris.iter().enumerate() {
        let c = [
            (gt.verts[t[0] as usize][0] + gt.verts[t[1] as usize][0] + gt.verts[t[2] as usize][0]) / 3.0,
            (gt.verts[t[0] as usize][1] + gt.verts[t[1] as usize][1] + gt.verts[t[2] as usize][1]) / 3.0,
            (gt.verts[t[0] as usize][2] + gt.verts[t[1] as usize][2] + gt.verts[t[2] as usize][2]) / 3.0,
        ];
        cent.push(c);
        grid.entry(key(c, cell)).or_default().push(i);
    }

    for iter in 0..iters {
        let mut target = vec![[0.0f32; 3]; n];
        for i in 0..n {
            let p = v[i];
            let base = key(p, cell);
            let mut best = f32::INFINITY;
            let mut best_q = p;
            for dx in -1..=1 {
                for dy in -1..=1 {
                    for dz in -1..=1 {
                        if let Some(list) = grid.get(&(base.0 + dx, base.1 + dy, base.2 + dz)) {
                            for &ti in list {
                                let t = gt.tris[ti];
                                let q = closest_on_tri(
                                    p,
                                    gt.verts[t[0] as usize],
                                    gt.verts[t[1] as usize],
                                    gt.verts[t[2] as usize],
                                );
                                let d = (q[0] - p[0]).powi(2) + (q[1] - p[1]).powi(2) + (q[2] - p[2]).powi(2);
                                if d < best {
                                    best = d;
                                    best_q = q;
                                }
                            }
                        }
                    }
                }
            }
            target[i] = best_q;
        }

        // 翻转锚：当前法线与初始法线反向的三角形，其顶点加软位置锚（把翻回的面拉回去）。
        let mut extra = vec![0.0f32; n];
        if fold_w > 0.0 {
            let mut flipped = vec![false; n];
            let mut tri_cnt = 0usize;
            for t in &rc.tris {
                let (a, b, c) = (t[0] as usize, t[1] as usize, t[2] as usize);
                if tri_flip_metric(v[a], v[b], v[c], v0[a], v0[b], v0[c]) < 0.0 {
                    tri_cnt += 1;
                    flipped[a] = true;
                    flipped[b] = true;
                    flipped[c] = true;
                }
            }
            for i in 0..n {
                if flipped[i] {
                    extra[i] = fold_w;
                }
            }
            eprintln!("iter={iter} flipped_tris={tri_cnt}");
        }

        let mut rot = vec![[[0.0f32; 3]; 3]; n];
        for i in 0..n {
            let mut s = [[0.0f32; 3]; 3];
            for &j in &nbrs[i] {
                let a = sub(v0[i], v0[j as usize]);
                let b = sub(v[i], v[j as usize]);
                for r in 0..3 {
                    for c in 0..3 {
                        s[r][c] += a[r] * b[c];
                    }
                }
            }
            rot[i] = polar_rotation(s);
        }

        let mut b = vec![[0.0f32; 3]; n];
        for &(i, j) in &edges {
            let e = sub(v0[i as usize], v0[j as usize]);
            let q = scale(add(mul3(rot[i as usize], e), mul3(rot[j as usize], e)), 0.5);
            b[i as usize] = add(b[i as usize], q);
            b[j as usize] = sub(b[j as usize], q);
        }
        for i in 0..n {
            b[i] = add(b[i], scale(target[i], lambda));
        }
        // 翻转锚项：fold_w * ||v_i - v0_i||²，把翻回去的顶点拉向初始位置。
        for i in 0..n {
            if extra[i] > 0.0 {
                b[i] = add(b[i], scale(v0[i], extra[i]));
            }
        }
        // 体积项：w*(V - Vt)² 线性化后每个顶点减 2w(V-Vt)·∂V/∂v_i。
        if vol_w > 0.0 {
            let (vol_now, g) = volume_and_grad(&v, &rc.tris);
            let gmax = g
                .iter()
                .map(|x| dot(*x, *x).sqrt())
                .fold(0.0f32, f32::max)
                .max(1e-9);
            // 限幅：每个顶点受到的体积力不超过 0.05（与 ARAP 的 b 同量级），避免过冲发散。
            let cap = 0.05 / gmax;
            let coef = (2.0 * vol_w * (vol_now - target_vol)).clamp(-cap, cap);
            for i in 0..n {
                b[i] = sub(b[i], scale(g[i], coef));
            }
            eprintln!("iter={iter} vol={vol_now:.6} target={target_vol:.6} coef={coef:.6}");
        }
        v = cg(&nbrs, &b, lambda, &extra, v, cg_iters);

        // 折叠修复（投影式约束）：只对翻转子顶点做局部 Laplacian，削平双层重叠。
        if repair_iters > 0 {
            for _ in 0..repair_iters {
                let mut bad = vec![false; n];
                for t in &rc.tris {
                    let (a, b, c) = (t[0] as usize, t[1] as usize, t[2] as usize);
                    if tri_flip_metric(v[a], v[b], v[c], v0[a], v0[b], v0[c]) < 0.0 {
                        bad[a] = true;
                        bad[b] = true;
                        bad[c] = true;
                    }
                }
                let old = v.clone();
                for i in 0..n {
                    if !bad[i] || nbrs[i].is_empty() {
                        continue;
                    }
                    let mut sum = [0.0f32; 3];
                    for &j in &nbrs[i] {
                        sum = add(sum, old[j as usize]);
                    }
                    let inv = 1.0 / nbrs[i].len() as f32;
                    for k in 0..3 {
                        v[i][k] = old[i][k] + repair_w * (sum[k] * inv - old[i][k]);
                    }
                }
            }
        }
    }

    let mut out = String::from("# arap\n");
    for p in &v {
        out.push_str(&format!("v {:.6} {:.6} {:.6}\n", p[0], p[1], p[2]));
    }
    for t in &rc.tris {
        out.push_str(&format!("f {} {} {}\n", t[0] + 1, t[1] + 1, t[2] + 1));
    }
    std::fs::write(&out_path, out).expect("写 OBJ");
    println!(
        "{{\"verts\":{},\"tris\":{},\"iters\":{},\"lambda\":{},\"cg\":{},\"fold\":{},\"vol\":{},\"vol_target\":{}}}",
        n,
        rc.tris.len(),
        iters,
        lambda,
        cg_iters,
        fold_w,
        vol_w,
        target_vol
    );
}
