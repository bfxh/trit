//! 三值 LUT 层：加载 `trit-build --emit-gpu` 的 `truth.gpu.bin`（TGPU v1），
//! 在 CPU 上按 base-3 行号查表；可对 [`crate::VoxelGrid`] 做三值规则后处理。
//!
//! 容器格式（与 `trit-build/src/gpu.rs` 一致）：
//! `TGPU` | version u8 | algebra u8 | nvars u8 | reserved u8
//! | len u32 LE | words u32 LE | pos[words] u32 LE | neg[words] u32 LE（头 16 字节）。
//!
//! 取位规则：`pos=1 → +1`，`neg=1 → -1`，否则 `0`；越界 code 输出 `0`。
//! 这是**离散逻辑层**，不是神经网络权重；三值账里属于 A（原生三值语义）。

use std::path::Path;

use crate::VoxelGrid;

/// 三值查表器。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TernaryLut {
    nvars: usize,
    len: usize,
    pos: Vec<u32>,
    neg: Vec<u32>,
}

impl TernaryLut {
    /// 从 TGPU 字节解析。
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, String> {
        if bytes.len() < 16 {
            return Err(format!("TGPU 头不足 16 字节：{}", bytes.len()));
        }
        if &bytes[0..4] != b"TGPU" {
            return Err("TGPU 魔数不符".into());
        }
        let version = bytes[4];
        if version != 1 {
            return Err(format!("不支持的 TGPU 版本：{version}"));
        }
        let nvars = bytes[6] as usize;
        let len = u32::from_le_bytes([bytes[8], bytes[9], bytes[10], bytes[11]]) as usize;
        let words = u32::from_le_bytes([bytes[12], bytes[13], bytes[14], bytes[15]]) as usize;
        let need = 16 + words * 8;
        if bytes.len() != need {
            return Err(format!("TGPU 体积 {} 与头要求 {need} 不符", bytes.len()));
        }
        if len > (1usize << (2 * nvars.min(30))) {
            return Err(format!("len={len} 与 nvars={nvars} 不匹配"));
        }
        let mut pos = Vec::with_capacity(words);
        let mut neg = Vec::with_capacity(words);
        for i in 0..words {
            let off = 16 + i * 4;
            pos.push(u32::from_le_bytes([
                bytes[off],
                bytes[off + 1],
                bytes[off + 2],
                bytes[off + 3],
            ]));
        }
        for i in 0..words {
            let off = 16 + words * 4 + i * 4;
            neg.push(u32::from_le_bytes([
                bytes[off],
                bytes[off + 1],
                bytes[off + 2],
                bytes[off + 3],
            ]));
        }
        Ok(Self {
            nvars,
            len,
            pos,
            neg,
        })
    }

    /// 从文件加载。
    pub fn load(path: &Path) -> Result<Self, String> {
        let bytes = std::fs::read(path).map_err(|e| format!("读不了 {}：{e}", path.display()))?;
        Self::from_bytes(&bytes)
    }

    /// 变量数。
    pub fn nvars(&self) -> usize {
        self.nvars
    }

    /// 表项数（3^nvars）。
    pub fn len(&self) -> usize {
        self.len
    }

    /// 是否空表。
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// 按 base-3 行号查表。
    pub fn eval_code(&self, code: usize) -> i8 {
        if code >= self.len {
            return 0;
        }
        let word = code >> 5;
        let bit = (code & 31) as u32;
        let pos = (self.pos.get(word).copied().unwrap_or(0) >> bit) & 1;
        let neg = (self.neg.get(word).copied().unwrap_or(0) >> bit) & 1;
        if pos == 1 { 1 } else if neg == 1 { -1 } else { 0 }
    }

    /// 按输入三值向量查表：`-1→0 / 0→1 / +1→2`，`Σ digit·3^i`。
    pub fn eval(&self, inputs: &[i8]) -> i8 {
        assert_eq!(inputs.len(), self.nvars, "输入个数与 nvars 不符");
        self.eval_code(encode_inputs(inputs))
    }

    /// 全表（按 code 顺序）。
    pub fn table(&self) -> Vec<i8> {
        (0..self.len).map(|c| self.eval_code(c)).collect()
    }
}

/// 三值向量 → base-3 行号。
pub fn encode_inputs(inputs: &[i8]) -> usize {
    let mut code = 0usize;
    let mut pow = 1usize;
    for &v in inputs {
        let digit = match v {
            -1 => 0,
            0 => 1,
            1 => 2,
            _ => panic!("三值输入只能是 -1/0/+1，实得 {v}"),
        };
        code += digit * pow;
        pow *= 3;
    }
    code
}

/// 对体素网格做三值规则后处理：每个体素取 `(v(x,y,z), v(x+1), v(x,y+1))` 三个输入，
/// 应用 LUT 后写回同位置。边界按模回绕（确定性）。
pub fn apply_to_voxels(grid: &VoxelGrid, lut: &TernaryLut) -> VoxelGrid {
    assert_eq!(lut.nvars(), 3, "体素规则层要求 3 输入 LUT");
    let s = grid.size;
    let mut out = VoxelGrid::new(s);
    for z in 0..s {
        for y in 0..s {
            for x in 0..s {
                let inputs = [
                    grid.get(x, y, z),
                    grid.get((x + 1) % s, y, z),
                    grid.get(x, (y + 1) % s, z),
                ];
                out.set(x, y, z, lut.eval(&inputs));
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 构造 2 变量 K3 AND 的 TGPU（3²=9 项）。
    fn and_lut() -> TernaryLut {
        let mut pos = 0u32;
        let mut neg = 0u32;
        for code in 0..9usize {
            let a = (code % 3) as i8 - 1;
            let b = (code / 3) as i8 - 1;
            let r = if a == -1 || b == -1 {
                -1
            } else if a == 1 && b == 1 {
                1
            } else {
                0
            };
            if r == 1 {
                pos |= 1 << code;
            }
            if r == -1 {
                neg |= 1 << code;
            }
        }
        let mut bytes = Vec::new();
        bytes.extend_from_slice(b"TGPU");
        bytes.push(1);
        bytes.push(0);
        bytes.push(2);
        bytes.push(0);
        bytes.extend_from_slice(&9u32.to_le_bytes());
        bytes.extend_from_slice(&1u32.to_le_bytes());
        bytes.extend_from_slice(&pos.to_le_bytes());
        bytes.extend_from_slice(&neg.to_le_bytes());
        TernaryLut::from_bytes(&bytes).expect("parse")
    }

    #[test]
    fn parse_and_eval_and_table() {
        let lut = and_lut();
        assert_eq!(lut.nvars(), 2);
        assert_eq!(lut.len(), 9);
        assert_eq!(lut.eval(&[-1, -1]), -1);
        assert_eq!(lut.eval(&[1, 1]), 1);
        assert_eq!(lut.eval(&[0, 1]), 0);
        assert_eq!(lut.eval_code(99), 0);
        assert_eq!(lut.table().len(), 9);
    }

    #[test]
    fn rejects_bad_magic() {
        assert!(TernaryLut::from_bytes(b"XXXX000000000000").is_err());
    }

    #[test]
    fn encode_inputs_base3() {
        assert_eq!(encode_inputs(&[-1, -1]), 0);
        assert_eq!(encode_inputs(&[1, 1]), 8);
        assert_eq!(encode_inputs(&[0, 1]), 7);
    }
}
