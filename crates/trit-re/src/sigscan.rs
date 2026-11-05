//! # sigscan —— YARA 子集的三值匹配（路线图 N4 第一层）
//!
//! 恶意软件签名匹配的核心子集：**hex 串（通配 + 跳跃）、文本串
//! （ascii/nocase/wide）、声明式条件**（All/Any/Count/At/In）。
//!
//! ## 三值纪律（与验收判据逐字对应）
//!
//! **"未命中"只在完整扫描后给出**：命中 → [`Ternary::True`]（命中不依赖
//! 完整性——看到就是看到）；完整扫描且未命中 → [`Ternary::False`]；
//! **输入截断**（调用方声明，例如文件读取中断）→ [`Ternary::Unknown`]——
//! 截断的没看见不能区分"真的没有"和"没看见"，与 [`crate::functions`]
//! 共现判定同一条 fail-closed 纪律。整体裁决 = 各规则的 Kleene OR
//! （报警语义：任一命中即报警；`False ∨ Unknown = Unknown`）。
//!
//! ## 已知边界（诚实记录的子集范围）
//!
//! - 不是 YARA 文本规则的解析器：规则经 Rust API 构造（解析规则文件是
//!   独立层）；与官方 YARA 的命中集合对拍（N4 验收的另一半）等规则文件
//!   解析 + 真实样本批次就位后做（N1 口径）。
//! - hex 只认 `??` 通配与 `[n]`/`[n-m]` 跳跃；文本大小写折叠仅 ASCII；
//!   wide 只产 UTF-16LE 变体（无 `fullword` 语义）。
//! - 每模式命中数上限 [`MAX_HITS`]（1024），跳跃上限 [`MAX_JUMP`]，
//!   单模式步数预算 [`SCAN_BUDGET`]（跳跃回溯在对抗数据上会指数爆炸——
//!   预算耗尽 = 扫描不完整 = 规则裁决 `Unknown`，与截断输入同一纪律）；
//!   病态模式在收割侧截断，**不挂起**。
//!
//! ```
//! use trit_re::sigscan::{self, Condition, Pattern, Rule, SigScan};
//! use trit_core::value::Ternary;
//!
//! let rules = vec![
//!     Rule::new("mz_peek")
//!         .push("hdr", Pattern::hex("4D 5A ?? 00").unwrap())
//!         .condition(Condition::All),
//! ];
//! let report = SigScan::new(&rules).scan(&[0x4D, 0x5A, 0x90, 0x00], false);
//! assert_eq!(report.rules[0].value, Ternary::True);
//! assert_eq!(report.value, Ternary::True);
//! // 截断输入：未命中不得判 False
//! let report = SigScan::new(&rules).scan(&[0x4D], true);
//! assert_eq!(report.value, Ternary::Unknown);
//! ```

use trit_core::value::Ternary;

/// 每个模式保留的命中数上限（病态输入护栏；计数受影响时如实截断）。
pub const MAX_HITS: usize = 1024;
/// 单次跳跃的最大跨度（`[n-m]` 的 m 上限；YARA 自身也有同类上限）。
pub const MAX_JUMP: u32 = 0x1000;

/// hex 串的单个字节项：字面量或 `??` 通配。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HexByte {
    Lit(u8),
    Wild,
}

/// 一次跳跃：`[n]` → `min == max == n`；`[n-m]` → 闭区间。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Jump {
    pub min: u32,
    pub max: u32,
}

/// hex 串模式：首段字节 + （跳跃, 字节段）序列——
/// `"4D 5A [2-4] 50"` → lead=`4D 5A`，rest=`[(2..=4, "50")]`。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HexPattern {
    lead: Vec<HexByte>,
    rest: Vec<(Jump, Vec<HexByte>)>,
}

/// 文本串模式：修饰符在构造时展开成字节变体
/// （ascii → 原样；wide → UTF-16LE；两者都开 → 两个变体都试）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TextPattern {
    variants: Vec<Vec<u8>>,
    nocase: bool,
}

/// 字符串模式。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Pattern {
    Hex(HexPattern),
    Text(TextPattern),
}

impl Pattern {
    /// 解析 hex 串：两位十六进制（空格分隔可选）、`??` 通配、
    /// `[n]` / `[n-m]` 跳跃（`[2-4]` 与 `[2 - 4]` 两种写法都认）。语法错误如实报错。
    pub fn hex(spec: &str) -> Result<Self, String> {
        let mut lead: Vec<HexByte> = Vec::new();
        let mut rest: Vec<(Jump, Vec<HexByte>)> = Vec::new();
        let mut current = &mut lead;
        // 方括号两侧统一插空格：`[2-4]` 与 `[ 2 - 4 ]` 同一处理路径
        let padded = spec.replace('[', " [ ").replace(']', " ] ");
        let mut tokens = padded.split_whitespace();
        while let Some(tok) = tokens.next() {
            if tok == "[" {
                // 收集到 "]" 为止，拼成跳跃区间（`2` / `2-4` / `2 - 4`）
                let mut span = String::new();
                let closed = loop {
                    match tokens.next() {
                        Some("]") => break true,
                        Some(t) => {
                            if !span.is_empty() {
                                span.push(' ');
                            }
                            span.push_str(t);
                        }
                        None => break false,
                    }
                };
                if !closed {
                    return Err("跳跃未闭合".into());
                }
                let (lo, hi) = match span.split_once('-') {
                    Some((a, b)) => (parse_num(a.trim())?, parse_num(b.trim())?),
                    None => {
                        let n = parse_num(span.trim())?;
                        (n, n)
                    }
                };
                if hi < lo {
                    return Err(format!("跳跃区间反了：{lo}-{hi}"));
                }
                if hi > MAX_JUMP {
                    return Err(format!("跳跃超过上限 {MAX_JUMP}"));
                }
                rest.push((Jump { min: lo, max: hi }, Vec::new()));
                current = &mut rest.last_mut().expect("刚 push").1;
                continue;
            }
            if tok == "]" {
                return Err("多余的 ']'".into());
            }
            current.push(parse_hex_byte(tok)?);
        }
        if lead.is_empty() {
            return Err("hex 串不能以跳跃开头".into());
        }
        if rest.iter().any(|(_, chunk)| chunk.is_empty()) {
            return Err("跳跃后必须有字节段（以跳跃结尾不合法）".into());
        }
        Ok(Pattern::Hex(HexPattern { lead, rest }))
    }

    /// 文本串：`nocase`（ASCII 大小写折叠，两形态都生效）；`wide` 对齐
    /// YARA 语义——`false` = 仅 ASCII 原样，`true` = 仅 UTF-16LE 变体。
    pub fn text(needle: &str, nocase: bool, wide: bool) -> Self {
        let variants = if wide {
            vec![
                needle
                    .encode_utf16()
                    .flat_map(|u| u.to_le_bytes())
                    .collect(),
            ]
        } else {
            vec![needle.as_bytes().to_vec()]
        };
        Pattern::Text(TextPattern { variants, nocase })
    }
}

fn parse_num(tok: &str) -> Result<u32, String> {
    tok.parse::<u32>()
        .map_err(|_| format!("跳跃数字不合法：{tok:?}"))
}

fn parse_hex_byte(tok: &str) -> Result<HexByte, String> {
    if tok == "??" {
        return Ok(HexByte::Wild);
    }
    if tok.len() != 2 || !tok.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(format!("hex 字节不合法：{tok:?}"));
    }
    Ok(HexByte::Lit(
        u8::from_str_radix(tok, 16).map_err(|e| e.to_string())?,
    ))
}

/// 条件：声明式子集 + YARA 规则文件解析（N4 后半）产出的表达式树。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Condition {
    /// 全部命中。
    All,
    /// 任一命中。
    Any,
    /// 至少 n 条命中。
    Count(usize),
    /// 指定串的首次命中恰在 `offset`。
    At { index: usize, offset: u64 },
    /// 指定串有命中落在 `[lo, hi]`。
    In { index: usize, lo: u64, hi: u64 },
    /// 单串引用 `$id`：有命中即真（解析器按规则串表解析成下标）。
    Ref(usize),
    /// 逻辑与。
    And(Vec<Condition>),
    /// 逻辑或。
    Or(Vec<Condition>),
    /// 逻辑非。
    Not(Box<Condition>),
    /// `any of them/($x*)`：前缀集合里任一命中。
    OfAny { prefix: Option<String> },
    /// `all of them/($x*)`：前缀集合里全部命中。
    OfAll { prefix: Option<String> },
    /// `n of them/($x*)`：前缀集合里至少 n 条命中。
    OfCount { n: usize, prefix: Option<String> },
}

/// 一条签名规则：名字 + 字符串表 + 条件。
#[derive(Debug, Clone)]
pub struct Rule {
    pub name: String,
    patterns: Vec<(String, Pattern)>,
    condition: Condition,
}

impl Rule {
    pub fn new(name: &str) -> Self {
        Self {
            name: name.to_string(),
            patterns: Vec::new(),
            condition: Condition::All,
        }
    }

    pub fn push(mut self, id: &str, pattern: Pattern) -> Self {
        self.patterns.push((id.to_string(), pattern));
        self
    }

    pub fn condition(mut self, condition: Condition) -> Self {
        self.condition = condition;
        self
    }

    /// 供同 crate 的解析器装配（字段保持私有，外部仍走 new/push/condition）。
    pub(crate) fn from_parts(
        name: String,
        patterns: Vec<(String, Pattern)>,
        condition: Condition,
    ) -> Self {
        Self {
            name,
            patterns,
            condition,
        }
    }

    /// 串表（`$id` 去掉 `$` 后的 id + 模式）——解析器据此把 `$id` 引用
    /// 解析成下标。
    pub fn patterns(&self) -> &[(String, Pattern)] {
        &self.patterns
    }

    pub fn condition_ref(&self) -> &Condition {
        &self.condition
    }
}

/// 一条规则的裁决。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuleVerdict {
    pub rule: String,
    pub value: Ternary,
    /// 命中的模式（`模式id, 命中偏移` 列表；每模式最多 [`MAX_HITS`] 个）。
    pub hits: Vec<(String, u64)>,
}

/// 一次扫描的报告：逐规则裁决 + 整体（Kleene OR 报警语义）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SigReport {
    pub rules: Vec<RuleVerdict>,
    pub value: Ternary,
}

/// 扫描器（规则集不可变，可反复扫描）。
pub struct SigScan<'a> {
    rules: &'a [Rule],
}

impl<'a> SigScan<'a> {
    pub fn new(rules: &'a [Rule]) -> Self {
        Self { rules }
    }

    /// 扫描一段字节。`truncated` = 调用方声明输入不完整（读取中断/分片）。
    pub fn scan(&self, data: &[u8], truncated: bool) -> SigReport {
        let mut rules = Vec::new();
        for rule in self.rules {
            // 每个模式的命中偏移（首个起，最多 MAX_HITS 个）+ 扫描完成度
            //（hex 预算耗尽 = 不完整——"未命中只在完整扫描后给出"）
            let mut occurrences: Vec<(String, Vec<u64>)> = Vec::new();
            let mut complete = true;
            for (id, pattern) in &rule.patterns {
                let (hits, pattern_complete) = match pattern {
                    Pattern::Hex(hp) => find_hex(data, hp),
                    Pattern::Text(tp) => (find_text(data, tp), true),
                };
                complete &= pattern_complete;
                occurrences.push((id.clone(), hits));
            }
            let met = eval(&rule.condition, &occurrences);
            let hits: Vec<(String, u64)> = occurrences
                .iter()
                .flat_map(|(id, offs)| offs.iter().map(move |o| (id.clone(), *o)))
                .collect();
            let value = if met {
                Ternary::True
            } else if truncated || !complete {
                Ternary::Unknown
            } else {
                Ternary::False
            };
            rules.push(RuleVerdict {
                rule: rule.name.clone(),
                value,
                hits,
            });
        }
        // 整体 = Kleene OR：任一 True → True；否则有 Unknown → Unknown；
        // 全 False → False（报警语义：截断的沉默不是清白）。
        let mut value = Ternary::False;
        for r in &rules {
            value = match (value, r.value) {
                (Ternary::True, _) | (_, Ternary::True) => Ternary::True,
                (Ternary::Unknown, _) | (_, Ternary::Unknown) => Ternary::Unknown,
                _ => Ternary::False,
            };
        }
        SigReport { rules, value }
    }
}

/// 条件求值（基于每模式的命中偏移表）。
fn eval(condition: &Condition, occurrences: &[(String, Vec<u64>)]) -> bool {
    let set = |prefix: &Option<String>| -> Vec<&(String, Vec<u64>)> {
        occurrences
            .iter()
            .filter(|(id, _)| prefix.as_deref().is_none_or(|p| id.starts_with(p)))
            .collect()
    };
    match condition {
        Condition::All => occurrences.iter().all(|(_, offs)| !offs.is_empty()),
        Condition::Any => occurrences.iter().any(|(_, offs)| !offs.is_empty()),
        Condition::Count(n) => occurrences.iter().filter(|(_, o)| !o.is_empty()).count() >= *n,
        Condition::At { index, offset } => occurrences
            .get(*index)
            .is_some_and(|(_, offs)| offs.first() == Some(offset)),
        Condition::In { index, lo, hi } => occurrences
            .get(*index)
            .is_some_and(|(_, offs)| offs.iter().any(|o| o >= lo && o <= hi)),
        Condition::Ref(index) => occurrences
            .get(*index)
            .is_some_and(|(_, offs)| !offs.is_empty()),
        Condition::And(conds) => conds.iter().all(|c| eval(c, occurrences)),
        Condition::Or(conds) => conds.iter().any(|c| eval(c, occurrences)),
        Condition::Not(inner) => !eval(inner, occurrences),
        Condition::OfAny { prefix } => set(prefix).iter().any(|(_, o)| !o.is_empty()),
        Condition::OfAll { prefix } => {
            let set = set(prefix);
            !set.is_empty() && set.iter().all(|(_, o)| !o.is_empty())
        }
        Condition::OfCount { n, prefix } => {
            set(prefix).iter().filter(|(_, o)| !o.is_empty()).count() >= *n
        }
    }
}

/// 单模式扫描的步数预算（段尝试 + 首段候选比较合计）。跳跃模式的回溯在
/// 对抗数据上会指数爆炸（`{ 90 [0-4096] 90 [0-4096] … }` 配全 0x90 数据），
/// 预算封顶 = **不挂起**（"任何输入不 panic 不挂起"不变量的扫描面）。
/// 耗尽 = 扫描不完整 → 规则裁决按 `Unknown` 落账（**"未命中只在完整扫描
/// 后给出"的组成面**：截断的沉默不是清白，预算耗尽的沉默同样不是）；
/// YARA 用时间超时，我们用确定性预算——同样的 fail-closed，可复算。
pub const SCAN_BUDGET: u64 = 1_000_000;

/// hex 模式匹配结果：命中 + 扫描是否完整（预算耗尽 = 不完整）。
fn find_hex(data: &[u8], hp: &HexPattern) -> (Vec<u64>, bool) {
    let mut hits = Vec::new();
    let mut budget = SCAN_BUDGET;
    let mut complete = true;
    for pos in 0..data.len() {
        budget = budget.saturating_sub(1);
        if budget == 0 {
            complete = false;
            break; // 预算耗尽：该模式停止搜索（不挂起）
        }
        if chunk_matches(data, pos, &hp.lead)
            && extend_rest(data, &hp.rest, pos + hp.lead.len(), &mut budget)
        {
            hits.push(pos as u64);
            if hits.len() >= MAX_HITS {
                break;
            }
        }
    }
    (hits, complete)
}

/// 从 `pos` 起，按 `rest` 的（跳跃, 段）序列依次匹配——每段起点 ∈
/// `[前段末尾+min, 前段末尾+max]`（YARA 跳跃语义），全部段落位即成功。
/// 每次段尝试扣一步预算，耗尽即整条放弃（fail-closed 不挂起）。
fn extend_rest(data: &[u8], rest: &[(Jump, Vec<HexByte>)], pos: usize, budget: &mut u64) -> bool {
    let Some((jump, chunk)) = rest.first() else {
        return true; // 没有后续段：首段落位即整条命中
    };
    let start = pos.saturating_add(jump.min as usize);
    let last = pos.saturating_add(jump.max as usize);
    for seg_start in start..=last {
        *budget = budget.saturating_sub(1);
        if *budget == 0 {
            return false; // 预算耗尽：放弃（不挂起）
        }
        if seg_start + chunk.len() > data.len() {
            break;
        }
        if chunk_matches(data, seg_start, chunk)
            && extend_rest(data, &rest[1..], seg_start + chunk.len(), budget)
        {
            return true;
        }
    }
    false
}

fn chunk_matches(data: &[u8], pos: usize, chunk: &[HexByte]) -> bool {
    pos + chunk.len() <= data.len()
        && chunk.iter().enumerate().all(|(i, b)| match b {
            HexByte::Lit(v) => data[pos + i] == *v,
            HexByte::Wild => true,
        })
}

/// 文本匹配：逐变体找全部命中（nocase = ASCII 折叠）。
fn find_text(data: &[u8], tp: &TextPattern) -> Vec<u64> {
    let mut hits = Vec::new();
    for variant in &tp.variants {
        let folded = if tp.nocase {
            variant.to_ascii_lowercase()
        } else {
            variant.clone()
        };
        if folded.is_empty() {
            continue;
        }
        let mut from = 0usize;
        while from + folded.len() <= data.len() {
            let window = if tp.nocase {
                data[from..from + folded.len()].to_ascii_lowercase()
            } else {
                data[from..from + folded.len()].to_vec()
            };
            if window == folded {
                hits.push(from as u64);
                if hits.len() >= MAX_HITS {
                    return hits;
                }
                from += folded.len();
            } else {
                from += 1;
            }
        }
    }
    hits.sort_unstable();
    hits.dedup();
    hits
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rule(name: &str, id: &str, pattern: Pattern) -> Rule {
        Rule::new(name).push(id, pattern)
    }

    #[test]
    fn hex_literal_and_wildcard() {
        let p = Pattern::hex("4D 5A").unwrap();
        let data = [0x00, 0x4D, 0x5A, 0x90];
        let report = SigScan::new(&[rule("r", "a", p)]).scan(&data, false);
        assert_eq!(report.rules[0].value, Ternary::True);
        assert_eq!(report.rules[0].hits, vec![("a".into(), 1)]);
        let wild = Pattern::hex("4D ?? 90").unwrap();
        let data = [0x4D, 0x11, 0x90];
        assert_eq!(
            SigScan::new(&[rule("r", "a", wild)])
                .scan(&data, false)
                .rules[0]
                .value,
            Ternary::True
        );
    }

    #[test]
    fn hex_jump_window() {
        // 4D 5A [2-4] 50：间隔 2..=4 字节
        let p = Pattern::hex("4D 5A [2-4] 50").unwrap();
        let ok = [0x4D, 0x5A, 1, 2, 0x50]; // 间隔 2
        let ok4 = [0x4D, 0x5A, 1, 2, 3, 4, 0x50]; // 间隔 4
        let too_far = [0x4D, 0x5A, 1, 2, 3, 4, 5, 0x50]; // 间隔 5
        let scan = |d: &[u8]| SigScan::new(&[rule("r", "a", p.clone())]).scan(d, false);
        assert_eq!(scan(&ok).rules[0].value, Ternary::True);
        assert_eq!(scan(&ok4).rules[0].value, Ternary::True);
        assert_eq!(scan(&too_far).rules[0].value, Ternary::False);
        // 语法：[n] 定长
        let fixed = Pattern::hex("4D 5A [2] 50").unwrap();
        let scan =
            |d: &[u8]| SigScan::new(&[Rule::new("r").push("a", fixed.clone())]).scan(d, false);
        assert_eq!(scan(&ok).rules[0].value, Ternary::True);
        assert_eq!(scan(&ok4).rules[0].value, Ternary::False, "[2] 不容间隔 4");
    }

    #[test]
    fn hex_spec_errors_fail_closed() {
        assert!(Pattern::hex("4D 5A [2-").is_err(), "跳跃未闭合");
        assert!(Pattern::hex("4D 5A [4-2] 50").is_err(), "区间反了");
        assert!(Pattern::hex("4D [2-4]").is_err(), "跳跃后必须有字节段");
        assert!(Pattern::hex("4D 5A [2-4]").is_err(), "以跳跃结尾不合法");
        assert!(Pattern::hex("4D 5A [0x10] 50").is_err(), "非法数字");
        assert!(Pattern::hex("4D ZZ").is_err(), "非法字节");
        assert!(Pattern::hex("?? ??").is_ok(), "通配开头合法");
    }

    #[test]
    fn text_nocase_and_wide() {
        let p = Pattern::text("CreateRemoteThread", false, false);
        let data = b"xx CreateRemoteThread yy";
        assert_eq!(
            SigScan::new(&[rule("r", "a", p.clone())])
                .scan(data, false)
                .rules[0]
                .value,
            Ternary::True
        );
        let nocase = Pattern::text("createremotethread", true, false);
        assert_eq!(
            SigScan::new(&[Rule::new("r").push("a", nocase)])
                .scan(data, false)
                .rules[0]
                .value,
            Ternary::True,
            "nocase = ASCII 折叠"
        );
        // wide：仅 UTF-16LE 变体（对齐 YARA `wide` 语义：ascii 变体不再命中）
        let wide = Pattern::text("AZ", false, true);
        let wdata: Vec<u8> = "AZ".encode_utf16().flat_map(|u| u.to_le_bytes()).collect();
        assert_eq!(
            SigScan::new(&[Rule::new("r").push("a", wide.clone())])
                .scan(&wdata, false)
                .rules[0]
                .value,
            Ternary::True
        );
        assert_eq!(
            SigScan::new(&[Rule::new("r").push("a", wide)])
                .scan(b"AZ", false)
                .rules[0]
                .value,
            Ternary::False,
            "wide=true 时 ASCII 形态不命中"
        );
    }

    #[test]
    fn conditions_all_any_count_at_in() {
        let a = Pattern::hex("01").unwrap();
        let b = Pattern::hex("02").unwrap();
        let data = [0x01, 0xFF, 0x02];
        let s = |c: Condition| {
            SigScan::new(&[Rule::new("r")
                .push("a", a.clone())
                .push("b", b.clone())
                .condition(c)])
            .scan(&data, false)
            .rules[0]
                .value
        };
        assert_eq!(s(Condition::All), Ternary::True);
        assert_eq!(s(Condition::Any), Ternary::True);
        assert_eq!(s(Condition::Count(2)), Ternary::True);
        assert_eq!(s(Condition::Count(3)), Ternary::False);
        // $a at 1（首次命中恰在偏移 1？a 首次命中在 0）→ False
        assert_eq!(
            s(Condition::At {
                index: 0,
                offset: 1
            }),
            Ternary::False
        );
        assert_eq!(
            s(Condition::At {
                index: 0,
                offset: 0
            }),
            Ternary::True
        );
        // $b in (1..=2)：b 命中在 2
        assert_eq!(
            s(Condition::In {
                index: 1,
                lo: 1,
                hi: 2
            }),
            Ternary::True
        );
        assert_eq!(
            s(Condition::In {
                index: 1,
                lo: 0,
                hi: 1
            }),
            Ternary::False
        );
    }

    #[test]
    fn truncated_input_never_false() {
        // 验收判据的本体："未命中"只在完整扫描后给出
        let p = Pattern::hex("4D 5A 90 00").unwrap();
        let rules = [rule("r", "a", p)];
        let report = SigScan::new(&rules).scan(&[0x4D], true);
        assert_eq!(report.rules[0].value, Ternary::Unknown);
        assert_eq!(report.value, Ternary::Unknown);
        // 命中不依赖完整性：截断但有命中 → True
        let report = SigScan::new(&rules).scan(&[0x4D, 0x5A, 0x90, 0x00], true);
        assert_eq!(report.rules[0].value, Ternary::True);
        // 完整 + 未命中 → False
        let report = SigScan::new(&rules).scan(&[0x00, 0x00], false);
        assert_eq!(report.rules[0].value, Ternary::False);
    }

    #[test]
    fn overall_is_kleene_or() {
        let hit = rule("hit", "a", Pattern::hex("01").unwrap());
        let miss = rule("miss", "b", Pattern::hex("99").unwrap());
        let rules = [hit, miss];
        // True ∨ False = True（报警语义）
        let report = SigScan::new(&rules).scan(&[0x01], false);
        assert_eq!(report.value, Ternary::True);
        // False ∨ False = False
        let report = SigScan::new(&rules).scan(&[0x00], false);
        assert_eq!(report.value, Ternary::False);
    }

    #[test]
    fn hits_are_capped() {
        // 0x00 填满 + 模式 00 → 命中数封顶 MAX_HITS
        let data = vec![0x00u8; 5000];
        let rules = [rule("r", "a", Pattern::hex("00").unwrap())];
        let report = SigScan::new(&rules).scan(&data, false);
        assert_eq!(report.rules[0].hits.len(), MAX_HITS);
    }

    #[test]
    fn pathological_jump_pattern_yields_unknown_not_hang() {
        // 全 0x90 数据 + 永不成立的跳跃链：朴素回溯 ≈ 4096³ 条路径（挂起）；
        // 预算封顶后立即返回，且耗尽 = 扫描不完整 → Unknown 而非 False
        //（"未命中只在完整扫描后给出"——预算耗尽的沉默不是清白）
        let data = vec![0x90u8; 8192];
        let p = Pattern::hex("90 [0-4096] 00 [0-4096] 00 [0-4096] 00").unwrap();
        let report = SigScan::new(&[Rule::new("r").push("a", p)]).scan(&data, false);
        assert_eq!(
            report.rules[0].value,
            Ternary::Unknown,
            "预算耗尽 = 不完整扫描 → Unknown（绝不挂起、绝不误判 False）"
        );
        assert_eq!(report.value, Ternary::Unknown);
    }

    #[test]
    fn normal_patterns_still_false_within_budget() {
        // 小数据正常模式：预算远未耗尽 → MISS 照旧判 False（语义不漂移）
        let data = vec![0x90u8; 64];
        let p = Pattern::hex("90 [0-8] 00 [0-8] 00 [0-8] 00").unwrap();
        let report = SigScan::new(&[Rule::new("r").push("a", p)]).scan(&data, false);
        assert_eq!(report.rules[0].value, Ternary::False, "小数据完整扫描");
    }
}
