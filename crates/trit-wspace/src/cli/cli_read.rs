//! CLI `read`：打印工件摘要与 WSP1 正文（从 `cli.rs` 拆出，守台账上限）。

use serde_json::json;

use crate::to_wsp;
use crate::wire;

use super::{emit, load_net, one, parse, write_out};

pub(super) fn read(args: &[String]) -> Result<(), String> {
    let parsed = parse(args, &[])?;
    let path = one(&parsed, "read <artifact|-> [--out <path>]")?;
    let net = load_net(path)?;
    write_out(&parsed, &net)?;
    emit(&json!({
        "summary": wire::net_summary(&net),
        "wsp1": to_wsp(&net),
    }))
}
