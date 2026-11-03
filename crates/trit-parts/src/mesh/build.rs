use std::collections::VecDeque;

#[derive(Default, Clone, Debug)]
pub struct Mesh {
    pub verts: Vec<[f32; 3]>,
    pub tris: Vec<[u32; 3]>,
}

/// 占用栅格：索引 `x + nx*(y + ny*z)`。
#[derive(Clone, Debug)]
pub struct Grid {
    pub occ: Vec<u8>,
    pub nx: usize,
    pub ny: usize,
    pub nz: usize,
    /// 格子 0 号角点的世界坐标。
    pub origin: [f32; 3],
    /// 格子边长（米）。
    pub cell: f32,
}

impl Grid {
    pub fn idx(&self, x: usize, y: usize, z: usize) -> usize {
        x + self.nx * (y + self.ny * z)
    }

    pub fn get(&self, x: usize, y: usize, z: usize) -> u8 {
        self.occ[self.idx(x, y, z)]
    }

    /// 从点云建占用栅格：点落在哪格就把哪格置 1；`margin` 是四周留的空格数。
    pub fn from_points(points: &[[f32; 3]], cell: f32, margin: usize) -> Option<Grid> {
        if points.is_empty() {
            return None;
        }
        let mut lo = [f32::INFINITY; 3];
        let mut hi = [f32::NEG_INFINITY; 3];
        for p in points {
            for a in 0..3 {
                lo[a] = lo[a].min(p[a]);
                hi[a] = hi[a].max(p[a]);
            }
        }
        let cell = cell.max(1e-4);
        let dims = [0, 1, 2].map(|a| ((hi[a] - lo[a]) / cell).ceil() as usize + 1 + 2 * margin);
        let origin = [
            lo[0] - margin as f32 * cell,
            lo[1] - margin as f32 * cell,
            lo[2] - margin as f32 * cell,
        ];
        let mut g = Grid {
            occ: vec![0u8; dims[0] * dims[1] * dims[2]],
            nx: dims[0],
            ny: dims[1],
            nz: dims[2],
            origin,
            cell,
        };
        for p in points {
            let c = [0, 1, 2].map(|a| {
                (((p[a] - origin[a]) / cell).round() as isize).clamp(0, dims[a] as isize - 1)
                    as usize
            });
            let i = g.idx(c[0], c[1], c[2]);
            g.occ[i] = 1;
        }
        Some(g)
    }
}

/// 把点云外壳填成实体：从**外表面**泛洪标记「外部」，剩下的空格全算内部。
/// 边界按「外面」处理（栅格四周一定连通到外部）。
pub fn fill_solid(g: &mut Grid) {
    let (nx, ny, nz) = (g.nx, g.ny, g.nz);
    let mut outside = vec![false; g.occ.len()];
    let mut q: VecDeque<usize> = VecDeque::new();
    let push = |i: usize, outside: &mut Vec<bool>, q: &mut VecDeque<usize>| {
        if g.occ[i] == 0 && !outside[i] {
            outside[i] = true;
            q.push_back(i);
        }
    };
    for z in 0..nz {
        for y in 0..ny {
            for x in 0..nx {
                let on_edge =
                    x == 0 || y == 0 || z == 0 || x + 1 == nx || y + 1 == ny || z + 1 == nz;
                if on_edge {
                    push(g.idx(x, y, z), &mut outside, &mut q);
                }
            }
        }
    }
    while let Some(i) = q.pop_front() {
        let x = i % nx;
        let y = (i / nx) % ny;
        let z = i / (nx * ny);
        if x > 0 {
            push(i - 1, &mut outside, &mut q);
        }
        if x + 1 < nx {
            push(i + 1, &mut outside, &mut q);
        }
        if y > 0 {
            push(i - nx, &mut outside, &mut q);
        }
        if y + 1 < ny {
            push(i + nx, &mut outside, &mut q);
        }
        if z > 0 {
            push(i - nx * ny, &mut outside, &mut q);
        }
        if z + 1 < nz {
            push(i + nx * ny, &mut outside, &mut q);
        }
    }
    for (i, o) in outside.iter().enumerate() {
        if !*o {
            g.occ[i] = 1;
        }
    }
}

/// 占用栅格膨胀 `r` 格（26 邻域，封住体素化小孔）。
pub fn dilate_occ(g: &mut Grid, r: usize) {
    for _ in 0..r {
        let src = g.occ.clone();
        let (nx, ny, nz) = (g.nx, g.ny, g.nz);
        for z in 0..nz {
            for y in 0..ny {
                for x in 0..nx {
                    if src[x + nx * (y + ny * z)] != 0 {
                        continue;
                    }
                    let mut hit = false;
                    'nb: for dz in -1i32..=1 {
                        for dy in -1i32..=1 {
                            for dx in -1i32..=1 {
                                let (xx, yy, zz) = (x as i32 + dx, y as i32 + dy, z as i32 + dz);
                                if xx < 0
                                    || yy < 0
                                    || zz < 0
                                    || xx >= nx as i32
                                    || yy >= ny as i32
                                    || zz >= nz as i32
                                {
                                    continue;
                                }
                                if src[xx as usize + nx * (yy as usize + ny * zz as usize)] != 0 {
                                    hit = true;
                                    break 'nb;
                                }
                            }
                        }
                    }
                    if hit {
                        g.occ[x + nx * (y + ny * z)] = 1;
                    }
                }
            }
        }
    }
}

/// 消除「只靠对角/棱接触」的实心体素：每个 2×2×2 块里若只有两个对角实心，
/// 补上另两个中的一个，避免 Surface Nets 在棱/角处产生非流形边。
pub fn fix_diagonals(g: &mut Grid) {
    let (nx, ny, nz) = (g.nx, g.ny, g.nz);
    let src = g.occ.clone();
    for z in 0..nz.saturating_sub(1) {
        for y in 0..ny.saturating_sub(1) {
            for x in 0..nx.saturating_sub(1) {
                let mut cnt = 0;
                for dz in 0..2 {
                    for dy in 0..2 {
                        for dx in 0..2 {
                            cnt += src[x + dx + nx * (y + dy + ny * (z + dz))] as i32;
                        }
                    }
                }
                if cnt != 2 {
                    continue;
                }
                // 对角对判定：两两对角
                let idx = |dx: usize, dy: usize, dz: usize| x + dx + nx * (y + dy + ny * (z + dz));
                let v = |dx, dy, dz| src[idx(dx, dy, dz)] != 0;
                let pairs = [
                    (v(0, 0, 0) && v(1, 1, 1), [(1, 0, 0), (0, 1, 0), (0, 0, 1)]),
                    (v(1, 0, 0) && v(0, 1, 1), [(0, 0, 0), (0, 1, 0), (0, 0, 1)]),
                    (v(0, 1, 0) && v(1, 0, 1), [(1, 1, 0), (1, 0, 0), (0, 0, 0)]),
                    (v(0, 0, 1) && v(1, 1, 0), [(1, 0, 1), (1, 0, 0), (0, 1, 1)]),
                ];
                for (yes, fills) in pairs {
                    if yes {
                        let (fdx, fdy, fdz) = fills[0];
                        g.occ[idx(fdx, fdy, fdz)] = 1;
                        break;
                    }
                }
            }
        }
    }
}

/// Surface Nets：对实心栅格提等值面（iso = 0.5）。
/// 每个跨面的格子放一个顶点（边交点均值），每个跨面的栅格棱生成一对三角形。
pub fn surface_nets(g: &Grid) -> Mesh {
    let (nx, ny, nz) = (g.nx, g.ny, g.nz);
    if nx < 2 || ny < 2 || nz < 2 {
        return Mesh::default();
    }
    let (cx, cy, cz) = (nx - 1, ny - 1, nz - 1);
    let mut vid = vec![u32::MAX; cx * cy * cz];
    let cidx = |x: usize, y: usize, z: usize| x + cx * (y + cy * z);
    let corner = |x: usize, y: usize, z: usize| g.get(x, y, z) as f32;

    let mut m = Mesh::default();
    for z in 0..cz {
        for y in 0..cy {
            for x in 0..cx {
                let mut v = [0.0f32; 8];
                for (i, (dx, dy, dz)) in [
                    (0, 0, 0),
                    (1, 0, 0),
                    (0, 1, 0),
                    (1, 1, 0),
                    (0, 0, 1),
                    (1, 0, 1),
                    (0, 1, 1),
                    (1, 1, 1),
                ]
                .iter()
                .enumerate()
                {
                    v[i] = corner(x + dx, y + dy, z + dz);
                }
                let inside = v.iter().filter(|&&c| c > 0.5).count();
                if inside == 0 || inside == 8 {
                    continue;
                }
                // 12 条棱（角点编号见上表）
                const EDGES: [(usize, usize); 12] = [
                    (0, 1),
                    (2, 3),
                    (4, 5),
                    (6, 7),
                    (0, 2),
                    (1, 3),
                    (4, 6),
                    (5, 7),
                    (0, 4),
                    (1, 5),
                    (2, 6),
                    (3, 7),
                ];
                const POS: [[f32; 3]; 8] = [
                    [0.0, 0.0, 0.0],
                    [1.0, 0.0, 0.0],
                    [0.0, 1.0, 0.0],
                    [1.0, 1.0, 0.0],
                    [0.0, 0.0, 1.0],
                    [1.0, 0.0, 1.0],
                    [0.0, 1.0, 1.0],
                    [1.0, 1.0, 1.0],
                ];
                let mut acc = [0.0f32; 3];
                let mut n = 0f32;
                for (a, b) in EDGES {
                    let (va, vb) = (v[a], v[b]);
                    if (va > 0.5) == (vb > 0.5) {
                        continue;
                    }
                    let t = (0.5 - va) / (vb - va);
                    let p = [
                        POS[a][0] + t * (POS[b][0] - POS[a][0]),
                        POS[a][1] + t * (POS[b][1] - POS[a][1]),
                        POS[a][2] + t * (POS[b][2] - POS[a][2]),
                    ];
                    for k in 0..3 {
                        acc[k] += p[k];
                    }
                    n += 1.0;
                }
                if n == 0.0 {
                    continue;
                }
                let local = [acc[0] / n, acc[1] / n, acc[2] / n];
                vid[cidx(x, y, z)] = m.verts.len() as u32;
                m.verts.push([
                    g.origin[0] + (x as f32 + local[0]) * g.cell,
                    g.origin[1] + (y as f32 + local[1]) * g.cell,
                    g.origin[2] + (z as f32 + local[2]) * g.cell,
                ]);
            }
        }
    }

    // 每条跨面的栅格棱 → 周围 4 个格子的顶点组成一个四边形
    let getv = |vid: &[u32], x: isize, y: isize, z: isize| -> Option<u32> {
        if x < 0 || y < 0 || z < 0 || x as usize >= cx || y as usize >= cy || z as usize >= cz {
            return None;
        }
        let v = vid[x as usize + cx * (y as usize + cy * z as usize)];
        if v == u32::MAX { None } else { Some(v) }
    };
    let mut quad = |a: Option<u32>, b: Option<u32>, c: Option<u32>, d: Option<u32>, flip: bool| {
        if let (Some(a), Some(b), Some(c), Some(d)) = (a, b, c, d) {
            if flip {
                m.tris.push([a, b, c]);
                m.tris.push([a, c, d]);
            } else {
                m.tris.push([a, d, c]);
                m.tris.push([a, c, b]);
            }
        }
    };
    for z in 0..nz {
        for y in 0..ny {
            for x in 0..nx {
                let here = g.get(x, y, z);
                // x 向棱
                if x + 1 < nx {
                    let there = g.get(x + 1, y, z);
                    if (here > 0) != (there > 0) {
                        let (xi, yi, zi) = (x as isize, y as isize, z as isize);
                        quad(
                            getv(&vid, xi, yi - 1, zi - 1),
                            getv(&vid, xi, yi, zi - 1),
                            getv(&vid, xi, yi, zi),
                            getv(&vid, xi, yi - 1, zi),
                            here > 0,
                        );
                    }
                }
                // y 向棱
                if y + 1 < ny {
                    let there = g.get(x, y + 1, z);
                    if (here > 0) != (there > 0) {
                        let (xi, yi, zi) = (x as isize, y as isize, z as isize);
                        quad(
                            getv(&vid, xi - 1, yi, zi - 1),
                            getv(&vid, xi - 1, yi, zi),
                            getv(&vid, xi, yi, zi),
                            getv(&vid, xi, yi, zi - 1),
                            here > 0,
                        );
                    }
                }
                // z 向棱
                if z + 1 < nz {
                    let there = g.get(x, y, z + 1);
                    if (here > 0) != (there > 0) {
                        let (xi, yi, zi) = (x as isize, y as isize, z as isize);
                        quad(
                            getv(&vid, xi - 1, yi - 1, zi),
                            getv(&vid, xi, yi - 1, zi),
                            getv(&vid, xi, yi, zi),
                            getv(&vid, xi - 1, yi, zi),
                            here > 0,
                        );
                    }
                }
            }
        }
    }
    m
}

/// 占用栅格降采样（factor 倍，任意格为实则该粗格为实）。返回新栅格 + 粗格边长。
pub fn downsample_occupancy(g: &Grid, factor: usize) -> Grid {
    let f = factor.max(1);
    let (nx, ny, nz) = (g.nx.div_ceil(f), g.ny.div_ceil(f), g.nz.div_ceil(f));
    let mut occ = vec![0u8; nx * ny * nz];
    for z in 0..g.nz {
        for y in 0..g.ny {
            for x in 0..g.nx {
                if g.get(x, y, z) == 0 {
                    continue;
                }
                let (cx, cy, cz) = (x / f, y / f, z / f);
                occ[cx + nx * (cy + ny * cz)] = 1;
            }
        }
    }
    Grid {
        occ,
        nx,
        ny,
        nz,
        origin: g.origin,
        cell: g.cell * f as f32,
    }
}

/// 顶点聚类简化：把落入同一粗格的顶点并成一个（取均值），退化的三角面丢弃。
pub fn simplify_cluster(m: &Mesh, cell: f32) -> Mesh {
    use std::collections::HashMap;
    let cell = cell.max(1e-6);
    let key = |p: &[f32; 3]| {
        (
            (p[0] / cell).floor() as i64,
            (p[1] / cell).floor() as i64,
            (p[2] / cell).floor() as i64,
        )
    };
    let mut map: HashMap<(i64, i64, i64), u32> = HashMap::new();
    let mut acc: Vec<[f64; 3]> = Vec::new();
    let mut cnt: Vec<u32> = Vec::new();
    let mut remap = vec![0u32; m.verts.len()];
    for (i, p) in m.verts.iter().enumerate() {
        let k = key(p);
        let id = *map.entry(k).or_insert_with(|| {
            acc.push([0.0; 3]);
            cnt.push(0);
            (acc.len() - 1) as u32
        });
        for a in 0..3 {
            acc[id as usize][a] += p[a] as f64;
        }
        cnt[id as usize] += 1;
        remap[i] = id;
    }
    let verts: Vec<[f32; 3]> = acc
        .iter()
        .zip(&cnt)
        .map(|(s, c)| [0, 1, 2].map(|a| (s[a] / *c as f64) as f32))
        .collect();
    let mut tris = Vec::with_capacity(m.tris.len());
    for t in &m.tris {
        let (a, b, c) = (
            remap[t[0] as usize],
            remap[t[1] as usize],
            remap[t[2] as usize],
        );
        if a != b && b != c && a != c {
            tris.push([a, b, c]);
        }
    }
    Mesh { verts, tris }
}

/// 网格体积（散度定理，面按逆时针朝外时为正）。
pub fn mesh_volume(m: &Mesh) -> f64 {
    let mut v = 0.0f64;
    for t in &m.tris {
        let a = m.verts[t[0] as usize];
        let b = m.verts[t[1] as usize];
        let c = m.verts[t[2] as usize];
        let (ax, ay, az) = (a[0] as f64, a[1] as f64, a[2] as f64);
        let (bx, by, bz) = (b[0] as f64, b[1] as f64, b[2] as f64);
        let (cx, cy, cz) = (c[0] as f64, c[1] as f64, c[2] as f64);
        v += ax * (by * cz - bz * cy) - ay * (bx * cz - bz * cx) + az * (bx * cy - by * cx);
    }
    v / 6.0
}

/// 水密检查：每条无向边恰好被两个三角面共享（返回违例边数）。
pub fn non_manifold_edges(m: &Mesh) -> usize {
    use std::collections::HashMap;
    let mut cnt: HashMap<(u32, u32), usize> = HashMap::new();
    for t in &m.tris {
        for (a, b) in [(t[0], t[1]), (t[1], t[2]), (t[2], t[0])] {
            let e = if a < b { (a, b) } else { (b, a) };
            *cnt.entry(e).or_insert(0) += 1;
        }
    }
    cnt.values().filter(|&&c| c != 2).count()
}

/// 边界边数：只被 1 个三角面使用的无向边。**= 0 才是“没有洞”的闭合网格**。
/// （非流形边 >0 表示 4 个面共边——naive Surface Nets 在复杂体上必然出现，不等于有洞。）
pub fn boundary_edges(m: &Mesh) -> usize {
    use std::collections::HashMap;
    let mut cnt: HashMap<(u32, u32), usize> = HashMap::new();
    for t in &m.tris {
        for (a, b) in [(t[0], t[1]), (t[1], t[2]), (t[2], t[0])] {
            let e = if a < b { (a, b) } else { (b, a) };
            *cnt.entry(e).or_insert(0) += 1;
        }
    }
    cnt.values().filter(|&&c| c == 1).count()
}
