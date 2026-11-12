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
//!   `pe.*` 标量/数组式子集从 PE 头求值（`pe.sections[N].*` 越界 = 确定的未定义）；子集外路径/未知函数（`pe.rich_signature.*`、`math.*`）记名保留、求值恒 `Unknown`。
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
    /// 正则串（Pike VM 编译产物，见 [`crate::sigscan::regex`]）；不支持
    /// 的语法不走此形态（→ Unscannable 记名弃权）。
    Regex(crate::sigscan::regex::RegexPat),
    /// base64 串（相位变体集，见 [`crate::sigscan::b64`]，扫描并集计数）。
    Base64(Vec<TextPattern>),
    /// 可解析不可扫描（正则 / base64 / private）：整条规则判 `Unknown`。
    Unscannable(Unscannable),
}

// Pattern 构造器（hex / 文本 / base64）在 pattern.rs 与 b64.rs——守预算。

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
    pub(crate) rules: &'a [Rule],
}

// 扫描循环（`impl SigScan`）在 [`sigscan::scan`]——从本文件拆出守预算。

// 三值条件求值（拆出守预算）。
mod eval;

use eval::{EvalCtx, eval};

// 正则引擎（Pike VM，探针定案）。
pub mod regex;
// base64 相位变体展开（探针定案）。
pub mod b64;
mod sections; // pe.sections[N] 数组式访问（pe.* 深水区第一潜）
// Pattern 构造器（hex / 文本串，从本文件拆出守预算）。
mod pattern;
// 操作数解析三态（从 eval.rs 拆出守预算）。
mod resolve;
// 扫描循环（impl SigScan，从本文件拆出守预算）。
mod scan;

mod peprops; // pe.* 模块属性子集（YARA 完整子集方向）

use peprops::resolve as resolve_pe_prop;

// 字节级匹配引擎（拆出守预算）。
mod matchers;

pub use matchers::SCAN_BUDGET;
use matchers::{find_hex, find_text};

#[cfg(test)]
mod tests;
