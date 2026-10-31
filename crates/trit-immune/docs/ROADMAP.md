# 平台路线图：愿景拆解与定位

> 用户愿景原文的分解与状态账本。定位先说死：**trit-immune = 三进制 + Rust 的裁决内核**，
> 对语言工具的四个主任务：**裁决（verdict）、记账（journal）、传播（propagate）、同步（sync）**。
> 它不造语言、不替代 git/GitHub（并行）、不做 LLM 决策——本地调度、确定性优先。

## 一、状态总账

| # | 事项 | 状态 | 落点 |
|---|------|------|------|
| 1 | 构建产物清理 + 源码路径登记 + 语言贴标签 | ✅ | `harvest.rs`（白名单删除、根围栏、拒符号链接、追加式注册表） |
| 2 | 合并偏好：绿色合并开关 / 实验室 / 封存 / 测试基准 | ✅ | `policy.rs::MergePolicy + decide`（按通道显式开启，永不默认） |
| 3 | 分层 CI：这层过了才扫下一层，永不一次性放行 | ✅ | `policy.rs` 层裁决（-1 阻断 / 0 挂起去扫下一层）+ `next_layer` 调度 |
| 4 | 预测合并：合并后状态预测 | ✅ | 预测器 = 免疫网络本身（`merge_gate` + `effective_verdicts`），端到端测试 `tests/policy.rs` |
| 5 | 结构耦合检测（不靠 LLM，直接从图上读） | ✅（图级） | `dag::coupling`：扇入/扇出 + 入/出边最坏通过值 |
| 6 | 本地服务器 + 联网 | ✅ | `server.rs` + `net::check_url`（SSRF 守卫） |
| 7 | ADV 桥接：扫描器作为裁决源 | ✅（协议就绪） | `POST /v1/scan` + `docs/ADV-BRIDGE.md` 映射表 |
| 8 | 编译器管线（rust-analyzer/rustc 诊断 → 裁决源） | ✅ | `json.rs` 零依赖解析器 + `diag.rs`（rustc JSON 流）+ `lsp.rs`（publishDiagnostics，0 基行号转 1 基，severity 映射）+ `POST /v1/diag?node=N` |
| 9 | 公共流程图（完成/未完成仓库的全量图 + 自动合并流程图） | ✅（第一步） | `fleet.rs` 舰队：聚合收敛表 + 聚合流程图（每仓一节）+ 克隆副本配对同步；跨仓 Delta 边 🔜；自动合并流程图由 `policy::decide` 提供数据层 |
| 10 | 智能体通道（多通道并发写入） | ✅（协议层） | 每智能体一个副本 + 写者 id + `/v1/deltas`；通道发现/注册 🔜 |
| 11 | 本地调度器（扫问题、排任务，无 LLM） | ✅ | `scheduler.rs`：`drain_plan`（DAG→有限任务队列：Review/Fix/Refactor）+ worker 池（scoped threads，每任务恰好一次）+ `JournalHandler` 兜底替身；收敛到固定点测试钉死 |
| 12 | MD → 任务栏（文档直接变任务队列） | ✅ | `taskmd.rs`：checkbox 解析（嵌套/#标签/散文不误伤）+ `ingest`（Message 节点 **0 态入账，作者打勾≠+1**、幂等去重、外部 MD 自动隔离）+ `taskbar` 视图（未完成且未隔离）+ server `POST /v1/taskmd`/`GET /v1/taskbar`；调度器有界 drain 消费任务栏测试钉死 |
| 13 | GPU 加速（传播/点积/合并的本地 GPU 路径） | 🔗 ADV/engine 域 | 内核先保证确定性 CPU 路径；GPU 是引擎域的活 |
| 14 | 代码级耦合（imports/调用图） | 🔗 ADV/ide 域 | 图级耦合已在内核；AST 级走 ADV 的 ide_callgraph 桥 |
| 15 | 生态算子库 + 批处理自定义 | 🔗 **trit 域** | 算子库（三值点积/传播/打包等数值与逻辑算子）是 **bfxh/trit** 的需要；内核只消费算子，不自攒 |
| 16 | 传输加密（TLS） | 🔜 排队 | std 无 TLS；接 rustls 时接口不变（帧层已留认证） |
| 17 | **概念台账**（反"换词不换设计"污染：规范名+别名+设计属性同构检测，标签解释用） | ✅ | `vocab.rs`（内核概念已预置种子） |
| 18 | **仓库防杂乱模板**（排版规范 + 理念纪律 + 换词纪律 + 提交前清单） | ✅ | `docs/REPO-TEMPLATE.md` |
| 19 | **收敛度量**（距"全 +1 固定点"的距离 + 折叠率，只许单向改善） | ✅ | `dag::convergence` + `vocab::absorption`，目标论文见 `docs/GOAL.md` |
| 20 | **有界智能体劳动**（Hold 携带机器可执行 Action，循环可证明终止） | ✅ | `policy::Action` + 模拟测试 `agent_loop_terminates_within_bound` |
| 21 | **设计层检查**（耦合阈值 → 0 态候选，机械判据替人眼挡噪音） | ✅ | `dag::design_smells(max_fan_in)` |
| 22 | **跨仓 Delta 边**（舰队级污染传播：A 仓漏洞沿依赖边打进 B 仓；注册表持久化） | ✅ | `fleet::add_cross_link/contamination/effective_verdicts` + LSP `POST /v1/lsp?node=N` + **TLS 可选 feature**（rustls + 自签证书） |
| 23 | **设计决策生产线**（不审代码审决策：三分类路由 / 可逆性×影响范围分级 / owner+SLA+默认通过 / 案例法→成文法架构测试） | ✅ | `design.rs`：`classify/DesignQueue/Statutes::arch_test` + `POST /v1/intake`；论文映射见 `docs/DESIGN-PIPELINE.md` |
| 24 | **落地状态机**（变更包 + 依赖图精准推送 + 已发布→已通知→已评估→已适配→已验签→已归队 + 超时默认策略[小自动/大降级] + 自动验签 + 旧路径下线倒计时 + 事件账本重放） | ✅ | `adoption.rs`：`publish/confirm_impact/mark_adapted/verify/align/tick/board/replay` |
| 26 | **自动化 intake 管线**（外部代码 → 自动扫安全 + 死依赖检测 → 自动隔离入账 → 调度器 drain → 智能体处理） | ✅ | `intake.rs`：`find_dead_deps`（Cargo.toml vs use/:: 交叉比对）+ `intake_pr`（全自动管线：扫→死依赖→隔离→等智能体，扫描器永不 +1） |
| 27 | **性质测试**（例子测试说"这个输入对"，性质测试说"任何输入都对"：门禁单调 / 合并交换 / 传播单调 / 重放一致 / 收敛单调，五条不变量 × 确定性随机输入） | ✅ | `tests/properties.rs`（xorshift64 固定种子可复现；GOAL.md 升级①"形式化验证"的落地形态——K3 AND 单调性、HLC 全序归并、"日志即数据库"、(min,max) 半环的充分条件验证） |
| 28 | **供应链完整性**（GOAL.md 升级②：依赖投毒 / dependency confusion / 锁文件篡改的确定性防线——Cargo.lock `[[package]]` 解析 + TSV 基线比对：新依赖→`-1`、checksum 变化→`-1`（Dispositive）、移除→`+1`；Cargo.toml SHA-256 清单指纹） | ✅ | `supply.rs`（`parse_lock/audit/encode_baseline/parse_baseline/fingerprint`，零网络零第三方）+ `intake::intake_supply`（finding 以**锚定攻击分支**挂 parent，`-1` 收紧 merge_gate；兄弟分支不殃及语义不变） |
| 29 | **容错与恢复**（AI/人类误操作可返回：改动前快照 + 保留期周期清除 + 整树恢复——journal"追加式不可变"哲学落到文件系统层） | ✅ | `recovery.rs`：`BackupVault::capture`（先验后写 fail fast + 原子改名）/`restore`（恢复前自动 capture 现场，undo 链可续）/`gc`（份数+天数双上限，顺带清 `.tmp` 残留与损坏目录）/根围栏拒 `..` 逃逸/损坏快照如实上报（`Inventory::corrupt`）不静默 |
| 30 | **数据转移桥**（从 Git/GitHub 等方便转移：转入走防污染协议，转出 git-friendly） | ✅ | `import.rs`：`parse_git_log`（`\x1e/\x1f` 分隔纯函数）+ `collect_git_log`（git CLI 便捷封装，固定参数无注入面）+ `import_commits`（每条 commit → Message **0 态**入账——历史是事实不是裁决，作者声称≠已验证；外部仓库整批自动隔离）+ `export_dag_tsv`（全量导出，恒 6 列可 diff） |
| 31 | **默认项目模板**（高维护性脚手架：无上帝对象从第一天开始，且机器可查） | ✅ | `template.rs`：`scaffold`（模块化骨架：lib.rs 只装配、小模块单一职责、测试/文档齐备）+ `audit_discipline`（反上帝对象机械判据：单文件行数/公开项上限 → 0 态候选）；**模板生成物必须通过自身纪律检查**（测试钉死） |
| 32 | **LSP 实时流守护进程**（rust-analyzer 实时桥的协议+会话层：内核零进程创建，spawn 留给部署层） | ✅ | `daemon.rs`：`encode_frame/read_frame`（LSP over stdio 帧协议，纯函数）+ `LspSession`（**泛型 IO**——子进程 stdio/TCP/测试内存流同一代码路径；initialize 握手 + didOpen/didSave + `pump` 泵读 + 请求应答途中通知缓存不丢消息）+ 全链路测试：握手→didOpen→pump→`LspBridge`→journal 落账（E→-1）；部署层集成方式（`Command::new` + stdio）在模块文档给出参考实现 |
| 33 | **跨仓 Delta 完整协议**（跨仓影响从"查询时视图"升级为"入账事实"：目标仓自己的 journal 里出现 `Delta::Record`，独立打开可见影响历史、门禁可折叠、重放一致） | ✅ | `fleet::sync_cross_repo`（跨仓 Delta 反熵：扫描全部边，落账值 = `max(源 verdict, 边 pass)` 与 (min,max) 查询语义严格一致；**变坏才落账**天然幂等；**`+1` 永不跨仓落账**——下游的 +1 必须自己挣，与"扫描器永不 +1"同一条纪律；未知仓/越界节点如实跳过）+ `repo_verdict`/`record_repo`（仓内读写原语）+ `Trit` 派生 `Ord`（坏→好单调序） |
| 34 | **FleetServer**（fleet 层的 HTTP 面，与单仓 `Server` 平行：聚合视图读端点 + 跨仓协议写端点，全部 TSV 文本与单仓同风格） | ✅ | `server.rs`：`FleetServer` + `route_fleet` 纯函数路由（不绑 TCP 直接测）——`GET health/repos/board/render/links/verdicts?repo=/verdict?repo=&node=`、`POST links`（声明边，逐行校验拒绝）/`record`（仓内写原语）/`sync`（触发反熵返回落账清单）；405/404 区分（已知端点方法错 vs 未知路径）；TCP 冒烟测试钉死 |
| 35 | **研究轮落地①：矛盾检测器 + 打穿来源回溯**（Belnap FOUR 的 ⊥ 态视图 + Sheyner 攻击图极简形态；六方向研究纪要见 `docs/RESEARCH.md`） | ✅ | `dag::contradictions()`（历史既有 `-1` 又有 `+1` 且未隔离 → 0 态候选；K3 折叠把矛盾压成坏消息支配，但矛盾本身是信息；⊥ 不进存储、查询时从账本派生）+ `dag::breach_sources(target)` + `BreachKind`（Ancestor/AnchoredAttack/CrossLink 三种打穿路径，与 merge_gate 折叠来源严格对齐——"门禁为什么坏"与"红队渗透路径"是同一份账的两种读法） |
| 36 | **研究轮落地②：OpenVEX 导出**（三值裁决 → CISA 四状态互操作标准，系统成为 SBOM 生态的 VEX 提供方） | ✅ | `vex.rs`：`export_vex(dag, author, product)` 输出 OpenVEX v0.2.0 JSON（`-1→affected`、`0→under_investigation`、`+1→not_affected`、`+1 且历史有 -1→fixed`——**fixed 账本派生**：无 `曾 -1` 证据不得声明，与"作者打勾≠+1"同一条纪律）；手写序列化零依赖，转义复用 `daemon::json_escape`，导出经 `json::parse` 往回验证合法 |
| 37 | **多跳攻击链枚举**（`dag::breach_sources` 的跨仓完全体：fleet 级渗透路径报告——从任一确认 `-1` 源到 target 的完整污染路径，跨仓边 + 仓内因果链 + 锚定攻击分支三形态） | ✅ | `fleet::attack_chains(target_repo, target, max_chains, max_depth)` + `AttackChain/ChainStep/ChainVia`：**源判定 = 节点自身 `-1`**（#33 入账事实的价值：跨仓污染已落账进节点自身）；**只有确认穿透边传打穿**（(min,max) 语义：可疑边把 -1 拉成 0）；可疑 0 继续追根不 emit；自身 +1 挡路即停；深度/条数/环三重截断如实报告；`steps[i].via` = 攻击方向进入该节点的边（对位：edges[j] 连接 n_{j-1} 与 n_j） |
| 38 | **FleetServer 攻击链端点**（`GET /v1/fleet/attacks?repo=&node=[&max_chains=&max_depth=]`，多跳渗透路径经 HTTP 暴露） | ✅ | `route_fleet` + 端到端测试（穿透边 → chains=1、via 列 source/crosslink、缺参 400、自定义上限）；KNOWN 端点表更新（405/404 区分保持） |
| 39 | **HMAC 签名 attestation**（in-toto Statement v1 形态：subject digest 绑定账本内容 + predicate 照实转写裁决集） | ✅ | `attest.rs`：`attest(dag, author, product, key, key_id)` + `verify`（HMAC-SHA256 + 常量时间比较，复用 `auth.rs`）；digest = `export_dag_tsv` 的 SHA-256——声明与账本强绑定；篡改 payload/错钥/账本变更 → 拒绝，测试钉死 |
| 40 | **Kani 模型检查 harness**（GOAL.md 升级①完全体：符号执行穷举输入空间，超越随机采样） | ✅（待触发） | `kani_proofs.rs`（`#[cfg(kani)]` 门控，常规编译零影响 + Cargo.toml check-cfg 登记）：K3 AND 交换/结合/单调/幂等（3×3、3³ 全空间）+ 门禁单调（任意三裁决列表追加 -1 不升级）；`cargo install kani-verifier && cargo kani` 即跑（本机未装，触发条件就绪） |
| 41 | **上游 PR 正式化**（并入 bfxh/trit workspace：crates/trit-immune + compat 对接） | ✅ | **PR #197**（bfxh/trit）：`crates/trit-immune` 加入 workspace members + `compat.rs`（Trit ↔ Ternary 双向 From + **3×3 同构穷举**：K3 AND ≡ Kleene BitAnd，False/Vulnerable 同为支配元）；上游 workspace 内 `cargo test -p trit-immune` 217 全绿 |
| 42 | **`trit` CLI：journal 代替 git 的命令行**（git 的每个核心操作在账本上都有对应物，大多更强） | ✅ | `src/bin/trit.rs`：`init`（建账本）/`log`（HLC 全序历史）/`status`（收敛+门禁）/`branch`（spawn 分叉——DAG 天生多分支）/`commit -m -v -n`（记录裁决，默认 0 未测不可信）/`merge`（**δ-CRDT 无冲突合并**，与顺序无关——比 git merge 强的地方：没有冲突标记，只有收敛）/`checkout`（**时间旅行**：重放到任意节点，日志不可变历史永不丢）/`clone`（账本即工件）/`gate`（三值门禁+打穿来源——git 只知道冲突不知道风险）/`suspects`（0 态待办）；全工作流端到端测试（含克隆侧增量经 δ-CRDT 合并回主账本） |

### 标签语义澄清（2026-10-31）

**标签是解释用的**：概念台账（`vocab.rs`）里每个概念的标签 = 规范名 + 别名 + 定义 +
设计属性，用途是**解释与同构检测**（换词不换设计 → 拒收并指认出处），不是语言分类。
`harvest` 的语言普查是另一回事——那是吸收项目时的**运维需要**（知道手里是什么语言
的技术栈），两者不要混。病灶是理念层的：杂乱 = 换词重复 + 定位外搭车 + 排版失序；
对 ADV 类问题项目的处置同样走台账：同构 → 挂别名收编或拒收，杂 → 按模板立规矩。

## 二、语言兼容立场

- **不造语言，做兼容**：校准协议（scan/Rule）对纯文本工作 → 任何语言可接；
- **贴标签**：`harvest` 语言普查给项目/语料打标签，一切跨语言数据带标签流动；
- **提取三进制相关逻辑**：凡是能表达为"裁决/传播/因果"的逻辑，提升为内核的
  Trit/CrossLink/Delta；表达不了的留在原域，贴标签挂桥；
- **三进制不是信仰**：门禁 = K3 AND、传播 = (min,max)、时序 = HLC——选它们是因为
  语义恰好咬合（证伪支配、未知冻结、确定性收敛），不是为新而新。

## 三、CI 分层哲学

代码打到一层 ≠ 真通过。层级：`静态扫描 → 沙盒/模糊测试 → 红队重放 → 传播影响面 → 基准`。
每层裁决独立入账（0 = 未跑，±1 = 结果），`decide` 严格按序消费：任何 -1 阻断、
任何 0 挂起去跑下一层、全 +1 且基准齐 + 开关开才自动合并。CI 永远在动：合并不是终点，
新 Delta 进入又从第一层开始。

## 四、防污染与防浪费

- 防污染：见 ADV-BRIDGE.md §四（外部内容 → Message + 隔离；账外无数据；
  导出剔除隔离子树）；
- 防浪费：PR 流程的 token 大头是智能体反复跑全流程——内核把**可判定的部分下沉为
  本地确定性计算**（merge/propagate/decide 都是纯函数），智能体只消费 `Decision` 与
  `suspects()` 队列——与 ADV"AI 只保留决策层"同一条哲学。

## 五、与 git / GitHub 的关系

并行，不替代：journal ↔ git 对象库（stamp 寻址 ↔ 内容寻址）、反熵 ↔ fetch/push、
`decide` ↔ 分支保护规则（但裁决来自因果传播，不是人勾的复选框）。本地优先：
一切流程本地可跑、可复算；GitHub 是远端镜像之一。
