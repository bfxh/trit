//! WSPM 容器条目 blob 区间不相交校验（#631）。
//!
//! 回归：合法多张量容器解包后逐张量 trits / bias / scales / biasf 与源一致。
//! 极端几何：完全重叠、部分重叠、首尾相接（放行）、零长度 blob、同 offset 退化；
//! 重叠报错复用既有 `Wire` 变体，文案点名冲突的两个张量名。

use trit_wspace::wspm::{self, ModelItem};
use trit_wspace::{Error, Layer, Net, to_wsp};

/// 2×3 张量 A（含 f32 偏置幅度）。
fn net_a() -> Net {
    let layer = Layer::with_scales(2, 3, &[1, 0, -1, 0, 1, -1], &[1, -1], vec![1.0, 2.0])
        .expect("layer a")
        .with_bias_f32(vec![0.25, -0.25])
        .expect("biasf a");
    Net::new(vec![layer]).expect("net a")
}

/// 2×3 张量 B（与 A 同形状，内容不同）。
fn net_b() -> Net {
    let layer = Layer::with_scales(2, 3, &[-1, 1, 0, 1, -1, 0], &[-1, 1], vec![2.0, 1.0])
        .expect("layer b")
        .with_bias_f32(vec![-0.25, 0.25])
        .expect("biasf b");
    Net::new(vec![layer]).expect("net b")
}

/// 1×2 向量张量。
fn net_c() -> Net {
    let layer = Layer::new(1, 2, &[1, -1], &[0]).expect("layer c");
    Net::new(vec![layer]).expect("net c")
}

fn pack_pair() -> (Vec<Net>, Vec<String>, wspm::PackOutput) {
    let nets = vec![net_a(), net_b()];
    let names = vec!["a.weight".to_string(), "b.weight".to_string()];
    let items: Vec<ModelItem<'_>> = names
        .iter()
        .zip(&nets)
        .map(|(name, net)| ModelItem {
            name,
            vector: false,
            bias: None,
            net,
        })
        .collect();
    let packed = wspm::pack(&items, "auto").expect("pack pair");
    (nets, names, packed)
}

/// FNV-1a 64（与 `wsp2::bytes::fnv` 同常数；测试内自包含重算容器尾校验和）。
fn fnv1a(data: &[u8]) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325_u64;
    for &byte in data {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

/// 解析每个 entry 的定长区起点与 offset 字段偏移（线格式见 `wspm/mod.rs` 模块文档）。
fn entry_fields(bytes: &[u8]) -> Vec<(usize, usize)> {
    let count = u32::from_le_bytes(bytes[6..10].try_into().expect("count")) as usize;
    let mut pos = 10;
    let mut fields = Vec::with_capacity(count);
    for _ in 0..count {
        let name_len =
            u16::from_le_bytes(bytes[pos..pos + 2].try_into().expect("name len")) as usize;
        let fixed = pos + 2 + name_len;
        fields.push((fixed, fixed + 22));
        pos = fixed + 38;
    }
    fields
}

/// 改写第 `index` 个条目的 offset/len，并重算尾部 FNV 校验和。
fn patch_region(bytes: &mut [u8], index: usize, offset: u64, len: u64) {
    let (fixed, off_off) = entry_fields(bytes)[index];
    bytes[off_off..off_off + 8].copy_from_slice(&offset.to_le_bytes());
    bytes[fixed + 30..fixed + 38].copy_from_slice(&len.to_le_bytes());
    let checksum = fnv1a(&bytes[..bytes.len() - 8]);
    let tail = bytes.len() - 8;
    bytes[tail..].copy_from_slice(&checksum.to_le_bytes());
}

fn unpack_err(bytes: &[u8], what: &str) -> Error {
    match wspm::unpack(bytes) {
        Err(error) => error,
        Ok(_) => panic!("{what}必须拒绝"),
    }
}

#[test]
fn legal_multi_tensor_round_trip_fields_match() {
    let nets = vec![net_a(), net_b(), net_c()];
    let names = ["a.weight", "b.weight", "c.bias"];
    let items: Vec<ModelItem<'_>> = names
        .iter()
        .zip(&nets)
        .map(|(name, net)| ModelItem {
            name,
            vector: false,
            bias: None,
            net,
        })
        .collect();
    let packed = wspm::pack(&items, "auto").expect("pack three");
    let model = wspm::unpack(&packed.bytes).expect("unpack three");

    assert_eq!(model.nets.len(), nets.len());
    for (source, decoded) in nets.iter().zip(&model.nets) {
        // 文本级逐字节一致（trits / bias / scales / biasf 全包含）。
        assert_eq!(to_wsp(source), to_wsp(decoded), "WSP1 文本逐字节一致");
        let src = source.layers()[0].clone();
        let dec = decoded.layers()[0].clone();
        assert_eq!(src.trits(), dec.trits(), "trits 一致");
        assert_eq!(src.bias(), dec.bias(), "bias 一致");
        assert_eq!(src.scales(), dec.scales(), "scales 一致");
        assert_eq!(src.bias_f32(), dec.bias_f32(), "biasf 一致");
    }
}

#[test]
fn full_overlap_rejected_with_named_error() {
    let (_nets, _names, packed) = pack_pair();
    let mut bytes = packed.bytes.clone();
    // 条目 B 的区间整体指向条目 A 的 blob：同 offset、同 len，完全重叠。
    let fields = entry_fields(&bytes);
    let a_off = u64::from_le_bytes(
        bytes[fields[0].1..fields[0].1 + 8]
            .try_into()
            .expect("a off"),
    );
    let a_len = u64::from_le_bytes(
        bytes[fields[0].0 + 30..fields[0].0 + 38]
            .try_into()
            .expect("a len"),
    );
    patch_region(&mut bytes, 1, a_off, a_len);

    let error = unpack_err(&bytes, "完全重叠");
    let Error::Wire(msg) = &error else {
        panic!("期望 Wire 错误，得到 {error:?}");
    };
    assert!(msg.contains("相交"), "文案点明区间相交：{msg}");
    assert!(msg.contains("a.weight"), "文案含首张量名：{msg}");
    assert!(msg.contains("b.weight"), "文案含次张量名：{msg}");
}

#[test]
fn partial_overlap_rejected() {
    let (_nets, _names, packed) = pack_pair();
    let mut bytes = packed.bytes.clone();
    let fields = entry_fields(&bytes);
    let a_off = u64::from_le_bytes(
        bytes[fields[0].1..fields[0].1 + 8]
            .try_into()
            .expect("a off"),
    );
    let a_len = u64::from_le_bytes(
        bytes[fields[0].0 + 30..fields[0].0 + 38]
            .try_into()
            .expect("a len"),
    );
    // B 起点右移 5 字节、长度同步缩短：区间落在 A 内部，部分重叠。
    patch_region(&mut bytes, 1, a_off + 5, a_len - 5);

    let error = unpack_err(&bytes, "部分重叠");
    let Error::Wire(msg) = &error else {
        panic!("期望 Wire 错误，得到 {error:?}");
    };
    assert!(msg.contains("a.weight"), "文案含首张量名：{msg}");
    assert!(msg.contains("b.weight"), "文案含次张量名：{msg}");
}

#[test]
fn back_to_back_regions_allowed() {
    let (_nets, _names, packed) = pack_pair();
    // 合法打包的 blob 天然首尾相接：B 的起点 == A 的终点。
    let model = wspm::unpack(&packed.bytes).expect("首尾相接必须放行");
    let (a, b) = (&model.entries[0], &model.entries[1]);
    assert_eq!(b.offset, a.offset + a.len, "区间首尾相接");

    // 显式把 B 钉在 A 的终点上（边界 `second_start == first_end`），仍应解包成功。
    let mut bytes = packed.bytes.clone();
    patch_region(&mut bytes, 1, a.offset + a.len, b.len);
    let moved = wspm::unpack(&bytes).expect("边界相接不得判重叠");
    assert_eq!(moved.nets.len(), 2);

    // 对照：B 起点左移 1 字节即落入 A 尾部，必须拒绝。
    let mut bytes = packed.bytes.clone();
    patch_region(&mut bytes, 1, a.offset + a.len - 1, b.len);
    assert!(wspm::unpack(&bytes).is_err(), "侵入 A 尾部 1 字节必须拒绝");
}

#[test]
fn zero_length_blob_rejected_before_region_check() {
    let (_nets, _names, packed) = pack_pair();
    let mut bytes = packed.bytes.clone();
    // 零长度 blob 在 entry 解析阶段即被拒绝（既有语义），不进入区间相交判断。
    patch_region(&mut bytes, 1, 0, 0);
    let error = unpack_err(&bytes, "零长度 blob");
    let Error::Wire(msg) = &error else {
        panic!("期望 Wire 错误，得到 {error:?}");
    };
    assert!(msg.contains("blob 为空"), "文案：{msg}");
}

#[test]
fn same_offset_degenerate_is_full_overlap() {
    let (_nets, _names, packed) = pack_pair();
    let mut bytes = packed.bytes.clone();
    // 两条目同 offset、长度各自非零：退化情形按完全重叠处理。
    let fields = entry_fields(&bytes);
    let a_off = u64::from_le_bytes(
        bytes[fields[0].1..fields[0].1 + 8]
            .try_into()
            .expect("a off"),
    );
    let b_len = u64::from_le_bytes(
        bytes[fields[1].0 + 30..fields[1].0 + 38]
            .try_into()
            .expect("b len"),
    );
    patch_region(&mut bytes, 1, a_off, b_len);
    let error = unpack_err(&bytes, "同 offset");
    let Error::Wire(msg) = &error else {
        panic!("期望 Wire 错误，得到 {error:?}");
    };
    assert!(msg.contains("a.weight"), "文案含首张量名：{msg}");
    assert!(msg.contains("b.weight"), "文案含次张量名：{msg}");
}
