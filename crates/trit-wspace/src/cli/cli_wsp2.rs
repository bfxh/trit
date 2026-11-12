//! CLI `pack-artifact` / `unpack-artifact`：WSP1 ↔ 2-bit WSP2 压缩容器。

use std::fs;

use serde_json::json;

use crate::to_wsp;
use crate::wsp3::pack_format;
use crate::wsp4::{decode_auto, encode};

use super::{atomic_write, emit, err, load_net, one, parse, value};

pub(super) fn pack(args: &[String]) -> Result<(), String> {
    let parsed = parse(args, &["rle", "huffman"])?;
    let path = one(&parsed, "pack-artifact <artifact|-> --out <bin> [--rle]")?;
    let out = value(&parsed, "out")?;
    let net = load_net(path)?;
    let huffman = parsed.switches.contains("huffman");
    let rle = parsed.switches.contains("rle");
    let format = pack_format(huffman, rle);
    let bytes = encode(&net, format).map_err(err)?;
    fs::write(out, &bytes).map_err(|error| format!("{out}: {error}"))?;
    emit(&json!({
        "bytes": bytes.len(),
        "format": format,
    }))
}

pub(super) fn unpack(args: &[String]) -> Result<(), String> {
    let parsed = parse(args, &[])?;
    let path = one(&parsed, "unpack-artifact <bin> --out <path>")?;
    let out = value(&parsed, "out")?;
    let bytes = fs::read(path).map_err(|error| format!("{path}: {error}"))?;
    atomic_write(out, &to_wsp(&decode_auto(&bytes).map_err(err)?))?;
    emit(&json!({ "out": out }))
}
