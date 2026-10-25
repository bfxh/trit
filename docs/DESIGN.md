# trit 的设计与定位

## 一句话

**可嵌入、可判红的三值推理工具箱**：让 `Unknown` 成为一等公民，并且能在 CI 里对策略/网表/公式给出
确定结论或反例。不做 EDA 全流程，也不做通用模型检验器。

## 系统分层

| 层 | 内容 | 本仓落位 |
|---|---|---|
| L0 语义 | K3 / Ł3 / B3 | `trit-core::algebra` |
| L1 表示 | AST/解析器、真值表、双导轨、位切片 | `trit-core` |
| L2 引擎 | 穷举参考、SAT 等价、BDD 等价、化简、网表分析 | `trit-eval` / `trit-sat` / `trit-bdd` / `trit-circuit` |
| L3 应用 | 决策表门禁、硬件 X 检查、教学 | `trit-policy` + `trit-cli` + `action/` |
| L4 工程 | 交叉验证、变异、基准、文档 | 各 crate 的 tests / examples |

## 表示不变量

- 三值量 `(ones, unknowns)`：`(0,0)=False`、`(1,0)=True`、`(0,1)=Unknown`，不允许 `(1,1)`。
- 二进制是 `unknowns = None`（或全零）的特例。`TriVec::from_binary` 接管 `Vec<u64>`，零拷贝。
- `Unknown` 平面惰性分配：纯二值路径不付第二平面的内存/带宽。
- 位级 `TriBit` 与标量 `Ternary` 是同一语义的两种粒度；转换不改变表示。

## 引擎与正确性

- **穷举是 oracle**：`trit-eval::equivalent` 用位并行真值表；`trit-sat` 必须与它逐条对拍。
- **SAT 是规模路径**：按代数真值表逐门加子句，K3/Ł3/B3 共用；反例必须能被 `eval` 复核。
- **最小反例是充分的**：删变量用对偶检查（“两边还可能相等吗”），UNSAT 才删；
  未列出的变量任取都保持区分，而不是只保证存在某个补全。
- **BDD 是 canonical 的第三条路径**：双导轨建 BDD，差集为 false 即等价；二值变量只建 t 导轨。
  规模依赖变量顺序与函数结构，可能与 SAT 互有胜负，按基准选。
- **量化按 trit 取值**：`∃x` / `∀x` 在 `{0,1,X}` 上量化（二值变量在 `{0,1}`），不是对两根导轨分别量化；
  `f` 指“确定为 True”。实现为 `∀x f = ¬∃x(valid_x ∧ ¬f)`，量化后仍对自由变量加合法性约束；
  见证/反例只含自由变量。
- **二值快速路径**：声明为二值的变量及其子图走经典 CNF（AND 3 / OR 3 / NOT 2 / XOR 4），
  200 变量内不再生成 unknown 导轨；100 变量 AND 链子句数 3866 → 598。
- **网表是独立语义**：组合逻辑、穷举输入；不做时序。
- 回归纪律：任何新语义先加穷举↔SAT 对拍，再加变异（改一个真值表项必须判红）。

## 决策表验证

- `trit verify` 读取 JSON 策略：`definitions` 先展开（检测循环），每条 `assertions` 在全合法赋值上检查。
- `assumptions` 是额外前提：先检查其可满足性，再检查 `assumptions ⇒ assertion` 恒真；前提不会改变变量的域。
- 三值变量取 `{0,1,X}`，`binary` 变量只取 `{0,1}`；断言恒真用 `expr ≡ 1` 判定。
- SAT 失败给充分最小反例；BDD 给 canonical 差集与完整赋值；任何失败退出码 1。
- Action 在调用方仓库里构建 trit 并跑 `verify`，策略路径相对调用方 workspace。

## 性能边界

| 路径 | 规模 | 手段 |
|---|---|---|
| 真值表 / 穷举 | ≤18 变量 | 位并行 64 路 |
| SAT 等价 | 数百变量（结构化公式） | dual-rail + CDCL |
| SAT ATPG / 桥接 | ≤64 主输入 | 逐故障 CNF，`--jobs` 按故障分块；结果顺序固定 |
| 大规模 SAT ATPG | 10k 门 / 40k 故障 | `--shared-cnf`：共享 CNF + selector，512 故障/块 |
| 策略断言 | 任意条数 | 前提只检查一次，断言间并行；结果顺序固定 |
| 参数化 Verilog | 参数组合 ≤256 | 源码特化后再走原解析/层级展开 |
| CLI 硬超时 | 任意子命令 | 重执行子进程 + kill；退出码 124 |
| 位切片批量 | 任意长度 | 2 bit/elem、按字运算 |
| 二进制互操作 | 任意长度 | `Option<unknowns>` + move-only 视图 |
| SDF 延迟注入 | celltype + per-instance | 实例最坏 IOPATH 优先；不做引脚级 |

## 非目标

- 归因规则见 [TERNARY-BOUNDARIES.md](TERNARY-BOUNDARIES.md)：A 原生红利 / B 模拟税 /
  C 生态路径依赖 / D 基数无关复杂度，四类不得混谈。
- 故障与检查策略见工作区策略（不在本仓，故不在此做链接）：bug 分
  K1 保留并告知 / K2 失败即停 / K3 故意探针；并发只接受「定义良好的无锁或显式随机」；
  检查按编译器式多层独立门，不堆同质断言。
- 时序模型检验、CTL/LTL、不动点（TVLA 那条线是另一个产品）。
- 硬件四态 `0/1/X/Z` 与 X-optimism（真值函数三值不够）。
- 通用 SMT / 完整 EDA 仿真。

## 关于上帝对象（对使用者与维护者分开看）

- **本库内部不允许上帝对象**：模块和类型按职责拆分，解析、求值、分析、导出、CLI 编排不塞进同一个
  类型或文件。这条约束针对维护者，不是针对调用方。
- **对外 API 不禁止用户组合大对象**：用户想把 `Expr` / `Circuit` 组合成多大、多集中的结构，是用户的
  建模自由。我们不做运行时或类型层面的“反上帝对象”限制，也不为极端最小化牺牲扩展性——那是吃力不讨好的事。
- **提醒可以，门禁不造**：Rust/Clippy 已有 `too_many_lines`、`too_many_arguments`、
  `struct_excessive_bools`、`type_complexity` 等现成 lint；需要提醒就启用/记录这些，不重复造自研检查器。
- **2026-10-24 修订**：加了唯一一道自研的**模块预算门**（`scripts/module_budget.py`）。
  触发条件是「拆模块之后又长回 1000+ 行」反复发生，而 clippy 的 `too_many_lines` 管的是函数不是文件。
  门的形态刻意做得最轻：单文件 800 行预算 + 一个**只允许变短**的台账 + 10 条自证；
  它只判「文件有没有变大」，不判「职责该不该拆」——后者仍然是评审的事。
- 判断标准是“职责是否可独立变化”，不是行数或字段数本身；为拆而拆同样会损失扩展性。

## 实验层（`publish = false`）

有七个 crate 是**实验层**：`trit-net`、`trit-machines`、`trit-codec`、`trit-float`、`trit-attn`、
`trit-dot`、`trit-gpu`。规则：

- **不属于稳定 API**：可以随时改签名、删模块，不进 crates.io，不发版；
- **彼此不许互相依赖**：`cargo package --workspace` 会连实验层一起打包，而指向未发布 crate 的
  路径依赖没有版本可填 ⇒ 打包失败。共享代码只能下沉到可发布 crate。这条由 `scripts/publish_graph.py` 强制；
- **入口是独立二进制**（`trit-net`、`trit-machines`、…），不并进 `trit` CLI，避免实验面污染稳定命令行；
- **结论要带回边界标签**：实验层里关于「三值红利」的结论必须按 `docs/TERNARY-BOUNDARIES.md` 归类，
  并同时给二进制基线（§6）与高压测试（§7）；
- **彼此不许互相依赖**的例外是 dev-dependencies（不进发布链，`cargo package` 剥离 path dev-deps，
  2026-10-24 实证并由 `scripts/publish_graph.py` 放行）。

## 不批的设计提案（存档：防止同一设计被反复重提）

- **2026-10-24，PR #116 的 `trit-codec::layout`（packed ↔ dual-rail `Bitplanes`）——不批**。
  问题一：`Bitplanes` 在 trit-codec 里**另设了第二套双导轨类型**，而双导轨的规范表示
  已下沉在可发布的 `trit-core::TriVec`（`bulk.rs`，含导轨维护与边界校验）——两套互不
  互通的类型意味着双份维护与两套非法导轨策略。问题二：`Bitplanes::get` 对非法 `(1,1)`
  导轨直接 `panic!`，违反 codec「非法输入显式报错而不是猜」的铁律（字段还是 `pub`，
  调用方可构造出非法态）。判据：`x^i mod g` 式的重复实现不做；要桥接 packed ↔ dual-rail，
  正确设计是**桥到 `TriVec`**（`trit-core` 已有 `get`/`set`/`try_from_binary`），无需新类型。
  该模块已从 PR #116 剔除（quantize / sparse / interleave / checksum 四组件设计可批，随 PR 合入）。
  重提条件：以 TriVec 为桥、非法导轨返回结构化错误、给出一个 codec 侧的真实消费方。

## 路线

- **v0.2**：五 crate workspace、二进制零开销桥、SAT 与网表并入。
- **v0.3**：反例最小化或 BDD 后端（二选一，按基准）；`trit verify` 决策表 + GitHub Action 模板。
- **v0.4**：Belnap 四值（`trit-belnap` + `trit belnap`，opt-in，不改三值）与 WASM Playground
  （`trit-wasm` + `playground/`，浏览器里跑求值/真值表/等价）。
- **v0.5**：静态审查整改——引脚级故障模型、解析深度与入口规模上限、Action SHA 固定、
  文档与格式边界澄清。
- **v0.6**：库级统一预算 `Limits`（深度/节点/枚举变量数）覆盖 eval/SAT/BDD/策略入口，
  CLI 与 WASM 共用；关键规则的变异门（12/12）纳入每周 CI。
- **v0.7**：ATPG（二进制域可测性、原因分类、按门类型覆盖率、全向量/精确最小覆盖）；
  BDD 静态变量排序与节点预算；变异门扩到 17 条。
- **v0.8（已完成，0.8.1）**：芯片链路——结构化 Verilog 子集、索引化仿真、STA-lite（按门类型延迟 +
  top-N 路径）、NAND-only 与时序驱动技术映射、`trit check` 流水线、ATPG X don't-care、
  桥接故障、多比特 Verilog；变异门扩到 53 条。
- **v0.9（已完成，0.9.0）**：bus 元数据、Verilog 尺寸常量（b/o/d/h）与网表 JSON `format` 版本。
- **v0.10（已完成，随 0.11.0 发布）**：Verilog 多模块实例（标量 + `[N:0]` / `[0:N]` 向量端口，
  命名 / 位置连接、层级展开、生成名冲突检测）；SAT-based ATPG（≤64）、桥接故障 SAT（≤64）、
  映射面积（`--area-first` 按 area）、STA I/O 约束（`input_arrival` / `output_required`）。
- **v0.11（已完成，0.11.0）**：并行 ATPG/桥接、共享 CNF（10k 门/40k 故障 2.6 s）、
  策略断言并行；SDF per-instance；Verilog 整数参数/实例覆盖；CLI 全局硬超时。
- **v0.12（进行中）**：`trit-schedule` 静态三值 DAG 调度（True/False/Unknown、多 worker、
  关键路径）、依赖驱动 runtime（协作式取消、失败停止派发、cpu/memory 配额、严格优先级准入）
  与可恢复任务协议（`step` 检查点、步间抢占、状态保留）已完成；线程版
  `run_resumable_threaded`、统一 `RuntimeMetrics`、Prometheus 文本导出与检查点协议
  （`CheckpointTask`/`CheckpointStore`/`CheckpointedTask`）已接上；不做运行中强杀。
  检查点支持内存与版本化文件存储（原子写新代、prune、多任务索引、跨进程锁），
  并可从存储恢复任务图（`restore_specs`）；分布式存储是下一步。
  下一步 SDF 引脚级/TIMESCALE、Liberty/SDC、100k 门、Python/C 绑定与 1.0 API 收口。

## 参考

- Kleene 1952（K3）；Łukasiewicz；Bochvar 1938（B3）。
- Belnap 1977, *A Useful Four-Valued Logic*。
- Sagiv, Reps, Wilhelm 1999/2002, *Parametric Shape Analysis via 3-Valued Logic*（TVLA）。
- Bruns & Godefroid 1999, *Model Checking Partial State Spaces with 3-Valued Temporal Logics*。
- Chechik et al., *Multi-Valued Symbolic Model-Checking*。
- IEEE 1364 四态与 X-propagation；Bryant 1986（BDD）；Tseitin 1968（CNF）。
  单遍 Kleene 传播不保留重汇聚相关性；`propagate_exact` 用完成枚举在预算内给出精确结果。
