//! ICC 配置选择与 profile 结构测试。

use super::profile;
use crate::color::{ColorSpace, Gamut};
fn tags(profile: &[u8]) -> Vec<([u8; 4], usize, usize)> {
    let count = u32::from_be_bytes(profile[128..132].try_into().unwrap()) as usize;
    (0..count)
        .map(|i| {
            let entry = 132 + i * 12;
            (
                profile[entry..entry + 4].try_into().unwrap(),
                u32::from_be_bytes(profile[entry + 4..entry + 8].try_into().unwrap()) as usize,
                u32::from_be_bytes(profile[entry + 8..entry + 12].try_into().unwrap()) as usize,
            )
        })
        .collect()
}
#[test]
fn default_srgb_raw_has_no_icc() {
    assert!(profile(Gamut::Srgb, ColorSpace::Raw).is_none());
    assert!(profile(Gamut::Rec2020, ColorSpace::Srgb).is_none());
}

#[test]
fn standard_combinations_use_v4_profiles() {
    for (gamut, transfer) in [
        (Gamut::Srgb, ColorSpace::Srgb),
        (Gamut::DisplayP3, ColorSpace::Srgb),
        (Gamut::Rec2020, ColorSpace::Bt1886),
    ] {
        let p = profile(gamut, transfer).expect("标准组合应有 ICC");
        assert_eq!(
            p.len() as u32,
            u32::from_be_bytes(p[0..4].try_into().unwrap())
        );
        assert_eq!(&p[36..40], b"acsp");
        assert_eq!(&p[12..16], b"mntr");
        let all = tags(&p);
        for sig in [
            b"desc", b"wtpt", b"chad", b"rXYZ", b"gXYZ", b"bXYZ", b"rTRC",
        ] {
            assert!(all.iter().any(|(name, _, _)| name == sig), "{sig:?}");
        }
    }
    let p3 = profile(Gamut::DisplayP3, ColorSpace::Srgb);
    let srgb = profile(Gamut::Srgb, ColorSpace::Srgb);
    assert_ne!(p3, srgb);
}
#[test]
fn patched_gamma_replaces_shared_curve_only() {
    let base = profile(Gamut::Srgb, ColorSpace::Srgb).unwrap();
    let p = profile(Gamut::Srgb, ColorSpace::Bt1886).unwrap();
    assert_eq!(p.len(), base.len());
    assert_eq!(&p[..128], &base[..128]);
    let curves: Vec<_> = tags(&p)
        .into_iter()
        .filter(|(s, _, _)| s == b"rTRC")
        .collect();
    assert_eq!(curves.len(), 1);
    let (_, off, size) = curves[0];
    assert_eq!(size, 16);
    assert_eq!(
        u32::from_be_bytes(p[off + 12..off + 16].try_into().unwrap()),
        157_286
    );
}
