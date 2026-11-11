//! CLI `pack-artifact` / `unpack-artifact`：WSP1 ↔ 2-bit WSP2 压缩容器。

use std::fs;

use serde_json::json;

use crate::to_wsp;
use crate::wsp2::{from_wsp2, to_wsp2};

use super::{atomic_write, emit, err, load_net, one, parse, value};

pub(super) fn pack(args: &[String]) -> Result<(), String> {
    let parsed = parse(args, &[])?;
    let path = one(&parsed, "pack-artifact <artifact|-> --out <bin>")?;
    let out = value(&parsed, "out")?;
    let net = load_net(path)?;
    let bytes = to_wsp2(&net).map_err(err)?;
    fs::write(out, &bytes).map_err(|error| format!("{out}: {error}"))?;
    emit(&json!({
        "out": out,
        "layers": net.layers().len(),
        "bytes": bytes.len(),
    }))
}

pub(super) fn unpack(args: &[String]) -> Result<(), String> {
    let parsed = parse(args, &[])?;
    let path = one(&parsed, "unpack-artifact <bin> --out <path>")?;
    let out = value(&parsed, "out")?;
    let bytes = fs::read(path).map_err(|error| format!("{path}: {error}"))?;
    let net = from_wsp2(&bytes).map_err(err)?;
    atomic_write(out, &to_wsp(&net))?;
    emit(&json!({
        "in": path,
        "out": out,
        "layers": net.layers().len(),
        "bytes": bytes.len(),
    }))
}
