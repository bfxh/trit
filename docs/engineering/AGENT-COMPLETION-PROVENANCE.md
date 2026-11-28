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
- CLI `finish` 新增 `receipts` 映射；`min_assertions` 为可选全局下限，缺省时完全采用
  各 `TaskSpec.policy.min_assertions`（旧 JSON 缺该字段时默认 1）；默认要求回执。
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
| `crates/trit-orch/tests/receipt_plan.rs::receipt_without_plan_binding_is_refused` | 缺 `plan` 绑定 → `receipt_observation_mismatch`，`field=plan` |
| `crates/trit-orch/tests/receipt_plan.rs::receipt_from_another_revision_is_refused` | 回执 revision 与快照不符 → `field=revision` |
| `crates/trit-orch/tests/receipt_plan.rs::changed_task_definition_invalidates_the_receipt` | 同名任务改定义（`max_tokens += 1`）→ `field=acceptance_digest` |
| `crates/trit-orch/tests/receipt_plan.rs::self_reported_receipt_is_refused_by_default` | 默认拒绝自报回执；仅 `--allow-self-reported` 逃生舱接受 |
| `crates/trit-orch/tests/receipt_threshold.rs::receipt_measured_with_other_threshold_is_refused` | 测量用 2 条断言、完成换 1 条 → `field=acceptance_digest` |
| `crates/trit-orch/tests/receipt_threshold.rs::receipt_and_threshold_must_both_match` | 同一阈值 + 同一任务定义才放行 |
| `crates/trit-orch/tests/receipt_task_threshold.rs::task_floor_raises_above_global_floor` | 任务要求 3、全局 1 → 有效阈值 3，回执按 3 放行 |
| `crates/trit-orch/tests/receipt_task_threshold.rs::global_floor_can_raise_but_not_lower_task_floor` | 全局 4 抬高后回执不匹配；全局 0 不能压低任务下限 |
| `crates/trit-orch/tests/receipt_task_threshold.rs::legacy_task_json_without_policy_still_gets_min_one` | 旧 `TaskSpec` JSON 缺 `min_assertions` → 默认 1，顶层 `epoch` 仍可解析 |
| `crates/trit-orch/tests/finish_threshold.rs::task_min_assertions_is_authoritative_in_cli_finish` | CLI 未给全局阈值时按任务 3 判，2 条断言回执被拒 |
| `crates/trit-orch/tests/unauthorized_finish.rs::finish_with_receipts_rejects_unauthorized_settled_task` | `authorization=False` 的 settled 任务不能报完成 |

本轮实测：

```text
cargo check -p trit-orch --all-targets      -> ok
cargo test -p trit-orch --test cli          -> 18 passed / 0 failed
cargo test -p trit-orch --test finish       -> 4 passed / 0 failed
cargo test -p trit-orch --test finish_receipts -> 3 passed / 0 failed
cargo test -p trit-orch --test receipt_plan -> 4 passed / 0 failed
cargo test -p trit-orch --test receipt_threshold -> 2 passed / 0 failed
cargo test -p trit-orch --test receipt_task_threshold -> 3 passed / 0 failed
cargo test -p trit-orch --test finish_threshold -> 1 passed / 0 failed
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
- `finish_with_receipts` 的默认 `HostMeasuredOnly` 路径已要求每条 decisive 回执带
  `plan`：`revision` 必须等于 `snapshot.revision`，`acceptance_digest` 必须等于
  `acceptance_digest(spec, effective_threshold)`（见 `crates/trit-orch/src/progress/finish/check.rs`）。
  `effective_threshold = max(TaskSpec.policy.min_assertions, 调用方全局下限)`；缺绑定/不符返回
  `receipt_observation_mismatch`，`field` 取 `plan` / `revision` /
  `acceptance_digest`；显式 `--allow-self-reported` 逃生舱不要求 `plan` 绑定。
- `acceptance_digest` 固定的是 `TaskSpec` 的 `serde_json` 字节（现在包含
  `policy.min_assertions`）加本次有效阈值，属于结构等价而非跨语言语义规范哈希；
  它防「测量后换阈值」，但不证明阈值本身合理。
- `min_assertions` 已由 `TaskSpec.policy` 给出（默认 1，兼容旧 JSON）；CLI 只能作为全局下限
  **抬高**，不能压低任务下限。因此本层能保证「任务声明的最低覆盖度」与「测量/完成阈值」一致，
  但仍不能证明这个数字对任务充分。
- 只覆盖 `Proved` / `Refuted` 的 decisive 结果；`inconclusive` / `conflicts` 仍然直接拒绝完成。
- `min_assertions` 默认 1 是保守下限，不是任务充分性证明；真实任务阈值应写进 `TaskSpec`。
- 不替代 `tools/claim_audit.py`：`claim_audit` 审模型文本 claim，本门审完成声明与执行回执的
  一致性；两者可以串联，但尚未自动串联。

## 5. 后续待办

- 是否给 `Receipt` 加宿主侧 HMAC / 签名，区分「执行器自报」与「宿主可验证」；
- 是否把 `acceptance_digest` 升级为跨语言规范编码，摆脱 `serde_json` 字节序依赖；
- 是否把 `claim_audit.py` 的 `test-pass` / `number` / `aggregate` 结论作为第二证据链接入完成门。
