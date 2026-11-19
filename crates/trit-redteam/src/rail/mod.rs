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

mod analyze;
mod parser;

pub(crate) use analyze::{analyze, has_findings};
pub(crate) use parser::eval_expr;

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

#[cfg(test)]
mod tests {
    use super::*;

    /// 由 `trit-build --emit-verilog` 对 `(a & b) | !c` 生成的真实导出。
    const FIXTURE: &str = include_str!("../../fixtures/three_rail.v");

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
