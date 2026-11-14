//! dimsweep 的步数扫描：在降维学生上跑带 `keep` 约束的 `repair`。
//!
//! 从 `curve.rs` 拆出：module_budget 台账只收紧、不抬高单文件行数。

use crate::basis::{oracle_of, reduced_net};
use crate::ctx::CurveCtx;
use crate::evaluate::losses;
use serde_json::{Value, json};
use trit_wspace::repair::{RepairOptions, repair};

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
