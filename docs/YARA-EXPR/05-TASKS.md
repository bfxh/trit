# 任务卡：YARA 完整语义（T00–T32）

> 每张卡一个 PR。验收命令见 `README.md` §4；规范见 `01`–`04`。
> 卡上未写明的语义一律按红线 1 弃权，不猜。

---

## P0 基建（阻塞，必须先做）

### T00 接缝补丁与基线核对

- **当量**：0.5 天
- **前置**：仓库内提供 `ref/0001-seam.patch`（外部补丁）。
- **做什么**：
  1. `git checkout e0f37d8` 建分支，应用补丁，提交为「接缝」提交。
  2. 核对 `04-CODE-MAP.md` §2 的「已验证」文件全部存在，`y2.rs` /
     `pe2.rs` 仍是占位（`Err` / `Unknown`）。
  3. 跑 `cargo test -p trit-re`，确认新增的 `yara_expr_vectors` 回放
     打印接缝读数（369 / 2363 / 0）。
- **验收**：回放三数 = 369 / 2363 / 0；其余门禁全绿。
- **禁区**：不改旧文件；不删既有测试；不抬台账。
- **依赖**：无。

### T01 向量集、fixtures 与回放 harness

- **当量**：1 天（若向量需现生成，另加 0.5 天 + `yara-python 4.5.4` 环境）
- **前置**：外部提供 2732 条向量与 `pe_fields.tsv`；最小集已入库
  （`tools/gen_vectors.py` 生成 79 + 4 条）。
- **做什么**：
  1. 把 2732 条向量与 fixtures 入库（禁止手改）。
  2. `tests/yara_expr_vectors.rs`：按 `04-CODE-MAP.md` §5 的三态判法
     回放；`GAP_CEILING = 2363`；支持 `YARA_EXPR_SHOW` 分组。
  3. `tools/check_vectors.py` 对官方引擎复核；`tools/cite.py find '<条件>'`
     可按条件检索同类向量。
- **验收**：最小集回放 **9 / 74 / 0**；`YARA_EXPR_SHOW=A` 能列出 A 组弃权。
  全集向量接入后，`GAP_CEILING` 重设为全集非 ERR 条数。
- **禁区**：向量与 fixtures 只读；错判不是 0 就不许提交。
- **依赖**：T00。

### T02 预算台账与任务卡落地

- **当量**：0.5 天
- **做什么**：
  1. `git checkout e0f37d8 -- scripts/module_budget_baseline.json`。
  2. `python -I scripts/module_budget.py --update` 登记新文件、收紧旧文件。
  3. 把本文件复制成 `tasks/T*.md`（若智能体按文件读卡）。
- **验收**：`module_budget.py` 报 `capped == files`、`debt=0`。
- **禁区**：不手改 JSON；旧文件上限不变。

---

## P1 前端（`yaraparse/y2/`，与 P2 并行）

### T10 词法 `y2/lex.rs`

- **当量**：0.5 天
- **必读**：`02-FRONTEND.md` §1（含 1.8 全部单测表）。
- **产出**：`Tok` 枚举、`lex() -> Vec<Tok>`（永不失败，错误用 `Bad`）。
- **验收**：§1.8 的 11 行单测全部通过；`lex` 对任意输入不 panic、不挂起
  （每轮至少前进 1 字节）。
- **禁区**：空白只认 ` \t\r\n`；`0X`/`0o8`/`1e3` 等按规范拆成数字+标识符。

### T11 规则骨架 `y2/file.rs`

- **当量**：0.5 天
- **必读**：`02-FRONTEND.md` §2。
- **产出**：`parse_rules_v2` 的顶层与单规则切分；段顺序、重名、标签、
  未引用串检查；`Rule::from_parts(..., private)` + `with_flags`。
- **验收**：单规则内部错误只影响该规则（`Unsupported`），不拖累后续规则；
  规则名先记入表再解析下一条。
- **禁区**：`include` 整个文件 `Err`；未知 `import` 整个文件 `Err`。

### T12 strings 段 `y2/strings.rs`

- **当量**：0.5 天
- **必读**：`02-FRONTEND.md` §3；旧 `yaraparse/valuemod.rs`（语义不改）。
- **产出**：三种值（Text/Hex/Regex）+ 修饰符矩阵；`Pattern::text_bytes` /
  `base64_plan` / `regex` 接入。
- **验收**：重复修饰符、`xor`+`nocase`、hex 上的非 `private` 修饰 →
  规则 `Unsupported`；空文本串 → `Unsupported`。
- **禁区**：不改 `Pattern` 语义；非 UTF-8 + base64 → `Unscannable`。

### T13 条件框架 `y2/cx.rs`

- **当量**：0.5 天
- **必读**：`02-FRONTEND.md` §4.1–4.2。
- **产出**：`Cx`、`Node`、`TyX`、`prim` 传播、`parse_condition` /
  `parse_or` / `parse_and` / `parse_not` / `parse_rel`、深度计数（>200 报错）。
- **验收**：`not a == b`、`not a and b`、`defined a == b` 的结合性与
  规范一致；条件尾部多余 token → `Err`。
- **禁区**：括号分支统一走 `parse_or()`，不另写「只解析算术」。

### T14 算术与比较 `y2/cx_arith.rs`

- **当量**：1 天
- **必读**：`02-FRONTEND.md` §4.3–4.5；`01-SEMANTICS.md` §2、§3、§4。
- **产出**：优先级爬升、一元 `-`/`~`、`make_bin` / `make_cmp` /
  `make_strop` / `make_matches`、常量折叠与编译期除零/溢出检查。
- **验收**：`INT64_MIN * 2` 不报错、`1 * INT64_MIN` 报错；`>>` 的 konst
  用左移（官方笔误）；浮点 `==` 用 `DBL_EPSILON`。
- **禁区**：`konst` 只用于编译期判错，不改 AST；模块常量 `konst=None`。

### T15 原子与串引用 `y2/cx_atom.rs`

- **当量**：1 天
- **必读**：`02-FRONTEND.md` §4.4、§4.6；`01-SEMANTICS.md` §7。
- **产出**：字面量、`filesize`/`entrypoint`、`intN`、`#`/`@`/`!`、
  `$ref`/`at`/`in`、`parse_range`。
- **验收**：`@a[i]` 下标不查类型；`$a at x` 的 x 必须 Int；零命中
  `@b` 是 Undef 不是编译错误。
- **禁区**：`StrWild` 只能出现在串集合里。

### T16 of / for / 标识符 `y2/cx_of.rs`、`cx_for.rs`、`cx_ident.rs`

- **当量**：1.5 天（可拆三张子卡）
- **必读**：`02-FRONTEND.md` §5–§7；`01-SEMANTICS.md` §8–§10。
- **产出**：量词、串/规则集合、`for … of`、`for … in`（range/枚举/数组/字典）、
  循环栈与槽位（`2L` / `2L+1`）、模块路径 `walk_object`、`skip_opaque`。
- **验收**：循环嵌套 ≤4；内层变量不重名；`for … of` 不可嵌套；数组迭代
  必须 `[i]`；字典必须两个变量；`pe.all` 因关键字不是 `Ident` 而报错。
- **禁区**：集合是多重集不去重；`for` 的层号在进入前取。

### T17 `parse_rules_v2` 接入

- **当量**：0.5 天
- **做什么**：把占位 `Err` 换成真实实现；`yaraparse.rs` 只保留 `mod y2;`
  与 `pub use`（行数只减不增）。
- **验收**：回放弃权数低于 T16 后的 `GAP_CEILING`；错判 0。
- **依赖**：T10–T16。

---

## P2 pe 对象模型（`sigscan/pe2/`，与 P1 并行）

### T20 头部 / 数据目录 / 节表 / overlay

- **当量**：1 天
- **必读**：`03-PE-MODEL.md` §1–§3。
- **产出**：`build()` 的 PE 判定、COFF/可选头字段、数据目录、节表、
  `full_name`、overlay；非 PE 时 `is_pe=0` + `missing_undef:true`。
- **验收**：`pe_fields.tsv` 中头部/节表/overlay 相关向量错判 0；
  `entry_point` 映射不到时是 `-1`（有定义）。
- **禁区**：不用 `crate::pe::PeInfo`；所有读边界检查。

### T21 Rich 头

- **当量**：0.5 天
- **必读**：`03-PE-MODEL.md` §4。
- **产出**：`rich_signature` 结构 + `version`/`toolid` 函数。
- **验收**：`pe64_full` 的 `version(0x7809)==7`、`toolid(0x103)==14`、
  `version(0x7809,0x105)==2`；没有 Rich 时整个结点 `Undef`。
- **禁区**：解密按 4 字节小端；`len<16` 时函数 `Undef`。

### T22 导入表

- **当量**：1 天
- **必读**：`03-PE-MODEL.md` §5。
- **产出**：`import_details`、`number_of_imports`、
  `number_of_imported_functions`、`imports()` 重载。
- **验收**：`pe64_full` 的 3 DLL / 6 函数；非法 DLL 名与函数名被跳过；
  序号导入查 `ordinals`；延迟导入与正则重载按规范返回 `Unknown`。
- **禁区**：`call` 开头先看 `is_pe`；DLL/函数名比较不区分大小写。

### T23 导出表

- **当量**：1 天
- **必读**：`03-PE-MODEL.md` §6。
- **产出**：`export_details`、`number_of_exports`、`exports()` /
  `exports_index()` 重载。
- **验收**：`pe64_full` 3 个导出、序号 5–7；`exports(5)` 不命中
  （5 > 3）；forwarder 与 name/offset 的 Undef 规则一致。
- **禁区**：严格按 §6 的提前返回顺序；缺字段显式 `Undef`。

### T24 资源与版本信息

- **当量**：1.5 天
- **必读**：`03-PE-MODEL.md` §7。
- **产出**：资源遍历（含 `OffsetToData==0` 不前进的官方怪癖）、
  `resource_timestamp` / `resource_version`、`resources` 数组、
  `version_info` 字典与列表、`locale` / `language`。
- **验收**：`pe64_full` 的 4 项版本信息；`ProductName` 转窄为
  `caf\xe9 -`；`Empty` 是空串（有定义）；UTF-16 辅助函数的越界规则一致。
- **禁区**：收集上限 65536；`Size` 不在 `1..=0x3FFFFFFF` 不收集。

### T25 其余属性与缺口表

- **当量**：1 天
- **必读**：`03-PE-MODEL.md` §8–§9。
- **产出**：`is_dll()`（返回 `0x2000`/`0`）、`is_32bit`/`is_64bit`、
  `section_index`、`rva_to_offset`、`calculate_checksum`、`pdb_path`；
  §9 的未实现项在 `docs/YARA-EXPR.md` 缺口表逐条登记。
- **验收**：checksum 的折叠顺序与官方一致；`pdb_path` 三种签名。
- **禁区**：不为单个样本加特判。

---

## P3 收尾

### T30 切换旧前端

- **当量**：1 天
- **前置**：T10–T17、T20–T25 全部合入。
- **做什么**：按 `04-CODE-MAP.md` 的切换卡删除旧前端文件；`main.rs` /
  `yaraparse.rs` 只保留新入口；逐行改法以卡为准。
- **验收**：全量测试 + 回放错判 0；旧断言改动逐条列出对应向量 ID。
- **禁区**：单独 PR；不同时动 pe 模型。

### T31 文档与缺口表

- **当量**：0.5 天
- **做什么**：`docs/YARA-EXPR.md`（语法表、三值传播、已知差异、缺口表）；
  `CHANGELOG.md` Unreleased；`SECURITY-RE.md` N4；`CALIBRATION.md`。
- **验收**：`docs_hygiene` 过；每个 `Unknown`/`Unsupported` 有原因。

### T32 全量门禁与真实样本对拍

- **当量**：0.5 天
- **做什么**：全部门禁；`scripts/yara_diff.py` 对 N1 四象限样本跑
  `tools/gen_value_probes.py` 生成的逐字段规则；DIFF 0。
- **验收**：DIFF 0；`SUBSET-GAP` 逐条登记；测试数只增不减。

---

## 任务依赖图

```
T00 → T01 → T02
        ├→ T10 → T11 → T12 → T13 → T14 → T15 → T16 → T17
        └→ T20 → T21 → T22 → T23 → T24 → T25
                                   ↓
                              T30 → T31 → T32
```

**可并行**：`T10–T17` 与 `T20–T25` 两条线；`T30` 等两边合入。
