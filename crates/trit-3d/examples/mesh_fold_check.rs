//! 网格折叠/退化检查：给每个三角形算法线，用面积加权顶点法线判定局部翻转，
//! 并统计相邻面折叠（二面角过大）与退化三角形数量。
//!
//! 运行：`cargo run --release -q -p trit-3d --example mesh_fold_check -- <mesh.obj> [mesh2.obj ...]`

use std::collections::HashMap;

use trit_3d::mesh::TriMesh;

fn sub(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

fn cross(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

fn dot(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn norm(a: [f32; 3]) -> [f32; 3] {
    let n = dot(a, a).sqrt();
    if n > 1e-20 {
        [a[0] / n, a[1] / n, a[2] / n]
    } else {
        [0.0, 0.0, 0.0]
    }
}

fn report(path: &str) {
    let m = match TriMesh::from_obj(std::path::Path::new(path)) {
        Ok(m) => m,
        Err(e) => {
            println!("{{\"path\":\"{path}\",\"error\":\"{e}\"}}");
            return;
        }
    };
    let nv = m.verts.len();
    let nt = m.tris.len();

    let mut fnorm = Vec::with_capacity(nt);
    let mut farea = Vec::with_capacity(nt);
    let mut vnorm = vec![[0.0f32; 3]; nv];
    let mut degenerate = 0usize;
    let mut areas: Vec<f32> = Vec::with_capacity(nt);

    for t in &m.tris {
        let a = m.verts[t[0] as usize];
        let b = m.verts[t[1] as usize];
        let c = m.verts[t[2] as usize];
        let n = cross(sub(b, a), sub(c, a));
        let twice = dot(n, n).sqrt();
        let area = 0.5 * twice;
        if area < 1e-10 {
            degenerate += 1;
        }
        let unit = norm(n);
        fnorm.push(unit);
        farea.push(area);
        areas.push(area);
        for k in 0..3 {
            let v = t[k] as usize;
            for j in 0..3 {
                vnorm[v][j] += unit[j] * area;
            }
        }
    }

    // 三角形法线 vs 该三角形三顶点面积加权法线平均：点积 < 0 视为局部翻转。
    let mut flipped = 0usize;
    for (i, t) in m.tris.iter().enumerate() {
        let mut acc = [0.0f32; 3];
        for k in 0..3 {
            let v = t[k] as usize;
            for j in 0..3 {
                acc[j] += vnorm[v][j];
            }
        }
        if dot(fnorm[i], norm(acc)) < 0.0 {
            flipped += 1;
        }
    }

    // 相邻面折叠：同一条边两侧面法线点积 < -0.5（>120°）。
    let mut edge_faces: HashMap<(u32, u32), Vec<usize>> = HashMap::new();
    for (i, t) in m.tris.iter().enumerate() {
        for k in 0..3 {
            let a = t[k];
            let b = t[(k + 1) % 3];
            let key = if a < b { (a, b) } else { (b, a) };
            edge_faces.entry(key).or_default().push(i);
        }
    }
    let mut folded = 0usize;
    let mut total_pairs = 0usize;
    for list in edge_faces.values() {
        for x in 0..list.len() {
            for y in (x + 1)..list.len() {
                total_pairs += 1;
                if dot(fnorm[list[x]], fnorm[list[y]]) < -0.5 {
                    folded += 1;
                }
            }
        }
    }

    areas.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let median = if areas.is_empty() { 0.0 } else { areas[areas.len() / 2] };
    let amin = areas.first().copied().unwrap_or(0.0);
    let amax = areas.last().copied().unwrap_or(0.0);
    let total: f32 = areas.iter().sum();

    println!(
        "{{\"path\":\"{path}\",\"verts\":{nv},\"tris\":{nt},\"degenerate\":{degenerate},\
\"flipped\":{flipped},\"flipped_ratio\":{:.6},\"folded\":{folded},\"folded_ratio\":{:.6},\
\"area_total\":{:.6},\"area_min\":{:.8},\"area_median\":{:.8},\"area_max\":{:.8}}}",
        flipped as f32 / nt.max(1) as f32,
        folded as f32 / total_pairs.max(1) as f32,
        total,
        amin,
        median,
        amax
    );
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.is_empty() {
        eprintln!("用法: mesh_fold_check <mesh.obj> [mesh2.obj ...]");
        std::process::exit(2);
    }
    for p in &args {
        report(p);
    }
}
