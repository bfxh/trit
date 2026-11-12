//! WSP1 权重分片：把单层网络按行切成多个合法 WSP1 文件 + 一份 manifest。
//!
//! 真实模型的大张量（例如 30522×384 的 embedding）WSP1 文本可达数十 MB；按行切块后
//! 每块仍是可 `read` / `patch` / 校验的独立工件，manifest 记录顺序与行范围。合并时
//! 校验 cols、总行数与块顺序，不做“能拼就拼”的猜测。

use serde_json::{Value, json};

use crate::codec::{from_wsp, to_wsp};
use crate::error::{Error, Result};
use crate::net::{Layer, Net};

/// 一个分片的元信息。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WeightShard {
    /// 文件名（相对分片目录）。
    pub file: String,
    /// 起始行（含）。
    pub start: usize,
    /// 行数。
    pub rows: usize,
}

/// 分片清单。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WeightManifest {
    /// 总行数。
    pub rows: usize,
    /// 列数。
    pub cols: usize,
    /// 每片最大行数。
    pub shard_rows: usize,
    /// 分片列表（按顺序）。
    pub shards: Vec<WeightShard>,
}

impl WeightManifest {
    /// 转成 JSON（CLI 写 `manifest.json`）。
    #[must_use]
    pub fn to_json(&self) -> Value {
        json!({
            "rows": self.rows,
            "cols": self.cols,
            "shard_rows": self.shard_rows,
            "shards": self.shards.iter().map(|shard| json!({
                "file": shard.file,
                "start": shard.start,
                "rows": shard.rows,
            })).collect::<Vec<_>>(),
        })
    }

    /// 从 JSON 解析；形状不对时返回 [`Error::Wire`]。
    pub fn from_json(value: &Value) -> Result<Self> {
        let rows = usize_of(value, "rows")?;
        let cols = usize_of(value, "cols")?;
        let shard_rows = usize_of(value, "shard_rows")?;
        let entries = value
            .get("shards")
            .and_then(Value::as_array)
            .ok_or_else(|| Error::Wire("manifest 缺 shards 数组".into()))?;
        let mut shards = Vec::with_capacity(entries.len());
        for entry in entries {
            let file = entry
                .get("file")
                .and_then(Value::as_str)
                .ok_or_else(|| Error::Wire("manifest 分片缺 file".into()))?
                .to_string();
            let start = usize_of(entry, "start")?;
            let rows = usize_of(entry, "rows")?;
            shards.push(WeightShard { file, start, rows });
        }
        Ok(Self {
            rows,
            cols,
            shard_rows,
            shards,
        })
    }
}

fn usize_of(value: &Value, key: &str) -> Result<usize> {
    value
        .get(key)
        .and_then(Value::as_u64)
        .map(|value| value as usize)
        .ok_or_else(|| Error::Wire(format!("manifest 缺整数 {key}")))
}

/// 把单层网络按 `shard_rows` 行切块；返回清单与每块的 WSP1 文本。
pub fn split(net: &Net, shard_rows: usize) -> Result<(WeightManifest, Vec<String>)> {
    if net.layers().len() != 1 {
        return Err(Error::Wire(format!(
            "分片只支持单层网络（当前 {} 层）",
            net.layers().len()
        )));
    }
    if shard_rows == 0 {
        return Err(Error::InvalidBlock { block: shard_rows });
    }
    let layer = &net.layers()[0];
    let (rows, cols) = (layer.rows(), layer.cols());
    let mut manifest = WeightManifest {
        rows,
        cols,
        shard_rows,
        shards: Vec::new(),
    };
    let mut texts = Vec::new();
    for (index, start) in (0..rows).step_by(shard_rows).enumerate() {
        let take = shard_rows.min(rows - start);
        let mut trits = Vec::with_capacity(take * cols);
        let mut bias = Vec::with_capacity(take);
        for row in start..start + take {
            let source = layer.row(row).expect("行在范围内");
            trits.extend(source.iter().map(|trit| trit.to_i8()));
            bias.push(layer.bias_trit(row).expect("偏置在范围内").to_i8());
        }
        let scales = layer.scales()[start..start + take].to_vec();
        let shard = Layer::with_scales(take, cols, &trits, &bias, scales)?;
        let shard = if layer.bias_f32().is_empty() {
            shard
        } else {
            shard.with_bias_f32(layer.bias_f32()[start..start + take].to_vec())?
        };
        texts.push(to_wsp(&Net::new(vec![shard])?));
        manifest.shards.push(WeightShard {
            file: format!("part-{index:03}.wsp"),
            start,
            rows: take,
        });
    }
    Ok((manifest, texts))
}

/// 按清单合并分片文本；校验顺序、cols、总行数与块形状。
pub fn join(manifest: &WeightManifest, texts: &[String]) -> Result<Net> {
    if texts.len() != manifest.shards.len() {
        return Err(Error::Wire(format!(
            "分片数 {} 与清单 {} 不符",
            texts.len(),
            manifest.shards.len()
        )));
    }
    let mut trits = Vec::new();
    let mut bias = Vec::new();
    let mut scales = Vec::new();
    let mut bias_f32: Option<Vec<f32>> = None;
    let mut next = 0usize;
    for (shard, text) in manifest.shards.iter().zip(texts) {
        if shard.start != next {
            return Err(Error::Wire(format!(
                "分片 {} 从第 {} 行开始，期望 {next}",
                shard.file, shard.start
            )));
        }
        let net = from_wsp(text)?;
        if net.layers().len() != 1 {
            return Err(Error::Wire(format!("分片 {} 不是单层", shard.file)));
        }
        let layer = &net.layers()[0];
        if layer.rows() != shard.rows || layer.cols() != manifest.cols {
            return Err(Error::Wire(format!(
                "分片 {} 形状 {}×{}，清单要求 {}×{}",
                shard.file,
                layer.rows(),
                layer.cols(),
                shard.rows,
                manifest.cols
            )));
        }
        trits.extend(layer.trits().iter().map(|trit| trit.to_i8()));
        bias.extend(layer.bias().iter().map(|trit| trit.to_i8()));
        scales.extend_from_slice(layer.scales());
        match (bias_f32.as_mut(), layer.bias_f32()) {
            (None, []) => {}
            (None, values) => bias_f32 = Some(values.to_vec()),
            (Some(acc), values) => {
                if values.is_empty() {
                    return Err(Error::Wire(format!(
                        "分片 {} 缺少 biasf，其它分片有",
                        shard.file
                    )));
                }
                acc.extend_from_slice(values);
            }
        }
        next += shard.rows;
    }
    if next != manifest.rows {
        return Err(Error::Wire(format!(
            "分片总行数 {next} 与清单 {} 不符",
            manifest.rows
        )));
    }
    let layer = Layer::with_scales(manifest.rows, manifest.cols, &trits, &bias, scales)?;
    let layer = match bias_f32 {
        Some(values) => layer.with_bias_f32(values)?,
        None => layer,
    };
    Net::new(vec![layer])
}
