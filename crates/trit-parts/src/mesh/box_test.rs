use super::*;
#[test]
fn full_inner_box_is_watertight() {
    // 12³ 栅格，内部 [2..10)³ 实，外圈留空：应得到水密盒子
    let n = 12usize;
    let mut occ = vec![0u8; n * n * n];
    for z in 2..10 {
        for y in 2..10 {
            for x in 2..10 {
                occ[x + n * (y + n * z)] = 1;
            }
        }
    }
    let g = Grid {
        occ,
        nx: n,
        ny: n,
        nz: n,
        origin: [0.0; 3],
        cell: 1.0,
    };
    let m = surface_nets(&g);
    assert!(!m.tris.is_empty());
    assert_eq!(non_manifold_edges(&m), 0, "内部实心盒应水密");
    let want = 8.0f64.powi(3);
    let got = mesh_volume(&m);
    assert!(
        (got - want).abs() / want < 0.2,
        "体积 {got:.1} vs {want:.1}"
    );
}
