//! 作业文件解析（扁平 JSON → 命令行参数）。
//!
//! 设计要点：**未知键直接报错**，并给出最近邻建议（`--sity-by-material` → 你是不是想写 `style-by-material`）。
//! 作业文件是给人手写的，拼错一个键被静默忽略比报错危险得多。

/// 解析扁平 JSON（`{"key": 值, ...}`），`allowed` 是允许的键名（不含 `--`）。
///
/// 值为字符串/数字/布尔：布尔 `true` 展开成单开关、`false` 跳过；字符串可含空格。
/// 以 `_` 开头的键被当作注释跳过。
pub fn parse_job(txt: &str, allowed: &[&str]) -> Result<Vec<String>, String> {
    let b: Vec<char> = txt.chars().collect();
    let mut out: Vec<String> = Vec::new();
    let mut i = 0usize;
    while i < b.len() {
        if b[i] != '"' {
            i += 1;
            continue;
        }
        let ks = i + 1;
        let mut ke = ks;
        while ke < b.len() && b[ke] != '"' {
            ke += 1;
        }
        let key: String = b[ks..ke.min(b.len())].iter().collect();
        i = (ke + 1).min(b.len());
        while i < b.len() && b[i] != ':' {
            i += 1;
        }
        i += 1;
        while i < b.len() && b[i].is_whitespace() {
            i += 1;
        }
        if i >= b.len() {
            break;
        }
        let (val, is_str) = if b[i] == '"' {
            i += 1;
            let mut v = String::new();
            while i < b.len() && b[i] != '"' {
                v.push(b[i]);
                i += 1;
            }
            i += 1;
            (v, true)
        } else {
            let mut v = String::new();
            while i < b.len() && b[i] != ',' && b[i] != '}' && b[i] != '\n' {
                v.push(b[i]);
                i += 1;
            }
            (v.trim().to_string(), false)
        };
        if key.starts_with('_') || key.is_empty() {
            continue;
        }
        if !allowed.contains(&key.as_str()) {
            return Err(match nearest(&key, allowed) {
                Some(n) => format!("未知键 {key:?}（你是不是想写 {n:?}？）"),
                None => format!("未知键 {key:?}（可用键：{}）", allowed.join(", ")),
            });
        }
        if !is_str && (val == "true" || val == "false") {
            if val == "true" {
                out.push(format!("--{key}"));
            }
            continue;
        }
        if val.is_empty() {
            continue;
        }
        out.push(format!("--{key}"));
        out.push(val);
    }
    Ok(out)
}

/// 最近邻（编辑距离最小，且距离 ≤ 3）。平手时取字典序最小的，保证确定性。
fn nearest<'a>(key: &str, allowed: &[&'a str]) -> Option<&'a str> {
    let mut best: Option<(usize, &str)> = None;
    for a in allowed {
        let d = levenshtein(key, a);
        if d > 3 {
            continue;
        }
        match best {
            Some((bd, bk)) if d > bd || (d == bd && *a > bk) => {}
            _ => best = Some((d, a)),
        }
    }
    best.map(|(_, a)| a)
}

fn levenshtein(a: &str, b: &str) -> usize {
    let (a, b): (Vec<char>, Vec<char>) = (a.chars().collect(), b.chars().collect());
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    let mut cur = vec![0usize; b.len() + 1];
    for i in 1..=a.len() {
        cur[0] = i;
        for j in 1..=b.len() {
            let cost = if a[i - 1] == b[j - 1] { 0 } else { 1 };
            cur[j] = (prev[j] + 1).min(cur[j - 1] + 1).min(prev[j - 1] + cost);
        }
        std::mem::swap(&mut prev, &mut cur);
    }
    prev[b.len()]
}

#[cfg(test)]
mod tests {
    use super::*;

    const OK: [&str; 5] = ["scene", "frames", "clean", "atlas", "style-by-material"];

    #[test]
    fn parses_strings_numbers_and_bools() {
        let txt = r#"{"scene": "E:/a b/c", "frames": "0,1", "clean": true, "atlas": 512}"#;
        let v = parse_job(txt, &OK).unwrap();
        assert_eq!(
            v,
            vec![
                "--scene", "E:/a b/c", "--frames", "0,1", "--clean", "--atlas", "512"
            ]
        );
    }

    #[test]
    fn false_bool_is_skipped() {
        let v = parse_job(r#"{"clean": false, "atlas": 256}"#, &OK).unwrap();
        assert_eq!(v, vec!["--atlas", "256"]);
    }

    #[test]
    fn unknown_key_fails_with_suggestion() {
        let e = parse_job(r#"{"sity-by-material": "wood=cel:1"}"#, &OK).unwrap_err();
        assert!(e.contains("未知键"), "错误应点名：{e}");
        assert!(
            e.contains("style-by-material"),
            "应给出最近邻建议，实际：{e}"
        );
    }

    #[test]
    fn unknown_key_without_close_match_lists_allowed() {
        let e = parse_job(r#"{"zzzzzzzz": 1}"#, &OK).unwrap_err();
        assert!(e.contains("scene") && e.contains("可用键"), "实际：{e}");
    }

    #[test]
    fn underscore_keys_are_comments() {
        let v = parse_job(r#"{"_note": "hello", "atlas": 512}"#, &OK).unwrap();
        assert_eq!(v, vec!["--atlas", "512"]);
    }

    #[test]
    fn nearest_is_deterministic_on_ties() {
        // 两个候选距离相同时取字典序小的
        assert_eq!(nearest("clen", &["clean", "cleam"]), Some("clean"));
    }
}
