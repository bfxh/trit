//! 确定性模糊测试（N4 扫描面）：**任何输入不 panic、不挂起，只 Ok 或显式
//! Err**（外部输入第一道墙，与 trit-immune 的 fuzz_smoke 同一不变量）。
//! xorshift64 固定种子——失败可复现；真实样本结构变异 + 随机字节流 +
//! 对抗性跳跃链（预算封顶）四路夹击 `yaraparse` / `Pattern::hex` / `SigScan`。

use trit_re::sigscan::{Pattern, Rule, SigScan};
use trit_re::yaraparse::parse_rules;

/// xorshift64（固定种子可复现）。
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

    fn below(&mut self, n: usize) -> usize {
        if n == 0 {
            0
        } else {
            (self.next() % n as u64) as usize
        }
    }
}

/// 危险字符 alphabet：括号/引号/反斜杠/十六进制/中文/控制邻居全混。
/// 2026-11-07 补新子集词元：`@ # < > ! . K B M x i n t w 1 8 6`——
/// 数值比较 / for-of / xor / fullword / @ 下标都能被随机拼出来。
fn random_char(rng: &mut Rng) -> char {
    const ALPHABET: &[char] = &[
        '4', 'D', '5', 'A', '0', '9', 'F', '?', '?', '[', ']', '-', '(', ')', '{', '}', '"', '\\',
        '/', '$', '*', ' ', '\t', '\n', 'r', 'u', 'l', 'e', ':', '=', 'a', 'b', 'c', '注', '入',
        '打', '✓', '\u{7f}', '2', 'e', '@', '#', '<', '>', '!', '.', 'x', 'K', 'B', 'M', 'i', 'n',
        't', 'w', '1', '8', '6', 'f', 'o', 'z',
    ];
    ALPHABET[rng.below(ALPHABET.len())]
}

fn mutate(rng: &mut Rng, s: &str) -> String {
    let chars: Vec<char> = s.chars().collect();
    if chars.is_empty() {
        return String::new();
    }
    let mut out = chars.clone();
    for _ in 0..(rng.below(8) + 1) {
        let pos = rng.below(out.len());
        match rng.below(3) {
            0 => {
                out.remove(pos); // 删
            }
            1 => {
                out.insert(pos, random_char(rng)); // 插
            }
            _ => {
                out[pos] = random_char(rng); // 替
            }
        }
        if out.is_empty() {
            break;
        }
    }
    out.iter().collect()
}

/// 真实规则样本（与 tests/yara.rs 的夹具同源——结构变异的基底）。
/// 2026-11-07 补新子集种子：数值比较 / for-of / xor / fullword /
/// @ 下标 / Unscannable 串——变异器从真实形态出发才打得到新语法。
const REAL_RULES: &[&str] = &[
    r#"rule demo {
  strings:
    $mz = { 4D 5A ?? 00 }
    $str = "CreateRemoteThread" nocase
  condition:
    $mz at 0 and any of ($str*) or all of them or 2 of ($*) or not $str
}"#,
    r#"rule hexy : tag1 {
  strings:
    $h = { 4D 5A [2-4] 50 }
    $r = /evil[0-9]+/
  condition:
    $h or $r in (1..100)
}"#,
    r#"rule numeric {
  strings:
    $m = { 4D }
  condition:
    uint16(0) == 0x5A4D and filesize > 2KB and #m == 1 and @m[1] < 16
}"#,
    r#"rule loops {
  strings:
    $a = "ab"
    $b = { 02 }
  condition:
    for all of ($a*) : ( # == 1 ) or for 2 of them : ( $ at entrypoint ) or ($b and not $a)
}"#,
    r#"rule mods {
  strings:
    $x = "AZ" xor(0x10-0x20) fullword
    $w = "宽" ascii wide nocase
    $re = /mshta\.exe/ nocase
  condition:
    any of them
}"#,
];

#[test]
fn yaraparse_never_panics_on_random_text() {
    let mut rng = Rng(0x5EED_1234);
    for _ in 0..4000 {
        let len = rng.below(160);
        let s: String = (0..len).map(|_| random_char(&mut rng)).collect();
        // 不变量：只 Ok 或 Err，绝不 panic
        let _ = parse_rules(&s);
    }
}

#[test]
fn yaraparse_survives_structural_mutation_of_real_rules() {
    let mut rng = Rng(0xBEEF_CAFE);
    for _ in 0..3000 {
        let base = REAL_RULES[rng.below(REAL_RULES.len())];
        let mutated = mutate(&mut rng, base);
        // 不变量：变异后的规则文件只 Ok 或 Err
        let _ = parse_rules(&mutated);
    }
}

#[test]
fn hex_parser_never_panics_on_random_specs() {
    let mut rng = Rng(0xDEAD_BEEF);
    const HEX_ALPHA: &[char] = &[
        '4', 'D', '5', 'A', '0', '9', 'F', 'E', '?', '?', '[', ']', '-', ' ', '2', '8', '0', 'x',
        '{', '}',
    ];
    for _ in 0..4000 {
        let len = rng.below(24);
        let spec: String = (0..len)
            .map(|_| HEX_ALPHA[rng.below(HEX_ALPHA.len())])
            .collect();
        // 不变量：只 Ok 或 Err
        let _ = Pattern::hex(&spec);
    }
}

#[test]
fn sigscan_never_panics_on_random_data_and_patterns() {
    let mut rng = Rng(0xF00D_1234);
    for _ in 0..800 {
        let len = rng.below(2048);
        let data: Vec<u8> = (0..len).map(|_| rng.next() as u8).collect();
        let truncated = rng.below(4) == 0;
        // 随机合法模式（hex/文本）组成的规则集
        let mut rules = Vec::new();
        for i in 0..3 {
            let pattern = match rng.below(3) {
                0 => {
                    let spec_len = rng.below(10);
                    let spec: String = (0..spec_len)
                        .map(|_| {
                            const A: &[char] = &['9', '0', '0', '1', 'F', '?', '?'];
                            A[rng.below(A.len())]
                        })
                        .collect();
                    match Pattern::hex(&spec) {
                        Ok(p) => p,
                        Err(_) => continue,
                    }
                }
                1 => Pattern::text(
                    &"ab".repeat(rng.below(4) + 1),
                    rng.below(2) == 0,
                    rng.below(2) == 0,
                ),
                _ => match Pattern::hex("90 [0-64] 00 [0-64] 90") {
                    Ok(p) => p,
                    Err(_) => continue,
                },
            };
            rules.push(Rule::new(&format!("r{i}")).push("s", pattern));
        }
        if rules.is_empty() {
            continue;
        }
        let report = SigScan::new(&rules).scan(&data, truncated);
        // 命中数封顶纪律
        for r in &report.rules {
            assert!(r.hits.len() <= trit_re::sigscan::MAX_HITS);
        }
    }
}

#[test]
fn pathological_jump_chains_complete_within_budget() {
    // 对抗性跳跃链 × 对抗数据：朴素回溯会指数爆炸挂起；预算封顶必须返回。
    // （这条测试在无预算的版本上会跑几分钟——有预算后毫秒级。）
    let mut rng = Rng(0xD00D_1337);
    for _ in 0..40 {
        let data_len = rng.below(8192);
        let data: Vec<u8> = (0..data_len)
            .map(|_| if rng.below(2) == 0 { 0x90u8 } else { 0x00u8 })
            .collect();
        let segments = rng.below(5) + 2;
        let mut spec = String::from("90");
        for _ in 0..segments {
            spec.push_str(" [0-4096] ");
            if rng.below(2) == 0 {
                spec.push_str("90");
            } else {
                spec.push_str("00");
            }
        }
        let p = Pattern::hex(&spec).expect("合法跳跃链");
        let report = SigScan::new(&[Rule::new("r").push("a", p)]).scan(&data, false);
        // 裁决三态皆可——关键是**有限时间返回**（本测试跑完 = 不挂起）
        let _ = report.value;
    }
}
