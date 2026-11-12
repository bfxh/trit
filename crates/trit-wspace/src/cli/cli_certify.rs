//! CLI `certify`：给一次编辑出保持集副作用证书（从 `cli.rs` 拆出）。

use crate::certify::{self, CertifyOptions};
use crate::wire;

use super::{
    atomic_write, emit, err, load_json, load_net, load_oracle, no_double_stdin, one, opt_u64,
    opt_usize, parse, write_out,
};

pub(super) fn certify(args: &[String]) -> Result<(), String> {
    let parsed = parse(args, &["strict"])?;
    let path = one(
        &parsed,
        "certify <artifact|-> (--keep <oracle|-> | --keep-samples N [--seed S]) \
         (--patch <json|-> | --edited <artifact|->) [--edit <oracle|->] [--allow N] \
         [--limit N] [--strict] [--out <path>] [--certificate <path>]",
    )?;
    let keep_path = parsed.values.get("keep").map(String::as_str);
    let keep_samples = parsed.values.get("keep-samples").map(String::as_str);
    let patch_path = parsed.values.get("patch").map(String::as_str);
    let edited_path = parsed.values.get("edited").map(String::as_str);
    if patch_path.is_some() == edited_path.is_some() {
        return Err(
            "certify 需要 `--patch <json>` 或 `--edited <artifact>` 之一（不能都给）".into(),
        );
    }
    let edit_path = parsed.values.get("edit").map(String::as_str);
    let mut sources = vec![path];
    for extra in [keep_path, patch_path, edited_path, edit_path]
        .into_iter()
        .flatten()
    {
        sources.push(extra);
    }
    no_double_stdin(&sources)?;

    let base = load_net(path)?;
    let keep = match (keep_path, keep_samples) {
        (Some(_), Some(_)) => return Err("`--keep` 与 `--keep-samples` 只能给一个".into()),
        (Some(source), None) => load_oracle(source)?,
        (None, Some(raw)) => {
            let samples = raw
                .parse::<usize>()
                .map_err(|_| format!("`--keep-samples` 需要正整数，得到 `{raw}`"))?;
            certify::behavior_set(&base, samples, opt_u64(&parsed, "seed", 7)?).map_err(err)?
        }
        (None, None) => {
            return Err("需要 `--keep <oracle>` 或 `--keep-samples N`".into());
        }
    };
    let edit = edit_path.map(load_oracle).transpose()?;
    let options = CertifyOptions {
        allow: opt_usize(&parsed, "allow", 0)?,
        rows: opt_usize(&parsed, "limit", certify::DEFAULT_ROWS)?,
    };

    let (certificate, edited) = match patch_path {
        Some(patch_source) => {
            let patch = wire::patch_from_value(&load_json(patch_source)?).map_err(err)?;
            certify::apply_and_certify(&base, &patch, &keep, edit.as_ref(), &options)
                .map_err(err)?
        }
        None => {
            let edited = load_net(edited_path.expect("checked above"))?;
            let certificate = certify::certify_nets(&base, &edited, &keep, edit.as_ref(), &options)
                .map_err(err)?;
            (certificate, edited)
        }
    };

    write_out(&parsed, &edited)?;
    let report = certificate.to_json();
    if let Some(certificate_path) = parsed.values.get("certificate") {
        let text = serde_json::to_string_pretty(&report)
            .map_err(|error| format!("证书序列化失败：{error}"))?;
        atomic_write(certificate_path, &text)?;
    }
    let passed = certificate.passed();
    emit(&report)?;
    if !passed && parsed.switches.contains("strict") {
        let edit_loss = certificate.edit.as_ref().map_or(0, |one| one.after_loss);
        return Err(format!(
            "证书不成立：keep violations={}（允许 {}），edit loss={}",
            certificate.keep.violations(),
            certificate.allow,
            edit_loss
        ));
    }
    Ok(())
}
