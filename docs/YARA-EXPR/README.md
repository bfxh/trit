# YARA 完整语义实现包（规范 + 任务卡）

> 状态：**规范 + P0 最小向量/harness 已入库；接缝补丁仍缺**（2026-11-14）。本目录的 00–04 是
> 外部提供的规范原文；`05-TASKS.md` 是按 `docs/AGENT-TASK-STANDARD.md`
> 整理出的可派发任务卡。**在 P0 基建补齐之前，不要派发实现卡。**

## 1. 这套东西是什么

把 trit-re 的 YARA 子集从「标量属性 + 手写递归下降」升级为**官方
YARA 4.5.4 语义的完整前端 + pe 对象模型**：

- 新前端 `crates/trit-re/src/yaraparse/y2/`：词法、规则骨架、strings、
  条件表达式（算术/比较/字符串算子/of/for/模块路径）。
- 新 pe 模型 `crates/trit-re/src/sigscan/pe2/`：从原始字节构造对象树，
  覆盖头部/节表/overlay/Rich/导入/导出/资源/版本信息。
- 验收：`tests/data/yara_expr/*.tsv` 的 **2732 条官方向量**回放，
  **错判必须为 0**，弃权数只降不升。

规范文件：

| 文件 | 内容 |
|---|---|
| `00-RULES.md` | 七条红线、工作方式、预算台账、门禁 |
| `01-SEMANTICS.md` | 官方语义（三态、算术、浮点、字符串、of/for、模块对象） |
| `02-FRONTEND.md` | 词法、规则骨架、strings、条件解析、文件拆分 |
| `03-PE-MODEL.md` | pe 对象模型（伪代码 + 字段表 + 读数） |
| `04-CODE-MAP.md` | 接缝补丁后的代码地图、已验证/待写清单 |
| `05-TASKS.md` | 任务卡（T00–T32）与依赖顺序 |

## 2. 现状核对（2026-11-14）

仓库 `origin/main`（`0f833b3d`）与起点 `e0f37d8` 上**都没有**下列文件，
规范里描述的「接缝阶段」不存在：

| 规范里引用的东西 | 仓库现状 |
|---|---|
| `ref/0001-seam.patch`（接缝补丁） | **缺失**（仍阻塞） |
| `tests/data/yara_expr/*.tsv` + `fixtures/` | **部分入库**：83 条可复现最小集（`tools/gen_vectors.py` 生成；非 2732 条全集） |
| `tests/yara_expr_vectors.rs`（回放 harness） | **已入库**（最小集回放） |
| `tools/gen_vectors.py` | **已入库**；`check_vectors.py` / `cite.py` / `gen_value_probes.py` 仍缺 |
| `tasks/`（任务卡原文） | **缺失**（本目录 `05-TASKS.md` 是整理版） |
| `crates/trit-re/src/yaraparse/y2.rs` | **占位已入库**（`Err`） |
| `sigscan/pe2.rs`、`xscan.rs`、`xeval.rs` 等接缝模块 | **缺失** |

⇒ **红线 3（错判必须 0）现在无法执行**：没有向量就没有验收。按
`00-RULES.md` 的红线 1，也不能靠「读规范猜语义」硬写。

### P0 已交付（2026-11-14）

- `tools/gen_vectors.py`：用官方 `yara-python 4.5.4` 生成向量（规则源写在
  脚本里，可复现）；当前 **79 条 expr + 4 条 rule_level**，fixtures =
  `text` / `bytes256` / `empty`。
- `crates/trit-re/tests/data/yara_expr/`：TSV + `.hex` fixtures。
- `crates/trit-re/tests/yara_expr_vectors.rs`：回放 harness，
  `GAP_CEILING = 74`；当前基线 = **一致 9 / 弃权 74 / 错判 0**
  （`parse_rules_v2` 占位 `Err`）。
- `crates/trit-re/src/yaraparse/y2.rs`：解析入口占位。
- **已知与外部规范的差异**（以向量为准）：官方 4.5.4 实测
  `"abc" contains ""` 与 `"" contains ""` 都是 MISS（外部规范 §6.2 写
  「空针恒命中」）；`for any i in (2..1)` 是编译错误（常量区间端点检查）。
  这些已冻结在 TSV 里。
- 仍缺：`ref/0001-seam.patch`（`xscan`/`xeval`/`xval` 等接缝模块）与
  2732 条全集向量；**P0 补丁完成前不能派发实现卡**。

## 3. 执行顺序

```
P0 基建（阻塞，必须先做）
  T00 应用接缝补丁 + 基线核对
  T01 向量集 / fixtures / 回放 harness / 工具
  T02 预算台账与任务卡落地
      ↓
P1 前端（可并行于 P2）
  T10 lex.rs → T11 file.rs → T12 strings.rs
  T13 cx.rs → T14 cx_arith.rs → T15 cx_atom.rs → T16 cx_of/cx_for/cx_ident.rs
  T17 parse_rules_v2 接入
      ↓
P2 pe 模型（可并行于 P1）
  T20 头部/目录/节表/overlay → T21 Rich → T22 导入
  T23 导出 → T24 资源/版本信息 → T25 其余 + 缺口表
      ↓
P3 收尾
  T30 切换旧前端 → T31 文档/CHANGELOG → T32 全量门禁 + 真实样本对拍
```

**并行约束**：T10–T17 与 T20–T25 互不依赖（前端只依赖 `sigscan::expr`
结点，pe 只依赖 `modreg::Node`），可以两条线同时推；T30 必须等两边都
完成，且单独一个 PR。

## 4. 验收链（每张卡都跑）

```bash
cargo fmt --all -- --check
cargo clippy -p trit-re --all-targets --locked -- -D warnings
cargo test -p trit-re --locked
cargo test -p trit-re --test yara_expr_vectors -- --nocapture
git checkout e0f37d8 -- scripts/module_budget_baseline.json
python -I scripts/module_budget.py --update
python -I scripts/module_budget.py
python -I scripts/hazard_gate.py
python -I scripts/secret_scan.py
python -I scripts/docs_hygiene.py
python -I scripts/test_surface.py
```

回放输出三个数：**一致 / 弃权 / 错判**。`错判 != 0` 一律不许提交；
每张卡把 `tests/yara_expr_vectors.rs` 的 `GAP_CEILING` 调到本次弃权数
（只许降）。

接缝阶段基线（规范 `04-CODE-MAP.md` §5）：

| 向量文件 | 一致 | 弃权 | 错判 |
|---|---:|---:|---:|
| `expr_core.tsv`（1408） | 334 | 1074 | 0 |
| `rule_level.tsv`（72） | 35 | 37 | 0 |
| `pe_fields.tsv`（1252） | 0 | 1252 | 0 |
| **合计（2732）** | **369** | **2363** | **0** |

## 5. 工作量与可行性评估

| 阶段 | 卡数 | 当量（智能体） | 说明 |
|---|---:|---:|---|
| P0 基建 | 3 | 1–2 天 | 依赖外部补丁/向量；如果向量要现生成，需 `yara-python 4.5.4` 环境 |
| P1 前端 | 8 | 4–6 天 | 词法/语法为主，`01-SEMANTICS` 覆盖充分 |
| P2 pe 模型 | 6 | 6–9 天 | **最重**：1252 条逐字段向量；资源/版本信息是深水区 |
| P3 收尾 | 3 | 2–3 天 | T30 删旧前端，影响面最大，必须单独 PR |
| **合计** | **20** | **约 15 天当量** | 两条实现线并行时，墙钟约 2–3 周 |

**可行性判断**：

- 规范质量高：每条语义都带向量 ID / 官方源码依据，三态纪律与现有
  `Unknown` 不折叠原则一致，适合交给智能体执行。
- **最大阻塞不是代码，是验收资产**：补丁、2732 条向量、回放 harness
  都不在仓库。没有它们，任务只能写成「实现完再对拍」，而红线和门禁
  都要求每个 PR 错判 0。
- 预算台账很紧：旧文件余量为 0（`sigscan.rs` 280/280、`eval.rs` 213/213、
  `rule.rs` 87/89、`scan.rs` 43/101、`yaraparse.rs` 285/286）；所有新逻辑
  必须落新文件，且新文件目标 ≤400 行。
- T30 会删旧前端并改 `main.rs`/`yaraparse.rs`，是唯一允许改旧断言的卡；
  必须单独评审。

## 6. 建议

1. **先补齐 P0**：把 `ref/0001-seam.patch`、`tests/data/yara_expr/`、
   `tools/check_vectors.py` 三者入库（或授权用 `yara-python 4.5.4` 现生成
   并冻结）。没有这一步，后面的卡都不满足「可复算验收」。
2. P0 完成后，T10–T17 与 T20–T25 分两条线并行；每张卡一个 PR，互不阻塞。
3. T30 等两条线合入后再做，单独 PR，并在描述里列出被替换的旧断言与
   对应向量 ID。
4. 任何「规范没写、向量没覆盖」的情况按红线 1 弃权并登记，不要猜。

## 7. 与主库其他文档的关系

- 高层任务卡：`docs/AGENT-TASKS/S-YARA-FULL.md`（S 级四硬门槛）。
- 任务包标准：`docs/AGENT-TASK-STANDARD.md`。
- 现有 YARA 子集读数：`docs/SECURITY-RE.md` N4 行、`docs/CALIBRATION.md`。
