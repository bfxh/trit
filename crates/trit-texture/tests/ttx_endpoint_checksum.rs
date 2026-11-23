//! #636 回归：TTX1/TTX2 的 RGB565 端点字节必须进入校验流。

use trit_texture::ttx::{HEADER_BYTES, from_ttx, to_ttx};
use trit_texture::{TextureError, encode_rgba};

fn sample(width: usize, height: usize) -> Vec<u8> {
    (0..width * height * 4).map(|i| (i % 251) as u8).collect()
}

#[test]
fn endpoint_bytes_are_covered_by_checksum() {
    // F-1：端点 4 字节原先不进校验流，端点篡改可通过闸门解码出错色。
    // 逐字节打翻 e0/e1 的每一字节，都必须判成 TtxChecksum。
    let texture = encode_rgba(&sample(8, 8), 8, 8).unwrap();
    for offset in 0..4usize {
        let mut container = to_ttx(&texture);
        container[HEADER_BYTES + offset] ^= 0x1;
        assert!(
            matches!(from_ttx(&container), Err(TextureError::TtxChecksum { .. })),
            "端点字节 {offset} 未被校验流覆盖"
        );
    }
}

#[test]
fn endpoint_extremes_round_trip_unchanged() {
    // 边界相邻：e0=全 0、e1=全 1 的 RGB565 端点对，合法输入解码逐字节不变。
    let mut texture = encode_rgba(&sample(8, 8), 8, 8).unwrap();
    texture.blocks[0][0..2].copy_from_slice(&0u16.to_be_bytes());
    texture.blocks[0][2..4].copy_from_slice(&u16::MAX.to_be_bytes());
    let container = to_ttx(&texture);
    let back = from_ttx(&container).unwrap();
    assert_eq!(back, texture);
}
