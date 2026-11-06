//! 直接对 CNF 打随机语料：`dpll::count_cnf` 的计数必须等于对 `2^nvars` 逐条枚举的结果。
//!
//! 这条链和 `crosscheck.rs` 不一样：那里从 `Expr` 出发，先经编码器再造 CNF，
//! 编码器一旦把公式写错就会**同时**骗到两边；这里跳过编码器，直接喂随机子句集，
//! 钉的是 DPLL 本体（规范形整理、单元传播、分量分解、缓存键）。

use trit_bench::SeedRng;
use trit_count::cnf::Cnf;
use trit_count::dpll::count_cnf;
use trit_fuzz::corpus::random_cnf;

/// 朴素枚举：对 `2^nvars` 个赋值逐条检查全部子句。
fn brute(clauses: &[Vec<i32>], nvars: usize) -> u128 {
    (0..1u128 << nvars)
        .filter(|&bits| {
            clauses.iter().all(|clause| {
                clause.iter().any(|&lit| {
                    let idx = lit.unsigned_abs() as usize - 1;
                    let value = (bits >> idx) & 1 == 1;
                    if lit > 0 { value } else { !value }
                })
            })
        })
        .count() as u128
}

#[test]
fn dpll_counts_random_cnfs_exactly() {
    let mut rng = SeedRng::new(0xC0FF_EE00_1234_5678);
    let mut mismatch = 0usize;
    let mut checked = 0usize;
    for nvars in 1..=9usize {
        for len in [1usize, 2, 3, 4] {
            for _ in 0..12 {
                let clauses = random_cnf(&mut rng, nvars, 2 * nvars, len);
                let formula = Cnf { nvars, clauses };
                let got = count_cnf(&formula).count;
                let want = brute(&formula.clauses, formula.nvars);
                checked += 1;
                if got != want {
                    mismatch += 1;
                    eprintln!(
                        "nvars={nvars} len={len} DPLL={got} 枚举={want} 子句={:?}",
                        formula.clauses
                    );
                }
            }
        }
    }
    assert_eq!(
        mismatch, 0,
        "{checked} 个随机 CNF 里有 {mismatch} 个计数不一致"
    );
    assert!(checked >= 400, "语料太少：{checked} 条");
}

/// 两端形状：空子句集 = 全部赋值都是解；含空子句 = 无解。
/// 这两个极端最容易被"顺手加个特判"改坏，单独钉住。
#[test]
fn empty_clause_sets_are_counted_at_their_extremes() {
    let all = Cnf {
        nvars: 5,
        clauses: Vec::new(),
    };
    assert_eq!(count_cnf(&all).count, 32, "空子句集应给 2^5 个解");
    assert_eq!(count_cnf(&all).count, brute(&all.clauses, all.nvars));

    let none = Cnf {
        nvars: 5,
        clauses: vec![vec![], vec![1, -2]],
    };
    assert_eq!(count_cnf(&none).count, 0, "有空子句就没有解");
    assert_eq!(count_cnf(&none).count, brute(&none.clauses, none.nvars));
}

/// 库里的穷举尺（`oracle::count_cnf_exhaustive`）必须和这份**故意另写**的朴素枚举逐例相同。
/// 两条链共用一把尺就等于没有复核：`--fuzz-cnf` 用的是库里那把，这里证明它没被写歪。
#[test]
fn library_ruler_agrees_with_the_independent_enumeration() {
    let mut rng = SeedRng::new(0xFEED_FACE_0000_0001);
    for nvars in 1..=10usize {
        for len in [2usize, 3, 5] {
            let clauses = random_cnf(&mut rng, nvars, 3 * nvars, len);
            let formula = Cnf { nvars, clauses };
            assert_eq!(
                trit_count::oracle::count_cnf_exhaustive(&formula),
                brute(&formula.clauses, formula.nvars),
                "nvars={nvars} len={len} 两把穷举尺不一致"
            );
        }
    }
}

/// CLI 面：`--fuzz-cnf` 必须自己报一致，而不是只在库测试里成立。
///
/// 找二进制的尺和 `trit-cli/tests/*` 一致：`CARGO_BIN_EXE_*` 在有些作业里不注入，
/// 回退成"让 PATH 去找裸名字"会报成产品故障（这轮在 MSRV 上刚踩过）。
#[test]
fn cli_fuzz_cnf_reports_agreement() {
    let bin = if let Ok(path) = std::env::var("CARGO_BIN_EXE_trit-count") {
        std::path::PathBuf::from(path)
    } else {
        let mut dir = std::env::current_exe().expect("current exe");
        dir.pop();
        dir.pop();
        dir.join(format!("trit-count{}", std::env::consts::EXE_SUFFIX))
    };
    assert!(bin.exists(), "找不到 trit-count 二进制：{}", bin.display());
    let out = std::process::Command::new(&bin)
        .args(["--fuzz-cnf", "--nvars", "8", "--cases", "2"])
        .output()
        .unwrap_or_else(|e| panic!("跑 {} 失败：{e}", bin.display()));
    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
    assert_eq!(
        out.status.code(),
        Some(0),
        "stderr={stderr} stdout={stdout}"
    );
    assert!(stdout.contains("逐一对账一致"), "没报一致：{stdout}");
    assert!(
        stdout.contains("16 个"),
        "nvars 1..=8 × 2 轮应是 16 例：{stdout}"
    );

    // 上限守卫：`--nvars 99` 会让穷举变成 2^99，必须拒了而不是开始等。
    let over = std::process::Command::new(&bin)
        .args(["--fuzz-cnf", "--nvars", "99"])
        .output()
        .expect("跑上限守卫");
    assert_eq!(over.status.code(), Some(2), "越界输入该用法错误码 2");
    let msg = String::from_utf8_lossy(&over.stderr).into_owned();
    assert!(msg.contains("上限取 40"), "报错要给出上限：{msg}");
}
