//! `trit-codec`：三值编解码的最小 CLI 与读数入口。

use std::process::ExitCode;

use trit_codec::{
    balanced, checksum, count, delta, gray, hamming, interleave, pack, plan, quantize, rle, sop,
    sparse,
};
use trit_core::value::Ternary;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let result = match args.first().map(String::as_str) {
        Some("pack") => pack_cmd(&args[1..]),
        Some("unpack") => unpack_cmd(&args[1..]),
        Some("balanced") => balanced_cmd(&args[1..]),
        Some("hamming") => hamming_cmd(&args[1..]),
        Some("stats") => stats_cmd(&args[1..]),
        Some("plan") => plan_cmd(&args[1..]),
        Some("quantize") => quantize_cmd(&args[1..]),
        Some("dequantize") => dequantize_cmd(&args[1..]),
        Some("sparse") => sparse_cmd(&args[1..]),
        Some("interleave") => interleave_cmd(&args[1..]),
        Some("checksum") => checksum_cmd(&args[1..]),
        Some("rle") => rle_cmd(&args[1..]),
        Some("delta") => delta_cmd(&args[1..]),
        Some("gray") => gray_cmd(&args[1..]),
        Some("sop") => sop_cmd(&args[1..]),
        Some("count") => count_cmd(&args[1..]),
        _ => Err(usage()),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("{message}");
            ExitCode::FAILURE
        }
    }
}

fn usage() -> String {
    "用法：trit-codec <pack|unpack|balanced|hamming|stats|plan|quantize|dequantize|sparse|interleave|checksum|rle|delta|gray|sop|count>"
        .to_string()
}

fn parse_trits(raw: &str) -> Result<Vec<Ternary>, String> {
    raw.chars()
        .map(|ch| {
            Ternary::parse_char(ch).ok_or_else(|| format!("未知 trit {ch:?}（只接受 0/1/X）"))
        })
        .collect()
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn from_hex(raw: &str) -> Result<Vec<u8>, String> {
    if !raw.len().is_multiple_of(2) {
        return Err(format!("十六进制长度必须是偶数: {raw}"));
    }
    (0..raw.len() / 2)
        .map(|index| {
            u8::from_str_radix(&raw[index * 2..index * 2 + 2], 16)
                .map_err(|_| format!("非法十六进制: {raw}"))
        })
        .collect()
}

fn pack_cmd(args: &[String]) -> Result<(), String> {
    let raw = args.first().ok_or("pack 需要 TRITS 参数")?;
    let values = parse_trits(raw)?;
    let bytes = pack::pack(&values);
    let obj = serde_json::json!({
        "trits": values.len(),
        "bytes": hex(&bytes),
        "packed_len": bytes.len(),
        "dual_rail_len": pack::dual_rail_bytes(values.len()),
        "text_len": values.len(),
        "bits_per_trit": if values.is_empty() { 0.0 } else { bytes.len() as f64 * 8.0 / values.len() as f64 },
    });
    println!("{}", serde_json::to_string_pretty(&obj).unwrap_or_default());
    Ok(())
}

fn unpack_cmd(args: &[String]) -> Result<(), String> {
    let raw = args.first().ok_or("unpack 需要 HEX 参数")?;
    let len = args
        .iter()
        .position(|arg| arg == "--len")
        .and_then(|index| args.get(index + 1))
        .ok_or("unpack 需要 --len N")?
        .parse::<usize>()
        .map_err(|_| "长度不是整数".to_string())?;
    let values = pack::unpack(&from_hex(raw)?, len).map_err(|error| error.to_string())?;
    let text: String = values.iter().map(|value| value.as_char()).collect();
    println!(
        "{}",
        serde_json::to_string_pretty(&serde_json::json!({"trits": text, "len": values.len()}))
            .unwrap_or_default()
    );
    Ok(())
}

fn balanced_cmd(args: &[String]) -> Result<(), String> {
    let raw = args.first().ok_or("balanced 需要整数")?;
    let value = raw.parse::<i64>().map_err(|_| format!("不是 i64: {raw}"))?;
    let text = balanced::encode(value);
    println!(
        "{}",
        serde_json::to_string_pretty(&serde_json::json!({
            "value": value,
            "balanced": text,
            "digits": text.chars().count(),
        }))
        .unwrap_or_default()
    );
    Ok(())
}

fn hamming_cmd(args: &[String]) -> Result<(), String> {
    if let Some(index) = args.iter().position(|arg| arg == "--decode") {
        let raw = args.get(index + 1).ok_or("--decode 需要 4 个 trit")?;
        let values = parse_trits(raw)?;
        let word: [Ternary; 4] = values
            .as_slice()
            .try_into()
            .map_err(|_| format!("码字必须是 4 个 trit: {raw}"))?;
        let decoded = hamming::decode(word);
        let text: String = decoded
            .message
            .iter()
            .map(|value| value.as_char())
            .collect();
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "word": raw,
                "message": text,
                "corrected": decoded.corrected,
            }))
            .unwrap_or_default()
        );
        return Ok(());
    }
    let raw = args
        .first()
        .ok_or("hamming 需要 2 个消息 trit 或 --decode")?;
    let values = parse_trits(raw)?;
    let [a, b]: [Ternary; 2] = values
        .as_slice()
        .try_into()
        .map_err(|_| format!("消息必须是 2 个 trit: {raw}"))?;
    let word = hamming::encode(a, b);
    let text: String = word.iter().map(|value| value.as_char()).collect();
    println!(
        "{}",
        serde_json::to_string_pretty(&serde_json::json!({
            "message": raw,
            "codeword": text,
            "distance": hamming::DISTANCE,
        }))
        .unwrap_or_default()
    );
    Ok(())
}

fn stats_cmd(args: &[String]) -> Result<(), String> {
    let trits = args
        .iter()
        .position(|arg| arg == "--trits")
        .and_then(|index| args.get(index + 1))
        .map_or(Ok(1000), |raw| {
            raw.parse::<usize>().map_err(|_| "不是整数".to_string())
        })?;
    let values = vec![Ternary::Unknown; trits];
    let packed = pack::pack(&values).len();
    let dual = pack::dual_rail_bytes(trits);
    let mut balanced_max_digits = 0;
    for value in [i64::MIN, i64::MAX] {
        balanced_max_digits = balanced_max_digits.max(balanced::encode(value).chars().count());
    }
    let obj = serde_json::json!({
        "trits": trits,
        "packed_bytes": packed,
        "dual_rail_bytes": dual,
        "text_bytes": trits,
        "packed_bits_per_trit": packed as f64 * 8.0 / trits as f64,
        "saving_vs_dual_rail": 1.0 - packed as f64 / dual as f64,
        "balanced_max_digits_i64": balanced_max_digits,
        "hamming": {
            "message": hamming::MESSAGE,
            "codeword": hamming::CODEWORD,
            "distance": hamming::DISTANCE,
            "single_error_cases": 9 * 4 * 2,
        },
    });
    println!("{}", serde_json::to_string_pretty(&obj).unwrap_or_default());
    Ok(())
}

fn plan_cmd(args: &[String]) -> Result<(), String> {
    let trits = args
        .iter()
        .position(|arg| arg == "--trits")
        .and_then(|index| args.get(index + 1))
        .map_or(Ok(4096usize), |raw| {
            raw.parse().map_err(|_| "不是整数".to_string())
        })?;
    fn binary(i: usize) -> Ternary {
        if i.is_multiple_of(3) {
            Ternary::True
        } else {
            Ternary::False
        }
    }
    fn ternary(i: usize) -> Ternary {
        match i % 3 {
            0 => Ternary::False,
            1 => Ternary::Unknown,
            _ => Ternary::True,
        }
    }
    fn sparse_1(i: usize) -> Ternary {
        if i.is_multiple_of(100) {
            Ternary::True
        } else {
            Ternary::Unknown
        }
    }
    fn sparse_4(i: usize) -> Ternary {
        if i.is_multiple_of(25) {
            Ternary::True
        } else {
            Ternary::Unknown
        }
    }
    fn sparse_10(i: usize) -> Ternary {
        if i.is_multiple_of(10) {
            Ternary::True
        } else {
            Ternary::Unknown
        }
    }
    type Distribution = (&'static str, fn(usize) -> Ternary);
    let distributions: [Distribution; 5] = [
        ("binary", binary),
        ("ternary-1/3-X", ternary),
        ("sparse-1%", sparse_1),
        ("sparse-4%", sparse_4),
        ("sparse-10%", sparse_10),
    ];
    let mut rows = Vec::new();
    for (name, make) in distributions {
        let values: Vec<Ternary> = (0..trits).map(make).collect();
        let chosen = plan::plan(&values);
        let (encoding, bytes) = plan::encode(&values);
        // 零拷贝的收益在「只取少数元素」：对比「整条解码 + 随机取」与「视图随机取」。
        let samples = 200usize;
        let mut state = 0x9E37_79B9_7F4A_7C15u64;
        let indices: Vec<usize> = (0..samples)
            .map(|_| {
                state ^= state << 13;
                state ^= state >> 7;
                state ^= state << 17;
                (state as usize) % values.len().max(1)
            })
            .collect();
        let iterations = 20;
        let start = std::time::Instant::now();
        let mut decoded_hits = 0usize;
        for _ in 0..iterations {
            let decoded = plan::decode(encoding, &bytes, values.len()).unwrap();
            for index in &indices {
                if std::hint::black_box(decoded[*index]) != Ternary::Unknown {
                    decoded_hits += 1;
                }
            }
        }
        let decode_ns = start.elapsed().as_nanos() as f64 / iterations as f64;
        let start = std::time::Instant::now();
        let mut view_hits = 0usize;
        for _ in 0..iterations {
            let view = plan::View::new(encoding, &bytes, values.len()).unwrap();
            for index in &indices {
                if std::hint::black_box(view.get(*index).unwrap_or(Ternary::Unknown))
                    != Ternary::Unknown
                {
                    view_hits += 1;
                }
            }
        }
        let view_ns = start.elapsed().as_nanos() as f64 / iterations as f64;
        debug_assert_eq!(decoded_hits, view_hits);
        rows.push(serde_json::json!({
            "decode_ns": decode_ns,
            "view_ns": view_ns,
            "view_speedup": decode_ns / view_ns,
            "samples": samples,
            "distribution": name,
            "encoding": chosen.encoding.as_str(),
            "bytes": chosen.bytes,
            "dual_rail_bytes": chosen.dual_rail_bytes,
            "bits_per_trit": chosen.bits_per_trit,
            "saving_vs_dual_rail": 1.0 - chosen.bytes as f64 / chosen.dual_rail_bytes as f64,
        }));
    }
    let obj = serde_json::json!({"trits": trits, "rows": rows});
    println!("{}", serde_json::to_string_pretty(&obj).unwrap_or_default());
    Ok(())
}

fn quantize_cmd(args: &[String]) -> Result<(), String> {
    let raw = args.first().ok_or("quantize 需要逗号分隔的浮点数")?;
    let beta = args
        .iter()
        .position(|a| a == "--beta")
        .and_then(|i| args.get(i + 1))
        .map(|s| s.parse::<f32>())
        .transpose()
        .map_err(|_| "beta 不是数字".to_string())?
        .unwrap_or(quantize::DEFAULT_BETA);
    let values: Vec<f32> = raw
        .split(',')
        .map(|s| {
            s.trim()
                .parse::<f32>()
                .map_err(|e| format!("不是数字 {s:?}: {e}"))
        })
        .collect::<Result<_, _>>()?;
    let q = quantize::quantize_with_beta(&values, beta);
    let text: String = q.trits.iter().map(|t| t.as_char()).collect();
    let err = quantize::quantization_error(&values, &q);
    println!(
        "{}",
        serde_json::to_string_pretty(&serde_json::json!({
            "trits": text,
            "scale": q.scale,
            "beta": q.beta,
            "quantization_error_l2": err,
        }))
        .unwrap_or_default()
    );
    Ok(())
}

fn dequantize_cmd(args: &[String]) -> Result<(), String> {
    let raw = args.first().ok_or("dequantize 需要 TRITS 串")?;
    let scale = args
        .iter()
        .position(|a| a == "--scale")
        .and_then(|i| args.get(i + 1))
        .ok_or("--scale 需要一个数字")?
        .parse::<f32>()
        .map_err(|_| "scale 不是数字".to_string())?;
    let trits = parse_trits(raw)?;
    let back = quantize::dequantize(&trits, scale);
    println!(
        "{}",
        serde_json::to_string_pretty(&serde_json::json!({
            "trits": raw,
            "scale": scale,
            "values": back,
        }))
        .unwrap_or_default()
    );
    Ok(())
}

fn sparse_cmd(args: &[String]) -> Result<(), String> {
    if args.iter().any(|a| a == "--decode") {
        let len: usize = args
            .iter()
            .position(|a| a == "--len")
            .and_then(|j| args.get(j + 1))
            .ok_or("sparse --decode 需要 --len N")?
            .parse()
            .map_err(|_| "len 不是整数".to_string())?;
        // 位置参数 = 所有不是 "--flag" 也不是 flag 值的参数。
        let positional = filter_positional(args);
        let raw = positional.first().ok_or("--decode 需要 entries JSON")?;
        // encode 输出的 entries 是 [[u32, "0"/"X"/"1"], ...]，先解 char 再 parse。
        let raw_entries: Vec<(u32, char)> =
            serde_json::from_str(raw).map_err(|e| format!("entries 不是合法 JSON: {e}"))?;
        let entries: Result<Vec<(u32, Ternary)>, String> = raw_entries
            .into_iter()
            .map(|(i, c)| {
                Ternary::parse_char(c)
                    .map(|t| (i, t))
                    .ok_or_else(|| format!("entries 里非法 trit 字符 {c:?}"))
            })
            .collect();
        let entries = entries?;
        let values = sparse::decode(&entries, len).map_err(|e| e.to_string())?;
        let text: String = values.iter().map(|t| t.as_char()).collect();
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({"trits": text, "len": len}))
                .unwrap_or_default()
        );
        return Ok(());
    }
    let raw = args.first().ok_or("sparse 需要 TRITS 串")?;
    let values = parse_trits(raw)?;
    let entries = sparse::encode(&values);
    let text_entries: Vec<(u32, char)> = entries.iter().map(|(i, v)| (*i, v.as_char())).collect();
    println!(
        "{}",
        serde_json::to_string_pretty(&serde_json::json!({
            "trits": raw,
            "entries": text_entries,
            "non_zero": entries.len(),
            "len": values.len(),
            "sparsity": sparse::sparsity(&entries, values.len()),
        }))
        .unwrap_or_default()
    );
    Ok(())
}

/// 有值 flag：遇到后连下一个参数一起跳过。
const VALUED_FLAGS: &[&str] = &["--block", "--len", "--beta", "--scale", "--index", "--n"];

/// 从 args 里去掉所有 flag 和 flag 的值，返回剩下的位置参数。
fn filter_positional(args: &[String]) -> Vec<&String> {
    let mut out = Vec::new();
    let mut skip_next = false;
    for a in args {
        if skip_next {
            skip_next = false;
            continue;
        }
        if a.starts_with("--") {
            if VALUED_FLAGS.contains(&a.as_str()) {
                skip_next = true;
            }
            continue;
        }
        out.push(a);
    }
    out
}

fn interleave_cmd(args: &[String]) -> Result<(), String> {
    let block: usize = args
        .iter()
        .position(|a| a == "--block")
        .and_then(|i| args.get(i + 1))
        .ok_or("interleave 需要 --block N")?
        .parse()
        .map_err(|_| "block 不是整数".to_string())?;
    let is_de = args.iter().any(|a| a == "--deinterleave");
    let positional = filter_positional(args);
    let raw = positional.first().ok_or("interleave 需要 TRITS 串")?;
    let values = parse_trits(raw)?;
    let out = if is_de {
        let len: usize = args
            .iter()
            .position(|a| a == "--len")
            .and_then(|j| args.get(j + 1))
            .map(|s| s.parse())
            .transpose()
            .map_err(|_| "len 不是整数".to_string())?
            .unwrap_or(values.len());
        interleave::deinterleave(&values, block, len).map_err(|e| e.to_string())?
    } else {
        interleave::interleave(&values, block).map_err(|e| e.to_string())?
    };
    let text: String = out.iter().map(|t| t.as_char()).collect();
    println!(
        "{}",
        serde_json::to_string_pretty(&serde_json::json!({
            "block": block,
            "deinterleave": is_de,
            "trits": text,
        }))
        .unwrap_or_default()
    );
    Ok(())
}

fn checksum_cmd(args: &[String]) -> Result<(), String> {
    let is_verify = args.iter().any(|a| a == "--verify");
    let raw = args
        .iter()
        .find(|a| !a.starts_with("--"))
        .ok_or("checksum 需要 TRITS 串")?;
    let values = parse_trits(raw)?;
    if is_verify {
        let ok = checksum::verify(&values);
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({"valid": ok})).unwrap_or_default()
        );
        if !ok {
            return Err("校验失败".to_string());
        }
        return Ok(());
    }
    let c = checksum::compute(&values);
    let ctext: String = c.iter().map(|t| t.as_char()).collect();
    let appended = checksum::append(&values);
    let atext: String = appended.iter().map(|t| t.as_char()).collect();
    println!(
        "{}",
        serde_json::to_string_pretty(&serde_json::json!({
            "data": raw,
            "checksum": ctext,
            "appended": atext,
        }))
        .unwrap_or_default()
    );
    Ok(())
}

fn rle_cmd(args: &[String]) -> Result<(), String> {
    if args.iter().any(|a| a == "--decode") {
        let len: usize = args
            .iter()
            .position(|a| a == "--len")
            .and_then(|j| args.get(j + 1))
            .ok_or("rle --decode 需要 --len N")?
            .parse()
            .map_err(|_| "len 不是整数".to_string())?;
        let positional = filter_positional(args);
        let raw = positional.first().ok_or("--decode 需要 runs JSON")?;
        let raw_runs: Vec<(char, u32)> =
            serde_json::from_str(raw).map_err(|e| format!("runs 不是合法 JSON: {e}"))?;
        let runs: Result<Vec<(Ternary, u32)>, String> = raw_runs
            .into_iter()
            .map(|(c, n)| {
                Ternary::parse_char(c)
                    .map(|t| (t, n))
                    .ok_or_else(|| format!("runs 里非法 trit 字符 {c:?}"))
            })
            .collect();
        let runs = runs?;
        let values = rle::decode(&runs, len).map_err(|e| e.to_string())?;
        let text: String = values.iter().map(|t| t.as_char()).collect();
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({"trits": text, "len": len}))
                .unwrap_or_default()
        );
        return Ok(());
    }
    let raw = args.first().ok_or("rle 需要 TRITS 串")?;
    let values = parse_trits(raw)?;
    let runs = rle::encode(&values);
    let text_runs: Vec<(char, u32)> = runs.iter().map(|(v, n)| (v.as_char(), *n)).collect();
    println!(
        "{}",
        serde_json::to_string_pretty(&serde_json::json!({
            "trits": raw,
            "runs": text_runs,
            "run_count": runs.len(),
            "compression_ratio": rle::compression_ratio(&runs, values.len()),
            "average_run_len": rle::average_run_len(&runs),
        }))
        .unwrap_or_default()
    );
    Ok(())
}

fn delta_cmd(args: &[String]) -> Result<(), String> {
    let positional = filter_positional(args);
    let raw = positional.first().ok_or("delta 需要 TRITS 串")?;
    let values = parse_trits(raw)?;
    if args.iter().any(|a| a == "--decode") {
        let back = delta::decode(&values);
        let text: String = back.iter().map(|t| t.as_char()).collect();
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({"deltas": raw, "trits": text}))
                .unwrap_or_default()
        );
        return Ok(());
    }
    let d = delta::encode(&values);
    let dtext: String = d.iter().map(|t| t.as_char()).collect();
    println!(
        "{}",
        serde_json::to_string_pretty(&serde_json::json!({"trits": raw, "deltas": dtext}))
            .unwrap_or_default()
    );
    Ok(())
}

fn gray_cmd(args: &[String]) -> Result<(), String> {
    if args.iter().any(|a| a == "--decode") {
        let positional = filter_positional(args);
        let raw = positional.first().ok_or("gray --decode 需要 WORD")?;
        let word = parse_trits(raw)?;
        let idx = gray::decode(&word);
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({"word": raw, "index": idx}))
                .unwrap_or_default()
        );
        return Ok(());
    }
    let index: usize = args
        .iter()
        .position(|a| a == "--index")
        .and_then(|i| args.get(i + 1))
        .ok_or("gray 需要 --index N")?
        .parse()
        .map_err(|_| "index 不是整数".to_string())?;
    let n: usize = args
        .iter()
        .position(|a| a == "--n")
        .and_then(|i| args.get(i + 1))
        .ok_or("gray 需要 --n M")?
        .parse()
        .map_err(|_| "n 不是整数".to_string())?;
    let word = gray::encode(index, n).map_err(|e| e.to_string())?;
    let text: String = word.iter().map(|t| t.as_char()).collect();
    println!(
        "{}",
        serde_json::to_string_pretty(&serde_json::json!({
            "index": index,
            "n": n,
            "codeword": text,
        }))
        .unwrap_or_default()
    );
    Ok(())
}

fn sop_cmd(args: &[String]) -> Result<(), String> {
    let positional = filter_positional(args);
    let raw = positional.first().ok_or("sop 需要 rows JSON")?;
    // rows: [[input_string, output_char], ...]
    let raw_rows: Vec<(String, char)> =
        serde_json::from_str(raw).map_err(|e| format!("rows 不是合法 JSON: {e}"))?;
    let mut rows: Vec<(Vec<Ternary>, Ternary)> = Vec::new();
    for (input_s, out_c) in raw_rows {
        let input = parse_trits(&input_s)?;
        let out = Ternary::parse_char(out_c)
            .ok_or_else(|| format!("输出字符 {out_c:?} 非法（只接受 0/1/X）"))?;
        rows.push((input, out));
    }
    let terms = sop::minimize(&rows).map_err(|e| e.to_string())?;
    let text_terms: Vec<String> = terms
        .iter()
        .map(|t| t.iter().map(|x| x.as_char()).collect())
        .collect();
    println!(
        "{}",
        serde_json::to_string_pretty(&serde_json::json!({
            "terms": text_terms,
            "term_count": terms.len(),
        }))
        .unwrap_or_default()
    );
    Ok(())
}

fn count_cmd(args: &[String]) -> Result<(), String> {
    let positional = filter_positional(args);
    let raw = positional.first().ok_or("count 需要 terms JSON")?;
    let raw_terms: Vec<String> =
        serde_json::from_str(raw).map_err(|e| format!("terms 不是合法 JSON: {e}"))?;
    let mut terms: Vec<Vec<Ternary>> = Vec::new();
    for s in &raw_terms {
        terms.push(parse_trits(s)?);
    }
    let n = count::count(&terms).map_err(|e| e.to_string())?;
    println!(
        "{}",
        serde_json::to_string_pretty(&serde_json::json!({
            "terms": raw_terms,
            "count": n,
        }))
        .unwrap_or_default()
    );
    Ok(())
}
