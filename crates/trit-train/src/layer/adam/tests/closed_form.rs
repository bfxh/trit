use crate::layer::{Adam, TernaryLinear};
use crate::quant::Ste;

#[test]
fn adam_first_step_matches_closed_form() {
    let mut layer = TernaryLinear::new(1, 1, |_, _| 1.0, Ste::Identity, 0.0, 1.0);
    let mut latent_opt = Adam::new(1, 0.1);
    let mut scale_opt = Adam::new(1, 0.0);
    // 输入 1、输出梯度 2 ⇒ ∂L/∂latent = 2；第一步 mhat=2、vhat=4。
    layer.train_step_with_grad_adam(&[1.0], &[2.0], &mut latent_opt, &mut scale_opt);
    let got = layer.latent()[0];
    assert!(
        (got - 0.9).abs() < 1e-5,
        "latent={got}（偏置校正后应为 1 - 0.1 = 0.9）"
    );
    // 直接钉住 `Adam::apply` 的偏置校正；否则 latent 改走 adam_step 后，
    // 这条变异只影响 scale（lr=0）而不被看见。
    let mut direct_opt = Adam::new(1, 0.1);
    direct_opt.begin();
    let direct = direct_opt.apply(0, 2.0, 1.0);
    assert!(
        (direct - 0.9).abs() < 1e-5,
        "Adam::apply={direct}（偏置校正后应为 0.9）"
    );
}
