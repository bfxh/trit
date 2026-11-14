//! CLI `fingerprint`：挑战集行为指纹的生成与校验。

use crate::fingerprint::{self, Fingerprint};

use super::{
    atomic_write, emit, err, load_json, load_net, load_oracle, one, opt_usize, parse, value,
};

pub(super) fn fingerprint(args: &[String]) -> Result<(), String> {
    match args.first().map(String::as_str).unwrap_or("") {
        "emit" => emit_fingerprint(args.get(1..).unwrap_or(&[])),
        "check" => check(args.get(1..).unwrap_or(&[])),
        other => Err(format!("fingerprint 只支持 emit / check，得到 `{other}`")),
    }
}

fn emit_fingerprint(args: &[String]) -> Result<(), String> {
    let parsed = parse(args, &[])?;
    let path = one(
        &parsed,
        "fingerprint emit <artifact|-> --oracle <json|-> [--out <path>]",
    )?;
    let oracle_path = value(&parsed, "oracle")?;
    let net = load_net(path)?;
    let oracle = load_oracle(oracle_path)?;
    let fingerprint = fingerprint::emit(&net, &oracle).map_err(err)?;
    let value = fingerprint.to_json();
    if let Some(out) = parsed.values.get("out") {
        let text =
            serde_json::to_string_pretty(&value).map_err(|error| format!("指纹 JSON：{error}"))?;
        atomic_write(out, &text)?;
    }
    emit(&value)
}

fn check(args: &[String]) -> Result<(), String> {
    let parsed = parse(args, &[])?;
    let path = one(
        &parsed,
        "fingerprint check <artifact|-> --oracle <json|-> --fingerprint <json|->",
    )?;
    let oracle_path = value(&parsed, "oracle")?;
    let fingerprint_path = value(&parsed, "fingerprint")?;
    let limit = opt_usize(&parsed, "limit", 16)?;
    let net = load_net(path)?;
    let oracle = load_oracle(oracle_path)?;
    let saved = Fingerprint::from_json(&load_json(fingerprint_path)?).map_err(err)?;
    let report = fingerprint::check(&net, &oracle, &saved, limit).map_err(err)?;
    emit(&report.to_json())
}
