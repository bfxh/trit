//! dimsweep 的宽度扫描（从 `main.rs` 拆出）。

use crate::basis::{Sample, oracle_of, reduced_net};
use crate::evaluate::{agreement_with_full, identity, losses};
use crate::judge::{Teacher, f32_reduced_accuracy};
use crate::options::Args;
use serde_json::{Value, json};
use trit_wspace::Net;
use trit_wspace::safetensors::Weights;

pub struct CurveCtx<'a> {
    pub args: &'a Args,
    pub query: &'a Weights,
    pub readout: &'a [f32],
    pub second: Option<&'a [f32]>,
    pub kind: Teacher,
    pub dim: usize,
    pub comps: &'a [Vec<f64>],
    pub student: &'a Net,
    pub samples: &'a [Sample],
    pub edit: &'a [Sample],
    pub keep: &'a [Sample],
    /// 判据是否改成「与全维学生一致」（`--criterion full`）。
    pub criterion_full: bool,
}

pub fn width_curve(cx: &CurveCtx) -> (Vec<Value>, Option<usize>, (usize, f32), Vec<Value>) {
    let args = cx.args;
    let query = cx.query;
    let readout = cx.readout;
    let second = cx.second;
    let teacher_kind = cx.kind;
    let dim = cx.dim;
    let comps = cx.comps;
    let student = cx.student;
    let samples = cx.samples;
    let edit_samples = cx.edit;
    let keep_samples = cx.keep;
    let mut width = Vec::new();
    let mut min_k: Option<usize> = None;
    let mut best_agree = (0usize, 0.0f32);
    let mut agree_pairs: Vec<(usize, f32)> = Vec::new();
    for &k in &args.dims {
        let basis = if k >= dim {
            identity(dim)
        } else if k <= comps.len() {
            comps[..k].to_vec()
        } else {
            continue;
        };
        let net = if k >= dim {
            student.clone()
        } else {
            reduced_net(query, readout, dim, &basis)
        };
        let edit = oracle_of(edit_samples, &basis);
        let keep = oracle_of(keep_samples, &basis);
        let (edit_loss, keep_loss) = losses(&net, &edit, &keep);

        let (f32_edit_acc, f32_keep_acc) = if k >= dim {
            (1.0f32, 1.0f32)
        } else {
            (
                f32_reduced_accuracy(
                    edit_samples,
                    &basis,
                    query,
                    readout,
                    second,
                    dim,
                    teacher_kind,
                ),
                f32_reduced_accuracy(
                    keep_samples,
                    &basis,
                    query,
                    readout,
                    second,
                    dim,
                    teacher_kind,
                ),
            )
        };
        // 全量样本上的一致性：区分「压缩了原学生」与「换成了教师」。
        let agree_full = if k >= dim {
            1.0
        } else {
            agreement_with_full(&net, samples, &basis, student)
        };
        agree_pairs.push((k, agree_full));
        if agree_full > best_agree.1 {
            best_agree = (k, agree_full);
        }
        // `--criterion full` 把「过判据」定义成「与全维学生完全一致」。
        let passed = if cx.criterion_full {
            agree_full >= 1.0
        } else {
            edit_loss == 0 && keep_loss == 0
        };
        if passed && min_k.is_none_or(|best| k < best) {
            min_k = Some(k);
        }
        width.push(json!({
            "k": k,
            "edit_loss": edit_loss,
            "keep_loss": keep_loss,
            "edit_acc": edit.accuracy(&net).expect("acc"),
            "keep_acc": keep.accuracy(&net).expect("acc"),
            "f32_edit_acc": f32_edit_acc,
            "f32_keep_acc": f32_keep_acc,
            "agree_full": agree_full,
            "trits": net.trit_count(),
            "reduction": net.trit_count() as f64 / student.trit_count() as f64,
        }));
    }
    (width, min_k, best_agree, crate::tau::tau_curve(&agree_pairs))
}
