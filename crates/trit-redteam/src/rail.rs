//! 双导轨 `design.v` 的非法态分析：`(pos, neg) = (1,1)` 不是合法值，但门级网表会传播它。
//!
//! 攻方问三个问题：
//! 1. 只喂**合法**输入（每根都是 01/10/00）时，输出会不会出非法态？（不该）
//! 2. 喂**非法**输入时，输出会不会被"只看 pos"的消费端读成 True？——这就是 fail-open。
//! 3. 全 1 输入（每根都是 (1,1)）会锁死成什么？
//!
//! 只解析本仓 `trit-build --emit-verilog` 生成的门级子集：`~ ! & |`、括号、标识符、`1'b0/1'b1`。

use std::collections::BTreeMap;

use anyhow::{Result, bail};

pub const MAX_SIGNALS: usize = 6;

struct Parser<'a> {
    chars: Vec<char>,
    index: usize,
    env: &'a BTreeMap<String, u8>,
}

impl Parser<'_> {
    fn peek(&self) -> Option<char> {
        self.chars.get(self.index).copied()
    }

    fn rest(&self) -> String {
        self.chars[self.index..].iter().collect()
    }

    fn skip_ws(&mut self) {
        while matches!(self.peek(), Some(' ') | Some('\t')) {
            self.index += 1;
        }
    }

    fn parse_or(&mut self) -> Result<u8> {
        let mut value = self.parse_and()?;
        loop {
            self.skip_ws();
            if self.peek() == Some('|') {
                self.index += 1;
                value |= self.parse_and()?;
            } else {
                return Ok(value);
            }
        }
    }

    fn parse_and(&mut self) -> Result<u8> {
        let mut value = self.parse_unary()?;
        loop {
            self.skip_ws();
            if self.peek() == Some('&') {
                self.index += 1;
                value &= self.parse_unary()?;
            } else {
                return Ok(value);
            }
        }
    }

    fn parse_unary(&mut self) -> Result<u8> {
        self.skip_ws();
        match self.peek() {
            Some('~') | Some('!') => {
                self.index += 1;
                let value = self.parse_unary()?;
                Ok(1 - value)
            }
            Some('(') => {
                self.index += 1;
                let value = self.parse_or()?;
                self.skip_ws();
                if self.peek() != Some(')') {
                    bail!("位置 {}：缺少右括号", self.index);
                }
                self.index += 1;
                Ok(value)
            }
            _ => self.parse_atom(),
        }
    }

    fn parse_atom(&mut self) -> Result<u8> {
        self.skip_ws();
        let rest = self.rest();
        if rest.starts_with("1'b0") {
            self.index += 4;
            return Ok(0);
        }
        if rest.starts_with("1'b1") {
            self.index += 4;
            return Ok(1);
        }
        let start = self.index;
        while let Some(c) = self.peek() {
            if c.is_alphanumeric() || c == '_' {
                self.index += 1;
            } else {
                break;
            }
        }
        if self.index == start {
            bail!("位置 {start}：表达式在此处无法解析");
        }
        let name: String = self.chars[start..self.index].iter().collect();
        match self.env.get(&name) {
            Some(value) => Ok(*value),
            None => bail!("位置 {start}：未定义信号 {name}"),
        }
    }
}

/// 求值一段门级表达式。
///
/// 这里**不用 panic**：输入是别人生成的网表，畸形写法属于预期输入，必须以
/// `Err` 回到调用方，否则一份坏文件就能让审计器崩掉（见台账 `L56`）。
pub fn eval_expr(text: &str, env: &BTreeMap<String, u8>) -> Result<u8> {
    let mut parser = Parser {
        chars: text.chars().collect(),
        index: 0,
        env,
    };
    let value = parser.parse_or()?;
    // 尾部残留必须报错：`a b` 若静默只算 `a`，分析结论会指向一个不存在的表达式。
    parser.skip_ws();
    if parser.index < parser.chars.len() {
        bail!("位置 {}：表达式后有多余字符", parser.index);
    }
    Ok(value)
}

/// 解析出来的模块：输入位线名与按文件顺序的赋值列表。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RailDesign {
    /// `input wire` 声明的位线名（顺序即文件顺序）。
    pub inputs: Vec<String>,
    /// `assign` 列表（左值、右值表达式）。
    pub assigns: Vec<(String, String)>,
}

/// 解析模块：输入位线名、按文件顺序的赋值列表。
///
/// 输入是别人生成的网表，畸形写法属于预期输入：一律返回 `Err`，不 panic。
pub fn parse_design(text: &str) -> Result<RailDesign> {
    let mut inputs = Vec::new();
    let mut assigns = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if let Some(list) = line.strip_prefix("input wire") {
            for part in list.trim_end_matches(',').split(',') {
                let name = part.trim();
                if name.is_empty() {
                    bail!("input wire 里有空名字：{line}");
                }
                inputs.push(name.to_string());
            }
        } else if let Some(rest) = line.strip_prefix("assign ") {
            let Some((name, rhs)) = rest.split_once('=') else {
                bail!("assign 缺少 '='：{line}");
            };
            let name = name.trim();
            if name.is_empty() {
                bail!("assign 缺少左侧信号名：{line}");
            }
            assigns.push((
                name.to_string(),
                rhs.trim_end_matches(';').trim().to_string(),
            ));
        }
    }
    Ok(RailDesign { inputs, assigns })
}

fn eval_design(assigns: &[(String, String)], env: &mut BTreeMap<String, u8>) -> Result<()> {
    for (name, rhs) in assigns {
        let value = eval_expr(rhs, env)?;
        env.insert(name.clone(), value);
    }
    Ok(())
}

fn class(pos: u8, neg: u8) -> &'static str {
    match (pos, neg) {
        (1, 0) => "true",
        (0, 1) => "false",
        (0, 0) => "unknown",
        (1, 1) => "invalid",
        _ => unreachable!("导轨只可能是 0/1"),
    }
}

#[derive(Debug, Clone)]
pub struct RailReport {
    pub signals: usize,
    pub combinations: usize,
    pub legal_inputs: usize,
    pub illegal_inputs: usize,
    /// 合法输入下输出出非法态的行数（它本该是 0）。
    pub legal_output_invalid: usize,
    /// 非法输入下输出被"只看 pos"的消费端读成 True 的行数。
    pub illegal_output_true_by_pos: usize,
    /// 非法输入下输出 **恰好** 是有效 True（`(1,0)`）的行数——最硬的 fail-open。
    pub illegal_output_true: usize,
    /// 非法输入下输出是非法态 `(1,1)` 的行数。
    pub illegal_output_invalid: usize,
    /// 全 (1,1) 输入下的输出类别。
    pub all_illegal_output: String,
    pub examples: Vec<serde_json::Value>,
}

pub fn analyze(text: &str) -> Result<RailReport> {
    let RailDesign { inputs, assigns } = parse_design(text)?;
    if inputs.is_empty() || assigns.is_empty() {
        bail!(
            "不像 trit-build 导出的门级网表（inputs={} assigns={}）",
            inputs.len(),
            assigns.len()
        );
    }
    let signals = inputs.len() / 2;
    if signals > MAX_SIGNALS {
        bail!("信号数 {signals} 超过分析上限 {MAX_SIGNALS}（4^k 会爆）");
    }
    if !inputs.chunks(2).all(|pair| pair.len() == 2) {
        bail!("输入位线不是成对的");
    }

    let mut report = RailReport {
        signals,
        combinations: 4usize.pow(signals as u32),
        legal_inputs: 0,
        illegal_inputs: 0,
        legal_output_invalid: 0,
        illegal_output_true_by_pos: 0,
        illegal_output_true: 0,
        illegal_output_invalid: 0,
        all_illegal_output: String::new(),
        examples: Vec::new(),
    };

    for code in 0..report.combinations {
        let mut env = BTreeMap::new();
        let mut rest = code;
        let mut illegal = false;
        let mut pattern = Vec::new();
        for pair in inputs.chunks(2) {
            let state = rest % 4;
            rest /= 4;
            let (pos, neg) = match state {
                0 => (0u8, 0u8),
                1 => (0u8, 1u8),
                2 => (1u8, 0u8),
                _ => (1u8, 1u8),
            };
            illegal |= state == 3;
            env.insert(pair[0].clone(), pos);
            env.insert(pair[1].clone(), neg);
            pattern.push(format!("{}={pos}{neg}", pair[0].trim_end_matches("_p")));
        }
        eval_design(&assigns, &mut env)?;
        // 输出必须是网表声明的：缺了就报错，而不是索引 panic（台账 `L54`/`L56`）。
        let (Some(&pos), Some(&neg)) = (env.get("y_p"), env.get("y_n")) else {
            bail!(
                "网表缺少 y_p/y_n 输出（解析得到的信号：{:?}）",
                env.keys().collect::<Vec<_>>()
            );
        };
        let output = class(pos, neg);
        if illegal {
            report.illegal_inputs += 1;
            if pos == 1 {
                report.illegal_output_true_by_pos += 1;
            }
            if output == "true" {
                report.illegal_output_true += 1;
            }
            if output == "invalid" {
                report.illegal_output_invalid += 1;
            }
            if pos == 1 && report.examples.len() < 5 {
                report.examples.push(serde_json::json!({
                    "inputs": pattern.join(","),
                    "y_p": pos,
                    "y_n": neg,
                    "output": output,
                }));
            }
            if code == report.combinations - 1 {
                report.all_illegal_output = output.to_string();
            }
        } else {
            report.legal_inputs += 1;
            if output == "invalid" {
                report.legal_output_invalid += 1;
            }
        }
    }
    Ok(report)
}

/// 分析结果里是否存在需要严肃对待的发现。
pub fn has_findings(report: &RailReport) -> bool {
    report.legal_output_invalid > 0
        || report.illegal_output_true > 0
        || report.illegal_output_invalid > 0
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 由 `trit-build --emit-verilog` 对 `(a & b) | !c` 生成的真实导出。
    const FIXTURE: &str = include_str!("../fixtures/three_rail.v");

    #[test]
    fn parses_the_exported_subset() {
        let design = parse_design(FIXTURE).expect("合法网表");
        assert_eq!(design.inputs.len(), 6);
        assert_eq!(design.assigns.len(), 6, "4 个门 wire + 2 个输出");
        let mut env = BTreeMap::new();
        for (name, value) in [
            ("a_p", 1u8),
            ("a_n", 0),
            ("b_p", 1),
            ("b_n", 0),
            ("c_p", 0),
            ("c_n", 0),
        ] {
            env.insert(name.to_string(), value);
        }
        eval_design(&design.assigns, &mut env).expect("求值");
        // a=T,b=T,c=U ⇒ (a∧b)=T ⇒ T ∨ ¬U = T ∨ U = T
        assert_eq!((env["y_p"], env["y_n"]), (1, 0));
    }

    #[test]
    fn legal_inputs_never_produce_an_invalid_output() {
        let report = analyze(FIXTURE).unwrap();
        assert_eq!(report.signals, 3);
        assert_eq!(report.combinations, 64);
        assert_eq!(report.legal_inputs, 27, "3^3 个合法组合");
        assert_eq!(report.illegal_inputs, 37);
        assert_eq!(report.legal_output_invalid, 0, "合法输入不该出 (1,1)");
    }

    #[test]
    fn illegal_inputs_are_read_as_true_by_a_pos_only_consumer() {
        let report = analyze(FIXTURE).unwrap();
        assert!(report.illegal_output_true_by_pos > 0, "报告：{report:?}");
        assert!(
            report.illegal_output_invalid > 0,
            "非法态应当粘滞：{report:?}"
        );
        assert!(!report.examples.is_empty());
        assert_eq!(
            report.all_illegal_output, "invalid",
            "全 (1,1) 输入应锁死在非法态"
        );
        assert!(has_findings(&report));
    }

    #[test]
    fn catches_a_fail_open_or_gate() {
        // 或门：a=(1,1)、b=T ⇒ pos=1、neg=0 ⇒ 输出是**有效 True**，非法输入被洗白。
        let design = "
module m(
  input wire a_p, a_n,
  input wire b_p, b_n,
  output wire y_p, y_n
);
  assign y_p = (a_p | b_p);
  assign y_n = (a_n & b_n);
  assign y_p = y_p;
  assign y_n = y_n;
endmodule
";
        let report = analyze(design).unwrap();
        assert!(
            report.illegal_output_true > 0,
            "必须抓到 fail-open：{report:?}"
        );
        assert_eq!(report.legal_output_invalid, 0);
    }

    fn env_of(pairs: &[(&str, u8)]) -> BTreeMap<String, u8> {
        pairs
            .iter()
            .map(|(name, value)| ((*name).to_string(), *value))
            .collect()
    }

    #[test]
    fn and_binds_tighter_than_or() {
        // `a | b & c` 必须是 a | (b & c)。若优先级反了，安全分析会把
        // 「有条件的与」读成「无条件或」——两个方向的结论都会错。
        // 取 a=1,b=1,c=0：正确为 1|0=1；错读成 (1|1)&0=0。
        let env = env_of(&[("a", 1), ("b", 1), ("c", 0)]);
        assert_eq!(
            eval_expr("a | b & c", &env).expect("求值"),
            1,
            "ORCH_REDTEAM_PRECEDENCE"
        );
        // 另一个方向：a=0,b=0,c=1 ⇒ 0|(0&1)=0；反读 (0|0)&1=0 同值，故只上面一条够。
    }

    #[test]
    fn parentheses_override_precedence() {
        let env = env_of(&[("a", 1), ("b", 1), ("c", 0)]);
        assert_eq!(eval_expr("(a | b) & c", &env).expect("求值"), 0);
        assert_eq!(eval_expr("a & (b | c)", &env).expect("求值"), 1);
    }

    #[test]
    fn both_negation_forms_are_one_bit_not() {
        let env = env_of(&[("a", 1), ("b", 0)]);
        assert_eq!(eval_expr("~a", &env).expect("求值"), 0);
        assert_eq!(eval_expr("!a", &env).expect("求值"), 0);
        assert_eq!(eval_expr("~~a", &env).expect("求值"), 1);
        assert_eq!(eval_expr("!b", &env).expect("求值"), 1);
        assert_eq!(eval_expr("~!a", &env).expect("求值"), 1);
    }

    #[test]
    fn verilog_bit_literals_are_understood() {
        let env = BTreeMap::new();
        assert_eq!(eval_expr("1'b0", &env).expect("求值"), 0);
        assert_eq!(eval_expr("1'b1", &env).expect("求值"), 1);
        assert_eq!(eval_expr("~1'b1", &env).expect("求值"), 0);
        assert_eq!(eval_expr("1'b1 & 1'b0", &env).expect("求值"), 0);
        assert_eq!(eval_expr("1'b0 | 1'b1", &env).expect("求值"), 1);
    }

    #[test]
    fn analyze_rejects_text_that_is_not_a_netlist() {
        // 选错文件是很常见的手滑；这里必须报错，而不是穷举一个空设计得出
        // 「零发现」——那会让人以为设计是干净的。
        let err = analyze("hello world").unwrap_err().to_string();
        assert!(err.contains("不像 trit-build 导出"), "实际：{err}");
        assert!(analyze("").is_err());
    }

    #[test]
    fn analyze_refuses_more_signals_than_the_exhaustion_limit() {
        // 7 根三值信号 = 4^7 = 16384 次门级求值；上限的意义是"别让一次误用变成卡死"。
        let mut design = String::from("module m(\n");
        for i in 0..7 {
            design.push_str(&format!("  input wire s{i}_p, s{i}_n,\n"));
        }
        design.push_str(
            "  output wire y_p, y_n\n);\n  assign y_p = s0_p;\n  assign y_n = s0_n;\nendmodule\n",
        );
        let err = analyze(&design).unwrap_err().to_string();
        assert!(err.contains("超过分析上限"), "实际：{err}");
    }

    #[test]
    fn analyze_rejects_unpaired_bit_lines() {
        // 双导轨的每根三值信号必须是 (pos, neg) 一对；落单说明导出被截断，
        // 此时继续分析会把 pos/neg 错位配到相邻信号上。
        let design = "
module m(
  input wire a_p, a_n,
  input wire b_p,
  output wire y_p, y_n
);
  assign y_p = a_p;
  assign y_n = a_n;
endmodule
";
        let err = analyze(design).unwrap_err().to_string();
        assert!(err.contains("不是成对的"), "实际：{err}");
    }

    #[test]
    fn a_netlist_without_the_y_outputs_is_an_error_not_a_panic() {
        // 曾经是 `env["y_p"]` 直接 panic（台账 L54）。审计器跑在别人的产物上，
        // 一份"格式对但没输出"的文件不该把进程带走。
        let design = "
module m(
  input wire a_p, a_n,
  output wire z_p, z_n
);
  assign z_p = a_p;
  assign z_n = a_n;
endmodule
";
        let err = analyze(design).unwrap_err().to_string();
        assert!(err.contains("缺少 y_p/y_n"), "实际：{err}");
    }

    #[test]
    fn malformed_netlists_are_errors_not_panics() {
        // 这四条以前分别是 assert_eq! / assert! / panic! / expect：任何一条都能被
        // 一份畸形 .v 触发。现在全部必须是 Err（台账 L56）。
        let base = "
module m(
  input wire a_p, a_n,
  output wire y_p, y_n
);
  assign y_p = %EXPR%;
  assign y_n = a_n;
endmodule
";
        for (expr, want) in [
            ("(a_p & a_n", "缺少右括号"),
            ("a_p & ghost", "未定义信号 ghost"),
            ("", "无法解析"),
            ("a_p a_n", "多余字符"),
        ] {
            let design = base.replace("%EXPR%", expr);
            let err = analyze(&design).unwrap_err().to_string();
            assert!(
                err.contains(want),
                "表达式 {:?} 应报「{want}」，实际：{err}",
                expr
            );
        }

        // 落单的 assign（缺 '='）也必须是 Err。
        let design = "
module m(
  input wire a_p, a_n,
  output wire y_p, y_n
);
  assign y_p a_p;
  assign y_n = a_n;
endmodule
";
        let err = analyze(design).unwrap_err().to_string();
        assert!(err.contains("缺少 '='"), "实际：{err}");
    }

    #[test]
    fn empty_input_line_names_are_rejected() {
        // `input wire , a_n`：空名字会让后面的 chunks(2) 配对错位。
        let design = "
module m(
  input wire , a_n,
  output wire y_p, y_n
);
  assign y_p = a_n;
  assign y_n = a_n;
endmodule
";
        let err = analyze(design).unwrap_err().to_string();
        assert!(err.contains("空名字"), "实际：{err}");
    }
}
