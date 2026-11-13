# YARA 表达式语言 + pe 对象模型（trit-re，N4）

本文是 `trit-re sigscan` 条件语言的规格与对拍记录。基准是官方
**yara-python 4.5.4（YARA 4.5.4）**：这里写的每一条语义都是在官方引擎上
探出来的读数，不是照文档推的；官方文档与官方行为不一致时跟行为
（例：`pe.entry_point` 是文件偏移；`pe.is_dll` 是函数）。

读数日期 2026-11-13，环境 x86-64 Linux。数字都可按「对拍」一节重算。

## 一、范围

| 项 | 内容 | 状态 |
|---|---|---|
| A | 算术 / 位运算 `+ - * \ % & \| ^ << >> ~`、一元 `-`、括号、官方优先级 | 已实现 |
| B | `for <量词> i in (a..b) : (…)`、`for … in (e1, e2, …)`、遍历数组 / 字典；作用域与嵌套 | 已实现 |
| C | 动态下标 `pe.sections[i].name` 等 | 已实现 |
| D | `contains` `icontains` `startswith` `istartswith` `endswith` `iendswith` `iequals` `matches` | 已实现 |
| E | `pe` 的导入 / 导出 / 资源 / 版本信息 / 数据目录 / Rich 头 / 调试目录 | 已实现；延迟导入、签名弃权 |
| F | 三态纪律：弃权绝不折成假；官方拒绝的写法整条 `Unsupported` 并给理由 | 已实现 |

不做：`math` / `hash` / `dotnet` / `elf` 模块，YARA-X，浮点数，运行样本。

## 二、语法

| 类别 | 写法 | 备注 |
|---|---|---|
| 整数 | `10` `0x1F` `0o17` `4KB` `2MB` | 后缀只认大写；超出 int64 是编译错误 |
| 字符串 | `"a\tb\n\r\"\\\x41"` | 只有这六种转义；别的是编译错误 |
| 正则 | `/re/` `/re/i` `/re/s` | 只出现在 `matches` 右边和 `pe.imports` / `pe.exports` 实参；`/s` 配 `.` 不支持 |
| 文件量 | `filesize` `entrypoint` | |
| 定长读 | `uint8/16/32(off)` `int8/16/32(off)` 及 `…be` | 越界（含负偏移）= 未定义 |
| 串引用 | `$a` `#a` `@a` `@a[i]` `$a at e` `$a in (a..b)` `#a in (a..b)` | `@a[i]` 从 1 起；`@a[0]` 可编译、未定义 |
| 算术 | `+ - * \ %`，一元 `-` | 除法是 `\`；`/` 是语法错误 |
| 位运算 | `& \| ^ ~ << >>` | `>>` 是算术右移 |
| 比较 | `== != < <= > >=` | 两边同为整数或同为字符串；不能连写 |
| 字符串算子 | 见 D 项 | 两边都得是字符串 |
| 布尔 | `and` `or` `not` `defined` `true` `false` | |
| 集合量词 | `any/all/none/N of (…)`、`N% of`、`… of them in (a..b)`、`… at e` | |
| 循环 | `for Q of S : (…)`、`for Q i in (a..b) : (…)`、`for Q i in (e, …)`、`for Q x in 数组`、`for Q k, v in 字典` | `Q` = `any` `all` `none` 或整数表达式 |
| 模块 | `pe.<字段>`、`pe.<数组>[i].<字段>`、`pe.<字典>["k"]`、`pe.<函数>(…)`、`pe.<常量>` | 需要 `import "pe"` |

### 优先级（低 → 高）

1. `or`
2. `and`
3. `not`、`defined`
4. 比较与字符串算子（不结合：`1 < 2 < 3`、`1 < 2 == true` 是编译错误）
5. `|`
6. `^`
7. `&`
8. `<<` `>>`
9. `+` `-`
10. `*` `\` `%`
11. 一元 `-` `~`

### 编译期检查（官方拒绝 → 整条规则 `Unsupported`）

常量除零（被除数不是常量也算）；常量移位数为负；常量 `+ - *` 溢出；
整数字面量超出 int64；比较两边类型不同；字符串算子遇到非字符串；未知
字段、对标量取下标 / 取成员、函数实参个数或类型不对；循环变量重名、
循环嵌套超过 4 层、循环变量出了作用域；常量区间 `lo > hi` 或 `lo < 0`；
常量量词为负；`for N% i in (…)`；`for … of` 套 `for … of`；`pe.is_dll`
不带括号；`pe.sections[0].name == 5`。

常量 `INT64_MIN \ -1`：官方 4.5.4 **编译时崩溃**（SIGFPE）；trit 判
`Unsupported`。对拍脚本因此一律在子进程里调官方引擎。

## 三、运行期语义（探针定案）

- **整数**：int64。`+ - *` 运行期回绕；`\` `%` 除以零或 `MIN \ -1` =
  未定义；移位数为负 = 未定义，≥ 64 得 0；负数右移保持负。
- **官方的"未定义"哨兵**：算出 `0xFFFABADAFABADAFF` 这个值就等于未定义
  （官方用它做哨兵）——照搬。
- **布尔上下文**：整数非零为真，字符串非空为真；单独一个整数或字符串
  可以当条件。
- **字符串**：比较按**有符号字节**的字典序；大小写折叠只管 ASCII；空串
  是任何串的前缀 / 后缀 / 子串。`matches` 是不锚定的搜索，起点取
  `0..len-1`（所以 `"abc" matches /$/` 不命中，`/c$/` 命中），空串只试
  起点 0。
- **循环**：区间含两端；端点未定义或运行期 `lo > hi` → 零次迭代；**零次
  迭代对任何量词都是假**；循环体未定义算"不为真"；运行期量词未定义或
  为负 → 只要迭代过就满足。
- **`N of`**：`N` 未定义 = `all`；`0` = 一个都没有。`P% of`：常量 `P`
  须在 1..100。
- **数组下标**：未定义 / 负 / 越界 → 未定义。官方把下标截成 C `int`：
  `pe.sections[4294967296]` 就是第 0 节。
- **pe，非 PE 文件**：`pe.is_pe == 0` 有定义，其余字段与函数未定义，常量
  照常。

### 三态传播

三种结果：**值**、**未定义**（官方语义里确定的 undefined）、**弃权**
（trit 证据不足，`Unknown`）。规则级：值为真 = HIT；值为假或未定义 =
MISS；弃权 = UNKNOWN。

| 算子 | 未定义 | 弃权 |
|---|---|---|
| 算术 / 位运算 / 比较 / 字符串算子 | 任一边未定义 → 未定义（另一边是什么都改变不了） | 否则任一边弃权 → 弃权 |
| `and` | 当假 | 有一项确定为假 → 假；否则有弃权 → 弃权 |
| `or` | 当假 | 有一项确定为真 → 真；否则有弃权 → 弃权 |
| `not` | 仍未定义 | 仍弃权 |
| `defined` | 假 | 弃权 |
| 下标 / 成员 / 函数实参 | 未定义 | 弃权 |
| 循环、`of` | 体未定义算"不为真" | 已确定的迭代足以定案就定案，否则弃权 |

弃权的来源：没有 PE 上下文却用了 `pe.*` / `entrypoint`；字段在模型外
（按名字弃权，见第六节）；输入被截断；字符串扫描没扫完；求值燃料耗尽。
**弃权从不折成假。**

## 四、分流：既有子集不动

旧条件解析器真能求值的规则仍由它求值，判据一字不动。交给表达式引擎的
情形（`yaraparse/xparse.rs::route`，逐条钉在 `tests/yara_expr_route.rs`）：

1. 条件里有表达式语言的标志（`+ \ % & | ^ ~ << >>`、字符串算子、
   `defined`、`for … in`）——只走新引擎，它拒绝就是 `Unsupported`。单独
   的 `-` 不算标志。
2. 旧解析器拒绝的（`none of them`、`50% of them`、`int8`、`uint32be` 等）
   ——新引擎再试；都拒绝则两个理由一并上报。
3. 旧解析器收下却只会按名字弃权的（`pe.number_of_imports`、
   `pe.imports(…)`、`pe.is_64bit()`、`pe.is_pe`、字符串比大小）——新引擎
   能解析就换它，只增证据。
4. `not` 罩着可能未定义的比较（`not (uint8(越界) == 1)`）。
5. 字符串字面量里有 `\xNN` / `\r`。
6. `for … of` 套 `for … of`。
7. 模块数组的字面量下标超出 C `int`。

第 4–7 条修的是既有子集里与官方**实测不一致**的地方（见第七节"已修"），
不是放宽判据。

### 旧字段别名

`pe.sections[i].raw_size`、`pe.sections[i].raw_ptr`、不带括号的
`pe.rich_signature.version` 是旧子集自己的名字，官方没有（官方叫
`raw_data_size` / `raw_data_offset`，`version` 是函数）。既有规则在用，
保留在旧引擎上；表达式引擎不认，带表达式标志时用了它们就是
`Unsupported`。

### 扫描没扫完时

字符串匹配有预算（`SCAN_BUDGET`，每个串 100 万步）。预算内没扫完的串，
"没找到"不是证据：

- 表达式引擎：用到没扫完的串的计数 / 偏移 / 缺席 → 弃权。
- 旧引擎（`sigscan/scan/lossy.rs`）：结论为真且条件对命中数单调上升
  （多找到一处也不会变假）才保留；其余一律 UNKNOWN。

这修掉了一个既有的假命中：1 MB 以上的样本里串落在预算之外时，旧引擎对
`not $a`、`#a == 0`、`#a < 1` 报 HIT，官方 MISS。

### 深度与栈

解析器、求值器、正则编译器都是递归的，上限在递归之前检查：

| 上限 | 值 | 超出 |
|---|---|---|
| 括号 / 方括号 / 一元算子嵌套 | 64 | `Unsupported` |
| 表达式树深度 | 256 | `Unsupported` |
| 循环嵌套 | 4（同官方） | `Unsupported` |
| `matches` 正则的分组嵌套 | 64 | `Unsupported` |
| 每条规则每次求值的燃料 | 2000 万（正则按步计） | UNKNOWN |

顶到上限的各种形状在 1 MiB 栈的线程上走完（实测占用 ≤ 512 KB），几万层
的输入在数 token 时就被拒绝——`tests/yara_expr_depth.rs`。上限比官方紧：
65 层括号官方能编译，这里是 `Unsupported`（给理由，不静默）。

## 五、pe 对象模型

照官方 `pe` 模块的解析顺序与提前返回逐项镜像，包括整数怪癖和各表的
16384 项上限。已建模：

- 文件头 / 可选头标量、`linker_version` 等版本对、`data_directories[]`、
  `sections[]`、`overlay`、`entry_point`（文件偏移）与 `entry_point_raw`；
- 导入：`number_of_imports`、`number_of_imported_functions`、
  `import_details[].{library_name, number_of_functions, functions[].{name, ordinal, rva}}`、
  `imports(…)` 各种重载、`import_rva(dll, 名字 | 序号)`；
- 导出：`number_of_exports`、`export_timestamp`、`dll_name`、
  `export_details[].{name, ordinal, offset, rva, forward_name}`、
  `exports(…)`、`exports_index(…)`；
- 资源：`number_of_resources`、`resource_timestamp`、`resource_version`、
  `resources[]`、`version_info["键"]`、`version_info_list[]`；
- `rich_signature.{offset, length, key, raw_data, clear_data, version_data}`；
- `pdb_path`（NB10 / RSDS / MTOC 三种 CodeView 头）；
- 函数：`is_dll()` `is_32bit()` `is_64bit()` `section_index(…)`
  `rva_to_offset(…)`；全部整数常量（`pe.IMPORT_ANY` 是 -1）。

**按序号导入的函数名。** 官方给只有序号的导入合成名字：DLL 名以
`ws2_32.dll` / `wsock32.dll` / `oleaut32.dll` 开头（不分大小写的**前缀**
比较，所以 `oleaut32.dll.mui` 算、不带扩展名的 `ws2_32` 不算）时查内置
表，其余叫 `ord<N>`。表在 `sigscan/expr/ordnames.rs`：winsock 117 个名字
（序号 1..500），oleaut32 398 个（2..443）。来历是行为探针——造出从目标
DLL 按序号 1..65535 导入的合成 PE，读官方吐出的对象树——可逐序号复核
（见下节）。

**`pdb_path` 的平台差异。** 路径长度上限是官方的 `YR_MAX_PATH`，随平台
变；这里取对拍基准（Linux，4096）。在路径 ≥ 260 字节的样本上，Windows
版官方引擎的读数会不同，本轮没有测。

## 六、对拍

官方引擎只需要 `pip install yara-python==4.5.4`。四种对拍，判据都是
**DIFF 0**：

**1. 规则集 × 样本集**（任务书的验收命令）

```text
python -I scripts/yara_diff.py --trit <trit-re> \
    --rules crates/trit-re/tests/fixtures/yara_expr_rules.yar --samples <PE 目录>
```

样本：CPython 3.12.7 官方 Windows 嵌入式发行包里的 92 个带签名 PE
（x64 31、x86 30、arm64 31；12 个大于 1 MB）。规则 178 条（`a_` 算术、
`b_` 循环、`c_` 动态下标、`d_` 字符串算子、`e_` pe 对象）。

读数：**MATCH 92，DIFF 0，SUBSET-GAP 12，OFFICIAL-ERR 0**。16376 个
规则 × 样本裁决里 96 个弃权，落在 11 条规则上；143 条规则在样本间有
区分度，26 条恒命中、9 条按设计恒不命中。

**2. 测试期望值复核**——`tests/yara_expr*.rs` 里每张期望表拿去问官方：

```text
TRIT_EXPR_FIXTURE=<文件> cargo test -p trit-re --test yara_expr_pe
python -I scripts/yara_expr_probe.py --pe <文件>
```

读数：631 条期望（HIT 369、MISS 141、REJECT 90、UNKNOWN 26、GAP 5），
不一致 0。UNKNOWN / GAP 表里是 trit 弃权或拒绝、官方有结论的条目，脚本
核的是"官方确实有结论"。

**3. 对象树遍历**——从官方吐出的 `pe` 对象树反推规则：每个叶子一条
`== 值`，每个数组三条越界下标，每个缺失字段一条 `not defined`，按构造
官方全部命中；再让 trit 跑同一批规则。

```text
python -I scripts/yara_pe_walk.py --trit <trit-re> --samples <PE 目录>
```

读数（92 样本）：**OK 53803，ABSTAIN 689，GAP 0，DIFF 0**。

序号导入名逐序号复核（6 个 DLL 名 × 序号 1..65535，54 个合成 PE）：

```text
python -I scripts/yara_pe_walk.py --write-ordinal-pes <空目录>
python -I scripts/yara_pe_walk.py --trit <trit-re> --samples <该目录> --cap 20000
```

读数：OK 1187784，ABSTAIN 216，DIFF 0。

调试目录 / Rich 头的字节级变体（20 个）：

```text
TRIT_EXPR_VARIANTS=<空目录> cargo test -p trit-re --test yara_expr_variants
python -I scripts/yara_pe_walk.py --trit <trit-re> --samples <该目录>
```

读数：OK 5340，ABSTAIN 80，DIFF 0。

**4. 开发期差分批**——约 1500 条手写条件（含各类畸形写法）逐条比对，
脚本没有入库；它们发现的每一处不一致都已变成上面三种里的一条期望。

### SUBSET-GAP 登记（规则集对拍的 96 个弃权）

全部是同一个原因：**串落在 `SCAN_BUDGET` 之外，扫描没扫完，不拿"没找到"
当证据**（官方扫完整个文件）。出现在 12 个大于 1 MB 的样本上。

| 规则 | 弃权数 |
|---|---|
| `a_match_at_computed_offset` | 12 |
| `a_offset_of_missing_occurrence` | 12 |
| `a_string_counts_in_arithmetic` | 12 |
| `b_loop_over_strings_and_range` | 12 |
| `b_range_inside_string_loop` | 12 |
| `b_string_loop_inside_range` | 12 |
| `c_string_offset_by_expression` | 12 |
| `b_of_in_range` | 4 |
| `b_none_of` | 3 |
| `b_percent_of` | 3 |
| `b_n_of_with_expression` | 2 |

后四条只在 x86 的 4 个大文件上弃权：它们找的 `.pdata` 等节名 x86 映像
里没有，要扫完整个文件才能说"没有"；x64 / arm64 映像的节表里一开头就
找到了，结论已定。

### 对象树遍历的弃权登记（689）

| 字段 | 数 | 原因 |
|---|---|---|
| `pe.delayed_import_details[…]` | 321 | 延迟导入表未建模 |
| `pe.number_of_delayed_imports` | 92 | 同上 |
| `pe.number_of_delayed_imported_functions` | 92 | 同上 |
| `pe.number_of_signatures` | 92 | Authenticode 未建模 |
| `pe.is_signed` | 92 | 同上 |

遍历器跳过 `pe.signatures[]` 子树（同样弃权）。

## 七、与官方的已知差异

DIFF（两边都下结论而结论相反）在上述对拍集上是 0。下面是其余各类。

**弃权（官方有结论，trit UNKNOWN）**

- 延迟导入、签名（见上表）；`pe.imphash()`、`pe.calculate_checksum()`、
  `pe.locale(…)` / `pe.language(…)`、`pe.rich_signature.toolid(…)` /
  `version(…)`、`pe.delayed_import_rva(…)`。
- 没有 PE 上下文时所有 `pe.*` / `entrypoint` 条件都弃权。官方在非 PE 上
  多数不命中，但 `not pe.is_pe`、`defined pe.is_pe`、
  `not defined entrypoint` 命中。
- 串落在扫描预算外；循环跑完燃料。
- `@a[N]` 的 `N` 超出 C `int`（官方未定义）。

**拒绝（官方能编译，trit `Unsupported`）**

- 浮点数；`!a` 匹配长度；`@a["x"]`；`matches /…/s` 里依赖 `.` 匹配换行
  的写法；`\B`；超过第四节上限的嵌套。

**宽松（官方编译错误，trit 求值或弃权）**——都在旧引擎路径上，本轮没有
收紧，因为收紧会改变既有规则的判据：

- 旧字段别名（第四节）。
- `1kb`、`0x10KB`、`9223372036854775808`；`uint64(0)`、`int64(0)`；
  `matches /a**/`。
- 声明了却没引用的串、重复的串标识符。
- 不带表达式标志的非法 pe 用法：`pe.nope == 1`、`pe.machine.x`、
  `pe.is_dll == 1`、`pe.imports()`、`pe.sections[0].name == 5`——弃权而
  不是拒绝。

**既有的不一致，本轮没修**

- 空正则 `//` 仍被词法器当行注释（官方同款），本行余下内容不可见；
  其余三类（正则串 `/…/i`、hex 里的 `( 41 | 42 )`、条件里多出的 `)`）
  现在只让**该条规则** `Unsupported`，不再拖累整份文件。
- 条件里有 `{`、或串的值本身不被支持时，规则在到达表达式引擎之前就是
  `Unsupported`。
- 嵌套过深的理由文本不统一（多数是深度闸的原文，极长的 `-` 链会带上
  旧解析器的理由）。

**已修（基线上的不一致，现在一致）**

| 条件 | 基线 | 官方 / 现在 |
|---|---|---|
| `pe.is_dll() == 8192` | MISS | HIT（返回 `characteristics & 0x2000`） |
| `not (uint8(越界) == 1)` | HIT | MISS |
| `"\x41" == "A"` | MISS | HIT |
| `"hello" matches /HELLO/i` | 不支持 | HIT |
| `$r = /HELLO/ nocase` 对 `hello` | MISS | HIT（模式与数据两侧都折叠） |
| 单条规则里的 `/…/i`、hex `|`、多余 `)` | 整份文件 Err | 该条 Unsupported |
| 串在扫描预算外时的 `not $a`、`#a == 0` | HIT | 官方 MISS；现在 UNKNOWN |
| `for … of` 套 `for … of` | HIT | 编译错误 / `Unsupported` |
| `pe.sections[4294967296].name == ".text"` | MISS | HIT |

最后一条是写本文时才发现的，固定规则集没有覆盖到它。旧引擎路径上可能
还有同类未测到的不一致；"DIFF 0"说的是上面列出的对拍集，不是全部语法
空间。
