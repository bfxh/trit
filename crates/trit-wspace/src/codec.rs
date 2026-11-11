//! WSP1：权重空间的源码式文本格式。
//!
//! 目标是**可 diff、可复算、可校验**，不是紧凑：
//!
//! ```text
//! wsp1
//! arch 2 2 1
//! layer 0
//! scale 1 1
//! bias 0 0
//! row -0
//! row +-
//! layer 1
//! scale 1
//! bias 0
//! row -+
//! end
//! checksum 0123456789abcdef
//! ```
//!
//! - 每个 trit 一个字符（`-` / `0` / `+`），一行一个神经元，git diff 按行读；
//! - 解析时允许空行与 `#` 注释，但序列化永远输出规范形式；
//! - 校验和按**解析后的规范文本**重算，所以注释/空白改不动它，改一个 trit 一定变。

use crate::error::{Error, Result};
use crate::net::{Layer, Net};
use crate::trit::Trit;

/// 文件头。
pub const MAGIC: &str = "wsp1";

/// 序列化为规范 WSP1 文本（含校验和）。
#[must_use]
pub fn to_wsp(net: &Net) -> String {
    let body = body(net);
    format!("{body}checksum {:016x}\n", checksum(net))
}

/// 规范文本的 64 位 FNV-1a 校验和（不含 checksum 行）。
#[must_use]
pub fn checksum(net: &Net) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325_u64;
    for &byte in body(net).as_bytes() {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x100_0000_01b3);
    }
    hash
}

/// 解析 WSP1 文本并校验校验和。
pub fn from_wsp(text: &str) -> Result<Net> {
    let lines: Vec<(usize, &str)> = text
        .lines()
        .enumerate()
        .filter_map(|(index, raw)| {
            let line = raw.trim();
            (!line.is_empty() && !line.starts_with('#')).then_some((index + 1, line))
        })
        .collect();
    let mut pos = 0usize;

    let (_, magic) = next(&lines, &mut pos, "wsp1")?;
    if magic != MAGIC {
        return Err(Error::Parse {
            line: 1,
            msg: format!("期望 {MAGIC}，得到 {magic}"),
        });
    }

    let (line, arch) = next(&lines, &mut pos, "arch")?;
    let tokens: Vec<&str> = arch.split_whitespace().collect();
    if tokens.first() != Some(&"arch") || tokens.len() < 3 {
        return Err(Error::Parse {
            line,
            msg: "arch 行需要 `arch <输入维> <输出维>...`".into(),
        });
    }
    let sizes = parse_sizes(&tokens[1..], line)?;

    let mut layers = Vec::with_capacity(sizes.len() - 1);
    for index in 0..sizes.len() - 1 {
        let rows = sizes[index + 1];
        let cols = sizes[index];
        let (line, header) = next(&lines, &mut pos, "layer")?;
        let header_tokens: Vec<&str> = header.split_whitespace().collect();
        if header_tokens.len() != 2
            || header_tokens[0] != "layer"
            || header_tokens[1] != index.to_string()
        {
            return Err(Error::Parse {
                line,
                msg: format!("期望 `layer {index}`"),
            });
        }

        let (line, scale_line) = next(&lines, &mut pos, "scale")?;
        let scale_tokens: Vec<&str> = scale_line.split_whitespace().collect();
        if scale_tokens.first() != Some(&"scale") || scale_tokens.len() != rows + 1 {
            return Err(Error::Parse {
                line,
                msg: format!("scale 行需要 {rows} 个值"),
            });
        }
        let mut scales = Vec::with_capacity(rows);
        for token in &scale_tokens[1..] {
            let value = token.parse::<f32>().map_err(|_| Error::Parse {
                line,
                msg: format!("scale `{token}` 不是 f32"),
            })?;
            scales.push(value);
        }

        let (line, bias_line) = next(&lines, &mut pos, "bias")?;
        let bias_tokens: Vec<&str> = bias_line.split_whitespace().collect();
        if bias_tokens.first() != Some(&"bias") {
            return Err(Error::Parse {
                line,
                msg: "期望 bias 行".into(),
            });
        }
        let bias = parse_trits(&bias_tokens[1..], rows, line, "bias")?;

        let (mut line, mut row_line) = next(&lines, &mut pos, "row")?;
        let mut bias_f32 = Vec::new();
        if row_line.split_whitespace().next() == Some("biasf") {
            let tokens: Vec<&str> = row_line.split_whitespace().collect();
            if tokens.len() != rows + 1 {
                return Err(Error::Parse {
                    line,
                    msg: format!("biasf 行需要 {rows} 个值"),
                });
            }
            for token in &tokens[1..] {
                let value = token.parse::<f32>().map_err(|_| Error::Parse {
                    line,
                    msg: format!("biasf `{token}` 不是 f32"),
                })?;
                bias_f32.push(value);
            }
            let (next_line, next_row) = next(&lines, &mut pos, "row")?;
            line = next_line;
            row_line = next_row;
        }

        let mut trits = Vec::with_capacity(rows * cols);
        for index in 0..rows {
            if index > 0 {
                let (next_line, next_row) = next(&lines, &mut pos, "row")?;
                line = next_line;
                row_line = next_row;
            }
            let row_tokens: Vec<&str> = row_line.split_whitespace().collect();
            if row_tokens.first() != Some(&"row") {
                return Err(Error::Parse {
                    line,
                    msg: "期望 row 行".into(),
                });
            }
            trits.extend(parse_trits(&row_tokens[1..], cols, line, "row")?);
        }
        let layer = Layer::with_scales(rows, cols, &trits, &bias, scales)?;
        layers.push(layer.with_bias_f32(bias_f32)?);
    }

    let (line, end) = next(&lines, &mut pos, "end")?;
    if end != "end" {
        return Err(Error::Parse {
            line,
            msg: "期望 end".into(),
        });
    }
    let (line, sum_line) = next(&lines, &mut pos, "checksum")?;
    let sum_tokens: Vec<&str> = sum_line.split_whitespace().collect();
    if sum_tokens.len() != 2 || sum_tokens[0] != "checksum" {
        return Err(Error::Parse {
            line,
            msg: "期望 `checksum <16 进制>`".into(),
        });
    }
    let expected = u64::from_str_radix(sum_tokens[1], 16).map_err(|_| Error::Parse {
        line,
        msg: format!("checksum `{}` 不是 16 进制 u64", sum_tokens[1]),
    })?;
    if pos != lines.len() {
        return Err(Error::Parse {
            line: lines[pos].0,
            msg: "checksum 之后还有内容".into(),
        });
    }

    let net = Net::new(layers)?;
    let got = checksum(&net);
    if got != expected {
        return Err(Error::Checksum { expected, got });
    }
    Ok(net)
}

fn body(net: &Net) -> String {
    let mut out = String::new();
    out.push_str(MAGIC);
    out.push('\n');
    out.push_str("arch");
    for size in net.arch() {
        out.push(' ');
        out.push_str(&size.to_string());
    }
    out.push('\n');
    for (index, layer) in net.layers().iter().enumerate() {
        out.push_str(&format!("layer {index}\n"));
        out.push_str("scale");
        for &scale in layer.scales() {
            out.push(' ');
            out.push_str(&scale.to_string());
        }
        out.push('\n');
        out.push_str("bias");
        for &bias in layer.bias() {
            out.push(' ');
            out.push(bias.to_char());
        }
        out.push('\n');
        if !layer.bias_f32().is_empty() {
            out.push_str("biasf");
            for &value in layer.bias_f32() {
                out.push(' ');
                out.push_str(&value.to_string());
            }
            out.push('\n');
        }
        for row in 0..layer.rows() {
            out.push_str("row");
            if let Some(weights) = layer.row(row) {
                for &weight in weights {
                    out.push(' ');
                    out.push(weight.to_char());
                }
            }
            out.push('\n');
        }
    }
    out.push_str("end\n");
    out
}

fn next<'a>(
    lines: &[(usize, &'a str)],
    pos: &mut usize,
    what: &'static str,
) -> Result<(usize, &'a str)> {
    let Some(&(line, text)) = lines.get(*pos) else {
        return Err(Error::MissingLine(what));
    };
    *pos += 1;
    Ok((line, text))
}

fn parse_sizes(tokens: &[&str], line: usize) -> Result<Vec<usize>> {
    let mut sizes = Vec::with_capacity(tokens.len());
    for token in tokens {
        let size = token.parse::<usize>().map_err(|_| Error::Parse {
            line,
            msg: format!("arch 维度 `{token}` 不是 usize"),
        })?;
        if size == 0 {
            return Err(Error::Parse {
                line,
                msg: "arch 维度不能为 0".into(),
            });
        }
        sizes.push(size);
    }
    if sizes.len() < 2 {
        return Err(Error::Parse {
            line,
            msg: "arch 至少需要输入维与输出维".into(),
        });
    }
    Ok(sizes)
}

fn parse_trits(tokens: &[&str], expected: usize, line: usize, what: &str) -> Result<Vec<i8>> {
    let chars: Vec<char> = if tokens.len() == 1 {
        tokens[0].chars().collect()
    } else {
        let mut chars = Vec::with_capacity(tokens.len());
        for token in tokens {
            let mut iter = token.chars();
            let Some(c) = iter.next() else {
                return Err(Error::Parse {
                    line,
                    msg: format!("{what} 里有空 token"),
                });
            };
            if iter.next().is_some() {
                return Err(Error::Parse {
                    line,
                    msg: format!("{what} token `{token}` 不是单个字符"),
                });
            }
            chars.push(c);
        }
        chars
    };
    if chars.len() != expected {
        return Err(Error::Parse {
            line,
            msg: format!("{what} 需要 {expected} 个字符，得到 {}", chars.len()),
        });
    }
    let mut out = Vec::with_capacity(expected);
    for c in chars {
        let trit = Trit::from_char(c).ok_or(Error::Parse {
            line,
            msg: format!("非法三值字符 `{c}`"),
        })?;
        out.push(trit.to_i8());
    }
    Ok(out)
}
