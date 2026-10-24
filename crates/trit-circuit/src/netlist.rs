//! 网表文本格式：`input` / `output` / `gate` / `not` / `buf` / `const`。

use serde::de::Error as _;

use crate::circuit::{Circuit, CircuitError, Gate, GateKind, Signal, validate};
use trit_core::value::Ternary;

/// 网表解析错误。
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum NetlistError {
    /// 某一行语法错误。
    #[error("第 {line} 行：{message}")]
    Line {
        /// 行号（1 起）。
        line: usize,
        /// 说明。
        message: String,
    },
    /// 没有声明主输入。
    #[error("网表没有主输入")]
    NoInputs,
    /// 没有声明主输出。
    #[error("网表没有主输出")]
    NoOutputs,
    /// 超出解析预算（文本字节 / 网络数 / 门数）。
    #[error("网表超出预算：{0}")]
    Limit(String),
}

/// 文本解析预算；默认值与 Verilog 同量级，服务端可显式收紧。
#[derive(Debug, Clone, Copy)]
pub struct Limits {
    /// 文本字节上限。
    pub bytes: usize,
    /// 网络（输入 + 输出）上限。
    pub nets: usize,
    /// 门数上限。
    pub gates: usize,
}

impl Limits {
    /// 默认预算：1 MiB / 10 万网络 / 10 万门。
    pub const DEFAULT: Self = Self {
        bytes: 1 << 20,
        nets: 100_000,
        gates: 100_000,
    };
}

fn line_error(line: usize, message: impl Into<String>) -> NetlistError {
    NetlistError::Line {
        line,
        message: message.into(),
    }
}

/// 解析网表文本（默认预算）。
pub fn parse_netlist(text: &str) -> Result<Circuit, NetlistError> {
    parse_netlist_with_limits(text, Limits::DEFAULT)
}

/// 解析网表文本；分配前先过预算（复审 M11：两个公开文本 parser 都曾无上限）。
pub fn parse_netlist_with_limits(text: &str, limits: Limits) -> Result<Circuit, NetlistError> {
    if text.len() > limits.bytes {
        return Err(NetlistError::Limit(format!(
            "文本 {} 字节，超过上限 {}",
            text.len(),
            limits.bytes
        )));
    }
    let mut circuit = Circuit::new("netlist");
    for (lineno, raw) in text.lines().enumerate() {
        if circuit.gates.len() > limits.gates {
            return Err(NetlistError::Limit(format!(
                "门数超过上限 {}",
                limits.gates
            )));
        }
        if circuit.inputs.len() + circuit.outputs.len() > limits.nets {
            return Err(NetlistError::Limit(format!(
                "网络数超过上限 {}",
                limits.nets
            )));
        }
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let number = lineno + 1;
        let parts: Vec<&str> = line.split_whitespace().collect();
        match parts[0] {
            "input" => {
                if parts.len() != 2 {
                    return Err(line_error(number, "input 需要 1 个参数"));
                }
                circuit.add_input(Signal::new(parts[1]));
            }
            "output" => {
                if parts.len() != 2 {
                    return Err(line_error(number, "output 需要 1 个参数"));
                }
                circuit.add_output(Signal::new(parts[1]));
            }
            "gate" => {
                if parts.len() != 5 {
                    return Err(line_error(
                        number,
                        "gate 语法为 `gate <类型> <输入1> <输入2> <输出>`",
                    ));
                }
                let kind = GateKind::parse(parts[1])
                    .ok_or_else(|| line_error(number, format!("未知门类型 {:?}", parts[1])))?;
                let inputs = vec![Signal::new(parts[2]), Signal::new(parts[3])];
                circuit.add_gate(Gate::new(kind, inputs, Signal::new(parts[4])));
            }
            "not" => {
                if parts.len() != 3 {
                    return Err(line_error(number, "not 语法为 `not <输入> <输出>`"));
                }
                circuit.add_gate(Gate::new(
                    GateKind::Not,
                    vec![Signal::new(parts[1])],
                    Signal::new(parts[2]),
                ));
            }
            "buf" => {
                if parts.len() != 3 {
                    return Err(line_error(number, "buf 语法为 `buf <输入> <输出>`"));
                }
                circuit.add_gate(Gate::new(
                    GateKind::Buf,
                    vec![Signal::new(parts[1])],
                    Signal::new(parts[2]),
                ));
            }
            "const" => {
                if parts.len() != 3 {
                    return Err(line_error(number, "const 语法为 `const <0|1|X> <输出>`"));
                }
                let mut chars = parts[1].chars();
                let ch = chars
                    .next()
                    .ok_or_else(|| line_error(number, "const 缺取值"))?;
                if chars.next().is_some() {
                    return Err(line_error(number, "const 的取值只能是单个字符 0/1/X"));
                }
                let value = Ternary::parse_char(ch)
                    .ok_or_else(|| line_error(number, format!("const 取值 {ch:?} 不是 0/1/X")))?;
                circuit.add_gate(Gate::constant(value, Signal::new(parts[2])));
            }
            other => return Err(line_error(number, format!("未知指令 {other:?}"))),
        }
    }
    if circuit.inputs.is_empty() {
        return Err(NetlistError::NoInputs);
    }
    if circuit.outputs.is_empty() {
        return Err(NetlistError::NoOutputs);
    }
    // 最后一行的新增也要复核（循环只在下一轮开头检查）。
    if circuit.gates.len() > limits.gates {
        return Err(NetlistError::Limit(format!(
            "门数超过上限 {}",
            limits.gates
        )));
    }
    if circuit.inputs.len() + circuit.outputs.len() > limits.nets {
        return Err(NetlistError::Limit(format!(
            "网络数超过上限 {}",
            limits.nets
        )));
    }
    Ok(circuit)
}

/// 当前网表 JSON 格式版本；缺省 `format` 视为旧版 0。
const JSON_FORMAT: u32 = 2;

#[derive(serde::Serialize)]
struct NetlistJson<'a> {
    format: u32,
    #[serde(flatten)]
    circuit: &'a Circuit,
}

#[derive(serde::Deserialize)]
struct NetlistJsonOwned {
    #[serde(default)]
    format: u32,
    #[serde(flatten)]
    circuit: Circuit,
}

/// 序列化为 JSON：带 `format` 版本字段，其余字段与 [`Circuit`] 一致。
pub fn to_json(circuit: &Circuit) -> Result<String, serde_json::Error> {
    serde_json::to_string_pretty(&NetlistJson {
        format: JSON_FORMAT,
        circuit,
    })
}

/// 从 JSON 读取电路；旧版（无 `format`）按 0 接受，更新版本拒绝。
pub fn from_json(text: &str) -> Result<Circuit, serde_json::Error> {
    let parsed: NetlistJsonOwned = serde_json::from_str(text)?;
    if parsed.format > JSON_FORMAT {
        return Err(serde_json::Error::custom(format!(
            "不支持的网表 JSON 格式版本 {}（当前支持 0..={JSON_FORMAT}）",
            parsed.format
        )));
    }
    Ok(parsed.circuit)
}

/// 把电路序列化回网表文本（`parse_netlist` 的逆操作）。
pub fn to_netlist(circuit: &Circuit) -> Result<String, CircuitError> {
    validate(circuit)?;
    let mut out = String::new();
    for input in &circuit.inputs {
        out.push_str(&format!("input {}\n", input.name()));
    }
    for gate in &circuit.gates {
        let output = gate.output.name();
        match gate.kind {
            GateKind::Const => {
                let value = gate.const_value.unwrap_or(Ternary::False);
                out.push_str(&format!("const {} {output}\n", value.as_char()));
            }
            GateKind::Buf => out.push_str(&format!("buf {} {output}\n", gate.inputs[0].name())),
            GateKind::Not => out.push_str(&format!("not {} {output}\n", gate.inputs[0].name())),
            kind => out.push_str(&format!(
                "gate {} {} {} {output}\n",
                kind.as_str(),
                gate.inputs[0].name(),
                gate.inputs[1].name()
            )),
        }
    }
    for output in &circuit.outputs {
        out.push_str(&format!("output {}\n", output.name()));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn budgets_reject_oversized_input() {
        // 复审 M11：公开文本 parser 原先无任何字节/网络/门上界。
        let text = "input a\noutput y\ngate AND a a y\n";
        assert!(parse_netlist(text).is_ok(), "默认预算内必须能解析");
        let tiny_bytes = Limits {
            bytes: 8,
            nets: 10,
            gates: 10,
        };
        assert!(matches!(
            parse_netlist_with_limits(text, tiny_bytes),
            Err(NetlistError::Limit(_))
        ));
        let zero_gates = Limits {
            bytes: 1 << 20,
            nets: 10,
            gates: 0,
        };
        assert!(matches!(
            parse_netlist_with_limits(text, zero_gates),
            Err(NetlistError::Limit(_))
        ));
        let one_net = Limits {
            bytes: 1 << 20,
            nets: 1,
            gates: 10,
        };
        assert!(matches!(
            parse_netlist_with_limits(text, one_net),
            Err(NetlistError::Limit(_))
        ));
    }

    use crate::circuit::{GateKind, analyze};
    use trit_core::algebra::Algebra;

    const AND: &str = "input a\ninput b\noutput y\ngate AND a b y\n";

    #[test]
    fn parses_basic_netlist() {
        let circuit = parse_netlist(AND).unwrap();
        assert_eq!(circuit.input_count(), 2);
        assert_eq!(circuit.outputs.len(), 1);
        assert_eq!(circuit.gates[0].kind, GateKind::And);
    }

    #[test]
    fn export_rejects_structurally_bad_circuit() {
        let mut circuit = Circuit::new("bad");
        circuit.add_input(Signal::new("a"));
        circuit.add_gate(Gate::new(GateKind::Not, vec![], Signal::new("y")));
        circuit.add_output(Signal::new("y"));
        assert!(to_netlist(&circuit).is_err(), "非法元数不能直接导出");
    }

    #[test]
    fn reports_line_numbers() {
        let err = parse_netlist("input a\noutput y\nbogus a y\n").unwrap_err();
        assert!(matches!(err, NetlistError::Line { line: 3, .. }), "{err}");
        let err = parse_netlist("input a\ninput b\ngate AND a b\n").unwrap_err();
        assert!(matches!(err, NetlistError::Line { line: 3, .. }), "{err}");
    }

    #[test]
    fn rejects_missing_ports() {
        assert_eq!(
            parse_netlist("output y\ngate AND a b y\n").unwrap_err(),
            NetlistError::NoInputs
        );
        assert_eq!(
            parse_netlist("input a\ninput b\ngate AND a b y\n").unwrap_err(),
            NetlistError::NoOutputs
        );
    }

    #[test]
    fn json_round_trip() {
        let circuit = parse_netlist(AND).unwrap();
        let json = to_json(&circuit).unwrap();
        assert!(json.contains("\"inputs\""), "{json}");
        assert!(json.contains("\"And\""), "{json}");
        assert_eq!(from_json(&json).unwrap(), circuit);
    }

    #[test]
    fn json_format_is_versioned_and_rejects_newer() {
        let circuit = parse_netlist(AND).unwrap();
        let json = to_json(&circuit).unwrap();
        assert!(json.contains("\"format\": 2"), "{json}");

        let legacy = serde_json::to_string(&circuit).unwrap();
        assert!(!legacy.contains("\"format\""), "{legacy}");
        assert_eq!(from_json(&legacy).unwrap(), circuit);

        let newer = json.replace("\"format\": 2", "\"format\": 3");
        let err = from_json(&newer).unwrap_err();
        assert!(err.to_string().contains("格式版本 3"), "{err}");
    }

    #[test]
    fn round_trip_preserves_behavior() {
        let text =
            "input a\ninput b\nconst 1 one\nnot a na\ngate AND na b n\ngate OR n one y\noutput y\n";
        let circuit = parse_netlist(text).unwrap();
        let again = parse_netlist(&to_netlist(&circuit).unwrap()).unwrap();
        assert_eq!(circuit, again);
        assert_eq!(
            analyze(&circuit, Algebra::Kleene).unwrap().faults,
            analyze(&again, Algebra::Kleene).unwrap().faults
        );
    }
}
