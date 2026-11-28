/// 一颗高斯：原始 PLY 数值（未解码 log/sigmoid）。
#[derive(Clone, Debug)]
pub struct RawGaussian {
    pub position: [f32; 3],
    pub f_dc: [f32; 3],
    /// f_rest 展平（SH 阶数由调用方按 N/3 判定），可能为空。
    pub f_rest: Vec<f32>,
    pub opacity_logit: f32,
    pub scale_log: [f32; 3],
    /// xyzw（3DGS 存储口径）。
    pub rot_xyzw: [f32; 4],
}

/// 3DGS 激活：scale = exp(log_scale)，opacity = sigmoid(logit)。
/// rot：Brush 的 PLY rot_0..3 是 **[W,X,Y,Z]** 顺序（brush-serde quant.rs:63-67
/// `w = quat[0]`，raw 导入原样透传到内部 WXYZ 张量）——这里转成 xyzw 供
/// quat_to_mat3 使用。按 XYZW 解读会让所有各向异性 splat 旋转全错
/// （实测：cov2d 非对角元差 61×，整幅涂抹感，PSNR −10dB）。
pub fn activate(g: &RawGaussian) -> ([f32; 3], [f32; 4], f32) {
    let scale = [
        g.scale_log[0].exp(),
        g.scale_log[1].exp(),
        g.scale_log[2].exp(),
    ];
    let opacity = 1.0 / (1.0 + (-g.opacity_logit).exp());
    // PLY rot_0..3 = [W,X,Y,Z] → 转 xyzw: (rot_1, rot_2, rot_3, rot_0)
    let rot_xyzw = [g.rot_xyzw[1], g.rot_xyzw[2], g.rot_xyzw[3], g.rot_xyzw[0]];
    (scale, rot_xyzw, opacity)
}
