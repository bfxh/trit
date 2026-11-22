//! `--corpus`：吃 `ternary_corpus` 例子的 `truth.csv` / `balanced.csv`，
//! 用独立实现复核，把「另一语言实现可做差分」这条线接上消费者。
//!
//! 只认生成器写出的固定表头；行数/键集合要对得上，少了、重了都算失败。

use std::collections::BTreeSet;
use std::fs;
use std::path::Path;

use trit_codec::balanced;

use super::checks::oracle_balanced;
use super::report::{Check, Report};

pub fn run(report: &mut Report, dir: &Path) -> Result<(), String> {
    truth(report, &dir.join("truth.csv"))?;
    balanced_rows(report, &dir.join("balanced.csv"))?;
    report.metric("corpus.path", dir.display());
    Ok(())
}

/// 三值语义的独立代算：只用 f64 分数（F=0 / X=0.5 / T=1），不调库里的代数。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum V {
    F,
    X,
    T,
}

fn parse_v(raw: &str) -> Option<V> {
    match raw {
        "F" => Some(V::F),
        "X" => Some(V::X),
        "T" => Some(V::T),
        _ => None,
    }
}

fn score(v: V) -> f64 {
    match v {
        V::F => 0.0,
        V::X => 0.5,
        V::T => 1.0,
    }
}

fn from_score(s: f64) -> V {
    if s >= 1.0 {
        V::T
    } else if s <= 0.0 {
        V::F
    } else {
        V::X
    }
}

fn poison(b3: bool, a: V, b: V) -> bool {
    b3 && (a == V::X || b == V::X)
}

fn and(b3: bool, a: V, b: V) -> V {
    if poison(b3, a, b) {
        V::X
    } else {
        from_score(score(a).min(score(b)))
    }
}

fn or(b3: bool, a: V, b: V) -> V {
    if poison(b3, a, b) {
        V::X
    } else {
        from_score(score(a).max(score(b)))
    }
}

fn not(a: V) -> V {
    from_score(1.0 - score(a))
}

fn implies(b3: bool, l3: bool, a: V, b: V) -> V {
    if poison(b3, a, b) {
        V::X
    } else if l3 {
        from_score((1.0 - score(a) + score(b)).min(1.0))
    } else if a == V::F || b == V::T {
        V::T
    } else if a == V::T {
        b
    } else {
        V::X
    }
}

fn strong_add(_b3: bool, l3: bool, a: V, b: V) -> V {
    // K3 与 B3 都让 X 传播；只有 Ł3 走算术。
    if !l3 && (a == V::X || b == V::X) {
        V::X
    } else if l3 {
        from_score((score(a) + score(b)).min(1.0))
    } else if a == V::T && b == V::T {
        V::T
    } else {
        V::F
    }
}

fn strong_and(_b3: bool, l3: bool, a: V, b: V) -> V {
    if !l3 && (a == V::X || b == V::X) {
        V::X
    } else if l3 {
        from_score((score(a) + score(b) - 1.0).max(0.0))
    } else if a == V::T && b == V::T {
        V::T
    } else {
        V::F
    }
}

fn truth_row(algebra: &str, op: &str, a: V, b: Option<V>) -> Option<V> {
    let (b3, l3) = match algebra {
        "kleene" => (false, false),
        "lukasiewicz" => (false, true),
        "bochvar" => (true, false),
        _ => return None,
    };
    let b = b.unwrap_or(V::X);
    Some(match op {
        "not" => not(a),
        "and" => and(b3, a, b),
        "or" => or(b3, a, b),
        "strong_and" => strong_and(b3, l3, a, b),
        "strong_add" => strong_add(b3, l3, a, b),
        "implies" => implies(b3, l3, a, b),
        "equiv" => and(b3, implies(b3, l3, a, b), implies(b3, l3, b, a)),
        _ => return None,
    })
}

fn truth(report: &mut Report, path: &Path) -> Result<(), String> {
    let text = read(path)?;
    let mut lines = text.lines();
    if lines.next() != Some("algebra,op,a,b,result") {
        return Err(format!(
            "{}: 表头不是 algebra,op,a,b,result",
            path.display()
        ));
    }
    let mut check = Check::new("external.truth_corpus");
    let mut complete = Check::new("external.truth_complete");
    let mut seen = BTreeSet::new();
    let mut rows = 0u64;
    for line in lines.filter(|line| !line.trim().is_empty()) {
        rows += 1;
        let fields: Vec<&str> = line.split(',').collect();
        let [algebra, op, a, b, result] = fields.as_slice() else {
            return Err(format!("{}: 列数不是 5：{line}", path.display()));
        };
        let a_val = match parse_v(a) {
            Some(v) => v,
            None => return Err(format!("{}: 非 F/X/T 符号：{line}", path.display())),
        };
        let b_opt = if b.is_empty() {
            None
        } else {
            match parse_v(b) {
                Some(v) => Some(v),
                None => return Err(format!("{}: 非 F/X/T 符号：{line}", path.display())),
            }
        };
        let Some(exp) = parse_v(result) else {
            return Err(format!("{}: 非 F/X/T 符号：{line}", path.display()));
        };
        let independent = truth_row(algebra, op, a_val, b_opt)
            .ok_or_else(|| format!("{}: 未知代数/算子：{line}", path.display()))?;
        check.case(independent == exp, || {
            format!("{line} -> 独立实现 {:?}", independent)
        });
        let key = format!("{algebra},{op},{a},{b}");
        complete.case(seen.insert(key), || format!("重复行：{line}"));
    }
    // 3 代数 × (3 否定 + 5×9 二元 + 9 等价) = 171 个键。
    let expected = 3 * (3 + 45 + 9);
    complete.case(seen.len() == expected, || {
        format!("期望 {expected} 个键，实际 {}", seen.len())
    });
    report.finish(check);
    report.finish(complete);
    report.metric("corpus.truth_rows", rows);
    Ok(())
}

fn balanced_rows(report: &mut Report, path: &Path) -> Result<(), String> {
    let text = read(path)?;
    let mut lines = text.lines();
    if lines.next() != Some("value,text") {
        return Err(format!("{}: 表头不是 value,text", path.display()));
    }
    let mut check = Check::new("external.balanced_corpus");
    let mut rows = 0u64;
    for line in lines.filter(|line| !line.trim().is_empty()) {
        rows += 1;
        let (value, encoded) = line
            .split_once(',')
            .ok_or_else(|| format!("{}: 列数不是 2：{line}", path.display()))?;
        let value: i64 = value
            .parse()
            .map_err(|_| format!("{}: value 不是 i64：{line}", path.display()))?;
        let oracle_ok = oracle_balanced(encoded) == Some(i128::from(value));
        let lib_ok = balanced::decode(encoded) == Ok(value);
        let encode_ok = balanced::encode(value) == encoded;
        check.case(oracle_ok && lib_ok && encode_ok, || {
            format!("{line} -> 独立解析={oracle_ok} 库解码={lib_ok} 库编码={encode_ok}")
        });
    }
    report.finish(check);
    report.metric("corpus.balanced_rows", rows);
    Ok(())
}

fn read(path: &Path) -> Result<String, String> {
    fs::read_to_string(path).map_err(|e| format!("读不了 {}: {e}", path.display()))
}
