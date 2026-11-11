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
//!   `entrypoint` = 入口点**文件偏移**（官方语义，PE 与 ELF 双实测对拍判定：
//!   PE 的 RVA≠offset 判别样本与 ELF 的 ET_EXEC 判别样本都只认 offset），
//!   由调用方经 [`SigScan::scan_with_entrypoint`] 传入、缺上下文时弃权；
//!   模块/未知函数（`pe.*`、`math.*`）记名保留、求值恒 `Unknown`。
//! - 每模式命中数上限 [`MAX_HITS`]（65536，**封顶截断 = 扫描不完整**
//!   ——`at`/`in` 等位置性条件据此弃权，绝不据残表判假），跳跃上限
//!   [`MAX_JUMP`]，
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

/// 每个模式保留的命中数上限（病态输入护栏）。**封顶截断 = 命中表不完整
/// = 扫描不完整**——`at`/`in`/`@a[N]` 等位置性条件在记录集不能证明时
/// 如实弃权（"截断的沉默不是清白"落到命中表上），`#a` 计数按截断值如实
/// 下界。
pub const MAX_HITS: usize = 65536;
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
    /// `@a[N]`：串第 N 次（**1 起**，官方 YARA 语义——实测对拍判定）
    /// 命中的偏移；缺省首次。内部下标已由解析层折成 0 起。
    Offset { index: usize, nth: usize },
    /// `uintN(lit)`：偏移处读 N 字节小端整数；越界 = 不可求值。
    UintRead { size: u8, offset: u64 },
    /// `entrypoint`：入口点的**文件偏移**（官方 YARA 语义，实测对拍判定）。
    /// 值由调用方经 [`SigScan::scan_with_entrypoint`] 传入；缺上下文 = 弃权。
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
    /// 指定串**任一**命中恰在 `offset`（官方 YARA `at` 语义——不只是首次；
    /// 操作数可为字面量 / `entrypoint` 等）。
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
    /// `for bound of (set) : ( body )`——对集合中**每条串**求体（官方
    /// YARA 语义：未命中串也在迭代里——`#`=0、`$`=假、`@`=弃权；
    /// `for all of ($a*) : (# > 2)` 的惯用法正依赖全集迭代）。
    ForOf {
        bound: ForBound,
        prefix: Option<String>,
        body: Box<Condition>,
    },
    /// for-of 体内的 `$`（当前串有命中；体外出现 = 解析层拒绝）。
    CurrentRef,
    /// for-of 体内 `$ at X`：当前串**任一**命中恰在 X（官方 `at` 语义）。
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
        self.scan_with_entrypoint(data, truncated, None)
    }

    /// 带入口点上下文的扫描。`entrypoint` = 入口点的**文件偏移**——与官方
    /// YARA 语义一致（已实测对拍：RVA≠文件偏移的样本上，官方命中 offset
    /// 不命中 RVA）；由调用方从 PE/ELF 头解析传入（PE：`pe::parse` 的
    /// `entry_rva` 过 `rva_to_offset`）。`None` = 非可执行文件/解析失败 →
    /// `entrypoint` 操作数如实弃权（Unknown），绝不猜。
    pub fn scan_with_entrypoint(
        &self,
        data: &[u8],
        truncated: bool,
        entrypoint: Option<u64>,
    ) -> SigReport {
        self.scan_with_context(data, truncated, entrypoint, None)
    }

    /// 带 PE 模块上下文的扫描：`pe` = 调用方从 PE 头解析出的信息——
    /// `pe.*` 子集属性据此求值；`None` = 非 PE / 解析失败 → `pe.*`
    /// 如实弃权（Unknown），绝不猜。
    pub fn scan_with_context(
        &self,
        data: &[u8],
        truncated: bool,
        entrypoint: Option<u64>,
        pe: Option<&crate::pe::PeInfo>,
    ) -> SigReport {
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
                entrypoint,
                pe,
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

// ── 三值条件求值（拆出守 800 行预算）──────────────────────────────────

mod eval;

use eval::{EvalCtx, eval};

// ── pe.* 模块属性子集（YARA 完整子集方向）────────────────────────────

mod peprops;

use peprops::resolve as resolve_pe_prop;

// ── 字节级匹配引擎（拆出守 800 行预算）──────────────────────────────────

mod matchers;

pub use matchers::SCAN_BUDGET;
use matchers::{find_hex, find_text};

#[cfg(test)]
mod tests;
