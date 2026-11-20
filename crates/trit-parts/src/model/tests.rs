//! `model` 的测试（从 `model.rs` 下沉）：后端前向的 `res` 契约与 CPU 参考实现在这里。
use super::*;

#[test]
fn refcpu_is_deterministic_and_shaped() {
    // 用随机小模型验证形状与确定性（不依赖权重文件）
    let layers = vec![LayerW {
        e: vec![0.5; 3 * 3 * 3 * 4],
        bias: vec![0.0; 4],
        gamma: vec![1.0; 4],
        beta: vec![0.0; 4],
        cin: 3,
        cout: 4,
        kk: 3,
        relu: true,
        convt: false,
    }];
    let cpu = RefCpu {
        model: TriModel { layers },
    };
    let x = vec![0.1f32; 8 * 8 * 3];
    let y1 = cpu.conv(&x, 8, 8, &cpu.model.layers[0]);
    let y2 = cpu.conv(&x, 8, 8, &cpu.model.layers[0]);
    assert_eq!(y1, y2);
    assert_eq!(y1.len(), 8 * 8 * 4);
}

/// 网络是 U 形三层：`h2 = res/2`、`h4 = res/4` 是整数除法，回溯时上采样 ×2 两次。
/// 所以 res 必须是 4 的整数倍，且不能小于 4（否则 h4 = 0，卷积在空图上跑）。
#[test]
fn backend_res_must_be_a_positive_multiple_of_four() {
    assert!(res_is_backend_usable(4));
    assert!(res_is_backend_usable(64));
    assert!(res_is_backend_usable(384));
    assert!(!res_is_backend_usable(0), "h2/h4 都是 0");
    assert!(!res_is_backend_usable(1), "h4 = 0");
    assert!(!res_is_backend_usable(3), "h4 = 3/4 = 0");
    assert!(!res_is_backend_usable(6), "h2 = 3，上采样两次回不到 6");
    assert!(!res_is_backend_usable(10));
}
