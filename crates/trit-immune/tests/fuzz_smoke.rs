//! 解析器模糊测试（确定性随机字节流，无需 nightly）——攻击面永不 panic。
//!
//! 解析器是**外部输入的第一道墙**（JSON / git log / LSP / Cargo.lock / LSP
//! 帧），模糊测试的核心不变量只有一条：**任何输入都不 panic、不挂起、
//! 只返回 Ok 或显式 Err**。与 `tests/properties.rs` 同一方法学：xorshift64
//! 固定种子——失败的输入序列可复现。
//!
//! 这是"穷人版 fuzz"：没有 libFuzzer 的覆盖率引导，但确定性、可复现、
//! 零依赖、进常规测试套件。正式的 cargo-fuzz targets 见 `fuzz/` 目录
//! （触发条件：nightly 工具链，结构相同——invariant 一致）。

#[cfg(feature = "full")]
use trit_immune::{parse_git_log, parse_lsp_stream};
use trit_immune::{parse_lock, parse_stamped_line};

/// xorshift64：与性质测试同一 RNG（同种子同序列，失败可复现）。
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
}

/// 生成一段随机字节（0..n 长度，全字节域 0..=255）。
fn random_bytes(rng: &mut Rng) -> Vec<u8> {
    let n = (rng.next() % 512) as usize;
    (0..n).map(|_| (rng.next() % 256) as u8).collect()
}

/// 生成"结构化污损"文本：拿真实样本随机变异（删/插/替字节）——
/// 比纯随机更接近真实攻击（格式大体对、局部毒化）。
fn mutate(rng: &mut Rng, base: &str) -> String {
    let mut bytes = base.as_bytes().to_vec();
    let rounds = 1 + (rng.next() % 8) as usize;
    for _ in 0..rounds {
        if bytes.is_empty() {
            break;
        }
        let pos = (rng.next() % bytes.len() as u64) as usize;
        match rng.next() % 3 {
            0 => {
                bytes.remove(pos); // 删
            }
            1 => bytes.insert(pos, (rng.next() % 256) as u8), // 插
            2 => bytes[pos] = (rng.next() % 256) as u8,       // 替
            _ => unreachable!(),
        }
    }
    String::from_utf8_lossy(&bytes).into_owned()
}

// ── 纯随机字节流 ────────────────────────────────────────────────────────

#[test]
fn fuzz_json_random_bytes_never_panics() {
    let mut rng = Rng(0xF00D_0001);
    for _ in 0..500 {
        let bytes = random_bytes(&mut rng);
        let text = String::from_utf8_lossy(&bytes).into_owned();
        let _ = trit_immune::json::parse(&text); // Ok 或 Err 都行，panic 不行
    }
}

#[cfg(feature = "full")]
#[test]
fn fuzz_git_log_random_bytes_never_panics() {
    let mut rng = Rng(0xF00D_0002);
    for _ in 0..500 {
        let bytes = random_bytes(&mut rng);
        let _ = parse_git_log(&String::from_utf8_lossy(&bytes));
    }
}

#[cfg(feature = "full")]
#[test]
fn fuzz_lsp_random_bytes_never_panics() {
    let mut rng = Rng(0xF00D_0003);
    for _ in 0..500 {
        let bytes = random_bytes(&mut rng);
        let _ = parse_lsp_stream(&String::from_utf8_lossy(&bytes));
    }
}

#[test]
fn fuzz_lock_random_bytes_never_panics() {
    let mut rng = Rng(0xF00D_0004);
    for _ in 0..500 {
        let bytes = random_bytes(&mut rng);
        let _ = parse_lock(&String::from_utf8_lossy(&bytes));
    }
}

// ── 结构化污损（真实样本变异——比纯随机更毒） ────────────────────────────

const REAL_JSON: &str =
    r#"{"jsonrpc":"2.0","id":1,"result":{"capabilities":{"textDocumentSync":1}}}"#;
const REAL_GIT: &str = "\u{1e}abc123def456\u{1f}alice\u{1f}1700000000\u{1f}feat: 登录模块\n";
const REAL_LOCK: &str =
    "[[package]]\nname = \"libc\"\nversion = \"0.2.150\"\nchecksum = \"abc123\"\n";
const REAL_JOURNAL: &str = "1740000000000:0:0\tRECORD\t1\t+1\t-\t-\t-\t-\t回归通过";

#[test]
fn fuzz_json_mutated_real_sample_never_panics() {
    let mut rng = Rng(0xF00D_0011);
    for _ in 0..500 {
        let text = mutate(&mut rng, REAL_JSON);
        let _ = trit_immune::json::parse(&text);
    }
}

#[cfg(feature = "full")]
#[test]
fn fuzz_git_log_mutated_never_panics() {
    let mut rng = Rng(0xF00D_0012);
    for _ in 0..500 {
        let _ = parse_git_log(&mutate(&mut rng, REAL_GIT));
    }
}

#[test]
fn fuzz_lock_mutated_never_panics() {
    let mut rng = Rng(0xF00D_0014);
    for _ in 0..500 {
        let _ = parse_lock(&mutate(&mut rng, REAL_LOCK));
    }
}

#[test]
fn fuzz_journal_line_mutated_never_panics() {
    let mut rng = Rng(0xF00D_0015);
    for _ in 0..500 {
        let _ = parse_stamped_line(&mutate(&mut rng, REAL_JOURNAL));
    }
}

#[cfg(feature = "full")]
#[test]
fn fuzz_lsp_mutated_never_panics() {
    let mut rng = Rng(0xF00D_0013);
    for _ in 0..500 {
        let _ = parse_lsp_stream(&mutate(&mut rng, REAL_JSON));
    }
}

// ── 深度攻击：嵌套 JSON 不越限（解析器自带深度上限 128） ─────────────────

#[test]
fn fuzz_json_deep_nesting_is_bounded() {
    // 10_000 层嵌套：解析器必须显式报错而不是栈溢出
    let deep = format!("{}{}", "[".repeat(10_000), "]".repeat(10_000));
    assert!(
        trit_immune::json::parse(&deep).is_err(),
        "超限嵌套必须显式拒绝"
    );
    // 巨型字符串转义序列
    let escapes = format!("\"{}\"", "\\u0001".repeat(50_000));
    let _ = trit_immune::json::parse(&escapes); // 不 panic 即可
}
