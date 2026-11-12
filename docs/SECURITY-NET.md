# 网络安全面：三值弃权判决（M0）

本文只写**已经跑过的东西**和它的边界。证据等级：A = 源码 + 测试原文；B = 声明/文档；C = 论文结论。

## 授权边界

- 只对**自有实验室**：环回、私网、本机虚拟机。任何主动发包模块必须走显式 allowlist，默认拒绝。
- M0 **只读离线 pcap**：不发包、不建连接、不做指纹。
- 不做：DoS/压测式打击、大规模扫描、规避检测、向第三方投递载荷。

## 管线（A）

```
pcap::Reader → packet::decode → flow::FlowTable → features::ternarize → decision::decide
```

| 层 | 行为 | 明确拒绝 / 跳过 |
|---|---|---|
| `pcap` | 经典 pcap 四种 magic（LE/BE × 微秒/纳秒），linktype 1=Ethernet、101=RawIP | pcapng、其他 linktype、记录越界、`incl_len > 256 KiB` 全部报错 |
| `packet` | Ethernet（≤2 层 VLAN）、IPv4、IPv6（扩展头 0/43/60/51/44，链长 ≤8）、TCP/UDP | 非 IP、非首片分片、非 TCP/UDP 一律**计数上报**，不静默丢弃 |
| `flow` | 无向五元组键 + 首包定向；24 维特征；Welford 流式方差 | 离线不做超时切分（切分规则必须显式给，不猜） |
| `features` | 8 条规则给死区，落进死区 → `X` | 未覆盖维度固定 `False`（模型只看声明过的证据） |
| `decision` | `analyze` 枚举补全 → `malicious / benign / undetermined` | 预算不足报 `budget-exceeded`，**不降级成猜测** |

判决的三条出口：

- `definite-positive` → `malicious`；`definite-negative` → `benign`；
- 输出符号冲突（`ambiguous`）、输出神经元自己弃权（`output-zero`）、枚举超预算（`budget-exceeded`）→ 一律 `undetermined`。

## 读数（A）

- `trit-net` 测试 **20 条**（原 9 + netsec 11），全部通过；netsec 覆盖：LE/BE 同参同值、纳秒标度、pcapng、坏 magic、未知 linktype、截断记录、VLAN、非首片、双向流聚合、死区 → `X`、判决三条出口、CLI 端到端。
- `cargo clippy -p trit-net --all-targets -- -D warnings`、`cargo fmt --check`、`module_budget.py`（298 文件，超 800 行者 0）、`publish_graph.py` 全绿。

## 与论文的关系

**检测侧**：NetMamba、ET-BERT 系、IDS-INT 都在闭集上刷准确率，误报代价不在目标函数里。reject option 文献补的是这块（Franc & Prusa, JMLR 2023；综述 arXiv:2107.11277）。本实现的差别：弃权来自**枚举补全**而不是概率阈值——`X` 的含义是"证据不足"，且能给出让它变成 `+1`/`-1` 的见证，可解释、可复算。

**攻击侧**：自动化渗透（Skandylas 2025 的利用图、DRL 攻击自动化、LLM agent 那一挂）共同缺陷是**把"未验证"当"成功"**，攻击图的边没有证据等级。三值攻击图（T = 有证据、X = 待验证、F = 已排除）是 M2 的目标，语义与仓内 `trit-belnap` 对齐。

## M1 校准（真实数据）

### 语料

CSE-CIC-IDS2018 官方 S3 桶的两天 ML CSV（同一套 CICFlowMeter 特征；二进制不入库）：

| 天 | 行数 | 良性 / 攻击 | 攻击占比 | sha256（前 12） |
|---|---|---|---|---|
| `Thursday-01-03-2018`（Infiltration） | 331,125 | 238,037 / 93,063 | 28.2% | `b0534c5d7d8b` |
| `Friday-16-02-2018`（DoS/DDoS） | 1,028,220 | 565k / 463k | 45.0% | `da8a64840062` |

### 方法

- **等距抽样**到约 17 万行：逐日 CSV 按时间写，截前缀会让样本全落在当天早段。
- **训练/测试按行号哈希切 75/25**，测试集只用来出读数；也支持外部测试集（跨日）。
- **训练集多数类下采样到 1:1**：三值 STE 全批训练在不平衡数据上会塌成常数负例（良性占 71.8% 时 TPR 恒为 0，实测）。
- **选哪 8 维由数据定**：对训练集算 `log1p` 后的点二列相关，按 `|r|` 取前 8 维，方向由符号定。
- **阈值 = 训练集分位数** `[P(0.5-b), P(0.5+b)]`，`b` 是弃权带宽。每个 `b` 独立重训一个模型。

命令：

```
trit-net calibrate Friday-16-02-2018.csv --rows 200000 --epochs 300 --hidden 16 --bands 0,0.05,0.1,0.2,0.3
```

选中的 8 维（DDoS 日，`|r|` 由大到小）：

| 特征 | r |
|---|---|
| `fwd_pkt_len_mean` | −0.769 |
| `fwd_pkt_len_max` | −0.761 |
| `fwd_bytes` | −0.756 |
| `bwd_pkt_len_max` | −0.716 |
| `bwd_pkt_len_mean` | −0.714 |
| `bwd_bytes` | −0.702 |
| `fwd_pkt_len_std` | −0.674 |
| `iat_std_ms` | −0.638 |

### 读数（Friday-16-02-2018，同日切分，测试集约 5 万条）

| band | coverage | abstain | FPR | TPR | precision | 攻击弃权 | 良性弃权 |
|---|---|---|---|---|---|---|---|
| 0.00 | 1.0000 | 0.0000 | 0.1446 | 0.8774 | 0.8313 | 0.0000 | 0.0000 |
| 0.05 | 1.0000 | 0.0000 | 0.1446 | 0.8774 | 0.8313 | 0.0000 | 0.0000 |
| 0.10 | 0.9727 | 0.0273 | 0.1423 | 0.8889 | 0.8383 | 0.0155 | 0.0368 |
| 0.20 | 0.8579 | 0.1421 | **0.1413** | **0.9381** | 0.8607 | 0.0772 | **0.1949** |
| 0.30 | 0.7324 | 0.2676 | 0.1955 | 0.9378 | 0.8614 | 0.0775 | 0.4219 |

- 弃权在 0.10/0.20 两档**主要吸收良性**（良性弃权是攻击的 2.4–2.5 倍），方向对。
- 手工挑的 8 条规则（M1 首版）最好只到 TPR 0.916 / FPR 0.082，但那是靠额外 23% 弃权换来的；数据驱动选维后在 14% 弃权下拿到 TPR 0.938。
- 🔴 曲线仍不严格单调（每个 band 独立重训），`0.30` 档 FPR 反而回升。

### 读数（跨日：16-02 训练 → 01-03 测试）

| band | coverage | abstain | FPR | TPR | precision | accuracy |
|---|---|---|---|---|---|---|
| 0.00 | 0.6682 | 0.3318 | 0.5212 | 0.6748 | 0.3596 | 0.5381 |
| 0.10 | 0.9063 | 0.0937 | 0.6683 | 0.7627 | 0.3113 | 0.4540 |
| 0.20 | 0.5581 | 0.4419 | 0.4963 | 0.6848 | 0.3862 | 0.5604 |

⇒ **换一天就基本失效**：FPR 0.50–0.67，精确率 0.31–0.39。DDoS 日学到的阈值在 Infiltration 日把一半良性流打成攻击。这是 drift + 攻击类型不同的双重结果，照实记录。

### 读数（Thursday-01-03-2018，Infiltration，负结果）

- 线性基线（logistic，23 个原始特征，`log1p` + 标准化，400 轮全批）：**AUC = 0.4984**，准确率 0.7487（≈ 良性占比 0.718，即模型近乎常数）。
- 三值模型：影子（未量化）训练精度 0.46–0.56，等同随机。
- 单特征按中位数切分，攻击与良性的触发率最大差 **0.13**（`PSH Flag Cnt`、`Flow Pkts/s`）。

⇒ 这一天用 CICFlowMeter 流特征**基本没有信号**。这不是训练器独有的问题：线性基线上限同样只有 0.498。

## M1.6 训练器与阈值改进（A，真实读数）

「下一步」清单第 1、2 条的落地（2026-11-03）：

- **mini-batch 训练器**：`TrainOptions.batch_size`（`0/1` = 全批旧行为；`k>1` = 每
  轮先按 `seed ^ epoch` **确定性洗牌**（Fisher–Yates，同参重跑逐位一致），再每
  `k` 条累积一次梯度更新，`scale = lr / batch_len`）。CLI `--batch N`。
- **良性尾部阈值**：`Config.benign_tail: Option<f64>`（CLI `--benign-tail F`）。
  `Some(tail)` 时分位数锚在**良性子集**：`Bias::High` 取 `[良性 P(0.5-b), 良性
  P(1-tail)]`（Low 镜像）——越过良性尾部才算攻击，攻击占比高时不再被攻击
  分布把死区拖走。

### 读数（CSE-CIC-IDS2018 Friday-16-02，同日切分，200k 行，测试 ≈5 万）

| 配置 | band | abstain | FPR | TPR | precision |
|---|---|---|---|---|---|
| M1 基线（全批+对称带） | 0.00 | 0.000 | 0.1446 | 0.8774 | 0.8313 |
| M1 基线 | 0.20 | 0.142 | 0.1413 | 0.9381 | 0.8607 |
| **mini-batch(64)+对称带** | 0.00 | 0.000 | **0.1101** | 0.8650 | **0.8645** |
| **mini-batch(64)+对称带** | 0.30 | 0.259 | 0.1721 | **0.9987** | 0.8645 |
| 全批+良性尾部(0.05) | 0.00 | 0.237 | 0.1737 | 0.9373 | 0.8645 |
| **mini-batch(64)+良性尾部** | 0.10 | 0.226 | 0.1579 | **0.9982** | 0.8642 |
| **mini-batch(64)+良性尾部** | 0.30 | 0.228 | 0.1584 | **0.9988** | 0.8645 |

（基线与 M1 §读数逐位一致——复现成立。）

### 读数（跨日：16-02 训练 → 01-03 测试）

| 配置 | band | abstain | FPR | TPR | precision |
|---|---|---|---|---|---|
| M1 基线 | 0.00 | 0.332 | 0.5212 | 0.6748 | 0.3596 |
| mini-batch+良性尾部 | 0.00 | 0.504 | 0.3898 | 0.6336 | 0.4343 |
| mini-batch+良性尾部 | 0.10 | 0.690 | 0.9768 | 0.9885 | 0.3911 |

### 结论与诚实的边界（🔴）

- **mini-batch 是主收益**：同弃权下 TPR 从 0.877→0.865（band 0 持平略降），
  但高 band 段 TPR 0.9378→**0.9987**（弃权 0.26 附近）；band 0 的 FPR
  0.1446→**0.1101**、精确率 0.8313→0.8645。训练器收敛弱是 M1 §诚实的边界
  里点名的瓶颈，mini-batch 直接缓解。
- **良性尾部单独用是负结果**（在 DDoS 日）：TPR 小涨但 FPR 全面变差
  （0.14→0.17-0.31）、弃权大涨——良性分布在该日很窄，P95 锚得很低，把
  大量良性流放进阳性侧。**与 mini-batch 组合才成立**：TPR 0.998 档的
  FPR 0.156-0.158，代价是弃权 0.14-0.23。照实记录，不当万灵药。
- **跨日依旧失效**（drift + 攻击类型不同，M1 的负结果成立）：新配置跨日
  FPR 0.39-0.98，不比基线好。「下一步」第 3 条（漂移纳入常规门）仍然必要。
- 合成夹具测试（`tests/netsec.rs`）钉的是**机制**：良性尾部把被拖进死区的
  攻击簇从弃权里捞回（attack_abstain 0.335→0.0、覆盖率升、TPR 不降），
  不是精度声明。

### 漂移门（M1 下一步第 3 条的落地）

`netsec/drift.rs` + `trit-net drift <基线.json> <当前.json>`：把「换一天就基本
失效」从**照实记录**升级为**会红的门**。

- **对账**：基线校准读数（`calibrate --json`，随仓提交于
  `docs/SECURITY-NET-BASELINE.json`——Friday-16-02 200k 行、与 M1 §读数逐位
  一致的那份）与当前读数按 band 逐条比 ΔFPR / ΔTPR / Δ弃权；
  **FPR 升 > 0.1 / TPR 降 > 0.1 / 弃权升 > 0.2 任一即红**（阈值可调）。
- **fail-closed**：缺 band、多 band（配置漂移须显式对齐）、坏 JSON、
  非有限读数、负/NaN 阈值——一律红。对不上账的「绿」没有含义。
- **基线是账**：读数变了门就红，换基线必须显式重生成并在提交说明里给理由
  （实测：M1.6 变体对 M1 基线 = 红 rc=1，基线自比 = 绿）。
- **边界（🔴）**：本门只做读数对账，不归因漂移原因（数据分布漂移/攻击类型
  不同/代码回归）；归因是人的工作。CI 是否每次跑校准取决于数据集可得性
  （335 MB 不入库），操作流程：本地重跑校准 → `drift` 对账 → 红则停下归因。

## M2 三值攻击图（A）

攻击侧的落地：`netsec/attack.rs`。修的正是 §与论文的关系 里点名的缺陷——自动渗透文献把
「没验证」当「成功」，攻击图的边没有证据等级。

### 语义

- 节点与边都带证据等级：`T` 已证实 / `X` 待验证 / `F` 已排除；登记一律带**出处**
  （`source`，空出处拒绝登记）。
- **路径折叠 = K3 AND**（格序 min，`F` 支配；`Algebra::Kleene.and`，穷举测试与
  `F < X < T` 上的 min 对拍）：路径上每个节点与每条边都参与合取。
  **没有一条「已证实」结论能从未验证的环节得出。**注意不是 `strong_and`
  （⊗ 张量积会 let `X` 洗掉 `F`：`F ⊗ X = X`，已排除的跳数反而被「待验证」救活）。
- **跨路径合并 = K3 OR**（格序 max，`T` 支配）：一条已证实路线 ⇒ 可达已证实；
  所有路线已排除（或无路径）⇒ 已排除；否则待验证。
- **截断封顶**：路径枚举触达 `max_depth` / `max_paths` ⇒ 总判决**一律封顶 `X`**
  （`reason = "truncated"`）——枚举不全既不许说「已证实」，也不许说「已排除」。
- `X` 的判决带**见证**：逐路径列出每个待验证的环节（哪条边、哪个立场），
  验证清单是机器可读的（`open: [{node, edge_to}]`），不让人猜。

### 读数（A）

- `tests/attack.rs` **16 条**：全证实链、单 `X` 边封顶 + 见证、`X` 立场封顶、
  `F` 只毙自己的路线（另一条仍证实 ⇒ 总判决不降；两条都排除 ⇒ `all-ruled-out`）、
  无路径 = 图内已排除（`no-path`，边界见下）、自环/双环终止、深度与条数截断封顶、
  3×3 折叠/合并穷举对拍 K3 AND/OR、登记纪律（空出处拒绝 / 未知端点拒绝 /
  同 label 幂等复用 / 同端点重复登记 LWW 并报告旧值）、JSONL 台账往返、
  坏行显式报错（证据字符不认识 / 边端点未登记 / 空出处——**不静默丢行**）、
  菱形两路线各自折叠、`Reach`/`PathDetail`/`OpenStep` 走 JSON、CLI 端到端
  （`trit-net attack 台账.jsonl --src entry --dst core` 输出 verdict + 见证；
  未知节点如实失败退出）。

### 诚实的边界（🔴）

- 「无路径」的 `RuledOut` 是**图内已排除**：以登记的节点/边为准。图不完整时它只说明
  登记面没给出路，不是绝对不可能。
- 图是**登记制**：证据从哪来（人工复核、M3 的受控探测、`trit-re` 的 xref 台账）
  是登记时的 `source` 字段的事，本模块不发包、不探测、不生产证据。
- 同一对端点只保留一条边，重复登记**后写覆盖**（LWW）并报告旧证据；
  「同一对端点既登记过 T 又登记过 F」的冲突检测留给台账消费方（与 trit-immune
  `contradictions()` 同一思路），本模块不做历史。

## M3 受控探测（A）

证据生产端：`netsec/probe.rs`。M2 攻击图是登记制，证据从哪来是 `source` 字段的
事——M3 就是第一个机器化的证据来源：**allowlist 默认拒绝的 TCP 端口/服务探测，
只对自有实验室目标**。

### 授权边界的机械化（每条可测）

1. **只吃字面 IP**：allowlist 里写 DNS 名一律拒绝——不做解析，SSRF/DNS 重绑定
   从协议层排除（与 trit-immune 出网守卫同一硬约束）。
2. **实验室范围**：环回 / v4 私网 / v4 链路本地 / `::1` / ULA `fc00::/7` /
   v6 链路本地。刻意**不含** CGNAT 100.64/10、基准测试 198.18/15、组播、公网。
3. **默认拒绝**：只探测 allowlist 显式列出的 (ip, port)；主机在列、端口没列也拒。
4. **硬上限防扫全段**：目标数 ≤64、每目标端口 ≤32、总探测 ≤512，超顶拒绝运行
   （`ConfigTooHot`）而非缩放；**端口必须是显式列表，不支持端口段**——段就是
   扫描的形状。连接超时 ≤5s、探测间隔限速、全局 deadline。
   🔴→✅ **审计 U-3（2026-11-12）**：总探测 ≤512 这条原先是**死帽**——`HARD_MAX_TOTAL_PROBES`
   声明了却没人引用，64×32=2048 的聚合量纲照样放行（每目标/每端口帽执法，聚合帽没执法）。
   现在 `from_json` 去重后 `total += ports.len()` 累加、`total_probes_ok(total)` 真执法；
   `run` 探测后的 `sleep` 用 `sleep_budget(delay, elapsed, deadline)` 夹到剩余预算，不再把 deadline overshoot 一个 delay。
5. **串行单线程**：顺序连接，不做并发洪泛。

### 分类（结果只分类不升格）

| 连接结果 | 证据 | 含义 |
|---|---|---|
| 成功 | `T` | 端口开放，直接证据（附 banner 摘录与知名端口提示，只是线索） |
| 明确拒绝（RST） | `F` | 端口关闭 |
| 超时/不可达/其他 | `X` | 分不清 filtered 还是 down，**不许替网络下结论** |

平台现实（🔴）：Windows 的 `connect_timeout` 常把拒绝表面化成超时——那时落 `X`。
宁可少报信息，不多报结论：`F` 只在拿到明确 RST 时给。

### 读数（A）

- probe **22 条**（`tests/probe.rs` 15 + `tests/probe_limits.rs` 2 + `src/netsec/probe/budget.rs` 5 纯谓词单测，行棘轮拆分后纯搬家）：默认拒绝（空表/未列端口）、DNS 名拒绝、公网/CGNAT/
  基准段/组播拒绝、实验室范围穷举（v4 环回/三段私网/链路本地，v6 环回/ULA/链路
  本地 vs 全球单播）、硬上限（65 目标/33 端口/空端口列表）、配置超顶拒绝不缩放、
  分类纯函数（成功/拒绝/超时/其他 → T/F/X/X）、**环回真连接**（开端口 banner 捕获
  =`T`、关端口**永不绝不 `Confirmed`** + note 与证据一致、Linux 拒绝 vs Windows
  超时两种都诚实）、deadline 截断如实上报（planned/probed/truncated）、JSONL 往返；
  **U-3 新增（`tests/probe_limits.rs`）**：聚合上限执法（`allowlist_enforces_aggregate_probe_cap`：16×32=512 放行、
  513 ⇒ `ConfigTooHot`）、delay 夹到 deadline（`delay_sleep_is_clamped_to_deadline`：
  开端口不 accept、banner_timeout 300ms、delay 3000ms、deadline 200ms ⇒ 探测耗时 <1500ms 且 truncated）。
  **U-3 纯谓词单测（`src/netsec/probe/budget.rs` 5 条）**：`total_probes_ok` 与 `sleep_budget` 从 `probe.rs`
  拆成纯函数，边界直接喂值——`total_probes_ok_boundary`（0/512/513）、`sleep_budget` 四档
  （elapsed>deadline ⇒ 0、elapsed==deadline ⇒ 0、剩余<delay ⇒ 夹剩余、剩余充裕 ⇒ 睡满 delay），不碰网络与计时。
- 🔴→✅ **假信心变异（2026-11-12，CI 实测暴露）**：`saturating_sub`→裸减这条变异原先**只被计时依赖的集成测试**
  `delay_sleep_is_clamped_to_deadline` 杀——本机 Windows 慢（单次探测耗时 >deadline ⇒ `deadline - elapsed` 下溢 panic ⇒ 抓住），
  但 CI ubuntu 快（探测耗时 <deadline ⇒ 不下溢，sleep 补偿到正好 deadline ⇒ `probed==1` 仍成立 ⇒ **存活**）。
  只被「慢机器才触发的边界」杀掉的变异 = 假信心。修法：把谓词拆进 `budget.rs` 配确定性单测，
  `sleep_budget_clamps_to_zero_when_elapsed_exceeds_deadline` 直接把 elapsed=300ms>deadline=200ms 喂进去，
  裸减变异在**任何平台**都确定性 panic；3 条变异锚点随代码改指 `budget.rs`、filter 改成纯谓词单测。
  **CI 变异门跑在快平台正是暴露这类假信心的地方**——本地绿不等于门绿。
- 变异门 **31 条**（U-3 +6：`total_probes_ok` 边界 `<=`→`<`、聚合执法 `if !total_probes_ok`→恒放行、
  累加 `total += ports.len()`→`total = ports.len()`、死帽 `HARD_MAX_TOTAL_PROBES`→`usize::MAX`、
  sleep 夹紧 `delay.min(remaining)`→`delay`、`saturating_sub`→裸减）——全断言抓住，0 存活 0 假杀。
  其中边界/夹紧/裸减 3 条锚在 `budget.rs`、由确定性纯谓词单测杀（不靠计时，见上「假信心变异」）；
  聚合执法/累加/死帽 3 条锚在 `probe.rs`、由 `allowlist_enforces_aggregate_probe_cap` 杀。
- 测试全部只打 `127.0.0.1`；CLI 子命令无进程级 e2e（`std::process::Command`
  被 Mimosa 硬拦截，仓纪律：spawn 留给部署层），CLI 覆盖由库级测试承担。

### 登记桥（M3 → M2 的闭环）

`attach_probe(graph, &findings, station_label)`：把探测证据**半自动**登记进攻击图。

- 结构就三层：station 节点（`T`，probe 起点）→ 主机节点（label = 字面 IP，
  证据 = **主机应答**：任一探测拿到 `T` 或 `F` 即在网——refused 也是主机回的
  RST；全 `X` 才是 `X`）→ 服务节点（label = `ip:port`，证据 = finding 证据，
  banner 线索跟到 note）；边 station→host、host→service，出处 = `tcp-connect`。
- **分工**：桥只登记探测证据；攻击语义（哪个利用、哪条路径）由人随后用 LWW
  重新登记改写——同 label 节点/同端点边后写覆盖是攻击图既定纪律（本次顺带把
  `add_node` 也升级成 LWW，否则桥重跑时节点证据会和新探测结果打架）。
- **幂等**：同一份报告重复登记图不变；新报告（端口状态变了）重登记 → 节点与
  边刷新成最新证据。`reach` 从 station 直达任一服务，`X` 服务的见证自动点名
  待验证的边与节点。
- `trit-net bridge <攻击图.jsonl> <探测报告.jsonl> [--station name]`：CLI 入口，
  更新后的图 JSONL 走 stdout。

### 诚实的边界（🔴）

- TCP connect 探测只回答「端口开没开 + banner 是什么」：不做 SYN 隐扫、不做
  指纹库匹配、不做 UDP。banner 与知名端口提示是**线索不是判决**。
- `T`（端口开放）≠ 攻击图里的 `T`（攻击步骤已证实）：桥登记的 `T` 边只说
  「probe 站能到达这个服务」，不说「能利用」。把开放服务升级成攻击步骤仍需要
  人给出出处（这正是"半自动"的半）。

## 上限执法门（alloc_guard，审计后的预防层）

2026-11-12 中期审计（`trit-audit-report.md`）三条发现——U-1 sparse 解码无界分配、
U-2 LSP 帧/文档无界读与无顶分配、U-3 probe 聚合帽是死常量——**共同形状**是
「定义了上限却不执法 / 读与分配不设界」。修完三条还不够：同类代码以后还会写出来。
`scripts/alloc_guard.py` 把这个形状变成**会红的静态门**（与 `test_surface.py` 同口径的棘轮式体检）。

### 三类规则（判红分两档）

| 规则 | 形状 | 档位 | 对应审计 |
|---|---|---|---|
| 死帽 | 帽名常量（含 `MAX` 或以 `_LIMIT/_CAP/_BOUND` 结尾）全工作区只出现一次（=声明本身） | 拦 | U-3 `HARD_MAX_TOTAL_PROBES` |
| 无界读 | `read_line`/`read_until` 三行窗口内无 `take(n)` 限位 | 拦 | U-2 LSP 帧头 |
| 整体读无界 | `read_to_string`/`read_to_end` 无 `take(n)`（多用于可信文件） | 警 | — |
| 可能无界分配 | 裸长度 `vec![x; len]`/`with_capacity(len)`/`resize(len,..)` 且所在 fn 无 `try_reserve` | 警 | U-1 sparse 解码 |

- 拦=证据不成立，警=要人看一眼（噪音判成拦截，工具第二天就被绕过）。
- 豁免走 `scripts/alloc_guard_allow.json`，每条带 `file/kind/reason`——reviewed 例外，不是静默忽略。
- fail-closed：`crates/*/src` 扫不到 `.rs` ⇒ 判红（「查不到≠没问题」）；`blank_rust` 涂白注释/字符串/raw 串，扫描器不扫自己的文档。

### 它当场抓到的牵连面（「防多牵连」的实证）

门第一次扫全仓就点名两处无界读：
- `trit-cli/commands/repl.rs`：交互式 REPL 读本地 stdin（人类逐行输入限界，非远程帧攻击面）⇒ 带理由豁免。
- `trit-immune/daemon.rs` `read_frame`：对端可为子进程 stdio / **TCP 流** / 内存流，与 U-2 的 LSP 帧**同一类无界 `read_line` + `vec![0u8; len]`**——
  审计只点了 `trit-lsp`，这是它没点到的**同类牵连 bug**。同口径修复（头行 8KiB、帧 16MiB、`try_reserve_exact`），
  `daemon::tests` 9 条（+3 帧帽边界）、变异 3 条全抓住。**一条规则扫出一整类，而不是逐个等审计点名。**

### 读数（A）

- 自测 **16/16**（`python scripts/alloc_guard.py --self-test`）：每规则一个真会红的反证 + 绿例真跑 + 接线走 `main()`/`scan()`。
- 门**自身变异 8 条全抓住**（`mutation_check.py --pkg scripts/alloc_guard.py`，python runner=`--self-test`）：
  帽名丢 MAX / 死帽阈值放宽到 0 / take 守卫失效 / 无界读降警告 / 行注释不涂白 / 豁免不要理由 / try_reserve 识别失效 / 扫不到也判绿。
- 全仓实测：**拦 0、警 381**（内部有界分配等需人看一眼，不判红）。
- CI：`ci.yml` `alloc-guard` job（`alloc guard (limits enforcement)`）+ `required-checks.txt` 登记；
  `ci_integrity.py` 本地 21 项对得上、`ci_budget.py --offline` 绿。

### 诚实的边界（🔴）

- 这是**静态形状门**，不是运行时安全证明：警档（整体读 / 可能无界分配）要人复核输入是否可信、是否另有硬顶；
  它钉的是「上限有没有被执法」这个可复算形状，不替人判断业务语义。
- 死帽判定按**名字形状**（含 MAX / `_LIMIT` 等）：名字不像帽的真上限它看不见；命名纪律是它的前提。
- 豁免名单是 reviewed 例外，会随代码漂移过期——新增豁免必须带理由，且理由要能被复核（与 `flake_ledger` 隔离台账同纪律）。

## 诚实的边界（🔴）

- 只选 **8 维**（可调），不是 24 维全用；参与比较的仍是 CICFlowMeter 的流统计量。
- 三值训练器（`train.rs`，全批 STE）在十万级样本上收敛很弱：训练集影子精度 0.86–0.87，离上限很远。
- **同日 FPR 14% 偏高、跨日直接失效**，这份实现不能当"可用检测器"；它的价值是弃权语义、可复算的读数与负结果。
- 单种子；每档独立重训导致曲线不单调。
- ~~`DEFAULT_RULES` 的 8 条规则在 `flow` 子命令里仍是未校准占位；校准结果没有回写~~
  （✅ 2026-11-06 闭环：`calibrate --emit-rules <file>` 落盘校准规则、
  `flow --rules <file> [--band b]` 消费——`ternarize` 用已校准规则替代占位，
  summary 注明规则来源；`DEFAULT_RULES` 保留为无校准文件时的缺省兜底）。
  回写闭环了"规则进检测器"，但**不改善上面两条 ML 质量边界**——规则来自同一份
  校准，分位数质量受同样的同日/跨日分布漂移约束。
  **实测读数**（ids2018 Friday-16-02-2018 全量 1028220 行 / 170857 样本）：
  calibrate --emit-rules 4.4s 落盘（2 band × 8 条规则）；构造 pcap 过
  `flow --rules --band 0.3`——已校准规则成功驱动 `ternarize`
  （输出 `FFFXTXTFXTFFTFTFFFFFFFFF`），summary 注明规则来源；
  未校准/已校准的 `ternarize` 输出可对比（同流不同三值化 = 校准生效的实证）。

## 下一步

1. ~~**24 维全用 + 阈值按良性尾部定**~~（✅ 良性尾部已落地，见 M1.6——但单独用是负结果，只在 DDoS 日与 mini-batch 组合成立）。
2. ~~**训练器**~~（✅ mini-batch 已落地，见 M1.6——主收益项）。
3. ~~**漂移**~~（✅ 漂移门已落地，见 M1.6 §漂移门——基线入库 `docs/SECURITY-NET-BASELINE.json`，变坏即红）。
4. ~~**M2 三值攻击图**~~（✅ 已落地，见上文 M2 章节）。
5. ~~**M3 受控探测**~~（✅ 已落地，见上文 M3 章节，含 **M3→M2 登记桥**）。
5. ~~**M3 受控探测**~~（✅ 已落地，见上文 M3 章节）——后续：probe 报告 → 攻击图边的**半自动登记桥**（按确认服务生成候选边，人补出处与步骤语义）。

## 为什么在 trit-net 里（不放独立 crate）

`scripts/publish_graph.py` 禁止实验层之间互相依赖（实验层 → 实验层 = 红），而三值解析
`crate::analyze` 只在 `trit-net` 里有——攻击图与检测链路要复用同一套三值语义，拆成独立
crate 就会撞上那条依赖红线。所以 `netsec` 作为 `trit-net` 的一个模块存在，而不是另立门户。
（原先这段写在 `src/netsec/mod.rs` 的头注释里；模块索引文件的行数预算只留给"地图"，论证挪到这里。）
