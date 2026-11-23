//! #636 回归：TTX2 两级纹理的 RGB565 端点字节必须进入校验流。

use trit_texture::ttx2::{HEADER_BYTES, TtxDocument, from_ttx2, to_ttx2};
use trit_texture::{TRIT_BLOCK_BYTES, TextureError, encode_rgba};

fn sample(width: usize, height: usize) -> Vec<u8> {
    (0..width * height * 4).map(|i| (i % 251) as u8).collect()
}

#[test]
fn texture_endpoint_bytes_are_covered_by_checksum() {
    // F-1：端点 4 字节原先不进 TTX2 校验流；逐字节打翻两级端点都必须判
    // TtxChecksum，不能放行解码出错色。
    let fine = encode_rgba(&sample(16, 16), 16, 16).unwrap();
    let coarse = encode_rgba(&sample(8, 8), 8, 8).unwrap();
    let bytes = to_ttx2(&TtxDocument::Texture {
        levels: vec![fine, coarse],
    })
    .unwrap();
    // section：level_count(4) + w(4) + h(4)，首块端点紧随其后。
    for offset in 0..4usize {
        let mut bad = bytes.clone();
        bad[HEADER_BYTES + 12 + offset] ^= 0x1;
        assert!(
            matches!(from_ttx2(&bad), Err(TextureError::TtxChecksum { .. })),
            "粗调层端点字节 {offset} 未被校验流覆盖"
        );
    }
    let fine_block_bytes = 4 * TRIT_BLOCK_BYTES;
    let coarse_endpoint = 4 + 8 + fine_block_bytes + 8;
    let mut bad = bytes;
    bad[HEADER_BYTES + coarse_endpoint] ^= 0x1;
    assert!(matches!(
        from_ttx2(&bad),
        Err(TextureError::TtxChecksum { .. })
    ));
}
