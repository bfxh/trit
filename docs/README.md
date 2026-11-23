# 文档索引

> **唯一入口。** 仓里每篇文档都必须在这张表里有一行、且至少有一个别的文档指向它。
> 新增、搬迁、改名都要同时改这里——流程见 [specs/DOC-INDEX.md](specs/DOC-INDEX.md)，
> 一篇文档本身怎么写见 [specs/DOCUMENTATION.md](specs/DOCUMENTATION.md)。

## 先读两篇

- [positioning/DESIGN.md](positioning/DESIGN.md)：主库存什么、系统分层、「三进制」在这里指什么。
- [positioning/STATUS.md](positioning/STATUS.md)：四个状态词（跑通 / 实现未测 / 组件 / 否证）的含义与
  举证要求，判定脚本 `scripts/claims_evidence.py`。

## 目录分组

| 目录 | 放什么 | 不放什么 |
|---|---|---|
| `positioning/` | 定位、边界、分层、状态口径、路线图 | 具体读数与实验过程 |
| `reference/` | 组件地图、格式规范、模型卡、素材与容量口径 | 结论性读数（放 `results/`） |
| `results/` | 可复算的读数与结论（含负结果、未证栏） | 只有过程没有结论的日志 |
| `research/` | 研究线（`WSP-*`）的方法、读数、未证栏 | 已进入稳定 API 的能力（写进 crate 文档） |
| `security/` | 威胁模型、红队与防御沙盘、并发与高危政策 | 对真人的定位、攻击反制 |
| `engineering/` | 构建、worktree、更新与运维 | 功能设计 |
| `plans/` | 尚未落地的计划草案（每条都要写「边界」） | 已经落地的能力（挪进 `results/` 或 crate 文档） |
| `specs/` | 写作与治理规范本身 | 任何实验结论 |
| `AGENT-TASKS/` | 可直接派发给外部智能体的任务包（按 `S/A/B/C` 标级） | 需求讨论、过程记录 |
| `YARA-EXPR/` | YARA 完整语义的规范原文与可派发任务卡 | 实现进度与读数（放 `results/`） |
| `review/` | 按日期的评审记录（历史，不改写） | 活跃文档 |

## positioning/ — 定位与边界

| 文档 | 一句话 |
|---|---|
| [DESIGN.md](positioning/DESIGN.md) | 设计与定位：可嵌入、可判红的三值推理底座；系统分层与表示不变量 |
| [SCOPE.md](positioning/SCOPE.md) | 仓库边界：收什么、不收什么（数据 / 二进制 / 跨仓分工） |
| [LAYERS.md](positioning/LAYERS.md) | 新代码归位判据（算子 / 算法 / 学习 / 生产链）+ `scripts/layers.py` |
| [TERNARY-BOUNDARIES.md](positioning/TERNARY-BOUNDARIES.md) | 三进制四类问题不能混谈（A 红利 / B 模拟税 / C 路径依赖 / D 基数无关） |
| [STATUS.md](positioning/STATUS.md) | 状态口径：状态词、读数与复算命令的硬要求 |
| [ROADMAP.md](positioning/ROADMAP.md) | 主库路线图：该搬进 / 该新开 / 该搬出 |

## reference/ — 组件、格式与口径

| 文档 | 一句话 |
|---|---|
| [ATLAS.md](reference/ATLAS.md) | 组件地图：前端 / 后端 / 语义层 / 横切工具 |
| [TERNARY-TOOLCHAIN.md](reference/TERNARY-TOOLCHAIN.md) | 三进制构建工具链（TTIR → TRB2）的定位、论文依据与边界 |
| [TTX.md](reference/TTX.md) | TTX 三进制资产容器格式的竞争力定位与规范 |
| [MODEL-CARD.md](reference/MODEL-CARD.md) | 部件平面头（`trit-parts`）的模型卡：口径、读数、边界 |
| [DATASETS.md](reference/DATASETS.md) | 训练素材清单与可访问性实测 |
| [CAPACITY.md](reference/CAPACITY.md) | RAM / 显存 / CPU / GPU 容量预测与防爆设计 |
| [CALIBRATION.md](reference/CALIBRATION.md) | N1 校准工作手册（真实样本批次作业指导） |
| [N1-ARM64-CALIBRATION.md](reference/N1-ARM64-CALIBRATION.md) | N1 校准：真实 ARM64 PE 上的交叉引用读数 |
| [AGENT-DATA.md](reference/AGENT-DATA.md) | 智能体/语言数据质量门 M0–M2 与数据卡判据 |
| [CODEBASE-DATA.md](reference/CODEBASE-DATA.md) | 大型代码库数据：RepoUnit、依赖图、仓库级质量门 |
| [REPO-SCAN.md](reference/REPO-SCAN.md) | 本地代码库全量扫描读数：依赖图、测试关联、去重、上下文 |
| [TRACE-BACKENDS.md](TRACE-BACKENDS.md) | 轨迹后端：JSONL 或 trit-db 的取舍与迁移 |

## results/ — 读数与结论

| 文档 | 一句话 |
|---|---|
| [RESULTS.md](results/RESULTS.md) | 结果总目录（自动生成）：各族读到哪一步 |
| [TEST-MATRIX.md](TEST-MATRIX.md) | 测试扩面：为什么是这些格子（四 crate 补测的读数与判据） |
| [DEPTH-RESULTS.md](results/DEPTH-RESULTS.md) | 深度头结果（自动生成） |
| [PARTS-RESULTS.md](results/PARTS-RESULTS.md) | 部件头结果（自动生成） |
| [TEXTURE-RESULTS.md](results/TEXTURE-RESULTS.md) | 纹理容器结果（自动生成） |
| [EXTRACT-RESULTS.md](results/EXTRACT-RESULTS.md) | 提取结果（自动生成） |
| [WSP3-RESULTS.md](results/WSP3-RESULTS.md) | WSP3（RLE 二级压缩）在真实 MiniLM 上的实测：无收益 |
| [WSP-E2E-RESULTS.md](results/WSP-E2E-RESULTS.md) | 真实模型端到端 bias 读数（all-MiniLM-L6-v2 第 0 层） |
| [WSP-STS.md](results/WSP-STS.md) | 真句向量（STS 风格 pair）上的编辑集门禁 |
| [WSP-STS-BENCH.md](results/WSP-STS-BENCH.md) | 真实 STS benchmark（1,379 行）上的编辑集门禁 |
| [WSP-STS-TUNED.md](results/WSP-STS-TUNED.md) | 真实 tuned 权重（非合成 Δ）上的编辑集门禁 |
| [LESSONS.md](results/LESSONS.md) | 经验与负结果（先读这篇再谈优化） |
| [COUNT-EVOLUTION.md](COUNT-EVOLUTION.md) | 变量序进化 × 随机 3-SAT 计数的同预算消融（负结果：1.17× 读数属噪声） |

## research/ — 研究线（WSP 权重空间编程 + 扫描线）

| 文档 | 一句话 |
|---|---|
| [WSP-RESEARCH-ROADMAP.md](research/WSP-RESEARCH-ROADMAP.md) | 权重空间编程的论文地图、技术树（L0–L7）与任务清单 |
| [WSP-INTERNAL-LOGIC.md](research/WSP-INTERNAL-LOGIC.md) | 从「读特征」升级到「读程序」：最小自动机提取、轨迹承重、Tracr 反向闭环的判据与 M0 |
| [WSP-INTERNAL-LOGIC-RESULTS.md](research/WSP-INTERNAL-LOGIC-RESULTS.md) | M0 读数：三值网络 → 最小 DFA 的提取与参考对拍（DIFF=0，7/7 状态） |
| [WSP-LIMITS.md](research/WSP-LIMITS.md) | `trit-wspace` 未证栏汇总（每条给来源与补证所需） |
| [WSP-E2E-REAL-ACT.md](research/WSP-E2E-REAL-ACT.md) | 真实激活校准：MiniLM 第 0 层三段的随机 vs 真实输入对照 |
| [WSP-BERT6-RESULTS.md](research/WSP-BERT6-RESULTS.md) | MiniLM 完整 6 层：f32 对照与三值线性权重（随机输入方向翻转） |
| [WSP-BLOCK-INDEX.md](research/WSP-BLOCK-INDEX.md) | WSP2 块索引与随机访问 |
| [WSP-WRITE-LOCK.md](research/WSP-WRITE-LOCK.md) | CLI 并发写锁（`--out` / `--log` 分发前加锁） |
| [WSP-TILE-EDIT.md](research/WSP-TILE-EDIT.md) | WSP1 tile 编辑：任意矩形块的事务编辑与审计 |
| [WSP-MODEL-PACK.md](research/WSP-MODEL-PACK.md) | 整模型批量容器 WSPM |
| [WSP-BOUND.md](research/WSP-BOUND.md) | `repair` 上界：候选空间内最少改几个 trit 能达成 edit 集 |
| [WSP-CERTIFY.md](research/WSP-CERTIFY.md) | `certify`：保持集副作用证书 |
| [WSP-EDITSET.md](research/WSP-EDITSET.md) | 编辑集门禁：真实 384 维输入上的达成率 / 破坏率 |
| [WSP-FFN.md](research/WSP-FFN.md) | 两层 FFN 上的编辑集门禁（单层表达力上限） |
| [WSP-ALIGNED.md](research/WSP-ALIGNED.md) | 阈值对齐后的单层 / 两层对照 |
| [WSP-REPAIR.md](research/WSP-REPAIR.md) | `repair` 路线尝试：修复已知错误样本（负结果） |
| [WSP-TIES.md](research/WSP-TIES.md) | `task-merge --trim`：TIES 的裁剪步骤 |
| [WSP-LEARNED-PROPOSER.md](research/WSP-LEARNED-PROPOSER.md) | 学习式单点编辑提议器 |
| [WSP-GRADIENT-PROPOSER.md](research/WSP-GRADIENT-PROPOSER.md) | 从手工特征到梯度提议器 |
| [WSP-GPU-SCALE.md](research/WSP-GPU-SCALE.md) | 更大模型与 GPU 落点 |
| [WSP-EVOLUTION.md](research/WSP-EVOLUTION.md) | 三值红蓝共同进化：想法进化的多智能体机制 |
| [WSP-ADV-TRAIN.md](research/WSP-ADV-TRAIN.md) | 对抗训练：蓝方吃进化攻击样本（RowDropout 行级注入） |
| [WSP-PRIMING.md](research/WSP-PRIMING.md) | 三值创造性预激发：悖论假设检验（本配置不显著+代价曲线） |
| [WSP-ADAPTIVE-DEFENSE.md](research/WSP-ADAPTIVE-DEFENSE.md) | 自适应防御：探针生成器 + 对抗集成（50× 探针效率；初始化轨迹脆弱性 5/6 塌缩） |
| [WSP-ROW-GUARD.md](research/WSP-ROW-GUARD.md) | 输出行架构冗余：行复制 + 分歧告警（击杀成本 ×K；tanh 饱和反取证发现） |
| [WSP-DGA-QAT.md](research/WSP-DGA-QAT.md) | 三值 QAT：DGA 域名检测与家族归因（真实安全数据集） |
| [GGUF-TRIT.md](research/GGUF-TRIT.md) | 三值权重 GGUF 容器（`trit-export`）的最小 v3 子集 |
| [TERNARY-AUDIT.md](research/TERNARY-AUDIT.md) | 三进制专项评估：编码可逆、逐位一致、有损量化与 WSP 模型级 GGUF 复验（58,945 + 237 例 + 103 张量） |
| [FORK-FLUSH.md](FORK-FLUSH.md) | Fork-and-Flush：多智能体任务分叉 + 清空 + 按验收分数选优 |
| [CRAM-3D-DRIFT.md](research/CRAM-3D-DRIFT.md) | CRAM + 3D 漂移：S 级任务包与 2D 最小切片（设计，未开工） |
| [CRAM-3D-DRIFT-RESULTS.md](research/CRAM-3D-DRIFT-RESULTS.md) | M0 负结果：轨迹生成器 0% 可行（约束不在 loss） |
| [CRAM-3D-DRIFT-COND.md](research/CRAM-3D-DRIFT-COND.md) | 真条件生成：goal 独立通路（代码就位；两轮负结果：条件能被用起来、路径仍画不出） |
| [CRAM-3D-DRIFT-CREATIVE.md](research/CRAM-3D-DRIFT-CREATIVE.md) | 把创造力接进漂移模型：过程/产品解耦（primer 分层 + 掩码解码 + 极小适配器，设计未开工） |
| [GAME-SCAN.md](research/GAME-SCAN.md) | 游戏环境扫描、故障注入与恢复（Phase 0） |
| [DEEP-RISK-MAP.md](research/DEEP-RISK-MAP.md) | 深层风险地图：更新 / 认领 / 牵连 |
| [DEVICE-ADAPT.md](DEVICE-ADAPT.md) | 行为级设备仿真 × 设备适配策略：CPU / GPU / NPU 计划选择与实测校准 |
| [DEVICE-ENV-MATRIX.md](DEVICE-ENV-MATRIX.md) | 多环境 / 多容器验证矩阵：同一份代码在原生宿主、WSL 与两个容器的读数对照 |

## security/ — 威胁模型、红队与防御

| 文档 | 一句话 |
|---|---|
| [SECURITY-NET.md](security/SECURITY-NET.md) | 网络安全面 M0：pcap → 双向流 → 三值弃权判决（含授权边界） |
| [SECURITY-RE.md](security/SECURITY-RE.md) | 网安 / 逆向技术调研：PYAS、KSword 与静态二进制分析对照 |
| [WSP-REDTEAM-STACK.md](security/WSP-REDTEAM-STACK.md) | 容器栈红队：结构化攻击读数（每条发现对应一条常跑用例） |
| [WSP-REDTEAM-MODEL.md](security/WSP-REDTEAM-MODEL.md) | 模型侧红队：三值工件的比特翻转攻击读数 |
| [WSP-REDTEAM-PROBE.md](security/WSP-REDTEAM-PROBE.md) | 红队第三弹：实测定位攻击与攻击者经济学 |
| [WSP-SEC-LOOP.md](security/WSP-SEC-LOOP.md) | 语义红队 + 完整性审计模型（负结果与方向修正） |
| [WSP-SHIELD.md](security/WSP-SHIELD.md) | 防御沙盘三件套：预测 / DDoS 盾 / 身份锁定（流量全合成） |
| [WSP-SPAM-QAT.md](security/WSP-SPAM-QAT.md) | 三值 QAT + 工件红队：SMS Spam 全链闭环 |
| [GUARD.md](security/GUARD.md) | 训练看门狗（parts-guard）：跑完知道、卡死不漏 GPU、崩了能续 |
| [HAZARD-POLICY.md](security/HAZARD-POLICY.md) | 并发 / 内存高危用法的放行判据（`scripts/hazard_gate.py` 依据） |

## engineering/ — 构建与运维

| 文档 | 一句话 |
|---|---|
| [BUILD-PERF.md](engineering/BUILD-PERF.md) | 构建性能：本机 / CI 实测、缓存边界、采纳与未采纳项 |
| [DOCTOR-V2.md](engineering/DOCTOR-V2.md) | doctor 开销治理：门缓存 1.26 s → 0.54 s（2.3×）、simchain 第七面接入（AI30–AI35）、ledger/registry 成本实测与不扩缓存的理由、Rust / 三进制 / 四进制评估与剩余待办 |
| [AGENT-COORDINATION.md](engineering/AGENT-COORDINATION.md) | 多智能体认领门与交接门：`--claim-gate` / `--handoff` 的三态语义、state_hash 口径、开销实测（53 / 271 ms）与未落地项 |
| [AGENT-CAPACITY.md](engineering/AGENT-CAPACITY.md) | 本机容量租约与跨项目协调：CPU/load/内存/GPU/磁盘/IO 探针、共享租约目录 + 跨进程锁 + TTL + 硬 deadline + heartbeat、`subagent_dispatch --capacity auto / enforce` 的 provider 前准入与 CAPACITY CONTRACT、`sandbox_run`/`run_caps` 共用租约；20/20 自测与诚实边界 |
| [AGENT-MODELS.md](engineering/AGENT-MODELS.md) | 子智能体按角色选模型：`route/probe/call`、配置 schema、密钥只走环境变量、16 条路由自测、25 条派发与输出隔离自测、未落地项 |
| [AGENT-MODEL-MANIFEST.md](engineering/AGENT-MODEL-MANIFEST.md) | 前端/后端模型与插件清单：人类面/智能体面分工、后端默认不接前端面板、多模态准入、论文式面板顺序与任务 brief；清单为模板，不是已接入声明 |
| [AGENT-SANDBOX.md](engineering/AGENT-SANDBOX.md) | workspace 沙盒：复制输入 + policy 前置 + run_caps + 宿主指纹；37 条自测、40/40 并发矩阵、hostwrite 100/100、WSL+bwrap 内核隔离 11/11 与诚实边界 |
| [AGENT-TEXT-GUARD.md](engineering/AGENT-TEXT-GUARD.md) | 外部文字污染门：不可信数据封套、隐形字符剥离、NFKD/多层编码扫描、派发输入 `prompt_guard` 与产物 `output_guard`、59/59 自测、81 例红队、90 例变形矩阵与自适应攻击边界 |
| [AGENT-REDTEAM.md](engineering/AGENT-REDTEAM.md) | 入口红队：81 例文字语料（0 攻击漏检、0 overblock）、沙盒 junction/硬链接/忽略文件及本轮 reparse 路径洞、修复与残余边界 |
| [AGENT-COMPLETION-PROVENANCE.md](engineering/AGENT-COMPLETION-PROVENANCE.md) | 编排完成回执：`finish` 默认要求 executor `Receipt`，伪造 `Passed` 报 `unattested_evidence`；CLI/库层回归与诚实边界 |
| [AGENT-METRICS.md](engineering/AGENT-METRICS.md) | 回执观测面：full/compact/坏回执的 verdict、step、run、成本、预算、claims 与 per-agent/lang/profile 统计；`agent_metrics.py` 178/178 自测，6 条真实场景聚合 |
| [AGENT-BENCH.md](engineering/AGENT-BENCH.md) | 真实任务基准：28 个代码/策略夹具、gold+negative 双 patch、Python/Node/Go/Rust 真实测试执行、policy/budget/dry-run/compact 分支；SWE-bench/Terminal-Bench/τ-bench 等谱系与本地切片边界，331/331 自测 |
| [WORKTREES.md](engineering/WORKTREES.md) | worktree 负责人表（`git worktree list` 实时生成） |
| [AUTO-UPDATE.md](engineering/AUTO-UPDATE.md) | 自动更新与一键更新（`--check` / `--apply`） |
| [VCS-BRANCH-ISSUES.md](engineering/VCS-BRANCH-ISSUES.md) | trit-vcs 分支与更新问题清单（主库侧观察，只记录不修） |
| [ERRORS.md](ERRORS.md) | 错误台账：每条错误的现场、根因，以及固化成的检查编号（`tools/checks.sh` 判红） |

## plans/ — 计划草案

| 文档 | 一句话 |
|---|---|
| [GENERAL-PLAN.md](plans/GENERAL-PLAN.md) | 通用提取路线：任意物体 → 部件 → 多网格，全自建 |
| [VISUAL-PLAN.md](plans/VISUAL-PLAN.md) | 视觉扩展：光影 / 风格 / 视频提取 / 画质 / UI |
| [PRIVACY-PLAN.md](plans/PRIVACY-PLAN.md) | 隐私存储：威胁模型、容器与挂载器路线、硬边界 |
| [DATA-CLEANING-PLAN.md](plans/DATA-CLEANING-PLAN.md) | 数据清洗 × 三进制：九条方向与底层缺口 |
| [MULTI-CHIP.md](plans/MULTI-CHIP.md) | 多芯片后端（x86 / ARM / 移动 SoC）设计 |

## specs/ — 写作与治理规范

| 文档 | 一句话 |
|---|---|
| [DOCUMENTATION.md](specs/DOCUMENTATION.md) | 一篇文档必须有什么：定位段、状态词、读数与复算、未证栏、对比基线（弱基准会把倍数放大） |
| [TERMINOLOGY.md](specs/TERMINOLOGY.md) | 「三进制」的三层含义与唯一用词表 |
| [README-SPEC.md](specs/README-SPEC.md) | README 规范：首屏只放什么、性能数字怎么给 |
| [DOC-INDEX.md](specs/DOC-INDEX.md) | 新增 / 搬迁 / 改名文档的流程与判红条件 |
| [AGENT-TASK-STANDARD.md](AGENT-TASK-STANDARD.md) | 高智商智能体任务标准：自包含起点、机器可判验收、显式禁区、失败回退、S/A/B/C 分级 |

## AGENT-TASKS/ — 可派发的任务包

| 文档 | 一句话 |
|---|---|
| [S-YARA-FULL.md](AGENT-TASKS/S-YARA-FULL.md) | S 级任务：YARA 完整语义（官方引擎差分 + 三值性质测试 + 性能预算三条同时成立） |
| [HANDOFF-YARA-EXPR.md](HANDOFF-YARA-EXPR.md) | 交接包：YARA 完整表达式语言 + 模块对象模型（起点 `feat/pe-sections`，验收 = 官方 4.5.4 DIFF 0） |

## YARA-EXPR/ — YARA 完整语义规范与任务卡

| 文档 | 一句话 |
|---|---|
| [README.md](YARA-EXPR/README.md) | 实现包总览：规范 00–04 + 任务卡 05，写明「P0 基建补齐前不派发」 |
| [00-RULES.md](YARA-EXPR/00-RULES.md) | 红线与工作方式：与任务卡冲突时以本文为准并停下来报告 |
| [01-SEMANTICS.md](YARA-EXPR/01-SEMANTICS.md) | 语义规范：官方 YARA 4.5.4 的算子 / 类型 / 字符串语义（2732 条向量） |
| [02-FRONTEND.md](YARA-EXPR/02-FRONTEND.md) | 前端规范：词法、规则骨架、条件解析在 `yaraparse/y2/` 里写成什么 |
| [03-PE-MODEL.md](YARA-EXPR/03-PE-MODEL.md) | `pe` 对象模型：从字节到对象树，`sigscan/pe2/` 的接口与不变量 |
| [04-CODE-MAP.md](YARA-EXPR/04-CODE-MAP.md) | 代码地图：接缝补丁里的接口清单与待写部分 |
| [05-TASKS.md](YARA-EXPR/05-TASKS.md) | 任务卡 T00–T32，每张一 PR，验收命令见目录 README §4 |

## review/ — 评审记录（按日期，不改写）

| 文档 | 一句话 |
|---|---|
| [trit-audit-2026-10-25.md](review/trit-audit-2026-10-25.md) | 只读审计报告（基于 `582b6c6`） |
| [gpu-unlock-2026-10-26.md](review/gpu-unlock-2026-10-26.md) | gpu-unlock 兼容性评估（Issue #167） |
| [tensor-core-path-audit-2026-10-26.md](review/tensor-core-path-audit-2026-10-26.md) | 有没有张量核 GEMM 路径（存在性核查） |
| [mimosa-deep-scan-2026-10-28.md](review/mimosa-deep-scan-2026-10-28.md) | Mimosa 深度安全扫描复核 |
| [self-audit-2026-10-28.md](review/self-audit-2026-10-28.md) | 工具链自审：用自己的扫描器挖自己 |

## 其他

- `assets/`：README 图表（SVG，由 `scripts/gen_docs_charts.py` 生成）。
- `SECURITY-NET-BASELINE.json`：`SECURITY-NET.md` 的基线台账，不单独阅读。

## 维护口径

- **新增文档**：写文件 → 在本索引对应分组加一行 → 让至少一篇别的文档指向它。只做第一步等于没提交
  （[specs/DOC-INDEX.md](specs/DOC-INDEX.md)）。
- **搬迁 / 改名**：全仓引用一起改。链接可达性由 `python scripts/doc_pointers.py` 判红
  （它同时核对「行号指针」与「markdown 链接目标存在」）。
- **`CHANGELOG.md` 里的旧路径**：历史记录按当时路径叙述，不随搬迁改写；查当前路径以本索引为准。
- **`review/`**：评审记录按日期沉淀，允许引用已过期路径，不参与活跃索引的维护要求。
