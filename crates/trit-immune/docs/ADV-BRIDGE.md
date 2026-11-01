# ADV 桥：定位、接管映射与数据污染流程

> 对象：bfxh/ADV（unified-rx）——本地工具平台：MCP + 团队网页面，89 工具 / 15 域，
> 纯 stdlib 纪律，"AI 只保留决策层，确定性体力活下沉为本地工具"。

## 一、定位（先说清楚）

- **trit-immune = 三进制裁决内核（秩序层）**：`{-1,0,+1}` 代数、因果 DAG、门禁、传播、
  HLC 时序、多副本同步。它不做扫描、不做 IDE、不做 LSP。
- **ADV = 工具平台（执行层）**：扫描器、IDE 工具、hub 流水线、门禁脚本。它不裁决、
  不记账、不同步。
- 关系不是"替代"，是**裁决权上收**：ADV 的工具面继续干活，但一切**发现**（findings）
  统一交给内核校准、入账、传播、合并——ADV 的裁决是二值文本（definite/clue），
  没有代数、没有因果时序、没有多副本同步，这三样正是内核存在的理由。

## 二、接管映射（ADV → trit-immune）

| ADV 现状 | 内核接管 | 说明 |
|---|---|---|
| `bug_scan` 的 definite / clue 两级 | `scan::Rule(Dispositive / Heuristic)` | definite → `-1`（规则即漏洞本体），clue → `0`（冻结进红队队列）——与 ADV H3 实测 precision≈1.0 的扫描器直接对位 |
| `secrets_hunt` 凭据泄漏扫描 | Dispositive 规则 | 凭据泄漏即漏洞本体，无需利用即证伪 |
| `rust_taint_scan` / data-flow gate | `CrossLink` 污染边 + `propagate` | taint 的 definite/naive 分级 → 边通过值 `-1/0`；`(min,max)` 半环给出可复算的影响面，替代文本级污点台账 |
| `[untrusted-content]` 前缀（B3 立场） | `Message` 节点 + `quarantine` | 不信任标记从"文本前缀"升级为"因果链隔离 + 0 态沿链传播" |
| `hallucination_guard`（A 臂一致率 1.0） | 判据保留，事件入 DAG | 守卫的分歧事件落 Message 节点，成为可审计的因果事实 |
| `lesson` 教训记忆（H4：0/8→3/8） | lesson = Message 节点 | 教训可引用、可隔离、可随副本同步——ADV 的记忆是关键词检索，无因果、无隔离传播 |
| hub 账本（哈希链，如实标注 `unsigned`） | journal + HMAC-SHA256 帧 | 内核既验"未被改"也验"来源"（PSK）；ADV 对"不能验谁签的"的诚实态度值得保留并在此基础上前进 |
| `breaker` 工具熔断 | 吸收（server 限流待做） | 同工具+参数窗口超阈即断——server 层值得抄 |
| 性能门统计判定（5 轮 + Bootstrap 95% CI 三态） | 吸收（给 merge 做性能门时用） | "单点比值必然假红"的实测结论直接复用 |

## 三、ADV 强于内核、需要吸收的

工具面规模与评测纪律（双臂 A/B、幻觉守卫判据、本地门 37 步、PR 纪律）、LSP 集成、
跨工具词汇表。这些**留在 ADV**，通过桥接协议喂给内核；内核不重新发明工具。

## 四、数据污染流程（硬性，"炼化自己的项目"与未来网站同此纪律）

1. **外部内容一律落 `Message` 节点 + 自动 `quarantine`**（0 态沿链传播），永不直接 ±1——
   网页、抓取日志、他人输出、用户输入，同此一条；
2. 扫描发现走 `scan` 校准协议——**扫描器永不产生 `+1`**（清白不等于安全）；
3. 进因果链的每一字节都在 journal（追加式 + 帧可选 HMAC）——**没有账外数据**，
   提炼（炼化）过程中产生的任何中间结论若要生效，必须作为 Delta 入账；
4. 跨副本只收签名帧（PSK-HMAC）；重放旧帧无害（stamp 去重 + 全序 LWW）；
5. 给 AI 的上下文导出：`render`/日志附节点 kind 与隔离标记，**隔离子树默认剔除**、
   等人工放行——防止被污染的历史反向污染新会话、新语料、新训练数据；
6. 未来网站同构：用户/爬取输入 = Message 节点；渲染输出 = 只读面（render/gate/suspects）；
   写面全部过门禁；出网一律过 `net::check_url`（http/https only、拒 localhost/环回/
   私网/保留地址、解析后逐 IP 复核）。

## 五、高并发：AI 高频合并怎么办

- **单 journal 单写者**（追加式）——高频 AI 不该抢一把锁，而是各开副本（各自写者 id）、
  经 `/v1/deltas` 或反熵合并；写者数量横向扩展，不靠更大的锁；
- **合并交换律已被测试钉死**（与输入顺序无关、幂等、裁决恰好归位不串号）——高频合并
  不需要全局锁序，冲突 = LWW 后到者胜且两条历史都保留；
- server 读多写少：`RwLock`，读并行/写串行；6 写者 × 50 Delta 多序洗牌收敛压力测试见
  `tests/sync.rs::high_frequency_merge_six_replicas_is_order_independent`；
- 并发读写验证见 `tests/server.rs::server_handles_concurrent_readers_and_writers`。

## 六、相关技术（论文与系统）

- CRDT：Shapiro, Preguiça, Baquéro, Zawirski, *Conflict-free Replicated Data Types* (SSS 2011)
  ——内核 ≈ 状态合并 + 每节点 LWW 寄存器 + 追加日志；
- HLC：Kulkarni, Demirbas, Madeppa, Avva, Leone, *Logical Physical Clocks* (SNAML 2014)；
- delta-state CRDT 与 anti-entropy（pull/push/push-pull）——`anti_entropy` 的学术出处；
- Kleppmann, *Designing Data-Intensive Applications*（第 5 章复制、第 9 章一致性与共识）；
- git 对象模型——内容寻址与本 journal 的 stamp 寻址同构；"日志即数据库"同构于事件溯源（Event Sourcing）。
