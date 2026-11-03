# trit-orch

三值准入、当前快照的可派发批次选择、预算/上下文闸门、副作用台账、进度轨迹，以及
**路径认领/租约 + 四态回执 + 交接包**。

> 当前分支：`feat/orchestration-core`（不在 `main`）。
> 依赖：`trit-core` + `trit-schedule` + `serde`/`thiserror`/`serde_json`。
> 不做：执行外部调用（宿主桥执行）、强杀、跨智能体协议适配。

---

## 1. 它解决什么

| 问题 | 本 crate 的回答 |
|---|---|
| 这一轮能不能派发？ | `admit`：依赖/授权/资源/新鲜度的 K3 AND |
| 认领是不是我的？ | `ClaimStore::gate`：认领作为第五门 |
| 上下文放不下怎么办？ | `compaction_gate`：Keep/Prune/Summarize；`Unknown` 不批准有损 |
| 选哪些任务？ | `Plan::select`：当前 epoch/新鲜/前驱已结算/容量允许 |
| 副作用发出去了吗？ | `effects::Ledger`：Prepared/Sending/Sent/Confirmed/Refuted/Unknown |
| 对方收到了吗？ | `DeliveryReceipt`：Sent/Delivered/Read/Acted + Rejected/Expired |
| 交接能不能接？ | `handoff_gate`：结构/过期/状态哈希三值判定 |
| 做完没有？ | `Progress::finish`：未定/未验一律拒绝报完成 |

---

## 2. 快速用法

### 2.1 认领 + 第五门

```rust
use trit_orch::{admit_with_claim, ClaimOutcome, ClaimStore, Ternary};

let mut store = ClaimStore::new();
let claim = match store
    .claim("alice", &["crates/trit-orch/src".into()], "edit", 100, 30)
    .unwrap()
{
    ClaimOutcome::Acquired(claim) => claim,
    ClaimOutcome::Contested { contenders, .. } => {
        panic!("contested by {contenders:?}");
    }
};

let claim_gate = store
    .gate(&["crates/trit-orch/src/claim.rs".into()], "alice", 101)
    .unwrap();
let gate = admit_with_claim(
    Ternary::True,
    Ternary::True,
    Ternary::True,
    Ternary::True,
    claim_gate,
);
assert_eq!(gate, Ternary::True);
```

- `paths` 是仓库相对路径；目录前缀覆盖子路径。
- 过期 **不会** 自动释放：`gate` 返回 `Unknown`，必须重新 claim。
- `fencing` 单调；旧执行器的 `renew`/`release` 一律拒绝。

### 2.2 四态回执

```rust
use trit_orch::{DeliveryReceipt, ReceiptState};

let mut receipt = DeliveryReceipt::sent("r1", "handoff:h1").unwrap();
receipt.advance(ReceiptState::Delivered).unwrap();
receipt.advance(ReceiptState::Read).unwrap();
receipt.advance(ReceiptState::Acted).unwrap();
assert!(receipt.is_acted());
```

- 不能跳级；重复推进到当前态是幂等的。
- `reject(reason)` / `expire()` 是终止失败；之后不能再推进。
- `ReceiptLedger` 按 `idempotency_key` 去重。

### 2.3 交接包

```rust
use trit_orch::{HandoffPacket, handoff_gate, Ternary};

let packet = HandoffPacket {
    id: "h1".into(),
    from: "alice".into(),
    to: "bob".into(),
    goal: "finish trit-re split".into(),
    base_commit: "0725d92".into(),
    state_hash: "sha256:abc".into(),
    decisions: vec![],
    open_questions: vec![],
    claims: vec!["claim-7".into()],
    evidence: vec!["commit:0725d92".into()],
    next_action: "run gates".into(),
    idempotency_key: "handoff:h1".into(),
    expires_at: 200,
};

assert_eq!(handoff_gate(&packet, "sha256:abc", 100), Ternary::True);
assert_eq!(handoff_gate(&packet, "sha256:other", 100), Ternary::Unknown);
```

---

## 3. 宿主桥 CLI

`trit-orch` 从 stdin 读一份 JSON，stdout 回一份 JSON；成功永远带 `"ok": true`，
失败输出 `{"ok":false,"kind":"…","error":"…"}` 并以 1 退出。

协调面命令：

```bash
# 认领（首轮）
echo '{"owner":"alice","paths":["src/a.rs"],"intent":"edit","now":100,"ttl":10}' \
  | trit-orch claim

# 第五门（把返回的 store 原样带回）
echo '{"store":{…},"owner":"alice","paths":["src/a.rs"],"now":101}' \
  | trit-orch claim-gate

# 续约 / 释放
echo '{"store":{…},"id":"claim-1","owner":"alice","fencing":1,"now":101,"ttl":20}' \
  | trit-orch renew
echo '{"store":{…},"id":"claim-1","owner":"alice","fencing":2}' \
  | trit-orch release

# 四态回执：record → advance → …
echo '{"action":"record","id":"r1","key":"k1"}' | trit-orch receipt
echo '{"ledger":{…},"action":"advance","key":"k1","state":"Delivered"}' | trit-orch receipt

# 交接门
echo '{"packet":{…},"receiver_state_hash":"sha256:abc","now":100}' \
  | trit-orch handoff-gate

# 五门 K3 AND
echo '{"dependencies":"True","authorization":"True","resources":"True","freshness":"True","claim":"True"}' \
  | trit-orch admit-with-claim
```

- 三值输入接受 `True/Unknown/False` 或 `+1/0/-1`。
- 回执状态：`sent/delivered/read/acted`。
- `store` / `ledger` 是上一条命令返回的对象；CLI 不自己落盘，宿主负责持久化。

---

## 4. 模块地图

```text
src/
  admission.rs  K3 准入 + admit_with_claim（第五门）
  claim.rs      路径认领/租约/fencing/TTL/冲突
  receipt.rs    DeliveryReceipt 四态 + ReceiptLedger + HandoffPacket
  model.rs      TaskSpec/Limits/Phase/CheckReport/Observation/Snapshot/PlanPatch
  plan.rs       Plan/PlanPatch/select
  selection.rs  选择/Counts
  budget.rs     workers/cpu/memory/tokens
  context.rs    compaction_gate（Keep/Prune/Summarize）
  effects.rs    Effect/Ledger/Recovery/dispatch_ready
  evidence.rs   执行回执 Receipt（与通讯回执不同）
  progress.rs   轮次/完成闸门
  policy.rs     Baseline/CriticalPath/Eager/LongestFirst
  shadow.rs     TraceStore/TraceStep/Replay
  status.rs     Counts/brief
  error.rs      稳定 kind
  bin/          stdin/stdout JSON 宿主桥
```

> 注意：`evidence::Receipt` 是**执行回执**（exit/断言数），
> `receipt::DeliveryReceipt` 是**通讯回执**（Sent→Acted）。两者不要混。

---

## 5. 测试与门禁

```bash
cargo test -p trit-orch
cargo clippy -p trit-orch --all-targets -- -D warnings
cargo fmt -p trit-orch -- --check

# 变异门：只跑 trit-orch 的登记项
python scripts/mutation_check.py --pkg trit-orch
```

协调面测试（`tests/coordination.rs`，14 条）：

- 认领/续约/释放、过期回收、fencing、前缀冲突、完整覆盖
- 四态严格顺序/幂等/终止失败、ledger 去重
- 交接包结构/过期/状态哈希
- `admit_with_claim` 243 组 K3 AND

变异登记（`scripts/orch_mutants.py`）覆盖：过期续约、旧 fence、前缀冲突、
未覆盖门、回执跳级、Acted 终止、交接哈希/过期。

---

## 6. 边界

- 本 crate 不读系统时钟；`now`/`ttl` 由调用方传入，便于确定性测试。
- 不持久化认领表；宿主用 `ClaimStore` 的 serde 形态自行落盘，
  恢复用 `ClaimStore::from_claims`（允许重叠，便于显示 Contested）。
- 不执行外部副作用；`effects::Ledger` 只记状态，调用由宿主桥执行。
- 不做跨智能体协议；信封/能力协商见 `agent-wiki` 的 `Agent-Interop`。

详细语义见 [docs/COORDINATION.md](docs/COORDINATION.md)。
