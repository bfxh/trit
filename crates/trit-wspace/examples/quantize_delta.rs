//! f32 微调差分 → 三值最小补丁（`quantize-delta`）。
//!
//! ```bash
//! cargo run --release -p trit-wspace --example quantize_delta -- \
//!   --base base.safetensors --tuned tuned.safetensors --tensor dense.weight \
//!   --trim 0.2 --patch-out patch.json --out edited.wsp --oracle oracle.json
//! ```
//!
//! 先把 base/tuned 各自按 absmean 三值化，只对三值发生变化的坐标生成
//! `SetTrit`/`SetBias`；`--trim` 按原始 f32 差分幅度保留 top 比例。scale 保留 base。

use serde_json::{Value, json};
use trit_wspace::delta;
use trit_wspace::safetensors;
use trit_wspace::to_wsp;
use trit_wspace::wire;

fn main() {
    let mut base_path = String::new();
    let mut tuned_path = String::new();
    let mut tensor = String::new();
    let mut bias_base = None;
    let mut bias_tuned = None;
    let mut trim = 1.0f32;
    let mut patch_out = None;
    let mut out = None;
    let mut oracle_path = None;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--base" => base_path = args.next().expect("base"),
            "--tuned" => tuned_path = args.next().expect("tuned"),
            "--tensor" => tensor = args.next().expect("tensor"),
            "--bias-base" => bias_base = Some(args.next().expect("bias-base")),
            "--bias-tuned" => bias_tuned = Some(args.next().expect("bias-tuned")),
            "--trim" => trim = args.next().expect("trim").parse().expect("num"),
            "--patch-out" => patch_out = Some(args.next().expect("patch-out")),
            "--out" => out = Some(args.next().expect("out")),
            "--oracle" => oracle_path = Some(args.next().expect("oracle")),
            other => panic!("unknown arg {other}"),
        }
    }
    assert!(
        !base_path.is_empty() && !tuned_path.is_empty() && !tensor.is_empty(),
        "需要 --base / --tuned / --tensor"
    );
    let base = safetensors::read_tensor(&base_path, &tensor).expect("base tensor");
    let tuned = safetensors::read_tensor(&tuned_path, &tensor).expect("tuned tensor");
    let base_bias = bias_base
        .as_ref()
        .map(|name| safetensors::read_vector(&base_path, name).expect("base bias"));
    let tuned_bias = bias_tuned
        .as_ref()
        .map(|name| safetensors::read_vector(&tuned_path, name).expect("tuned bias"));
    let (patch, base_net, stats) = delta::diff_weights(
        &base,
        &tuned,
        base_bias.as_deref(),
        tuned_bias.as_deref(),
        trim,
    )
    .expect("diff");
    let mut report = json!({
        "base": base_path,
        "tuned": tuned_path,
        "tensor": tensor,
        "rows": base.rows,
        "cols": base.cols,
        "trim": stats.trim,
        "changed": stats.changed(),
        "kept": stats.kept(),
        "dropped": stats.changed() - stats.kept(),
        "changed_trits": stats.changed_trits,
        "changed_bias": stats.changed_bias,
        "kept_trits": stats.kept_trits,
        "kept_bias": stats.kept_bias,
    });

    if let Some(path) = &oracle_path {
        let text = std::fs::read_to_string(path).expect("oracle");
        let value: Value = serde_json::from_str(&text).expect("oracle json");
        let oracle = wire::oracle_from_value(&value).expect("oracle");
        let before = oracle.loss(&base_net).expect("loss");
        let (full_patch, _, _) = delta::diff_weights(
            &base,
            &tuned,
            base_bias.as_deref(),
            tuned_bias.as_deref(),
            1.0,
        )
        .expect("full diff");
        let mut full_net = base_net.clone();
        full_patch.apply(&mut full_net).expect("apply full");
        let full_loss = oracle.loss(&full_net).expect("loss full");
        let mut trim_net = base_net.clone();
        patch.apply(&mut trim_net).expect("apply trim");
        let trim_loss = oracle.loss(&trim_net).expect("loss trim");
        report["oracle"] = json!({
            "before_loss": before,
            "full_loss": full_loss,
            "trim_loss": trim_loss,
        });
    }
    if let Some(path) = &patch_out {
        let text = serde_json::to_string_pretty(&wire::patch_to_value(&patch)).expect("patch json");
        std::fs::write(path, text + "\n").expect("write patch");
        report["patch_out"] = json!(path);
    }
    if let Some(path) = &out {
        let mut edited = base_net.clone();
        patch.apply(&mut edited).expect("apply");
        std::fs::write(path, to_wsp(&edited)).expect("write wsp");
        report["out"] = json!(path);
    }
    println!(
        "{}",
        serde_json::to_string_pretty(&report).expect("report json")
    );
}
