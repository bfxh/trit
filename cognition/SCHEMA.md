# cognition/ 语料契约（SCHEMA）

训练层与 owner 仓之间唯一的耦合是**数据形状**：四份语料 JSONL 契约的字段、类型、含义与隐私
级别（外加第 6 节的声明集契约——它是审计闸的输入形状，不是语料）。本文以写入方工具的真实存储实现为准（`tools/signal.py`、`tools/watch.py`、
`tools/pitfalls.py`），collect.py 按这份契约读取，不 import 任何一方。

## 总则

- 一行一个 JSON 对象，UTF-8；空行跳过；**坏行 fail-closed**——拒绝整个读取并报行号，
  不跳过、不半读（半读的语料教一半的真相；signal.py/pitfalls.py 同款纪律）。
- `ts` 一律是写入方**本机时区**的 ISO 8601（秒精度，带偏移量），不是 UTC。因此跨文件
  按时间排序要先解析偏移，字典序只在同偏移内可靠——collect.py 不做跨源时间排序。
- 隐私级别定义：
  - **public-safe** = 允许出现在随仓提交的文件里（公开树上可见）；
  - **private** = 只允许留在本机（gitignore 的 store 或 gitignore 的 corpus 目录），
    克隆不会带走。
- collect.py 只读 store，永不写回 owner root。

## 1. signals.jsonl（分诊 + 惩罚）

位置：`<owner-root>/.agent/signals.jsonl`（signal.py 追加写，ensure_ascii，本仓已
gitignore——**整份 store 按私有对待**）。一行一个事件，两种行型，由 `kind` 区分：

### classify 行（kind != "penalty"）

| 字段 | 类型 | 含义 | 隐私级别 |
|---|---|---|---|
| `ts` | str | 分诊发生时间（ISO 8601 本地偏移） | public-safe |
| `kind` | str | `"task"`（纯任务）/ `"emotion"`（纯情绪）/ `"mixed"`（并存） | public-safe |
| `emotion_score` | int 0..3 | 情绪规则权重累计。关键字规则表的命中计分——**是信号不是事实** | public-safe |
| `signals` | list[str] | 命中的规则名（情绪规则 + 任务规则），按命中位置排序 | public-safe |
| `task_hints` | list[str] | 从原文抽出的祈使动词 / 工具名 / 文件名形态片段（去重，≤8 个） | **private（原文片段）** |
| `text_len` | int | 原文字符数 | public-safe |
| `text` | str | 原文。超过 8192 字符的输入为 `""`——含义是"未存储"，永远不是"空输入" | **private** |

### penalty 行（kind == "penalty"）

| 字段 | 类型 | 含义 | 隐私级别 |
|---|---|---|---|
| `ts` | str | 惩罚记录时间（ISO 8601 本地偏移） | public-safe |
| `kind` | str | `"penalty"` | public-safe |
| `score` | int 1..3 | 惩罚分（0 不收——不带信号的惩罚不是惩罚） | public-safe |
| `reason` | str | 惩罚原因，自由文本 | **private** |

注意同一份 store 里两种行型的键名不同：惩罚行写 `score`/`reason`，分诊行写
`emotion_score`/`text`。按行型分支读取，用错一方的键就是 KeyError。

## 2. watch.jsonl（事件 + agent_id + turn）

位置：`<owner-root>/.agent/watch.jsonl`（watch.py 追加写，ensure_ascii）。**设计上随仓
提交**，所以 record 在门口就拒收机器绝对路径（盘符 / 家目录形态）出现在 event、agent_id
与 note 里。每行**恒定六键**：

| 字段 | 类型 | 含义 | 隐私级别 |
|---|---|---|---|
| `ts` | str | 事件时间（ISO 8601 本地偏移） | public-safe |
| `event` | str | 事件标签。自由词，无白名单（harness 发明词汇的速度快过白名单老化） | public-safe |
| `agent_id` | str | 产生事件的（子）代理标识；可为 `""` | public-safe |
| `turn` | int ≥ 0 或 null | 会话内轮位（疲劳分析的轴）；无轮位事件为 null | public-safe |
| `verdict` | str | `"ok"` / `"fail"` / `""`（记号事件没有判定） | public-safe |
| `note` | str | 自由文本备注；本机路径形态在 record 门口已被拒 | public-safe |

## 3. pitfalls.jsonl（五域痛点）

位置：`<owner-root>/.agent/pitfalls.jsonl`（pitfalls.py 写，**故意随仓提交**——痛点语料
就是要教下一个模型的）。一行一条：

| 字段 | 类型 | 含义 | 隐私级别 |
|---|---|---|---|
| `id` | str | add 生成：`"p"` + sha256(cat+trigger) 前 12 位；或种子 id（`pf-*`，**载重不可改名**——改名会重导入、分裂语料） | public-safe |
| `cat` | str | 五域之一：`git` / `github` / `infra` / `api` / `semantics` | public-safe |
| `trigger` | str | 什么情况下会踩 | public-safe |
| `consequence` | str | 踩了会怎样（没有后果的痛点教不了东西） | public-safe |
| `prevention` | str | 怎么避免 | public-safe |
| `source` | str | 出处（台账行号 / 协作规则）；可为 `""` | public-safe |
| `ts` | str | 最近一次写入时间（ISO 8601 本地偏移） | public-safe |

写入方在 add 门口拒收机器用户路径（家目录形态）；collect.py 在快照门口**再拦一道**——
store 是手改得了的，knowledge 要随仓提交。

## 4. corpus 导出格式（隐私闸之后的投影）

### export 投影（signal.py export 默认输出，六字段）

| 字段 | 类型 | 含义 | 隐私级别 |
|---|---|---|---|
| `ts` | str | 来自台账行 | public-safe |
| `kind` | str | `task` / `emotion` / `mixed` / `penalty` | public-safe |
| `emotion_score` | int | penalty 行 = score；其余 = emotion_score | public-safe |
| `penalty` | int | penalty 行 = score；其余 = 0 | public-safe |
| `text_len` | int | penalty 行 = len(reason)；其余 = text_len | public-safe |
| `signals` | list[str] | classify 行的命中规则；penalty 行为 `[]` | public-safe |

`--with-text` 才追加 `text` 字段（显式 opt-in；输出落在 owner root 内会被拒——防止带
原文的语料被误提交）。默认投影**没有** `text` / `task_hints`——抽出来的原文片段也是原文。

### collect.py 的 corpus/events.jsonl（合并行）

- signals 行 = 上述六字段（无 `text`，永不带原文）；
- watch 行 = 六字段 + 观测四键：`event`、`verdict`、`agent_id`、`turn`；`kind` 固定
  `"watch"`，`emotion_score` = 0，`penalty` = 0，`text_len` = len(note)，`signals` = `[]`；
- 行序：signals 段在前（账本序），watch 段在后（账本序）；**不跨源按时间排序**——两本
  账的 ts 都带本机时区偏移，跨偏移的字典序不是时间线；
- 文件 ensure_ascii 写出（纯 ASCII，任何宿主都读不坏）；
- **目录级私有**：内容是结构化投影（无原文），但交互语料整体按私有对待——整个
  `corpus/` 被 gitignore，克隆不带它；
- 快照语义：重跑覆盖；行数一致即幂等。

### collect.py 的 knowledge/pitfalls.jsonl（公开知识快照）

- 从 `<owner-root>/.agent/pitfalls.jsonl` 逐行快照，字段与契约 3 完全一致，原样保真
  （含非 ASCII 内容，UTF-8 不转义不丢字）；
- **public-safe，随 cognition/ 入库**；store 不存在时不写文件（没有 0 行快照冒充数据）；
- 快照门口两道拒绝（都带行号）：坏条目（缺 id/cat/trigger/consequence/prevention）与
  机器用户路径（扫**解析后的字符串值**，不扫原始行——JSON 会把反斜杠成双编码，扫原始
  行对盘符用户路径结构性失明，闸看起来在工作实则空转）；
- 幂等：store 不变 → 字节不变。

## 5. attest-ledger.jsonl（token 认证链）

位置：`<Store>/attest-ledger.jsonl`（attest.py 写，**仓库外**，与 cost-ledger.json /
journal store 同站点口径——整份 store 按私有对待，gitignore 的本机目录，克隆不带它）。
一行一个 JSON 对象（JSONL），ASCII 转义写出；**坏行 fail-closed**——append/verify/
balance 拒整份账并点名行号，不跳过、不半读。写入方：`attest.py append`（编排链的
`--spent-token` 落账）、`attest.py import`（收编 cost-ledger 旧账）。collect.py 不读
这份账（它是运行期预算账本，不是训练语料）；契约在此只记录**数据形状**。

链式语义（防篡改的全部机制所在）：

- `payload_hash = sha256(除 prev_hash 外全字段规范化 JSON)`（sort_keys + 紧凑分隔符）；
  `prev_hash = 上一条 payload_hash`；**创世行 prev_hash 是固定常量**（64 个 0）。
- 除 `prev_hash` 外任何一字被改 → payload 复算不符；删行/重排 → seq 与 prev_hash
  双断。verify 全链复算逐条验，**报第一条断点**。
- 防重复计数：去重键 `(agent_id, task_id, runid, kind)` 全链唯一，`payload_hash`
  全链唯一——同一笔 spend 重复记账在 append 门口被**结构性拒绝**（不是警告不是合并），
  这是 Claude /cost 恢复会话重复计数的直接解法。
- **诚实边界（写死在 usage，也写在这份契约里）：认证 = 防篡改的记账与去重，不是模型
  侧 ground truth。工具层永远测不了模型侧 token；`claimed_tokens` 仍是调用方申报。链
  保证的是"申报过的账改不了、丢不了、数不了两遍"，仅此三件。**

| 字段 | 类型 | 含义 | 隐私级别 |
|---|---|---|---|
| `seq` | int ≥ 1 | 链上序号，从 1 连续递增；断号即链断 | public-safe |
| `ts` | str | 记账时间（ISO 8601 本地偏移，微秒精度） | public-safe |
| `agent_id` | str | 申报消耗的（子）代理标识；可为 `""` | public-safe |
| `task_id` | str | 消耗归属作用域：agent_verify 的 task-id，无则 agent-id，再无则 `"anon"`（与 cost-ledger 的 `av|` 标签同源推导——import 收编时按同规则还原，两本账身份不同源就会双扣） | public-safe |
| `runid` | str | 调用方选的本次记账动作唯一 id；去重键成员，重放同一笔必须撞键被拒 | public-safe |
| `claimed_tokens` | int | 调用方**申报**的 token 数：spend/grant > 0，correction 非零可负（负数即下修）；不是测量值 | public-safe |
| `kind` | str | `spend`（消耗）/ `grant`（补给）/ `correction`（带符号修正） | public-safe |
| `note` | str | 自由文本备注；import 行写明来源账（含原账记账时间） | public-safe |
| `prev_hash` | str | 上一条 `payload_hash`；创世行是 64 个 0 的固定常量 | public-safe |
| `payload_hash` | str | `sha256(除 prev_hash 外全字段规范化 JSON)`，全链唯一 | public-safe |

预算闸口径：agent_verify 的 `--budget-token` 在链存在时消费**链余额**
（`spent_net = Σ spend + Σ correction`，grant 不进闸面——补给是记录不是扣除）；无链
维持声明制并在回执 `budget.source` 如实标注 `declared-only`；链 verify 不过 → 闸整体
拒绝（闸不信任自己的账就不放行）。

## 6. claims 文件（声明分解契约）

位置：由调用方传给 `tools/claim_audit.py -Claims <f>` 的 JSONL 声明集。审计产物可能随仓
提交，所以 load 门口就拒收机器绝对路径（盘符 / 家目录 / /mnt/ 形态）——扫**解析后的字符串
值**（含 decomposition 内部），不扫原始行（L110 同族纪律）。一行一条声明，两种行型：

### 原子行（kind ∈ test-pass / file-exists / number / command-ran）

| 字段 | 类型 | 含义 | 隐私级别 |
|---|---|---|---|
| `claim` | str | 声明正文；必须 root 相对，机器路径形态在门口被拒 | public-safe |
| `kind` | str | verifier 名字；未知 kind 一律 UNVERIFIED（不存在的 verifier 作不了证） | public-safe |
| `test` / `file` / `command` / `source.path` | str | 各 kind 的证据指针；缺省时从 claim 文本抽取 | public-safe |
| `value` | int/float | number 行的显式数值（缺省时抽 claim 文本里的数字） | public-safe |

### 聚合行（kind = "aggregate"）

聚合声明是合取语义（"全部都成立"）。判定只吃逐原子判词，不吃结论句本身：

| 字段 | 类型 | 含义 | 隐私级别 |
|---|---|---|---|
| `claim` | str | 聚合结论句。**零证据效力**——判定不经由它 | public-safe |
| `kind` | str | `"aggregate"` | public-safe |
| `decomposition` | list[atom] | 原子声明列表，每条 atom 自带 `claim`+`kind`（+该 kind 的证据字段）；**缺省或空列表 → 聚合 UNVERIFIED，永不 VERIFIED** | public-safe |

聚合判定规则（钉死，无裁量）：任一原子 FABRICATED → 聚合 FABRICATED；全部原子 VERIFIED →
聚合 VERIFIED；有任何 UNVERIFIED 原子 → 聚合 UNVERIFIED。atom 不许再是 aggregate（分解不
递归，嵌套层没有 verifier → UNVERIFIED）；`decomposition` 在场但形状坏（非列表 / atom 缺
claim+kind）→ 整份 fail-closed（exit 3）。出处：过程监督优于结果监督——OpenAI "Let's
Verify Step by Step"（2023）。编排链把分解规模透传进回执（`claims.aggregates`/`claims.atoms`，
appended 字段）。

### second-opinion（独立复核，opt-in）——同一声明两条独立证据路径

`claim_audit.py second-opinion` 不"帮看看"，而是重新推导再 diff：src1 = 最近回执单独作证
（mtime 最新者优先）；src2 = 盘上现实 + cost-ledger 的运行痕迹（entry key 内嵌
sha8(command)，与 agent_verify 写账同源推导）/ 回执 journal changes（added/modified/deleted）。
判定口径：两源都给出一致证据 → VERIFIED 并标注 `dual-source`；任一源与现实矛盾 →
FABRICATED（双证据并列打印）；任一源无证据 → UNVERIFIED（缺证据永不升级）。kind=number
没有第二条独立路径（被引工件是唯一出处），永不升级。被点名但解析不了的回执/台账 → 整份
exit 3 fail-closed。出处：独立复推——arXiv:2610.12234 式（第三方重新推导再 diff，不是
"再看一眼"——同一本账投两票是伪复核）。编排链 `--second-opinion`（opt-in）让 claims 步骤
自动跑两源，结果进回执 `claims.second`（appended 字段）；复核判 FABRICATED 与主审计同级
拒绝全链。回执 JSON 的机器路径纪律对 `claims.second` 同样适用（appended 字段只带计数，
不带本地路径）。
