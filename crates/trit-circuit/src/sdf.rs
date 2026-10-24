//! SDF（Standard Delay Format）的保守子集读取。
//!
//! 只读取 `(CELL (CELLTYPE ...) (DELAY (ABSOLUTE (IOPATH ...))))` 里的 rise/fall
//! 延迟，按 celltype 取所有实例/弧的最大值，向上取整为整数周期。`TIMESCALE` 不换算，
//! `COND` 条件弧按同一 celltype 的最坏值合并。用于 STA-lite 的跨工具延迟注入口；
//! 不是完整 SDF 语义，也不保留 per-instance 差异。

use std::collections::BTreeMap;

use crate::circuit::{Circuit, GateKind};
use crate::timing::TimingModel;

type InstanceDelayMap = BTreeMap<(String, String), f64>;

/// SDF 输入字节数上限：在解析前拒绝超大文件（与 Verilog 同额）。
const MAX_SDF_BYTES: usize = 1 << 20;
/// SDF 括号嵌套深度上限：`parse_node` 是递归下降，深度不设防会耗尽栈。
const MAX_SDF_DEPTH: usize = 256;

/// SDF 读取错误。
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SdfError {
    /// 括号/引号/原子不合法。
    #[error("SDF 语法错误（字节 {at}）：{message}")]
    Syntax {
        /// 出错位置（字节偏移）。
        at: usize,
        /// 说明。
        message: String,
    },
    /// 没有可用的 IOPATH 延迟。
    #[error("SDF 不含任何可识别的 IOPATH 延迟")]
    NoDelays,
    /// 延迟值不是非负有限数。
    #[error("SDF 延迟值无效：{0}")]
    InvalidDelay(String),
    /// 输入超出预算（字节数或括号嵌套深度）。
    #[error("SDF 超出预算：{0}")]
    Limit(String),
}

/// SDF 中一个实例的最坏延迟（周期，向上取整）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SdfInstance {
    /// 实例路径（保留 SDF 的 `.` / `/` 分隔符）。
    pub instance: String,
    /// 原始 celltype 名。
    pub celltype: String,
    /// 最坏 IOPATH 延迟。
    pub delay: u32,
}

/// 解析 SDF，返回门类型短名（`AND` / `NOT` / ...）到最坏延迟（周期，向上取整）。
pub fn parse_sdf(text: &str) -> Result<BTreeMap<String, u32>, SdfError> {
    let (raw, _) = collect_sdf(text)?;
    if raw.is_empty() {
        return Err(SdfError::NoDelays);
    }
    let mut delays = BTreeMap::new();
    for (cell, value) in raw {
        let Some(kind) = map_celltype(&cell) else {
            continue;
        };
        let delay = rounded_delay(value, &cell)?;
        delays
            .entry(kind.as_str().to_string())
            .and_modify(|current: &mut u32| *current = (*current).max(delay))
            .or_insert(delay);
    }
    if delays.is_empty() {
        return Err(SdfError::NoDelays);
    }
    Ok(delays)
}

/// 解析每个 `(CELLTYPE, INSTANCE)` 的最坏延迟；没有 `INSTANCE` 的 CELL 不返回。
pub fn parse_sdf_instances(text: &str) -> Result<Vec<SdfInstance>, SdfError> {
    let (_, instances) = collect_sdf(text)?;
    let mut out = Vec::with_capacity(instances.len());
    for ((celltype, instance), value) in instances {
        if map_celltype(&celltype).is_none() {
            continue;
        }
        out.push(SdfInstance {
            instance,
            celltype: celltype.clone(),
            delay: rounded_delay(value, &celltype)?,
        });
    }
    Ok(out)
}

/// 从 SDF 构造 `TimingModel`：延迟按 celltype 取最坏值，负载延迟为 0。
///
/// SDF 未覆盖的门类型回退到 `default_delay = 1`。
pub fn timing_model_from_sdf(text: &str) -> Result<TimingModel, SdfError> {
    Ok(TimingModel {
        default_delay: 1,
        delays: parse_sdf(text)?,
        arcs: BTreeMap::new(),
        load_delay: 0,
        instance_delays: BTreeMap::new(),
    })
}

/// 同 [`timing_model_from_sdf`]，并把 `INSTANCE` 对齐到层级展开后的门。
///
/// 匹配规则：SDF 实例路径等于 `Gate::instance`，或 SDF 路径以 `.<Gate::instance>` 结尾
/// （容忍 SDF 带顶层前缀）。同一实例只保留同 celltype 的最坏值；匹配不到时保留 celltype 回退。
pub fn timing_model_from_sdf_for(text: &str, circuit: &Circuit) -> Result<TimingModel, SdfError> {
    let mut model = timing_model_from_sdf(text)?;
    let instances = parse_sdf_instances(text)?;
    for gate in &circuit.gates {
        let Some(path) = &gate.instance else {
            continue;
        };
        let mut best: Option<u32> = None;
        for sdf in &instances {
            if !instance_matches(&sdf.instance, path) {
                continue;
            }
            if map_celltype(&sdf.celltype) != Some(gate.kind) {
                continue;
            }
            best = Some(best.map_or(sdf.delay, |current| current.max(sdf.delay)));
        }
        if let Some(delay) = best {
            model
                .instance_delays
                .insert(gate.output.name().to_string(), delay);
        }
    }
    Ok(model)
}

fn instance_matches(sdf: &str, gate: &str) -> bool {
    if sdf == gate {
        return true;
    }
    sdf.strip_suffix(gate)
        .is_some_and(|prefix| prefix.ends_with('.') || prefix.ends_with('/'))
}

fn rounded_delay(value: f64, name: &str) -> Result<u32, SdfError> {
    let rounded = value.ceil();
    if !rounded.is_finite() || rounded > f64::from(u32::MAX) {
        return Err(SdfError::InvalidDelay(name.to_string()));
    }
    Ok(rounded as u32)
}

fn collect_sdf(text: &str) -> Result<(BTreeMap<String, f64>, InstanceDelayMap), SdfError> {
    if text.len() > MAX_SDF_BYTES {
        return Err(SdfError::Limit(format!(
            "SDF 文本 {} 字节，超过上限 {MAX_SDF_BYTES}",
            text.len()
        )));
    }
    let nodes = Parser::new(text).parse_all()?;
    let mut raw = BTreeMap::new();
    let mut instances = BTreeMap::new();
    let mut found_file = false;
    for node in &nodes {
        if is_head(node, "DELAYFILE") {
            found_file = true;
            collect(node, None, None, &mut raw, &mut instances)?;
        }
    }
    if !found_file {
        return Err(SdfError::Syntax {
            at: 0,
            message: "缺少 (DELAYFILE ...)".to_string(),
        });
    }
    Ok((raw, instances))
}

fn map_celltype(name: &str) -> Option<GateKind> {
    let upper = name.trim().to_ascii_uppercase();
    if upper.starts_with("XNOR") {
        return None;
    }
    // SDF 的 celltype 来自标准单元库，`XOR2` 是**经典**二输入异或
    // （对应 Verilog `xor` ⇒ `GateKind::XorClassic`），不是 trit 的三值强加 `Xor`。
    // 映射错了会让按实例覆盖静默落空、回退到默认延迟（复审 H11）。
    const PREFIXES: [(&str, GateKind); 8] = [
        ("NAND", GateKind::Nand),
        ("NOR", GateKind::Nor),
        ("AND", GateKind::And),
        ("OR", GateKind::Or),
        ("XOR", GateKind::XorClassic),
        ("INV", GateKind::Not),
        ("NOT", GateKind::Not),
        ("BUF", GateKind::Buf),
    ];
    for (prefix, kind) in PREFIXES {
        if let Some(rest) = upper.strip_prefix(prefix)
            && (rest.is_empty() || rest.starts_with(|ch: char| ch.is_ascii_digit()))
        {
            return Some(kind);
        }
    }
    None
}

fn collect(
    node: &Node,
    cell: Option<&str>,
    instance: Option<&str>,
    out: &mut BTreeMap<String, f64>,
    instances: &mut InstanceDelayMap,
) -> Result<(), SdfError> {
    let Node::List(items) = node else {
        return Ok(());
    };
    let head = items.first().and_then(atom);
    match head {
        Some("CELL") => {
            let celltype = child_atom(items, "CELLTYPE").or(cell);
            let current_instance = child_atom(items, "INSTANCE").or(instance);
            for item in items {
                collect(item, celltype, current_instance, out, instances)?;
            }
        }
        Some("IOPATH") => {
            let Some(cell) = cell else {
                return Ok(());
            };
            let mut worst = 0.0f64;
            let mut found = false;
            for item in items.iter().skip(3) {
                let Some(values) = list_items(item) else {
                    continue;
                };
                for value in values {
                    let Some(text) = atom(value) else {
                        continue;
                    };
                    let delay = parse_delay(text)?;
                    worst = worst.max(delay);
                    found = true;
                }
            }
            if found {
                out.entry(cell.to_string())
                    .and_modify(|current| *current = current.max(worst))
                    .or_insert(worst);
                if let Some(instance) = instance {
                    instances
                        .entry((cell.to_string(), instance.to_string()))
                        .and_modify(|current| *current = current.max(worst))
                        .or_insert(worst);
                }
            }
        }
        _ => {
            for item in items {
                collect(item, cell, instance, out, instances)?;
            }
        }
    }
    Ok(())
}

fn child_atom<'a>(items: &'a [Node], name: &str) -> Option<&'a str> {
    items.iter().find_map(|item| match list_head(item) {
        Some(head) if head == name => list_items(item)
            .and_then(|values| values.get(1))
            .and_then(atom),
        _ => None,
    })
}

fn parse_delay(text: &str) -> Result<f64, SdfError> {
    let mut worst = 0.0f64;
    let mut found = false;
    for part in text.split(':') {
        let value: f64 = part
            .trim()
            .parse()
            .map_err(|_| SdfError::InvalidDelay(text.to_string()))?;
        if !value.is_finite() || value < 0.0 {
            return Err(SdfError::InvalidDelay(text.to_string()));
        }
        worst = worst.max(value);
        found = true;
    }
    if found {
        Ok(worst)
    } else {
        Err(SdfError::InvalidDelay(text.to_string()))
    }
}

fn is_head(node: &Node, name: &str) -> bool {
    list_head(node) == Some(name)
}

fn list_head(node: &Node) -> Option<&str> {
    list_items(node)?.first().and_then(atom)
}

fn list_items(node: &Node) -> Option<&[Node]> {
    match node {
        Node::List(items) => Some(items),
        _ => None,
    }
}

fn atom(node: &Node) -> Option<&str> {
    match node {
        Node::Atom(text) => Some(text),
        Node::List(_) => None,
    }
}

#[derive(Debug, PartialEq)]
enum Node {
    Atom(String),
    List(Vec<Node>),
}

struct Parser<'a> {
    bytes: &'a [u8],
    at: usize,
    depth: usize,
}

impl<'a> Parser<'a> {
    fn new(text: &'a str) -> Self {
        Self {
            bytes: text.as_bytes(),
            at: 0,
            depth: 0,
        }
    }

    fn parse_all(&mut self) -> Result<Vec<Node>, SdfError> {
        let mut nodes = Vec::new();
        loop {
            self.skip_trivia();
            if self.at >= self.bytes.len() {
                return Ok(nodes);
            }
            nodes.push(self.parse_node()?);
        }
    }

    fn parse_node(&mut self) -> Result<Node, SdfError> {
        self.skip_trivia();
        let Some(&byte) = self.bytes.get(self.at) else {
            return Err(self.error(self.at, "意外的文件结尾"));
        };
        match byte {
            b'(' => {
                self.at += 1;
                // 深度护栏只在成功路径上递减；出错时整个 Parser 随之丢弃，不必还原。
                self.depth += 1;
                if self.depth > MAX_SDF_DEPTH {
                    return Err(SdfError::Limit(format!(
                        "SDF 括号嵌套超过 {MAX_SDF_DEPTH} 层"
                    )));
                }
                let mut items = Vec::new();
                loop {
                    self.skip_trivia();
                    match self.bytes.get(self.at) {
                        Some(b')') => {
                            self.at += 1;
                            self.depth -= 1;
                            return Ok(Node::List(items));
                        }
                        None => return Err(self.error(self.at, "括号未闭合")),
                        _ => items.push(self.parse_node()?),
                    }
                }
            }
            b')' => Err(self.error(self.at, "多余的 ')'")),
            b'"' => self.parse_quoted(),
            _ => self.parse_atom(),
        }
    }

    fn parse_quoted(&mut self) -> Result<Node, SdfError> {
        let start = self.at;
        self.at += 1;
        let mut value = String::new();
        while let Some(&byte) = self.bytes.get(self.at) {
            match byte {
                b'"' => {
                    self.at += 1;
                    return Ok(Node::Atom(value));
                }
                b'\\' => {
                    self.at += 1;
                    let Some(&escaped) = self.bytes.get(self.at) else {
                        return Err(self.error(self.at, "字符串转义未完成"));
                    };
                    value.push(escaped as char);
                    self.at += 1;
                }
                _ => {
                    let rest = &self.bytes[self.at..];
                    let ch = std::str::from_utf8(rest)
                        .ok()
                        .and_then(|text| text.chars().next())
                        .unwrap_or(byte as char);
                    value.push(ch);
                    self.at += ch.len_utf8().max(1);
                }
            }
        }
        Err(self.error(start, "字符串未闭合"))
    }

    fn parse_atom(&mut self) -> Result<Node, SdfError> {
        let start = self.at;
        while let Some(&byte) = self.bytes.get(self.at) {
            if byte.is_ascii_whitespace() || matches!(byte, b'(' | b')' | b'"' | b';') {
                break;
            }
            self.at += 1;
        }
        if self.at == start {
            return Err(self.error(start, "缺少原子值"));
        }
        let text = std::str::from_utf8(&self.bytes[start..self.at])
            .map_err(|_| self.error(start, "原子值不是合法 UTF-8"))?;
        Ok(Node::Atom(text.to_string()))
    }

    fn skip_trivia(&mut self) {
        loop {
            while self.bytes.get(self.at).is_some_and(u8::is_ascii_whitespace) {
                self.at += 1;
            }
            if self.bytes.get(self.at) == Some(&b'/') {
                match self.bytes.get(self.at + 1) {
                    Some(b'/') => {
                        self.at += 2;
                        while self.bytes.get(self.at).is_some_and(|byte| *byte != b'\n') {
                            self.at += 1;
                        }
                        continue;
                    }
                    Some(b'*') => {
                        self.at += 2;
                        while self.at + 1 < self.bytes.len() {
                            if self.bytes[self.at] == b'*' && self.bytes[self.at + 1] == b'/' {
                                self.at += 2;
                                break;
                            }
                            self.at += 1;
                        }
                        continue;
                    }
                    _ => {}
                }
            }
            return;
        }
    }

    fn error(&self, at: usize, message: &str) -> SdfError {
        SdfError::Syntax {
            at,
            message: message.to_string(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"
        (DELAYFILE
          (SDFVERSION "3.0")
          (DESIGN "and2")
          (TIMESCALE 1ns)
          (CELL
            (CELLTYPE "AND2")
            (INSTANCE u1)
            (DELAY (ABSOLUTE (IOPATH a y (1.2) (1.8)))))
          (CELL
            (CELLTYPE "INV")
            (INSTANCE u2)
            (DELAY (ABSOLUTE (IOPATH a y (0.4) (0.6)))))
        )
    "#;

    #[test]
    fn parses_and_rounds_up_the_worst_arc() {
        let delays = parse_sdf(SAMPLE).unwrap();
        assert_eq!(delays["AND"], 2);
        assert_eq!(delays["NOT"], 1);
    }

    #[test]
    fn takes_the_max_across_instances_and_conditional_arcs() {
        let text = r#"
        (DELAYFILE
          (CELL (CELLTYPE "OR2") (INSTANCE u1)
            (DELAY (ABSOLUTE (IOPATH a y (1.0) (2.4)))))
          (CELL (CELLTYPE "OR2") (INSTANCE u2)
            (DELAY (ABSOLUTE
              (COND b (IOPATH a y (3.0) (3.2))))))
        )
        "#;
        let delays = parse_sdf(text).unwrap();
        assert_eq!(delays["OR"], 4);
    }

    #[test]
    fn xor_celltypes_map_to_the_classic_gate() {
        // 复审 H11：`XOR2` 曾被映射成三值强加 `Xor`，经典 XOR 的时序覆盖静默落空。
        assert_eq!(map_celltype("XOR2"), Some(GateKind::XorClassic));
        assert_eq!(map_celltype("xor"), Some(GateKind::XorClassic));
        assert_eq!(map_celltype("XOR"), Some(GateKind::XorClassic));
        assert_eq!(map_celltype("XNOR2"), None);
        let text = r#"
        (DELAYFILE
          (CELL (CELLTYPE "XOR2") (INSTANCE u1)
            (DELAY (ABSOLUTE (IOPATH a y (3.0) (3.0)))))
        )
        "#;
        let delays = parse_sdf(text).unwrap();
        assert_eq!(delays[GateKind::XorClassic.as_str()], 3);
        assert!(!delays.contains_key(GateKind::Xor.as_str()));
    }

    #[test]
    fn accepts_min_typ_max_and_ignores_unknown_cells() {
        let text = r#"
        (DELAYFILE
          (CELL (CELLTYPE "NAND3") (INSTANCE u1)
            (DELAY (ABSOLUTE (IOPATH a y (1.0:2.1:3.0)))))
          (CELL (CELLTYPE "DFF") (INSTANCE u2)
            (DELAY (ABSOLUTE (IOPATH clk q (9.0) (9.0)))))
        )
        "#;
        let delays = parse_sdf(text).unwrap();
        assert_eq!(delays["NAND"], 3);
        assert!(!delays.contains_key("DFF"));
    }

    #[test]
    fn rejects_bad_syntax_and_missing_delays() {
        assert!(matches!(
            parse_sdf("(DELAYFILE (CELL"),
            Err(SdfError::Syntax { .. })
        ));
        assert_eq!(
            parse_sdf("(DELAYFILE (DESIGN \"x\"))").unwrap_err(),
            SdfError::NoDelays
        );
        assert!(matches!(
            parse_sdf("(DELAYFILE (CELL (CELLTYPE \"AND2\") (IOPATH a y (-1.0))))"),
            Err(SdfError::InvalidDelay(_))
        ));
    }

    #[test]
    fn budget_rejects_deep_nesting_and_oversized_input() {
        // 复审 H02：3 万层括号曾直接栈溢出（abort 134），超大文本无预算。
        let deep = format!("{}x{}", "(".repeat(30_000), ")".repeat(30_000));
        assert!(matches!(parse_sdf(&deep), Err(SdfError::Limit(_))));
        let oversized = " ".repeat(MAX_SDF_BYTES + 1);
        assert!(matches!(parse_sdf(&oversized), Err(SdfError::Limit(_))));
        // 正常深度要放行：250 层嵌套不超限。
        let nested = format!(
            "(DELAYFILE {}(DESIGN \"x\"){})",
            "(".repeat(250),
            ")".repeat(250)
        );
        assert_eq!(parse_sdf(&nested).unwrap_err(), SdfError::NoDelays);
    }

    #[test]
    fn timing_model_uses_one_cycle_for_unlisted_cells() {
        let model = timing_model_from_sdf(SAMPLE).unwrap();
        assert_eq!(model.default_delay, 1);
        assert_eq!(model.delays["AND"], 2);
        assert_eq!(model.load_delay, 0);
    }
}

#[cfg(test)]
mod instance_tests {
    use super::*;
    use crate::timing::analyze_timing;

    const TWO_LEAVES: &str = "module leaf(input a, input b, output y);\n\
        assign y = a & b;\n\
        endmodule\n\
        module top(input a, input b, input c, input d, output y1, output y2);\n\
        leaf u1 (.a(a), .b(b), .y(y1));\n\
        leaf u2 (.a(c), .b(d), .y(y2));\n\
        endmodule\n";

    const TWO_LEAVES_SDF: &str = r#"
        (DELAYFILE
          (SDFVERSION "3.0")
          (DESIGN "top")
          (CELL (CELLTYPE "AND2") (INSTANCE u1)
            (DELAY (ABSOLUTE (IOPATH a y (2.0) (2.0)))))
          (CELL (CELLTYPE "AND2") (INSTANCE u2)
            (DELAY (ABSOLUTE (IOPATH a y (5.0) (5.0)))))
        )
    "#;

    #[test]
    fn parses_per_instance_delays() {
        let instances = parse_sdf_instances(TWO_LEAVES_SDF).unwrap();
        assert_eq!(instances.len(), 2);
        assert!(
            instances
                .iter()
                .any(|item| item.instance == "u1" && item.delay == 2)
        );
        assert!(
            instances
                .iter()
                .any(|item| item.instance == "u2" && item.delay == 5)
        );
    }

    #[test]
    fn instance_delays_override_celltype_in_sta() {
        let circuit = crate::parse_verilog(TWO_LEAVES).unwrap();
        assert_eq!(
            circuit
                .gates
                .iter()
                .find(|gate| gate.output.name() == "y1")
                .unwrap()
                .instance
                .as_deref(),
            Some("u1")
        );
        let model = timing_model_from_sdf_for(TWO_LEAVES_SDF, &circuit).unwrap();
        assert_eq!(model.instance_delays["y1"], 2);
        assert_eq!(model.instance_delays["y2"], 5);
        let report = analyze_timing(&circuit, model, None).unwrap();
        let arrival = |name: &str| {
            report
                .outputs
                .iter()
                .find(|output| output.name == name)
                .unwrap()
                .arrival
        };
        assert_eq!(arrival("y1"), 2);
        assert_eq!(arrival("y2"), 5);
        assert_eq!(report.critical_path, 5);
    }

    #[test]
    fn instance_path_accepts_a_design_prefix() {
        assert!(instance_matches("top.u1", "u1"));
        assert!(instance_matches("top/u1", "u1"));
        assert!(!instance_matches("top.u10", "u1"));
    }

    const CLASSIC_XOR: &str = "module leaf(input a, input b, output y);\n\
        assign y = a ^ b;\n\
        endmodule\n\
        module top(input a, input b, output y);\n\
        leaf u9 (.a(a), .b(b), .y(y));\n\
        endmodule\n";

    #[test]
    fn sdf_instance_delay_applies_to_classic_xor() {
        // 复审 H11 复现：Verilog `xor` 实例配 `XOR2` 延迟 100，修复前关键路径仍是默认 1。
        let circuit = crate::parse_verilog(CLASSIC_XOR).unwrap();
        assert!(
            circuit
                .gates
                .iter()
                .any(|gate| gate.kind == GateKind::XorClassic
                    && gate.instance.as_deref() == Some("u9")),
            "经典 XOR 实例没有展开成 XorClassic/u9"
        );
        let sdf = r#"
        (DELAYFILE
          (CELL (CELLTYPE "XOR2") (INSTANCE u9)
            (DELAY (ABSOLUTE (IOPATH a y (100.0) (100.0)))))
        )
        "#;
        let model = timing_model_from_sdf_for(sdf, &circuit).unwrap();
        assert_eq!(model.instance_delays["y"], 100);
        let report = analyze_timing(&circuit, model, None).unwrap();
        assert_eq!(report.critical_path, 100);
    }
}
