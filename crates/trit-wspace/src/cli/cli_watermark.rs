//! CLI `watermark`：零活跃 trit 水印的嵌入与提取。

use serde_json::{Value, json};

use crate::codec::to_wsp;
use crate::watermark;

use super::{
    atomic_write, emit, err, load_json, load_net, load_oracle, one, opt_u64, opt_usize, parse,
    value,
};

fn parse_hex(text: &str) -> Result<Vec<bool>, String> {
    let mut bits = Vec::new();
    for ch in text.chars().filter(|ch| !ch.is_whitespace()) {
        let digit = ch
            .to_digit(16)
            .ok_or_else(|| format!("非法十六进制字符 `{ch}`"))?;
        for shift in (0..4).rev() {
            bits.push((digit >> shift) & 1 == 1);
        }
    }
    Ok(bits)
}

fn positions_of(value: &Value) -> Result<Vec<(usize, usize, usize)>, String> {
    let entries = value.as_array().ok_or("positions 需要数组")?;
    let mut positions = Vec::with_capacity(entries.len());
    for (index, entry) in entries.iter().enumerate() {
        let triple = entry
            .as_array()
            .ok_or_else(|| format!("positions[{index}] 需要三元组"))?;
        let mut parts = [0usize; 3];
        for (slot, raw) in triple.iter().enumerate().take(3) {
            parts[slot] = usize::try_from(raw.as_u64().ok_or("位置需要非负整数")?)
                .map_err(|_| "位置超出 usize".to_string())?;
        }
        positions.push((parts[0], parts[1], parts[2]));
    }
    Ok(positions)
}

pub(super) fn watermark(args: &[String]) -> Result<(), String> {
    let action = args.first().map(String::as_str).unwrap_or("");
    let rest = args.get(1..).unwrap_or(&[]);
    match action {
        "embed" => embed(rest),
        "extract" => extract(rest),
        other => Err(format!("watermark 只支持 embed / extract，得到 `{other}`")),
    }
}

fn embed(args: &[String]) -> Result<(), String> {
    let parsed = parse(args, &[])?;
    let path = one(
        &parsed,
        "watermark embed <artifact|-> --oracle <json|-> --message <hex> [--seed N] [--out <path>]",
    )?;
    let oracle_path = value(&parsed, "oracle")?;
    let message = parse_hex(value(&parsed, "message")?)?;
    let seed = opt_u64(&parsed, "seed", 7)?;
    let max_scan = opt_usize(&parsed, "max-scan", 100_000)?;
    let net = load_net(path)?;
    let oracle = load_oracle(oracle_path)?;
    let (edited, report) =
        watermark::embed(&net, &oracle, &message, seed, max_scan).map_err(err)?;
    if let Some(out) = parsed.values.get("out") {
        atomic_write(out, &to_wsp(&edited))?;
    }
    let mut value = report.to_json();
    value["artifact"] = json!(to_wsp(&edited));
    emit(&value)
}

fn extract(args: &[String]) -> Result<(), String> {
    let parsed = parse(args, &[])?;
    let path = one(
        &parsed,
        "watermark extract <artifact|-> --positions <json|->",
    )?;
    let positions_path = value(&parsed, "positions")?;
    let net = load_net(path)?;
    let positions = positions_of(&load_json(positions_path)?)?;
    let bits = watermark::extract(&net, &positions);
    let hex: String = bits
        .iter()
        .map(|bit| if *bit { '1' } else { '0' })
        .collect();
    emit(&json!({ "bits": bits.len(), "hex": hex }))
}
