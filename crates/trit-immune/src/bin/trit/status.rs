//! `trit status`（拆出 bin/trit.rs 守模块行数）。

use super::{open_ledger, CliError};
use trit_immune::NodeId;

pub(super) fn cmd_status(args: &[String]) -> Result<(), CliError> {
    let (j, path) = open_ledger(args)?;
    let dag = j.dag().clone();
    drop(j.into_dag().map_err(|e| CliError::Runtime(e.to_string()))?);
    let conv = dag.convergence();
    let root_gate = dag
        .nodes()
        .len()
        .checked_sub(1)
        .map(|i| dag.merge_gate(NodeId(i as u32)));
    println!("账本：{}", path.display());
    println!(
        "节点：{}（+{} / 0:{} / -{} / 隔离 {}）",
        conv.total, conv.hardened, conv.unknown, conv.vulnerable, conv.quarantined
    );
    println!("收敛率：{:.3}", conv.ratio);
    println!("尾门禁：{root_gate:?}");
    let suspects = dag.suspects();
    println!("可疑队列：{} 个", suspects.len());
    Ok(())
}
