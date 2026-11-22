//! 重写闭环：重写后的文本必须通过**保真检查**，再送回同一质量门。

use std::collections::BTreeSet;

use crate::gate::{Config, Verdict, evaluate};
use crate::schema::Trajectory;

pub trait Rewriter {
    fn rewrite(&self, text: &str) -> Result<String, String>;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Fidelity {
    pub ok: bool,
    pub missing: Vec<String>,
    pub length_ratio_ok: bool,
}

/// 从原文提取必须保留的硬事实：数字、URL、反引号代码、引用编号。
pub fn anchors(text: &str) -> Vec<String> {
    let mut out: BTreeSet<String> = BTreeSet::new();
    let mut num = String::new();
    for c in text.chars() {
        if c.is_ascii_digit() || c == '.' {
            num.push(c);
        } else {
            if num.chars().filter(|x| x.is_ascii_digit()).count() >= 2 {
                out.insert(num.clone());
            }
            num.clear();
        }
    }
    if num.chars().filter(|x| x.is_ascii_digit()).count() >= 2 {
        out.insert(num);
    }
    for w in text.split_whitespace() {
        if w.starts_with("http://") || w.starts_with("https://") {
            out.insert(w.trim_end_matches([',', '。', ')', ']', '，']).to_string());
        }
    }
    let mut in_code = false;
    let mut code = String::new();
    for part in text.split('`') {
        if in_code {
            let p = part.trim();
            if !p.is_empty() {
                out.insert(p.to_string());
            }
        } else {
            code.push_str(part);
        }
        in_code = !in_code;
    }
    let _ = code;
    let bytes = text.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'[' {
            let mut j = i + 1;
            if j < bytes.len() && bytes[j] == b'^' {
                j += 1;
            }
            let start = j;
            while j < bytes.len() && bytes[j].is_ascii_digit() {
                j += 1;
            }
            if j > start && j < bytes.len() && bytes[j] == b']' {
                out.insert(text[i..=j].to_string());
            }
        }
        i += 1;
    }
    out.into_iter().collect()
}

pub fn fidelity(original: &str, rewritten: &str) -> Fidelity {
    let missing: Vec<String> = anchors(original)
        .into_iter()
        .filter(|a| !rewritten.contains(a))
        .collect();
    let a = original.chars().count().max(1) as f64;
    let b = rewritten.chars().count() as f64;
    let ratio = b / a;
    let length_ratio_ok = (0.5..=2.0).contains(&ratio);
    Fidelity {
        ok: missing.is_empty() && length_ratio_ok,
        missing,
        length_ratio_ok,
    }
}

/// 重写整条轨迹的文本字段（thought / observation / final），再跑同一质量门。
pub fn rewrite_trajectory(
    r: &dyn Rewriter,
    t: &Trajectory,
    cfg: &Config,
) -> Result<(Trajectory, Fidelity, Verdict), String> {
    let mut out = t.clone();
    let mut missing = Vec::new();
    let mut ratio_ok = true;
    for step in &mut out.steps {
        if let Some(text) = &step.thought {
            let new = r.rewrite(text)?;
            let f = fidelity(text, &new);
            ratio_ok &= f.length_ratio_ok;
            missing.extend(f.missing);
            step.thought = Some(new);
        }
        if let Some(text) = &step.observation {
            let new = r.rewrite(text)?;
            let f = fidelity(text, &new);
            ratio_ok &= f.length_ratio_ok;
            missing.extend(f.missing);
            step.observation = Some(new);
        }
    }
    if !out.outcome.final_text.is_empty() {
        let new = r.rewrite(&out.outcome.final_text)?;
        let f = fidelity(&t.outcome.final_text, &new);
        ratio_ok &= f.length_ratio_ok;
        missing.extend(f.missing);
        out.outcome.final_text = new;
    }
    let fid = Fidelity {
        ok: missing.is_empty() && ratio_ok,
        missing,
        length_ratio_ok: ratio_ok,
    };
    let mut seen = BTreeSet::new();
    let v = evaluate(&out, cfg, &mut seen);
    Ok((out, fid, v))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gate::Decision;
    use trit_learn::json::Json;

    struct Append;

    impl Rewriter for Append {
        fn rewrite(&self, text: &str) -> Result<String, String> {
            Ok(format!("{text}（改写）"))
        }
    }

    const OK: &str = r#"{"task_id":"t1","model":"m","steps":[{"role":"assistant","thought":"I will run the test suite now to check the build","action":{"tool":"bash","args":{"cmd":"pytest -q"}},"observation":"3 passed"},{"role":"assistant","thought":"All tests pass so I can finish this task","action":{"tool":"finish","args":{}},"observation":"done"}],"outcome":{"verifier":"pytest","passed":true,"evidence":["sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"],"final":"修复了 42 个用例"}}"#;

    fn td(s: &str) -> Trajectory {
        Trajectory::from_json(&Json::parse(s).unwrap()).unwrap()
    }

    #[test]
    fn anchors_extract_hard_facts() {
        let text = "运行 `pytest -q` 得到 42 passed，见 https://example.com/a 与 [1][^2]。";
        let a = anchors(text);
        assert!(a.iter().any(|x| x == "42"), "{a:?}");
        assert!(a.iter().any(|x| x == "https://example.com/a"), "{a:?}");
        assert!(a.iter().any(|x| x == "pytest -q"), "{a:?}");
        assert!(a.iter().any(|x| x == "[1]"), "{a:?}");
        assert!(a.iter().any(|x| x == "[^2]"), "{a:?}");
    }

    #[test]
    fn fidelity_detects_missing_anchor_and_truncation() {
        let original = "运行 `pytest -q` 得到 42 passed，见 https://example.com/a";
        assert!(fidelity(original, original).ok);
        let bad = fidelity(original, "完全不同的文本，没有保留任何硬事实");
        assert!(!bad.ok && !bad.missing.is_empty());
        let truncated = fidelity(original, "");
        assert!(!truncated.length_ratio_ok);
    }

    #[test]
    fn rewrite_then_gate_keeps_accept() {
        let t = td(OK);
        let (out, fid, v) = rewrite_trajectory(&Append, &t, &Config::default()).unwrap();
        assert!(fid.ok, "{:?}", fid.missing);
        assert_eq!(v.decision, Decision::Accept, "{:?}", v.reasons);
        assert!(out.outcome.final_text.contains("42"));
        assert!(out.steps[0].thought.as_deref().unwrap().contains("改写"));
    }
}
