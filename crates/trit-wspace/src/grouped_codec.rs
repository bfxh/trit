//! WSP1G：分组三值层的文本格式（`wsp1` 的扩展）。
//!
//! `wsp1` 每行一个神经元的 trit（`-`/`0`/`+`），scale 是**每行一个**。分组三值多了两样东西
//! 必须落盘：**每 元素所属的组号**（排序划分是额外信息）与**每组一个 scale**。
//!
//! ```text
//! wsp1g
//! rows 2
//! cols 3
//! group 2
//! scales 0.5 2
//! bias 0 0
//! row -0-
//! row +-+
//! groups 0 1 0 1 0 1
//! end
//! checksum 0123456789abcdef
//! ```
//!
//! ## 这个格式顺带给出了"最诚实的压缩率"
//!
//! `wsp1` 每个 trit 占**一个 ASCII 字节**（8 bit），不是理论上的 2 bit。所以文本格式下：
//!
//! | 表示 | 每元素字节 | 相对 f32 |
//! |---|---|---|
//! | f32（二进制） | 4 | 1× |
//! | `wsp1` 三值（per-tensor） | 1 | **4×** |
//! | `wsp1g` 分组三值 | 1 + 组号文本（约 1.5–2） | **约 1.3–1.6×** |
//!
//! 也就是说：**文本格式下三值只有 4×，与 int8 持平**；分组三值因为要落盘组号，实测比
//! per-tensor 三值更**大**——"排序信息不是免费的"在文件层面的证据。

use crate::error::{Error, Result};
use crate::grouped::GroupedLayer;
use crate::trit::Trit;

/// 文件头。
pub const MAGIC_G: &str = "wsp1g";

fn trit_char(trit: Trit) -> char {
    match trit {
        Trit::Neg => '-',
        Trit::Zero => '0',
        Trit::Pos => '+',
    }
}

fn char_trit(value: char) -> Result<Trit> {
    match value {
        '-' => Ok(Trit::Neg),
        '0' => Ok(Trit::Zero),
        '+' => Ok(Trit::Pos),
        other => Err(Error::InvalidTrit(if other == ' ' { 127 } else { 126 })),
    }
}

fn body(layer: &GroupedLayer) -> String {
    let mut out = String::new();
    out.push_str(MAGIC_G);
    out.push('\n');
    out.push_str(&format!("rows {}\n", layer.rows()));
    out.push_str(&format!("cols {}\n", layer.cols()));
    out.push_str(&format!("group {}\n", layer.group_size()));
    out.push_str("scales");
    for scale in layer.scales() {
        out.push_str(&format!(" {scale}"));
    }
    out.push('\n');
    out.push_str("bias");
    for value in &layer.bias_f32 {
        out.push_str(&format!(" {value}"));
    }
    out.push('\n');
    for row in 0..layer.rows() {
        out.push_str("row ");
        for col in 0..layer.cols() {
            out.push(trit_char(layer.trit(row, col).unwrap_or(Trit::Zero)));
        }
        out.push('\n');
    }
    out.push_str("groups");
    for row in 0..layer.rows() {
        for col in 0..layer.cols() {
            out.push_str(&format!(" {}", layer.group_of(row, col).unwrap_or(0)));
        }
    }
    out.push('\n');
    out.push_str("end\n");
    out
}

/// 序列化为规范 WSP1G 文本（含校验和）。
#[must_use]
pub fn to_wspg(layer: &GroupedLayer) -> String {
    format!("{}checksum {:016x}\n", body(layer), checksum(layer))
}

/// 规范文本的 64 位 FNV-1a 校验和（不含 checksum 行）。
#[must_use]
pub fn checksum(layer: &GroupedLayer) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325_u64;
    for &byte in body(layer).as_bytes() {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x100_0000_01b3);
    }
    hash
}

fn parse_usize(word: &str, what: &'static str) -> Result<usize> {
    word.parse::<usize>().map_err(|_| Error::Shape {
        what,
        expected: 0,
        got: 0,
    })
}

fn parse_f32s(rest: &str) -> Vec<f32> {
    rest.split_whitespace()
        .filter_map(|word| word.parse::<f32>().ok())
        .collect()
}

/// 解析 WSP1G 文本并校验校验和。
pub fn from_wspg(text: &str) -> Result<GroupedLayer> {
    let lines: Vec<&str> = text
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .collect();
    if lines.first().copied() != Some(MAGIC_G) {
        return Err(Error::MissingLine(MAGIC_G));
    }
    let mut rows = 0usize;
    let mut cols = 0usize;
    let mut group_size = 1usize;
    let mut scales: Vec<f32> = Vec::new();
    let mut bias: Vec<f32> = Vec::new();
    let mut trits: Vec<i8> = Vec::new();
    let mut groups: Vec<u32> = Vec::new();
    let mut expected_checksum: Option<u64> = None;
    for line in lines.iter().skip(1) {
        if let Some(rest) = line.strip_prefix("rows ") {
            rows = parse_usize(rest, "rows")?;
        } else if let Some(rest) = line.strip_prefix("cols ") {
            cols = parse_usize(rest, "cols")?;
        } else if let Some(rest) = line.strip_prefix("group ") {
            group_size = parse_usize(rest, "group")?;
        } else if let Some(rest) = line.strip_prefix("scales") {
            scales = parse_f32s(rest);
        } else if let Some(rest) = line.strip_prefix("bias") {
            bias = parse_f32s(rest);
        } else if let Some(rest) = line.strip_prefix("row ") {
            for value in rest.chars() {
                trits.push(char_trit(value)?.to_i8());
            }
        } else if let Some(rest) = line.strip_prefix("groups") {
            groups = rest
                .split_whitespace()
                .filter_map(|word| word.parse::<u32>().ok())
                .collect();
        } else if let Some(rest) = line.strip_prefix("checksum ") {
            // 序列化用 `{:016x}`（十六进制），这里必须按 16 进制解析；
            // 用 `parse::<u64>()` 会因字母失败并被 `.ok()` 静默吞掉，导致校验整个失效。
            expected_checksum = u64::from_str_radix(rest.trim(), 16).ok();
        } else if *line == "end" {
            // 不能 break：`checksum` 行在 `end` 之后，break 会让校验永远读不到。
            continue;
        }
    }
    // 组数 ≠ 组大小：用文本里的准确 group_size 覆盖，否则 body 不一致会让校验和误报。
    let mut layer = GroupedLayer::from_trits(rows, cols, &trits, &groups, &scales, &bias)?;
    layer.group_size = group_size;
    if let Some(want) = expected_checksum {
        let got = checksum(&layer);
        if got != want {
            return Err(Error::Checksum {
                expected: want,
                got,
            });
        }
    }
    Ok(layer)
}
