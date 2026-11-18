//! M0：从三值前馈网络提取最小 DFA，并与参考语言对拍。
//!
//! 判据（`docs/research/WSP-INTERNAL-LOGIC.md` §6）：
//! - 提取的 DFA 与原网络在声明域上逐串相等（DIFF=0）；
//! - 状态数等于前缀树最小化后的等价类数（Moore 最小化给出最小）；
//! - 与参考语言的最小 DFA 状态数一致、逐串相等。
//!
//! 域写在输出里（`max_len` 与 `words`），域外不主张任何等价性。
//! **「最小」是相对声明域的最小**：域截断后「剩余可扩展长度」成为状态的一部分，
//! 所以域内状态数会大于该语言在全域上的经典最小 DFA（本例域内 7、全域 3）。

mod cases;
mod compile;
mod dfa;
mod intervene;
mod recurrent;
mod trace;
mod word;

use trit_wspace::Trit;

/// 一个案例的读数。
struct Report {
    name: &'static str,
    max_len: usize,
    words: usize,
    accepted: usize,
    states_net: usize,
    states_ref: usize,
    diff_net_vs_dfa: usize,
    diff_dfa_vs_ref: usize,
    hidden_classes: usize,
    hidden_to_out_conflicts: usize,
    hidden_to_state_conflicts: usize,
    max_variants: usize,
}

impl Report {
    fn pass(&self) -> bool {
        self.diff_net_vs_dfa == 0
            && self.diff_dfa_vs_ref == 0
            && self.states_net == self.states_ref
            && self.hidden_to_out_conflicts == 0
    }
}

/// 打印逐串轨迹（`--trace`）：串、层 1 激活、输出层 pre-activation、输出、DFA 状态。
fn print_traces(case: &cases::Case, dfa: &dfa::Dfa) {
    for w in word::all_words(case.max_len) {
        let t = trace::trace(&case.net, &w, case.max_len);
        let bits: Vec<bool> = w.iter().map(|x| *x == Trit::Pos).collect();
        let hidden: Vec<i8> = t.hidden.iter().map(|x| x.to_i8()).collect();
        println!(
            "    word={:<3} hidden={:?} pre_out={:>2} out={:?} state=q{}",
            word::bits(&t.word),
            hidden,
            t.pre_out,
            t.out,
            dfa.state_of(&bits)
        );
    }
}

/// 跑一个案例。
fn run(case: &cases::Case, dump: bool) -> Report {
    let max_len = case.max_len;
    let words = word::all_words(max_len);

    // 路线 1：网络判决 → 前缀树 → 最小化。
    let net_dfa = dfa::from_accepts(max_len, |w| {
        let input = word::encode(w, max_len);
        case.net
            .forward(&input)
            .map(|out| out[0] == Trit::Pos)
            .unwrap_or(false)
    })
    .minimize();

    // 路线 2：参考语言 → 前缀树 → 最小化（ground truth）。
    let ref_dfa = dfa::from_accepts(max_len, case.lang).minimize();

    if dump {
        print_traces(case, &net_dfa);
    }

    // 轨迹层：层 1 激活与 DFA 状态的对齐。
    let align = trace::align(&case.net, max_len, &net_dfa);

    let mut report = Report {
        name: case.name,
        max_len,
        words: words.len(),
        accepted: 0,
        states_net: net_dfa.states,
        states_ref: ref_dfa.states,
        diff_net_vs_dfa: 0,
        diff_dfa_vs_ref: 0,
        hidden_classes: align.hidden_classes,
        hidden_to_out_conflicts: align.hidden_to_out_conflicts,
        hidden_to_state_conflicts: align.hidden_to_state_conflicts,
        max_variants: align.max_variants,
    };
    for w in &words {
        let bits: Vec<bool> = w.iter().map(|t| *t == Trit::Pos).collect();
        let input = word::encode(w, max_len);
        let net_out = case.net.forward(&input).expect("编码后维度一致")[0] == Trit::Pos;
        let dfa_out = net_dfa.run(&bits);
        let ref_out = ref_dfa.run(&bits);
        if net_out != dfa_out {
            report.diff_net_vs_dfa += 1;
            println!("  mismatch net/dfa on {:?}", word::bits(w));
        }
        if dfa_out != ref_out {
            report.diff_dfa_vs_ref += 1;
            println!("  mismatch dfa/ref on {:?}", word::bits(w));
        }
        if net_out {
            report.accepted += 1;
        }
    }
    report
}

/// `parity` 程序：偶数个 1。
fn parity_program() -> dfa::Dfa {
    dfa::Dfa {
        states: 2,
        start: 0,
        accept: vec![true, false],
        delta: vec![[0, 1], [1, 0]],
    }
}

/// 状态爆炸预算：扫 `max_len`，看域内最小状态数怎么长。
fn budget() {
    let program = parity_program();
    println!("max_len words states_ref states_net same");
    for max_len in 1..=14 {
        let net = compile::compile(&program, max_len).expect("编译 parity");
        let extracted =
            dfa::from_accepts(max_len, |w| compile::run(&net, w, max_len)).minimize();
        let reference = dfa::from_accepts(max_len, |w| {
            let bits: Vec<bool> = w.iter().map(|t| *t == Trit::Pos).collect();
            program.run(&bits)
        })
        .minimize();
        let words = (1usize << (max_len + 1)) - 1;
        println!(
            "{} {} {} {} {}",
            max_len,
            words,
            reference.states,
            extracted.states,
            extracted.states == reference.states
        );
    }
}

fn programs() -> Vec<(&'static str, dfa::Dfa)> {
    vec![
        (
            "parity",
            dfa::Dfa {
                states: 2,
                start: 0,
                accept: vec![true, false],
                delta: vec![[0, 1], [1, 0]],
            },
        ),
        (
            "mod3",
            dfa::Dfa {
                states: 3,
                start: 0,
                accept: vec![true, false, false],
                delta: vec![[0, 1], [1, 2], [2, 0]],
            },
        ),
        (
            "paren",
            dfa::Dfa {
                states: 3,
                start: 0,
                accept: vec![true, false, false],
                delta: vec![[0, 1], [0, 2], [1, 2]],
            },
        ),
        (
            // 目标状态 0 有 4 个转移源，专门用来验证 OR 扇入 > 2。
            "four_sources",
            dfa::Dfa {
                states: 3,
                start: 0,
                accept: vec![true, false, false],
                delta: vec![[0, 0], [0, 1], [2, 2]],
            },
        ),
        (
            "ends_with_1",
            dfa::Dfa {
                states: 2,
                start: 0,
                accept: vec![false, true],
                delta: vec![[0, 1], [0, 1]],
            },
        ),
        (
            // 目标状态 2 有 3 个转移源。
            "contains_01",
            dfa::Dfa {
                states: 3,
                start: 0,
                accept: vec![false, false, true],
                delta: vec![[1, 0], [1, 2], [2, 2]],
            },
        ),
        (
            // 5 状态：模 5 计数，接受 q0。
            "mod5",
            dfa::Dfa {
                states: 5,
                start: 0,
                accept: vec![true, false, false, false, false],
                delta: vec![[0, 1], [1, 2], [2, 3], [3, 4], [4, 0]],
            },
        ),
        (
            // 8 状态，目标状态 0 有 9 个转移源（8 个 q 的 0 边 + q7 的 1 边），
            // 专门压 OR 扇入与常量单元数量。
            "star8",
            dfa::Dfa {
                states: 8,
                start: 0,
                accept: vec![true, false, false, false, false, false, false, false],
                delta: vec![
                    [0, 1],
                    [0, 2],
                    [0, 3],
                    [0, 4],
                    [0, 5],
                    [0, 6],
                    [0, 7],
                    [0, 0],
                ],
            },
        ),
        (
            // 20 状态：模 20 计数，压状态数上限（远超 M0 之前的 10 状态）。
            "mod20",
            dfa::Dfa {
                states: 20,
                start: 0,
                accept: vec![true, false, false, false, false, false, false, false, false, false, false, false, false, false, false, false, false, false, false, false],
                delta: vec![
                    [0, 1],
                    [1, 2],
                    [2, 3],
                    [3, 4],
                    [4, 5],
                    [5, 6],
                    [6, 7],
                    [7, 8],
                    [8, 9],
                    [9, 10],
                    [10, 11],
                    [11, 12],
                    [12, 13],
                    [13, 14],
                    [14, 15],
                    [15, 16],
                    [16, 17],
                    [17, 18],
                    [18, 19],
                    [19, 0],
                ],
            },
        ),
        (
            // 10 状态：模 10 计数，压状态数上限。
            "mod10",
            dfa::Dfa {
                states: 10,
                start: 0,
                accept: vec![true, false, false, false, false, false, false, false, false, false],
                delta: vec![
                    [0, 1],
                    [1, 2],
                    [2, 3],
                    [3, 4],
                    [4, 5],
                    [5, 6],
                    [6, 7],
                    [7, 8],
                    [8, 9],
                    [9, 0],
                ],
            },
        ),
    ]
}


/// 打印循环执行的轨迹（`--trace-cell`）：每步之后的状态向量。
fn trace_cell() {
    for (name, program) in programs().iter().take(3) {
        let cell = compile::compile_cell(program).expect("cell");
        for w in word::all_words(2) {
            let r = recurrent::run(&cell, program, &w, 2);
            let trace: Vec<Vec<i8>> = r
                .trace
                .iter()
                .map(|s| s.iter().map(|t| t.to_i8()).collect())
                .collect();
            println!(
                "{name} {:?}: trace={trace:?} accept={}",
                word::bits(&w),
                r.accept
            );
        }
    }
}

fn main() {
    if std::env::args().any(|a| a == "--budget") {
        budget();
        return;
    }
    if std::env::args().any(|a| a == "--trace-cell") {
        trace_cell();
        return;
    }

    let dump = std::env::args().any(|a| a == "--trace");
    let dump_int = std::env::args().any(|a| a == "--intervene");
    let mut failed = false;
    for case in cases::all() {
        let r = run(&case, dump);
        println!(
            "case={} max_len={} words={} accepted={}",
            r.name, r.max_len, r.words, r.accepted
        );
        println!(
            "  dfa states: net={} ref={}",
            r.states_net, r.states_ref
        );
        println!(
            "  diff: net_vs_dfa={} dfa_vs_ref={}",
            r.diff_net_vs_dfa, r.diff_dfa_vs_ref
        );
        println!(
            "  trace: hidden_classes={} hidden_to_out_conflicts={} hidden_to_state_conflicts={} max_variants={}",
            r.hidden_classes, r.hidden_to_out_conflicts, r.hidden_to_state_conflicts, r.max_variants
        );
        if dump_int {
            let units = case.net.layer(0).expect("有层 1").rows();
            let m = intervene::sweep(&case.net, case.max_len, 0, units, word::encode);
            // 编译网络的 OR 层逐单元读数（第一档程序）。
            for (pname, program) in programs().iter().take(1) {
                let net = compile::compile(program, case.max_len).expect("编译");
                for step in 0..case.max_len {
                    let idx = 2 + 2 * step;
                    let cm = intervene::sweep(&net, case.max_len, idx, program.states, compile::encode3);
                    println!(
                        "    compiled {pname} step {step}: live={}/{}",
                        cm.live_units(),
                        cm.units.len()
                    );
                    for u in &cm.units {
                        println!(
                            "      state {} flips_zero={} flips_neg={}",
                            u.unit, u.flips_zero, u.flips_neg
                        );
                    }
                }
            }
            println!(
                "  intervene: units={} live={} words={}",
                m.units.len(),
                m.live_units(),
                m.words
            );
            for u in &m.units {
                println!(
                    "    unit {}: flips_zero={} flips_neg={}",
                    u.unit, u.flips_zero, u.flips_neg
                );
            }
        }
        println!("  {}", if r.pass() { "PASS" } else { "FAIL" });
        failed |= !r.pass();
    }
    if failed {
        std::process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn m0_case_passes() {
        for case in cases::all() {
            let r = run(&case, false);
            assert!(r.pass(), "案例 {} 未通过", case.name);
        }
    }

    #[test]
    fn reference_dfa_agrees_with_language() {
        for case in cases::all() {
            let dfa = dfa::from_accepts(case.max_len, case.lang).minimize();
            for w in word::all_words(case.max_len) {
                let bits: Vec<bool> = w.iter().map(|t| *t == Trit::Pos).collect();
                assert_eq!(
                    dfa.run(&bits),
                    (case.lang)(&w),
                    "串 {:?} 在参考 DFA 与语言定义下不一致",
                    word::bits(&w)
                );
            }
        }
    }

    #[test]
    fn minimize_is_idempotent() {
        let dfa = dfa::from_accepts(3, |w| w.iter().filter(|t| **t == Trit::Pos).count() >= 2);
        let once = dfa.minimize();
        let twice = once.minimize();
        assert_eq!(once.states, twice.states);
        assert_eq!(once.delta, twice.delta);
    }

    #[test]
    fn encode_shape_and_padding() {
        let w = [Trit::Pos, Trit::Neg];
        let enc = word::encode(&w, 3);
        assert_eq!(enc.len(), 6);
        // 位置 0 = 1，位置 1 = 0，位置 2 = 填充。
        assert_eq!(&enc[0..2], &[Trit::Pos, Trit::Neg]);
        assert_eq!(&enc[2..4], &[Trit::Zero, Trit::Neg]);
        assert_eq!(&enc[4..6], &[Trit::Zero, Trit::Pos]);
    }

    #[test]
    fn trace_hidden_determines_output() {
        for case in cases::all() {
            let dfa = dfa::from_accepts(case.max_len, case.lang).minimize();
            let a = trace::align(&case.net, case.max_len, &dfa);
            assert_eq!(
                a.hidden_to_out_conflicts, 0,
                "案例 {}：层 1 激活不足以决定输出",
                case.name
            );
        }
    }

    #[test]
    fn intervene_has_live_units() {
        for case in cases::all() {
            let units = case.net.layer(0).expect("有层 1").rows();
            let m = intervene::sweep(&case.net, case.max_len, 0, units, word::encode);
            assert!(m.live_units() > 0, "案例 {}：没有任何单元承重", case.name);
        }
    }


    #[test]
    fn compile_then_decompile() {
        for (name, program) in programs() {
            let net = compile::compile(&program, 3).expect("编译程序");
            // 当前编译器不处理「串结束」（End 会被当成符号继续执行转移），
            // 所以闭环只在定长域上比较：短串统一判 false，两侧同一口径。
            let extracted = dfa::from_accepts(3, |w| compile::run(&net, w, 3)).minimize();
            let reference = dfa::from_accepts(3, |w| {
                let bits: Vec<bool> = w.iter().map(|t| *t == Trit::Pos).collect();
                program.run(&bits)
            })
            .minimize();
            assert_eq!(extracted.states, reference.states, "{name}: 状态数不一致");
            assert_eq!(extracted.delta, reference.delta, "{name}: 转移表不一致");
            assert_eq!(extracted.accept, reference.accept, "{name}: 接受集不一致");
        }
    }

    #[test]
    fn compile_then_decompile_across_max_len() {
        for (name, program) in programs() {
            for max_len in 1..=4 {
                let net = compile::compile(&program, max_len).expect("编译");
                let extracted =
                    dfa::from_accepts(max_len, |w| compile::run(&net, w, max_len)).minimize();
                let reference = dfa::from_accepts(max_len, |w| {
                    let bits: Vec<bool> = w.iter().map(|t| *t == Trit::Pos).collect();
                    program.run(&bits)
                })
                .minimize();
                assert_eq!(
                    extracted.states, reference.states,
                    "{name}@{max_len}: 状态数不一致"
                );
                assert_eq!(
                    extracted.delta, reference.delta,
                    "{name}@{max_len}: 转移表不一致"
                );
                assert_eq!(
                    extracted.accept, reference.accept,
                    "{name}@{max_len}: 接受集不一致"
                );
            }
        }
    }

    #[test]
    fn end_padding_is_inert() {
        // End 自环的性质：同一串补不同长度的 End，判决必须一致。
        for (name, program) in programs() {
            let net4 = compile::compile(&program, 4).expect("编译 4");
            let net5 = compile::compile(&program, 5).expect("编译 5");
            for w in word::all_words(4) {
                let a = compile::run(&net4, &w, 4);
                let b = compile::run(&net5, &w, 5);
                assert_eq!(
                    a, b,
                    "{name}: 串 {:?} 在 4/5 位域判决不一致",
                    word::bits(&w)
                );
            }
        }
    }


    #[test]
    fn intervene_compiled_or_layer_states() {
        // 编译网络层结构：0 = 初始化；2 + 2*step = 第 step 步的 OR 层（状态位在前 k 个）；
        // 最后一层是输出。只对 OR 层的状态位做干预——层 0 是初始状态，每串相同，干预它没有意义。
        for (name, program) in programs() {
            let max_len = 3;
            let net = compile::compile(&program, max_len).expect("编译");
            for step in 0..max_len {
                let idx = 2 + 2 * step;
                let m = intervene::sweep(&net, max_len, idx, program.states, compile::encode3);
                assert!(
                    m.live_units() > 0,
                    "{name}: step {step}（层 {idx}）的状态位全不承重"
                );
            }
        }
    }

    #[test]
    fn cell_loop_matches_expanded() {
        // M3：同一 cell 权重重复用（真串行）与展开式前馈必须逐串等价。
        // >2 源的程序 cell 版不支持（OR 层没有常量单元的空间），跳过。
        for (name, program) in programs() {
            let Ok(cell) = compile::compile_cell(&program) else {
                continue;
            };
            let net = compile::compile(&program, 3).expect("展开");
            for w in word::all_words(3) {
                let r = recurrent::run(&cell, &program, &w, 3);
                let expanded = compile::run(&net, &w, 3);
                assert_eq!(
                    r.accept, expanded,
                    "{name}: 串 {:?} 循环与展开不一致",
                    word::bits(&w)
                );
                assert_eq!(r.trace.len(), 3, "{name}: 轨迹长度不对");
            }
        }
    }

}
