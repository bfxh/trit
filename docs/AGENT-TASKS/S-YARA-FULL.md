# S 级任务：YARA 完整语义（表达式语言 + 模块对象 + 官方差分 + 性质测试）

> 级别 **S**（3–10 天当量）；可 2–3 条子线并行；起点分支见 §1。
> 详细规范与可派发任务卡：`docs/YARA-EXPR/`（`00`–`05`）。本文件是高层验收卡。
> 这不是「把语法补一补」：判据是**官方引擎差分 + 三值性质测试 + 性能预算**
> 三条同时成立。

## 1. 起点

- 仓库：`bfxh/trit`；分支：`feat/pe-sections`（含 PR #457 的前置工作）。
- 环境：Rust 1.88+；`yara-python 4.5.4`（`uv` 或 pip）；N1 四象限样本目录。
- 基线：`cargo test -p trit-re` = 194 通过（以起点 SHA 实测为准）。

## 2. 必读

- `docs/SECURITY-RE.md` N4 行（历次官方差分结论与语义分歧）。
- `docs/CALIBRATION.md` §三（对拍 harness 与探针方法）。
- `crates/trit-re/src/yaraparse/{cond,cond_cmp,cond_leaf,cond_tail,rulebuild}.rs`
- `crates/trit-re/src/sigscan/{eval,resolve,strval,pepaths,sections,rich,regex}.rs`

## 3. 目标（每条都可判）

### G1 算术/位运算表达式
`+ - * / % & | ^ << >> ~`、一元 `-`、括号、官方优先级；除零/非法位移
在解析层 `Unsupported`。判据：合成表达式矩阵 × 官方输出逐条一致。

### G2 变量与区间迭代
`for any|all|N i in (lo..hi) : ( body )`，含嵌套与作用域；端点语义由
官方探针钉死。判据：≥40 条规则 × 官方差分 DIFF 0。

### G3 动态数组索引
`pe.sections[i].name`（`i` 为变量/算术）；越界 = 确定的未定义 → 假；
缺上下文 = 弃权。判据：边界样本（0 / n-1 / n / n+1）逐条断言。

### G4 字符串函数
`contains` / `icontains` / `startswith` / `istartswith` / `endswith` /
`iendswith` / `matches`；空串、大小写、非 UTF-8 字节由官方探针定案。

### G5 pe 模块对象
`pe.imports` / `pe.exports` / `pe.resources` / `pe.version_info` 的
`number_of_*` + 索引字段；子集外字段继续记名弃权。

### G6 三值性质测试（硬门槛）
新增 `tests/yara_props.rs`（或等价）：
- `Unknown` 参与任意新运算符/函数时不折叠成 `False`；
- Kleene 定律（交换/结合/吸收/De Morgan）在求值层成立；
- 单调性：证据增加时 `False → True/Unknown`，不得反向；
- 随机规则 × 随机样本 10k 判定对，不 panic、不挂起。

### G7 规模差分（硬门槛）
≥200 条规则 × N1 四象限 ≥50 个真实样本：**DIFF 0**；每个
`SUBSET-GAP` 逐条登记理由（文档表）。

### G8 性能预算
新增语法在既有规则集上扫描吞吐退化 ≤10%（同一机器、同一批样本、
至少 5 次取中位数）；`SCAN_BUDGET` 行为不回退。

## 4. 非目标

- 不实现 `math` / `hash` / `dotnet` / `elf` 模块（除非 G5 验证必需）。
- 不做 YARA-X 兼容；不运行样本（纯静态）。
- 不放宽任何既有三值判据。

## 5. 验收

```bash
cargo test -p trit-re
cargo clippy -p trit-re --all-targets --locked -- -D warnings
cargo fmt --all -- --check
python -I scripts/module_budget.py
python -I scripts/hazard_gate.py
python -I scripts/secret_scan.py
python -I scripts/docs_hygiene.py
python -I scripts/yara_diff.py --trit <exe> --rules <rules-v3.yar> --samples <N1>
```

期望：DIFF 0；测试数 ≥ 基线 + 新增；所有门禁退出码 0。

## 6. 禁区

- 不推 `main`；不修改 `module_budget_baseline.json` 的行数上限（只允许
  `--update` 登记新文件/收紧）。
- 不删除或弱化既有 `Unsupported`/`Unknown` 路径。
- 不把「官方不支持」的语法扩展成超集。

## 7. 失败模式

- 语义不可判定 → 先写官方探针（双候选/边界样本），不猜。
- 达不到性能预算 → 记录基线、退化幅度、原因，作为负结果入册。
- 官方引擎拒绝的语法 → 解析层同型拒绝，整条 `Unsupported`。

## 8. 交付物

- 代码 + 单元/集成/性质测试。
- `docs/YARA-EXPR.md`：语法表、优先级、三值传播表、已知差异。
- CHANGELOG / SECURITY-RE N4 / CALIBRATION 同步。
- PR 描述含：测试数、DIFF/SUBSET-GAP 计数、吞吐基线对比。

## 9. 自检

见 `docs/AGENT-TASK-STANDARD.md` §6；另加：G6 性质测试必须能在
`cargo test -p trit-re --release` 与 debug 两种构建下通过。
