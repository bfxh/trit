//! 子命令路由表（从 `cli.rs` 拆出；新增命令只改这里，不再挤压 `cli.rs`）。

use super::usage::usage_text;
use super::{
    cli_apply, cli_artifact_shard, cli_attribute, cli_block, cli_capability, cli_causal_trace,
    cli_certify, cli_diagnose, cli_diff, cli_exhaustive, cli_explain, cli_fingerprint,
    cli_forgetting, cli_formats, cli_gguf, cli_gradient, cli_impact, cli_interference,
    cli_inventory, cli_learned, cli_localize, cli_merge, cli_metrics, cli_patch, cli_perm_attack,
    cli_perturb, cli_probe, cli_project, cli_read, cli_repair, cli_safetensors, cli_scan,
    cli_script, cli_sequence, cli_task, cli_trace, cli_verify, cli_watermark, cli_wsp2,
    cmd_dare_drop, cmd_rebase, cmd_transplant,
};

pub(super) fn dispatch(args: &[String]) -> Result<(), String> {
    let Some(command) = args.first().map(String::as_str) else {
        return Err(usage_text());
    };
    match command {
        "read" => cli_read::read(&args[1..]),
        "verify" => cli_verify::verify_cmd(&args[1..]),
        "watermark" => cli_watermark::watermark(&args[1..]),
        "capability" => cli_capability::capability_cmd(&args[1..]),
        "scan" => cli_scan::scan_cmd(&args[1..]),
        "causal-trace" => cli_causal_trace::causal_trace(&args[1..]),
        "certify" => cli_certify::certify(&args[1..]),
        "task-merge" => cli_task::task_merge(&args[1..]),
        "task-scale" => cli_task::task_scale(&args[1..]),
        "task-combine" => cli_task::task_combine(&args[1..]),
        "script" => cli_script::script_cmd(&args[1..]),
        "sequence" => cli_sequence::sequence(&args[1..]),
        "patch" => cli_patch::patch_cmd(&args[1..]),
        "apply-candidate" => cli_apply::apply_candidate_cmd(&args[1..]),
        "cost" => cli_metrics::cost_cmd(&args[1..]),
        "history" => cli_metrics::history_cmd(&args[1..]),
        "rebase" => cmd_rebase(&args[1..]),
        "transplant" => cmd_transplant(&args[1..]),
        "dare-drop" => cmd_dare_drop(&args[1..]),
        "repair" => cli_repair::repair(&args[1..]),
        "project" => cli_project::project(&args[1..]),
        "propose" => cli_learned::propose(&args[1..]),
        "train-proposer" => cli_learned::train_proposer(&args[1..]),
        "propose-learned" => cli_learned::propose_learned(&args[1..]),
        "propose-gradient" => cli_gradient::propose_gradient(&args[1..]),
        "explain" => cli_explain::explain_cmd(&args[1..]),
        "impact" => cli_impact::impact_cmd(&args[1..]),
        "interference" => cli_interference::interference(&args[1..]),
        "inventory" => cli_inventory::inventory_cmd(&args[1..]),
        "repair-exhaustive" => cli_exhaustive::exhaustive_cmd(&args[1..]),
        "attribute" => cli_attribute::attribute_cmd(&args[1..]),
        "probe" => cli_probe::probe_cmd(&args[1..]),
        "trace" => cli_trace::trace_cmd(&args[1..]),
        "distribution" => cli_trace::distribution_cmd(&args[1..]),
        "perm-attack" => cli_perm_attack::perm_attack(&args[1..]),
        "perturb" => cli_perturb::perturb_cmd(&args[1..]),
        "diagnose" => cli_diagnose::diagnose_cmd(&args[1..]),
        "localize" => cli_localize::localize_cmd(&args[1..]),
        "audit" => crate::cli_audit::cmd_audit(&args[1..]),
        "shard" => crate::cli_audit::cmd_shard(&args[1..]),
        "audit-shards" => crate::cli_audit::cmd_audit_shards(&args[1..]),
        "replay" => crate::cli_audit::cmd_replay(&args[1..]),
        "diff" => cli_diff::diff(&args[1..]),
        "merge" => cli_merge::merge(&args[1..]),
        "list-safetensors" => cli_safetensors::list(&args[1..]),
        "import-safetensors" => cli_safetensors::import(&args[1..]),
        "export-safetensors" => cli_safetensors::export(&args[1..]),
        "import-safetensors-dir" => cli_safetensors::import_dir(&args[1..]),
        "export-safetensors-dir" => cli_safetensors::export_dir(&args[1..]),
        "import-gguf" => cli_gguf::import(&args[1..]),
        "export-gguf" => cli_gguf::export(&args[1..]),
        "index-artifact" => cli_block::index_artifact(&args[1..]),
        "read-block" => cli_block::read_block(&args[1..]),
        "split-artifact" => cli_artifact_shard::split(&args[1..]),
        "join-artifact" => cli_artifact_shard::join(&args[1..]),
        "pack-artifact" => cli_wsp2::pack(&args[1..]),
        "unpack-artifact" => cli_wsp2::unpack(&args[1..]),
        "fingerprint" => cli_fingerprint::fingerprint(&args[1..]),
        "formats" => cli_formats::formats(&args[1..]),
        "forgetting" => cli_forgetting::forgetting(&args[1..]),
        "mcp" => crate::mcp::serve(std::io::stdin().lock(), std::io::stdout().lock())
            .map_err(|error| error.to_string()),
        "--help" | "-h" | "help" => {
            println!("{}", usage_text());
            Ok(())
        }
        other => Err(format!("未知子命令 `{other}`\n{}", usage_text())),
    }
}
