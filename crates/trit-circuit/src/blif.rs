//! BLIF 子集读写：`.model` / `.inputs` / `.outputs` / `.names`（组合逻辑）与 `.end`。
//!
//! `parse_blif` 把每个 `.names` 的 on-set cover 展开成 AND/OR/NOT 网络。
//! BLIF 本身是二值格式：往返等价只在 0/1 激励上保证；X 输入下展开网络按 Kleene 传播，
//! 可能与 `NAND` / `NOR` 门自身的 X-optimism 不同（例如 NAND(X,0)=1，而 cover 展开得到 X）。
//! `to_blif` 只导出能写成二值真值表的门；三值强加 `Xor` 与 `X` 常量明确拒绝。

use std::collections::BTreeSet;

use trit_core::value::Ternary;

use crate::circuit::{Circuit, Gate, GateKind, Signal};

/// BLIF 解析/生成错误。
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum BlifError {
    /// 某一行语法错误。
    #[error("第 {line} 行：{message}")]
    Line {
        /// 行号（1 起）。
        line: usize,
        /// 说明。
        message: String,
    },
    /// 缺少 `.model`。
    #[error("BLIF 缺少 .model")]
    NoModel,
    /// 没有 `.inputs`。
    #[error("BLIF 没有 .inputs")]
    NoInputs,
    /// 没有 `.outputs`。
    #[error("BLIF 没有 .outputs")]
    NoOutputs,
    /// 该门无法写成二值 BLIF。
    #[error("门 {0:?} 不能写成二值 BLIF")]
    Unsupported(GateKind),
    /// 超出解析预算（文本字节 / 网络数 / 门数 / cover 项数）。
    #[error("BLIF 超出预算：{0}")]
    Limit(String),
    /// 电路结构不合法（元数/常量/重名等），导出前由 `circuit::validate` 拦下。
    #[error("电路结构不合法：{0}")]
    Structure(String),
    /// 标识符写不进 BLIF（空、含空白或以 `.` 开头会与指令混淆）。
    #[error("标识符不能写进 BLIF：{0:?}（不能为空、含空白或以 `.` 开头）")]
    Name(String),
}

/// BLIF 文本解析预算。
#[derive(Debug, Clone, Copy)]
pub struct Limits {
    /// 文本字节上限。
    pub bytes: usize,
    /// 网络数上限。
    pub nets: usize,
    /// 门数上限。
    pub gates: usize,
    /// cover 行总数上限。
    pub cover_terms: usize,
}

impl Limits {
    /// 默认预算：1 MiB / 10 万网络 / 10 万门 / 20 万 cover 行。
    pub const DEFAULT: Self = Self {
        bytes: 1 << 20,
        nets: 100_000,
        gates: 100_000,
        cover_terms: 200_000,
    };
}

fn line_error(line: usize, message: impl Into<String>) -> BlifError {
    BlifError::Line {
        line,
        message: message.into(),
    }
}

fn strip_comment(line: &str) -> &str {
    match line.find('#') {
        Some(index) => &line[..index],
        None => line,
    }
}

type Row = (Vec<Option<bool>>, bool);

struct NamesBlock {
    inputs: Vec<String>,
    output: String,
    rows: Vec<Row>,
}

fn build_cover(
    circuit: &mut Circuit,
    used: &mut BTreeSet<String>,
    counter: &mut usize,
    block: NamesBlock,
) -> Result<(), BlifError> {
    let mut fresh = || loop {
        let name = format!("__blif{counter}");
        *counter += 1;
        if used.insert(name.clone()) {
            return name;
        }
    };

    let mut on_set: Vec<Vec<Option<bool>>> = block
        .rows
        .iter()
        .filter(|(_, value)| *value)
        .map(|(literals, _)| literals.clone())
        .collect();
    if on_set.is_empty() {
        circuit.add_gate(Gate::constant(
            Ternary::False,
            Signal::new(block.output.clone()),
        ));
        return Ok(());
    }
    if on_set
        .iter()
        .any(|literals| literals.iter().all(Option::is_none))
    {
        circuit.add_gate(Gate::constant(
            Ternary::True,
            Signal::new(block.output.clone()),
        ));
        return Ok(());
    }

    let mut terms: Vec<String> = Vec::new();
    for literals in on_set.drain(..) {
        let mut parts: Vec<String> = Vec::new();
        for (name, value) in block.inputs.iter().zip(literals) {
            match value {
                None => {}
                Some(true) => parts.push(name.clone()),
                Some(false) => {
                    let inverted = fresh();
                    circuit.add_gate(Gate::new(
                        GateKind::Not,
                        vec![Signal::new(name)],
                        Signal::new(inverted.clone()),
                    ));
                    parts.push(inverted);
                }
            }
        }
        let term = match parts.len() {
            0 => {
                let name = fresh();
                circuit.add_gate(Gate::constant(Ternary::True, Signal::new(name.clone())));
                name
            }
            1 => parts[0].clone(),
            _ => {
                let mut current = parts[0].clone();
                for part in &parts[1..] {
                    let next = fresh();
                    circuit.add_gate(Gate::new(
                        GateKind::And,
                        vec![Signal::new(current), Signal::new(part)],
                        Signal::new(next.clone()),
                    ));
                    current = next;
                }
                current
            }
        };
        terms.push(term);
    }

    match terms.len() {
        1 => {
            circuit.add_gate(Gate::new(
                GateKind::Buf,
                vec![Signal::new(terms[0].clone())],
                Signal::new(block.output.clone()),
            ));
        }
        _ => {
            let mut current = terms[0].clone();
            for (index, term) in terms[1..].iter().enumerate() {
                let target = if index + 2 == terms.len() {
                    block.output.clone()
                } else {
                    fresh()
                };
                circuit.add_gate(Gate::new(
                    GateKind::Or,
                    vec![Signal::new(current), Signal::new(term)],
                    Signal::new(target.clone()),
                ));
                current = target;
            }
        }
    }
    Ok(())
}

/// 解析 BLIF 组合子集。`.latch` 等时序语句明确拒绝。
pub fn parse_blif(text: &str) -> Result<Circuit, BlifError> {
    parse_blif_with_limits(text, Limits::DEFAULT)
}

/// 解析 BLIF；分配前先过预算（复审 M11）。
pub fn parse_blif_with_limits(text: &str, limits: Limits) -> Result<Circuit, BlifError> {
    if text.len() > limits.bytes {
        return Err(BlifError::Limit(format!(
            "文本 {} 字节，超过上限 {}",
            text.len(),
            limits.bytes
        )));
    }
    let mut cover_terms = 0usize;
    let mut circuit = Circuit::new("blif");
    let mut have_model = false;
    let mut declared: BTreeSet<String> = BTreeSet::new();
    let mut counter = 0usize;
    let lines: Vec<&str> = text.lines().collect();
    let mut index = 0usize;
    while index < lines.len() {
        let line_no = index + 1;
        let line = strip_comment(lines[index]).trim();
        index += 1;
        if line.is_empty() {
            continue;
        }
        if circuit.gates.len() > limits.gates {
            return Err(BlifError::Limit(format!("门数超过上限 {}", limits.gates)));
        }
        if declared.len() > limits.nets {
            return Err(BlifError::Limit(format!("网络数超过上限 {}", limits.nets)));
        }
        if let Some(rest) = line.strip_prefix(".model") {
            let name = rest.trim();
            if name.is_empty() {
                return Err(line_error(line_no, ".model 缺少名字"));
            }
            circuit.name = name.to_string();
            have_model = true;
            continue;
        }
        if let Some(rest) = line.strip_prefix(".inputs") {
            for name in rest.split_whitespace() {
                if declared.insert(name.to_string()) {
                    circuit.add_input(Signal::new(name));
                }
            }
            continue;
        }
        if let Some(rest) = line.strip_prefix(".outputs") {
            for name in rest.split_whitespace() {
                declared.insert(name.to_string());
                circuit.add_output(Signal::new(name));
            }
            continue;
        }
        if line.starts_with(".latch") || line.starts_with(".r") {
            return Err(line_error(line_no, "暂不支持时序元件（.latch）"));
        }
        if let Some(rest) = line.strip_prefix(".names") {
            let mut tokens = rest.split_whitespace();
            let mut names: Vec<String> = tokens.by_ref().map(str::to_string).collect();
            let Some(output) = names.pop() else {
                return Err(line_error(line_no, ".names 缺少输出名"));
            };
            let mut rows: Vec<Row> = Vec::new();
            while index < lines.len() {
                let peek = strip_comment(lines[index]).trim();
                if peek.is_empty() {
                    index += 1;
                    continue;
                }
                if peek.starts_with('.') {
                    break;
                }
                index += 1;
                let mut parts: Vec<&str> = peek.split_whitespace().collect();
                let row_no = index;
                let Some(value) = parts.pop() else {
                    return Err(line_error(row_no, "空行不能作为 .names 的一行"));
                };
                let row_value = match value {
                    "1" => true,
                    "0" => false,
                    other => {
                        return Err(line_error(
                            row_no,
                            format!("cover 行输出 {other:?} 只能是 0/1"),
                        ));
                    }
                };
                // 兼容 `11 1`（字面量连写）与 `1 1 1`（空格分开）两种写法。
                let literal_text: Vec<String> = if parts.len() == 1
                    && names.len() > 1
                    && parts[0].len() == names.len()
                    && parts[0].chars().all(|ch| matches!(ch, '0' | '1' | '-'))
                {
                    parts[0].chars().map(|ch| ch.to_string()).collect()
                } else {
                    parts.iter().map(|part| part.to_string()).collect()
                };
                if literal_text.len() != names.len() {
                    return Err(line_error(
                        row_no,
                        format!(
                            "cover 行有 {} 个字面量，但 .names 有 {} 个输入",
                            literal_text.len(),
                            names.len()
                        ),
                    ));
                }
                let mut literals = Vec::with_capacity(literal_text.len());
                for part in &literal_text {
                    literals.push(match part.as_str() {
                        "-" => None,
                        "0" => Some(false),
                        "1" => Some(true),
                        other => {
                            return Err(line_error(
                                row_no,
                                format!("cover 字面量 {other:?} 只能是 0/1/-"),
                            ));
                        }
                    });
                }
                cover_terms += 1;
                if cover_terms > limits.cover_terms {
                    return Err(BlifError::Limit(format!(
                        "cover 行超过上限 {}",
                        limits.cover_terms
                    )));
                }
                rows.push((literals, row_value));
            }
            if rows.is_empty() {
                rows.push((vec![None; names.len()], false));
            }
            declared.insert(output.clone());
            build_cover(
                &mut circuit,
                &mut declared,
                &mut counter,
                NamesBlock {
                    inputs: names,
                    output,
                    rows,
                },
            )?;
            continue;
        }
        if line == ".end" {
            break;
        }
        if line.starts_with('.') {
            let directive = line.split_whitespace().next().unwrap_or(".");
            return Err(line_error(line_no, format!("未知指令 {directive:?}")));
        }
        return Err(line_error(line_no, "cover 行必须跟在 .names 之后"));
    }
    if circuit.gates.len() > limits.gates {
        return Err(BlifError::Limit(format!("门数超过上限 {}", limits.gates)));
    }
    if declared.len() > limits.nets {
        return Err(BlifError::Limit(format!("网络数超过上限 {}", limits.nets)));
    }
    if !have_model {
        return Err(BlifError::NoModel);
    }
    if circuit.inputs.is_empty() {
        return Err(BlifError::NoInputs);
    }
    if circuit.outputs.is_empty() {
        return Err(BlifError::NoOutputs);
    }
    // 解析方向同样要过结构校验：否则会返回"能解析但导不回去"的电路（忠实输入非法）。
    crate::circuit::validate(&circuit).map_err(|error| BlifError::Structure(error.to_string()))?;
    Ok(circuit)
}

/// 把组合网表写成 BLIF。三值强加 `Xor` 与 `X` 常量明确拒绝。
pub fn to_blif(circuit: &Circuit) -> Result<String, BlifError> {
    // 复审 L03：导出前先跑与 Verilog 导出同一套结构校验，再检查标识符——
    // 否则手工构造的非法电路会"成功"导出一个读不回来的文件。
    crate::circuit::validate(circuit).map_err(|error| BlifError::Structure(error.to_string()))?;
    let gate_names = || {
        circuit.gates.iter().flat_map(|gate| {
            std::iter::once(gate.output.name()).chain(gate.inputs.iter().map(Signal::name))
        })
    };
    for name in std::iter::once(circuit.name.as_str())
        .chain(circuit.inputs.iter().map(Signal::name))
        .chain(circuit.outputs.iter().map(Signal::name))
        .chain(gate_names())
    {
        if name.is_empty() || name.chars().any(char::is_whitespace) || name.starts_with('.') {
            return Err(BlifError::Name(name.to_string()));
        }
    }
    let mut out = format!(".model {}\n", circuit.name);
    let inputs: Vec<&str> = circuit.inputs.iter().map(Signal::name).collect();
    let outputs: Vec<&str> = circuit.outputs.iter().map(Signal::name).collect();
    out.push_str(&format!(".inputs {}\n", inputs.join(" ")));
    out.push_str(&format!(".outputs {}\n", outputs.join(" ")));
    for gate in &circuit.gates {
        let output = gate.output.name();
        let a = gate.inputs.first().map(Signal::name).unwrap_or("");
        let b = gate.inputs.get(1).map(Signal::name).unwrap_or(a);
        match gate.kind {
            GateKind::Const => match gate.const_value.unwrap_or(Ternary::False) {
                Ternary::False => out.push_str(&format!(".names {output}\n")),
                Ternary::True => out.push_str(&format!(".names {output}\n1\n")),
                Ternary::Unknown => return Err(BlifError::Unsupported(GateKind::Const)),
            },
            GateKind::Buf => out.push_str(&format!(".names {a} {output}\n1 1\n")),
            GateKind::Not => out.push_str(&format!(".names {a} {output}\n0 1\n")),
            GateKind::And => out.push_str(&format!(".names {a} {b} {output}\n11 1\n")),
            GateKind::Or => {
                out.push_str(&format!(".names {a} {b} {output}\n1- 1\n-1 1\n"));
            }
            GateKind::Nand => {
                out.push_str(&format!(".names {a} {b} {output}\n00 1\n01 1\n10 1\n"));
            }
            GateKind::Nor => out.push_str(&format!(".names {a} {b} {output}\n00 1\n")),
            GateKind::XorClassic => {
                out.push_str(&format!(".names {a} {b} {output}\n10 1\n01 1\n"));
            }
            GateKind::Xor => return Err(BlifError::Unsupported(GateKind::Xor)),
        }
    }
    out.push_str(".end\n");
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn export_validates_structure_and_identifiers() {
        // 复审 L03：含空白的信号名曾直接拼进文本，导出不可解析。
        let mut bad_name = Circuit::new("m");
        bad_name.add_input(Signal::new("a b"));
        bad_name.add_output(Signal::new("y"));
        bad_name.add_gate(Gate::new(
            GateKind::Buf,
            vec![Signal::new("a b")],
            Signal::new("y"),
        ));
        let actual = to_blif(&bad_name);
        assert!(
            matches!(&actual, Err(BlifError::Name(name)) if name == "a b"),
            "空白名必须拒绝，实际 {actual:?}"
        );

        let mut dot_name = Circuit::new("m");
        dot_name.add_input(Signal::new(".names"));
        dot_name.add_output(Signal::new("y"));
        dot_name.add_gate(Gate::new(
            GateKind::Buf,
            vec![Signal::new(".names")],
            Signal::new("y"),
        ));
        assert!(matches!(to_blif(&dot_name), Err(BlifError::Name(_))));

        // 元数错误：AND 只给一个输入。
        let mut wrong_arity = Circuit::new("m");
        wrong_arity.add_input(Signal::new("a"));
        wrong_arity.add_output(Signal::new("y"));
        wrong_arity.add_gate(Gate::new(
            GateKind::And,
            vec![Signal::new("a")],
            Signal::new("y"),
        ));
        assert!(
            matches!(to_blif(&wrong_arity), Err(BlifError::Structure(_))),
            "非法元数必须拒绝"
        );
    }

    #[test]
    fn parser_rejects_structurally_invalid_blif() {
        assert!(matches!(
            parse_blif(
                ".model m
.inputs a
.outputs y
.end
"
            ),
            Err(BlifError::Structure(_))
        ));
    }

    #[test]
    fn budgets_reject_oversized_blif() {
        // 复审 M11：cover 行与网络/门数原先无预算。
        let text = ".model m\n.inputs a b\n.outputs y\n.names a b y\n11 1\n.end\n";
        assert!(parse_blif(text).is_ok(), "默认预算内必须能解析");
        let tiny_bytes = Limits {
            bytes: 8,
            ..Limits::DEFAULT
        };
        assert!(matches!(
            parse_blif_with_limits(text, tiny_bytes),
            Err(BlifError::Limit(_))
        ));
        let no_covers = Limits {
            cover_terms: 0,
            ..Limits::DEFAULT
        };
        assert!(matches!(
            parse_blif_with_limits(text, no_covers),
            Err(BlifError::Limit(_))
        ));
        let one_net = Limits {
            nets: 1,
            ..Limits::DEFAULT
        };
        assert!(matches!(
            parse_blif_with_limits(text, one_net),
            Err(BlifError::Limit(_))
        ));
    }

    use crate::circuit::propagate;
    use trit_core::algebra::Algebra;
    use trit_core::value::Ternary;

    fn check(circuit: &Circuit) {
        let text = to_blif(circuit).unwrap();
        let again = parse_blif(&text).unwrap();
        assert_eq!(again.inputs.len(), circuit.inputs.len());
        assert_eq!(again.outputs.len(), circuit.outputs.len());
        // BLIF 是二值格式：只对 0/1 激励做等价断言。
        let total = 1u64 << circuit.input_count();
        for combo in 0..total {
            let mut assignment = std::collections::BTreeMap::new();
            for (bit, input) in circuit.inputs.iter().enumerate() {
                let value = if (combo >> bit) & 1 == 0 {
                    Ternary::False
                } else {
                    Ternary::True
                };
                assignment.insert(input.name().to_string(), value);
            }
            let left = propagate(circuit, &assignment, Algebra::Kleene).unwrap();
            let right = propagate(&again, &assignment, Algebra::Kleene).unwrap();
            for output in &circuit.outputs {
                assert_eq!(
                    left.signals.get(output.name()),
                    right.signals.get(output.name()),
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
    fn parses_plain_names_cover() {
        let text = ".model m\n.inputs a b\n.outputs y\n.names a b y\n11 1\n.end\n";
        let circuit = parse_blif(text).unwrap();
        let mut assignment = std::collections::BTreeMap::new();
        assignment.insert("a".to_string(), Ternary::True);
        assignment.insert("b".to_string(), Ternary::True);
        let values = propagate(&circuit, &assignment, Algebra::Kleene).unwrap();
        assert_eq!(values.signals.get("y"), Some(&Ternary::True));
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
    fn constants_and_dont_care_rows() {
        let text = ".model m\n.inputs a\n.outputs y z\n\
                    .names a y\n- 1\n\
                    .names z\n\
                    .end\n";
        let circuit = parse_blif(text).unwrap();
        let mut assignment = std::collections::BTreeMap::new();
        assignment.insert("a".to_string(), Ternary::Unknown);
        let values = propagate(&circuit, &assignment, Algebra::Kleene).unwrap();
        assert_eq!(values.signals.get("y"), Some(&Ternary::True));
        assert_eq!(values.signals.get("z"), Some(&Ternary::False));
    }

    #[test]
    fn rejects_latch_and_strong_xor() {
        let text = ".model m\n.inputs a\n.outputs y\n.latch a y\n.end\n";
        assert!(parse_blif(text).is_err());
        let circuit = gate(GateKind::Xor, &["a", "b"], "y");
        assert_eq!(
            to_blif(&circuit).unwrap_err(),
            BlifError::Unsupported(GateKind::Xor)
        );
    }

    #[test]
    fn rejects_bad_cover_rows() {
        let text = ".model m\n.inputs a b\n.outputs y\n.names a b y\n110 1\n.end\n";
        assert!(matches!(
            parse_blif(text).unwrap_err(),
            BlifError::Line { .. }
        ));
        let text = ".model m\n.inputs a\n.outputs y\n.names a y\n2 1\n.end\n";
        assert!(matches!(
            parse_blif(text).unwrap_err(),
            BlifError::Line { .. }
        ));
    }
}
