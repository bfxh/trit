//! 三值栈式解释器：遇到 `X` 分支就分叉，最后按「路径结果」给结论。
//!
//! 与二值解释器不同，`Jz` 的条件是 `X` 时不猜：两条路都走。于是同一个程序会得到
//! **一组**可能输出而不是一个值；只有一个输出才算确定，否则并列所有结果与路径数。

use std::collections::BTreeMap;

use trit_core::value::Ternary;

/// 指令集（栈式，`usize` 是程序计数器）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Op {
    /// 压入常量。
    Push(Ternary),
    /// 复制栈顶。
    Dup,
    /// 三值取反。
    Not,
    /// 三值合取。
    And,
    /// 三值析取。
    Or,
    /// 三值异或（按当前 `Algebra::Kleene` 的强加法语义由调用方选择输入）。
    Xor,
    /// 无条件跳转。
    Jmp(usize),
    /// 弹出条件：`False` 跳转，`True` 顺序执行，`X` **两条都走**。
    Jz(usize),
    /// 停机，栈顶即输出。
    Halt,
}

/// 一次运行的结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunReport {
    /// 每个可能输出及其路径数；`X` 也会作为具体输出出现。
    pub outputs: BTreeMap<Ternary, usize>,
    /// 是否所有路径都停了（预算内）。
    pub halted: bool,
    /// 走过的路径总数。
    pub paths: usize,
    /// 消耗的总步数。
    pub steps: usize,
    /// 是否因为超出步数/路径预算而截断。
    pub truncated: bool,
}

impl RunReport {
    /// 只有一个输出才算确定。
    pub fn verdict(&self) -> Option<Ternary> {
        if self.truncated {
            return None;
        }
        let mut iter = self.outputs.keys();
        match (iter.next(), iter.next()) {
            (Some(value), None) => Some(*value),
            _ => None,
        }
    }
}

/// 解释器的执行预算。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limits {
    /// 单条路径最多执行多少步。
    pub max_steps: usize,
    /// 最多保留多少条并发路径。
    pub max_paths: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            max_steps: 1_000,
            max_paths: 256,
        }
    }
}

/// 运行程序；每条路径有自己的栈与程序计数器。
pub fn run(program: &[Op], limits: Limits) -> RunReport {
    let mut outputs: BTreeMap<Ternary, usize> = BTreeMap::new();
    let mut steps = 0usize;
    let mut paths = 1usize;
    let mut truncated = false;
    let mut queue: Vec<(usize, Vec<Ternary>)> = vec![(0, Vec::new())];
    'path: while let Some((mut pc, mut stack)) = queue.pop() {
        loop {
            if steps >= limits.max_steps {
                truncated = true;
                break 'path;
            }
            steps += 1;
            let Some(op) = program.get(pc).copied() else {
                truncated = true;
                break 'path;
            };
            match op {
                Op::Push(value) => {
                    stack.push(value);
                    pc += 1;
                }
                Op::Dup => match stack.last().copied() {
                    Some(value) => {
                        stack.push(value);
                        pc += 1;
                    }
                    None => {
                        truncated = true;
                        break 'path;
                    }
                },
                Op::Not | Op::And | Op::Or | Op::Xor => {
                    let arity = if op == Op::Not { 1 } else { 2 };
                    if stack.len() < arity {
                        truncated = true;
                        break 'path;
                    }
                    let value = match op {
                        Op::Not => match stack.pop().unwrap() {
                            Ternary::False => Ternary::True,
                            Ternary::True => Ternary::False,
                            Ternary::Unknown => Ternary::Unknown,
                        },
                        _ => {
                            let b = stack.pop().unwrap();
                            let a = stack.pop().unwrap();
                            match op {
                                Op::And => and(a, b),
                                Op::Or => or(a, b),
                                _ => xor(a, b),
                            }
                        }
                    };
                    stack.push(value);
                    pc += 1;
                }
                Op::Jmp(target) => pc = target,
                Op::Jz(target) => match stack.pop() {
                    Some(Ternary::False) => pc = target,
                    Some(Ternary::True) => pc += 1,
                    Some(Ternary::Unknown) => {
                        if paths >= limits.max_paths {
                            truncated = true;
                            break 'path;
                        }
                        paths += 1;
                        queue.push((target, stack.clone()));
                        pc += 1;
                    }
                    None => {
                        truncated = true;
                        break 'path;
                    }
                },
                Op::Halt => {
                    // 空栈停机与 DUP/JZ/二元运算空栈口径一致：判截断，不伪造 Unknown。
                    let Some(value) = stack.last().copied() else {
                        truncated = true;
                        break 'path;
                    };
                    *outputs.entry(value).or_insert(0) += 1;
                    break;
                }
            }
        }
    }
    RunReport {
        halted: !truncated,
        outputs,
        paths,
        steps,
        truncated,
    }
}

fn and(a: Ternary, b: Ternary) -> Ternary {
    match (a, b) {
        (Ternary::False, _) | (_, Ternary::False) => Ternary::False,
        (Ternary::True, Ternary::True) => Ternary::True,
        _ => Ternary::Unknown,
    }
}

fn or(a: Ternary, b: Ternary) -> Ternary {
    match (a, b) {
        (Ternary::True, _) | (_, Ternary::True) => Ternary::True,
        (Ternary::False, Ternary::False) => Ternary::False,
        _ => Ternary::Unknown,
    }
}

fn xor(a: Ternary, b: Ternary) -> Ternary {
    match (a, b) {
        (Ternary::Unknown, _) | (_, Ternary::Unknown) => Ternary::Unknown,
        (Ternary::True, Ternary::True) | (Ternary::False, Ternary::False) => Ternary::False,
        _ => Ternary::True,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn masking_through_unknown_is_definite() {
        let program = [
            Op::Push(Ternary::Unknown),
            Op::Push(Ternary::False),
            Op::And,
            Op::Halt,
        ];
        let report = run(&program, Limits::default());
        assert_eq!(report.verdict(), Some(Ternary::False));
        assert_eq!(report.paths, 1);
        assert!(!report.truncated);
    }

    #[test]
    fn kleene_cannot_see_reconvergent_correlation() {
        let program = [
            Op::Push(Ternary::Unknown),
            Op::Dup,
            Op::Not,
            Op::And,
            Op::Halt,
        ];
        let report = run(&program, Limits::default());
        assert_eq!(report.verdict(), Some(Ternary::Unknown));
    }

    #[test]
    fn branch_on_unknown_forks_but_agrees() {
        let program = [
            Op::Push(Ternary::Unknown),
            Op::Jz(5),
            Op::Push(Ternary::True),
            Op::Halt,
            Op::Halt,
            Op::Push(Ternary::True),
            Op::Halt,
        ];
        let report = run(&program, Limits::default());
        assert_eq!(report.verdict(), Some(Ternary::True));
        assert_eq!(report.paths, 2);
        assert_eq!(report.outputs[&Ternary::True], 2);
    }

    #[test]
    fn branch_on_unknown_with_different_outputs_is_ambiguous() {
        let program = [
            Op::Push(Ternary::Unknown),
            Op::Jz(5),
            Op::Push(Ternary::True),
            Op::Halt,
            Op::Halt,
            Op::Push(Ternary::False),
            Op::Halt,
        ];
        let report = run(&program, Limits::default());
        assert_eq!(report.verdict(), None);
        assert_eq!(report.outputs.len(), 2);
        assert!(report.outputs.contains_key(&Ternary::True));
        assert!(report.outputs.contains_key(&Ternary::False));
    }

    #[test]
    fn known_branch_does_not_fork() {
        let program = [
            Op::Push(Ternary::False),
            Op::Jz(5),
            Op::Push(Ternary::True),
            Op::Halt,
            Op::Halt,
            Op::Push(Ternary::False),
            Op::Halt,
        ];
        let report = run(&program, Limits::default());
        assert_eq!(report.verdict(), Some(Ternary::False));
        assert_eq!(report.paths, 1);
    }

    #[test]
    fn infinite_loop_is_reported_as_truncated_not_guessed() {
        let program = [Op::Jmp(0)];
        let report = run(
            &program,
            Limits {
                max_steps: 50,
                max_paths: 8,
            },
        );
        assert!(report.truncated);
        assert!(!report.halted);
        assert_eq!(report.verdict(), None);
    }

    #[test]
    fn path_budget_stops_forking() {
        let program = [
            Op::Push(Ternary::Unknown),
            Op::Jz(6),
            Op::Push(Ternary::Unknown),
            Op::Jz(6),
            Op::Push(Ternary::True),
            Op::Halt,
            Op::Push(Ternary::True),
            Op::Halt,
        ];
        let report = run(
            &program,
            Limits {
                max_steps: 100,
                max_paths: 2,
            },
        );
        assert!(report.truncated);
        assert!(report.paths <= 2);
    }
}
