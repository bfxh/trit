//! 权重容器与 BN 折叠。
//!
//! ckpt 由导出侧写成 `weights.bin` + `weights.json`（名字 / 偏移 / 形状索引），
//! 这里只做两件事：按名字取张量、把 BN 折叠进卷积权重与偏移。
//! 三值形态与稠密形态共用同一份折叠后的 bias，两者的差就是量化损失本身。

use std::collections::HashMap;
use std::path::Path;

/// BN 折叠的数值稳定项。
pub const EPS: f32 = 1e-5;

pub struct Tensors {
    pub data: HashMap<String, (Vec<f32>, Vec<usize>)>,
}

impl Tensors {
    pub fn load(dir: &Path) -> Result<Self, String> {
        let idx = std::fs::read_to_string(dir.join("weights.json")).map_err(|e| e.to_string())?;
        let raw = std::fs::read(dir.join("weights.bin")).map_err(|e| e.to_string())?;
        let mut data = HashMap::new();
        for blk in idx.split('{').skip(1) {
            let blk = blk.split('}').next().unwrap_or("");
            let name = field_str(blk, "name").ok_or("weights.json 缺 name")?;
            let off: usize = field_num(blk, "off").ok_or("weights.json 缺 off")?;
            let n: usize = field_num(blk, "n").ok_or("weights.json 缺 n")?;
            let shape = field_shape(blk).ok_or("weights.json 缺 shape")?;
            let start = off
                .checked_mul(4)
                .ok_or_else(|| format!("{name}: off 偏移 {off} 溢出"))?;
            let end = off
                .checked_add(n)
                .and_then(|s| s.checked_mul(4))
                .ok_or_else(|| format!("{name}: off + n 溢出（off {off}，n {n}）"))?;
            let bytes = raw.get(start..end).ok_or_else(|| format!("{name}: 越界"))?;
            // 容量只认真实字节数。`n` 已被上式约束成 `n * 4 == end - start`，
            // 但这里不拿它当真：分配依据只剩 `weights.bin` 的实际长度。
            let mut v = Vec::with_capacity(bytes.len() / 4);
            for c in bytes.as_chunks::<4>().0 {
                v.push(f32::from_le_bytes(*c));
            }
            data.insert(name, (v, shape));
        }
        Ok(Self { data })
    }

    pub fn get(&self, name: &str) -> Result<&(Vec<f32>, Vec<usize>), String> {
        self.data.get(name).ok_or_else(|| format!("缺张量 {name}"))
    }

    pub fn v(&self, name: &str) -> Result<&[f32], String> {
        Ok(&self.get(name)?.0)
    }

    pub fn len(&self) -> usize {
        self.data.len()
    }

    pub fn is_empty(&self) -> bool {
        self.data.is_empty()
    }
}

fn field_str(blk: &str, key: &str) -> Option<String> {
    let k = format!("\"{key}\":");
    let i = blk.find(&k)? + k.len();
    let rest = &blk[i..];
    let a = rest.find('"')? + 1;
    let b = rest[a..].find('"')? + a;
    Some(rest[a..b].to_string())
}

fn field_num(blk: &str, key: &str) -> Option<usize> {
    let k = format!("\"{key}\":");
    let i = blk.find(&k)? + k.len();
    let rest = blk[i..].trim_start();
    let end = rest
        .find(|c: char| !c.is_ascii_digit())
        .unwrap_or(rest.len());
    rest[..end].parse().ok()
}

fn field_shape(blk: &str) -> Option<Vec<usize>> {
    let k = "\"shape\":";
    let i = blk.find(k)? + k.len();
    let rest = &blk[i..];
    let a = rest.find('[')? + 1;
    let b = rest[a..].find(']')? + a; // 从 `[` 之后找 `]`（第 34 轮：反序会反向切片）
    Some(
        rest[a..b]
            .split(',')
            .filter_map(|s| s.trim().parse().ok())
            .collect(),
    )
}

/// 一层卷积的两种权重形态 + 折叠后的偏移。
pub struct ConvW {
    /// 稠密权重 `[(ky*kk+kx)*cin + ic][cout]`，已折叠 BN。
    pub wf: Vec<f32>,
    /// 三值权重，布局同上。
    pub wt: Vec<i32>,
    /// 每输出通道 scale（含 BN）。
    pub ws: Vec<f32>,
    /// 折叠后的偏移。
    pub bias: Vec<f32>,
    pub cin: u32,
    pub cout: u32,
    pub kk: u32,
}

mod fold;
pub use fold::{conv_w, convt_w, transpose_conv, transpose_convt};
