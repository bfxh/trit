//! 生成三值语义的黄金语料（CSV）。
//!
//! 目的：让**独立实现**（另一语言、另一套代码）能对着仓库自己的语义做差分，
//! 而不是两边互相抄。用法：
//!
//! ```text
//! cargo run -p trit-codec --example ternary_corpus -- <out-dir>
//! ```
//!
//! 产出两个文件：
//! - `truth.csv`：`algebra,op,a,b,result`，覆盖 3 种代数 × 6 种算子 × 全部输入组合。
//! - `balanced.csv`：`value,text`，平衡三进制整数编解码的边界与抽样。
//!
//! 这两个文件是**语料**，不是格式契约；格式契约在 `pack.rs` 的模块文档里。

use std::fs;
use std::path::PathBuf;

use trit_codec::balanced;
use trit_core::algebra::Algebra;
use trit_core::value::Ternary;

type Bin = fn(Algebra, Ternary, Ternary) -> Ternary;

const ALGS: [(&str, Algebra); 3] = [
    ("kleene", Algebra::Kleene),
    ("lukasiewicz", Algebra::Lukasiewicz),
    ("bochvar", Algebra::Bochvar),
];

/// 五种二元算子；`equiv` 单独列因为它由 `implies` + `and` 复合而成。
const BIN_OPS: [(&str, Bin); 5] = [
    ("and", |g, a, b| g.and(a, b)),
    ("or", |g, a, b| g.or(a, b)),
    ("strong_and", |g, a, b| g.strong_and(a, b)),
    ("strong_add", |g, a, b| g.strong_add(a, b)),
    ("implies", |g, a, b| g.implies(a, b)),
];

fn sym(t: Ternary) -> char {
    match t {
        Ternary::False => 'F',
        Ternary::Unknown => 'X',
        Ternary::True => 'T',
    }
}

fn truth_csv() -> String {
    let all = [Ternary::False, Ternary::Unknown, Ternary::True];
    let mut out = String::from("algebra,op,a,b,result\n");
    for (name, alg) in ALGS {
        for a in all {
            out.push_str(&format!("{name},not,{},,{}\n", sym(a), sym(alg.not(a))));
        }
        for (op, f) in BIN_OPS {
            for a in all {
                for b in all {
                    out.push_str(&format!(
                        "{name},{op},{},{},{}\n",
                        sym(a),
                        sym(b),
                        sym(f(alg, a, b))
                    ));
                }
            }
        }
        for a in all {
            for b in all {
                out.push_str(&format!(
                    "{name},equiv,{},{},{}\n",
                    sym(a),
                    sym(b),
                    sym(alg.equiv(a, b))
                ));
            }
        }
    }
    out
}

fn balanced_csv() -> String {
    // 边界 + 进位密集 + 随机抽样
    let mut values: Vec<i64> = vec![
        0,
        1,
        -1,
        2,
        -2,
        3,
        -3,
        13,
        -13,
        40,
        -40,
        364,
        -364,
        29_524,
        -29_524,
        21_523_360,
        -21_523_360,
        1_000_000_007,
        -1_000_000_007,
        i64::MAX,
        i64::MIN,
    ];
    let mut s: u64 = 0x9e37_79b9_7f4a_7c15;
    for _ in 0..64 {
        s ^= s << 13;
        s ^= s >> 7;
        s ^= s << 17;
        values.push(s as i64);
    }
    let mut out = String::from("value,text\n");
    for v in values {
        out.push_str(&format!("{v},{}\n", balanced::encode(v)));
    }
    out
}

fn main() {
    let out_dir: PathBuf = std::env::args()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("target/ternary-corpus"));
    fs::create_dir_all(&out_dir).expect("创建输出目录");
    let truth = truth_csv();
    let bal = balanced_csv();
    fs::write(out_dir.join("truth.csv"), &truth).expect("写 truth.csv");
    fs::write(out_dir.join("balanced.csv"), &bal).expect("写 balanced.csv");
    println!(
        "wrote {} rows truth, {} rows balanced -> {}",
        truth.lines().count() - 1,
        bal.lines().count() - 1,
        out_dir.display()
    );
}
