//! dimsweep 的评估部分：f32 对照、与全维学生的一致性、双 oracle 违规计数。
//!
//! 从 `basis.rs` 拆出：module_budget 台账只收紧、不抬高单文件行数。

use crate::basis::{Sample, oracle_of, project, reduced_net, trit_row};
use serde_json::{Value, json};
use trit_wspace::safetensors::Weights;
use trit_wspace::{Net, Oracle};

/// 降维学生与全维三值学生在同一批样本上的一致率。
///
/// 这是「零违规」的含义检查：若 k=1 时一致率远低于 1，说明降维学生其实退化成了 f32 教师
/// （线性判据），而不是压缩了原来的三值学生。必须在**全量样本**上算，不能在 edit/keep
/// 子集上算——那两个子集正是按「与教师是否一致」挑出来的。
pub fn agreement_with_full(
    reduced: &Net,
    samples: &[Sample],
    basis: &[Vec<f64>],
    full: &Net,
) -> f32 {
    if samples.is_empty() {
        return 0.0;
    }
    let hit = samples
        .iter()
        .filter(|s| {
            let a = reduced
                .forward(&trit_row(&project(&s.x, basis)))
                .expect("reduced forward")[0];
            let b = full.forward(&trit_row(&s.x)).expect("full forward")[0];
            a == b
        })
        .count();
    hit as f32 / samples.len() as f32
}

pub fn losses(net: &Net, edit: &Oracle, keep: &Oracle) -> (usize, usize) {
    (
        edit.loss(net).expect("edit loss"),
        keep.loss(net).expect("keep loss"),
    )
}

/// 逐维剔除：去掉第 `i` 个主方向后 edit+keep 的总违规增量。
pub fn necessity_ranking(
    query: &Weights,
    readout: &[f32],
    dim: usize,
    comps: &[Vec<f64>],
    edit_samples: &[Sample],
    keep_samples: &[Sample],
) -> Value {
    if comps.len() < 2 {
        return json!([]);
    }
    let full_net = reduced_net(query, readout, dim, comps);
    let (full_edit_loss, full_keep_loss) = losses(
        &full_net,
        &oracle_of(edit_samples, comps),
        &oracle_of(keep_samples, comps),
    );
    let baseline = full_edit_loss + full_keep_loss;
    let mut rows: Vec<(usize, usize, usize, isize)> = Vec::with_capacity(comps.len());
    for i in 0..comps.len() {
        let sub: Vec<Vec<f64>> = comps
            .iter()
            .enumerate()
            .filter(|(j, _)| *j != i)
            .map(|(_, v)| v.clone())
            .collect();
        let net = reduced_net(query, readout, dim, &sub);
        let (edit_loss, keep_loss) = losses(
            &net,
            &oracle_of(edit_samples, &sub),
            &oracle_of(keep_samples, &sub),
        );
        rows.push((
            i,
            edit_loss,
            keep_loss,
            (edit_loss + keep_loss) as isize - baseline as isize,
        ));
    }
    let mut sorted = rows.clone();
    sorted.sort_by_key(|a| std::cmp::Reverse(a.3));
    let pick = |(i, e, k, d): &(usize, usize, usize, isize)| json!({"component": i, "edit_loss": e, "keep_loss": k, "delta": d});
    let most: Vec<Value> = sorted.iter().take(5).map(pick).collect();
    let least: Vec<Value> = sorted.iter().rev().take(5).map(pick).collect();
    json!({
        "k": comps.len(),
        "baseline_loss": baseline,
        "most_necessary": most,
        "least_necessary": least,
        "all": rows.iter().map(pick).collect::<Vec<_>>(),
    })
}

/// 恒等基（`k == dim` 时就是原全维学生，用原输入）。
pub fn identity(dim: usize) -> Vec<Vec<f64>> {
    (0..dim)
        .map(|i| {
            let mut v = vec![0.0f64; dim];
            v[i] = 1.0;
            v
        })
        .collect()
}
