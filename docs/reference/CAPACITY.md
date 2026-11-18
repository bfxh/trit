# 容量预测扫描器：防止 RAM / 显存 / CPU / GPU 爆满的设计（先文档后实现）

> **状态：设计文档（M0）**——本文定义「执行前容量预测」的方法论、资源账公式
> 与实施路线；实现按 §7 的路线另起 PR。规则：预测全部来自**闭式资源账**与
> **可复算的设备档案**，不做运行时猜测；本文引用的文献都是真实出处（§8），
> 文中的机器数字一律标注「本机实测」或「待实测」，外部数字不作为结论。

## 1. 问题定义

执行任何 trit 工作负载之前，回答一个问题：

> **这个负载在这台设备上，会不会把 RAM / 显存 / CPU / GPU 打爆？**

回答必须是三档判决 + 可执行的建议：

| 判决 | 条件 | 动作 |
|---|---|---|
| `OK` | 各资源峰值 ≤ 预算的 80% | 放行 |
| `WARN` | 任一资源峰值 ≥ 预算 80% | 放行 + 打印逼近项与建议 |
| `REJECT` | 任一资源峰值 > 预算 | 拒绝 + 打印超预算的账与可行的降载参数 |

这本质上是**准入控制（admission control）**：与其运行到一半 OOM /
显卡驱动超时重置（TDR），不如在入口处拿负载描述符算一遍账。

**为什么是"预测"而不是"运行时监控"**：监控（procscan 已有的面）只能
告诉你「已经爆了」；预测要在分配第一字节之前就给出上界。两者的关系是
**校准回路**（§5）：预测给上界，实测修正常数。

## 2. 方法论：五个可用的理论工具

### 2.1 闭式资源账（静态最坏情形记账）

对每个操作写出 `资源需求 = f(负载参数)` 的封闭公式，入口处代入求值。
这与最坏情形执行时间（WCET）分析同族——WCET 社区的核心教训是：
**只有把程序结构限制成可分析的（无递归无环、循环有界），静态上界才 tight**
（Wilhelm et al. 2008，TECS 综述）。本仓的负载天然可分析：

- TTX3：瓦片数、每瓦片字节数都是尺寸的封闭函数（`level_storage_bytes`
  已存在）；
- TTX2 MOD：层形状决定权重字节数；
- 无递归、无不可预判的动态分配进入关键路径（瓦片缓冲、行缓冲都是
  常数大小）。

**这是预测器的地基：不是启发式，是复算。**

### 2.2 Roofline（算术强度 → 性能上界）

Williams, Waterman & Patterson（CACM 2009）：一个内核的可达性能

```text
P_ achievable = min( 峰值算力, 算术强度 × 峰值带宽 )
```

对本仓的意义：三值 GEMV / 纹理解码都是**低算术强度的访存型负载**
（每字节搬运摊不到几次乘加）——所以 CPU/GPU **时间**的下界几乎完全由
**字节搬运量 ÷ 有效带宽** 决定，算力墙不是瓶颈。这把"会不会爆 CPU"
的问题简化成了"搬运多少字节 × 有效带宽是多少"。

- 有效带宽必须实测校准（STREAM 型基准，McCalpin 1995），不同机器
  差 2–5 倍；`DeviceProfile` 里存**本机实测值**，不抄纸面数字。

### 2.3 外存模型（Out-of-core / External Memory）

Vitter（ACM Computing Surveys 2001）的外存算法模型回答"数据比内存大
怎么办"：把 I/O 次数（以块为单位）当作复杂度度量。TTX3 的瓦片化就是
这个模型的应用——`verify` / `decode_tile` / GPU 子矩形上传的 I/O 账
都是 `⌈字节数 / 瓦片字节⌉` 次块 I/O，内存占用 O(瓦片) 而非 O(整图)。

预测器据此断言：**凡实现了流式接口的负载，RAM 峰值 = 常数 × 并发度**，
与数据总量无关；凡没有流式接口的负载（整级物化），RAM 峰值 = 数据总量。
两种情况都进账，不混谈。

### 2.4 网络演算（爆满的数学定义）

Le Boudec & Thiran（Springer LNCS 2001）的确定性队列理论给出积压上界：

```text
积压(t) ≤ 到达曲线 α(t) − 服务曲线 β(t)   （对所有 t 取包络）
```

迁移到资源预测：负载的**内存到达曲线**（分配随时间的最坏包络）对照
设备的**服务/容量曲线**，两者之差的包络就是峰值占用——"爆满"从此有
数学定义：到达包络超过容量。M0 用简化形式（峰值 = 分配和，时间轴不
展开）；M2 引入时间轴后可以预测"什么时候"逼近爆满（比如 GPU 每帧
上传曲线 vs 传输带宽）。

### 2.5 GPU 分析模型（occupancy 与带宽饱和）

Hong & Kim（ISCA 2009）的分析模型用 memory-level / thread-level
parallelism 估计内核周期数。本仓不做逐 SM 的精确建模（太贵、维护不动），
只取它的两个**保守推论**：

1. 显存占用的硬账：`VRAM = 权重字节数 × 轨制膨胀 × 批量缩放`——
   三值权重的双轨/位平面转换是**放大系数固定**的变换，账是封闭的；
2. 带宽饱和的判定：只要并发行数 ≥ SM 数的某个保守倍数，内存带宽即视为
   饱和（带宽墙生效），时间账走 §2.2。occupancy 的精确值给**保守带**，
   不给点估计。

## 3. 预测器架构（四件套）

```text
DeviceProfile（设备档案）      WorkloadDescriptor（负载描述符）
  ram_total / ram_avail          Ttx3 { level_dir 摘要 }
  vram_total / vram_avail        Ttx2Mod { 层形状列表, 批量 }
  gpu_name / gpu_vram            VerifyAll / FullDecode { … }
  bandwidth_ram (实测)      ──▶  ┌────────────────┐
  bandwidth_vram (实测)      ──▶ │ 容量预测器      │ ──▶ 判决 + 建议
  bandwidth_disk (实测)          │ （闭式账求值）  │     （每资源一条账）
  tdp / cores (实测)             └────────────────┘
```

- **DeviceProfile**：一份可提交的常量表（TOML/JSON）。全部数字标注来源
  （`measured` / `spec`），`measured` 的必须带复算命令（STREAM 变体、
  `procscan` 采样）。无档案时给保守默认档（8 GiB RAM / 4 GiB VRAM /
  20 GB/s），并在判决里声明"用的是默认档"。
- **WorkloadDescriptor**：执行前就能拿到的东西——TTX3 解析头部与级目录
  （20 B + 20 B/级，不用读数据区）；TritModel 的层形状在模型头里。**描述符
  本身永远远小于负载**，这正是瓦片化/分节容器送的礼物。
- **资源账**：§4 的公式库，纯函数、逐项可测。
- **判决器**：`峰值 > 预算 → REJECT`；输出**每资源一条明细账**（哪一项、
  多大、为什么、怎么降）。

## 4. 资源账草表（M1 要实现的公式，先在这里定稿）

### 4.1 TTX3（瓦片化，天然流式）

| 操作 | RAM 峰值 | VRAM | 时间下界（Roofline） |
|---|---|---|---|
| `parse` | O(1)（头 + 级目录） | — | 级目录字节 / disk_bw |
| `decode_tile` | 320 KB + 瓦片数据 24 KB | 320 KB（子矩形上传） | 24 KB / min(disk_bw, ram_bw) |
| `decode_level`（整级物化） | **级 RGBA 全量**（10240² 级 = 400 MiB） | — | 同上 × 瓦片数 |
| `verify` | O(1) 每瓦片 | — | 全档字节 / disk_bw + CRC 算力 |
| `to_bytes` | Σ 级存档 + 一份输出缓冲 | — | 同 verify |

判据示例：`decode_level(10240²)` 在 8 GiB 默认档 → WARN（400 MiB ≈ 5%），
但 `decode_level` × 16 级并发 → REJECT；`decode_tile` 任何机器都 OK。

### 4.2 TTX2 MOD → GPU 推理（当前最大爆显存风险点）

| 操作 | RAM | VRAM | 时间 |
|---|---|---|---|
| 加载模型 | Σ 层权重（三值打包后） | — | 文件 / disk_bw |
| `to_dual_rail` | ×2（正/负轨展开） | Σ 权重 × 轨制膨胀 × 元素字节数 | 权重字节 / ram_bw |
| 上传 | — | 同上 | VRAM 字节 / vram_bw |
| GEMV 推理（批量 B） | 输入 × B | 激活 × B | 2 × 权重字节 × B / vram_bw（内存型） |

判据示例：GGUF→MOD 导入器现有预算「文件 256 MiB、权重 5000 万个」就是
这张表的手工版——预测器把它变成机器可查的账。

### 4.3 扫描器自身（自审能力：预测器也算自己的账）

| 操作 | RAM | 时间 |
|---|---|---|
| `trit-unsafe scan` | O(1) 逐行 + 站点表 | 文件字节 / disk_bw |
| `hazard_gate` | O(源码) | 同上 |
| procscan 采集 | 事件流 + 目录快照 | 采样间隔 × 轮数 |

## 5. 校准回路（预测 ↔ 实测）

```text
预测上界 ──执行──▶ procscan 实测（RSS / VRAM / CPU ms）
   ▲                        │
   └── DeviceProfile 常数修正 ◀┘（带宽、膨胀系数）
```

- 实测 ≥ 预测 ⇒ 预测模型漏项，**修模型**（这是正确性信号）；
- 实测 ≪ 预测 ⇒ 常数过保守，**收紧**（这是精度信号）；
- 每次修正记录进 DeviceProfile 的来源注释（谁、何时、什么命令）。

procscan 现有的事件面（RSS / CPU ms / 模块）已经是校准的数据源，
缺的只是把「工作负载描述符」一起落盘——M3 接线。

## 6. 诚实边界（不猜清单）

- 静态上界模型，**不是运行时保证**：动态分配碎片、jemalloc/mimalloc 的
  行为、页错误都不在账内；
- GPU 占用（occupancy）给保守带；精确到 SM 的建模不做（维护不起）；
- OOM-killer / TDR 的行为是平台策略，预测只对「本进程可见的资源」负责；
- 显存带宽的"实测"依赖基准代码质量——STREAM 型基准必须随档案提交复算命令；
- 多进程共享 GPU 显存的竞争不在单负载预测内（那是系统容量问题，
  属于 procscan 的观察面）。

## 7. 实施路线

| 里程碑 | 内容 | 验收 |
|---|---|---|
| M1 | `capacity` 模块：DeviceProfile / WorkloadDescriptor / §4 公式库（纯函数 + 测试） | 每条账有单元测试；TTX3 账与 `level_storage_bytes` 交叉对拍 |
| M2 | 判决器 + CLI（`trit capacity --profile p.toml --ttx3 file.ttx3`）：三档判决 + 明细账 | REJECT/WARN/OK 三路有测试；输出与本文档公式一致 |
| M3 | 校准回路：procscan 落盘描述符 + 实测值；DeviceProfile 生成器 | 一次真实运行 → 档案常量更新 diff 可审 |
| M4 | GPU 面接线：`to_dual_rail` 显存账 + 上传时间账接入判决 | 与 trit-gpu 真机读数对拍（本机实测标注） |

> **M5（执行层 + 系统全局 + 默认开启）见 §10**——M1–M4 只到「判决」，不到「执行」。

## 8. 文献（均已核实出处；数字一律本机复算，不抄纸面）

1. Samuel Williams, Andrew Waterman, David Patterson.
   *Roofline: An Insightful Visual Performance Model for Multicore
   Architectures.* CACM 52(6), 2009. —— §2.2 算术强度与性能上界。
2. Sunpyo Hong, Hyesoon Kim. *An Analytical Model for a GPU Architecture
   with Memory-Level and Thread-Level Parallelism Awareness.* ISCA 2009.
   —— §2.5 的 MLP/TLP 判据来源。
3. Jeffrey Scott Vitter. *External Memory Algorithms and Data Structures:
   Dealing with Massive Data.* ACM Computing Surveys 33(2), 2001.
   —— §2.3 外存模型，瓦片化与 I/O 账的理论依据。
4. Jean-Yves Le Boudec, Patrick Thiran. *Network Calculus: A Theory of
   Deterministic Queuing Systems for the Internet.* Springer LNCS 2050, 2001.
   —— §2.4 到达/服务曲线与积压上界。
5. Reinhard Wilhelm et al. *The Worst-Case Execution Time Problem —
   Overview of Methods and Survey of Tools.* ACM TECS, 2008.
   —— §2.1 静态最坏情形分析的可分析性教训。
6. John D. McCalpin. *Memory Bandwidth and Machine Balance in Current
   High Performance Computers.* IEEE TCCA Newsletter, 1995（STREAM 基准）.
   —— 设备带宽常数的实测方法。
7. John Little. *A Proof for the Queuing Formula L = λW.* Operations
   Research 9(3), 1961. —— 并发工作集 = 速率 × 驻留时间的推论来源。
8. Khronos. *KTX2 Container Format* 规范；Eric Lasota（Binomial LLC）.
   *Basis Universal*。—— 纹理瓦片化/转码的工程先例（兼容性参照）。

## 9. 与现有工具的关系

| 工具 | 在预测器里的角色 |
|---|---|
| TTX3 `level_storage_bytes` | §4.1 资源账的现成原语（已测试） |
| `trit-procscan` | 校准回路的实测数据源（RSS/CPU/网络事件） |
| `hazard_gate` | 静态源码面（高危形态门禁）；预测器是容量面——互补不重叠 |
| `trit-redteam binary` | 产物面证据；DeviceProfile 的 hardening 字段可引用其结论 |

## 10. M5：从「判决」到「执行」，从「单负载」到「系统全局」

> **状态：设计（未实现）**。§7 的 M1–M4 只做到「算账 + 打印判决」。这里补三类能力：
> ① **系统全局**（跨进程/跨应用）② **执行层**（真的让软件不启动）③ **默认开启**。
> 规则不变：数字标注「本机实测 / 待实测」，不把外部纸面数字当结论。

### 10.1 为什么单靠 §7 不够

§6 已明确划界：「多进程共享 GPU 显存的竞争不在单负载预测内」。
但现实中爆显存的场景**恰恰是别人先占着**——浏览器、游戏、另一个推理进程。
单负载账只能回答「我这个负载够不够」，回答不了「现在轮不轮得到我」。

### 10.2 四层能力（自下而上）

| 层 | 回答的问题 | 数据来源 | 权限 |
|---|---|---|---|
| L1 观测 | 现在谁占了多少 | `trit-procscan`（已有）+ 平台计数器 | 用户态 |
| L2 单负载账 | 这个负载需要多少 | capacity M1–M4（§4 公式） | 用户态 |
| L3 判决 | 现在能不能跑 | L1 + L2 + headroom | 用户态 |
| L4 执行 | 不让它跑 / 降载跑 | 启动钩子（§10.4） | 见下 |

### 10.3 系统全局：哪些测得到，哪些测不到

| 平台 | 可靠来源 | 口径 |
|---|---|---|
| Windows | `IDXGIAdapter3::QueryVideoMemoryInfo`（`PROCESS` + `GLOBAL` 两个口径） | 全系统显存占用的**唯一**可靠来源；PDH `GPU Engine` 计数器可补利用率 |
| Windows | PDH 性能计数器（进程 Working Set / Private Bytes） | RAM 面 |
| Linux | `/proc/<pid>/fdinfo/<n>` 的 `drm-total-vram0` | 每进程显存 |
| Linux | `nvidia-smi --query-compute-apps`、cgroup v2 `memory.current` | 显存 / RAM |

**不可靠（必须写进判决）**：

- WDDM 允许显存**超订**并分页到系统内存，驱动回收前「可用显存」是**乐观值**；
- 所以判决不能用「刚好够」，必须用 **headroom 水位**（要求可用 ≥ k × 峰值，k ≥ 2 起步，按校准回路修正）；
- L3 的正确性上限 = L1 的采样新鲜度。采样间隔 >100 ms 时，两个进程可以**同时通过判决再一起爆掉**——
  这是**竞态**，不是精度问题。⇒ 多进程并发准入需要 L4 的协调点（单机一个闸门进程），否则只能做概率性建议。

### 10.4 执行层：四种形态（按侵入度递增）

| 形态 | 拦得住谁 | 权限 | 代价 | 结论 |
|---|---|---|---|---|
| **A 建议式** | 无人（只打印） | 无 | 0 | 当前形态 |
| **B 自壳**（`trit exec`） | 经过 trit 的负载 | 无 | 小 | **M5.2 做** |
| **C 系统服务 + 启动钩子** | 所有应用 | 管理员 / 系统服务 | 中：杀软误报、卸载残留、崩溃面扩大 | **opt-in 后做（M5.3）** |
| **D 驱动 / 内核** | 所有应用（可靠） | 内核签名 | 高：蓝屏风险、签名与维护成本 | **明确不做** |

「更深层次的权限」就是 C。但要说实话：Windows 上要可靠地拦**所有**进程启动，用户态只有 IFEO /
AppInit 这类被安全软件重点盯防的钩子，可靠方案在内核（minifilter / 进程回调）。**代价与收益不成比例**，
所以本设计把 C 定为「用户显式安装的服务，可完全卸载」，D 定为不做。

### 10.5 「默认开启」到底开什么

用户的诉求是「默认是打开的」。但 default-on 必须逐项拆开，否则一次误判会锁死整条工具链：

| 能力 | 默认 | 理由 |
|---|---|---|
| L1 观测 | **开** | 只读，无副作用 |
| L2 单负载账 | **开** | 纯函数计算 |
| L3 判决 WARN | **开** | 只打印 |
| L3 判决 REJECT（阻断） | **关**，需显式开启 | 误判会挡掉用户自己真想跑的东西 |
| L4-C 系统级拦截 | **关** | 需管理员 + 安全软件可见 |

**三个逃生口缺一不可**：`--force`（单次越过）、白名单（`capacity.allow`）、全局 kill switch
（配置 / 环境变量）。缺任何一个，误判就等于工具不可用。

### 10.6 与「前后端分层」的关系（超后期，先不拆）

按观测面有两种拆法，本设计**现在都不拆**：

- 前端 = 交互与展示（谁给判决、怎么给人看）；后端 = 采样与算账（谁占资源）。
- 三者目前同进程、同一套 Rust 代码。**拆分的唯一触发条件是「需要长期驻留的服务」**，
  而驻留服务正是 L4-C 的形态 ⇒ 等 L4-C 立项再拆，现在拆只会凭空多一层 IPC。

### 10.7 M5 里程碑

| 里程碑 | 内容 | 验收 |
|---|---|---|
| M5.0 | 本文（设计） | 四层 + 四形态 + 默认表定稿 |
| M5.1 | L1 系统全局采样（Rust：Windows DXGI / Linux fdinfo）+ headroom 判决并入 `capacity` | 与任务管理器 / `nvidia-smi` **同刻**读数差 ≤5% |
| M5.2 | B 形态 `trit exec`：先算账再 spawn | REJECT 时子进程**确实未启动**（有反例测试）；`--force` 可越过 |
| M5.3 | C 形态（opt-in）：系统服务 + 启动钩子 | 仅在用户显式安装后生效；卸载后无残留、无残留服务项 |

**不做的**：D 形态（内核）；运行时抢占（把别人的负载踢掉）；按进程显存硬配额（WDDM 不支持）。

### 10.8 与 §6「诚实边界」的增补

- 「全局可用显存」是**驱动口径**，不是物理口径；超订场景下它是乐观值。
- 判决的**竞态**（两个负载同时通过）需要单一闸门进程才能消除，M5.2 的 `trit exec` 是第一个候选闸门。
- 拦截能力（L4-C）**不改变**预测精度：它只是把「预测错了」的后果从「爆显存」换成「误拦」。
