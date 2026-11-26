//! `PlaneMatrix::q()` 的惰性缓存回归。

use super::PlaneMatrix;

#[test]
fn q_is_lazy_and_cached() {
    let m = PlaneMatrix::from_latent(&[0.4, -0.6, 1.2, -1.9], &[1.0, 1.0], 2, 2);
    assert!(m.q.get().is_none());
    assert_eq!(m.q(), &[0i8, -1, 1, -1][..]);
    assert!(m.q.get().is_some());
    assert_eq!(m.q().as_ptr(), m.q().as_ptr());
}
