//! WSPM 条目 blob 区间两两不相交校验（issue #631）。
//!
//! 修前解包只判每条目自身 `offset+len <= payload_end`，不判两两相交；B 的
//! 区间指向 A 的 blob 时，校验和按重叠后字节重算仍合法，`unpack` 退出 0，
//! B 静默读出 A 的权重。
//!
//! 回归三件套：
//! * 重叠区间被拒，错误串点名冲突的两个条目；
//! * 边界：相邻条目 `prev_end == next_start` 恰好相接放行；
//! * 合法打包（两权重不同、区间不重叠）仍解包成功。

use trit_wspace::wspm::{self, ModelItem};
use trit_wspace::{Layer, Net, to_wsp};

const HEADER: usize = 10;
const ENTRY_FIXED: usize = 38;
const OFF_FIELD: usize = 22;
const LEN_FIELD: usize = 30;

fn nets() -> Vec<Net> {
    let with_bias_f32 = Layer::with_scales(2, 3, &[1, 0, -1, 0, 1, -1], &[1, -1], vec![1.0, 2.0])
        .expect("l0")
        .with_bias_f32(vec![0.25, -0.75])
        .expect("biasf");
    vec![
        Net::new(vec![with_bias_f32]).expect("net0"),
        Net::new(vec![Layer::new(1, 2, &[1, -1], &[0]).expect("l1")]).expect("net1"),
    ]
}

fn pack_bytes() -> Vec<u8> {
    let nets = nets();
    let names = ["w.a".to_string(), "w.b".to_string()];
    let items = [
        ModelItem {
            name: &names[0],
            vector: false,
            bias: None,
            net: &nets[0],
        },
        ModelItem {
            name: &names[1],
            vector: true,
            bias: None,
            net: &nets[1],
        },
    ];
    wspm::pack(&items, "wsp2").expect("pack").bytes
}

/// 与容器尾部 FNV-1a 一致的本地重算（`wsp2::bytes::fnv` 是 pub(crate)，
/// 集成测试拿不到；算法就是标准 FNV-1a 64，常数照抄）。
fn fix_checksum(bytes: &mut [u8]) {
    let payload = bytes.len() - 8;
    let mut hash = 0xcbf2_9ce4_8422_2325_u64;
    for &byte in &bytes[..payload] {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    bytes[payload..].copy_from_slice(&hash.to_le_bytes());
}

/// 第 `index` 个条目定长头在容器里的起点。
fn fixed_start(entries: &[wspm::ModelEntry], index: usize) -> usize {
    let mut pos = HEADER;
    for entry in &entries[..index] {
        pos += 2 + entry.name.len() + ENTRY_FIXED;
    }
    pos += 2 + entries[index].name.len();
    pos
}

fn patch_interval(
    bytes: &mut [u8],
    entries: &[wspm::ModelEntry],
    index: usize,
    offset: u64,
    len: u64,
) {
    let fixed = fixed_start(entries, index);
    bytes[fixed + OFF_FIELD..fixed + OFF_FIELD + 8].copy_from_slice(&offset.to_le_bytes());
    bytes[fixed + LEN_FIELD..fixed + LEN_FIELD + 8].copy_from_slice(&len.to_le_bytes());
    fix_checksum(bytes);
}

#[test]
fn rejects_overlapping_blob_and_names_both_entries() {
    let packed = pack_bytes();
    let model = wspm::unpack(&packed).expect("原始打包必须可解包");
    let a = &model.entries[0];

    let mut bytes = packed.clone();
    // B 的区间改成指向 A 的 blob（完全重叠），重算尾部校验和。
    patch_interval(&mut bytes, &model.entries, 1, a.offset, a.len);

    let text = match wspm::unpack(&bytes) {
        Ok(_) => panic!("重叠区间必须被拒"),
        Err(e) => e.to_string(),
    };
    assert!(text.contains("w.a"), "错误串要点名 w.a：{text}");
    assert!(text.contains("w.b"), "错误串要点名 w.b：{text}");
    assert!(text.contains("重叠"), "{text}");
}

#[test]
fn adjacent_end_equals_next_start_is_accepted() {
    let packed = pack_bytes();
    let model = wspm::unpack(&packed).expect("原始打包必须可解包");
    let a = &model.entries[0];
    let a_end = a.offset + a.len;

    let mut bytes = packed.clone();
    // 把 B 的起点钉到 A 的终点（end == next_start，恰好相接）。
    patch_interval(&mut bytes, &model.entries, 1, a_end, model.entries[1].len);
    let back = wspm::unpack(&bytes).expect("恰好相接必须放行");
    assert_eq!(back.entries[1].offset, a_end);
}

#[test]
fn legal_non_overlapping_pack_unpacks() {
    let packed = pack_bytes();
    let model = wspm::unpack(&packed).expect("合法打包必须解包成功");
    assert_eq!(model.entries.len(), 2);
    // 两条目区间不相交，且权重不同。
    let a = &model.entries[0];
    let b = &model.entries[1];
    assert!(a.offset + a.len <= b.offset);
    assert_ne!(to_wsp(&model.nets[0]), to_wsp(&model.nets[1]));
}
