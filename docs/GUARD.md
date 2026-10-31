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

## agent-first（默认不吵人）

响铃/弹窗默认**关**（加 `--notify` 才开）。默认行为只写两个文件：
`status.json` 与 `RUNS.md`。理由：这是给**智能体**用的仪表，不是给机器前的人用的闹钟。

## 智能体自己的等待：`parts-guard wait`

```bash
parts-guard wait --status <run>/status.json --timeout-sec 3600
# 退出码：0=DONE  1=FAILED/STALL_STOP/DISK_LOW  3=超时
```

智能体可以在一次调用里同步等到终态再行动，不需要用户转述「好像跑完了」。

## 智能体可见（关键）

看门狗除了响铃**给机器前的人**，还会把每次状态变化追加到
`D:/KF/_handoff/RUNS.md`（`--registry`）。`NOW.md` 的首行要求「开工先读 RUNS.md」——
这样换会话/换智能体时，**训练跑完这件事不会只留在日志里**。

```
- 06:09:53 `gt3` **RUN** pid=5920
- 06:10:03 `gt3` **DONE** 训练结束
```

## 磁盘预检（`--disk-min-gb`，默认 5）

启动前检查状态目录所在盘的剩余空间；运行中每 30 秒复查一次：
低于下限 → **杀掉进程**、写 `DISK_LOW`、响铃。理由：磁盘写满时 Blender/训练会
「rc=0 但产物是 0 字节」，静默失败比报错危险（2026-10-31 实际踩过：E 盘满，
48 个渲染场景静默失败）。

## 参数

`--stall-sec`（默认 900）、`--poll-sec`（5）、`--retries`（0）、
`--status <json>`、`--log <path>`、`--registry <md>`、`--disk-min-gb <GB>`、
`--attach <pid>`（托管已在跑的进程）、`--quiet`。

## 边界（必须知道）

1. **看门狗自己也怕断电**：它只保证「进程层的结束/卡死」会被发现；整机断电只能靠
   `--state` 断点 + 重启后手动重跑（或把 `train_guarded.sh` 加进开机启动项，需你授权）。
2. **心跳靠日志**：训练必须周期性打日志（`--report` 不要设得比 `--stall-sec` 还大）。
3. **不会替你调参/降级**：它只做「停/重启/通知」，实验设计仍是人的事。
