# 智能体完成回执：从「快照自称通过」到「可信 executor 回执」

> 状态：已落地（库层 + CLI 默认 fail-closed + 回归，证据见 §3）
> 范围：`trit-orch` 的 `finish` 完成闸门；不覆盖通讯回执 `DeliveryReceipt`，也不覆盖容量租约
> 最后复核：2026-11-22

## 1. 问题

`Progress::finish` 原来只从 `Snapshot.observations[*].evidence` 读取
`Option<CheckReport>`。`CheckReport` 是一个可直接由 JSON 写出的结构：

```json
{"epoch": 0, "checks": 2, "result": "Passed"}
```

`Observation` 不保存 executor `Receipt`，完成闸门也不核对 receipt。于是模型或宿主只要在
snapshot 里写一个 `Passed`，`finish` 就会报完成，不需要任何 executor 真正跑过断言。

这是一个**编排幻觉**缺口，不是执行器缺口：执行器可能根本没有被调用，或者调用了但失败，
完成声明仍然可以是绿的。

## 2. 修复

- 库层新增 `Progress::finish_with_receipts(tasks, snapshot, receipts, spec)`：
  1. 先按原逻辑拒绝 `pending` / `in_flight` / `inconclusive` / `conflicts`；
  2. 对每个 `Proved` / `Refuted` 任务，要求 `receipts` 中存在同 task 的 `Receipt`；
  3. 用 `receipt.report(&spec)` 重算 `CheckReport`，比较 `epoch` / `checks` / `result`；
  4. 缺 receipt → `UnattestedEvidence { count, tasks }`；
  5. 不一致 → `ReceiptMismatch { task }`。
- 同一个完成门还收紧了授权：`Settled` 任务必须 `authorization == True`；`False` / `Unknown`
  一律计入 `inconclusive`，不能用一条 `Passed` 反推“已授权完成”。复现实验：
  `authorization:"False"` + 有效 receipt 在修复前返回 `ok:true`，修复后返回
  `kind:"incomplete"`、`inconclusive:1`。
- 旧 `Progress::finish` 保留为库层旧入口，并标记 `deprecated`：它只从快照计事实，
  不要求回执；新调用方应使用 `finish_with_receipts`。
- CLI `finish` 新增 `receipts` 映射和 `min_assertions`（默认 1）；默认要求回执。
- `trit-orch finish --allow-unattested` 是显式 legacy 逃生门，只接受无回执的旧快照；
  正式完成声明不应使用它。
- 错误 kind 固定为 `unattested_evidence` / `receipt_mismatch`，方便宿主和测试按 kind 判定。

## 3. 证据

回归锚点：

| 测试 | 断言 |
|---|---|
| `crates/trit-orch/tests/finish.rs::finish_refuses_incomplete_plans_and_reports_complete_ones` | 带 receipt 的完成通过；伪造 `Passed` 无 receipt 返回 `unattested_evidence` |
| `crates/trit-orch/tests/finish.rs::legacy_allow_unattested_is_explicit_only` | `--allow-unattested` 只有显式给出时才接受旧快照 |
| `crates/trit-orch/tests/finish_receipts.rs::finish_with_receipts_requires_matching_executor_receipt` | 匹配 receipt 通过；缺 receipt → `UnattestedEvidence`；receipt 与 evidence 不一致 → `ReceiptMismatch` |
| `crates/trit-orch/tests/finish_receipts.rs::finish_with_receipts_rejects_failed_receipt_for_passed_claim` | 失败 receipt 不能支撑 `Passed` 声明 |
| `crates/trit-orch/tests/unauthorized_finish.rs::finish_with_receipts_rejects_unauthorized_settled_task` | `authorization=False` 的 settled 任务不能报完成 |

本轮实测：

```text
cargo check -p trit-orch --all-targets      -> ok
cargo test -p trit-orch --test cli          -> 18 passed / 0 failed
cargo test -p trit-orch --test finish       -> 3 passed / 0 failed
cargo test -p trit-orch --test finish_receipts -> 2 passed / 0 failed
cargo test -p trit-orch --test unauthorized_finish -> 1 passed / 0 failed
cargo test -p trit-orch --test progress     -> 8 passed / 0 failed
cargo clippy -p trit-orch --all-targets -- -D warnings -> ok
```

复现命令（修复后在干净工作树应失败）：

```bash
printf '%s' '{"tasks":[{"task":{"id":"a","priority":0,"duration":1,"deps":[]},
"requirements":{"cpu":1,"memory":1},"max_tokens":100,"epoch":0}],
"snapshot":{"revision":1,"observations":{"a":{"phase":"Settled","epoch":0,
"authorization":"True","freshness":"True","evidence":{"epoch":0,"checks":2,
"result":"Passed"},"usage":{"fresh":10,"cached":0,"output":5}}}}}' \
  | trit-orch finish
# 期望：exit 1, kind = unattested_evidence
```

## 4. 诚实边界

- `Receipt` 只是**执行器自报**的 exit/断言数，不是密码学签名。恶意宿主可以伪造一条 receipt；
  本层解决的是「模型只写 CheckReport、不提供执行回执」这一类幻觉，不解决「宿主本身不可信」。
- `finish_with_receipts` 不校验 `snapshot.revision`；settled 任务现在要求
  `authorization == True`，但 `Snapshot` 的 revision 与任务定义哈希仍未钉到 receipt。
- 只覆盖 `Proved` / `Refuted` 的 decisive 结果；`inconclusive` / `conflicts` 仍然直接拒绝完成。
- `min_assertions` 默认 1 是保守下限，不是任务充分性证明；真实任务阈值应由 `TaskSpec` 或
  调用方显式给出。
- 不替代 `tools/claim_audit.py`：`claim_audit` 审模型文本 claim，本门审完成声明与执行回执的
  一致性；两者可以串联，但尚未自动串联。

## 5. 后续待办

- `trit-orch finish` 是否还要把 `snapshot.revision` 钉到 receipt 的 epoch/任务定义哈希；
- 是否给 `Receipt` 加宿主侧 HMAC / 签名，区分「执行器自报」与「宿主可验证」；
- 是否把 `claim_audit.py` 的 `test-pass` / `number` / `aggregate` 结论作为第二证据链接入完成门；
- 是否要求 `min_assertions` 来自 `TaskSpec`，而不是 CLI 默认值。
