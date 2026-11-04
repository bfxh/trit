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

/// 应用补丁：有验证集时走 `Program`（可带前后置条件），没有时直接事务式应用。
///
/// 返回 `(报告 JSON, 最终网络)`；后置失败时最终网络与输入逐位相同。
pub fn apply_patch(
    net: &Net,
    patch: Patch,
    oracle: Option<&Oracle>,
    require_before: bool,
    require_after: bool,
) -> Result<(Value, Net)> {
    let mut work = net.clone();
    let report = if let Some(oracle) = oracle {
        let mut program = Program::new("tool", patch);
        if require_before {
            program = program.require_before(oracle.behavior());
        }
        if require_after {
            program = program.require_after(oracle.behavior());
        }
        wire::report_to_value(&program.run(&mut work, oracle)?)
    } else {
        let cost = patch.cost(net);
        patch.apply(&mut work)?;
        json!({
            "applied": true,
            "rolled_back": false,
            "cost": wire::cost_to_value(&cost),
        })
    };
    Ok((report, work))
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
