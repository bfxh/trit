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

## 诚实的边界（🔴）

- **`features::DEFAULT_RULES` 未校准**：它们是占位阈值，不是检测结论。本文件里没有任何准确率/召回率声明，因为**还没做数据集评测**。
- **没有训练好的模型**：`decide` 只在合成模型上验证过语义（确定 / 冲突 / 输出弃权），没有用真实流量训练过。
- 流特征只有 24 维，且只统计**载荷字节**（不是线长）；snaplen 截断对特征的影响未建模。
- pcapng、IP 分片重组、超时切分、ICMP 处理均未实现；IPv6 只解扩展头，不做地址压缩表示以外的规范化。

## 下一步

1. **M1 校准**：公开数据集（CICIDS2017 / UNSW-NB15）→ 特征对齐 → 训练 `±1` 标签 → 出"弃权率 vs 误报率"曲线。阈值用分位数定，不用手调常数。
2. **M2 三值攻击图**：节点/边带证据等级，`X` 明确表示待验证；修掉 agent 把猜测当事实的问题。
3. **M3 受控探测**：allowlist 默认拒绝的端口/服务指纹，只对实验室目标。
