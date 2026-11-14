//! CLI `pack-artifact` / `unpack-artifact`：WSP1 ↔ 2-bit WSP2 压缩容器（含 WSP2G）。
use crate::wsp2::grouped::{is_grouped_wsp2, pack_text, unpack_grouped};
use crate::wsp3::pack_format;
use crate::wsp4::{decode_auto, encode};
use crate::{from_wsp, to_wsp};
use serde_json::json;

use super::{atomic_write, emit, err, one, parse, read_source, value};
pub(super) fn pack(args: &[String]) -> Result<(), String> {
    let parsed = parse(args, &["rle", "huffman"])?;
    let path = one(&parsed, "pack-artifact <artifact|-> --out <bin> [--rle]")?;
    let out = value(&parsed, "out")?;
    let text = read_source(path)?;
    if let Some(bytes) = pack_text(&text).map_err(err)? {
        std::fs::write(out, &bytes).map_err(|error| format!("{out}: {error}"))?;
        return emit(&json!({ "bytes": bytes.len(), "format": "wsp2g" }));
    }
    let huffman = parsed.switches.contains("huffman");
    let rle = parsed.switches.contains("rle");
    let format = pack_format(huffman, rle);
    let bytes = encode(&from_wsp(&text).map_err(err)?, format).map_err(err)?;
    std::fs::write(out, &bytes).map_err(|error| format!("{out}: {error}"))?;
    emit(&json!({ "bytes": bytes.len(), "format": format }))
}
pub(super) fn unpack(args: &[String]) -> Result<(), String> {
    let parsed = parse(args, &[])?;
    let path = one(&parsed, "unpack-artifact <bin> --out <path>")?;
    let out = value(&parsed, "out")?;
    let bytes = std::fs::read(path).map_err(|error| format!("{path}: {error}"))?;
    if is_grouped_wsp2(&bytes) {
        atomic_write(out, &unpack_grouped(&bytes).map_err(err)?)?;
        return emit(&json!({ "out": out, "format": "wsp2g" }));
    }
    atomic_write(out, &to_wsp(&decode_auto(&bytes).map_err(err)?))?;
    emit(&json!({ "out": out }))
}
