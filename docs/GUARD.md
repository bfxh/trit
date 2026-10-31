# 训练看门狗（parts-guard）

解决三类事故：**跑完不知道**、**卡死不释放 GPU**、**崩了/重启要能续**。

## 用法

```bash
# 托管（训练命令放在 -- 之后；必须带 --state 才能断点续跑）
scripts/train_guarded.sh E:/runs/exp1 -- \
  parts-train --data ... --state E:/runs/exp1 --steps 60000 ...
```

约定：
- 日志 `E:/runs/exp1/train.log`：看门狗用它判心跳（默认 **30 分钟**没更新 = 卡死）；
- 状态 `E:/runs/exp1/status.json`：`RUN / DONE / STALL / STALL_STOP / RESTART / FAILED`；
- 结束/卡死都会**响铃**并尽力弹系统消息（`msg`），进程被停掉后 GPU 立即释放。

## 行为表

| 事件 | 动作 | 状态 |
|---|---|---|
| 训练正常结束 | 停下、响铃、写状态 | `DONE` |
| 日志 30 分钟没更新 | **杀掉进程**、响铃 | `STALL_STOP`（无重试）/ `RESTART`（有重试） |
| 训练崩溃（非 0 退出） | 按 `--retries` 重启用同一命令（配 `--state` 自动续跑） | `RESTART` → `DONE` / `FAILED` |
| 机器重启 | 看门狗自身不驻留；**重启后重跑同一条 `train_guarded.sh` 即从断点继续** | — |

## 参数

`--stall-sec`（默认 900）、`--poll-sec`（5）、`--retries`（0）、
`--status <json>`、`--log <path>`、`--attach <pid>`（托管已在跑的进程）、`--quiet`。

## 边界（必须知道）

1. **看门狗自己也怕断电**：它只保证「进程层的结束/卡死」会被发现；整机断电只能靠
   `--state` 断点 + 重启后手动重跑（或把 `train_guarded.sh` 加进开机启动项，需你授权）。
2. **心跳靠日志**：训练必须周期性打日志（`--report` 不要设得比 `--stall-sec` 还大）。
3. **不会替你调参/降级**：它只做「停/重启/通知」，实验设计仍是人的事。
