# trit-immune —— 三进制免疫系统核心

把**渗透测试**做成 DeltaDB 分支图的原生伴生机制：每个节点/分支用三进制裁决
`{-1, 0, +1}`（Kleene K3）标注，红蓝对抗史沉淀为因果 DAG，合并门禁三态动作。

语义与 [bfxh/trit](https://github.com/bfxh/trit) 的 `trit-core`（`Ternary`，K3/Ł3/B3）一致；
本 crate 是其上的**安全裁决域层**，零依赖，可直接并入该工作区作为新成员。

## 三进制语义

| 值 | 逻辑（K3） | 安全含义 | 门禁动作 |
|----|-----------|----------|----------|
| `-1` | False | **漏洞确认**：攻击成功（注入、越权、Prompt 注入……） | `BLOCK` |
| ` 0` | Unknown | **未知**：可疑攻击面 / 沙盒挂起 / 待人裁 | `FREEZE` |
| `+1` | True | **防御成功**：攻击被拦截，无可利用面 | `ALLOW` |

关键性质（Kleene K3）：

- `-1 & x == -1`：已证伪即否决——一条确证漏洞堵死整条因果链，无论还有多少未知；
- `+1 | x == +1`：已证安全即放行；
- `0` 传染但不武断：未知面冻结合并，**不会被猜成安全**；
- 新代码默认 `0`（**未测即不可信**），不是默认 `+1`。

## 构想 → 代码的映射

| 构想 | 落点 |
|------|------|
| 三进制结果 -1/0/+1 | `Trit`（`src/trit.rs`，K3 全连接词 + Ł3 蕴含） |
| 分支即攻击向量 | `NodeKind::AttackVector` + `ImmuneDag::spawn`（零成本分支） |
| 因果图即攻击链 | `ImmuneDag::chain`（root→tip 因果链） |
| 负数消息回链 | `ExploitEvidence.target` 指回被打穿节点，沉淀在 `Node::history` |
| 0 → 冻结 + 沙盒 | `GateDecision::Freeze` + `Sandbox::fuzz`（确定性替身） |
| 红蓝对抗自动化 | `Node::history`：攻击成功 → 补丁 → 重放失败的完整时间线 |
| 人类主控（0 态） | `ImmuneDag::human_review`：唯一解除隔离 / 落锤 0 的通道 |
| Prompt 注入溯源 | `NodeKind::Message` + `ImmuneDag::quarantine`（沿因果链隔离） |
| 合并门禁 | `ImmuneDag::merge_gate`：K3 AND 折叠 → 三态动作 |

## 快速开始

```bash
cargo test                      # 真值表 + 免疫网络行为全对拍
cargo run --example scenario    # 端到端红蓝对抗场景演示
```

```rust
use trit_immune::{GateDecision, ImmuneDag, NodeId, NodeKind, Trit};

let mut dag = ImmuneDag::root("main");
let feat = dag.spawn(NodeId(0), NodeKind::Delta, "登录模块");
assert_eq!(dag.merge_gate(feat), GateDecision::Freeze); // 未测即 0 → 冻结

let atk = dag.spawn(feat, NodeKind::AttackVector, "SQLi 重放");
dag.record(atk, Trit::Vulnerable, None, "打穿");
assert_eq!(dag.merge_gate(feat), GateDecision::Block);  // 一条 -1 堵死全链
```

## 持久化：DeltaDB 的 delta 编码

`ImmuneDag` 的每次变更都是一条 [`Delta`](src/delta.rs)（`ROOT / SPAWN / RECORD / QUARANTINE / REVIEW`），
实时写入与日志重放共用 `ImmuneDag::apply` 同一条代码路径——**日志即数据库**：

```rust
use trit_immune::{Journal, NodeId, NodeKind, Trit};

let mut j = Journal::create("repo.journal", "main")?;
let feat = j.spawn(NodeId(0), NodeKind::Delta, "登录模块")?;
j.record(feat, Trit::Hardened, None, "回归通过")?;
drop(j);

let j = Journal::open("repo.journal")?;   // 全量重放恢复
assert_eq!(j.dag().merge_gate(feat), trit_immune::GateDecision::Allow);
```

文件格式：一行一条 Delta，制表符分字段，字符串转义（`\t` `\n` `\r` `\\`），
可安全携带空格、换行、引号与中文：

```text
trit-immune-journal v2
1740000000000:0:0	ROOT	main
1740000000000:1:0	SPAWN	0	DELTA	登录模块
1740000000002:0:0	RECORD	1	+1	-	-	-	-	回归通过
```

**时间戳（HLC）**：每条 Delta 盖混合逻辑时钟戳 `(物理毫秒, 计数, 写者)`——
同写者严格递增（物理钟回拨只推计数）、`observe` 远端戳后本地新戳因果必然更晚、
三元组构成全序（并发戳按写者破平）。重放时非递增日志直接拒绝；
[`merge_order`](src/delta.rs) 据此对多副本 Delta 流做确定性归并——跨副本同步的前置原语已就位。

写盘失败进入 poison 状态拒绝继续追加（内存/磁盘不静默分叉），重新 `open` 即恢复一致。

**快照（重放缓存）**：`Journal::snapshot` 把全量状态（含完整审计历史与跨树边）写到
`<journal>.snapshot` sidecar（临时文件 + 原子改名）；`open` 优先从快照恢复、只重放
尾部增量，快照损坏/不匹配自动回退全量重放。前缀校验用 FNV-1a（只防意外损坏，不防
篡改）。日志本体仍只增不减——它是审计链；快照约束的是重放成本。

尚未做：副本发现/传输协议（同步的合并内核 [`merge_replicas`](src/sync.rs) 已就位）。

## 三值传播算法

跨树因果（依赖关系、共享污染消息）用 `CrossLink` 边声明，在 `(min, max)` 半环上做
标签修正传播（Bellman-Ford 式，O(V·E)）：

- **污染源**：所有非 `+1` 或被隔离的节点——确证漏洞以 `-1` 强度外泄，可疑/隔离以 `0` 强度；
- **路径强度** = max(源严重度, 沿途边通过值)：一条链的强度由最薄弱的攻击环节决定；
- **目标污染等级** = min(所有路径)：攻击者走最优路径；
- **边通过值**：`-1` 确认可穿透 / `0` 疑似 / `+1` 已证拦截（拦截边视为断开）；
- **门禁**：`merge_gate` 折叠的是**有效裁决 = min(自身裁决, 污染等级)**——
  兄弟分支的确证漏洞沿依赖边传染，同样堵死你的合并；源被打上补丁后下游自动恢复。

与 [bfxh/trit](https://github.com/bfxh/trit) `trit-core::TernaryTopology` 的关系：
其三值可达性是 max∘min（路径取边值最小、节点对取所有路径最大，`True` 边可通行）；
本 crate 安全语义里 `-1` 是"可穿透"（最坏值），故取其**极性对偶** min∘max——
测试在 81 种边值组合上与简单路径穷举对拍，并验证了对偶恒等式。

```rust
use trit_immune::{GateDecision, ImmuneDag, NodeId, NodeKind, Trit};

let mut dag = ImmuneDag::root("main");
let login = dag.spawn(NodeId(0), NodeKind::Delta, "登录");
let pay = dag.spawn(NodeId(0), NodeKind::Delta, "支付");
dag.record(pay, Trit::Hardened, None, "自身通过");
dag.cross_link(login, pay, Trit::Vulnerable, "支付复用登录会话解析");

let atk = dag.spawn(login, NodeKind::AttackVector, "SQLi");
dag.record(atk, Trit::Vulnerable, None, "打穿");
dag.record(login, Trit::Vulnerable, None, "面已证伪");
// 登录 -1 沿依赖边传染：支付自身 +1 也被拉低
assert_eq!(dag.merge_gate(pay), GateDecision::Block);
```

## 静态扫描路线：规则引擎作为三值裁决源

[`scan`](src/scan.rs) 把传统 SAST/规则引擎接进同一套 Delta 协议。校准协议只有一条
不对称规则：**扫描器永远不产生 `+1`**。

- `Dispositive`（规则即漏洞本体：硬编码凭据、关闭 TLS 校验）→ `-1`，直接证伪；
- `Heuristic`（可疑模式：eval、XSS sink、SQL 拼接）→ `0`，冻结并进红队队列；
- 未命中 → 不产生任何记录——清白的扫描只是"没发现"，不等于"安全"；
  `+1` 只能来自红队拦截、沙盒通过或人工裁定。

多条命中按 K3 AND 折叠（最坏者胜），产出 `Delta::Record` 走同一条 apply/日志路径；
被抑制（范围外）的节点整点跳过——跳过不是"洗白"。内置匹配器是演示级的确定性子串
规则，接真实引擎（Semgrep/CodeQL 输出）时只需替换 `Rule` 的匹配函数，校准协议不变。

```rust
use trit_immune::{scan, GateDecision, ImmuneDag, NodeId, NodeKind, Trit};

let mut dag = ImmuneDag::root("main");
let cfg = dag.spawn(NodeId(0), NodeKind::Delta, "配置加载");
dag.record(cfg, Trit::Hardened, None, "自身回归通过");

let report = scan::run_scan(&[(cfg, "db.password = \"hunter2\";")], scan::RULES, &[]);
for d in report.to_deltas("TRI") {
    dag.apply(&d).unwrap();
}
assert_eq!(dag.merge_gate(cfg), GateDecision::Block); // 规则即漏洞，直接证伪
```

## 跨副本同步：stamp 即身份，全序即合并

[`sync`](src/sync.rs) 把 DeltaDB 从单机免疫推向分布式免疫。模型（v1：克隆 + 追加）：

- 副本日志 = 共同前缀（原写者盖戳，克隆而来）+ 本写者盖戳的追加；
- **stamp 即 Delta 身份**：两份日志交换后按 HLC 戳去重，克隆前缀自动消重；
- 合并 = HLC 全序 → 逐条把**写者本地 id 改写为全局 id** → 复用 `apply` 同一条代码路径
  重放——两个副本各自合并，得到**逐字段相同**的 DAG（顺序无关，测试断言）；
- 独立建库的重复 `ROOT` 跳过；重放失败的 Delta 进**隔离区**（不静默丢弃，留人裁）；
- 共享节点上的裁决冲突按全序**后到者胜**，两条历史事件都保留（审计不丢）。

```rust
use trit_immune::{merge_replicas, Stamped};

// ra / rb：两份副本日志（Vec<Stamped>，journal 行解析可得）
let m = merge_replicas(vec![(0, ra), (1, rb)]);
assert_eq!(m.deferred.len(), 0);
// m.dag 即收敛后的免疫网络；m.canonical 可作为各副本的新日志基线
```

传输层与采纳：

- `Journal::adopt(canonical)`：原子重写日志文件、重建内存状态与时钟、删除失效快照；
- `sync::anti_entropy(&mut a, &mut b)`：进程内双向反熵——双方合并彼此日志并各自采纳，
  **日志文件逐字节一致**（合并内核确定性的直接推论），可反复执行；
- `sync::tcp::{serve_merge, pull_merge}`：std TCP 一问一答（8 字节大端长度 + 日志帧）。
  安全边界：只接受字面 `IP:端口`（`SocketAddr`），**不做域名解析、不请求 URL**——
  把 SSRF / DNS 重绑定面从协议层排除；对端地址由运维显式配置；
- **帧认证（可选 PSK）**：`sync::sign_frame / verify_frame` 用 HMAC-SHA256
  （[`auth`](src/auth.rs) 模块，FIPS 180-4 / RFC 2104 标准算法 + 官方测试向量对拍，
  常量时间比较）保证帧完整性与来源。威胁模型：无认证通道上最毒的攻击是注入伪造的
  `+1` 裁决"洗白"漏洞节点；重放旧帧无害（stamp 去重 + 全序 LWW，旧 Delta 不可能
  覆盖新裁决）。`*_with` 变体接受 PSK；合规部署建议将手写原语替换为经审计的
  `sha2`/`hmac` crate（签名接口一致，替换不动调用方）。

## 本地服务器（联网）

`server::Server` 把免疫网络暴露成本地 HTTP 服务（std 线程 + RwLock，读并行/写串行）：

- `GET /v1/health`、`GET /v1/render`、`GET /v1/gate?tip=N`、`GET /v1/suspects`（只读面）；
- `POST /v1/deltas`：body = 日志帧（`trit-immune-log v1`）→ 合并采纳，网络化同步；
- `POST /v1/scan`：body = `SNIP\t<node>\t<转义代码>` 行 → 校准入账（**扫描器永不 +1**）；
- `POST /v1/diag?node=N`：body = `rustc --message-format=json` 诊断流 → 编译层裁决
  （error → -1、warning → 0、干净构建 → **+1**——编译是**检查**不是扫描，通过即权威结论；
  空体/无 JSON 拒绝，防"洗白"）；
- `POST /v1/lsp?node=N`：body = LSP `publishDiagnostics` 通知流（rust-analyzer）→ 同检查类语义；
- `POST /v1/taskmd?parent=N&source=名&quarantine=0|1`：body = Markdown → 任务入账
  （Message 节点 **0 态，作者打勾≠+1**，幂等，外部 MD 自动隔离）；`GET /v1/taskbar?parent=N`：任务栏视图；
- 默认绑 `127.0.0.1`（监听 ≠ 出网请求）；一切未来出网走 `net::check_url`
  （http/https only、拒 localhost/环回/私网/保留地址、解析后逐 IP 复核）；
- 与外部工具平台（bfxh/ADV 等）的桥接与数据污染流程见 [docs/ADV-BRIDGE.md](docs/ADV-BRIDGE.md)。

## 诚实边界

- `Sandbox` 是**确定性替身**（FNV-1a 决定是否打穿，可离线复算），用于把裁决逻辑测透；
  真实部署应替换为边缘隔离执行环境（Durable Object / 容器），接口不变。
- 红队/蓝队 Agent 本身是外部组件；本 crate 只定义它们写入 DAG 的**协议**（节点、裁决、证据、门禁）。
- 只做真值函数三值逻辑（K3），与 `trit` 仓库一致；不做时序模型检验、通用 SMT。

## 测试

- `tests/trit_tables.rs`：K3 `¬/∧/∨/→/⊕` 全 9 组合逐项对拍，Ł3 蕴含对照（分岔点 `0=>0`）；
- `tests/immune.rs`：门禁三态、攻击回链、隔离传播与人工解除、沙盒确定性、0 态队列、
  兄弟分支不殃及。

## 许可

MIT OR Apache-2.0（与上游 `trit` 一致）。
