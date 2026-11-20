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
                    let value = stack.last().copied().unwrap_or(Ternary::Unknown);
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
        // X & 0 = 0：三值运算本身就能给出确定值，不需要分叉。
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
        // 栈机没有相关性分析：X & ~X 仍是 X（这正是需要 X-Pessimism 分析器的地方）。
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
        // if X { 1 } else { 1 }：两条路径、同一个输出。
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
        // 连续两次 X 分支 -> 4 条路径；预算 2 时截断。
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

    #[test]
    fn ternary_connectives_cover_all_nine_cells() {
        // 三值的三个连接词就是三张 9 格表。任何一格错了，"在 X 下仍能确定"
        // 这条能力就变成了错误的确定性——比直接返回 X 更危险。
        use Ternary::{False as F, True as T, Unknown as X};
        let table = [F, X, T];
        for a in table {
            for b in table {
                // 与：F 吸收（含 X&F=F）；T&T 才 T；其余 X。
                let expect_and = match (a, b) {
                    (F, _) | (_, F) => F,
                    (T, T) => T,
                    _ => X,
                };
                // 或：T 吸收；只有 F∨F 才是 F。
                let expect_or = match (a, b) {
                    (T, _) | (_, T) => T,
                    (F, F) => F,
                    _ => X,
                };
                // 异或：X 传染；同号为 F，异号为 T。
                let expect_xor = match (a, b) {
                    (X, _) | (_, X) => X,
                    (T, T) | (F, F) => F,
                    _ => T,
                };
                let program = |op| {
                    run(&[Op::Push(a), Op::Push(b), op, Op::Halt], Limits::default())
                        .verdict()
                        .expect("单路径必须给出结论")
                };
                assert_eq!(program(Op::And), expect_and, "AND {a:?},{b:?}");
                assert_eq!(program(Op::Or), expect_or, "OR {a:?},{b:?}");
                assert_eq!(program(Op::Xor), expect_xor, "XOR {a:?},{b:?}");
            }
        }
        // 取反：只有 X 保持 X。
        for value in table {
            let expect = match value {
                F => T,
                T => F,
                X => X,
            };
            assert_eq!(
                run(&[Op::Push(value), Op::Not, Op::Halt], Limits::default())
                    .verdict()
                    .expect("单路径"),
                expect,
                "NOT {value:?}"
            );
        }
    }

    #[test]
    fn stack_underflow_truncates_instead_of_panicking() {
        // 解释器跑的是外部给的字节码：栈不够只能算"这一趟没跑完"，
        // 不能崩，否则一份坏程序就能把审计进程带走。
        for program in [
            vec![Op::Not, Op::Halt],
            vec![Op::Push(Ternary::True), Op::And, Op::Halt],
            vec![Op::Dup, Op::Halt],
            vec![Op::Jz(0)],
        ] {
            let report = run(&program, Limits::default());
            assert!(report.truncated, "应判为截断：{program:?}");
            assert!(!report.halted);
            assert_eq!(report.verdict(), None, "截断时不能给出结论");
        }
    }

    #[test]
    fn halt_with_an_empty_stack_reports_unknown() {
        // 空栈停机：没有可报告的结论，但那不是崩，而是 X。
        let report = run(&[Op::Halt], Limits::default());
        assert_eq!(report.outputs.len(), 1);
        assert_eq!(report.outputs[&Ternary::Unknown], 1);
        assert_eq!(report.verdict(), Some(Ternary::Unknown), "X 是确定的结论");
    }

    #[test]
    fn falling_off_the_end_of_the_program_is_truncation() {
        // 没有 Halt 的程序：走到末尾仍然没有结论 ⇒ 截断（而不是"默认返回 X"）。
        let report = run(&[Op::Push(Ternary::True)], Limits::default());
        assert!(report.truncated);
        assert!(report.outputs.is_empty());
        assert_eq!(report.verdict(), None);
    }

    #[test]
    fn a_truncated_run_never_claims_a_verdict_even_with_one_output() {
        // 关键：截断优先于"只有一个输出"。已经见过的路径全指向 T，
        // 但没走到的路径可能指向别的——此时说"结论是 T"就是作弊。
        let program = [
            Op::Push(Ternary::Unknown),
            Op::Jz(5),
            Op::Push(Ternary::True),
            Op::Halt,
            Op::Halt,
            Op::Push(Ternary::True),
            Op::Halt,
            Op::Jmp(0),
        ];
        let report = run(
            &program,
            Limits {
                max_steps: 5,
                max_paths: 8,
            },
        );
        assert!(report.truncated, "{report:?}");
        assert!(
            !report.outputs.is_empty(),
            "已经有一条路径停了——这正是最容易误判成结论的情形：{report:?}"
        );
        assert_eq!(report.verdict(), None, "截断时即便只有一个输出也不许下结论");
    }

    #[test]
    fn jumps_terminate_when_a_halt_is_reachable() {
        // 回边本身不是问题：预算够、有出口就应当停。
        let program = [
            Op::Push(Ternary::True), // 0
            Op::Jmp(0),              // 1 不可达
            Op::Halt,                // 2 不可达
        ];
        let report = run(
            &program,
            Limits {
                max_steps: 10,
                max_paths: 4,
            },
        );
        assert!(report.truncated, "这条确实没有出口");

        let program = [
            Op::Push(Ternary::True), // 0
            Op::Jz(3),               // 1 T ⇒ 顺序执行（Jz 会弹出条件）
            Op::Halt,                // 2 在这里停：栈已被 Jz 弹空 ⇒ 输出 X
            Op::Halt,                // 3 走不到
        ];
        let report = run(&program, Limits::default());
        assert!(!report.truncated, "有出口就该停：{report:?}");
        assert_eq!(
            report.verdict(),
            Some(Ternary::Unknown),
            "Jz 弹出条件后栈空，Halt 只能报 X：这正是『确定地不知道』"
        );
        assert_eq!(report.paths, 1, "已知条件不分叉");
    }

    #[test]
    fn dup_and_swap_free_arithmetic_stays_consistent() {
        // (X & 1) | (X & 0)：左支 X、右支 F ⇒ X。栈机必须算对。
        let program = [
            Op::Push(Ternary::Unknown),
            Op::Dup,
            Op::Push(Ternary::True),
            Op::And,
            Op::Push(Ternary::Unknown),
            Op::Push(Ternary::False),
            Op::And,
            Op::Or,
            Op::Halt,
        ];
        let report = run(&program, Limits::default());
        assert_eq!(report.verdict(), Some(Ternary::Unknown));
        assert_eq!(report.paths, 1, "这里没有分叉，只有三值运算");
    }
}
