//! f32 注意力的性质测试。

use trit_wspace::attention_f32::F32Attention;

fn head() -> F32Attention {
    F32Attention::new(
        vec![1.0, 0.0, 0.0, 1.0],
        vec![1.0, 0.0, 0.0, 1.0],
        vec![1.0, 0.0, 0.0, 1.0],
        2,
        2,
    )
    .expect("head")
}

#[test]
fn weights_sum_to_one_and_pick_dominant() {
    let head = head();
    let query = vec![1.0, 0.0];
    let keys = vec![vec![1.0, 0.0], vec![-1.0, 0.0]];
    let values = vec![vec![1.0, 0.0], vec![0.0, 1.0]];
    let trace = head.forward(&query, &keys, &values).expect("forward");
    let sum: f32 = trace.weights.iter().sum();
    assert!((sum - 1.0).abs() < 1e-6, "weights sum {sum}");
    assert_eq!(trace.dominant, 0);
    assert!(trace.weights[0] > trace.weights[1]);
    assert_eq!(trace.output.len(), 2);
}

#[test]
fn equal_scores_give_uniform_weights() {
    let head = head();
    let query = vec![0.0, 0.0];
    let keys = vec![vec![0.0, 0.0], vec![0.0, 0.0]];
    let values = vec![vec![1.0, 0.0], vec![0.0, 1.0]];
    let trace = head.forward(&query, &keys, &values).expect("forward");
    assert!((trace.weights[0] - 0.5).abs() < 1e-6);
    assert!((trace.weights[1] - 0.5).abs() < 1e-6);
}
