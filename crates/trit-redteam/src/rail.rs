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

    fn parse_or(&mut self) -> u8 {
        let mut value = self.parse_and();
        loop {
            self.skip_ws();
            if self.peek() == Some('|') {
                self.index += 1;
                value |= self.parse_and();
            } else {
                return value;
            }
        }
    }

    fn parse_and(&mut self) -> u8 {
        let mut value = self.parse_unary();
        loop {
            self.skip_ws();
            if self.peek() == Some('&') {
                self.index += 1;
                value &= self.parse_unary();
            } else {
                return value;
            }
        }
    }

    fn parse_unary(&mut self) -> u8 {
        self.skip_ws();
        match self.peek() {
            Some('~') | Some('!') => {
                self.index += 1;
                1 - self.parse_unary()
            }
            Some('(') => {
                self.index += 1;
                let value = self.parse_or();
                self.skip_ws();
                assert_eq!(self.peek(), Some(')'), "缺少右括号 @ {}", self.index);
                self.index += 1;
                value
            }
            _ => self.parse_atom(),
        }
    }

    fn parse_atom(&mut self) -> u8 {
        self.skip_ws();
        let rest = self.rest();
        if rest.starts_with("1'b0") {
            self.index += 4;
            return 0;
        }
        if rest.starts_with("1'b1") {
            self.index += 4;
            return 1;
        }
        let start = self.index;
        while let Some(c) = self.peek() {
            if c.is_alphanumeric() || c == '_' {
                self.index += 1;
            } else {
                break;
            }
        }
        assert!(self.index > start, "无法解析 @ {start}");
        let name: String = self.chars[start..self.index].iter().collect();
        *self
            .env
            .get(&name)
            .unwrap_or_else(|| panic!("未定义信号 {name}"))
    }
}

pub fn eval_expr(text: &str, env: &BTreeMap<String, u8>) -> u8 {
    let mut parser = Parser {
        chars: text.chars().collect(),
        index: 0,
        env,
    };
    parser.parse_or()
}

/// 解析模块：输入位线名、按文件顺序的赋值列表。
pub fn parse_design(text: &str) -> (Vec<String>, Vec<(String, String)>) {
    let mut inputs = Vec::new();
    let mut assigns = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if line.starts_with("input wire") {
            for part in line
                .trim_end_matches(',')
                .replace("input wire", "")
                .split(',')
            {
                inputs.push(part.trim().to_string());
            }
        } else if let Some(rest) = line.strip_prefix("assign ") {
            let (name, rhs) = rest.split_once('=').expect("assign 缺 =");
            assigns.push((
                name.trim().to_string(),
                rhs.trim_end_matches(';').trim().to_string(),
            ));
        }
    }
    (inputs, assigns)
}

fn eval_design(assigns: &[(String, String)], env: &mut BTreeMap<String, u8>) {
    for (name, rhs) in assigns {
        let value = eval_expr(rhs, env);
        env.insert(name.clone(), value);
    }
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
    let (inputs, assigns) = parse_design(text);
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
        eval_design(&assigns, &mut env);
        let (pos, neg) = (env["y_p"], env["y_n"]);
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
        let (inputs, assigns) = parse_design(FIXTURE);
        assert_eq!(inputs.len(), 6);
        assert_eq!(assigns.len(), 6, "4 个门 wire + 2 个输出");
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
        eval_design(&assigns, &mut env);
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
}
