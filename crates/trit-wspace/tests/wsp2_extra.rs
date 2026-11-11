//! WSP2 额外往返：f32 bias 与 v1 兼容。

use trit_wspace::{Layer, Net, from_wsp2, to_wsp2};

fn fnv(bytes: &[u8]) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325_u64;
    for &byte in bytes {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

#[test]
fn round_trip_preserves_bias_f32() {
    let layer = Layer::with_scales(2, 2, &[1, -1, 0, 1], &[1, -1], vec![0.5, 1.0])
        .expect("layer")
        .with_bias_f32(vec![0.25, -0.75])
        .expect("bias");
    let net = Net::new(vec![layer]).expect("net");
    let bytes = to_wsp2(&net).expect("encode");
    let back = from_wsp2(&bytes).expect("decode");
    assert_eq!(back.layers()[0].bias_f32(), &[0.25, -0.75]);
    assert_eq!(back.layers()[0].bias(), net.layers()[0].bias());
}

#[test]
fn decodes_v1_without_biasf() {
    let mut bytes = Vec::new();
    bytes.extend_from_slice(b"WSP2");
    bytes.push(1);
    bytes.extend_from_slice(&1u16.to_le_bytes());
    bytes.extend_from_slice(&1u32.to_le_bytes());
    bytes.extend_from_slice(&2u32.to_le_bytes());
    bytes.extend_from_slice(&1.0f32.to_le_bytes());
    bytes.push(0b0000_1001);
    bytes.push(0b0000_0001);
    let sum = fnv(&bytes);
    bytes.extend_from_slice(&sum.to_le_bytes());
    let net = from_wsp2(&bytes).expect("v1");
    let layer = &net.layers()[0];
    assert_eq!((layer.rows(), layer.cols()), (1, 2));
    let trits: Vec<i8> = layer.trits().iter().map(|trit| trit.to_i8()).collect();
    assert_eq!(trits, vec![0, 1]);
    assert!(layer.bias_f32().is_empty());
}
