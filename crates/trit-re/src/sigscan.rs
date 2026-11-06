//! # sigscan —— YARA 子集的三值匹配（路线图 N4 第一层）
//!
//! 恶意软件签名匹配的核心子集：**hex 串（通配 + 跳跃）、文本串
//! （ascii/nocase/wide/xor/fullword）、声明式条件**（All/Any/Count/At/In、
//! 数值比较 filesize/uintN/#a/@a、for-of 循环）。
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
//!   `ascii wide` 组合产双变体；xor 对 wide 展开后的字节逐位 XOR（该组合
//!   未与官方逐位对拍，等校准批次）；正则/base64/private 串**可解析不可
//!   扫描**——记原因、整条规则判 `Unknown`（fail-closed，不假未命中）；
//!   `entrypoint` 与模块/未知函数（`pe.*`、`math.*`）记名保留、求值恒
//!   `Unknown`。
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
/// （ascii → 原样；wide → UTF-16LE；`ascii wide` → 两个变体都试）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TextPattern {
    variants: Vec<TextVariant>,
    nocase: bool,
    /// XOR 密钥闭区间（`xor` → 全键 0x00..=0xFF；`xor(k)` / `xor(a-b)` → 子区间）。
    xor: Option<(u8, u8)>,
    /// `fullword`：命中两端不得邻接「字母数字下划线」（wide 按宽字符判）。
    fullword: bool,
}

/// 文本串的一个变体：字节 + 是否宽字符（fullword 的边界判定要用）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TextVariant {
    bytes: Vec<u8>,
    wide: bool,
}

/// 不可扫描的串形态（正则 / base64 / private 等）：带原因。
/// 扫描时整条规则按 `Unknown` 裁决——评估不了的串不能报"没命中"。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Unscannable {
    pub reason: String,
}

/// 字符串模式。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Pattern {
    Hex(HexPattern),
    Text(TextPattern),
    /// 可解析不可扫描（正则 / base64 / private）：整条规则判 `Unknown`。
    Unscannable(Unscannable),
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
        Self::text_ext(needle, nocase, wide, false, None, false)
    }

    /// 全参构造（解析器用）：`ascii_too` 与 `wide` 同开 = 双变体都试；
    /// `xor = Some((lo, hi))` 密钥闭区间展开（对变体最终字节逐位 XOR）；
    /// `fullword` 词边界。
    pub fn text_ext(
        needle: &str,
        nocase: bool,
        wide: bool,
        ascii_too: bool,
        xor: Option<(u8, u8)>,
        fullword: bool,
    ) -> Self {
        let ascii = TextVariant {
            bytes: needle.as_bytes().to_vec(),
            wide: false,
        };
        let wided = TextVariant {
            bytes: needle
                .encode_utf16()
                .flat_map(|u| u.to_le_bytes())
                .collect(),
            wide: true,
        };
        let variants = match (wide, ascii_too) {
            (false, _) => vec![ascii],
            (true, false) => vec![wided],
            (true, true) => vec![ascii, wided],
        };
        Pattern::Text(TextPattern {
            variants,
            nocase,
            xor,
            fullword,
        })
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

/// 数值比较算子。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CmpOp {
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
}

impl CmpOp {
    fn apply(self, a: u64, b: u64) -> bool {
        match self {
            CmpOp::Eq => a == b,
            CmpOp::Ne => a != b,
            CmpOp::Lt => a < b,
            CmpOp::Le => a <= b,
            CmpOp::Gt => a > b,
            CmpOp::Ge => a >= b,
        }
    }
}

/// 数值操作数：比较两侧的可求值项（`#a` / `@a[N]` / `filesize` /
/// `uintN(lit)` / 字面量 / 记名弃权项）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Operand {
    /// 整数字面量（十进制 / `0x` 十六进制 / `KB`·`MB` 后缀）。
    Lit(u64),
    /// `filesize`（截断输入上大小本身不完整 → 不可求值）。
    Filesize,
    /// `#a`：串的命中次数（按扫描报告，受 [`MAX_HITS`] 封顶——如实截断）。
    Count(usize),
    /// `@a[N]`：串第 N 次（0 起）命中的偏移；缺省首次。
    Offset { index: usize, nth: usize },
    /// `uintN(lit)`：偏移处读 N 字节小端整数；越界 = 不可求值。
    UintRead { size: u8, offset: u64 },
    /// `entrypoint`：需 PE 上下文，无联动 → 不可求值（`Unknown`，如实）。
    Entrypoint,
    /// 模块引用与未实现函数（`pe.number_of_sections`、`math.entropy(…)`）
    /// ——记原文，求值恒 `Unknown`。
    Opaque(String),
    /// for-of 体内的 `#$`：当前绑定串的命中次数。
    CurrentCount,
    /// for-of 体内的 `@$` / `@$[N]`：当前绑定串的命中偏移。
    CurrentOffset { nth: usize },
}

/// `for <bound> of <集合> : ( 体 )` 的量词。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ForBound {
    /// 至少 n 条命中串满足体。
    N(usize),
    /// 任一（≥1）。
    Any,
    /// 全部（集合为空 = 假，与 [`Condition::OfAll`] 同口径）。
    All,
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
    /// 指定串的首次命中恰在 `offset`（操作数可为字面量 / `entrypoint` 等）。
    At { index: usize, offset: Operand },
    /// 指定串有命中落在 `[lo, hi]`。
    In { index: usize, lo: u64, hi: u64 },
    /// 单串引用 `$id`：有命中即真（解析器按规则串表解析成下标）。
    Ref(usize),
    /// 数值比较 `lhs op rhs`（`filesize < 100KB`、`#a > 3`、`uint8(0) == 0x4D`）。
    Cmp {
        lhs: Operand,
        op: CmpOp,
        rhs: Operand,
    },
    /// `for bound of (set) : ( body )`——对集合中每条命中串求体。
    ForOf {
        bound: ForBound,
        prefix: Option<String>,
        body: Box<Condition>,
    },
    /// for-of 体内的 `$`（当前串有命中；体外出现 = 解析层拒绝）。
    CurrentRef,
    /// for-of 体内 `$ at X`：当前串的首次命中恰在 X。
    CurrentAt { offset: Operand },
    /// for-of 体内 `$ in (a..b)`：当前串有命中落在区间。
    CurrentIn { lo: u64, hi: u64 },
    /// `true` / `false` 字面量。
    Const(bool),
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
            //（hex/文本预算耗尽 = 不完整——"未命中只在完整扫描后给出"）
            let mut occurrences: Vec<(String, Vec<u64>)> = Vec::new();
            let mut complete = true;
            let mut has_unscannable = false;
            for (id, pattern) in &rule.patterns {
                let (hits, pattern_complete) = match pattern {
                    Pattern::Hex(hp) => find_hex(data, hp),
                    Pattern::Text(tp) => find_text(data, tp),
                    Pattern::Unscannable(_) => {
                        // 评估不了的串不能报"没命中"：整条规则按 Unknown 落账
                        has_unscannable = true;
                        (Vec::new(), true)
                    }
                };
                complete &= pattern_complete;
                occurrences.push((id.clone(), hits));
            }
            let ctx = EvalCtx {
                data,
                truncated,
                binding: None,
            };
            let met = eval(&rule.condition, &occurrences, &ctx);
            let hits: Vec<(String, u64)> = occurrences
                .iter()
                .flat_map(|(id, offs)| offs.iter().map(move |o| (id.clone(), *o)))
                .collect();
            let value = if has_unscannable {
                Ternary::Unknown
            } else {
                match met {
                    Ternary::True => Ternary::True,
                    Ternary::Unknown => Ternary::Unknown,
                    Ternary::False if truncated || !complete => Ternary::Unknown,
                    Ternary::False => Ternary::False,
                }
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

/// 求值上下文：扫描数据（filesize / uintN 的来源）+ 截断标志 +
/// for-of 循环的当前串绑定（occurrences 的下标）。
struct EvalCtx<'d> {
    data: &'d [u8],
    truncated: bool,
    binding: Option<usize>,
}

impl EvalCtx<'_> {
    fn with_binding(&self, index: usize) -> EvalCtx<'_> {
        EvalCtx {
            data: self.data,
            truncated: self.truncated,
            binding: Some(index),
        }
    }
}

/// Kleene 合取：F 吞一切；T∧T=T；其余 U。
fn kleene_and(a: Ternary, b: Ternary) -> Ternary {
    match (a, b) {
        (Ternary::False, _) | (_, Ternary::False) => Ternary::False,
        (Ternary::True, Ternary::True) => Ternary::True,
        _ => Ternary::Unknown,
    }
}

/// Kleene 析取：T 吞一切；F∨F=F；其余 U。
fn kleene_or(a: Ternary, b: Ternary) -> Ternary {
    match (a, b) {
        (Ternary::True, _) | (_, Ternary::True) => Ternary::True,
        (Ternary::False, Ternary::False) => Ternary::False,
        _ => Ternary::Unknown,
    }
}

/// Kleene 取反：U 不塌缩。
fn kleene_not(v: Ternary) -> Ternary {
    match v {
        Ternary::True => Ternary::False,
        Ternary::False => Ternary::True,
        Ternary::Unknown => Ternary::Unknown,
    }
}

fn bool3(b: bool) -> Ternary {
    if b { Ternary::True } else { Ternary::False }
}

/// 操作数解析：`None` = 不可求值（→ `Unknown`，不猜）。
fn resolve(op: &Operand, occurrences: &[(String, Vec<u64>)], ctx: &EvalCtx) -> Option<u64> {
    match op {
        Operand::Lit(v) => Some(*v),
        Operand::Filesize => {
            if ctx.truncated {
                None // 截断输入的大小不完整——filesize 上的比较不可信
            } else {
                Some(ctx.data.len() as u64)
            }
        }
        Operand::Count(index) => occurrences.get(*index).map(|(_, offs)| offs.len() as u64),
        Operand::Offset { index, nth } => occurrences
            .get(*index)
            .and_then(|(_, offs)| offs.get(*nth))
            .copied(),
        Operand::UintRead { size, offset } => {
            if ctx.truncated {
                return None;
            }
            let start = *offset as usize;
            let end = start.checked_add(*size as usize)?;
            if end > ctx.data.len() {
                return None; // 越界读：不可求值（不猜 0）
            }
            let mut padded = [0u8; 8];
            padded[..*size as usize].copy_from_slice(&ctx.data[start..end]);
            Some(u64::from_le_bytes(padded))
        }
        Operand::Entrypoint => None, // PE 上下文未联动（N4 边界，如实弃权）
        Operand::Opaque(_) => None,
        Operand::CurrentCount => ctx
            .binding
            .and_then(|i| occurrences.get(i))
            .map(|(_, offs)| offs.len() as u64),
        Operand::CurrentOffset { nth } => ctx
            .binding
            .and_then(|i| occurrences.get(i))
            .and_then(|(_, offs)| offs.get(*nth))
            .copied(),
    }
}

/// 条件求值（三值 Kleene；基于每模式的命中偏移表）。
/// `True` = 判真；`False` = 完整证据下的判假；`Unknown` = 证据不足
///（不可求值的操作数 / 循环体弃权）——绝不把弃权折叠成假。
fn eval(condition: &Condition, occurrences: &[(String, Vec<u64>)], ctx: &EvalCtx) -> Ternary {
    let set = |prefix: &Option<String>| -> Vec<usize> {
        occurrences
            .iter()
            .enumerate()
            .filter(|(_, (id, _))| prefix.as_deref().is_none_or(|p| id.starts_with(p)))
            .map(|(i, _)| i)
            .collect()
    };
    match condition {
        Condition::All => bool3(occurrences.iter().all(|(_, offs)| !offs.is_empty())),
        Condition::Any => bool3(occurrences.iter().any(|(_, offs)| !offs.is_empty())),
        Condition::Count(n) => {
            bool3(occurrences.iter().filter(|(_, o)| !o.is_empty()).count() >= *n)
        }
        Condition::At { index, offset } => {
            match occurrences
                .get(*index)
                .and_then(|(_, offs)| offs.first().copied())
            {
                Some(first) => match resolve(offset, occurrences, ctx) {
                    Some(v) => bool3(first == v),
                    None => Ternary::Unknown,
                },
                None => bool3(false), // 串零命中：无"首次命中"，At 恒假
            }
        }
        Condition::In { index, lo, hi } => bool3(
            occurrences
                .get(*index)
                .is_some_and(|(_, offs)| offs.iter().any(|o| o >= lo && o <= hi)),
        ),
        Condition::Ref(index) => bool3(
            occurrences
                .get(*index)
                .is_some_and(|(_, offs)| !offs.is_empty()),
        ),
        Condition::Cmp { lhs, op, rhs } => match (
            resolve(lhs, occurrences, ctx),
            resolve(rhs, occurrences, ctx),
        ) {
            (Some(a), Some(b)) => bool3(op.apply(a, b)),
            _ => Ternary::Unknown,
        },
        Condition::ForOf {
            bound,
            prefix,
            body,
        } => for_of(*bound, prefix, body, occurrences, ctx),
        Condition::CurrentRef => bool3(
            ctx.binding
                .and_then(|i| occurrences.get(i))
                .is_some_and(|(_, offs)| !offs.is_empty()),
        ),
        Condition::CurrentAt { offset } => {
            match ctx
                .binding
                .and_then(|i| occurrences.get(i))
                .and_then(|(_, offs)| offs.first().copied())
            {
                Some(first) => match resolve(offset, occurrences, ctx) {
                    Some(v) => bool3(first == v),
                    None => Ternary::Unknown,
                },
                None => bool3(false),
            }
        }
        Condition::CurrentIn { lo, hi } => bool3(
            ctx.binding
                .and_then(|i| occurrences.get(i))
                .is_some_and(|(_, offs)| offs.iter().any(|o| o >= lo && o <= hi)),
        ),
        Condition::Const(b) => bool3(*b),
        Condition::And(conds) => conds
            .iter()
            .map(|c| eval(c, occurrences, ctx))
            .fold(Ternary::True, kleene_and),
        Condition::Or(conds) => conds
            .iter()
            .map(|c| eval(c, occurrences, ctx))
            .fold(Ternary::False, kleene_or),
        Condition::Not(inner) => kleene_not(eval(inner, occurrences, ctx)),
        Condition::OfAny { prefix } => {
            bool3(set(prefix).iter().any(|i| !occurrences[*i].1.is_empty()))
        }
        Condition::OfAll { prefix } => {
            let set = set(prefix);
            bool3(!set.is_empty() && set.iter().all(|i| !occurrences[*i].1.is_empty()))
        }
        Condition::OfCount { n, prefix } => bool3(
            set(prefix)
                .iter()
                .filter(|i| !occurrences[**i].1.is_empty())
                .count()
                >= *n,
        ),
    }
}

/// for-of 求值：对集合中每条**命中**串求体（YARA 语义：迭代对象是有命中
/// 的串）。体弃权（Unknown）不折叠成假——N/any 在「已满足数不足且弃权
/// 可能补足」时如实上报 Unknown；`all` 有弃权即 Unknown。
fn for_of(
    bound: ForBound,
    prefix: &Option<String>,
    body: &Condition,
    occurrences: &[(String, Vec<u64>)],
    ctx: &EvalCtx,
) -> Ternary {
    let matched: Vec<usize> = occurrences
        .iter()
        .enumerate()
        .filter(|(_, (id, offs))| {
            prefix.as_deref().is_none_or(|p| id.starts_with(p)) && !offs.is_empty()
        })
        .map(|(i, _)| i)
        .collect();
    if matched.is_empty() {
        return bool3(false); // 空集合：any/N/all 都无从满足（与 OfAll 同口径）
    }
    let total = matched.len();
    let mut satisfied = 0usize;
    let mut abstained = 0usize;
    for i in matched {
        match eval(body, occurrences, &ctx.with_binding(i)) {
            Ternary::True => satisfied += 1,
            Ternary::Unknown => abstained += 1,
            Ternary::False => {}
        }
    }
    match bound {
        ForBound::N(n) => {
            if satisfied >= n {
                Ternary::True
            } else if satisfied + abstained >= n {
                Ternary::Unknown
            } else {
                Ternary::False
            }
        }
        ForBound::Any => {
            if satisfied >= 1 {
                Ternary::True
            } else if abstained >= 1 {
                Ternary::Unknown
            } else {
                Ternary::False
            }
        }
        ForBound::All => {
            if satisfied == total {
                Ternary::True
            } else if abstained > 0 {
                Ternary::Unknown
            } else {
                Ternary::False
            }
        }
    }
}

// ── 字节级匹配引擎（拆出守 800 行预算）──────────────────────────────────

mod matchers;

pub use matchers::SCAN_BUDGET;
use matchers::{find_hex, find_text};

#[cfg(test)]
mod tests;
