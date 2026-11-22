# AGENT-CAPACITY：本机容量租约与跨项目协调

> 定位：回答“这个智能体现在能不能开这么多并发/吃这么多内存”，以及“同一台机器上
> 别的项目已经占了多少”。它补的是 `subagent_dispatch` 在调用 provider 之前的
> **资源准入**，不是模型质量门，也不是命令沙盒。
> 状态：**已跑通**。`python tools/capacity_lease.py --self-test` **14/14**；
> `python tools/subagent_dispatch.py --self-test` **25/25**；全量 `selftest` 仍 **675/675**。

## 1. 为什么不是“CL 绿了就上”

CI/CL 证明的是**这份改动本身**通过检查，不证明：

- 这台机器现在还剩几个核、多少可用内存；
- 同机另一个项目是不是正在跑 32 线程构建/推理；
- 任务准备跑多久，到点谁收工、谁交接。

所以 `subagent_dispatch` 在 **provider 调用前**多了一道容量闸：

| `--capacity` | 行为 |
|---|---|
| `off` | 不申请租约，完全由调用者负责（旧行为） |
| `auto`（默认） | `--parallel > 2` 才申请；`<= 2` 视为低风险，不占租约 |
| `enforce` | 无论并行数，只要还有可调用任务就先申请 |

申请失败（`capacity-refused` / `capacity-unreadable`）时**整批拒绝**，不调用任何
provider，也不静默降级到串行。批准后，租约的 worker/内存/deadline 会被写成
`CAPACITY CONTRACT` 注入每条 prompt。

## 2. 探针：先看机器，不信任务自报

`capacity_lease.py probe` 读取：

- **CPU**：逻辑核数；本机有 `getloadavg`（Linux/macOS，或 psutil 可用）时再减
  `floor(load1)`；始终预留 **1 个逻辑核**给系统。
- **内存**：Windows `GlobalMemoryStatusEx`、Linux `/proc/meminfo` 的 `MemAvailable`、
  macOS `sysconf`/psutil；预留 `min(1 GiB, 可用量/8)` 给系统与其他工作。
- **磁盘**：租约目录最近存在父目录的空闲空间，仅作展示，不参与租约判定。

容量 = `cpu_workers` + `memory_mb`。租约只占这两项；磁盘/GPU 目前不租。
探针读数是**估算**，不是 cgroup/Job Object；真正的硬上限仍由 `run_caps.py` /
WSL `bwrap` / 调用方的进程管理负责。

## 3. 租约：共享目录、TTL、跨进程锁

默认目录：

- Windows：`%LOCALAPPDATA%\trit\capacity`
- 其他：`$XDG_STATE_HOME/trit/capacity`（缺省 `~/.local/state/trit/capacity`）
- 覆盖：`--state-dir` 或 `TRIT_CAPACITY_DIR`

目录结构：

```text
capacity/
  broker.lock          # 跨进程文件锁（Windows byte-range / Unix flock）
  leases/
    cap-<random>.json  # 一份租约一个文件，原子替换写入
```

租约字段：`id`、`project`、`agent`、`purpose`、`host`、`pid`、`created_at`、
`heartbeat_at`、`expires_at`、`resources{cpu_workers,memory_mb}`。过期租约在每次
`reserve/status/release` 时清理；损坏的租约文件**不会被忽略**，`reserve` 直接
`capacity-unreadable`，因为“读不出”不能当成“没占用”。

```powershell
python tools/capacity_lease.py probe --json
python tools/capacity_lease.py reserve --project trit --agent main `
  --workers 4 --memory-mb 2048 --ttl 900 --purpose "subagent dispatch"
python tools/capacity_lease.py status --json
python tools/capacity_lease.py contract --id cap-...
python tools/capacity_lease.py renew --id cap-... --ttl 600
python tools/capacity_lease.py release --id cap-... --owner trit:main
```

退出码：`0` 批准/查询成功，`2` 拒绝、不可读、参数错误或租约不存在。
`release` 是幂等清理的反面：不存在返回 `not-found`（exit 2），不假装释放成功。

## 4. 隐性指引：CAPACITY CONTRACT

批准后，派发器把下面这组约束放在用户 prompt 前面（用户文字仍先过
`context_guard`，契约文本是父智能体生成的受信文本）：

```text
CAPACITY CONTRACT (enforced by the parent before this task starts)
- lease: cap-... on <host>
- project / agent: trit / subagent-dispatch
- workers: at most 4; do not spawn extra threads or processes
- memory: at most 2048 MB
- deadline: 2026-11-22T...Z (UTC); finish or hand off before it
- shared host: other projects may hold leases; stay inside this slice
- if blocked or unable to finish, stop and report; do not silently retry or raise concurrency
- do not write outside the task's declared output paths
```

这不是“建议”，是任务边界的一部分：子智能体到点没做完，应当停止并报告，不靠
加线程/重试硬顶；父智能体要么续租，要么改派，要么把任务拆小。

## 5. 派发器接入

```powershell
# 默认 auto：parallel <= 2 不占租约；parallel > 2 先申请
python tools/subagent_dispatch.py --plan tasks.json --parallel 4 `
  --capacity auto --receipt .agent/subagent-dispatch.json

# 强制：即使 parallel=1 也先占租约
python tools/subagent_dispatch.py --plan tasks.json --parallel 4 `
  --capacity enforce --capacity-ttl 900 --capacity-memory-mb 2048

# 显式关闭：只给确认过不会碰机器上限的调用者
python tools/subagent_dispatch.py --plan tasks.json --parallel 4 --capacity off
```

回执 `capacity` 段记 `mode`、`required`、`lease`、`decision`（探针/已用/可用/其他
项目的租约）、`contract_sha256`、`release`。`--dry-run` 只做准入模拟，不落租约。

派发器在 `finally` 中释放租约；释放失败会把整批标成 `capacity-release-failed`
（exit 1），不把“资源泄漏”写成成功。

## 6. 证据（已跑）

`capacity_lease.py --self-test` **14/14**：

- 探针能力字段；
- 批准、释放、释放后容量归还；
- 跨项目超额拒绝；
- TTL 过期清理；
- 坏租约 `capacity-unreadable` fail-closed；
- **8 个进程同时抢 1 worker 租约**，在模拟 4 worker 主机上恰好 4 个成功；
- 派发器 `--capacity enforce` 注入 CAPACITY CONTRACT；
- 派发器结束自动释放；
- 租约被占满时派发器在 provider 调用前拒绝，输出文件不存在。

另跑：`subagent_dispatch --self-test` **25/25**，未回退；`python tools/checks.py run -Ci`
里 C047 会重跑 14/14。

## 7. 边界（诚实）

- **不是硬隔离**：租约靠调用方遵守；绕过 `subagent_dispatch` 直接调 provider 仍可超。
  硬上限仍看 `run_caps.py`、WSL `bwrap`、Job Object/AppContainer（后者尚未落地）。
- **只覆盖本机**：不跨机、不做共识、不防恶意伪造；能写租约目录的人也能改租约。
  它是协调文件，不是 ACL。
- **估算不是测量**：CPU 按逻辑核/load，内存按可用量；NUMA、容器配额、GPU 显存、
  磁盘 IO 没有纳入租约。
- **时钟/休眠**：TTL 用 wall clock；时钟回拨或长时间休眠会让 deadline 失真，
  长任务应 `renew` 或在 harness 层加 heartbeat。
- **`auto` 的阈值是策略**：`parallel <= 2` 不占租约，是“低风险默认”，不是证明
  两个线程一定安全；高价值/长任务应显式 `--capacity enforce`。
- **尚未接入**：`sandbox_run.py` 的 `run_caps` 还没有自动向租约 broker 申请；
  `capacity_lease` 也没有和 `cargo`/`ninja` 的 jobserver 打通。下一步应让
  `sandbox_run --CapacityLease` 与构建并发共用同一份租约。
