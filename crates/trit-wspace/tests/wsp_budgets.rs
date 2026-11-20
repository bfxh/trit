//! WSP1/WSP2 外部输入的分配预算回归（拆出 redteam 主文件守行数预算）。

/// WSP2 外部块索引：`rows` 远大于 `source_bytes`，修复前 `read_rows_io`
/// 直接 `Vec::with_capacity(rows*cols)` → capacity overflow。
#[test]
fn wsp2_index_huge_rows_rejected_by_source_budget() {
    let json = r#"{
        "magic": "WSP2-IDX",
        "version": 1,
        "source_bytes": 8,
        "source_checksum": 0,
        "block_rows": 64,
        "layers": [{
            "index": 0,
            "rows": 4611686018427387903,
            "cols": 4,
            "has_bias_f32": false,
            "layer_offset": 0,
            "scales_offset": 1,
            "trits_offset": 2,
            "bias_offset": 3,
            "end_offset": 4,
            "row_blocks": [{
                "row_start": 0,
                "row_end": 4611686018427387903,
                "trits_offset": 2,
                "trits_len": 4611686018427387903
            }]
        }]
    }"#;
    let value: serde_json::Value = serde_json::from_str(json).expect("json");
    let error = trit_wspace::wsp2::BlockIndex::from_json(&value).unwrap_err();
    assert!(
        error.to_string().contains("预算"),
        "应按 source_bytes 预算拒绝：{error}"
    );
}

/// WSP1 恶意文本：arch 声明 `usize::MAX × 1`。
/// 修复前：`Vec::with_capacity(rows * cols)` 在 62 字节输入上 abort。
#[test]
fn wsp1_huge_arch_rejected_by_text_budget() {
    let text = "wsp1\narch 18446744073709551615 1\nlayer 0\nscale 1\nbias 0\nrow -\nend\nchecksum 0000000000000000\n";
    let error = trit_wspace::from_wsp(text).unwrap_err();
    assert!(
        error.to_string().contains("权重总数"),
        "应按文本预算拒绝：{error}"
    );
}
