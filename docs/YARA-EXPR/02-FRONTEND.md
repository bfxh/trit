# 02 前端规范：词法、规则骨架、条件解析

> 这份文件规定 `crates/trit-re/src/yaraparse/y2/` 里要写什么。照着写，然后让 `tests/yara_expr_vectors.rs` 告诉你对不对。
> 官方依据：libyara v4.5.4 的 `lexer.l` 与 `grammar.y`。语义见 `01-SEMANTICS.md`。
> 总原则：**官方编译不过的，这边必须拒绝**（规则 `Unsupported`，带原因）；官方能编译而这边不会的，也拒绝（登记为缺口）。拿不准就拒绝，绝不放行一个算不对的规则。

---

## 1. 词法（`y2/lex.rs`）

### 1.1 Token

```rust
pub(super) enum Tok {
    Ident(String),                 // 标识符（不是关键字）
    Kw(&'static str),              // 关键字，见 1.3
    Int(i64),
    Float(f64),
    Str(Vec<u8>),                  // 字符串字面量，转义已还原，是字节串
    Regex { src: String, nocase: bool, dotall: bool },
    StrId(String),                 // $a   → "a"；单独的 $ → ""
    StrWild(String),               // $a*  → "a"；$* → ""
    StrCount(String),              // #a
    StrOffset(String),             // @a
    StrLength(String),             // !a
    IntFn { signed: bool, bytes: u8, big_endian: bool },   // int8 … uint32be
    Hex(String),                   // { … } hex 串整块：花括号里的原文，注释已去掉
    Sym(&'static str),             // 标点与算子，见 1.4
    Bad(String),                   // 词法错误，内容是原因
}

pub(super) fn lex(text: &str) -> Vec<Tok>      // 永不失败；错误用 Tok::Bad 表达
```

### 1.2 扫描规则

按字节扫描 `text.as_bytes()`，在位置 `i` 处按下表**从上到下**取第一条匹配的规则。

| # | 看到 | 动作 |
|---|---|---|
| 1 | 空格、`\t`、`\r`、`\n` | 跳过。**只有这四个是空白**；`\x0b` 之类不是 |
| 2 | `//` | 跳到行尾 |
| 3 | `/*` | 跳到 `*/`；到文件尾都没闭合 → `Bad("注释未闭合")` 并结束 |
| 4 | `/`（其它情况） | 正则字面量，见 1.5 |
| 5 | `"` | 字符串字面量，见 1.6 |
| 6 | `{` | 试着当 hex 串整块，见 1.7；不是就 `Sym("{")` |
| 7 | `0x` 后跟十六进制数字 | 十六进制整数：读完所有十六进制数字。超过 `i64::MAX` → `Bad("整数溢出")` |
| 8 | `0o` 后跟 0–7 | 八进制整数：读完所有 0–7 |
| 9 | 数字 | 读完所有十进制数字。接着若是 `.` 加数字 → 再读完数字，得 `Float`。否则是十进制整数；若紧跟 `KB` 或 `MB`（大写）则吃掉并乘以 1024 或 1048576。任何一步溢出 → `Bad("整数溢出")` |
| 10 | `$` | 读完 `[A-Za-z0-9_]*`；若紧跟 `*` 则吃掉并得 `StrWild`，否则 `StrId` |
| 11 | `!=` | `Sym("!=")` |
| 12 | `#`、`@`、`!` | 读完 `[A-Za-z0-9_]*`，得 `StrCount` / `StrOffset` / `StrLength` |
| 13 | 字母或 `_` | 读完 `[A-Za-z0-9_]*`。整词是 `u?int(8\|16\|32)(be)?` → `IntFn`；是关键字 → `Kw`；长度超过 128 → `Bad("标识符过长")`；否则 `Ident` |
| 14 | `..` `<=` `>=` `==` `<<` `>>` | 对应的两字符 `Sym` |
| 15 | 1.4 里的单字符 | 对应的 `Sym` |
| 16 | 其它任何字节 | `Bad("非法字符")` |

规则 7、8 只在 `0x` / `0o` 后面**确实跟着合法数字**时才成立。否则落到规则 9：`0X10` 是整数 0 加标识符 `X10`，`0o8` 是 0 加 `o8`，后面由语法报错。同理 `1e3`、`1_000`、`1Kb`、`1.5KB` 都会词法上拆成“数字 + 标识符”，语法上出错，这正是官方的行为。

### 1.3 关键字（全部小写，大小写敏感）

```
all and any ascii at base64 base64wide condition contains defined endswith entrypoint
false filesize for fullword global icontains iendswith iequals import in include
istartswith matches meta nocase none not of or private rule startswith strings them
true wide xor
```

### 1.4 符号

单字符：`{ } ( ) [ ] : = , . < > + - * \ % & | ^ ~`
两字符：`.. <= >= == != << >>`

### 1.5 正则字面量

从 `/` 之后读到下一个**未转义**的 `/`：

- `\/` → 正则体里放一个 `/`。
- `\` 加其它字符 → 两个字符都原样放进正则体。
- 遇到换行或文件尾 → `Bad("正则未闭合")`。
- 正则体为空 → `Bad("空正则")`。

闭合的 `/` 之后：若紧跟 `i` 则 `nocase = true` 并吃掉；接着若紧跟 `s` 则 `dotall = true` 并吃掉。顺序固定是先 `i` 后 `s`；`/a/si` 会词法成“正则（带 s）+ 标识符 i”，语法上出错。

### 1.6 字符串字面量

| 看到 | 动作 |
|---|---|
| `"` | 结束 |
| `\t` `\r` `\n` `\"` `\\` | 对应的一个字节 |
| `\x` 加两位十六进制 | 那个字节 |
| `\` 加其它 | `Bad("非法转义")`，然后继续扫到这个字符串结束 |
| 换行 | `Bad("字符串未闭合")` |
| 文件尾 | `Bad("字符串未闭合")` |
| 其它字节 | 原样放入 |

### 1.7 hex 串整块

官方把 `{ 4D 5A ?? }` 整个当一个 token。做法：从 `{` 之后往前看，只允许出现下面这些东西，直到遇到 `}`：

- 十六进制数字、空格、`\t`、`\r`、`\n`
- `-` `|` `~` `?` `[` `]` `(` `)`
- 注释 `/* … */` 与 `// …` 到行尾

遇到 `}` 且中间至少有一个字符 → `Tok::Hex(去掉注释后的原文)`，位置移到 `}` 之后。遇到任何别的字符，或者 `{}` 中间是空的 → 不是 hex 串，只产出 `Sym("{")`，位置移到 `{` 之后。

规则体的 `{ meta: … }` 里有 `m`、`t` 这些非十六进制字母，所以不会被误判。

### 1.8 词法单测（写进 `lex.rs` 的 `#[cfg(test)]`）

| 输入 | 期望的 token 序列 |
|---|---|
| `filesize>60and filesize<62` | `Kw(filesize) Sym(>) Int(60) Kw(and) Kw(filesize) Sym(<) Int(62)` |
| `0x10 0X10 0o17 0o8 010` | `Int(16) Int(0) Ident(X10) Int(15) Int(0) Ident(o8) Int(10)` |
| `1KB 1MB 1Kb 1.5 1. .5` | `Int(1024) Int(1048576) Int(1) Ident(Kb) Float(1.5) Int(1) Sym(.) Sym(.) Int(5)` |
| `9223372036854775808` | `Bad` |
| `9007199254740992KB` | `Bad` |
| `$a $a* $ $* #a @a !a !a!=3 !=` | `StrId(a) StrWild(a) StrId("") StrWild("") StrCount(a) StrOffset(a) StrLength(a) StrLength(a) Sym(!=) Int(3) Sym(!=)` |
| `uint8 int16be uint8x UINT8` | `IntFn IntFn Ident(uint8x) Ident(UINT8)` |
| `"\x41\t\"\\" "\q" "a` | `Str([0x41,9,0x22,0x5c]) Bad Bad` |
| `/a\/b/is /x/si //c` | `Regex{src:"a/b",nocase,dotall} Regex{src:"x",dotall} Ident(i)`（`//c` 是注释） |
| `{ 4D 5A ?? [2-4] } { meta: }` | `Hex(" 4D 5A ?? [2-4] ") Sym({) Kw(meta) Sym(:) Sym(})` |
| `(0..2) 1<<2 a.b` | `Sym(() Int(0) Sym(..) Int(2) Sym()) Int(1) Sym(<<) Int(2) Ident(a) Sym(.) Ident(b)` |
| `a\x0bb`（中间是 0x0B） | `Ident(a) Bad Ident(b)` |

---

## 2. 规则骨架（`y2/file.rs`）

入口（已在接缝补丁里占位）：

```rust
pub fn parse_rules_v2(text: &str) -> Result<ParsedFile, String>
```

返回值沿用现有的 `ParsedFile { rules: Vec<RuleOutcome>, notes: Vec<String> }`。

### 2.1 顶层

逐个 token 处理：

| 看到 | 动作 |
|---|---|
| `Kw(import)` + `Str` | 模块名必须在已知表里：`tests pe elf math time console string dotnet hash cuckoo magic macho dex`。不在 → 整个文件 `Err`。在 → 记入 imports 集（之后的规则才看得见它） |
| `Kw(include)` | 整个文件 `Err("include 不展开")` |
| 任意个 `Kw(global)` / `Kw(private)`，然后 `Kw(rule)` | 解析一条规则，见 2.2 |
| `Tok::Bad` | 整个文件 `Err`（带原因） |
| 其它 | 整个文件 `Err` |

### 2.2 一条规则

```
rule <名字> [: 标签 标签 …] { [meta: …] [strings: …] condition: … }
```

第一步，**先切出规则体**：`rule` 后面必须是 `Ident`（不是 → 文件 `Err`），然后可选的 `:` 加标签，然后必须是 `Sym("{")`（不是 → 文件 `Err`）。从这个 `{` 开始数花括号深度（`Sym("{")` 加一，`Sym("}")` 减一，`Tok::Hex` 不算），找到配对的 `}`。找不到 → 文件 `Err("规则未闭合")`。这样一条规则内部再怎么错，也不会影响后面的规则。

第二步，在切出来的 token 片段里解析。下面任何一条不满足，这条规则就是 `RuleOutcome::Unsupported { name, reason }`，然后继续处理下一条规则：

1. 片段里不能有 `Tok::Bad`（原因取它的内容）。
2. 规则名不能与本文件前面的规则重名。
3. 写了 `:` 就至少要有一个标签，标签都是 `Ident`。
4. 段的顺序固定：`meta`、`strings`、`condition`。前两个可省，`condition` 必须有。
5. `meta:` 至少一项，每项是 `Ident = 值`，值是 `Str`、`Int`、`- Int`、`true`、`false` 之一。只检查形状，不保存。
6. `strings:` 至少一条声明，见第 3 节。
7. `condition:` 之后到片段末尾是条件 token，交给第 4 节。条件为空是错误。
8. 条件解析完之后，检查**未被引用的串**：名字不以 `_` 开头的串（匿名串也算）必须在条件里被引用过。

第三步，不管这条规则成功还是 `Unsupported`，**规则名都要记入“已知规则名”表**。后面的规则引用它时能解析成 `RuleRef`，求值时拿到 `Unknown`。

成功时：

```rust
let rule = Rule::from_parts(name, patterns, condition, private_string_ids).with_flags(is_global, is_private);
// condition：如果整个条件就是一个裸的 `$a`（Expr::Found(StrRef::Id(i))），
// 用 Condition::Ref(i)；否则用 Condition::Expr(Box::new(expr))。
```

保留 `Condition::Ref(i)` 这个特例是因为 `tests/yara.rs` 里有一条断言依赖它。

---

## 3. strings 段（`y2/strings.rs`）

这部分是把旧的 `yaraparse/valuemod.rs` 搬到新 token 上，修饰符的语义**不要改**，先读懂旧文件再动手。

每条声明：`StrId(名字) Sym("=") 值 修饰符*`。

| 检查 | 不满足时 |
|---|---|
| 名字非空时不能与前面的串重名（匿名串 `$` 可以有多条） | 规则 Unsupported |
| 同一个修饰符不能写两次 | 规则 Unsupported |
| `xor` 与 `nocase` 不能同时出现 | 规则 Unsupported |

值的三种形态：

| 值 | 做法 |
|---|---|
| `Str(bytes)` | 空串 → 规则 Unsupported。否则用 `Pattern::text_bytes`（见下）；带 `base64` / `base64wide` 时走旧的 `Pattern::base64_plan`（它要 `&str`：字节不是合法 UTF-8 时，这条串记成 `Pattern::Unscannable`） |
| `Hex(spec)` | `Pattern::hex(&spec)`。它报错（子集外语法，如半字节通配 `4?`、`( A \| B )`、`~`）→ 规则 Unsupported。hex 串只允许 `private` 修饰符，其它 → 规则 Unsupported |
| `Regex{src, nocase, dotall}` | `dotall` 为真时先用 `crate::sigscan::expr::dot_matches_newline(&src)` 改写正则体。然后 `Pattern::regex(&body, nocase \|\| 有 nocase 修饰符, wide, xor, fullword)` |

修饰符：`nocase` `wide` `ascii` `fullword` `private` `xor` `xor(k)` `xor(a-b)` `base64` `base64wide`。`base64(...)` 带自定义字母表 → 规则 Unsupported。其它词 → 规则 Unsupported。

`Pattern::text_bytes(needle: &[u8], nocase, wide, ascii_too, xor, fullword)` 已经在接缝补丁里（`sigscan/pattern_bytes.rs`）：针是字节串，`wide` 按官方做法在每个字节后面跟一个 0x00。

---

## 4. 条件解析

### 4.1 结点与状态

```rust
/// 解析期的静态类型。Opaque = 未实现模块的值，类型无从检查。
enum TyX { Int, Float, Str, Bool, Regex, Opaque }

struct Node {
    e: Expr,
    ty: TyX,
    konst: Option<i64>,   // 编译期已知的整数值（官方的 value.integer）；不知道就是 None
    prim: bool,           // 是不是「主表达式」：只有主表达式能当算术和比较的操作数
}

struct LoopVar { name: String, slot: u8, kind: VarKind }
enum VarKind { Int, Str, Obj(String /* schema 路径前缀，如 "sections[]" */) }

struct Cx<'a> {
    toks: &'a [Tok],
    pos: usize,
    strings: &'a [String],        // 本规则的串名，顺序与串表一致
    referenced: Vec<bool>,        // 每条串是否被引用过
    imports: &'a [String],        // 此刻已 import 的模块
    rules: &'a [String],          // 已知规则名（含当前规则自己）
    loops: Vec<Vec<LoopVar>>,     // 循环栈，每层一组变量
    in_for_of: bool,              // 是否在 for … of 的体内
}
```

所有解析函数返回 `Result<Node, String>`。出错就返回 `Err(原因)`，整条规则 Unsupported。

**`prim` 是整个语法的关键。** 官方文法分两层：`primary_expression`（数、串、算术、函数调用、带括号的主表达式）和 `expression`（比较、`and`、`or`、`not`、串引用、`for`、`of`）。算术和比较的操作数只能是前者。所以 `(1 + 2) * 3` 合法，`(1 < 2) == 1` 和 `true + 1` 是语法错误。

### 4.2 函数阶梯

```
parse_condition():
    n = parse_or()
    若还有剩余 token → Err("条件尾部有多余的 token")
    返回 to_bool(n).e

to_bool(n):                       # 进入布尔上下文
    若 n.ty == Str → Node{ e: StrToBool(n.e), ty: Bool, konst: None, prim: false }
    否则 → n（把 prim 置为 false）

parse_or():
    left = parse_and()
    当下一个是 Kw(or)：吃掉；right = parse_and()
        left = Node{ e: Or(to_bool(left).e, to_bool(right).e), ty: Bool, konst: None, prim: false }
    返回 left

parse_and():                      # 同上，Kw(and)，下一层是 parse_not()，结点是 And

parse_not():
    Kw(not)     → 吃掉；inner = parse_not()；返回 Node{ Not(to_bool(inner).e), Bool, prim: false }
    Kw(defined) → 吃掉；inner = parse_not()；返回 Node{ Defined(to_bool(inner).e), Bool, prim: false }
    否则        → parse_rel()
```

这样 `not a == b` 是 `not (a == b)`，`not a and b` 是 `(not a) and b`，`defined a == b` 是 `defined (a == b)`，都与官方一致。

```
parse_rel():
    按下一个 token：
    Kw(true) / Kw(false)        → Node{ Bool(..), Bool, prim: false }
    Kw(for)                      → parse_for()                         # 第 6 节
    Kw(all) / Kw(any) / Kw(none) → 吃掉；下一个必须是 Kw(of)；parse_of(对应的量词)   # 第 5 节
    StrId(name)                  → parse_string_ref(name)              # 4.6
    其它：
        p = parse_arith(1)
        若 !p.prim → 返回 p                    # 括号里的布尔组，后面不能再接东西
        按下一个 token：
          Kw(of)                         → parse_of(量词 N(p))          # p 要过量词检查，见 5.1
          Sym(%) 且再下一个是 Kw(of)     → 吃掉两个；parse_of(百分比量词(p))
          == != < <= > >=                → 吃掉；r = parse_arith(1)；r 必须 prim；返回 make_cmp(op, p, r)
          contains 等 7 个关键字          → 吃掉；r = parse_arith(1)；r 必须 prim；返回 make_strop(op, p, r)
          Kw(matches)                    → 吃掉；下一个必须是 Tok::Regex；返回 make_matches(p, 正则)
          其它                           → 返回 p（prim 保持为 true）
```

比较之后不会再回到这里，所以 `1 == 1 == 1` 的第二个 `==` 会留在 token 流里，最终被 `parse_condition` 判为“多余的 token”。

### 4.3 算术：优先级爬升

优先级（数字越大绑得越紧），全部左结合：

| 级 | 算子 |
|---|---|
| 1 | `\|` |
| 2 | `^` |
| 3 | `&` |
| 4 | `<<` `>>` |
| 5 | `+` `-` |
| 6 | `*` `\` `%` |

```
parse_arith(min):
    left = parse_unary()
    循环：
        op = 下一个 token 若是上表里级别 ≥ min 的算子，否则跳出
        若 op 是 % 且再下一个 token 是 Kw(of) → 跳出      # 留给 parse_rel 处理百分比量词
        若 !left.prim → Err("布尔表达式不能参与算术")
        吃掉 op
        right = parse_arith(op 的级别 + 1)
        若 !right.prim → Err(同上)
        left = make_bin(op, left, right)                    # 4.5
    返回 left

parse_unary():
    Sym(-) → 吃掉；e = parse_unary()；e 必须 prim；类型必须是 Int / Float / Opaque
             返回 Node{ Neg{ float: e.ty == Float, e }, ty: e.ty, konst: e.konst 取回绕负, prim: true }
    Sym(~) → 吃掉；e = parse_unary()；e 必须 prim；类型必须是 Int / Opaque
             返回 Node{ BitNot(e), Int, konst: e.konst 按位取反, prim: true }
    否则   → parse_atom()
```

### 4.4 原子

```
parse_atom():
    Int(v)            → Node{ Int(v), Int, konst: Some(v), prim }
    Float(f)          → Node{ Float(f.to_bits()), Float, prim }
    Str(s)            → Node{ Str(s), Str, prim }
    Regex{..}         → Node{ Regex(RegexLit::new(src, nocase, dotall)), Regex, prim }
    Kw(filesize)      → Node{ Filesize, Int, prim }
    Kw(entrypoint)    → Node{ Entrypoint, Int, prim }
    IntFn{..}         → 必须跟 Sym(()；arg = parse_arith(1)，必须 prim 且类型 Int / Opaque；必须跟 Sym())
                        Node{ ReadInt{..}, Int, prim }
    StrCount(name)    → s = 串引用(name)；若下一个是 Kw(in) → 吃掉；(lo, hi) = parse_range()
                        → Node{ CountIn(s, lo, hi), Int, prim }；否则 Node{ Count(s), Int, prim }
    StrOffset(name)   → s = 串引用(name)；若下一个是 Sym([) → 吃掉；i = parse_arith(1)，必须 prim（**不查类型**）；必须跟 Sym(])
                        否则 i = Int(1)。Node{ Offset(s, i), Int, prim }
    StrLength(name)   → 同上，结点是 Length
    Ident(name)       → parse_identifier(name)                         # 第 7 节
    Sym(()            → 吃掉；inner = parse_or()；必须跟 Sym())
                        若 inner.prim → 返回 inner（仍是主表达式，类型和 konst 都保留）
                        否则          → 返回 inner（prim: false）
    其它              → Err("无法识别的表达式起点")
```

`串引用(name)`：name 为空 → 必须 `in_for_of`，否则 Err（官方：wrong use of anonymous string），返回 `StrRef::Current`。name 非空 → 在 `strings` 里找，找不到 Err（undefined string）；找到则 `referenced[i] = true`，返回 `StrRef::Id(i)`。

`parse_range()`：必须是 `Sym(()`；lo = parse_arith(1)；必须是 `Sym(..)`；hi = parse_arith(1)；必须是 `Sym())`。两端都必须 prim 且类型 Int / Opaque。**两端的 konst 都已知时**才做常量检查：lo > hi → Err；否则 lo < 0 → Err。只有一端已知时不检查（`(-1..filesize)` 合法）。

### 4.5 类型检查与常量折叠

`make_bin(op, l, r)`，其中 `l`、`r` 的类型里只要有 `Opaque`，就跳过类型检查，结果类型 `Opaque`、`konst: None`。

**`+ - * \`**

| 操作数类型 | 结果 |
|---|---|
| 两边都是 Int | Int，`float: false` |
| Int 与 Float，或两边 Float | Float，`float: true`，konst 一律 None，不做下面的常量检查 |
| 两边都是 Str | Err（strings don't support） |
| 其它 | Err（type mismatch） |

两边都是 Int 时的常量规则（`a`、`b` 是两边的 konst）：

| 算子 | 两边 konst 都已知时报错的条件（照抄官方公式） | 结果的 konst |
|---|---|---|
| `+` | `(b > 0 && a > i64::MAX - b) \|\| (b < 0 && a < i64::MIN - b)` | 都已知则 `a.wrapping_add(b)` |
| `-` | `(b < 0 && a > i64::MAX + b) \|\| (b > 0 && a < i64::MIN + b)` | 都已知则 `a.wrapping_sub(b)` |
| `*` | `b != 0 && a.wrapping_abs() > i64::MAX / b.wrapping_abs()` | 都已知则 `a.wrapping_mul(b)` |
| `\` | 见下 | 见下 |

`*` 的公式必须用 `wrapping_abs`：官方对 `INT64_MIN` 取绝对值会得到 `INT64_MIN` 本身，于是 `INT64_MIN * 2` 不报错而 `1 * INT64_MIN` 报错。这两条都有向量。

`\`（两边都是 Int）：

1. `b == Some(0)` → Err（division by zero）。注意只看除数：`filesize \ 0` 也报错。
2. 两边都已知且 `a == i64::MIN && b == -1` → Err（官方编译器在这里崩溃）。
3. 两边都已知 → konst = `a / b`；否则 None。

**`%`**：两边必须是 Int。常量规则与 `\` 完全相同（konst 是 `a % b`）。

**`& | ^`**：两边必须是 Int。两边 konst 都已知则算出来，否则 None。

**`<<` 与 `>>`**：两边必须是 Int。设 `b` 是右边的 konst：

1. `b` 已知且 < 0 → Err。
2. `b` 已知且 ≥ 64 → konst = `Some(0)`。**左边不是常量也是如此**，所以 `1 \ (filesize << 64)` 是编译错误。
3. 否则两边都已知 → konst = `a.wrapping_shl(b as u32)`。**`>>` 也用左移算**：这是官方的笔误，必须照抄。于是 `1 \ (1 >> 1)` 能编译（官方以为除数是 2），而 `1 \ (INT64_MIN >> 1)` 是编译错误（官方以为除数是 0）。
4. 否则 None。

konst 只用于编译期判错，**不改变 AST**：`Expr` 里仍然是原来的运算，运行期照常计算。

**模块常量不是编译期常量**：`pe.MACHINE_UNKNOWN` 这类标识符在 AST 里是 `Expr::Int(值)`，但它的 `konst` 必须是 `None`。所以 `1 \ pe.MACHINE_UNKNOWN` 能编译，运行期得 undefined。

`make_cmp(op, l, r)`：

| 操作数类型 | `kind` |
|---|---|
| 两边都是 Int | `CmpKind::Int` |
| Int 与 Float，或两边 Float | `CmpKind::Float` |
| 两边都是 Str | `CmpKind::Str` |
| 一边 Opaque | 另一边是 Str 取 Str，是 Float 取 Float，否则 Int |
| 其它（含 Bool、Regex） | Err（type mismatch） |

结果 `Node{ Cmp{..}, Bool, konst: None, prim: false }`。

`make_strop(op, l, r)`：两边必须是 Str（或 Opaque），否则 Err。结果类型 Bool，`prim: false`。

`make_matches(l, regex)`：左边必须是 Str（或 Opaque）。结果类型 Bool，`prim: false`。

### 4.6 串引用（表达式层）

```
parse_string_ref(name):               # 当前 token 是 StrId(name)
    吃掉；s = 串引用(name)
    Kw(at) → 吃掉；x = parse_arith(1)，必须 prim 且类型 Int / Opaque → Node{ FoundAt(s, x), Bool, prim: false }
    Kw(in) → 吃掉；(lo, hi) = parse_range() → Node{ FoundIn(s, lo, hi), Bool, prim: false }
    否则   → Node{ Found(s), Bool, prim: false }
```

`StrWild` 只能出现在串集合里，别处出现是错误。

---

## 5. `of`

### 5.1 量词

| 写法 | 结点 | 检查 |
|---|---|---|
| `all` / `any` / `none` | `Quant::All` / `Any` / `None` | |
| 主表达式 p | `Quant::N(p.e)` | p 的类型是 Float、Str、Regex → Err；`p.konst` 已知且 < 0 → Err |
| 主表达式 p 后跟 `%` | `Quant::Percent(p.e)` | p 的类型必须是 Int（或 Opaque）；`p.konst` 已知且不在 1..=100 → Err |

类型为 Bool 的主表达式（规则引用）可以当量词，官方不拦。

### 5.2 集合与尾巴

```
parse_of(quant):                      # Kw(of) 已吃掉
    若下一个是 Kw(them)，或 Sym(() 后面跟 StrId / StrWild：
        ids = parse_string_set()
        若量词是百分比：不允许尾巴
        否则若下一个是 Kw(in) → 吃掉；parse_range() → OfTail::In
        否则若下一个是 Kw(at) → 吃掉；x = parse_arith(1)，prim 且 Int → OfTail::At
        否则 OfTail::Plain
        返回 Node{ Of{ quant, OfSet::Strings(ids), tail }, Bool, prim: false }
    若是 Sym(() 后面跟 Ident：
        names = parse_rule_set()
        返回 Node{ Of{ quant, OfSet::Rules(names), OfTail::Plain }, Bool, prim: false }
    否则 Err

parse_string_set():
    Kw(them) → 规则没有任何串 → Err；否则全部下标，全部标记为已引用
    Sym(()   → 一项或多项，逗号分隔，然后 Sym())：
        StrId(name)   → name 必须非空且存在；加入它的下标
        StrWild(pre)  → 名字以 pre 开头的所有串（pre 为空就是全部）；一条都没有 → Err
        每一项涉及的串都标记为已引用
    结果是**列表**，重复的照留（`($a, $a)` 算两项）

parse_rule_set():
    Sym(()，一项或多项，逗号分隔，Sym())：
        Ident(name)              → 必须在已知规则名里；加入
        Ident(pre) 后跟 Sym(*)   → 已知规则名里以 pre 开头的全部；一条都没有 → Err
```

---

## 6. `for`

```
parse_for():                          # 当前 token 是 Kw(for)
    吃掉
    若 loops.len() == 4 → Err("循环嵌套超过 4 层")
    quant = 量词（5.1；这里不允许百分比）
    若下一个是 Kw(of)：                # for … of 串集合
        若 in_for_of → Err("for-of 不能嵌套")
        吃掉；ids = parse_string_set()
        必须跟 Sym(:) 和 Sym(()
        loops.push(空的一层)；in_for_of = true
        body = parse_or()
        in_for_of = false；loops.pop()
        必须跟 Sym())
        返回 Node{ For{ quant, Iter::Strings(ids), slot: 2 * 层号, body: to_bool(body).e }, Bool, prim: false }
    否则：                             # for … 变量 in 迭代源
        names = Ident（, Ident 最多再一个）
            每个名字都不能与任何外层循环变量重名 → 否则 Err
        必须跟 Kw(in)
        (iter, kinds) = parse_iterator(names.len())
        必须跟 Sym(:) 和 Sym(()
        loops.push(这一层的变量：第 k 个变量的 slot = 2 * 层号 + k)
        body = parse_or()
        loops.pop()
        必须跟 Sym())
        返回 Node{ For{ quant, iter, slot: 2 * 层号, body: to_bool(body).e }, Bool, prim: false }
```

“层号”是进入这个 `for` 之前 `loops.len()` 的值（0 到 3）。

注意 `in_for_of`：进入 `for … in` 的体时它保持原值，所以 `for any of them : (for any i in (0..1) : ($ at i))` 里的 `$` 仍然合法。

```
parse_iterator(nvars):
    若下一个是 Ident：
        按第 7 节解析对象路径，但要求最后停在数组或字典上
        数组 → 必须 nvars == 1；Iter::Array(path)；变量类别 Obj(该数组的 schema 路径 + "[]")
        字典 → 必须 nvars == 2；Iter::Dict(path)；第一个变量 Str，第二个变量 Str（pe 里唯一的字典值是字符串）
        其它 → Err（not iterable）
    若下一个是 Sym(()：
        必须 nvars == 1
        吃掉；first = parse_arith(1)，必须 prim
        若下一个是 Sym(..)：吃掉；last = parse_arith(1)；必须跟 Sym())
            两端类型必须是 Int；常量检查同 parse_range
            → Iter::Range(first, last)，变量类别 Int
        否则：枚举。继续读「, 主表达式」直到 Sym())
            第一项的类型必须是 Int 或 Str；后面每一项必须与第一项同类型
            → Iter::Ints(...) 变量类别 Int，或 Iter::Strs(...) 变量类别 Str
    否则 Err
```

---

## 7. 标识符（`y2/cx_ident.rs`）

```
parse_identifier(name):               # 当前 token 是 Ident(name)
    吃掉
    1. 在 loops 里从内层往外层找同名变量：
         VarKind::Int → Node{ Var(slot), Int, prim }
         VarKind::Str → Node{ Var(slot), Str, prim }
             这两种后面若跟 Sym(.) / Sym([) / Sym(() → Err
         VarKind::Obj(prefix) → walk_object(Root::Var(slot), prefix, 起始类别 = 结构)
    2. name 在 imports 里：
         "pe"  → walk_object(Root::Module("pe"), "", 起始类别 = 结构)
         其它  → skip_opaque(name)
    3. name 在已知规则名里 → Node{ RuleRef(name), Bool, konst: None, prim: true }
    4. 否则 Err（undefined identifier）
```

顺序很重要：循环变量遮蔽模块，模块遮蔽规则名。没 `import "pe"` 时 `pe` 会落到第 4 步。

### 7.1 `walk_object`：按 schema 走对象路径

用 `sigscan::pe2::schema` 里的三个函数：`kind_of(path)`、`is_func(path)`、`overload(path, 签名)`，以及 `sigscan::pe2::consts::constant(name)`。

状态：`path`（schema 路径字符串）、`segs`（已收集的 `Seg`）、`at`（当前停在什么上：结构 / 数组 / 字典 / 函数）。

```
循环：
  at == 结构：
      下一个必须是 Sym(.)，再下一个必须是 Ident(field)（关键字不行 → Err）
          否则 → Err（wrong usage of identifier：结构不能直接当值）
      full = path 为空 ? field : path + "." + field
      若 path 为空 且 constant(field) 有值 v（只有模块根上才有常量）：
          → 返回 Node{ Int(v), Int, konst: None, prim }        # konst 必须是 None
      按 kind_of(full)：
          Int     → segs 加 Field；返回叶子 Node{ Obj(..), Int, prim }
          Str     → 同上，类型 Str
          Struct  → segs 加 Field；path = full；at = 结构
          Array   → segs 加 Field；path = full + "[]"；at = 数组
          DictStr → segs 加 Field；path = full；at = 字典
          None 且 is_func(full) → segs 加 Field；path = full；at = 函数
          None → Err（invalid field name）
  at == 数组：
      下一个必须是 Sym([) → 吃掉；i = parse_arith(1)，prim 且类型 Int / Opaque，否则 Err；必须跟 Sym(])
          segs 加 Index(i)；at = 结构（数组元素都是结构，path 不变）
      不是 Sym([) → Err（数组不能直接当值）
  at == 字典：
      下一个必须是 Sym([) → 吃掉；k = parse_arith(1)，prim 且类型 Str / Opaque，否则 Err；必须跟 Sym(])
          segs 加 Key(k)；返回叶子 Node{ Obj(..), Str, prim }
  at == 函数：
      下一个必须是 Sym(() → 吃掉；读零个或多个实参，逗号分隔，每个实参用 parse_or()；必须跟 Sym())
          签名：每个实参按类型取一个字符 —— Int→i，Float→f，Bool→b，Str→s，Regex→r
              有实参是 Opaque → 返回 Node{ Abstain("实参类型未知"), Opaque, prim }
          ret = overload(path, 签名)；None → Err（wrong arguments）
          segs 加 Call(实参的 Expr)
          返回叶子 Node{ Obj(..), ret == 'i' ? Int : Str, prim }
      不是 Sym(() → Err（函数不能直接当值）
```

叶子后面若还跟着 `Sym(.)`、`Sym([)`、`Sym(()` → Err（not a structure / not indexable / not a function）。

`parse_iterator` 用的是同一个循环，只是在 `at == 数组` 或 `at == 字典` 且下一个 token 不是 `Sym([)` 时停下，返回路径和类别。

没有实现的字段（`signatures`、`imphash` 等）**照常解析**：schema 里有它们，类型检查照做。求值时对象树里没有这个结点，自然得 `Unknown`。

### 7.2 `skip_opaque`：未实现的模块

吃掉后面连续的 `.字段`、`[表达式]`、`(实参, …)`；方括号和圆括号里的内容仍然用 `parse_or()` 解析（保证语法检查不丢）。返回 `Node{ Abstain("模块 <名字> 未实现"), Opaque, konst: None, prim: true }`。

---

## 8. 文件怎么拆

每个文件登记进预算台账后就**不能再变长**（见 `00-RULES.md`），所以按下面拆，每个文件控制在 400 行以内：

| 文件 | 内容 |
|---|---|
| `y2.rs` | `mod` 声明 + `parse_rules_v2` |
| `y2/lex.rs` | 第 1 节 |
| `y2/file.rs` | 第 2 节 |
| `y2/strings.rs` | 第 3 节 |
| `y2/cx.rs` | `Cx`、`Node`、`TyX`、取 token 的小工具、`parse_condition`、`parse_or` / `parse_and` / `parse_not` / `parse_rel` |
| `y2/cx_arith.rs` | `parse_arith`、`parse_unary`、`make_bin`、`make_cmp`、`make_strop`、`make_matches` |
| `y2/cx_atom.rs` | `parse_atom`、串引用、`parse_range`、`parse_string_ref` |
| `y2/cx_of.rs` | 第 5 节 |
| `y2/cx_for.rs` | 第 6 节 |
| `y2/cx_ident.rs` | 第 7 节 |

## 9. 容易写错的地方

1. **`%` 的两种身份。** `5 % 3` 是取余，`50% of them` 是百分比。判断办法只有一个：`%` 后面紧跟的是不是 `Kw(of)`。
2. **括号。** `(` 之后永远调 `parse_or()`，然后看返回结点的 `prim`。不要另写一个“括号里只解析算术”的分支。
3. **`konst` 的传播。** 只有两边都已知才往上传，唯一的例外是移位量 ≥ 64 时结果的 konst 直接是 0。
4. **`>>` 的 konst 用左移。** 官方的笔误，向量里有两条专门验这个。
5. **`@a[i]` 的下标不查类型**，`$a at x` 的 x 要查。
6. **匿名串。** `$` 的名字是空串。它只能在 `for … of` 体内当“当前串”用；声明时可以有多条；它们只能通过 `them` 或 `($*)` 引用。
7. **串集合是列表不是集合。** 不要去重。
8. **引用标记。** `them`、`($a*)`、`$a`、`#a`、`@a`、`!a` 都算引用；`for … of` 体内的 `$` 不算。
9. **`for` 的嵌套上限在最前面查**，`for … of` 也占一层。
10. **模块常量的 konst 是 None。**
11. **关键字不能当字段名**：`pe.all` 是错误，因为 `all` 词法上是 `Kw` 不是 `Ident`。
12. **递归深度。** 仓库有一组模糊测试（`tests/fuzz_sigscan.rs`）要求任何输入都不 panic、不挂起。递归下降在 `((((…` 或 `not not not …` 上会爆栈，所以在 `Cx` 里放一个深度计数器：`parse_not`、`parse_unary`、`parse_atom` 的括号分支、`parse_for` 每进一层加一，出来减一，超过 200 就 `Err("嵌套过深")`。词法循环每一轮必须至少前进一个字节。
13. **不要 `unwrap`、不要直接下标。** 取 token 一律 `self.toks.get(self.pos)`；模糊测试会喂给你任何 token 序列。
