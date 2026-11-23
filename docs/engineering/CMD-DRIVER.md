# CMD-DRIVER：PTC 式命令行降错

**一句话**：把“模型每轮发一条命令、等输出、再重试”换成“先写执行计划，驱动一次跑完，逐步核验，只回摘要”。它不降低质量门：任何一步非零、超时或核验失败，任务直接判红；全量日志留在本地回执。

## 1. 为什么

命令行错误很少是单点问题，常见链路是：

- 命令拼错、cwd 错、解释器不存在；
- 磁盘满、权限不足、超时；
- 写入失败后模型看不到真实结果，盲目重试，把错误叠成循环；
- 每一步的完整 stdout/stderr 都进入下一轮上下文，历史越来越厚；
- 为了“看起来成功”，跳过核验，最后交付了坏产物。

PTC（Programmatic Tool Calling）的做法是先把机械动作移出模型回路：一份计划，驱动连续执行，只把结果摘要交回模型。参考公开视频的说法，某案例 291 万 token 降到约 30.8 万、70 步变 3 步、198.4 秒变 89.7 秒。本仓库不复刻这个数字，只落地同样的工程纪律。

参考：https://v.douyin.com/rwtJ39AJDT0/

## 2. 四段结构

```text
plan.json
  -> preflight（cwd/可执行文件/输出目录/剩余空间/plan schema）
  -> execute（argv 数组，不经过 shell；逐步超时）
  -> verify（stdout/regex/文件/JSON 字段/文件数）
  -> receipt（摘要进上下文，完整 stdout/stderr 留本地）
```

计划示例：

```json
{
  "schema": 1,
  "name": "example",
  "min_free_mb": 32,
  "steps": [
    {
      "id": "gen",
      "argv": ["python", "tools/build.py", "-Out", "${OUTDIR}/out.json"],
      "verify": [
        {"type": "file_exists", "path": "${OUTDIR}/out.json"},
        {"type": "json_field", "path": "${OUTDIR}/out.json", "field": "ok", "equals": true}
      ]
    }
  ]
}
```

`${ROOT}`、`${OUTDIR}`、`${PLAN_DIR}` 由驱动展开。`argv` 必须是字符串数组；**不允许 `shell`、管道字符串和 `retry` 字段**，从源头消掉引号、转义和盲目重试。

## 3. 失败策略

- **停止优先**：任一步非零、超时或核验失败，后续步骤不执行；不会出现“前一步没成功，后一步已经改文件”。
- **无盲重试**：`attempts` 恒为 1；超时标记 `uncertain`，先核验再决定，不在驱动里自动重跑。
- **允许预期非零**：`expect_rc: [5]` 用于负例/探测命令；预期非零不算运行时错误，但仍要过 `verify`。
- **核验不是可选装饰**：`verify` 失败会把步骤状态改为 `verify_failed`，整体退出 1。
- **全量日志保留**：摘要只含步数、质量门、错误分类、首失败摘要；完整 stdout/stderr、参数、cwd、rc、耗时写入 receipt。

错误分类：

| 分类 | 触发 |
|---|---|
| `not_found` | 可执行文件不在 PATH / 路径不存在 |
| `bad_cwd` | cwd 不存在 |
| `disk_low` / `disk_full` | 输出目录剩余空间低于门槛 / ENOSPC |
| `permission` | EPERM/EACCES/Access denied |
| `timeout` | 子进程超时，标记 uncertain |
| `nonzero` | 退出码不在 `expect_rc` |
| `verify_failed` | 命令成功但产物核验失败 |
| `os_error` | 其他 OS 错误 |

## 4. 核验类型

`stdout_contains`、`stdout_not_contains`、`stdout_regex`、`stderr_contains`、`stderr_not_contains`、`file_exists`、`file_absent`、`file_nonempty`、`file_min_bytes`、`file_count`、`json_field`、`json_keys`。

## 5. 读数和用法

```powershell
python tools/cmd_driver.py -SelfTest
python tools/cmd_driver.py -Plan testdata/work-bench/ptc-plan.json -Out target/agent-quality/ptc-receipt.json
python tools/cmd_driver.py -Plan <plan.json> -Out <receipt.json> -DryRun
```

`-DryRun` 只跑 preflight，不执行命令。`-Quiet` 在成功时只留退出码；失败仍打印首失败摘要。退出码：`0` 全过/干跑成功，`1` 执行或核验失败，`2` plan 非法或预检失败。

PTC 与 `agent_verify` 共享同一条分层：完整回执/日志留在本地，模型只读摘要或
compact 模型视图。`agent_verify` 的 `--model-receipt`（审计原文 + compact 路由视图）
和 `--brief`（成功只打汇总）见 `AGENT-GUARDRAILS.md` 的“回执怎么读”。

本机实测：

```text
cmd-driver self-test: 34/34
cmd-driver: 4/4 steps ok / quality-gates 13/13 / retries 0 / errors 0
            context 117B (local-detail 51172B) / wall 1.92s
```

完整仓库自测（`pwsh tools/selftest.ps1`）：**685 passed / 0 failed / 0 skipped**，
新增 `AU1`（驱动自测）和 `AU2`（work-bench 一条计划 4 步）都在全量里通过。

`ptc-plan.json` 用**一条命令**完成 work-bench 的 preflight、`-SelfTest 182/182`、`-Run 81/81 + negative 20/20`、`-Repeat 2` 一致；质量门没有降低，且摘要只把 117B 回给模型，51KB 报告留在本地。

## 6. 边界

- 不是 shell：不支持管道、重定向、`&&`；需要管道时写成脚本，再以 argv 调脚本。
- 不是安全沙箱：它只做执行编排和核验，不负责权限隔离；不受信任的 plan 仍应在沙箱/受控账户跑。
- full receipt 可能含敏感输出；`-Out` 要放在受控目录，不要直接把 receipt 贴进公开文档。
- 磁盘预检只看输出目录；其他盘的写入失败仍可能发生，会按 `disk_full` 分类并停止。
- 这套证明的是“命令执行更可控、质量门仍生效”，不证明“任何任务都不会失败”；失败被更早、更清楚地暴露，而不是被重试掩盖。
