//! WSP2 块索引区间几何校验（issue #630）。
//!
//! 修前 `LayerIndex::from_json` 只校验偏移单调顺序，不按 `rows/cols/flags`
//! 重算各段字节长度，也不校验段间不相交、终点不越 `source_bytes`、层间
//! 区间不重叠；篡改 `.idx.json` 偏移后默认读路径仍 exit 0，读出相邻区的
//! 错误 scales/bias。
//!
//! 回归三件套：
//! * 合法基线（扫描往返 + 整层读回）仍 `Ok`；
//! * 段长谎报 / 偏移漂移 / 层间重叠 / 终点越界被拒；
//! * 边界：相邻区间 `end == next_start` 恰好相接必须放行。

use serde_json::{Value, json};
use trit_wspace::wsp2::BlockIndex;
use trit_wspace::{Layer, Net, from_wsp2, to_wsp2};

/// 两层网络：第一层带 f32 偏置幅度，第二层不带。
fn sample() -> Net {
    let layer0 = Layer::with_scales(
        5,
        4,
        &[
            1, 0, -1, 1, 0, 1, -1, 0, 1, 0, -1, -1, 1, 0, 1, 0, -1, 0, 1, 1,
        ],
        &[1, -1, 0, 1, -1],
        vec![0.5, 1.0, 1.5, 2.0, 2.5],
    )
    .expect("layer0")
    .with_bias_f32(vec![0.1, -0.2, 0.3, -0.4, 0.5])
    .expect("bias_f32");
    let layer1 = Layer::new(
        3,
        5,
        &[1, 0, -1, 0, 1, -1, 1, 0, 1, -1, 0, 1, -1, 0, 1],
        &[0, 1, -1],
    )
    .expect("layer1");
    Net::new(vec![layer0, layer1]).expect("net")
}

fn scanned_index() -> (Vec<u8>, BlockIndex, Value) {
    let bytes = to_wsp2(&sample()).expect("wsp2");
    let index = BlockIndex::scan(&bytes).expect("scan");
    let value = index.to_json();
    (bytes, index, value)
}

#[test]
fn legal_index_round_trips_and_reads_back() {
    let (bytes, index, value) = scanned_index();
    let back = BlockIndex::from_json(&value).expect("合法索引必须通过几何校验");
    assert_eq!(back, index);
    let decoded = from_wsp2(&bytes).expect("decode");
    for number in 0..decoded.layers().len() {
        let layer = back.read_layer(&bytes, number).expect("read layer");
        assert_eq!(&layer, &decoded.layers()[number], "layer {number}");
    }
}

#[test]
fn rejects_scales_offset_drift() {
    let (_bytes, _index, mut value) = scanned_index();
    value["layers"][0]["scales_offset"] = json!(20);
    assert!(BlockIndex::from_json(&value).is_err());
}

#[test]
fn rejects_bias_offset_drift() {
    let (_bytes, _index, mut value) = scanned_index();
    value["layers"][0]["bias_offset"] = json!(59);
    assert!(BlockIndex::from_json(&value).is_err());
}

#[test]
fn rejects_trits_offset_drift() {
    let (_bytes, _index, mut value) = scanned_index();
    value["layers"][0]["trits_offset"] = json!(60);
    value["layers"][0]["row_blocks"][0]["trits_offset"] = json!(60);
    assert!(BlockIndex::from_json(&value).is_err());
}

#[test]
fn rejects_rows_length_lie() {
    let (_bytes, _index, mut value) = scanned_index();
    // rows 谎报成 6：scales 应长 24 字节，与下一段起点对不上。
    value["layers"][0]["rows"] = json!(6);
    assert!(BlockIndex::from_json(&value).is_err());
}

#[test]
fn rejects_end_past_source() {
    let (_bytes, _index, mut value) = scanned_index();
    let source_bytes = value["source_bytes"].as_u64().expect("source_bytes");
    value["layers"][0]["end_offset"] = json!(source_bytes + 1);
    assert!(BlockIndex::from_json(&value).is_err());
}

#[test]
fn rejects_layer_overlap() {
    let (_bytes, _index, mut value) = scanned_index();
    let first_end = value["layers"][0]["end_offset"].as_u64().expect("end");
    // 第二层起点退回到第一层区间内 → 层间重叠。
    value["layers"][1]["layer_offset"] = json!(first_end - 1);
    assert!(BlockIndex::from_json(&value).is_err());
}

#[test]
fn adjacent_end_equals_next_start_is_accepted() {
    let (_bytes, index, mut value) = scanned_index();
    // 扫描产物本就相邻；显式把第二层起点钉到第一层终点（end == next_start）。
    let first_end = index.layers[0].end_offset;
    value["layers"][1]["layer_offset"] = json!(first_end);
    let back = BlockIndex::from_json(&value).expect("恰好相接必须放行");
    assert_eq!(back.layers[1].layer_offset, first_end);
}
