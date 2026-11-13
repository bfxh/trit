//! dimsweep 的两条曲线：宽度扫描与步数扫描。
//!
//! 从 `main.rs` 拆出：module_budget 台账只收紧、不抬高单文件行数。

use crate::basis::{Sample, oracle_of, reduced_net};
use crate::evaluate::{agreement_with_full, identity, losses};
use crate::judge::{Teacher, f32_reduced_accuracy};
use crate::options::Args;
use serde_json::{Value, json};
use trit_wspace::Net;
use trit_wspace::repair::{RepairOptions, repair};
use trit_wspace::safetensors::Weights;

/// 一次扫描需要的全部上下文。
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
}

pub fn width_curve(cx: &CurveCtx) -> (Vec<Value>, Option<usize>) {
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
        if edit_loss == 0 && keep_loss == 0 && min_k.is_none_or(|best| k < best) {
            min_k = Some(k);
        }
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
    (width, min_k)
}

pub fn steps_curve(cx: &CurveCtx, min_k: Option<usize>) -> (Vec<Value>, Value) {
    let args = cx.args;
    let query = cx.query;
    let readout = cx.readout;
    let dim = cx.dim;
    let comps = cx.comps;
    let edit_samples = cx.edit;
    let keep_samples = cx.keep;
    let mut steps = Vec::new();
    let mut minimum: Option<(usize, usize, usize)> = min_k.map(|k| (k, 0, 0));
    for &k in &args.steps_dims {
        if k >= dim || k > comps.len() {
            continue;
        }
        let sub = &comps[..k];
        let net = reduced_net(query, readout, dim, sub);
        let edit = oracle_of(edit_samples, sub);
        let keep = oracle_of(keep_samples, sub);
        for &max_edits in &args.max_edits {
            for &beam in &args.beam_widths {
                let options = RepairOptions {
                    max_edits,
                    beam_width: beam,
                    exhaustive_pairs: 256,
                    exhaustive_states: 0,
                    keep: Some(keep.clone()),
                    ..RepairOptions::default()
                };
                let found = repair(&net, &edit, &options).expect("repair");
                let (edit_loss, keep_loss) = match &found {
                    Some(result) => {
                        let mut edited = net.clone();
                        result.patch.apply(&mut edited).expect("apply patch");
                        losses(&edited, &edit, &keep)
                    }
                    None => losses(&net, &edit, &keep),
                };
                if found.is_some() && edit_loss == 0 && keep_loss == 0 {
                    let key = (k, max_edits, beam);
                    if minimum.is_none_or(|best| key < best) {
                        minimum = Some(key);
                    }
                }
                steps.push(json!({
                    "k": k,
                    "max_edits": max_edits,
                    "beam_width": beam,
                    "found": found.is_some(),
                    "evaluated": found.as_ref().map(|r| r.evaluated),
                    "edit_loss": edit_loss,
                    "keep_loss": keep_loss,
                }));
            }
        }
    }
    let minimum_json = match minimum {
        Some((k, max_edits, beam)) => json!({
            "k": k,
            "max_edits": max_edits,
            "beam_width": beam,
        }),
        None => Value::Null,
    };
    (steps, minimum_json)
}
