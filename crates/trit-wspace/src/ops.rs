//! 工具面共用的高层操作：CLI 与 MCP 走同一条判决路径，避免两边分叉。
//!
//! 这里只组合已有原语，不引入新的语义：补丁仍是事务式的，修补仍只返回精确解。

use serde_json::{Value, json};

use crate::edit::Patch;
use crate::error::Result;
use crate::net::Net;
use crate::oracle::Oracle;
use crate::program::Program;
use crate::repair::{Repair, RepairOptions, repair};
use crate::wire;

/// 应用补丁的结果。
#[derive(Clone, Debug, PartialEq)]
pub struct PatchOutcome {
    /// 报告 JSON（`Program` 报告或直接应用的报告）。
    pub report: Value,
    /// 最终网络（回滚时与输入逐位相同）。
    pub net: Net,
    /// 补丁是否被应用过。
    pub applied: bool,
    /// 是否因后置失败而回滚。
    pub rolled_back: bool,
}

/// 应用补丁：有验证集时走 `Program`（可带前后置条件），没有时直接事务式应用。
pub fn apply_patch(
    net: &Net,
    patch: Patch,
    oracle: Option<&Oracle>,
    require_before: bool,
    require_after: bool,
) -> Result<PatchOutcome> {
    let mut work = net.clone();
    if let Some(oracle) = oracle {
        let mut program = Program::new("tool", patch);
        if require_before {
            program = program.require_before(oracle.behavior());
        }
        if require_after {
            program = program.require_after(oracle.behavior());
        }
        let report = program.run(&mut work, oracle)?;
        Ok(PatchOutcome {
            report: wire::report_to_value(&report),
            net: work,
            applied: report.applied,
            rolled_back: report.rolled_back,
        })
    } else {
        let cost = patch.cost(net);
        patch.apply(&mut work)?;
        Ok(PatchOutcome {
            report: json!({
                "applied": true,
                "rolled_back": false,
                "cost": wire::cost_to_value(&cost),
            }),
            net: work,
            applied: true,
            rolled_back: false,
        })
    }
}

/// 求精确补丁并应用；找不到返回 `None`（不返回“差不多对”的网络）。
pub fn repair_net(
    net: &Net,
    oracle: &Oracle,
    options: &RepairOptions,
) -> Result<Option<(Repair, Net)>> {
    match repair(net, oracle, options)? {
        Some(found) => {
            let mut work = net.clone();
            found.patch.apply(&mut work)?;
            Ok(Some((found, work)))
        }
        None => Ok(None),
    }
}
