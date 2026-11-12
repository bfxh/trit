//! yaraparse 的集成测试（从 src/yaraparse.rs 拆出，控制单文件行数预算）。

use trit_core::value::Ternary;
use trit_re::sigscan::{Condition, Pattern, SigScan};
use trit_re::yaraparse::{RuleOutcome, parse_rules};

#[test]
fn parses_full_rule_with_meta_strings_condition() {
    let src = r#"
// 演示规则：注释里的 // 和 " 引号不破坏词法
import "pe"   // 顶层注记

rule demo_rule : tag1 tag2 {
  meta:
    author = "someone // not a comment"
    version = 2
  strings:
    $mz = { 4D 5A ?? 00 }
    $str = "CreateRemoteThread" nocase
    $wide = "AZ" wide
  condition:
    $mz at 0 and any of ($str*) or all of them or 2 of ($*) or not $wide
}
"#;
    let out = parse_rules(src).expect("解析成功");
    assert_eq!(out.notes.len(), 1, "import 记入 notes");
    assert_eq!(out.rules.len(), 1);
    let RuleOutcome::Parsed(rule) = &out.rules[0] else {
        panic!("应为 Parsed");
    };
    assert_eq!(rule.name, "demo_rule");
    assert_eq!(rule.patterns().len(), 3);
}

#[test]
fn regex_string_parses_but_scans_unknown() {
    // 正则串可解析（Unscannable 形态）但不可扫描：整条规则判 Unknown
    let src = r#"
rule re {
  strings:
    $a = /mshta\.exe/ nocase
  condition:
    $a
}
"#;
    let out = parse_rules(src).unwrap();
    assert_eq!(out.rules.len(), 1);
    let RuleOutcome::Parsed(rule) = &out.rules[0] else {
        panic!("正则串现在可解析（Unscannable 形态）");
    };
    assert!(
        matches!(rule.patterns()[0].1, Pattern::Unscannable(_)),
        "串应记为 Unscannable"
    );
    // 8 字节完整扫描、串无法评估 → Unknown，绝不 False
    let report = SigScan::new(std::slice::from_ref(rule)).scan(b"mshta.exe", false);
    assert_eq!(report.rules[0].value, Ternary::Unknown);
    assert_eq!(report.value, Ternary::Unknown);
}

#[test]
fn filesize_condition_evaluates() {
    let src = r#"
rule fs {
  strings:
    $a = "x"
  condition:
    filesize < 100 and $a
}
"#;
    let out = parse_rules(src).unwrap();
    let RuleOutcome::Parsed(rule) = &out.rules[0] else {
        panic!("filesize 现在可评估");
    };
    // 2 字节 < 100 且 "x" 命中 → True
    let report = SigScan::new(std::slice::from_ref(rule)).scan(b"ax", false);
    assert_eq!(report.rules[0].value, Ternary::True);
    // 300 字节：filesize < 100 = False → 整条 False
    let report = SigScan::new(std::slice::from_ref(rule)).scan(&[0u8; 300], false);
    assert_eq!(report.rules[0].value, Ternary::False);
}

#[test]
fn unknown_string_reference_bails() {
    let src = r#"
rule ghost {
  strings:
    $a = "x"
  condition:
    $a and $missing
}
"#;
    let out = parse_rules(src).unwrap();
    assert!(matches!(out.rules[0], RuleOutcome::Unsupported { .. }));
}

#[test]
fn unsupported_rule_scans_as_unknown_not_false() {
    // 三值纪律的本体：含不可扫描串的规则报 Unknown，绝不 False；
    // 报警语义（Kleene OR）下 True ∨ Unknown = True。
    let good_src = r#"
rule good {
  strings:
    $a = { 4D 5A }
  condition:
    $a
}
"#;
    let bad_src = r#"
rule bad {
  strings:
    $a = /regex/
  condition:
    $a
}
"#;
    let good = parse_rules(good_src).unwrap();
    let bad = parse_rules(bad_src).unwrap();
    let mut rules = Vec::new();
    for outcome in good.rules.iter().chain(bad.rules.iter()) {
        if let RuleOutcome::Parsed(r) = outcome {
            rules.push(r.clone());
        }
    }
    let report = SigScan::new(&rules).scan(&[0x4D, 0x5A], false);
    assert_eq!(report.rules.len(), 2);
    assert_eq!(report.rules[0].value, Ternary::True, "good 规则照常命中");
    assert_eq!(
        report.rules[1].value,
        Ternary::Unknown,
        "含正则串的规则 Unknown（不假未命中）"
    );
    assert_eq!(report.value, Ternary::True, "T ∨ U = T（报警语义）");
}

#[test]
fn condition_tree_evaluates_through_sigscan() {
    let src = r#"
rule combo {
  strings:
    $a = { 01 }
    $b = { 02 }
  condition:
    $a and not $b or 2 of ($*)
}
"#;
    let out = parse_rules(src).unwrap();
    let RuleOutcome::Parsed(rule) = &out.rules[0] else {
        panic!("应为 Parsed");
    };
    // 只命中 $a：$a and not $b = True → Or 为 True
    let report = SigScan::new(std::slice::from_ref(rule)).scan(&[0x01], false);
    assert_eq!(report.rules[0].value, Ternary::True);
    // 都命中：not $b = False，2 of ($*) = True → Or 为 True
    let report = SigScan::new(std::slice::from_ref(rule)).scan(&[0x01, 0x02], false);
    assert_eq!(report.rules[0].value, Ternary::True);
    // 只命中 $b：$a 假、not $b 假、2 of 假 → False
    let report = SigScan::new(std::slice::from_ref(rule)).scan(&[0x02], false);
    assert_eq!(report.rules[0].value, Ternary::False);
}

#[test]
fn structural_errors_are_reported() {
    assert!(
        parse_rules("rule x { strings: $a = \"u\"").is_err(),
        "未闭合"
    );
    assert!(parse_rules("rule { condition: true }").is_err(), "缺名字");
    assert!(parse_rules("/* 未闭合").is_err(), "块注释未闭合");
    assert!(parse_rules("rule x { strings: $a = \"u\" } rule { }").is_err());
}

#[test]
fn hex_block_with_jump_parses() {
    let src = r#"
rule hexy {
  strings:
    $h = { 4D 5A [2-4] 50 }
  condition:
    $h
}
"#;
    let out = parse_rules(src).unwrap();
    let RuleOutcome::Parsed(rule) = &out.rules[0] else {
        panic!("应为 Parsed");
    };
    assert_eq!(Condition::Ref(0), *rule.condition_ref());
}

// ── N4 完整子集：数值比较 / for-of / xor / fullword / 记名弃权 ──────────

/// 解析单规则文件并扫描。
fn scan_one(src: &str, data: &[u8]) -> Ternary {
    let out = parse_rules(src).unwrap();
    assert_eq!(out.rules.len(), 1, "单规则文件");
    match &out.rules[0] {
        RuleOutcome::Parsed(rule) => {
            SigScan::new(std::slice::from_ref(rule))
                .scan(data, false)
                .rules[0]
                .value
        }
        RuleOutcome::Unsupported { name, reason } => {
            panic!("规则 {name} 意外 Unsupported：{reason}");
        }
    }
}

#[test]
fn numeric_conditions_evaluate() {
    // uintN 小端读 + filesize（KB 后缀）+ #a 计数 + @a 偏移，全可评估
    let src = r#"
rule num {
  strings:
    $mz = { 4D }
  condition:
    uint8(0) == 0x4D and filesize > 2KB and #mz == 1 and @mz < 16
}
"#;
    let mut data = vec![0x4Du8];
    data.resize(3000, 0x00);
    assert_eq!(scan_one(src, &data), Ternary::True);
    // 再放一个 0x4D：#mz == 1 不再成立 → False
    data[2999] = 0x4D;
    assert_eq!(scan_one(src, &data), Ternary::False);
}

#[test]
fn stringless_rule_with_numeric_condition() {
    // 无 strings 规则：纯数值条件合法（真实规则里的常见形态）
    let src = r#"
rule bare {
  condition:
    uint16(0) == 0x5A4D and filesize < 1MB
}
"#;
    let mut data = vec![0x4Du8, 0x5A];
    data.resize(64, 0x00);
    assert_eq!(scan_one(src, &data), Ternary::True);
    let wrong = vec![0x00u8; 64];
    assert_eq!(scan_one(src, &wrong), Ternary::False);
}

#[test]
fn for_of_binds_current_string() {
    let src = r#"
rule loops {
  strings:
    $a = "ab"
    $b = "cd"
  condition:
    for all of ($a*) : ( # == 1 )
}
"#;
    // "ab" 一次、"cd" 一次 → 全部满足 → True
    assert_eq!(scan_one(src, b"ab__cd"), Ternary::True);
    // "ab" 两次 → # == 2 → False
    assert_eq!(scan_one(src, b"abab_cd"), Ternary::False);
    // 未命中串也在迭代里（# = 0）→ 体假 → False（官方语义，差分实测判定）
    assert_eq!(scan_one(src, b"____cd"), Ternary::False);
}

#[test]
fn for_of_iterates_unmatched_strings() {
    // 差分测试抓到的第三个语义偏差：官方 for-of 迭代**整个串集**——
    // 未命中串 `#` = 0 → `# >= 0` 为真 → any 命中（即使针完全缺席）
    let src = r#"
rule v {
  strings:
    $a = "ab"
  condition:
    for any of ($a*) : ( # >= 0 )
}
"#;
    assert_eq!(
        scan_one(src, b"xxxxxxxx"),
        Ternary::True,
        "未命中串 #=0 满足体"
    );
    // `$`（当前串引用）对未命中串 = 假 → 体假 → any 无满足 → False
    let src2 = r#"
rule w {
  strings:
    $a = "ab"
  condition:
    for any of ($a*) : ( $ )
}
"#;
    assert_eq!(scan_one(src2, b"xxxxxxxx"), Ternary::False);
    assert_eq!(scan_one(src2, b"abxx"), Ternary::True);
}

#[test]
fn definite_undefined_compares_false_not_unknown() {
    // 差分测试抓到的第四个语义分歧：零命中串的 @a[N] 与非截断的越界
    // uintN 读是**确定的未定义**（证据完整）——与官方一致，比较为假；
    // 截断输入的 uintN 才是证据缺失 → Unknown。两种"无值"不混同。
    let absent_offset = r#"
rule o {
  strings:
    $a = "ab"
  condition:
    #a == 0 and @a[1] < 100
}
"#;
    // 官方：@a[1] 未定义 → 比较为假 → 整条 False（不是 Unknown）
    assert_eq!(scan_one(absent_offset, b"xxxxxxxx"), Ternary::False);
    let oob = r#"
rule u {
  condition:
    uint32(1000000) == 0
}
"#;
    // 非截断的越界读：字节客观不存在 → 比较假
    assert_eq!(scan_one(oob, b"xxxxxxxx"), Ternary::False);
    // 截断输入的 uintN：证据缺失 → Unknown（fail-closed 不变）
    let out = parse_rules(oob).unwrap();
    let RuleOutcome::Parsed(rule) = &out.rules[0] else {
        panic!("应为 Parsed");
    };
    let r = SigScan::new(std::slice::from_ref(rule)).scan(b"xxxxxxxx", true);
    assert_eq!(
        r.rules[0].value,
        Ternary::Unknown,
        "截断 = 证据缺失，照旧弃权"
    );
}

#[test]
fn for_any_with_entrypoint_context_evaluates() {
    // 经典形态 `$ at entrypoint`：调用方传入入口点上下文后可评估——
    // 无上下文仍弃权（Unknown），有上下文按命中判定
    let src = r#"
rule ep {
  strings:
    $a = { 4D 5A }
  condition:
    for any of ($a*) : ( $ at entrypoint )
}
"#;
    let out = parse_rules(src).unwrap();
    let RuleOutcome::Parsed(rule) = &out.rules[0] else {
        panic!("应为 Parsed");
    };
    let data = [0x90u8, 0x4D, 0x5A];
    // 无上下文：体弃权 → any = 0 满足 + 1 弃权 → Unknown
    let r = SigScan::new(std::slice::from_ref(rule)).scan(&data, false);
    assert_eq!(r.rules[0].value, Ternary::Unknown);
    // 入口点 = 0：$a 首次命中在 1 ≠ 0 → 体 False → any 无满足 → False
    let r = SigScan::new(std::slice::from_ref(rule)).scan_with_entrypoint(&data, false, Some(0));
    assert_eq!(r.rules[0].value, Ternary::False);
    // 入口点 = 1：命中恰在入口 → True
    let r = SigScan::new(std::slice::from_ref(rule)).scan_with_entrypoint(&data, false, Some(1));
    assert_eq!(r.rules[0].value, Ternary::True);
}

#[test]
fn at_entrypoint_direct_condition_evaluates() {
    let src = r#"
rule ep {
  strings:
    $mz = { 4D 5A 90 00 }
  condition:
    $mz at entrypoint
}
"#;
    let out = parse_rules(src).unwrap();
    let RuleOutcome::Parsed(rule) = &out.rules[0] else {
        panic!("应为 Parsed");
    };
    let data = [0x4Du8, 0x5A, 0x90, 0x00, 0xCC];
    let r = SigScan::new(std::slice::from_ref(rule)).scan_with_entrypoint(&data, false, Some(0));
    assert_eq!(r.rules[0].value, Ternary::True);
    let r = SigScan::new(std::slice::from_ref(rule)).scan_with_entrypoint(&data, false, Some(2));
    assert_eq!(r.rules[0].value, Ternary::False);
    let r = SigScan::new(std::slice::from_ref(rule)).scan(&data, false);
    assert_eq!(r.rules[0].value, Ternary::Unknown, "无上下文绝不 False");
}

#[test]
fn string_offset_index_is_one_based() {
    // 官方 YARA 语义（yara-python 实测判定）：@a[1] = 首次、@a[0] 无效。
    // 期望 @a[2] = 第二次命中的偏移（内部折成 0 起下标 1）。
    let src = r#"
rule off {
  strings:
    $a = { 01 }
  condition:
    #a == 3 and @a[1] == 0 and @a[2] == 2 and @a[3] == 5
}
"#;
    // $a = {01} 命中在 0, 2, 5
    let data = [0x01u8, 0xFF, 0x01, 0xFF, 0xFF, 0x01];
    assert_eq!(scan_one(src, &data), Ternary::True);
    // @a[2] 若错成 0 起下标会解成 5 ≠ 2 → False（钉死回归）
    let bad = src.replace("@a[2] == 2", "@a[2] == 5");
    assert_eq!(scan_one(&bad, &data), Ternary::False);
    // @a[0] 官方无效：如实 Unsupported（不猜）
    let zero = src.replace("@a[1] == 0", "@a[0] == 0");
    assert!(matches!(
        parse_rules(&zero).unwrap().rules[0],
        RuleOutcome::Unsupported { .. }
    ));
}

#[test]
fn module_refs_are_unknown_not_false() {
    // pe.* 记名弃权：Kleene U ∨ F = U（绝不假未命中）；T 吞弃权照常报警
    let src = r#"
rule mods {
  strings:
    $a = { 4D }
  condition:
    pe.number_of_sections > 5 or $a
}
"#;
    assert_eq!(scan_one(src, &[0x4D]), Ternary::True, "T ∨ U = T");
    assert_eq!(scan_one(src, &[0x00]), Ternary::Unknown, "U ∨ F = U");
}

#[test]
fn opaque_function_is_unknown() {
    let src = r#"
rule fn {
  strings:
    $a = { 4D }
  condition:
    math.entropy(0, filesize) > 7 or $a
}
"#;
    assert_eq!(scan_one(src, &[0x4D]), Ternary::True);
    assert_eq!(scan_one(src, &[0x00]), Ternary::Unknown);
}

#[test]
fn xor_modifier_matches() {
    // xor(k)：needle 逐字节 XOR 后扫描
    let keyed = r#"
rule x {
  strings:
    $a = "AZ" xor(0x20)
  condition:
    $a
}
"#;
    assert_eq!(scan_one(keyed, b"--az--"), Ternary::True, "'AZ'^0x20='az'");
    assert_eq!(
        scan_one(keyed, b"--AZ--"),
        Ternary::False,
        "明文形态不再命中"
    );
    // 裸 xor：全键 0x00..=0xFF
    let all = r#"
rule y {
  strings:
    $b = "hi" xor
  condition:
    $b
}
"#;
    let k1: Vec<u8> = b"hi".iter().map(|b| b ^ 0x01).collect();
    assert_eq!(scan_one(all, &k1), Ternary::True, "密钥 0x01 展开");
}

#[test]
fn fullword_requires_word_boundary() {
    let ascii = r#"
rule w {
  strings:
    $a = "cat" fullword
  condition:
    $a
}
"#;
    assert_eq!(scan_one(ascii, b"cat dog"), Ternary::True);
    assert_eq!(scan_one(ascii, b"scatter"), Ternary::False, "词中命中不算");
    // 下划线不算词字符（官方语义，yara-python 实测判定——与正则 \b 不同）
    assert_eq!(
        scan_one(ascii, b"a_cat"),
        Ternary::True,
        "下划线邻接 = fullword 可命中"
    );
    let wide = r#"
rule ww {
  strings:
    $w = "cat" wide fullword
  condition:
    $w
}
"#;
    let widestr: Vec<u8> = "cat".encode_utf16().flat_map(|u| u.to_le_bytes()).collect();
    let mut pref = "s"
        .encode_utf16()
        .flat_map(|u| u.to_le_bytes())
        .collect::<Vec<u8>>();
    pref.extend_from_slice(&widestr);
    assert_eq!(scan_one(wide, &widestr), Ternary::True);
    assert_eq!(scan_one(wide, &pref), Ternary::False, "宽字符邻接不算");
}

#[test]
fn base64_and_private_are_unscannable() {
    for modifier in ["base64", "private"] {
        let src = format!(
            r#"
rule u {{
  strings:
    $a = "foo" {modifier}
  condition:
    $a
}}
"#
        );
        let out = parse_rules(&src).unwrap();
        let RuleOutcome::Parsed(rule) = &out.rules[0] else {
            panic!("{modifier} 修饰应可解析为 Unscannable");
        };
        assert!(matches!(rule.patterns()[0].1, Pattern::Unscannable(_)));
        let report = SigScan::new(std::slice::from_ref(rule)).scan(b"foo", false);
        assert_eq!(report.rules[0].value, Ternary::Unknown);
    }
}

#[test]
fn arithmetic_bails_whole_rule() {
    // 算术表达式：如实 Unsupported（fail-closed 的另一半——不装作能评估）
    let src = r#"
rule arith {
  strings:
    $a = "x"
  condition:
    filesize + 1 < 100 and $a
}
"#;
    let out = parse_rules(src).unwrap();
    assert!(
        matches!(out.rules[0], RuleOutcome::Unsupported { .. }),
        "算术应使规则 Unsupported"
    );
}

/// F-02/M15 回归：任何 Unicode 空白都不能让 `lex` 原地死循环（否则整进程 100% CPU）。
/// 用子线程 + `recv_timeout` 把「死循环」变成「断言失败」——测试不挂死，变异门也能判「抓住」。
#[test]
fn lexer_terminates_on_every_unicode_whitespace() {
    use std::sync::mpsc;
    use std::time::Duration;
    // Unicode White_Space 全集（含 \x0b \x0c NBSP U+3000 U+2028 U+2029 …）
    const WS: &[char] = &[
        '\u{09}', '\u{0a}', '\u{0b}', '\u{0c}', '\u{0d}', '\u{20}', '\u{85}', '\u{a0}',
        '\u{1680}', '\u{2000}', '\u{2001}', '\u{2002}', '\u{2003}', '\u{2004}', '\u{2005}',
        '\u{2006}', '\u{2007}', '\u{2008}', '\u{2009}', '\u{200a}', '\u{2028}', '\u{2029}',
        '\u{202f}', '\u{205f}', '\u{3000}',
    ];
    for &c in WS {
        // 单字符 / 规则体内 / 关键字中间，三种位置都必须有限步返回
        for src in [
            c.to_string(),
            format!("rule a {{ {c} condition: true }}"),
            format!("rule{c}a {{ condition: true }}"),
        ] {
            let (tx, rx) = mpsc::channel();
            let probe = src.clone();
            std::thread::spawn(move || {
                let _ = parse_rules(&probe);
                let _ = tx.send(());
            });
            assert!(
                rx.recv_timeout(Duration::from_secs(2)).is_ok(),
                "lexer 在输入 {src:?} 上没有终止（死循环回归）"
            );
        }
    }
}
