//! 时序 AIGER（`L > 0`）的读写与三值步进。
//!
//! 与组合网表 [`crate::circuit::Circuit`] **分开**：时序网表有锁存器（初始值 + 下一状态），
//! 把它塞进组合检查（validate / sim / atpg）会把寄存器当组合门，语义就错了。
//! 这里用独立的最小类型，只做两件事：AIGER 时序往返、三值步进。
//!
//! 变量编号按 AIGER 标准：`1..=I` 是输入，`I+1..=I+L` 是锁存器，`I+L+1..=M` 是 AND 门；
//! 字面量 `0` = false、`1` = true、`2v` = 变量 `v`、`2v+1` = `¬v`。

use trit_core::value::Ternary;

/// AIGER 字面量。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Literal(pub u32);

impl Literal {
    /// 常量 false。
    pub const FALSE: Self = Self(0);
    /// 常量 true。
    pub const TRUE: Self = Self(1);

    /// 变量 `v` 的正字面量。
    #[must_use]
    pub const fn var(v: u32) -> Self {
        Self(2 * v)
    }

    /// 是否取反。
    #[must_use]
    pub const fn is_negated(self) -> bool {
        self.0 & 1 == 1
    }

    /// 变量号（对取反字面量也返回其变量）。
    #[must_use]
    pub const fn index(self) -> u32 {
        self.0 >> 1
    }

    /// 取反。
    #[must_use]
    pub const fn negate(self) -> Self {
        Self(self.0 ^ 1)
    }
}

/// 一个锁存器：初始值 + 下一状态字面量。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Latch {
    /// 上电初始值。
    pub init: bool,
    /// 下一状态。
    pub next: Literal,
}

/// 一个 AND 门：`lhs = rhs0 ∧ rhs1`。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AndGate {
    /// 输出变量号。
    pub lhs: u32,
    /// 左输入。
    pub rhs0: Literal,
    /// 右输入。
    pub rhs1: Literal,
}

/// 时序网表。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SequentialCircuit {
    /// 输入个数。
    pub inputs: usize,
    /// 锁存器。
    pub latches: Vec<Latch>,
    /// 输出字面量。
    pub outputs: Vec<Literal>,
    /// AND 门（必须按拓扑序排列）。
    pub ands: Vec<AndGate>,
}

/// 解析/生成错误。
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SequentialError {
    /// 某一行语法错误。
    #[error("第 {line} 行：{message}")]
    Line {
        /// 行号（1 起）。
        line: usize,
        /// 说明。
        message: String,
    },
    /// 头部不是 `aag M I L O A`。
    #[error("AIGER 头部必须是 `aag M I L O A`")]
    Header,
    /// 组合 AIGER（`L = 0`）。
    #[error("这是组合 AIGER（L = 0），组合网表请用 `aiger::parse_aiger`")]
    NotSequential,
    /// 字面量超出变量范围。
    #[error("字面量 {0} 超出变量范围（最大 {1}）")]
    LiteralOutOfRange(u32, u32),
    /// AND 门的 `lhs` 不合法。
    #[error("AND 门的 lhs 必须是变量 {i}..={m} 的偶数倍")]
    BadLhs {
        /// 期望的最小变量号。
        i: u32,
        /// 最大变量号。
        m: u32,
    },
}

fn num(text: &str, line: usize, what: &str) -> Result<usize, SequentialError> {
    text.parse::<usize>().map_err(|_| SequentialError::Line {
        line,
        message: format!("{what} {text:?} 不是无符号整数"),
    })
}

fn next_line<'a, I: Iterator<Item = &'a str>>(
    lines: &mut I,
    line_no: &mut usize,
) -> Result<&'a str, SequentialError> {
    *line_no += 1;
    lines.next().ok_or(SequentialError::Line {
        line: *line_no,
        message: "行数不足".into(),
    })
}

/// 解析时序 AIGER（ASCII `aag`，`L > 0`）。
pub fn parse_sequential(text: &str) -> Result<SequentialCircuit, SequentialError> {
    let mut lines = text.lines().map(str::trim).filter(|l| !l.is_empty());

    let header = lines.next().ok_or(SequentialError::Header)?;
    let parts: Vec<&str> = header.split_whitespace().collect();
    if parts.len() != 6 || parts[0] != "aag" {
        return Err(SequentialError::Header);
    }
    let m = num(parts[1], 1, "M")?;
    let i = num(parts[2], 1, "I")?;
    let l = num(parts[3], 1, "L")?;
    let o = num(parts[4], 1, "O")?;
    let a = num(parts[5], 1, "A")?;
    if l == 0 {
        return Err(SequentialError::NotSequential);
    }

    let mut line_no = 1usize;

    // 输入字面量（值本身不使用，变量编号按位置固定）。
    for _ in 0..i {
        let _ = next_line(&mut lines, &mut line_no)?;
    }

    // 锁存器初始值：0/1。
    let mut latches = Vec::with_capacity(l);
    for _ in 0..l {
        let text = next_line(&mut lines, &mut line_no)?;
        let init = match text {
            "0" => false,
            "1" => true,
            other => {
                return Err(SequentialError::Line {
                    line: line_no,
                    message: format!("锁存器初始值必须是 0/1，得到 {other:?}"),
                });
            }
        };
        latches.push(Latch {
            init,
            next: Literal::FALSE,
        });
    }

    // 输出字面量。
    let mut outputs = Vec::with_capacity(o);
    for _ in 0..o {
        let text = next_line(&mut lines, &mut line_no)?;
        outputs.push(Literal(num(text, line_no, "输出字面量")? as u32));
    }

    // AND 门。
    let mut ands = Vec::with_capacity(a);
    for idx in 0..a {
        let text = next_line(&mut lines, &mut line_no)?;
        let parts: Vec<&str> = text.split_whitespace().collect();
        if parts.len() != 3 {
            return Err(SequentialError::Line {
                line: line_no,
                message: "AND 门必须是 `lhs rhs0 rhs1`".into(),
            });
        }
        let lhs = num(parts[0], line_no, "lhs")? as u32;
        let expect = (i + l + idx + 1) as u32;
        if lhs != 2 * expect {
            return Err(SequentialError::BadLhs {
                i: expect,
                m: m as u32,
            });
        }
        ands.push(AndGate {
            lhs: expect,
            rhs0: Literal(num(parts[1], line_no, "rhs0")? as u32),
            rhs1: Literal(num(parts[2], line_no, "rhs1")? as u32),
        });
    }

    // 锁存器 next。
    for latch in &mut latches {
        let text = next_line(&mut lines, &mut line_no)?;
        latch.next = Literal(num(text, line_no, "next 字面量")? as u32);
    }

    let max_lit = 2 * m as u32 + 1;
    for out in &outputs {
        if out.0 > max_lit {
            return Err(SequentialError::LiteralOutOfRange(out.0, max_lit));
        }
    }
    for gate in &ands {
        for lit in [gate.rhs0, gate.rhs1] {
            if lit.0 > max_lit {
                return Err(SequentialError::LiteralOutOfRange(lit.0, max_lit));
            }
        }
    }

    Ok(SequentialCircuit {
        inputs: i,
        latches,
        outputs,
        ands,
    })
}

/// 生成时序 AIGER（ASCII `aag`）。
#[must_use]
pub fn to_sequential(c: &SequentialCircuit) -> String {
    let i = c.inputs;
    let l = c.latches.len();
    let o = c.outputs.len();
    let a = c.ands.len();
    let m = i + l + a;

    let mut out = String::new();
    out.push_str(&format!("aag {m} {i} {l} {o} {a}\n"));
    for k in 1..=i {
        out.push_str(&format!("{}\n", 2 * k));
    }
    for latch in &c.latches {
        out.push_str(if latch.init { "1\n" } else { "0\n" });
    }
    for lit in &c.outputs {
        out.push_str(&format!("{}\n", lit.0));
    }
    for gate in &c.ands {
        out.push_str(&format!(
            "{} {} {}\n",
            gate.lhs * 2,
            gate.rhs0.0,
            gate.rhs1.0
        ));
    }
    for latch in &c.latches {
        out.push_str(&format!("{}\n", latch.next.0));
    }
    out
}

fn eval(lit: Literal, vars: &[Ternary]) -> Ternary {
    match lit.0 {
        0 => Ternary::False,
        1 => Ternary::True,
        raw => {
            let v = vars[(raw >> 1) as usize];
            if raw & 1 == 1 { !v } else { v }
        }
    }
}

impl SequentialCircuit {
    /// 走一拍：给定输入与当前状态，返回（下一状态、输出）。
    ///
    /// 三值语义：AND 用 Kleene 合取，`X` 会正常传播。
    #[must_use]
    pub fn step(&self, inputs: &[Ternary], state: &[Ternary]) -> (Vec<Ternary>, Vec<Ternary>) {
        assert_eq!(inputs.len(), self.inputs, "输入个数不符");
        assert_eq!(state.len(), self.latches.len(), "状态个数不符");

        let m = self.inputs + self.latches.len() + self.ands.len();
        let mut vars = vec![Ternary::False; m + 1];
        for (k, value) in inputs.iter().enumerate() {
            vars[k + 1] = *value;
        }
        for (k, value) in state.iter().enumerate() {
            vars[self.inputs + k + 1] = *value;
        }
        for gate in &self.ands {
            let a = eval(gate.rhs0, &vars);
            let b = eval(gate.rhs1, &vars);
            vars[gate.lhs as usize] = a & b;
        }

        let next = self
            .latches
            .iter()
            .map(|latch| eval(latch.next, &vars))
            .collect();
        let out = self.outputs.iter().map(|l| eval(*l, &vars)).collect();
        (next, out)
    }

    /// 初始状态。
    #[must_use]
    pub fn initial_state(&self) -> Vec<Ternary> {
        self.latches
            .iter()
            .map(|l| {
                if l.init {
                    Ternary::True
                } else {
                    Ternary::False
                }
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 2 位计数器：状态 = (lo, hi)，每个节拍 +1。
    ///
    /// `lo' = ¬lo`，`hi' = hi ⊕ lo`。AIG 只有 AND 与取反，所以 XOR 展开成
    /// `¬(¬(lo ∧ ¬hi) ∧ ¬(¬lo ∧ hi))`，三个 AND 门。
    fn counter2() -> SequentialCircuit {
        SequentialCircuit {
            inputs: 0,
            latches: vec![
                Latch {
                    init: false,
                    next: Literal::var(1).negate(),
                },
                Latch {
                    init: false,
                    next: Literal::var(5),
                },
            ],
            outputs: vec![Literal::var(2)],
            ands: vec![
                AndGate {
                    lhs: 3,
                    rhs0: Literal::var(1),
                    rhs1: Literal::var(2).negate(),
                },
                AndGate {
                    lhs: 4,
                    rhs0: Literal::var(1).negate(),
                    rhs1: Literal::var(2),
                },
                AndGate {
                    lhs: 5,
                    rhs0: Literal::var(3).negate(),
                    rhs1: Literal::var(4).negate(),
                },
            ],
        }
    }

    #[test]
    fn roundtrip_is_stable() {
        let c = counter2();
        let text = to_sequential(&c);
        let back = parse_sequential(&text).expect("解析");
        assert_eq!(back, c);
    }

    #[test]
    fn counter_wraps_like_a_real_counter() {
        let c = counter2();
        let mut state = c.initial_state(); // (0, 0)
        let mut seen = vec![(state[0], state[1])];
        for _ in 0..4 {
            let (next, out) = c.step(&[], &state);
            assert_eq!(next.len(), 2);
            assert_eq!(out.len(), 1);
            state = next;
            seen.push((state[0], state[1]));
        }
        // 0→1→2→3→0：lo 每拍翻，hi 在 lo 由 1 回 0 时翻。
        let bits: Vec<(bool, bool)> = seen
            .iter()
            .map(|(lo, hi)| (*lo == Ternary::True, *hi == Ternary::True))
            .collect();
        assert_eq!(
            bits,
            vec![
                (false, false),
                (true, true),
                (false, true),
                (true, false),
                (false, false)
            ],
            "2 位计数器应循环 00→11→01→10→00（lo 低位在前）"
        );
    }

    #[test]
    fn x_propagates_through_and() {
        let c = counter2();
        // lo = X、hi = 0：hi' = hi ⊕ lo = X。
        let (next, _) = c.step(&[], &[Ternary::Unknown, Ternary::False]);
        assert_eq!(next[1], Ternary::Unknown, "unknown 应传播到 hi'");
    }

    #[test]
    fn rejects_combinational() {
        let text = "aag 1 1 0 1 0\n2\n2\n";
        assert_eq!(
            parse_sequential(text).unwrap_err(),
            SequentialError::NotSequential
        );
    }

    #[test]
    fn literal_helpers() {
        assert_eq!(Literal::var(3).0, 6);
        assert!(Literal::var(3).negate().is_negated());
        assert_eq!(Literal::var(3).negate().index(), 3);
        assert_eq!(Literal::FALSE.0, 0);
        assert_eq!(Literal::TRUE.0, 1);
    }
}
