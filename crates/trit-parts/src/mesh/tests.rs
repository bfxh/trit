use super::*;
fn solid_sphere(n: usize, r: f32) -> Grid {
    let c = (n as f32 - 1.0) / 2.0;
    let mut occ = vec![0u8; n * n * n];
    for z in 0..n {
        for y in 0..n {
            for x in 0..n {
                let d = ((x as f32 - c).powi(2) + (y as f32 - c).powi(2) + (z as f32 - c).powi(2))
                    .sqrt();
                occ[x + n * (y + n * z)] = (d <= r) as u8;
            }
        }
    }
    Grid {
        occ,
        nx: n,
        ny: n,
        nz: n,
        origin: [0.0; 3],
        cell: 1.0,
    }
}

#[test]
fn surface_nets_sphere_is_watertight_and_volume_ok() {
    let n = 40usize;
    let r = 12.0f32;
    let g = solid_sphere(n, r);
    let m = surface_nets(&g);
    assert!(m.tris.len() > 100, "三角面太少：{}", m.tris.len());
    assert_eq!(non_manifold_edges(&m), 0, "球面应水密");
    let want = 4.0 / 3.0 * std::f64::consts::PI * (r as f64).powi(3);
    let got = mesh_volume(&m);
    let rel = (got - want).abs() / want;
    assert!(
        rel < 0.10,
        "体积 {got:.1} vs {want:.1}（相对误差 {rel:.3}）"
    );
}

fn cube() -> Mesh {
    // 单位立方体，8 顶点 12 面（外法线不保证，但拓扑闭合且流形）
    let v = vec![
        [0.0, 0.0, 0.0],
        [1.0, 0.0, 0.0],
        [1.0, 1.0, 0.0],
        [0.0, 1.0, 0.0],
        [0.0, 0.0, 1.0],
        [1.0, 0.0, 1.0],
        [1.0, 1.0, 1.0],
        [0.0, 1.0, 1.0],
    ];
    let t = vec![
        [0, 2, 1],
        [0, 3, 2],
        [4, 5, 6],
        [4, 6, 7],
        [0, 1, 5],
        [0, 5, 4],
        [1, 2, 6],
        [1, 6, 5],
        [2, 3, 7],
        [2, 7, 6],
        [3, 0, 4],
        [3, 4, 7],
    ];
    Mesh { verts: v, tris: t }
}

fn slab(nx: usize, ny: usize, nz: usize, lo: usize, hi: usize) -> Vec<u8> {
    // z ∈ [lo, hi) 的厚板
    let mut o = vec![0u8; nx * ny * nz];
    for z in lo..hi {
        for y in 0..ny {
            for x in 0..nx {
                o[x + nx * (y + ny * z)] = 1;
            }
        }
    }
    o
}

#[test]
fn open_removes_one_voxel_sheet_but_keeps_thick_slabs() {
    let (nx, ny, nz) = (9usize, 9usize, 9usize);
    let thin = slab(nx, ny, nz, 4, 5); // 1 体素厚
    let thick = slab(nx, ny, nz, 2, 7); // 5 体素厚
    for r in [1usize, 2] {
        let o = open_solid(&thin, nx, ny, nz, r);
        assert_eq!(
            o.iter().filter(|&&v| v != 0).count(),
            0,
            "r={r} 应删掉 1 体素薄板"
        );
        let o = open_solid(&thick, nx, ny, nz, r);
        assert!(
            o.iter().filter(|&&v| v != 0).count() > 0,
            "r={r} 不该把 5 体素厚板删没"
        );
    }
}

#[test]
fn opening_is_idempotent() {
    let (nx, ny, nz) = (12usize, 12usize, 12usize);
    let mut occ = slab(nx, ny, nz, 2, 10);
    occ[5 + nx * (5 + ny * 5)] = 0; // 挖个单点洞
    let a = open_solid(&occ, nx, ny, nz, 1);
    let b = open_solid(&a, nx, ny, nz, 1);
    assert_eq!(a, b, "开运算应幂等");
    assert!(a.iter().any(|&v| v != 0), "不该把厚板删没");
}

#[test]
fn clean_is_noop_on_a_clean_mesh() {
    let m = cube();
    let (out, st) = clean(&m);
    assert_eq!(st.nonmanifold_before, 0);
    assert_eq!(st.boundary_before, 0);
    assert_eq!(st.degenerate, 0);
    assert_eq!(st.duplicate, 0);
    assert_eq!(out.tris.len(), 12, "干净网格不该被改动");
    assert_eq!(out.verts.len(), 8, "不该凭空复制顶点");
    assert_eq!(st.nonmanifold_after, 0);
    assert_eq!(st.boundary_after, 0);
}

#[test]
fn clean_drops_degenerate_and_duplicate_faces() {
    let mut m = cube();
    m.tris.push([0, 0, 1]); // 退化（重复索引）
    m.tris.push([0, 2, 1]); // 与 [0,2,1] 重复
    let (out, st) = clean(&m);
    assert_eq!(st.degenerate, 1);
    assert_eq!(st.duplicate, 1);
    assert_eq!(out.tris.len(), 12);
    assert_eq!(st.nonmanifold_after, 0);
    assert_eq!(st.boundary_after, 0, "丢掉的是多余面，不应留洞");
}

#[test]
fn clean_splits_a_pinched_edge() {
    // 三个面共用 ab 边：>=3 面 = 非流形；分裂后该边不再被 3 个面共用
    let m = Mesh {
        verts: vec![
            [0.0, 0.0, 0.0],
            [1.0, 0.0, 0.0],
            [0.0, 1.0, 0.0],
            [0.0, -1.0, 0.0],
            [0.0, 0.0, 1.0],
        ],
        tris: vec![[0, 1, 2], [0, 1, 3], [0, 1, 4]],
    };
    assert_eq!(super::multiface_edges(&m), 1);
    let (out, st) = clean(&m);
    assert_eq!(st.nonmanifold_after, 0, "分裂后不应再有非流形边");
    assert_eq!(out.tris.len(), 3, "不应丢面");
    assert!(
        st.boundary_after > 0,
        "相切的第三片面会变成开口（如实记录）"
    );
}

#[test]
fn fill_solid_fills_hollow_shell() {
    // 挖空球壳：只留表面一层；填实后的体积应接近实心球
    let n = 32usize;
    let mut g = solid_sphere(n, 10.0);
    let shell_only = {
        let mut o = vec![0u8; g.occ.len()];
        for z in 1..n - 1 {
            for y in 1..n - 1 {
                for x in 1..n - 1 {
                    let i = g.idx(x, y, z);
                    let nb_empty = [
                        g.idx(x - 1, y, z),
                        g.idx(x + 1, y, z),
                        g.idx(x, y - 1, z),
                        g.idx(x, y + 1, z),
                        g.idx(x, y, z - 1),
                        g.idx(x, y, z + 1),
                    ]
                    .iter()
                    .any(|&j| g.occ[j] == 0);
                    if g.occ[i] == 1 && nb_empty {
                        o[i] = 1;
                    }
                }
            }
        }
        o
    };
    g.occ = shell_only;
    fill_solid(&mut g);
    let solid = solid_sphere(n, 10.0);
    let a: u32 = g.occ.iter().map(|&v| v as u32).sum();
    let b: u32 = solid.occ.iter().map(|&v| v as u32).sum();
    assert!(
        (a as f64 - b as f64).abs() / (b as f64) < 0.05,
        "填实 {a} vs 实心 {b}"
    );
}

#[test]
fn downsample_halves_resolution_and_keeps_solid() {
    let g = solid_sphere(33, 10.0);
    let d = downsample_occupancy(&g, 2);
    assert_eq!((d.nx, d.ny, d.nz), (17, 17, 17));
    assert!((d.cell - 2.0).abs() < 1e-6);
    assert!(d.occ.contains(&1), "降采样后不能全空");
    let m = surface_nets(&d);
    assert!(m.tris.len() > 50);
}

#[test]
fn cluster_simplify_reduces_triangles_and_stays_closed() {
    let g = solid_sphere(40, 12.0);
    let m = surface_nets(&g);
    let s = simplify_cluster(&m, 2.0);
    assert!(s.tris.len() < m.tris.len(), "粗暴聚合应减少三角面");
    assert_eq!(
        non_manifold_edges(&s),
        0,
        "聚合后仍应闭合（这块球没有细结构）"
    );
}
