//! Conformance runner for the trit interop golden vectors.
//!
//! This is an independent implementation of `interop/spec/v0.md`; it does not
//! depend on the main `trit` Rust workspace. Usage:
//!
//! ```text
//! cd interop/runner
//! cargo run --release
//! cargo run --release -- --vectors ../vectors
//! cargo run --release -- --lean /tmp/lean-vectors.jsonl
//! ```

use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::PathBuf;

const SPEC_VERSION: &str = "0.1.0";
const FILES: [&str; 6] = [
    "k3.json",
    "k3_bitplane.json",
    "kgate.json",
    "pack5.json",
    "belnap4.json",
    "belnap4_bitplane.json",
];

fn main() {
    let (dir, lean_path) = parse_args();
    if let Some(path) = lean_path {
        let ok = check_lean(&dir, &path);
        std::process::exit(if ok { 0 } else { 1 });
    }
    let mut total = 0usize;
    let mut failed = 0usize;

    for file in FILES {
        let path = dir.join(file);
        let text = match fs::read_to_string(&path) {
            Ok(t) => t,
            Err(e) => {
                eprintln!("FAIL {file}: cannot read {}: {e}", path.display());
                failed += 1;
                continue;
            }
        };
        let root: Value = match serde_json::from_str(&text) {
            Ok(v) => v,
            Err(e) => {
                eprintln!("FAIL {file}: invalid JSON: {e}");
                failed += 1;
                continue;
            }
        };
        if root.get("spec_version").and_then(Value::as_str) != Some(SPEC_VERSION) {
            eprintln!("FAIL {file}: spec_version must be {SPEC_VERSION}");
            failed += 1;
            continue;
        }
        let profile = match root.get("profile").and_then(Value::as_str) {
            Some(p) => p,
            None => {
                eprintln!("FAIL {file}: missing profile");
                failed += 1;
                continue;
            }
        };
        let encoding = match root.get("encoding").and_then(Value::as_str) {
            Some(e) => e,
            None => {
                eprintln!("FAIL {file}: missing encoding");
                failed += 1;
                continue;
            }
        };
        let vectors = match root.get("vectors").and_then(Value::as_array) {
            Some(v) => v,
            None => {
                eprintln!("FAIL {file}: missing vectors array");
                failed += 1;
                continue;
            }
        };

        let mut passed = 0usize;
        for vector in vectors {
            total += 1;
            let id = vector
                .get("id")
                .and_then(Value::as_str)
                .unwrap_or("<missing-id>");
            match check_vector(profile, encoding, vector) {
                Ok(()) => passed += 1,
                Err(e) => {
                    failed += 1;
                    eprintln!("FAIL {id}: {e}");
                }
            }
        }
        println!("{file}: {passed}/{}", vectors.len());
    }

    println!("total: {}/{}", total.saturating_sub(failed), total);
    if failed > 0 {
        std::process::exit(1);
    }
}

fn parse_args() -> (PathBuf, Option<PathBuf>) {
    let mut args = std::env::args().skip(1);
    let mut dir = PathBuf::from("../vectors");
    let mut lean = None;
    while let Some(arg) = args.next() {
        if arg == "--vectors" {
            dir = PathBuf::from(args.next().expect("--vectors requires a directory"));
        } else if arg == "--lean" {
            lean = Some(PathBuf::from(args.next().expect("--lean requires a file")));
        } else {
            dir = PathBuf::from(arg);
        }
    }
    (dir, lean)
}

fn check_lean(dir: &PathBuf, lean_path: &PathBuf) -> bool {
    let text = match fs::read_to_string(lean_path) {
        Ok(t) => t,
        Err(e) => {
            eprintln!("lean bridge: cannot read {}: {e}", lean_path.display());
            return false;
        }
    };
    let mut lean: HashMap<String, Value> = HashMap::new();
    for (line_no, line) in text.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        let v: Value = match serde_json::from_str(line) {
            Ok(v) => v,
            Err(e) => {
                eprintln!("lean bridge: line {} invalid JSON: {e}", line_no + 1);
                return false;
            }
        };
        let id = match v.get("id").and_then(Value::as_str) {
            Some(id) => id.to_string(),
            None => {
                eprintln!("lean bridge: line {} missing id", line_no + 1);
                return false;
            }
        };
        if lean.insert(id.clone(), v).is_some() {
            eprintln!("lean bridge: duplicate id {id}");
            return false;
        }
    }

    let mut golden_ids = HashSet::new();
    let mut total = 0usize;
    let mut passed = 0usize;
    let mut failed = 0usize;

    for file in FILES {
        let path = dir.join(file);
        let text = match fs::read_to_string(&path) {
            Ok(t) => t,
            Err(e) => {
                eprintln!("lean bridge: cannot read {}: {e}", path.display());
                failed += 1;
                continue;
            }
        };
        let root: Value = match serde_json::from_str(&text) {
            Ok(v) => v,
            Err(e) => {
                eprintln!("lean bridge: {file} invalid JSON: {e}");
                failed += 1;
                continue;
            }
        };
        if root.get("spec_version").and_then(Value::as_str) != Some(SPEC_VERSION) {
            eprintln!("lean bridge: {file} spec_version mismatch");
            failed += 1;
            continue;
        }
        let vectors = match root.get("vectors").and_then(Value::as_array) {
            Some(v) => v,
            None => {
                eprintln!("lean bridge: {file} missing vectors");
                failed += 1;
                continue;
            }
        };
        for vector in vectors {
            total += 1;
            let id = vector
                .get("id")
                .and_then(Value::as_str)
                .unwrap_or("<missing-id>");
            golden_ids.insert(id.to_string());
            let line = match lean.get(id) {
                Some(v) => v,
                None => {
                    failed += 1;
                    eprintln!("FAIL lean missing {id}");
                    continue;
                }
            };
            let ok = match (vector.get("expected"), vector.get("expect_error")) {
                (Some(expected), None) => line
                    .get("expected")
                    .is_some_and(|actual| actual == expected),
                (None, Some(expected_error)) => {
                    let want = expected_error.as_str().unwrap_or("");
                    line.get("error").and_then(Value::as_str) == Some(want)
                }
                _ => false,
            };
            if ok {
                passed += 1;
            } else {
                failed += 1;
                eprintln!("FAIL lean {id}: golden={vector} lean={line}");
            }
        }
    }

    for id in lean.keys() {
        if !golden_ids.contains(id) {
            failed += 1;
            eprintln!("FAIL lean extra {id}");
        }
    }

    println!(
        "lean bridge: {passed}/{total} pass, {failed} fail, {} lean lines",
        lean.len()
    );
    failed == 0
}

fn check_vector(profile: &str, encoding: &str, v: &Value) -> Result<(), String> {
    let op = v
        .get("op")
        .and_then(Value::as_str)
        .ok_or_else(|| "missing op".to_string())?;
    let input = v.get("input").ok_or_else(|| "missing input".to_string())?;
    let result = run(profile, encoding, op, input, v);

    match (v.get("expected"), v.get("expect_error")) {
        (Some(expected), None) => match result {
            Ok(actual) if actual == *expected => Ok(()),
            Ok(actual) => Err(format!("expected {expected}, got {actual}")),
            Err(e) => Err(format!("unexpected error `{e}`; expected {expected}")),
        },
        (None, Some(expected_error)) => {
            let want = expected_error
                .as_str()
                .ok_or_else(|| "expect_error must be a string".to_string())?;
            match result {
                Err(e) if e == want => Ok(()),
                Err(e) => Err(format!("expected error `{want}`, got `{e}`")),
                Ok(actual) => Err(format!("expected error `{want}`, got value {actual}")),
            }
        }
        _ => Err("vector must have exactly one of expected/expect_error".to_string()),
    }
}

fn run(profile: &str, encoding: &str, op: &str, input: &Value, v: &Value) -> Result<Value, String> {
    match profile {
        "k3" => run_k3(encoding, op, input),
        "kgate" => run_kgate(op, input),
        "pack5" => run_pack5(op, input, v),
        "belnap4" => run_belnap4(encoding, op, input),
        _ => Err(format!("unknown profile `{profile}`")),
    }
}

fn parse_k3(v: &Value) -> Result<char, String> {
    match v.as_str() {
        Some("F") => Ok('F'),
        Some("U") => Ok('U'),
        Some("T") => Ok('T'),
        _ => Err(format!("invalid k3 value {v}")),
    }
}

fn k3_not(c: char) -> char {
    match c {
        'F' => 'T',
        'U' => 'U',
        'T' => 'F',
        _ => unreachable!(),
    }
}

fn k3_and(a: char, b: char) -> char {
    match (a, b) {
        ('F', _) | (_, 'F') => 'F',
        ('T', 'T') => 'T',
        _ => 'U',
    }
}

fn k3_or(a: char, b: char) -> char {
    match (a, b) {
        ('T', _) | (_, 'T') => 'T',
        ('F', 'F') => 'F',
        _ => 'U',
    }
}

fn k3_bits(c: char) -> [i64; 2] {
    match c {
        'F' => [0, 0],
        'T' => [1, 0],
        'U' => [0, 1],
        _ => unreachable!(),
    }
}

fn k3_from_bits(bits: &[i64]) -> Result<char, String> {
    match bits {
        [0, 0] => Ok('F'),
        [1, 0] => Ok('T'),
        [0, 1] => Ok('U'),
        [1, 1] => Err("invalid_bitplane".to_string()),
        _ => Err(format!("invalid bitplane {bits:?}")),
    }
}

fn run_k3(encoding: &str, op: &str, input: &Value) -> Result<Value, String> {
    match (encoding, op) {
        ("semantic", "not") => Ok(json!(k3_not(parse_k3(input)?).to_string())),
        ("semantic", "and") | ("semantic", "or") => {
            let arr = input
                .as_array()
                .ok_or_else(|| "and/or input must be [a,b]".to_string())?;
            if arr.len() != 2 {
                return Err("and/or input must have 2 values".to_string());
            }
            let a = parse_k3(&arr[0])?;
            let b = parse_k3(&arr[1])?;
            let out = if op == "and" {
                k3_and(a, b)
            } else {
                k3_or(a, b)
            };
            Ok(json!(out.to_string()))
        }
        ("bitplane", "encode_bitplane") => {
            let c = parse_k3(input)?;
            let [o, u] = k3_bits(c);
            Ok(json!([o, u]))
        }
        ("bitplane", "decode_bitplane") => {
            let arr = input
                .as_array()
                .ok_or_else(|| "decode_bitplane input must be [ones,unknown]".to_string())?;
            let bits = arr
                .iter()
                .map(|v| v.as_i64().ok_or_else(|| format!("invalid bit {v}")))
                .collect::<Result<Vec<_>, _>>()?;
            Ok(json!(k3_from_bits(&bits)?.to_string()))
        }
        _ => Err(format!("unknown k3 op `{op}` (encoding `{encoding}`)")),
    }
}

fn run_kgate(op: &str, input: &Value) -> Result<Value, String> {
    if op != "gate" {
        return Err(format!("unknown kgate op `{op}`"));
    }
    let arr = input
        .as_array()
        .ok_or_else(|| "gate input must be [required,verified]".to_string())?;
    if arr.len() != 2 {
        return Err("gate input must have 2 values".to_string());
    }
    let required = parse_k3(&arr[0])?;
    let verified = parse_k3(&arr[1])?;
    let out = match required {
        'F' => 'T',
        'U' => 'U',
        'T' => verified,
        _ => unreachable!(),
    };
    Ok(json!(out.to_string()))
}

fn parse_belnap4(v: &Value) -> Result<(i64, i64), String> {
    match v.as_str() {
        Some("N") => Ok((0, 0)),
        Some("T") => Ok((1, 0)),
        Some("F") => Ok((0, 1)),
        Some("B") => Ok((1, 1)),
        _ => Err(format!("invalid belnap4 value {v}")),
    }
}

fn belnap4_from_bits(t: i64, f: i64) -> Result<&'static str, String> {
    match (t, f) {
        (0, 0) => Ok("N"),
        (1, 0) => Ok("T"),
        (0, 1) => Ok("F"),
        (1, 1) => Ok("B"),
        _ => Err(format!("invalid belnap4 bitplane ({t},{f})")),
    }
}

fn run_belnap4(encoding: &str, op: &str, input: &Value) -> Result<Value, String> {
    match (encoding, op) {
        ("semantic", "not") => {
            let (t, f) = parse_belnap4(input)?;
            Ok(json!(belnap4_from_bits(f, t)?))
        }
        ("semantic", "and") | ("semantic", "or") => {
            let arr = input
                .as_array()
                .ok_or_else(|| "and/or input must be [a,b]".to_string())?;
            if arr.len() != 2 {
                return Err("and/or input must have 2 values".to_string());
            }
            let (ta, fa) = parse_belnap4(&arr[0])?;
            let (tb, fb) = parse_belnap4(&arr[1])?;
            let (t, f) = if op == "and" {
                (ta & tb, fa | fb)
            } else {
                (ta | tb, fa & fb)
            };
            Ok(json!(belnap4_from_bits(t, f)?))
        }
        ("bitplane", "encode_bitplane") => {
            let (t, f) = parse_belnap4(input)?;
            Ok(json!([t, f]))
        }
        ("bitplane", "decode_bitplane") => {
            let arr = input
                .as_array()
                .ok_or_else(|| "decode_bitplane input must be [truth,falsity]".to_string())?;
            let bits = arr
                .iter()
                .map(|v| v.as_i64().ok_or_else(|| format!("invalid bit {v}")))
                .collect::<Result<Vec<_>, _>>()?;
            if bits.len() != 2 {
                return Err("decode_bitplane input must have 2 bits".to_string());
            }
            Ok(json!(belnap4_from_bits(bits[0], bits[1])?))
        }
        _ => Err(format!("unknown belnap4 op `{op}` (encoding `{encoding}`)")),
    }
}

fn run_pack5(op: &str, input: &Value, v: &Value) -> Result<Value, String> {
    match op {
        "pack" => {
            let arr = input
                .as_array()
                .ok_or_else(|| "pack input must be an array of trits".to_string())?;
            let trits = arr
                .iter()
                .map(|v| v.as_i64().ok_or_else(|| format!("invalid trit {v}")))
                .collect::<Result<Vec<_>, _>>()?;
            for t in &trits {
                if !(-1..=1).contains(t) {
                    return Err("invalid_trit".to_string());
                }
            }
            let groups = trits.len().div_ceil(5);
            let mut out = Vec::with_capacity(groups);
            for g in 0..groups {
                let mut byte = 0u32;
                let mut pow = 1u32;
                for j in 0..5 {
                    let t = trits.get(g * 5 + j).copied().unwrap_or(0);
                    byte += ((t + 1) as u32) * pow;
                    pow *= 3;
                }
                out.push(byte as u8);
            }
            if out.len() == 1 {
                Ok(json!(out[0]))
            } else {
                Ok(json!(out))
            }
        }
        "unpack" => {
            let bytes = match input {
                Value::Number(n) => {
                    let b = n.as_u64().ok_or_else(|| format!("invalid byte {n}"))?;
                    vec![b]
                }
                Value::Array(arr) => arr
                    .iter()
                    .map(|v| v.as_u64().ok_or_else(|| format!("invalid byte {v}")))
                    .collect::<Result<Vec<_>, _>>()?,
                _ => return Err("unpack input must be a byte or an array of bytes".to_string()),
            };
            let count = v
                .get("trit_count")
                .and_then(Value::as_u64)
                .ok_or_else(|| "unpack requires trit_count".to_string())?
                as usize;
            for b in &bytes {
                if *b >= 243 {
                    return Err("invalid_byte".to_string());
                }
            }
            if count > bytes.len() * 5 {
                return Err("short_input".to_string());
            }
            let mut out = Vec::with_capacity(count);
            for i in 0..count {
                let b = bytes[i / 5];
                let mut v = b;
                for _ in 0..(i % 5) {
                    v /= 3;
                }
                out.push((v % 3) as i64 - 1);
            }
            Ok(json!(out))
        }
        _ => Err(format!("unknown pack5 op `{op}`")),
    }
}
