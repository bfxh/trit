//! CLI `import-gguf` / `export-gguf` / `gguf-audit`：三值 GGUF（`trit-export`）与 WSP1 目录互转。
//!
//! 目录约定与 `import-safetensors-dir` 一致：`manifest.json` + `NNNN-<name>.wsp`，
//! 每个 `.wsp` 是单层 WSP1。GGUF 侧只接受 `TYPE_TRIT` 2D 张量；元数据不进 WSP1
//! （往返后只剩导出时写入的 `trit.wsp1` 版本标记）。
//!
//! 目录读写与审计分别放在 `cli_gguf_io.rs` / `cli_gguf_audit.rs`，本文件只留 CLI 壳。

use std::fs;
use std::path::Path;

use serde_json::json;

use super::{emit, one, parse, value};

#[path = "cli_gguf_audit.rs"]
mod audit;
#[path = "cli_gguf_io.rs"]
mod io;

pub(super) use audit::audit;
use io::{read_source, write_model_dir};

pub(super) fn import(args: &[String]) -> Result<(), String> {
    let parsed = parse(args, &[])?;
    let source = one(&parsed, "import-gguf <file> --out <dir>")?;
    let out = value(&parsed, "out")?;
    let bytes = fs::read(source).map_err(|error| format!("{source}: {error}"))?;
    let model = trit_export::read(&bytes).map_err(|error| format!("GGUF {source}: {error}"))?;
    if model.tensors.is_empty() {
        return Err(format!("{source} 里没有张量"));
    }
    let entries = write_model_dir(&model, Path::new(out))?;
    emit(&json!({
        "out": out,
        "manifest": Path::new(out).join("manifest.json").display().to_string(),
        "tensors": entries,
        "metadata": model.metadata.keys().collect::<Vec<_>>(),
    }))
}

pub(super) fn export(args: &[String]) -> Result<(), String> {
    let parsed = parse(args, &[])?;
    let dir = one(&parsed, "export-gguf <dir> --out <file>")?;
    let out = value(&parsed, "out")?;
    let (model, source) = read_source(dir)?;
    let bytes = trit_export::write(&model).map_err(|error| format!("GGUF: {error}"))?;
    super::fsutil::atomic_write_bytes(out, &bytes)?;
    let trit_bytes: usize = model
        .tensors
        .iter()
        .map(|tensor| trit_export::tensor_size(tensor.rows, tensor.cols))
        .sum();
    emit(&json!({
        "dir": dir,
        "out": out,
        "tensors": source.iter().map(|entry| &entry.name).collect::<Vec<_>>(),
        "bytes": bytes.len(),
        "trit_bytes": trit_bytes,
    }))
}
