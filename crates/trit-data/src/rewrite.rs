//! 重写闭环：重写后的文本必须通过**保真检查**，再送回同一质量门。
//!
//! 重写器有两种来源：进程内实现（测试用）与外部命令（`CommandRewriter`）。
//! 外部命令的边界写在 `rewrite/command.rs`：stdin 送文本、stdout 收结果，
//! 失败一律由调用方 fail-closed（保留原文入册，不做“失败当成功”的降级）。

use std::collections::BTreeSet;

use crate::gate::{Config, Verdict, evaluate};
use crate::schema::Trajectory;

include!("rewrite/mods.inc");

pub use command::{CommandRewriter, run_command, split_command};
pub use fidelity::{Fidelity, anchors, fidelity};
pub use json::to_json;

pub trait Rewriter {
    fn rewrite(&self, text: &str) -> Result<String, String>;
}

/// 重写整条轨迹的文本字段（thought / observation / final），再跑同一质量门。
///
/// 去重集合只在这一条内生效；要多条共享去重（CLI 行为）用 [`rewrite_trajectory_with`]。
pub fn rewrite_trajectory(
    r: &dyn Rewriter,
    t: &Trajectory,
    cfg: &Config,
) -> Result<(Trajectory, Fidelity, Verdict), String> {
    rewrite_trajectory_with(r, t, cfg, &mut BTreeSet::new())
}

/// 同 [`rewrite_trajectory`]，但 dedup 的 `seen` 由调用方持有（跨记录生效）。
pub fn rewrite_trajectory_with(
    r: &dyn Rewriter,
    t: &Trajectory,
    cfg: &Config,
    seen: &mut BTreeSet<String>,
) -> Result<(Trajectory, Fidelity, Verdict), String> {
    let mut out = t.clone();
    let mut missing = Vec::new();
    let mut ratio_ok = true;
    for step in &mut out.steps {
        if let Some(text) = &step.thought {
            let new = r.rewrite(text)?;
            let f = fidelity(text, &new);
            ratio_ok &= f.length_ratio_ok;
            missing.extend(f.missing);
            step.thought = Some(new);
        }
        if let Some(text) = &step.observation {
            let new = r.rewrite(text)?;
            let f = fidelity(text, &new);
            ratio_ok &= f.length_ratio_ok;
            missing.extend(f.missing);
            step.observation = Some(new);
        }
    }
    if !out.outcome.final_text.is_empty() {
        let new = r.rewrite(&out.outcome.final_text)?;
        let f = fidelity(&t.outcome.final_text, &new);
        ratio_ok &= f.length_ratio_ok;
        missing.extend(f.missing);
        out.outcome.final_text = new;
    }
    let fid = Fidelity {
        ok: missing.is_empty() && ratio_ok,
        missing,
        length_ratio_ok: ratio_ok,
    };
    let v = evaluate(&out, cfg, seen);
    Ok((out, fid, v))
}
