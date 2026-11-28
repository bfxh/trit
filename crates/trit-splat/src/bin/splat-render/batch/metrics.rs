//! PSNR 汇总与 JSON 落盘。

use std::path::Path;

pub(super) fn mean_psnr(metrics: &[(usize, Option<f64>)]) -> Option<f64> {
    let values: Vec<f64> = metrics.iter().filter_map(|(_, db)| *db).collect();
    if values.is_empty() {
        return None;
    }
    if values.iter().any(|v| v.is_infinite()) {
        return Some(f64::INFINITY);
    }
    Some(values.iter().sum::<f64>() / values.len() as f64)
}

pub(super) fn db_label(db: f64) -> String {
    if db.is_infinite() {
        "∞ dB".into()
    } else {
        format!("{db:.2} dB")
    }
}

pub(super) fn write_psnr_json(
    path: &Path,
    metrics: &[(usize, Option<f64>)],
    mean: Option<f64>,
) -> Result<(), String> {
    let rows: Vec<serde_json::Value> = metrics
        .iter()
        .map(|(frame, db)| {
            serde_json::json!({
                "frame": frame,
                "psnr_db": db.filter(|v| v.is_finite()),
                "infinite": db.map(|v| v.is_infinite()).unwrap_or(false),
            })
        })
        .collect();
    let doc = serde_json::json!({
        "frames": rows,
        "mean_db": mean.filter(|v| v.is_finite()),
        "mean_infinite": mean.map(|v| v.is_infinite()).unwrap_or(false),
    });
    std::fs::write(
        path,
        serde_json::to_string_pretty(&doc).map_err(|e| e.to_string())?,
    )
    .map_err(|e| format!("写 PSNR JSON {}: {e}", path.display()))
}
