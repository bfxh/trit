use super::*;

/// `cell` 小到 1e-4 时，一个 1 米的包围盒会算出 10005³ ≈ 1e15 个体素；
/// 修复前 `vec![0u8; dims[0]*dims[1]*dims[2]]` 直接 abort，`margin` 更是连乘项。
#[test]
fn huge_grid_is_rejected_before_allocating() {
    let pts = [[0.0, 0.0, 0.0], [1.0, 1.0, 1.0]];
    assert!(Grid::from_points(&pts, 1e-4, 2).is_none());
}

/// 正常尺寸不受影响：60M 体素上限不该误伤 15³ 这种真实栅格。
#[test]
fn normal_grid_still_builds() {
    let pts = [[0.0, 0.0, 0.0], [1.0, 1.0, 1.0]];
    let g = Grid::from_points(&pts, 0.1, 2).expect("应有栅格");
    assert_eq!(g.occ.len(), g.nx * g.ny * g.nz);
}
