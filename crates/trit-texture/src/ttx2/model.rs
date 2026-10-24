//! TTX2 的模型域：三值层/模型、基 3 打包、层间衔接、MOD 规范校验流。
//!
//! 权重按**内容自己的基数**存储（5 trit/字节 = 1.6 bit/权重），与偏置、
//! 纹理索引同一套打包；GPU 消费方经 [`TritModelLayer::to_dual_rail`] 显式
//! 转换一次得到 trit-gpu 内核的双轨布局（兼容口，不是存储格式）。

use trit_core::value::Ternary;

use crate::TextureError;

/// 每字 trit 数（与 trit-gpu 的 `TRITS_PER_WORD` 一致，就地声明互不依赖）。
pub const TRITS_PER_WORD: usize = 32;
/// 基 3 打包密度（5 trit/字节，243 ≤ 256）。
pub(crate) const TRITS_PER_PACKED_BYTE: usize = 5;

/// 一层三值全连接：`out = sign(W · in + b)`，权重/偏置 ∈ {-1, 0, +1}。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TritModelLayer {
    /// 输入维数。
    pub inputs: usize,
    /// 输出维数。
    pub outputs: usize,
    /// 权重（行主序平面数组，`weight(row, col) = weights[row · inputs + col]`）。
    pub weights: Vec<i8>,
    /// 偏置，取值 `-1/0/1`。
    pub bias: Vec<i8>,
}

impl TritModelLayer {
    /// 从行主序权重构造（校验形状与取值域）。
    pub fn new(weights: &[Vec<i8>], bias: &[i8]) -> Result<Self, TextureError> {
        let outputs = weights.len();
        let inputs = weights.first().map_or(0, Vec::len);
        if outputs == 0 || inputs == 0 {
            return Err(TextureError::TtxModelShape {
                layer: 0,
                message: "inputs/outputs 必须非零".to_string(),
            });
        }
        let mut flat = Vec::with_capacity(outputs * inputs);
        for row_weights in weights {
            if row_weights.len() != inputs {
                return Err(TextureError::TtxModelShape {
                    layer: 0,
                    message: format!("权重列数应为 {inputs}，实际 {}", row_weights.len()),
                });
            }
            for value in row_weights {
                if !(-1..=1).contains(value) {
                    return Err(TextureError::TtxModelShape {
                        layer: 0,
                        message: format!("权重只能是 -1/0/1，实际 {value}"),
                    });
                }
                flat.push(*value);
            }
        }
        if bias.len() != outputs {
            return Err(TextureError::TtxModelShape {
                layer: 0,
                message: format!("偏置长度应为 {outputs}，实际 {}", bias.len()),
            });
        }
        for value in bias {
            if !(-1..=1).contains(value) {
                return Err(TextureError::TtxModelShape {
                    layer: 0,
                    message: format!("偏置只能是 -1/0/1，实际 {value}"),
                });
            }
        }
        Ok(Self {
            inputs,
            outputs,
            weights: flat,
            bias: bias.to_vec(),
        })
    }

    /// 第 `row` 行第 `col` 列的权重（下标越界按常规切片语义 panic，调用方保证界内）。
    pub fn weight(&self, row: usize, col: usize) -> i8 {
        self.weights[row * self.inputs + col]
    }

    /// 行视图：逐层取回 `Vec<Vec<i8>>`（trit-net `Layer::weights` 的形状，无损）。
    pub fn rows(&self) -> Vec<Vec<i8>> {
        self.weights
            .chunks(self.inputs)
            .map(<[i8]>::to_vec)
            .collect()
    }

    /// 转成 GPU 消费的双轨布局（trit-gpu `pack_rows`：**每行各自整字**、
    /// 行内 bit = `1 << (col % 32)`，pos 装 +1、neg 装 -1）。文件里不再存双轨——
    /// 这是加载后的一次显式转换。注意行起点在字边界：`inputs % 32 != 0` 时
    /// 平面 `index/32` 会跨行错位（真机端到端测试抓到过）。
    pub fn to_dual_rail(&self) -> (Vec<u32>, Vec<u32>) {
        let words = self.inputs.div_ceil(TRITS_PER_WORD);
        let mut pos = vec![0u32; self.outputs * words];
        let mut neg = vec![0u32; self.outputs * words];
        for row in 0..self.outputs {
            for col in 0..self.inputs {
                let value = self.weights[row * self.inputs + col];
                let bit = 1u32 << (col % TRITS_PER_WORD);
                match value {
                    1 => pos[row * words + col / TRITS_PER_WORD] |= bit,
                    -1 => neg[row * words + col / TRITS_PER_WORD] |= bit,
                    _ => {}
                }
            }
        }
        (pos, neg)
    }

    pub(crate) fn validate(&self, layer: usize) -> Result<(), TextureError> {
        if self.inputs == 0 || self.outputs == 0 {
            return Err(TextureError::TtxModelShape {
                layer,
                message: "inputs/outputs 必须非零".to_string(),
            });
        }
        let expected =
            self.outputs
                .checked_mul(self.inputs)
                .ok_or(TextureError::TtxModelShape {
                    layer,
                    message: "outputs × inputs 溢出".to_string(),
                })?;
        if self.weights.len() != expected {
            return Err(TextureError::TtxModelShape {
                layer,
                message: format!("权重数应为 {expected}，实际 {}", self.weights.len()),
            });
        }
        if self.bias.len() != self.outputs {
            return Err(TextureError::TtxModelShape {
                layer,
                message: format!("偏置长度应为 {}，实际 {}", self.outputs, self.bias.len()),
            });
        }
        for value in self.weights.iter().chain(&self.bias) {
            if !(-1..=1).contains(value) {
                return Err(TextureError::TtxModelShape {
                    layer,
                    message: format!("权重/偏置只能是 -1/0/1，实际 {value}"),
                });
            }
        }
        Ok(())
    }
}

/// 三值模型：层按顺序串联，**层间衔接（上层输出 = 下层输入）在容器层强制**，
/// 与 trit-net `Model::validate` 同语义。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TritModel {
    /// 各层。
    pub layers: Vec<TritModelLayer>,
}

impl TritModel {
    /// 构造并校验（≥1 层，逐层形状/取值 + 层间维度衔接）。
    pub fn new(layers: Vec<TritModelLayer>) -> Result<Self, TextureError> {
        if layers.is_empty() {
            return Err(TextureError::TtxEmpty);
        }
        for (index, layer) in layers.iter().enumerate() {
            layer.validate(index)?;
            if index > 0 && layer.inputs != layers[index - 1].outputs {
                return Err(TextureError::TtxModelShape {
                    layer: index,
                    message: format!(
                        "输入维数 {} 与上一层输出 {} 不衔接",
                        layer.inputs,
                        layers[index - 1].outputs
                    ),
                });
            }
        }
        Ok(Self { layers })
    }
}

pub(crate) fn value_digit(value: i8) -> u32 {
    // digit = value + 1 ∈ {0,1,2}（-1→0，0→1，+1→2）。
    (value + 1) as u32
}

pub(crate) fn digit_value(digit: u32) -> i8 {
    digit as i8 - 1
}

/// 基 3 打包：低位 trit 在低位（`Σ digit_i · 3^i`），尾字节不满 5 trit 照排。
pub(crate) fn pack_radix3(values: &[i8]) -> Vec<u8> {
    let mut out = vec![0u8; values.len().div_ceil(TRITS_PER_PACKED_BYTE)];
    for (index, value) in values.iter().enumerate() {
        out[index / TRITS_PER_PACKED_BYTE] +=
            (value_digit(*value) * 3u32.pow((index % TRITS_PER_PACKED_BYTE) as u32)) as u8;
    }
    out
}

pub(crate) fn unpack_radix3(bytes: &[u8], count: usize) -> Vec<i8> {
    let mut out = Vec::with_capacity(count);
    for index in 0..count {
        let digit = (bytes[index / TRITS_PER_PACKED_BYTE] as u32
            / 3u32.pow((index % TRITS_PER_PACKED_BYTE) as u32))
            % 3;
        out.push(digit_value(digit));
    }
    out
}

/// MOD 的规范 trit 流：每层全部权重 trit（行主序，自然个数无填充）+ 偏置 trit。
pub(crate) fn model_trits(model: &TritModel) -> Vec<Ternary> {
    let of = |v: i8| match value_digit(v) {
        0 => Ternary::False,
        1 => Ternary::Unknown,
        _ => Ternary::True,
    };
    let mut trits = Vec::new();
    for layer in &model.layers {
        trits.extend(layer.weights.iter().chain(&layer.bias).map(|v| of(*v)));
    }
    trits
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ttx2::{KIND_MOD, MAGIC, TtxDocument, from_ttx2, to_ttx2};

    #[test]
    fn model_roundtrip_pins_the_radix3_packing() {
        // 手算常量钉死基 3 打包：5 个权重 [ +1, -1, 0, +1, 0 ] → digits [2,0,1,2,1]
        // → 首字节 = 2·1 + 0·3 + 1·9 + 2·27 + 1·81 = 2 + 9 + 54 + 81 = 146。
        let layer = TritModelLayer::new(&[vec![1, -1, 0, 1, 0]], &[1]).unwrap();
        assert_eq!(pack_radix3(&layer.weights), vec![146]);
        for (row, col, expected) in [(0, 0, 1), (0, 1, -1), (0, 2, 0), (0, 3, 1), (0, 4, 0)] {
            assert_eq!(layer.weight(row, col), expected, "({row},{col})");
        }
        assert_eq!(layer.rows(), vec![vec![1, -1, 0, 1, 0]], "rows() 无损取回");
        let model = TritModel::new(vec![layer]).unwrap();
        let bytes = to_ttx2(&TtxDocument::Model(model.clone())).unwrap();
        assert_eq!(&bytes[..4], &MAGIC);
        assert_eq!(bytes[4], KIND_MOD);
        let TtxDocument::Model(back) = from_ttx2(&bytes).unwrap() else {
            panic!("kind 漂移");
        };
        assert_eq!(back, model);
    }

    #[test]
    fn dual_rail_conversion_matches_the_gpu_layout() {
        // 第 0 行 [ +1, -1, 0, +1, …第 32 列再一个 +1 ]（inputs=33 → W=2）：
        // pos 字 0 = bit0|bit3 = 9、字 1 = 1；neg 字 0 = bit1 = 2。
        let mut row0 = vec![0i8; 33];
        row0[0] = 1;
        row0[1] = -1;
        row0[3] = 1;
        row0[32] = 1;
        // 第 1 行只有 col0 = +1：**行起点在字边界**——必须落 pos[2]（第 1 行的
        // 首字），绝不能是平面 index/32 算出的 pos[1]。跨行边界曾在此错位
        //（真机端到端测试抓到的 bug 形态，这里钉死）。
        let mut row1 = vec![0i8; 33];
        row1[0] = 1;
        let layer = TritModelLayer::new(&[row0, row1], &[0, 0]).unwrap();
        let (pos, neg) = layer.to_dual_rail();
        assert_eq!(pos, vec![0b1001, 0b0000_0001, 0b1, 0]);
        assert_eq!(neg, vec![0b0010, 0, 0, 0]);
    }

    #[test]
    fn model_layer_rejects_bad_rows_values_and_bias() {
        assert!(matches!(
            TritModelLayer::new(&[], &[0]),
            Err(TextureError::TtxModelShape { .. })
        ));
        assert!(matches!(
            TritModelLayer::new(&[vec![0, 1], vec![1]], &[0, 0]),
            Err(TextureError::TtxModelShape { .. })
        ));
        assert!(matches!(
            TritModelLayer::new(&[vec![2, 0]], &[0]),
            Err(TextureError::TtxModelShape { .. })
        ));
        assert!(matches!(
            TritModelLayer::new(&[vec![0, 1]], &[5]),
            Err(TextureError::TtxModelShape { .. })
        ));
    }

    #[test]
    fn chain_dimensions_are_enforced_like_trit_net() {
        // 第二层输入 3 ≠ 第一层输出 2：与 trit-net Model::validate 同语义拒绝。
        let first = TritModelLayer::new(&[vec![0, 1], vec![1, 0]], &[0, 0]).unwrap();
        let second = TritModelLayer::new(&[vec![0, 1, 0], vec![1, 0, 0]], &[0, 0]).unwrap();
        assert!(matches!(
            TritModel::new(vec![first, second]),
            Err(TextureError::TtxModelShape { layer: 1, .. })
        ));
        // 衔接正确的两层通过。
        let first = TritModelLayer::new(&[vec![0, 1], vec![1, 0]], &[0, 0]).unwrap();
        let second = TritModelLayer::new(&[vec![0, 1], vec![1, 0]], &[0, 0]).unwrap();
        assert!(TritModel::new(vec![first, second]).is_ok());
    }
}
