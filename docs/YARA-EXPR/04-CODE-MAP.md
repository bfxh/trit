# 04 代码地图：接缝补丁里已经有什么，还剩什么

> 应用 `ref/0001-seam.patch` 之后的状态。标“已验证”的部分不需要你重写，读懂接口就行；标“待写”的是任务卡的内容。
> 路径都相对 `crates/trit-re/`。

---

## 1. 数据怎么流

```
.yar 文本
  │  yaraparse::parse_rules_v2                        ← 待写（T10–T16）
  ▼
Rule { patterns, condition: Condition::Expr(Box<Expr>), global, private_rule }
  │  SigScan::scan_file / scan_with_context
  ▼
sigscan/xscan.rs  scan_core                            ← 已验证
  │   每条规则：扫串 → 每条串一个 StrFact{hits, exact, fixed_len}
  │   Condition::Expr → judge() → xeval::eval()
  │   旧的声明式变体 → eval::eval()（旧求值器，行为冻结）
  ▼
sigscan/xeval.rs  eval(&Expr, &XCtx, &mut Env) -> Val   ← 已验证
  │   算术/比较 → xval.rs      字符串算子 → xstr.rs
  │   串事实 → xfacts.rs       循环与 of → xloop.rs
  │   模块对象 → modreg.rs → pe2::build / pe2::call     ← 这两个待写（T20–T25）
  ▼
Val → 规则裁决 Ternary → global / private 处理 → SigReport
```

新旧两套前端在切换（T30）之前并存：`parse_rules` 是旧的，原样工作；`parse_rules_v2` 是新的，接缝阶段返回 `Err`。

## 2. 文件清单

### 已验证（不要改，除非任务卡明说）

| 文件 | 行数 | 内容 | 怎么验证的 |
|---|---|---|---|
| `src/sigscan/expr.rs` | 273 | AST：`Expr`、`Quant`、`Iter`、`OfSet`、`OfTail`、`ObjPath`、`RegexLit` | 编译；被求值器和单测用到 |
| `src/sigscan/xval.rs` | 389 | `Val`、哨兵归一、`bin` / `cmp` / `neg` / `bit_not` / `not` / `defined`、`ss_compare` | 4 条单测，逐条对应官方读数 |
| `src/sigscan/xstr.rs` | 123 | 七个字符串算子 | 回放向量文件里 72 条字面量读数 |
| `src/sigscan/xfacts.rs` | 178 | `StrFact`：`found` / `found_at` / `found_in` / `count` / `count_in` / `offset` / `length` / `count_vs` | 2 条单测 |
| `src/sigscan/xloop.rs` | 370 | `run_loop`、`eval_for`、`of_result`、`eval_of` | 3 条单测 |
| `src/sigscan/xeval.rs` | 289 | `XCtx`、`Env`、`Slot`、`eval`、`EVAL_BUDGET` | 见下一行 |
| `src/sigscan/xtests.rs` | 451 | 手工搭 AST 的端到端单测 7 条，约 100 个断言 | 每个断言对应一条官方读数或一条三态设计规则 |
| `src/sigscan/xscan.rs` | 210 | `scan_core`、`scan_file`、`judge`、`apply_global`、`Rule::with_flags`、`Rule::report_hits` | `xtests.rs` 的规则级单测 |
| `src/sigscan/modreg.rs` | 176 | `Node`、`Mods`、`walk`、`value`、`items`、`c_str` | 编译；路径逻辑要等 pe 树接入后由向量验证 |
| `src/sigscan/pattern_bytes.rs` | 75 | `Pattern::text_bytes` | 1 条单测 |
| `src/sigscan/pe2/rva.rs` | 173 | `pe_header`、`rva_to_offset` | 2 条单测，对应合成样本上的官方读数 |
| `src/sigscan/pe2/schema.rs` | 280 | pe 声明表 + `kind_of` / `is_func` / `overload` | 生成文件；1 条单测 |
| `src/sigscan/pe2/consts.rs` | 201 | 171 个常量 + `constant` | 生成文件；值由官方引擎读出 |
| `src/sigscan/pe2/ordinals.rs` | 563 | 序号名表 + `ord_name` | 生成文件；1 条单测 |
| `tests/yara_expr_vectors.rs` | 169 | 向量回放 | 接缝阶段：错判 0 |
| `tests/data/yara_expr/` | | 2732 条向量 + 10 个样本 | 由 `tools/check_vectors.py` 对官方引擎复核 |

### 占位（待写）

| 文件 | 现状 | 任务 |
|---|---|---|
| `src/yaraparse/y2.rs` | `parse_rules_v2` 返回 `Err` | T10–T16 |
| `src/sigscan/pe2.rs` | `build` 返回 `Node::Unknown`，`call` 返回 `Val::Unknown` | T20–T25 |

### 动过的旧文件

| 文件 | 改动 |
|---|---|
| `src/sigscan.rs` | `Condition` 加 `Expr(Box<expr::Expr>)` 变体；模块声明改成一行一个带注释 |
| `src/sigscan/eval.rs` | 加一条分支：旧求值器遇到 `Condition::Expr` 返回 `Unknown` |
| `src/sigscan/scan.rs` | 循环体搬到 `xscan.rs`，只留四个入口 |
| `src/sigscan/rule.rs` | `Rule` 加 `global`、`private_rule`；`RuleVerdict` 加 `private`；`report_hits` 搬到 `xscan.rs` |
| `src/yaraparse.rs` | 加 `mod y2;` 和 `pub use y2::parse_rules_v2;` |

## 3. 你会用到的接口

### 3.1 解析器要产出的东西（`sigscan::expr`）

```rust
pub enum Expr {
    Int(i64), Float(u64 /* f64 的位模式 */), Str(Vec<u8>), Bool(bool), Regex(RegexLit),
    Filesize, Entrypoint,
    ReadInt { signed: bool, bytes: u8, big_endian: bool, at: Box<Expr> },
    Found(StrRef), FoundAt(StrRef, Box<Expr>), FoundIn(StrRef, Box<Expr>, Box<Expr>),
    Count(StrRef), CountIn(StrRef, Box<Expr>, Box<Expr>),
    Offset(StrRef, Box<Expr>), Length(StrRef, Box<Expr>),
    Var(u8),                      // 值型循环变量：整数、字符串、字典的键和值
    Obj(ObjPath),                 // 模块对象取值或函数调用
    Neg { float: bool, e: Box<Expr> }, BitNot(Box<Expr>),
    Bin { op: BinOp, float: bool, a: Box<Expr>, b: Box<Expr> },
    Cmp { op: CmpOp, kind: CmpKind, a: Box<Expr>, b: Box<Expr> },
    StrOp(StrOp, Box<Expr>, Box<Expr>), Matches(Box<Expr>, RegexLit),
    Not(Box<Expr>), Defined(Box<Expr>), And(Box<Expr>, Box<Expr>), Or(Box<Expr>, Box<Expr>),
    StrToBool(Box<Expr>),
    Of { quant: Quant, set: OfSet, tail: OfTail },
    For { quant: Quant, iter: Iter, slot: u8, body: Box<Expr> },
    RuleRef(String),
    Abstain(String),              // 记名弃权：求值恒 Unknown
}

pub enum StrRef { Id(usize), Current }
pub enum Quant { All, Any, None, N(Box<Expr>), Percent(Box<Expr>) }
pub enum Iter  { Range(Box<Expr>, Box<Expr>), Ints(Vec<Expr>), Strs(Vec<Expr>),
                 Array(ObjPath), Dict(ObjPath), Strings(Vec<usize>) }
pub enum OfSet { Strings(Vec<usize>), Rules(Vec<String>) }
pub enum OfTail { Plain, At(Box<Expr>), In(Box<Expr>, Box<Expr>) }
pub struct ObjPath { pub root: Root, pub segs: Vec<Seg> }
pub enum Root { Module(String), Var(u8) }
pub enum Seg  { Field(String), Index(Box<Expr>), Key(Box<Expr>), Call(Vec<Expr>) }

impl RegexLit { pub fn new(src: &str, nocase: bool, dotall: bool) -> Self }
```

`CmpOp` 是 `sigscan::CmpOp`（`Eq Ne Lt Le Gt Ge`），旧代码里就有。

搭 AST 的例子全在 `src/sigscan/xtests.rs`，写解析器时照着看每种语法对应什么结点。几个要点：

- 循环变量槽位：第 L 层循环（L 从 0 起）的第一个变量是 `2 * L`，第二个是 `2 * L + 1`。
- 对象型循环变量 `s`（`for any s in pe.sections`）的成员访问 `s.name` 是 `Expr::Obj(ObjPath { root: Root::Var(槽位), segs: [Field("name")] })`。
- 字典循环 `for k, v in pe.version_info`：`k` 和 `v` 都是 `Expr::Var(槽位)`。
- `@a` 等于 `Offset(.., Int(1))`，`!a` 等于 `Length(.., Int(1))`。
- 字符串进入布尔上下文要包 `StrToBool`。
- 常量 `pe.MACHINE_AMD64` 直接产出 `Expr::Int(值)`。
- 函数调用 `pe.imports("a", "b")`：`segs = [Field("imports"), Call([Str("a"), Str("b")])]`。

### 3.2 装配规则

```rust
// 旧接口，照用：
Rule::from_parts(name: String, patterns: Vec<(String, Pattern)>, condition: Condition, private: Vec<usize>) -> Rule
// 新增：
rule.with_flags(global: bool, private_rule: bool) -> Rule       // 用上它之后删掉它上面那行 #[allow(dead_code)]

Pattern::text_bytes(needle: &[u8], nocase, wide, ascii_too, xor: Option<(u8, u8)>, fullword) -> Pattern
Pattern::hex(spec: &str) -> Result<Pattern, String>             // 旧
Pattern::regex(body: &str, nocase, wide, xor: bool, fullword) -> Pattern   // 旧
Pattern::base64_plan(…)                                         // 旧，签名见 sigscan/b64.rs
```

### 3.3 pe 侧

```rust
// sigscan/pe2/rva.rs
pub(crate) fn pe_header(d: &[u8]) -> Option<PeHeader>           // 官方的 PE 判定
pub(crate) fn rva_to_offset(d: &[u8], h: &PeHeader, rva: u64) -> Option<u64>   // None = 官方的 -1
impl PeHeader { pub nt: usize; pub plus: bool; fn opt(&self) -> usize; fn section_table(&self, d: &[u8]) -> usize }

// sigscan/pe2/schema.rs
pub(crate) fn kind_of(path: &str) -> Option<Kind>               // Kind: Int Str Struct Array DictStr
pub(crate) fn is_func(path: &str) -> bool
pub(crate) fn overload(path: &str, signature: &str) -> Option<char>   // 返回类型 'i' 或 's'

// sigscan/pe2/consts.rs
pub(crate) fn constant(name: &str) -> Option<i64>

// sigscan/pe2/ordinals.rs
pub(crate) fn ord_name(dll: &[u8], ordinal: u16) -> String
```

解析器在 `yaraparse` 下面，访问这些走 `crate::sigscan::pe2::schema::kind_of` 这样的路径。可见性已经在接缝补丁里放开（`pub(crate)`），不用再动。

## 4. 三处临时的 `#[allow(dead_code)]`

| 位置 | 什么时候删 |
|---|---|
| `src/sigscan/xscan.rs` 的 `Rule::with_flags` | T11：解析器开始调用它 |
| `src/sigscan/modreg.rs` 的 `enum Node` | T20：`pe2::build` 开始构造这些变体 |
| `src/sigscan/pe2.rs` 第一行的 `#![allow(dead_code)]` | T16 之后表和 `rva` 都被用到；T20 完成时删掉，clippy 仍然通过才算数 |

## 5. 向量回放怎么读

```bash
cargo test -p trit-re --test yara_expr_vectors -- --nocapture
```

```
expr_core.tsv: 一致 334 / 弃权 1074（上限 1074）/ 错判 0
  各组弃权（ID 首字母: 条数）：[('A', 133), ('B', 61), ('U', 103), ('L', 122), ('S', 96), ('N', 48), ('F', 53), ('M', 248), ('I', 181), ('E', 29)]
rule_level.tsv: 一致 35 / 弃权 37（上限 37）/ 错判 0
pe_fields.tsv: 一致 0 / 弃权 1252（上限 1252）/ 错判 0
```

这是接缝阶段的读数：新前端什么都不会，所以官方报错的 334 + 35 条“一致”（这边也拒绝），其余全是弃权。你的工作就是让弃权数降下来，同时错判保持 0。

逐条看某一组还剩哪些弃权：

```bash
YARA_EXPR_SHOW=A cargo test -p trit-re --test yara_expr_vectors -- --nocapture     # A 组
YARA_EXPR_SHOW=LM cargo test …                                                     # L 组和 M 组
YARA_EXPR_SHOW=all cargo test …                                                    # 全部
```

三种结局的判法（在 `tests/yara_expr_vectors.rs` 的 `replay` 里）：

| 官方 | 这边 | 结局 |
|---|---|---|
| 命中集合 X | 命中集合 X | 一致 |
| 编译错误 | 拒绝（`Err` 或 `Unsupported`） | 一致 |
| 有裁决 | `Unknown`，或拒绝 | 弃权 |
| 编译错误 | `Unknown` | 弃权 |
| 命中集合 X | 命中集合 Y（Y ≠ X） | **错判** |
| 编译错误 | 给出了命中集合 | **错判** |
