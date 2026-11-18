//! 字符串函数（探针定案）：按**字节**比较（不看 UTF-8）；`i*` 只折叠 ASCII
//! 大小写（`"\xC4" iequals "\xE4"` 为假）；空针 `contains`/`startswith`/
//! `endswith` 为真；针比串长为假。`matches` 复用现有 Pike VM：非锚定搜索，
//! 起点只试 `0..len`；空串在位置 0 试一次。
#![allow(clippy::missing_const_for_thread_local)] // clippy 1.99 误报 const block

use super::eval::V;
use super::{Matcher, StrFn};
use crate::sigscan::SCAN_BUDGET;
use crate::sigscan::regex::{Run, run_at};
use std::cell::Cell;

thread_local! {
    /// 正则步数余额：求值器在跑正则前把剩余预算换成步数放进来，跑完再
    /// 取回（见 `Env::metered`）——循环里反复跑慢正则不能白跑。
    static ALLOWANCE: Cell<u64> = const { Cell::new(u64::MAX) };
}

pub(super) fn set_allowance(steps: u64) {
    ALLOWANCE.set(steps);
}

pub(super) fn allowance() -> u64 {
    ALLOWANCE.get()
}

fn find(hay: &[u8], needle: &[u8], eq: impl Fn(u8, u8) -> bool) -> bool {
    needle.is_empty()
        || hay
            .windows(needle.len())
            .any(|window| window.iter().zip(needle).all(|(a, b)| eq(*a, *b)))
}

fn exact(a: u8, b: u8) -> bool {
    a == b
}

fn folded(a: u8, b: u8) -> bool {
    a.eq_ignore_ascii_case(&b)
}

/// 字符串序（`== != < <= > >=`）：公共前缀之后，短者为小；首个相异字节按
/// **有符号 char** 比（官方 `ss_compare` 用 `char`，x86-64 上有符号——探针：
/// `"\xff" < "a"` 为真）。官方在 `char` 无符号的平台（如 aarch64 Linux）上
/// 结果会不同；这里以 x86-64 的 yara-python 4.5.4 为准。
pub(super) fn compare(a: &[u8], b: &[u8]) -> std::cmp::Ordering {
    let signed = |bytes: &[u8]| bytes.iter().map(|byte| *byte as i8).collect::<Vec<_>>();
    signed(a).cmp(&signed(b))
}

/// 两个已定义字符串上的字符串函数。
pub(super) fn apply(op: StrFn, hay: &[u8], needle: &[u8]) -> bool {
    let head = || hay.get(..needle.len());
    let tail = || hay.len().checked_sub(needle.len()).map(|at| &hay[at..]);
    match op {
        StrFn::Contains => find(hay, needle, exact),
        StrFn::IContains => find(hay, needle, folded),
        StrFn::StartsWith => head() == Some(needle),
        StrFn::IStartsWith => head().is_some_and(|h| h.eq_ignore_ascii_case(needle)),
        StrFn::EndsWith => tail() == Some(needle),
        StrFn::IEndsWith => tail().is_some_and(|t| t.eq_ignore_ascii_case(needle)),
        StrFn::IEquals => hay.eq_ignore_ascii_case(needle),
    }
}

/// `<串> matches /re/`：有命中 = 真；完整搜索无命中 = 假；预算耗尽 = 弃权。
pub(super) fn matches<'a>(re: &Matcher, hay: &[u8]) -> V<'a> {
    let left = ALLOWANCE.get();
    let budget = left.min(SCAN_BUDGET);
    let mut visited = vec![0u64; re.pat.prog.len()];
    let mut steps = 0u64;
    let mut verdict = V::Bool(false);
    for start in 0..hay.len().max(1) {
        match run_at(hay, &re.pat, start, &mut visited, &mut steps, budget) {
            Run::Miss => continue,
            Run::Hit => verdict = V::Bool(true),
            Run::Exhausted => verdict = V::Unknown,
        }
        break; // 第一个命中就够；预算耗尽则停手
    }
    // 起步价 = 程序长度（每次调用都要清一张访问表）
    ALLOWANCE.set(left.saturating_sub(steps.saturating_add(re.pat.prog.len() as u64)));
    verdict
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ordering_uses_signed_chars_like_the_reference_build() {
        use std::cmp::Ordering::{Equal, Greater, Less};
        assert_eq!(compare(b"abc", b"abc"), Equal);
        assert_eq!(compare(b"ab", b"abc"), Less);
        assert_eq!(compare(b"b", b"abc"), Greater);
        assert_eq!(compare(b"", b""), Equal);
        // 高位字节是负数：排在所有 ASCII 之前
        assert_eq!(compare(b"\xff", b"a"), Less);
        assert_eq!(compare(b"\x80", b"\x7f"), Less);
        assert_eq!(compare(b"a\x00", b"a"), Greater);
    }

    fn m(body: &str, nocase: bool, hay: &[u8]) -> Option<bool> {
        match matches(&Matcher::compile(body, nocase).expect("正则可编译"), hay) {
            V::Bool(b) => Some(b),
            _ => None,
        }
    }

    #[test]
    fn string_functions_match_probed_edges() {
        use StrFn::*;
        // 空针恒真（连空串对空针）
        assert!(apply(Contains, b"abc", b""));
        assert!(apply(Contains, b"", b""));
        assert!(apply(IContains, b"", b""));
        assert!(apply(StartsWith, b"abc", b""));
        assert!(apply(EndsWith, b"abc", b""));
        assert!(!apply(Contains, b"", b"a"));
        // 针比串长 = 假
        assert!(!apply(Contains, b"abc", b"abcd"));
        assert!(!apply(StartsWith, b"abc", b"abcd"));
        assert!(!apply(IEndsWith, b"abc", b"abcd"));
        // 大小写
        assert!(!apply(Contains, b"aBc", b"b"));
        assert!(apply(IContains, b"aBc", b"b"));
        assert!(apply(IStartsWith, b"ABC", b"ab"));
        assert!(apply(IEndsWith, b"abC", b"BC"));
        assert!(apply(IEquals, b"abc", b"ABC"));
        assert!(!apply(IEquals, b"abc", b"ABCd"));
    }

    #[test]
    fn case_folding_is_ascii_only_and_bytes_are_opaque() {
        use StrFn::*;
        // Latin-1 的 Ä/ä 不折叠（官方 yr_lowercase 只管 ASCII）
        assert!(!apply(IEquals, b"\xC4", b"\xE4"));
        assert!(apply(IContains, b"\xC4", b"\xC4"));
        // NUL 与非 UTF-8 字节照常参与比较
        assert!(apply(Contains, b"a\x00b", b"\x00b"));
        assert!(apply(StartsWith, b"a\x00b", b"a\x00"));
        assert!(apply(EndsWith, b"\xff\xfe", b"\xfe"));
    }

    #[test]
    fn matches_follows_probed_search_semantics() {
        assert_eq!(m("b", false, b"abc"), Some(true));
        assert_eq!(m("^b", false, b"abc"), Some(false));
        assert_eq!(m("^a.c$", false, b"abc"), Some(true));
        assert_eq!(m("b", false, b"aBc"), Some(false));
        assert_eq!(m("b", true, b"aBc"), Some(true));
        assert_eq!(m("a.c", false, b"a\nc"), Some(false));
        assert_eq!(m("c$", false, b"abc"), Some(true));
        // 起点只试 0..len：行尾锚单独出现不命中非空串
        assert_eq!(m("$", false, b"abc"), Some(false));
        assert_eq!(m("x*$", false, b"abc"), Some(false));
        // 空串在位置 0 试一次
        assert_eq!(m("a*", false, b""), Some(true));
        assert_eq!(m("^$", false, b""), Some(true));
        assert_eq!(m("a", false, b""), Some(false));
    }
}
