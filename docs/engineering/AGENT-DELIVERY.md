# AGENT-DELIVERY：交接包的 DeliveryReceipt 四态账本（Rust）

> 定位：补 `handoff` 没覆盖的一层——包交出去之后，谁在什么时候确认收到、读到、
> 真的动手。交接门证明“交接时与接收时是同一棵树”；本账本只证明“这条交接包在
> 本地账本上走到了哪一步”。实现、CLI 与测试全部在 `trit-orch` 的 Rust 侧，
> 不再维护 Python 版本的 DeliveryReceipt。
>
> 状态：**已跑通**（`cargo test -p trit-orch --test delivery --test delivery_cli` 6/6）。
> `cargo test -p trit-orch` 全包通过。边界：这是宿主自报账本，不是密码学签名，
> 也不是 ACL。

## 1. 四态

```text
sent -> delivered -> read -> acted
```

| 状态 | 谁登记 | 必填 | 含义 |
|---|---|---|---|
| `sent` | 包的 `from` | 包文件 | 发送方把包写进本地账本，绑定包字节 SHA-256 |
| `delivered` | 包的 `to` | `actor` 必须是 `to` | 接收方确认包已到手 |
| `read` | 包的 `to` | 同上 | 接收方确认读过 goal/next_action |
| `acted` | 包的 `to` | 同上 + `evidence` | 接收方确认已动手，证据必须是仓库相对文本 |

- 顺序不可跳：`act` 之前必须有 `read`，`read` 之前必须有 `delivered`；跳态返回
  `ReceiptSkip`。
- actor 不可冒名：`sent` 只能是 `from`，后三态只能是 `to`；不匹配 `InputParse`。
- `acted` 的证据不能为空，不能带盘符/UNC 路径；可以写 `commit:<sha>`、
  `pr:<编号>`、`receipt:<仓库相对路径>` 或一句可复核的说明。
- 同一 `packet-id` 的包字节变了，`sent` 拒绝续账；`verify` 带包文件时判 `invalid`。
- 每条记录都有 Unix 秒级 `at`；时间倒退拒绝。账本是整文件替换写入，单写者。

## 2. Rust 入口

库入口在 `crates/trit-orch/src/delivery.rs`：

```rust
use trit_orch::delivery::{self, DeliveryJournal};

let journal = delivery::sent(&root, &packet_path, "", at)?;
let (journal, attached) = delivery::attach(&root, &packet_path, at)?;
delivery::advance(&root, "h-001", ReceiptState::Read, "zcode:def456", "", "", at)?;
let report = delivery::verify(&root, "h-001", Some(&packet_path))?;
```

`attach` 只补 `sent + delivered`，**绝不自动写 `read/acted`**。`gate` 把交接门和
落账合成一个动作：`True` 才 attach；`Unknown/False` 不落账。

CLI 仍是 stdin 一份 JSON、stdout 一份 JSON：

```powershell
# 发：包由 handoff create 产生；sent 默认写 <root>/.agent/delivery/<id>.json
'{"action":"sent","root":".","packet":".agent/handoff/h-001.json","at":1000}' |
  trit-orch delivery

# 收：接收方按顺序登记；actor 必须是包里的 to
'{"action":"attach","root":".","packet":".agent/handoff/h-001.json","at":1001}' |
  trit-orch delivery
'{"action":"read","root":".","packet_id":"h-001","actor":"zcode:def456","note":"读完 goal","at":1002}' |
  trit-orch delivery
'{"action":"act","root":".","packet_id":"h-001","actor":"zcode:def456","evidence":"commit:abc123","at":1003}' |
  trit-orch delivery
# 或显式推进：state=sent/delivered/read/acted
'{"action":"advance","root":".","packet_id":"h-001","state":"Read","actor":"zcode:def456","at":1002}' |
  trit-orch delivery

# 验：status=complete / incomplete / missing / invalid；带 packet 时校验字节哈希
'{"action":"verify","root":".","packet_id":"h-001","packet":".agent/handoff/h-001.json"}' |
  trit-orch delivery

# 交接门 + 自动 sent/delivered：gate 不是 True 就不写账本
'{"action":"gate","root":".","packet":".agent/handoff/h-001.json","receiver_state_hash":"<hash>","at":0}' |
  trit-orch delivery
```

`at=0` 由 Rust 侧取当前 Unix 秒，不把 0 当真实时间。CLI 输出字段：
`ok/action/packet_id/ledger/states/journal`；`gate` 额外输出 `gate/attached/states`；
`verify` 输出 `verify.status/complete/reason`。

## 3. 落盘与 state_hash

- 默认 `.agent/delivery/<packet-id>.json`，已进 `.gitignore`。
- `tools/handoff.py` 的 `SKIP_UNTRACKED` 显式跳过 `.agent/delivery/`：
  登记 `sent` 之后，同一棵树的 `handoff gate` 仍应是 `True`，不会因为账本
  多了一个未跟踪文件就判 `Unknown`。
- 账本里只存包 id、包哈希、from/to、goal、状态与证据；不存包全文，不存本机绝对路径。
- 账本格式标识 `trit-delivery/1`；JSON 字段与 `DeliveryJournal` 的 serde 结构一致。

## 4. 测试

- `cargo test -p trit-orch --test delivery --test delivery_cli`：**6/6**
  - `sent_then_attach_is_idempotent_and_stays_at_delivered`：sent 幂等、attach 只到
    delivered、重复 attach 不再推进；
  - `four_states_require_order_actor_and_acted_evidence`：跳态拒绝、actor 必须匹配、
    acted 缺证据拒绝、四态齐全后 `verify=complete`；
  - `skip_and_packet_tampering_are_refused`：`act` 跳态被拒；包字节改写后 `sent`
    拒绝续账、`verify=invalid`；
  - `gate_attaches_only_when_true`：门 `Unknown` 不落账、`True` 才补齐 sent+delivered；
  - `cli_round_trip_writes_ledger_under_root`：CLI 真的在 `<root>/.agent/delivery/`
    写账本，gate/verify 回读一致；
  - `cli_advance_uses_state_field_and_stops_without_acted_evidence`：`advance` 读
    `state` 字段、缺 `acted` 证据时 CLI 返回 1、补证据后四态完整。
- `cargo test -p trit-orch` 全包通过；模块预算 `python scripts/module_budget.py` 绿。

## 5. 诚实边界

- **不是签名**：能写 `.agent/delivery/` 的人也能伪造四态；它防的是“忘了登记/顺序跳步/
  冒名/证据空写”，不防恶意宿主。
- **read/acted 是接收方自报**：工具不能证明对方模型真的读了上下文，只能证明接收方宿主
  在本地账本上登记了这个事件。
- **不替 handoff 判树**：包哈希只绑定包字节；树是否仍是交接时那棵，仍由交接门的
  `base_commit + state_hash` 回答。
- **不自动推进 read/acted**：`attach`/`gate` 最多写 delivered；read/acted 必须由接收方
  显式调用 `advance`，避免把“gate 通过”误写成“已读/已做”。
- **宿主集成是调用方责任**：Rust CLI 已提供 `gate` 一键动作，但 `agent_verify --handoff`
  仍只做交接门；是否在宿主里接 `gate` 由调用方决定，本文不声称单入口链已闭环。
- **跨机不适用**：账本默认在本机 `.agent/delivery/`；跨机交接仍需把包放到共享位置，
  账本各自维护。
- **Python 版本已退场**：`tools/delivery_receipt.py` 与 `selftest.ps1` 的 AW1–AW3 不再
  维护；不要新增 Python attach 路径。
