# AGENT-DELIVERY：交接包的 DeliveryReceipt 四态账本

> 定位：补 `handoff.py` 没覆盖的一层——包交出去之后，谁在什么时候确认收到、读到、
> 真的动手。`handoff.py` 证明“交接时与接收时是同一棵树”；本工具只证明“这条
> 交接包在本地账本上走到了哪一步”。
>
> 状态：**已跑通**（`tools/delivery_receipt.py --self-test` 19/19；`tools/selftest.ps1`
> AW1–AW3 链集成）。边界：这是宿主自报账本，不是密码学签名，也不是 ACL。

## 1. 四态

```text
sent -> delivered -> read -> acted
```

| 状态 | 谁登记 | 必填 | 含义 |
|---|---|---|---|
| `sent` | 包的 `from` | 包文件 | 发送方把包写进本地账本，绑定包字节 SHA-256 |
| `delivered` | 包的 `to` | `--actor` 必须是 `to` | 接收方确认包已到手 |
| `read` | 包的 `to` | 同上 | 接收方确认读过 goal/next_action |
| `acted` | 包的 `to` | 同上 + `--evidence` | 接收方确认已动手，证据必须是仓库相对文本 |

- 顺序不可跳：`act` 之前必须有 `read`，`read` 之前必须有 `delivered`。
- actor 不可冒名：`sent` 只能是 `from`，后三态只能是 `to`。
- `acted` 的证据不能为空，不能带本机盘符/UNC 路径；可以写 `commit:<sha>`、
  `pr:<编号>`、`receipt:<仓库相对路径>` 或一句可复核的说明。
- 同一 `packet-id` 的包字节变了，`sent` 拒绝续账；`verify --packet` 也会判红。

## 2. 用法

```powershell
# 发：包由 handoff.py create 产生；sent 默认写 .agent/delivery/<id>.json
python tools/delivery_receipt.py sent -Root . -Packet .agent/handoff/h-001.json

# 收：接收方按顺序登记
python tools/delivery_receipt.py deliver -Root . --packet-id h-001 --actor zcode:def456 --note "收到"
python tools/delivery_receipt.py read    -Root . --packet-id h-001 --actor zcode:def456 --note "读完 goal"
python tools/delivery_receipt.py act     -Root . --packet-id h-001 --actor zcode:def456 `
  --evidence "commit:abc123"

# 验：四态齐全 exit 0；只到中间态 exit 1；坏账本 exit 2
python tools/delivery_receipt.py verify -Root . --packet-id h-001 `
  --packet .agent/handoff/h-001.json --json

# 看完整账本
python tools/delivery_receipt.py show -Root . --packet-id h-001
```

输出是 ASCII JSON（`verify --json`），账本本身是可读 JSON；`show` 保留中文。

## 3. 落盘与 state_hash

- 默认 `.agent/delivery/<packet-id>.json`，已进 `.gitignore`。
- `tools/handoff.py` 的 `SKIP_UNTRACKED` 显式跳过 `.agent/delivery/`：
  登记 `sent` 之后，同一棵树的 `handoff gate` 仍应是 `True`，不会因为账本
  多了一个未跟踪文件就判 `Unknown`。
- 账本里只存包 id、包哈希、from/to、goal、状态与证据；不存包全文，不存本机绝对路径。

## 4. 测试

- `tools/delivery_receipt.py --self-test`：19/19，覆盖 sent 幂等、顺序不可跳、
  actor 不匹配、acted 缺证据、机器路径拒绝、包字节改写、只到 sent 的 incomplete、
  坏账本 shape。
- `tools/selftest.ps1` AW1–AW3：
  - AW1 工具自测 19/19；
  - AW2 `handoff create -> sent -> deliver -> read -> act -> verify complete`，
    且 `sent` 写入后 `handoff gate` 仍为 `True`（证明 `.agent/delivery` 被 state_hash 跳过）；
  - AW3 只登记 `sent` 的账本 `verify=incomplete`（exit 1），不把“发出去了”冒充“读到了”。

## 5. 诚实边界

- **不是签名**：能写 `.agent/delivery/` 的人也能伪造四态；它防的是“忘了登记/顺序跳步/
  冒名/证据空写”，不防恶意宿主。
- **read/acted 是接收方自报**：工具不能证明对方模型真的读了上下文，只能证明接收方宿主
  在本地账本上登记了这个事件。
- **不替 handoff 判树**：包哈希只绑定包字节；树是否仍是交接时那棵，仍由 `handoff.py gate`
  的 `base_commit + state_hash` 回答。
- **不自动推进**：`agent_verify --handoff` 目前只做 gate，不会自动替你写 delivered/read/acted；
  自动登记要等下一轮把四态接进单入口链，避免把“gate 通过”误写成“已读/已做”。
- **跨机不适用**：账本默认在本机 `.agent/delivery/`；跨机交接仍需把包放到共享位置，
  账本各自维护。
