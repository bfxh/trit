//! AIGER ASCII 子集读写（组合逻辑，`L = 0`）。
//!
//! 字面量约定与标准一致：偶数 `2*v` 表示变量 `v`，奇数表示取反；变量 `1..=I` 是输入，
//! `I+1..=I+A` 是 AND 门。AIGER 是二值格式，往返等价只在 0/1 激励上保证；
//! 三值强加 `Xor`、`X` 常量与时序（`L > 0`）明确拒绝。

use std::collections::BTreeMap;

use trit_core::value::Ternary;

use crate::circuit::{Circuit, Gate, GateKind, Signal, validate};

/// AIGER 解析/生成错误。
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum AigerError {
    /// 某一行语法错误。
    #[error("第 {line} 行：{message}")]
    Line {
        /// 行号（1 起）。
        line: usize,
        /// 说明。
        message: String,
    },
    /// 头部不是 `aag M I L O A`。
    #[error("AIGER 头部必须是 `aag M I L O A`（ASCII）")]
    Header,
    /// 传进来的是二进制 AIGER（`aig`）。
    #[error("`aig` 是二进制 AIGER，本解析器只接受 ASCII 的 `aag`")]
    BinaryFormat,
    /// 时序锁存器不受支持。
    #[error("暂不支持时序（L = {0}）")]
    Latches(usize),
    /// 输入超出预算（文本字节数或头部计数与文本行数不符）。
    #[error("AIGER 超出预算：{0}")]
    Limit(String),
    /// 该门无法写成二值 AIGER。
    #[error("门 {0:?} 不能写成二值 AIGER")]
    Unsupported(GateKind),
    /// 电路本身不合法。
    #[error(transparent)]
    Circuit(#[from] crate::circuit::CircuitError),
}

/// AIGER 输入字节数上限（与 Verilog/SDF 同额），在解析前拒绝超大文件。
const MAX_AIGER_BYTES: usize = 1 << 20;

fn line_error(line: usize, message: impl Into<String>) -> AigerError {
    AigerError::Line {
        line,
        message: message.into(),
    }
}

fn parse_number(text: &str, line: usize, what: &str) -> Result<usize, AigerError> {
    text.parse::<usize>()
        .map_err(|_| line_error(line, format!("{what} {text:?} 不是无符号整数")))
}

struct Parser {
    circuit: Circuit,
    vars: BTreeMap<usize, String>,
    used: std::collections::BTreeSet<String>,
    next_temp: usize,
}

impl Parser {
    fn fresh(&mut self, prefix: &str) -> String {
        loop {
            let name = format!("{prefix}{}", self.next_temp);
            self.next_temp += 1;
            if self.used.insert(name.clone()) {
                return name;
            }
        }
    }

    /// 把字面量变成信号名；取反时插一个 NOT 门。
    fn signal(&mut self, literal: usize, line: usize) -> Result<String, AigerError> {
        match literal {
            0 | 1 => {
                let name = self.fresh("__aig_const");
                self.circuit.add_gate(Gate::constant(
                    if literal == 1 {
                        Ternary::True
                    } else {
                        Ternary::False
                    },
                    Signal::new(name.clone()),
                ));
                Ok(name)
            }
            _ => {
                let var = literal / 2;
                let Some(source) = self.vars.get(&var).cloned() else {
                    return Err(line_error(
                        line,
                        format!("字面量 {literal} 引用了未定义变量 {var}"),
                    ));
                };
                if literal.is_multiple_of(2) {
                    Ok(source)
                } else {
                    let name = self.fresh("__aig_not");
                    self.circuit.add_gate(Gate::new(
                        GateKind::Not,
                        vec![Signal::new(source)],
                        Signal::new(name.clone()),
                    ));
                    Ok(name)
                }
            }
        }
    }
}

fn next_nonempty<'a>(lines: &'a [&'a str], index: &mut usize) -> Option<&'a str> {
    while *index < lines.len() {
        let line = lines[*index].trim();
        *index += 1;
        if !line.is_empty() && !line.starts_with('#') {
            return Some(line);
        }
    }
    None
}

/// 解析 AIGER ASCII 组合子集。
pub fn parse_aiger(text: &str) -> Result<Circuit, AigerError> {
    if text.len() > MAX_AIGER_BYTES {
        return Err(AigerError::Limit(format!(
            "AIGER 文本 {} 字节，超过上限 {MAX_AIGER_BYTES}",
            text.len()
        )));
    }
    let lines: Vec<&str> = text.lines().collect();
    let mut index = 0usize;
    let mut header = None;
    while index < lines.len() {
        let line = lines[index].trim();
        index += 1;
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        header = Some((index, line));
        break;
    }
    let Some((header_line, header_text)) = header else {
        return Err(AigerError::Header);
    };
    let parts: Vec<&str> = header_text.split_whitespace().collect();
    if parts.len() != 6 {
        return Err(AigerError::Header);
    }
    // AIGER 标准：`aag` 是 ASCII，`aig` 是二进制。按文本解析只能支持前者；
    // 把 `aig` 当文本硬读会静默解出错内容（复审 M01）。
    if parts[0] == "aig" {
        return Err(AigerError::BinaryFormat);
    }
    if parts[0] != "aag" {
        return Err(AigerError::Header);
    }
    let m = parse_number(parts[1], header_line, "M")?;
    let inputs = parse_number(parts[2], header_line, "I")?;
    let latches = parse_number(parts[3], header_line, "L")?;
    let outputs = parse_number(parts[4], header_line, "O")?;
    let ands = parse_number(parts[5], header_line, "A")?;
    // 复审 H07：头部计数在参与运算/分配**之前**先过预算——usize 相加溢出、
    // `Vec::with_capacity(usize::MAX)` 都曾是可输入触发的 panic。
    let declared = inputs
        .checked_add(outputs)
        .and_then(|total| total.checked_add(ands))
        .ok_or_else(|| AigerError::Limit("头部计数相加溢出".to_string()))?;
    // 每条输入/输出/AND 声明至少占一行文本；声明数超过行数必然解析不出来。
    if declared > lines.len() {
        return Err(AigerError::Limit(format!(
            "头部计数 I+O+A={declared} 超过文本行数 {}",
            lines.len()
        )));
    }
    if latches > 0 {
        return Err(AigerError::Latches(latches));
    }
    if m != inputs
        .checked_add(ands)
        .ok_or_else(|| AigerError::Limit("头部计数相加溢出".to_string()))?
    {
        return Err(line_error(
            header_line,
            format!("M={m} 与 I+A={} 不一致", inputs + ands),
        ));
    }
    if outputs == 0 {
        return Err(line_error(header_line, "AIGER 至少要有 1 个输出"));
    }

    let mut parser = Parser {
        circuit: Circuit::new("aiger"),
        vars: BTreeMap::new(),
        used: std::collections::BTreeSet::new(),
        next_temp: 0,
    };

    // header_line 同时是 1 起行号与“下一行”的 0 起下标。
    let mut next_line = header_line;
    for _ in 0..inputs {
        let Some(text) = next_nonempty(&lines, &mut next_line) else {
            return Err(line_error(next_line, "输入声明不完整"));
        };
        let line_no = next_line;
        let literal = parse_number(text, line_no, "输入字面量")?;
        if !literal.is_multiple_of(2) {
            return Err(line_error(line_no, "输入字面量必须是偶数"));
        }
        let var = literal / 2;
        if var == 0 || var > m {
            return Err(line_error(line_no, format!("输入变量 {var} 超出 1..={m}")));
        }
        let name = format!("in{var}");
        if parser.vars.insert(var, name.clone()).is_some() {
            return Err(line_error(line_no, format!("变量 {var} 重复声明")));
        }
        parser.used.insert(name.clone());
        parser.circuit.add_input(Signal::new(name));
    }

    // declared ≤ lines.len() 已由上面的预算检查保证，这里不必再按头部计数预留。
    let mut output_literals: Vec<(usize, usize)> = Vec::new();
    for _ in 0..outputs {
        let Some(text) = next_nonempty(&lines, &mut next_line) else {
            return Err(line_error(next_line, "输出声明不完整"));
        };
        let line_no = next_line;
        output_literals.push((parse_number(text, line_no, "输出字面量")?, line_no));
    }

    for _ in 0..ands {
        let Some(text) = next_nonempty(&lines, &mut next_line) else {
            return Err(line_error(next_line, "AND 门声明不完整"));
        };
        let line_no = next_line;
        let tokens: Vec<&str> = text.split_whitespace().collect();
        if tokens.len() != 3 {
            return Err(line_error(line_no, "AND 行必须是 `lhs rhs0 rhs1`"));
        }
        let lhs = parse_number(tokens[0], line_no, "AND 输出")?;
        let rhs0 = parse_number(tokens[1], line_no, "AND 输入")?;
        let rhs1 = parse_number(tokens[2], line_no, "AND 输入")?;
        if !lhs.is_multiple_of(2) {
            return Err(line_error(line_no, "AND 输出字面量必须是偶数"));
        }
        let var = lhs / 2;
        if var <= inputs || var > m {
            return Err(line_error(
                line_no,
                format!("AND 变量 {var} 必须落在 {}..={m}", inputs + 1),
            ));
        }
        if parser.vars.contains_key(&var) {
            return Err(line_error(line_no, format!("变量 {var} 重复定义")));
        }
        let left = parser.signal(rhs0, line_no)?;
        let right = parser.signal(rhs1, line_no)?;
        let name = format!("__aig{var}");
        parser.used.insert(name.clone());
        parser.circuit.add_gate(Gate::new(
            GateKind::And,
            vec![Signal::new(left), Signal::new(right)],
            Signal::new(name.clone()),
        ));
        parser.vars.insert(var, name);
    }

    for (slot, (literal, line_no)) in output_literals.into_iter().enumerate() {
        let source = parser.signal(literal, line_no)?;
        let name = format!("out{slot}");
        parser.used.insert(name.clone());
        parser.circuit.add_gate(Gate::new(
            GateKind::Buf,
            vec![Signal::new(source)],
            Signal::new(name.clone()),
        ));
        parser.circuit.add_output(Signal::new(name));
    }
    Ok(parser.circuit)
}

/// 把组合网表写成 AIGER ASCII。三值强加 `Xor` 与 `X` 常量明确拒绝。
pub fn to_aiger(circuit: &Circuit) -> Result<String, AigerError> {
    validate(circuit)?;
    let mut lit_of: BTreeMap<String, usize> = BTreeMap::new();
    for (index, input) in circuit.inputs.iter().enumerate() {
        lit_of.insert(input.name().to_string(), 2 * (index + 1));
    }
    let mut and_lines: Vec<String> = Vec::new();
    let mut next_var = circuit.inputs.len() + 1;

    fn emit_and(lines: &mut Vec<String>, next_var: &mut usize, a: usize, b: usize) -> usize {
        let var = *next_var;
        *next_var += 1;
        lines.push(format!("{} {a} {b}", 2 * var));
        2 * var
    }

    fn lookup(lit_of: &BTreeMap<String, usize>, name: &str) -> Result<usize, AigerError> {
        lit_of.get(name).copied().ok_or_else(|| AigerError::Line {
            line: 0,
            message: format!("信号 {name:?} 未定义"),
        })
    }

    for gate in &circuit.gates {
        let output = gate.output.name().to_string();
        let a = gate.inputs.first().map(Signal::name).unwrap_or("");
        let b = gate.inputs.get(1).map(Signal::name).unwrap_or(a);
        let literal = match gate.kind {
            GateKind::Const => match gate.const_value.unwrap_or(Ternary::False) {
                Ternary::False => 0,
                Ternary::True => 1,
                Ternary::Unknown => return Err(AigerError::Unsupported(GateKind::Const)),
            },
            GateKind::Buf => lookup(&lit_of, a)?,
            GateKind::Not => lookup(&lit_of, a)? ^ 1,
            GateKind::And => emit_and(
                &mut and_lines,
                &mut next_var,
                lookup(&lit_of, a)?,
                lookup(&lit_of, b)?,
            ),
            GateKind::Nand => {
                emit_and(
                    &mut and_lines,
                    &mut next_var,
                    lookup(&lit_of, a)?,
                    lookup(&lit_of, b)?,
                ) ^ 1
            }
            GateKind::Or => {
                emit_and(
                    &mut and_lines,
                    &mut next_var,
                    lookup(&lit_of, a)? ^ 1,
                    lookup(&lit_of, b)? ^ 1,
                ) ^ 1
            }
            GateKind::Nor => emit_and(
                &mut and_lines,
                &mut next_var,
                lookup(&lit_of, a)? ^ 1,
                lookup(&lit_of, b)? ^ 1,
            ),
            GateKind::XorClassic => {
                let left = lookup(&lit_of, a)?;
                let right = lookup(&lit_of, b)?;
                let t = emit_and(&mut and_lines, &mut next_var, left, right ^ 1);
                let u = emit_and(&mut and_lines, &mut next_var, left ^ 1, right);
                let v = emit_and(&mut and_lines, &mut next_var, t ^ 1, u ^ 1);
                v ^ 1
            }
            GateKind::Xor => return Err(AigerError::Unsupported(GateKind::Xor)),
        };
        lit_of.insert(output, literal);
    }

    let output_literals: Vec<usize> = circuit
        .outputs
        .iter()
        .map(|output| {
            lit_of
                .get(output.name())
                .copied()
                .ok_or_else(|| AigerError::Line {
                    line: 0,
                    message: format!("输出 {:?} 未定义", output.name()),
                })
        })
        .collect::<Result<_, _>>()?;
    let ands = next_var - circuit.inputs.len() - 1;
    let mut out = format!(
        "aag {} {} 0 {} {}\n",
        circuit.inputs.len() + ands,
        circuit.inputs.len(),
        circuit.outputs.len(),
        ands
    );
    for index in 0..circuit.inputs.len() {
        out.push_str(&format!("{}\n", 2 * (index + 1)));
    }
    for literal in output_literals {
        out.push_str(&format!("{literal}\n"));
    }
    for line in and_lines {
        out.push_str(&line);
        out.push('\n');
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::circuit::propagate;
    use trit_core::algebra::Algebra;

    fn check(circuit: &Circuit) {
        let text = to_aiger(circuit).unwrap();
        let again = parse_aiger(&text).unwrap();
        assert_eq!(again.inputs.len(), circuit.inputs.len());
        assert_eq!(again.outputs.len(), circuit.outputs.len());
        let total = 1u64 << circuit.input_count();
        for combo in 0..total {
            let mut assignment = std::collections::BTreeMap::new();
            for (bit, input) in circuit.inputs.iter().enumerate() {
                assignment.insert(
                    input.name().to_string(),
                    if (combo >> bit) & 1 == 0 {
                        Ternary::False
                    } else {
                        Ternary::True
                    },
                );
            }
            let left = propagate(circuit, &assignment, Algebra::Kleene).unwrap();
            let mut parsed_assignment = std::collections::BTreeMap::new();
            for (bit, input) in again.inputs.iter().enumerate() {
                parsed_assignment.insert(
                    input.name().to_string(),
                    if (combo >> bit) & 1 == 0 {
                        Ternary::False
                    } else {
                        Ternary::True
                    },
                );
            }
            let right = propagate(&again, &parsed_assignment, Algebra::Kleene).unwrap();
            for (index, output) in circuit.outputs.iter().enumerate() {
                assert_eq!(
                    left.signals.get(output.name()),
                    right.signals.get(&format!("out{index}")),
                    "输出 {} combo {combo}\n{text}",
                    output.name()
                );
            }
        }
    }

    fn gate(kind: GateKind, inputs: &[&str], output: &str) -> Circuit {
        let mut circuit = Circuit::new("m");
        for name in inputs {
            circuit.add_input(Signal::new(*name));
        }
        circuit.add_gate(Gate::new(
            kind,
            inputs.iter().map(|name| Signal::new(*name)).collect(),
            Signal::new(output),
        ));
        circuit.add_output(Signal::new(output));
        circuit
    }

    #[test]
    fn parses_plain_and() {
        let text = "aag 3 2 0 1 1\n2\n4\n6\n6 2 4\n";
        let circuit = parse_aiger(text).unwrap();
        let mut assignment = std::collections::BTreeMap::new();
        assignment.insert("in1".to_string(), Ternary::True);
        assignment.insert("in2".to_string(), Ternary::True);
        let values = propagate(&circuit, &assignment, Algebra::Kleene).unwrap();
        assert_eq!(values.signals.get("out0"), Some(&Ternary::True));
    }

    #[test]
    fn round_trip_all_binary_gate_kinds() {
        check(&gate(GateKind::And, &["a", "b"], "y"));
        check(&gate(GateKind::Or, &["a", "b"], "y"));
        check(&gate(GateKind::Nand, &["a", "b"], "y"));
        check(&gate(GateKind::Nor, &["a", "b"], "y"));
        check(&gate(GateKind::XorClassic, &["a", "b"], "y"));
        check(&gate(GateKind::Not, &["a"], "y"));
        check(&gate(GateKind::Buf, &["a"], "y"));
    }

    #[test]
    fn constant_outputs_round_trip() {
        let mut circuit = Circuit::new("consts");
        circuit.add_input(Signal::new("a"));
        circuit.add_gate(Gate::constant(Ternary::False, Signal::new("z")));
        circuit.add_gate(Gate::constant(Ternary::True, Signal::new("o")));
        circuit.add_output(Signal::new("z"));
        circuit.add_output(Signal::new("o"));
        let text = to_aiger(&circuit).unwrap();
        let again = parse_aiger(&text).unwrap();
        let mut assignment = std::collections::BTreeMap::new();
        assignment.insert("in1".to_string(), Ternary::True);
        let values = propagate(&again, &assignment, Algebra::Kleene).unwrap();
        assert_eq!(values.signals.get("out0"), Some(&Ternary::False));
        assert_eq!(values.signals.get("out1"), Some(&Ternary::True));
    }

    #[test]
    fn rejects_latches_strong_xor_and_bad_header() {
        let text = "aag 2 1 1 1 0\n2\n0\n4\n";
        assert_eq!(parse_aiger(text).unwrap_err(), AigerError::Latches(1));
        assert_eq!(
            to_aiger(&gate(GateKind::Xor, &["a", "b"], "y")).unwrap_err(),
            AigerError::Unsupported(GateKind::Xor)
        );
        assert_eq!(parse_aiger("nope").unwrap_err(), AigerError::Header);
    }

    #[test]
    fn ascii_header_is_aag_and_binary_aig_is_rejected() {
        // 复审 M01：AIGER 标准里 ASCII 是 `aag`、`aig` 是二进制。
        let text = "aag 3 2 0 1 1
2
4
6
6 2 4
";
        assert!(parse_aiger(text).is_ok(), "标准 aag 被拒绝");
        assert_eq!(
            parse_aiger(
                "aig 3 2 0 1 1
2
4
6
6 2 4
"
            )
            .unwrap_err(),
            AigerError::BinaryFormat
        );
        assert_eq!(
            parse_aiger(
                "xor 1 0 0 1 0
2
2
"
            )
            .unwrap_err(),
            AigerError::Header
        );
        // 写回也必须是 `aag`，否则往返第一步就会自拒。
        let circuit = parse_aiger(text).unwrap();
        assert!(to_aiger(&circuit).unwrap().starts_with("aag "));
    }

    #[test]
    fn aiger_budget_rejects_huge_headers_and_oversized_input() {
        // 复审 H07：头部 O 计数曾直接 Vec::with_capacity——usize::MAX 触发
        // capacity overflow panic；I+A 相加也会溢出。现在都必须先过预算。
        assert!(matches!(
            parse_aiger("aag 0 0 0 18446744073709551615 0"),
            Err(AigerError::Limit(_))
        ));
        assert!(matches!(
            parse_aiger("aag 0 18446744073709551615 0 1 1"),
            Err(AigerError::Limit(_))
        ));
        // 声明数超过文本行数：必然解析不出来，提前拒绝。
        assert!(matches!(
            parse_aiger("aag 5 1000 0 1 5\n2\n6\n8 2 4\n"),
            Err(AigerError::Limit(_))
        ));
        // 文本字节上限。
        let oversized = "aig\n".to_string() + &" ".repeat(MAX_AIGER_BYTES + 1);
        assert!(matches!(parse_aiger(&oversized), Err(AigerError::Limit(_))));
    }

    #[test]
    fn rejects_out_of_range_literal() {
        let text = "aag 3 2 0 1 1\n2\n4\n6\n6 2 8\n";
        assert!(matches!(
            parse_aiger(text).unwrap_err(),
            AigerError::Line { .. }
        ));
    }
}
