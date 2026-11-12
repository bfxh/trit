//! YARA 词法器终止性回归（从 tests/yara.rs 拆出，控制单文件行数预算）。

use trit_re::yaraparse::parse_rules;

/// F-02/M15 回归：任何 Unicode 空白都不能让 `lex` 原地死循环（否则整进程 100% CPU）。
/// 用子线程 + `recv_timeout` 把「死循环」变成「断言失败」——测试不挂死，变异门也能判「抓住」。
#[test]
fn lexer_terminates_on_every_unicode_whitespace() {
    use std::sync::mpsc;
    use std::time::Duration;
    // Unicode White_Space 全集（含 \x0b \x0c NBSP U+3000 U+2028 U+2029 …）
    const WS: &[char] = &[
        '\u{09}', '\u{0a}', '\u{0b}', '\u{0c}', '\u{0d}', '\u{20}', '\u{85}', '\u{a0}', '\u{1680}',
        '\u{2000}', '\u{2001}', '\u{2002}', '\u{2003}', '\u{2004}', '\u{2005}', '\u{2006}',
        '\u{2007}', '\u{2008}', '\u{2009}', '\u{200a}', '\u{2028}', '\u{2029}', '\u{202f}',
        '\u{205f}', '\u{3000}',
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
