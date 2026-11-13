//! WSP2 的编码侧（从 `wsp2.rs` 拆出，模块预算"要腾就拆"）。

use crate::error::{Error, Result};
use crate::net::Net;

use super::bytes::{fnv, pack_trits};
use super::{MAGIC, VERSION};

/// 把网络编码成 WSP2 二进制。
pub fn to_wsp2(net: &Net) -> Result<Vec<u8>> {
    let nlayers =
        u16::try_from(net.layers().len()).map_err(|_| Error::Wire("层数超过 u16".into()))?;
    let mut out = Vec::new();
    out.extend_from_slice(MAGIC);
    out.push(VERSION);
    out.extend_from_slice(&nlayers.to_le_bytes());
    for layer in net.layers() {
        let rows = u32::try_from(layer.rows()).map_err(|_| Error::Wire("行数超过 u32".into()))?;
        let cols = u32::try_from(layer.cols()).map_err(|_| Error::Wire("列数超过 u32".into()))?;
        let has_bias_f32 = !layer.bias_f32().is_empty();
        if has_bias_f32 && layer.bias_f32().len() != layer.rows() {
            return Err(Error::Shape {
                what: "bias_f32",
                expected: layer.rows(),
                got: layer.bias_f32().len(),
            });
        }
        out.extend_from_slice(&rows.to_le_bytes());
        out.extend_from_slice(&cols.to_le_bytes());
        out.push(u8::from(has_bias_f32));
        for scale in layer.scales() {
            out.extend_from_slice(&scale.to_le_bytes());
        }
        if has_bias_f32 {
            for value in layer.bias_f32() {
                out.extend_from_slice(&value.to_le_bytes());
            }
        }
        let trits: Vec<i8> = layer.trits().iter().map(|trit| trit.to_i8()).collect();
        out.extend_from_slice(&pack_trits(&trits));
        let bias: Vec<i8> = layer.bias().iter().map(|trit| trit.to_i8()).collect();
        out.extend_from_slice(&pack_trits(&bias));
    }
    let checksum = fnv(&out);
    out.extend_from_slice(&checksum.to_le_bytes());
    Ok(out)
}
