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
mod dfa;
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
}

impl Report {
    fn pass(&self) -> bool {
        self.diff_net_vs_dfa == 0 && self.diff_dfa_vs_ref == 0 && self.states_net == self.states_ref
    }
}

/// 跑一个案例。
fn run(case: &cases::Case) -> Report {
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

    let mut report = Report {
        name: case.name,
        max_len,
        words: words.len(),
        accepted: 0,
        states_net: net_dfa.states,
        states_ref: ref_dfa.states,
        diff_net_vs_dfa: 0,
        diff_dfa_vs_ref: 0,
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

fn main() {
    let mut failed = false;
    for case in cases::all() {
        let r = run(&case);
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
            let r = run(&case);
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
}
