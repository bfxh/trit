//! 三进制专项评估：`trit-export` GGUF 容器的「往返 + 逐位一致」独立复核。
//!
//! 复核点：GGUF v3 容器 write→read 后每个张量的 trit 顺序、scale 的 f32 位模式、bias
//! 三值、元数据键值与序列化确定性；坏输入（magic/版本/截断/重名/零形状）必须显式报错。
//! 与 `trit-codec` 的口径分开：这里只用 `trit_core::pack` 的线格式契约做独立对拍，
//! 不把容器格式和编码器混成一件事。
//!
//! 用法：
//!
//! ```text
//! cargo run -p trit-export --bin gguf_verify [-- --json]
//! ```
//!
//! 退出码：0 全绿；2 有失败。读数与口径见 `docs/research/TERNARY-AUDIT.md`。

use std::collections::BTreeMap;
use std::process::ExitCode;

use trit_core::value::Ternary;
use trit_export::{Error, MetaValue, Model, Tensor, read, write};

const SEED: u64 = 0x2545_f491_4f6c_dd1d;
/// scale 取样：普通值、负值、极小、极大（都必须是有限 f32）。
const SCALES: [f32; 4] = [0.5, -1.25, 1e-30, 3.0e38];

fn main() -> ExitCode {
    let json = std::env::args().skip(1).any(|arg| arg == "--json");
    let checks: Vec<Check> = vec![
        roundtrip_checks(),
        metadata_checks(),
        bad_input_checks(),
        pack_parity_checks(),
    ];

    if json {
        println!("{}", to_json(&checks));
    } else {
        print!("{}", to_text(&checks));
    }
    let failed: u64 = checks.iter().map(|check| check.failed).sum();
    if failed == 0 {
        ExitCode::SUCCESS
    } else {
        ExitCode::from(2)
    }
}

// ---------------------------------------------------------------- 基础件

struct Check {
    name: &'static str,
    cases: u64,
    failed: u64,
    first: Option<String>,
}

impl Check {
    fn new(name: &'static str) -> Self {
        Self {
            name,
            cases: 0,
            failed: 0,
            first: None,
        }
    }

    fn case(&mut self, ok: bool, detail: impl FnOnce() -> String) {
        self.cases += 1;
        if !ok {
            self.failed += 1;
            if self.first.is_none() {
                self.first = Some(detail());
            }
        }
    }
}

/// 极小 xorshift64：造张量 trit，不承担密码学用途。
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }

    fn trit(&mut self) -> Ternary {
        Ternary::from_index((self.next() % 3) as usize)
    }
}

fn trits(count: usize, seed: u64) -> Vec<Ternary> {
    let mut rng = Rng(seed | 1);
    (0..count).map(|_| rng.trit()).collect()
}

fn model(rows: usize, cols: usize, name: &str, seed: u64) -> Model {
    let scales = (0..rows)
        .map(|index| SCALES[index % SCALES.len()])
        .collect();
    let bias = (0..rows)
        .map(|index| Ternary::from_index((index + 1) % 3))
        .collect();
    Model {
        metadata: BTreeMap::new(),
        tensors: vec![Tensor {
            name: name.to_string(),
            rows,
            cols,
            trits: trits(rows * cols, seed),
            scales,
            bias,
        }],
    }
}

/// 逐字段比对；scale 用 `to_bits` 比，避免 `-0.0 == 0.0` 把位模式差异吞掉。
fn same(original: &Model, back: &Model) -> Result<(), String> {
    if original.tensors.len() != back.tensors.len() {
        return Err(format!(
            "张量数 {} != {}",
            original.tensors.len(),
            back.tensors.len()
        ));
    }
    if !same_metadata(&original.metadata, &back.metadata) {
        return Err("元数据不等".to_string());
    }
    for (left, right) in original.tensors.iter().zip(&back.tensors) {
        if left.name != right.name || left.rows != right.rows || left.cols != right.cols {
            return Err(format!("形状/名字不等: {}", left.name));
        }
        if left.trits != right.trits {
            return Err(format!("trit 不等: {}", left.name));
        }
        if left.scales.len() != right.scales.len() {
            return Err(format!("scale 长度不等: {}", left.name));
        }
        for (a, b) in left.scales.iter().zip(&right.scales) {
            if a.to_bits() != b.to_bits() {
                return Err(format!("scale 位模式不等: {} {} != {}", left.name, a, b));
            }
        }
        if left.bias != right.bias {
            return Err(format!("bias 不等: {}", left.name));
        }
    }
    Ok(())
}

fn same_metadata(left: &BTreeMap<String, MetaValue>, right: &BTreeMap<String, MetaValue>) -> bool {
    if left.len() != right.len() {
        return false;
    }
    left.iter()
        .all(|(key, value)| match (value, right.get(key)) {
            (MetaValue::U32(a), Some(MetaValue::U32(b))) => a == b,
            (MetaValue::F32(a), Some(MetaValue::F32(b))) => a.to_bits() == b.to_bits(),
            (MetaValue::Str(a), Some(MetaValue::Str(b))) => a == b,
            _ => false,
        })
}

// ---------------------------------------------------------------- 检查

fn roundtrip_checks() -> Check {
    let mut check = Check::new("gguf.roundtrip_all_shapes");
    for rows in 1..=6usize {
        for cols in 1..=20usize {
            let original = model(rows, cols, "w", SEED ^ (rows * 131 + cols) as u64);
            match write(&original).and_then(|bytes| read(&bytes)) {
                Ok(back) => match same(&original, &back) {
                    Ok(()) => check.case(true, String::new),
                    Err(message) => check.case(false, || format!("{rows}x{cols}: {message}")),
                },
                Err(error) => check.case(false, || format!("{rows}x{cols}: 读写报错 {error}")),
            }
        }
    }
    // 病态 trit 分布与大张量：全 Unknown / 全 True / 全 False，以及 33×129（4257 trit）。
    for (label, fill, rows, cols) in [
        ("all-unknown", Ternary::Unknown, 2usize, 9usize),
        ("all-true", Ternary::True, 3, 7),
        ("all-false", Ternary::False, 4, 5),
    ] {
        let mut original = model(rows, cols, label, SEED);
        original.tensors[0].trits = vec![fill; rows * cols];
        match write(&original).and_then(|bytes| read(&bytes)) {
            Ok(back) => match same(&original, &back) {
                Ok(()) => check.case(true, String::new),
                Err(message) => check.case(false, || format!("{label}: {message}")),
            },
            Err(error) => check.case(false, || format!("{label}: 读写报错 {error}")),
        }
    }
    let big = model(33, 129, "big", SEED ^ 0x5151);
    match write(&big).and_then(|bytes| read(&bytes)) {
        Ok(back) => match same(&big, &back) {
            Ok(()) => check.case(true, String::new),
            Err(message) => check.case(false, || format!("big: {message}")),
        },
        Err(error) => check.case(false, || format!("big: 读写报错 {error}")),
    }
    check
}

fn metadata_checks() -> Check {
    let mut check = Check::new("gguf.metadata_and_determinism");
    let mut first = model(2, 5, "encoder.weight", SEED);
    first.metadata.insert(
        "general.name".to_string(),
        MetaValue::Str("trit-audit".to_string()),
    );
    first
        .metadata
        .insert("trit.version".to_string(), MetaValue::U32(7));
    first
        .metadata
        .insert("trit.alpha".to_string(), MetaValue::F32(-0.75));
    let second = Tensor {
        name: "encoder.bias".to_string(),
        rows: 4,
        cols: 3,
        trits: trits(12, SEED ^ 7),
        // 位模式敏感值：-0.0、最小正规、最小次正规、NaN。
        scales: vec![-0.0, f32::MIN_POSITIVE, 1e-45, f32::NAN],
        bias: vec![
            Ternary::Unknown,
            Ternary::False,
            Ternary::True,
            Ternary::Unknown,
        ],
    };
    first.tensors.push(second);

    match write(&first) {
        Ok(bytes_a) => {
            let bytes_b = write(&first).expect("同一模型第二次写出");
            check.case(bytes_a == bytes_b, || {
                "两次写出字节不同（序列化不确定）".to_string()
            });
            match read(&bytes_a) {
                Ok(back) => {
                    match same(&first, &back) {
                        Ok(()) => check.case(true, String::new),
                        Err(message) => check.case(false, || message),
                    }
                    let again = write(&back).expect("读回后再写出");
                    check.case(again == bytes_a, || "读回再写字节不同".to_string());
                }
                Err(error) => check.case(false, || format!("读回报错 {error}")),
            }
        }
        Err(error) => check.case(false, || format!("写出报错 {error}")),
    }
    check
}

fn bad_input_checks() -> Check {
    let mut check = Check::new("gguf.rejects_bad_input");
    let bytes = write(&model(2, 3, "w", SEED)).expect("基准模型写出");

    let mut bad_magic = bytes.clone();
    bad_magic[0] = b'X';
    check.case(matches!(read(&bad_magic), Err(Error::Magic)), || {
        "magic 损坏未报 Magic".to_string()
    });

    let mut bad_version = bytes.clone();
    bad_version[4] = 2;
    check.case(matches!(read(&bad_version), Err(Error::Version(2))), || {
        "版本 2 未报 Version(2)".to_string()
    });

    check.case(read(&bytes[..bytes.len() / 2]).is_err(), || {
        "半截文件未报错".to_string()
    });
    check.case(read(&[0u8; 4]).is_err(), || {
        "只有 magic 的文件未报错".to_string()
    });

    let mut duplicate = model(1, 2, "same", SEED);
    duplicate.tensors.push(Tensor {
        name: "same".to_string(),
        rows: 1,
        cols: 2,
        trits: trits(2, SEED ^ 3),
        scales: vec![0.5],
        bias: vec![Ternary::False],
    });
    check.case(
        matches!(write(&duplicate), Err(Error::Duplicate(_))),
        || "重名张量未拒绝".to_string(),
    );

    let zero_rows = Model {
        metadata: BTreeMap::new(),
        tensors: vec![Tensor {
            name: "z".to_string(),
            rows: 0,
            cols: 3,
            trits: Vec::new(),
            scales: Vec::new(),
            bias: Vec::new(),
        }],
    };
    check.case(write(&zero_rows).is_err(), || "0 行张量未拒绝".to_string());

    check
}

/// 独立打包器（5 trits/byte，低位在前）：与容器里 trit 的逐位一致性对拍。
fn oracle_pack(values: &[Ternary]) -> Vec<u8> {
    let mut out = Vec::new();
    for chunk in values.chunks(5) {
        let mut byte = 0u32;
        let mut weight = 1u32;
        for value in chunk {
            byte += value.index() as u32 * weight;
            weight *= 3;
        }
        out.push(byte as u8);
    }
    out
}

fn pack_parity_checks() -> Check {
    let mut check = Check::new("gguf.pack_bit_parity");
    for rows in 1..=4usize {
        for cols in 1..=13usize {
            let original = model(rows, cols, "w", SEED ^ (rows * 53 + cols * 11) as u64);
            let bytes = write(&original).expect("写出");
            let back = read(&bytes).expect("读回");
            let before = &original.tensors[0].trits;
            let after = &back.tensors[0].trits;
            check.case(
                trit_core::pack::pack(before) == trit_core::pack::pack(after),
                || format!("{rows}x{cols}: 往返后线格式字节不同"),
            );
            check.case(trit_core::pack::pack(before) == oracle_pack(before), || {
                format!("{rows}x{cols}: 与独立打包器不一致")
            });
        }
    }
    check
}

// ---------------------------------------------------------------- 输出

fn to_text(checks: &[Check]) -> String {
    let mut out = String::new();
    out.push_str("三进制专项评估：trit-export GGUF 容器往返与逐位一致复核\n");
    out.push_str(&format!(
        "kind=trit-export-verify seed=0x{SEED:016x} 形状=1..6 行 × 1..20 列 + 病态分布\n"
    ));
    out.push_str(&format!(
        "{:<30} {:>8} {:>8}  {}\n",
        "check", "cases", "failed", "note"
    ));
    for check in checks {
        let note = match &check.first {
            Some(message) => format!("首个反例: {message}"),
            None => String::new(),
        };
        out.push_str(&format!(
            "{:<30} {:>8} {:>8}  {}\n",
            check.name, check.cases, check.failed, note
        ));
    }
    let cases: u64 = checks.iter().map(|check| check.cases).sum();
    let failed: u64 = checks.iter().map(|check| check.failed).sum();
    out.push_str(&format!(
        "结论：{}（{} 项检查 / {cases} 例 / {failed} 失败）\n",
        if failed == 0 { "绿" } else { "红" },
        checks.len()
    ));
    out
}

fn to_json(checks: &[Check]) -> String {
    let mut out = format!(
        "{{\"schema\":1,\"kind\":\"trit-export-verify\",\"seed\":\"0x{SEED:016x}\",\"checks\":["
    );
    for (index, check) in checks.iter().enumerate() {
        if index > 0 {
            out.push(',');
        }
        out.push_str(&format!(
            "{{\"name\":\"{}\",\"cases\":{},\"failed\":{},\"first_failure\":",
            check.name, check.cases, check.failed
        ));
        match &check.first {
            Some(message) => out.push_str(&format!("\"{}\"", escape(message))),
            None => out.push_str("null"),
        }
        out.push('}');
    }
    let cases: u64 = checks.iter().map(|check| check.cases).sum();
    let failed: u64 = checks.iter().map(|check| check.failed).sum();
    out.push_str(&format!(
        "],\"totals\":{{\"cases\":{cases},\"failed\":{failed}}},\"ok\":{}}}\n",
        failed == 0
    ));
    out
}

fn escape(raw: &str) -> String {
    let mut out = String::new();
    for ch in raw.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sample_model_survives_write_read() {
        let original = model(2, 7, "w", SEED);
        let back = read(&write(&original).expect("写出")).expect("读回");
        assert!(same(&original, &back).is_ok());
    }

    #[test]
    fn oracle_pack_matches_wire_vectors() {
        assert_eq!(oracle_pack(&[Ternary::False; 5]), vec![0]);
        assert_eq!(oracle_pack(&[Ternary::True; 5]), vec![242]);
        assert_eq!(oracle_pack(&[Ternary::Unknown; 5]), vec![121]);
        assert_eq!(oracle_pack(&[Ternary::True]), vec![2]);
    }

    #[test]
    fn metadata_bit_compare_keeps_negative_zero_apart() {
        let mut left = BTreeMap::new();
        left.insert("a".to_string(), MetaValue::F32(-0.0));
        let mut right = BTreeMap::new();
        right.insert("a".to_string(), MetaValue::F32(0.0));
        assert!(!same_metadata(&left, &right));
    }
}
