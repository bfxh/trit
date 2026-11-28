# AGENT-COORDINATION：认领门与交接门

> 定位：给在本仓多会话/多 worktree 里干活的人读。它回答：改动前凭什么判断“这些路径归我”、
> 接活时凭什么判断“这还是上一棒那棵树”，以及两道门在什么情况下必须拒绝。
> 边界：只覆盖宿主侧 `tools/agent_verify.py` 的 `--claim-gate` / `--handoff` 与
> `tools/handoff.py`；不覆盖跨机信封、分布式锁；四态确认账本见
> [AGENT-DELIVERY.md](AGENT-DELIVERY.md)。
> 状态：**已跑通**（claim/handoff 门在 AF23/AF24 共 8 条断言；四态账本
> Rust `trit-orch delivery` 9/9；Python 全仓 716/0/0）。
> 容量/并发协调另见 [AGENT-CAPACITY.md](AGENT-CAPACITY.md)：硬件探针 + 跨项目租约 +
> `subagent_dispatch --capacity auto|enforce` 的 provider 前准入。本文只覆盖认领与交接。

## 1. 为什么是这两道门

- 2026-11-04 实测：12 个 worktree 里 28 个 holder 声称工作、6 个真脏、只有 2 个被认领覆盖；
  按概率抽一个 claim，26.7% 能读出“登记过但没在改”。
- `scripts/claims_audit.py` 只能**事后**对账（脏 worktree 没在效认领就判红），
  `agent_verify.py` 原来只有 `--claims-file` 做声明审计；**没有任何前置认领门**。
  文档写“开工先 claim”无法阻止下一棒直接改别人的路径。
- `tools/resume.py` 管的是“断粮后续跑”，不管 A→B 的状态转移；`HandoffPacket` /
  `handoff_gate` 只在 `crates/trit-orch` 里，Python 宿主没有消费者。

所以两道门都接在 `agent_verify.py` 的单入口链上，位置在 **preflight 之后、budget 之前**：
树不对/路径不归你，连预算与检查都不必花。

## 2. 认领门：`--claim-gate`

```powershell
python tools/agent_verify.py -Root . -Changed pkg/core.py -Lang python `
  --claim-gate --claim-who qoder:abc123 --claims-ledger <CLAIMS.md> `
  -Receipt .agent/last-receipt.json
```

- `--claim-who` 必填，形态是 `宿主:会话id`；它和 `--agent-id`（自由 run 标签）是两个身份：
  前者对账本，后者对回执。文档与命令里不要混用。
- 只检查本次 `-Changed` 的路径：先转成仓库相对（绝对路径在 `-Root` 内合法，越界即 unknown），
  再调 `scripts/agent_claims.py gate --json`。账本默认在宿主仓外的 `_handoff/CLAIMS.md`
  `--claims-ledger` 可覆盖（夹具/临时仓用）。
- 三态与拒绝：

| gate 读数 | 含义 | 本门动作 |
|---|---|---|
| `true` | `who` 的在效认领完整覆盖全部路径 | 放行，回执 `claim.verdict=true` |
| `false` | 有**别人**的在效认领与路径重叠 | `refused by claim gate`，exit 1 |
| `unknown` | 没登记 / 只覆盖一部分 / 账本读不出 | **同样拒绝**（Unknown 不是默认许可） |

- 回执 `claim` 段固定写 `who`、`paths`、`ledger`（账本相对路径或 `default`）、`verdict`；
  失败时 reason 是人类可读的一句话，并打印 claim 命令建议（含 `--plan` 与重叠时
  `compare` 后由败方 release/收窄）。
- 账本是**协调文件，不是 ACL**：能写账本的人也能改它。这道门防的是“没对账就动手”，
  不是防恶意伪造；后者要靠回执/证据链，不靠这把锁。

## 3. 交接门：`--handoff`

```powershell
# 交班：写包（默认 .agent/handoff/<id>.json，已进 .gitignore）
python tools/handoff.py create -Root . --id h-001 --from qoder:abc123 --to zcode:def456 `
  --goal "把 doctor 第七面接到 run 前" --next-action "跑 selftest AF 组" `
  --decision "不扩 ledger 缓存" --open-question "跨机包怎么传" --out <共享路径>/h-001.json

# 接班：先验包，再干活
python tools/handoff.py gate -Root . -Packet <共享路径>/h-001.json --json
python tools/agent_verify.py -Root . -Changed pkg/core.py -Lang python `
  --handoff <共享路径>/h-001.json -Receipt .agent/last-receipt.json
```

- 包字段严格镜像 Rust `HandoffPacket::validate`：`id`/`from`/`to`/`goal`/`base_commit`/
  `state_hash`/`next_action`/`idempotency_key` 必填非空；四个列表
  `decisions`/`open_questions`/`claims`/`evidence` 的项必须非空；`expires_at` 为正整数且晚于 `now`。
- `state_hash` = HEAD + `git status --porcelain -z --no-renames`（过滤本机账本后的行）+
  `git diff HEAD --binary` 的哈希 + 未跟踪文件内容哈希。它证明“交接时与接收时是同一棵树”，
  **不证明树是对的**；后者是测试/回执的活。超过 256 MB 或 20000 个未跟踪文件判“状态不可证”。
- 不计入的路径：`.journal/`、`.agent/handoff/`、`.agent/delivery/`、`.agent/resume/`、`.agent/memory/`、
  `.agent/signals.jsonl`、`.agent/last-selftest.json`、`.agent/last-simchain.json`、
  `.agent/doctor-cache.json`——宿主账本不是下一棒继承的源码状态。
- 三态与拒绝：

| gate 读数 | 条件 | 本门动作 |
|---|---|---|
| `true` | 结构合法 + 未过期 + `base_commit`/`state_hash` 都对上 | 放行 |
| `unknown` | 过期 / base 变了 / 状态指纹不一致 / 状态不可证 | `refused by handoff`，exit 1 |
| `false` | 结构不合法（缺字段、空字段、坏 JSON、本机盘符路径） | 同样拒绝 |

- 过期**不是** False：旧线索可能仍有用，只是不能当通行证。`handoff.py gate` 退出码
  0/1/2 对应 True/Unknown/False，`--json` 里同时给 `id`、`base_commit`、`state_hash`、`reason`。

## 4. 证据

- `AF23a/AF23/AF23b/AF23c`：认领后放行（`claim.verdict=true`）；未认领路径拒绝；
  他人占同路径拒绝（`false`）；`--claim-who` 缺失时参数校验拒绝。
- `AF24a/AF24/AF24b/AF24c`：同树放行（`handoff.verdict=True`）；改文件后 `Unknown` 拒绝；
  坏包 `False` 拒绝；过期包/`base_commit` 改坏 `Unknown` 拒绝。
- 本机开销（真实仓库 9 个脏文件，各 5 次）：
  `handoff.py gate` 均值 **271.0 ms**（286.7/275.4/268.5/262.9/261.5）；
  `agent_claims.py gate` 均值 **53.3 ms**（53.1/52.5/51.7/56.2/52.9）。
  两者都含 Python 启动 ~24 ms；handoff 的大头是 `git diff HEAD --binary`。
  脏树越大越慢，**没有**为它做缓存——状态指纹本身就是“每次重算”。
- 全量验收：`pwsh tools/selftest.ps1`（AF23/AF24 在内）与
  `python tools/terminology.py check` 双绿才算跑通；本文只记录已跑过的夹具，不代替全量。

## 5. 边界（诚实）

- 不是分布式锁、不做共识、不做跨机同步；单机文件 + 调用方传 `now`。
- `state_hash` 只覆盖 `-Root` 下 git 看得见的东西：子模块只覆盖 gitlink，树外写入不在范围；
  它是出处指纹，不是防篡改。
- 认领门是 **opt-in**：不传 `--claim-gate` 时行为与以前完全一样。是否在 `--agent-id` 存在时
  强制，尚未定；子代理规范要求带门，但工具不替调用者做身份推断。
- 交接门只验“同一棵树”，不验“上一棒做对了没有”；`DeliveryReceipt` 的
  Sent→Delivered→Read→Acted 四态账本已在 Rust `trit-orch delivery` 落地
  （见 [AGENT-DELIVERY.md](AGENT-DELIVERY.md)），`gate` 只在交接门为 `True` 时
  补 `sent+delivered`；`agent_verify --handoff` 仍只做交接门，宿主是否接
  `trit-orch delivery gate` 由调用方决定，不能声称单入口链已闭环。
- 不合并内容冲突；冲突归 VCS。包内禁止本机盘符/UNC 路径，跨机传递用 `--out` 到共享位置。

## 6. 还没落地（下一棒清单）

1. ~~DeliveryReceipt 四态 + 账本：谁在什么时候读到了包、有没有真的动手。~~
   **已落地** Rust `trit-orch delivery`（`cargo test -p trit-orch --test delivery --test delivery_cli --test delivery_check`
   9/9）；只读 `check` 可一次给出交接门 + 账本状态，自动接进 `agent_verify --handoff` 仍是下一棒。
2. ~~认领生命周期自动化：dispatch 前自动 claim、收工自动 release；现在是调用者手动两步。~~
   **已落地** `subagent_dispatch --claim-who <宿主:会话id>`：路由、capability、容量都通过后，
   在 provider 调用前自动 claim 本批任务的 `out` 路径；收工自动 release。声明冲突时整批
   `claim-refused`，并把刚写入的声明回滚，不留垃圾行。`--claims-ledger` / `--claim-repo` /
   `--claim-ttl` 可覆盖默认账本、仓名与 TTL；`TRIT_ORCH_CLAIM_WHO` 可代替 CLI 参数。
3. ~~调度环 `plan/select/dispatch` 的宿主侧驱动：Rust CLI 有原语，Python 还没有通用循环；
   `tools/subagent_dispatch.py` 只是“按 role 分模型并行调用”的模型派发环，不消费
   `trit-orch` 的 `Plan::select`，也不自动 claim/release；容量准入已在 provider 前
   接入（`tools/capacity_lease.py`，见 [AGENT-CAPACITY.md](AGENT-CAPACITY.md)）。~~
   **已接上主链**：`subagent_dispatch --orch-select <SelectInput.json>` 消费
   `trit-orch select`，只派发 selected 任务；自动 claim/release 见第 2 条。
   仍缺的是“执行后自动更新 snapshot/ledger 并开始下一轮”的完整多轮循环——当前由宿主
   在轮间更新状态文件后重跑；这是有意的边界，避免派发器替执行器猜状态。
4. 跨机包传输与签名：现在只能 `--out` 到共享盘，没有来源认证。
5. 把 `--claim-gate` 从 opt-in 提升为默认（需先量旧夹具/旧命令的兼容成本）。
