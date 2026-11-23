//! 端点字节的 FNV-1a 摘要（与 TTX3 `tile_crc` 同口径）。
//!
//! TTX1/TTX2 的 GF(3) 余数原本只盖索引 trit 载荷，每块前 4 字节 RGB565 端点
//! 不在校验流里——端点被篡改后仍能通过校验、解码出错色。这里把端点字节折进
//! 同一个摘要：编码器写什么、解码器重算什么，两端共用同一份输入。

pub(crate) fn endpoint_fnv<'a>(levels: impl IntoIterator<Item = &'a crate::TritTexture>) -> u32 {
    let mut fnv: u32 = 0x811c_9dc5;
    for texture in levels {
        for block in &texture.blocks {
            for &byte in &block[..4] {
                fnv ^= u32::from(byte);
                fnv = fnv.wrapping_mul(0x0100_0193);
            }
        }
    }
    fnv
}
