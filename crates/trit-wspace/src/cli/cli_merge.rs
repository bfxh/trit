//! CLI `merge`：投票合并 / 对齐合并（从 `cli.rs` 拆出，守住台账上限）。

use serde_json::json;

use crate::merge::{merge_aligned, merge_vote};
use crate::to_wsp;
use crate::wire;

use super::{emit, err, load_net, no_double_stdin, parse, two, write_out};

pub(super) fn merge(args: &[String]) -> Result<(), String> {
    let parsed = parse(args, &["aligned"])?;
    let (left, right) = two(&parsed, "merge <a|-> <b|-> [--aligned]")?;
    no_double_stdin(&[left, right])?;
    let a = load_net(left)?;
    let b = load_net(right)?;
    let merged = if parsed.switches.contains("aligned") {
        merge_aligned(&a, &b).map_err(err)?
    } else {
        merge_vote(&a, &b).map_err(err)?
    };
    write_out(&parsed, &merged)?;
    emit(&json!({
        "summary": wire::net_summary(&merged),
        "artifact": to_wsp(&merged),
    }))
}
