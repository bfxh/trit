# 协调语义：认领、回执、交接

> 本文件定义 `trit-orch` 的运行时协调面：
> **谁有权写（claim）→ 对方收到没有（receipt）→ 下一个会话能不能接（handoff）**。
> 代码：`src/claim.rs`、`src/receipt.rs`、`src/admission.rs`。

---

## 1. 为什么需要这一层

调度器能排出“下一个跑谁”，但排不出：

- 这个路径现在是不是被另一个会话占着？
- 旧执行器复活后，会不会覆盖新写？
- 消息发出去了，对方是“没收到”还是“没读”还是“没做”？
- 交接包过期/状态哈希不一致时，该不该执行？

这些都不是 DAG 能回答的。所以需要认领、回执、交接三个原语。

---

## 2. 认领（Claim / Lease）

### 2.1 数据结构

```rust
pub struct Claim {
    pub id: String,
    pub owner: String,
    pub paths: Vec<String>,   // 仓库相对路径；目录前缀覆盖子路径
    pub intent: String,       // 要做什么，便于冲突判断
    pub fencing: u64,         // 单调递增，防旧写者
    pub acquired_at: u64,
    pub expires_at: u64,
}
```

时间由调用方传入（`now`/`ttl`），模块不读系统时钟。

### 2.2 状态

| 状态 | 含义 |
|---|---|
| `Free` | 没有认领 |
| `Held` | 有有效认领 |
| `Contested` | 恢复数据里出现多个有效 owner（正常 acquire 不会产生） |
| `Expired` | 有认领，但已过期；**不等于 Free** |

`state_of(id, now)` 看单个认领；`state_of_paths(paths, now)` 看一组路径。

### 2.3 认领结果

```rust
pub enum ClaimOutcome {
    Acquired(Claim),
    Contested { contenders: Vec<String>, paths: Vec<String> },
}
```

- 与别的 owner 的有效认领重叠 ⇒ `Contested`，**不写入**。
- 与自己的有效认领重叠 ⇒ 合并成一条，`fencing` 递增。
- 过期认领不阻塞新认领；新认领拿到更高的 `fencing`。

### 2.4 路径规则

- 仓库相对路径；反斜杠归一成 `/`。
- 拒绝空路径、绝对路径、盘符/冒号、`..`、`*`、空段。
- 目录前缀覆盖子路径：`src` 覆盖 `src/a.rs`。
- 前缀按 `/` 边界：`src/a` 不覆盖 `src/ab`。
- 单次最多 `MAX_CLAIM_PATHS = 256` 条；单条最长 512。

### 2.5 fencing

- 每次成功 claim/renew 使 `fencing` 递增。
- `renew`/`release` 必须带当前 `fencing`；旧 fence 返回 `StaleFence`。
- 旧会话即使“复活”，也不能用旧 fence 提交。

### 2.6 TTL 与过期

- `ttl` 必须 > 0；`now + ttl` 溢出判红。
- 过期后 `renew` 返回 `ClaimExpired`；**不能复活**。
- 过期后 `release` 仍允许（清理用）。
- 过期后要写，必须重新 `claim`，拿新 fence。

### 2.7 认领门（第五门）

```rust
pub fn gate(&self, paths: &[String], owner: &str, now: u64) -> Result<Ternary, Error>
```

| 结果 | 条件 |
|---|---|
| `True` | 所有请求路径都被 `owner` 的有效认领**完整覆盖** |
| `False` | 任一请求路径被别的 owner 的有效认领**重叠** |
| `Unknown` | 未认领、已过期、或只覆盖了一部分 |

规则：

- **False 支配 Unknown**：先扫别人的有效认领，有冲突直接 False。
- 自己的窄认领不能覆盖父路径：持有 `src/a.rs` 时，请求 `src` 是 `Unknown`。
- 未认领不是 `True`：必须显式 claim，不能“默认没人”。

### 2.8 接入准入

```rust
let claim = store.gate(paths, owner, now)?;
let gate = admit_with_claim(dependencies, authorization, resources, freshness, claim);
```

`admit_with_claim` 就是四门 K3 AND 再与 `claim` 做 K3 AND：

```
dependencies & authorization & resources & freshness & claim
```

任一 `False` ⇒ `False`；没有 `False` 但有 `Unknown` ⇒ `Unknown`；全 `True` ⇒ `True`。

---

## 3. 回执（DeliveryReceipt）

> 注意：`evidence::Receipt` 是执行回执（exit/断言数），
> `receipt::DeliveryReceipt` 是通讯回执。两者不要混。

### 3.1 四态

```
Sent -> Delivered -> Read -> Acted
```

| 状态 | 含义 | 证据 |
|---|---|---|
| `Sent` | 已发出 | 发送日志 |
| `Delivered` | 对方进程收到 | 传输层 ack |
| `Read` | 对方读过 | 读取事件 |
| `Acted` | 对方据此行动 | 关联的写/PR/门 |

- 不能跳级；`Sent -> Read` 判 `ReceiptSkip`。
- 重复推进到当前态是幂等的（at-least-once）。
- 只有 `Acted` 才能说“对方照做了”。

### 3.2 终止失败

| failure | 含义 |
|---|---|
| `Rejected` | 对方明确拒绝；`reason` 保留原因 |
| `Expired` | 超时/过期 |

- 失败后不能再推进四态（`ReceiptFailed`）。
- 已 `Acted` 不能再 reject/expire（`ReceiptAlreadyActed`）。
- 重复 reject/expire 幂等。

### 3.3 去重

`ReceiptLedger` 按 `idempotency_key` 去重：

- 同一 key 重复 `record` 返回已有条目，不覆盖 id。
- 重复 `advance` 到当前态幂等。
- 未知 key 返回 `ReceiptNotFound`。

---

## 4. 交接包（HandoffPacket）

### 4.1 字段

```rust
pub struct HandoffPacket {
    pub id: String,
    pub from: String,
    pub to: String,
    pub goal: String,
    pub base_commit: String,
    pub state_hash: String,
    pub decisions: Vec<String>,
    pub open_questions: Vec<String>,
    pub claims: Vec<String>,
    pub evidence: Vec<String>,
    pub next_action: String,
    pub idempotency_key: String,
    pub expires_at: u64,
}
```

必填非空：`id/from/to/goal/base_commit/state_hash/next_action/idempotency_key`。
列表里的每一项也不能是空白。

### 4.2 交接门

```rust
pub fn handoff_gate(packet: &HandoffPacket, receiver_state_hash: &str, now: u64) -> Ternary
```

| 结果 | 条件 | 动作 |
|---|---|---|
| `True` | 结构合法、未过期、状态哈希一致 | 接收 |
| `Unknown` | 过期，或状态哈希不一致 | 先对账/重新取证 |
| `False` | 结构不合法（空字段等） | 拒绝 |

> 过期不是 `False`：它可能是“旧线索仍有用”，所以是 `Unknown`。
> 结构不合法才是 `False`。

---

## 5. 持久化与恢复

- `ClaimStore`/`ReceiptLedger` 都 derive `Serialize/Deserialize`；宿主自行落盘。
- 恢复认领用 `ClaimStore::from_claims(Vec<Claim>)`：
  - 允许重叠（恢复数据可能已经是 `Contested`），便于如实显示。
  - 拒绝空 id/owner、非法路径、重复 id、`expires_at <= acquired_at`。
  - `next_fencing = max(fencing)`；下一次 claim 继续递增。
- 恢复后先跑 `gate`/`state_of_paths`，不要默认“没有冲突”。

---

## 6. 安全边界

- 认领表是**权限**，不是建议：`gate` 不通过就不能写。
- 跨会话只能 propose；目标 accept 才执行（协议层，不在本 crate）。
- 不可逆动作（删除、force push、发布）要人确认 + 快照。
- 不信任外部输入：路径/owner/intent 都有长度/字符/范围限制。
- 不把 secret 放进 `intent`/`payload`；用引用。

---

## 7. 测试与变异

测试：`tests/coordination.rs`（14 条）

- 认领/续约/释放、过期回收、fencing、前缀冲突、完整覆盖
- 四态严格顺序/幂等/终止失败、ledger 去重
- 交接包结构/过期/状态哈希
- `admit_with_claim` 243 组 K3 AND

变异：`scripts/orch_mutants.py` 新增 9 条，覆盖：

- 过期续约、旧 fence
- 前缀冲突、未覆盖 gate
- 回执跳级、Acted 终止
- 交接哈希/过期

跑法：

```bash
python scripts/mutation_check.py --pkg trit-orch
```

---

## 8. 非目标

- 不做分布式锁/共识；单机内存 + 宿主持久化。
- 不做强杀；执行层用检查点/协作取消。
- 不做跨智能体信封；见 `agent-wiki/Agent-Interop`。
- 不做文件内容冲突合并；那是 VCS/CRDT 的职责。
