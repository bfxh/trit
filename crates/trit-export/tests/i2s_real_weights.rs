//! J7：真实权重（microsoft/bitnet-b1.58-2B-4T）的 GGUF I2_S 张量与 safetensors 打包一致。
//!
//! 夹具取 `blk.0.ffn_down.weight` 的第 0 个 I2_S 块（32 字节 = 128 个三值）与
//! `model.layers.0.mlp.down_proj.weight` 的前 128 字节（U8，每个低 2 bit 是一个三值）。
//! 全量对拍（4,423,680 字节）由 `scripts/check_bitnet_i2s_parity.py` 联网复跑。

use trit_core::Ternary;
use trit_export::bitnet::i2s::unpack;

const FIXTURE: &str = include_str!("fixtures/bitnet_i2s_real.txt");

fn hex_of(tag: &str) -> Vec<u8> {
    let line = FIXTURE
        .lines()
        .find(|l| l.starts_with(tag))
        .unwrap_or_else(|| panic!("夹具缺少 {tag}"));
    let h = line[tag.len()..].trim();
    (0..h.len() / 2)
        .map(|i| u8::from_str_radix(&h[2 * i..2 * i + 2], 16).unwrap())
        .collect()
}

#[test]
fn gguf_i2s_block_matches_safetensors_packing() {
    let gguf = hex_of("gguf ");
    let st = hex_of("safetensors ");
    assert_eq!(gguf.len(), 32, "I2_S 块 0 是 32 字节");
    assert_eq!(st.len(), 128, "对应 safetensors 的 128 个字节");

    // safetensors 的 U8：每个字节的低 2 bit 是一个三值（0/1/2 = -1/0/+1）
    let want: Vec<Ternary> = st
        .iter()
        .map(|b| match b & 3 {
            0 => Ternary::False,
            1 => Ternary::Unknown,
            _ => Ternary::True,
        })
        .collect();
    assert_eq!(unpack(&gguf, 128).unwrap(), want);
}
