use crate::ply::{RawGaussian, activate};

#[test]
fn activation_decodes_log_and_sigmoid() {
    let g = RawGaussian {
        position: [0.0; 3],
        f_dc: [0.0; 3],
        f_rest: vec![],
        opacity_logit: 0.0,
        scale_log: [0.0, 0.0, 0.0],
        // PLY 存储序 [W,X,Y,Z]：恒等四元数是 [1,0,0,0]
        rot_xyzw: [1.0, 0.0, 0.0, 0.0],
    };
    let (scale, rot, opacity) = activate(&g);
    assert!((opacity - 0.5).abs() < 1e-6);
    assert!((scale[0] - 1.0).abs() < 1e-6);
    // 转成 xyzw 后 w 在末位
    assert!((rot[3] - 1.0).abs() < 1e-6);
}
