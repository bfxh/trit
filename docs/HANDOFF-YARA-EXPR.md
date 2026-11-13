# HANDOFF：YARA 表达式语言与模块对象模型（超级难度）

> 给接手的工程师/模型：这是一个**独立可验证**的高难度任务。起点分支
> `feat/pe-sections`（GitHub `bfxh/trit`），前置工作已把 YARA 子集推进到
> 「标量属性 + 数组式 + rich_signature + private + 字符串等值」。本任务把
> 它升级为**完整表达式语言 + 模块对象模型**，验收标准是与官方
> `yara-python 4.5.4` 的命中集合 **DIFF 0**。

## 0. 先读什么（按顺序）

1. `docs/SECURITY-RE.md` 的 N4 行——当前子集边界与历次官方差分结论。
2. `docs/CALIBRATION.md` 的「官方 YARA 对拍」一节——harness 与探针方法。
3. 代码起点：
   - 解析：`crates/trit-re/src/yaraparse/{cond,cond_cmp,cond_leaf,cond_tail,rulebuild}.rs`
   - 求值：`crates/trit-re/src/sigscan/{eval,resolve,strval,pepaths,sections,rich}.rs`
   - 三态类型：`resolve::Resolved`（数值）、`strval::StrResolved`（字符串）、
     `trit_core::value::Ternary`（规则裁决）。

## 1. 目标（全部必须）

### A. 算术与位运算表达式
`+ - * / % & | ^ << >> ~`、一元 `-`、括号、官方优先级；除零/非法位移 =
编译错误（与官方同型，解析层 `Unsupported`，不静默弃权）。

### B. 变量与区间迭代
```yara
for any i in (0..pe.number_of_sections - 1) : (
    pe.sections[i].name == ".text"
)
for all i in (1..N) : ( uint8(i) != 0 )
```
循环变量作用域、嵌套、`0..N` 端点语义（含/不含）以官方探针为准。

### C. 动态数组索引
`pe.sections[i].name`（`i` = 变量/算术表达式）；越界 = **确定的未定义**
→ 比较为假（沿用现有三态纪律）；缺上下文 = 弃权。

### D. 字符串函数
`contains` / `icontains` / `startswith` / `istartswith` / `endswith` /
`iendswith` / `matches`。只允许字符串操作数；`matches` 复用现有 Pike VM。
每个函数的空串/大小写/非 UTF-8 字节语义用官方探针钉死。

### E. 模块对象扩展（pe）
`pe.imports` / `pe.exports` / `pe.resources` / `pe.version_info` 的数组式
访问（至少 `number_of_*` + 索引字段）；字段集合以官方文档 +
`yara-python` 探针为准。子集外字段继续**记名弃权**（`Unknown`），不猜。

### F. 三值纪律（硬约束）
任何新结构都必须定义 `Unknown` 传播；**未知绝不折叠成假**。解析不了的
官方编译错误同型语法 → 整条规则 `Unsupported`（带原因），不静默跳过。

## 2. 非目标

- 不实现 `math` / `hash` / `dotnet` / `elf` 模块（除非为验证 pe 所需）。
- 不做 YARA-X 兼容。
- 不运行被扫描样本（纯静态）。
- 不修改既有三态判据的含义（只允许加证据面，不允许放宽）。

## 3. 验收（可复算）

```bash
cargo test -p trit-re
cargo clippy -p trit-re --all-targets --locked -- -D warnings
cargo fmt --all -- --check
python -I scripts/module_budget.py
python -I scripts/hazard_gate.py
python -I scripts/secret_scan.py
python -I scripts/docs_hygiene.py
python -I scripts/yara_diff.py --trit <trit-re.exe> --rules <new_rules.yar> --samples <N1 样本目录>
```

判据：
- 官方差分 **DIFF 0**；每个 `SUBSET-GAP` 必须在文档登记理由（不是失败，
  但必须逐条解释）。
- 测试数只增不减；`module_budget` 不抬台账（要腾地方就拆文件）。
- CHANGELOG / SECURITY-RE N4 / CALIBRATION / 新增 `docs/YARA-EXPR.md` 同步。

## 4. 交付物

1. 代码 + 单元/集成测试（含对抗性语法：除零、溢出、嵌套循环、越界索引）。
2. `docs/YARA-EXPR.md`：语法表、优先级、三态传播表、与官方的已知差异。
3. CHANGELOG、SECURITY-RE N4、CALIBRATION 更新。
4. PR 描述含实测读数（MATCH/DIFF/SUBSET-GAP 计数 + 测试数）。

## 5. 已知陷阱（省时间用）

- `for ... in (range)` 的端点与空区间语义（官方探针跑两套边界样本）。
- 算术溢出：官方用 64 位回绕还是拒绝？先探针再实现。
- `pe.sections[i]` 越界在迭代体内 = 确定的未定义（不是 Unknown）。
- 字符串函数对 `wide`/非 UTF-8 字节的行为；`matches` 只吃现有正则子集。
- Unknown 参与算术：官方先短路还是整体 undefined？逐运算符钉死。
- 预算棘轮很紧：新逻辑优先拆新文件（`cond_arith.rs`、`strfunc.rs` 等），
  不改台账上限。

## 6. 起点与协作

```bash
git clone git@github.com:bfxh/trit.git
cd trit
git switch feat/pe-sections
```

参考 PR：#457（`pe.sections[N]` + private + `.name` + rich_signature 三潜）。
完成后按项目惯例提交、push、开 PR；不要直接推 `main`。
