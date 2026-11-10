# trit-wspace — 三值权重空间编程

把权重当**源码**：读出行为、写出补丁、验证后提交。不做梯度，不改 f32 latent，
只编辑 `{-1,0,+1}` 的权重本身。

> 外部背景（**未复现**）：2026-11-02 MIT CSAIL 报道 GPT-6 Astra 直接读写
> Qwen3.5-0.8B 的原始权重，改约 **0.016%** 的权重把 MATH-500 从 **7.9% 提到 48.1%**，
> 不走梯度（报道署名 Takehi 等）。本 crate 不复现该结果，只实现它暴露出的接口形态：
> **读 / 写 / 验证**，并把三值的离散性变成可审计性。

## 为什么三值适合权重空间编程

| 性质 | 三值权重 | f32 权重 |
|---|---|---|
| 一次编辑 | 离散、精确（三选一） | 连续值，误差不可复现 |
| 判等 | 逐位相等 | 要容差 |
| 回滚 | 存旧值即可逐位还原 | 要快照 |
| 行为验证 | 有界域可**穷举**，给出完全等价 | 只能抽样/留出 |
| 工件 | `WSP1` 文本可 diff、可校验和 | 二进制 blob |

代价：没有 f32 预训练生态；表达力受限；大规模穷举不可行。M0 的边界就是
**小网络 + 有界域**。

## 技术树（做到哪、还缺什么）

| 层 | 需要什么 | 本 crate | 缺口 |
|---|---|---|---|
| L0 表示 | 规范文本、逐 trit 可读、校验和、可 diff | `codec`（WSP1） | 大模型分片/压缩 |
| L1 原语 | 改 trit/偏置/scale、换行、取反 | `edit`（`Edit`）+ `transplant`（单元级跨层搬运） | 任意块/子张量编辑 |
| L2 事务 | 整体校验、失败零改动、逐位回滚 | `Patch` + `Journal` + `rebase`（三方重放） | 并发写锁 |
| L3 程序语义 | 前置/后置行为条件、失败原样回滚 | `program` | 程序组合、条件跳转 |
| L4 读取 | 行为穷举、指纹、f32 裕量读数 | `oracle` + `readout` | 大模型探针/解释器 |
| L5 搜索 | 无梯度精确修补、最小编辑数 | `repair`（小网络完备 BFS；大网络半径 1/2 + 束搜索） | 大规模目标、学习式提议器 |
| L6 代数 | 消置换对称性、合并 | `merge`（贪心对齐 + 符号投票）+ `task`（任务向量投票）+ `dare`（DARE 丢弃） | 大规模合并 |
| L7 规模 | GPU、外部 LLM 读写协议、模型库 | `safetensors`（`list`/`import`/`export`，单/多张量组装成多层，F32/F16/BF16） | 整模型分片/压缩、GPU、模型库 |

## 三值的两条规范自由度（本库钉死）

1. **正 scale 不改行为**：符号判决只看累加符号，scale 只影响 f32 读数；
2. **隐藏单元置换**：交换第 `l` 层两行并交换第 `l+1` 层两列，行为不变。

所以行为判等必须走 `Oracle` 穷举，不能比较权重。`merge` 先对齐再投票，
`Patch` 允许改 scale 但不把它算进行为。

## 快速验证

```bash
cargo test -p trit-wspace
cargo run -p trit-wspace --example program_xor
python scripts/mutation_check.py --pkg trit-wspace
python scripts/module_budget.py
```

## 给模型 / 脚本的工具面

二进制 `trit-wspace` 有两套入口，共用 `ops` 里同一条判决路径：

### CLI

```bash
trit-wspace read   xor.wsp
trit-wspace verify xor.wsp --oracle xor.json
trit-wspace task-merge base.wsp tuned-a.wsp tuned-b.wsp --aligned --out merged.wsp
trit-wspace task-merge base.wsp tuned-a.wsp tuned-b.wsp --aligned --dare-rate 0.5 --dare-seed 7 --out merged.wsp
trit-wspace task-merge base.wsp tuned-a.wsp tuned-b.wsp --aligned --dare-per-task 0.5 --dare-seed 7 --out merged.wsp
trit-wspace task-merge base.wsp tuned-a.wsp tuned-b.wsp --weight 2,1 --out merged.wsp
trit-wspace script xor.wsp --script repair.json --oracle xor.json --out fixed.wsp
trit-wspace patch  xor.wsp --patch break.json --oracle xor.json --require-after --out fixed.wsp
trit-wspace apply-candidate broken.wsp --candidate fix.json --oracle xor.json --require-after --out fixed.wsp --log audit.jsonl
trit-wspace rebase newer.wsp --base older.wsp --patch fix.json --oracle xor.json --require-after --out fixed.wsp
trit-wspace transplant dest.wsp --from src.wsp --layer 0 --rows 0,1 --oracle xor.json --require-after --out fixed.wsp
trit-wspace dare-drop base.wsp --tuned tuned.wsp --rate 0.5 --seed 7 --out dropped.wsp
trit-wspace dare-drop base.wsp --tuned tuned.wsp --rate 0.5 --seed 7 --by-row --out dropped.wsp
trit-wspace repair broken.wsp --oracle xor.json --max-edits 2 --out fixed.wsp
trit-wspace propose broken.wsp --oracle xor.json --limit 8
trit-wspace propose broken.wsp --oracle xor.json --pairs --limit 8
trit-wspace propose broken.wsp --oracle xor.json --depth 3 --limit 8
trit-wspace explain broken.wsp --oracle xor.json --sample 0 --top 8
trit-wspace probe   xor.wsp --oracle probe.json --budget 64 --top 16 --adaptive
trit-wspace localize broken.wsp --oracle xor.json --rows 3 --row-budget 64 --max-trits 64 --top-trits 8
trit-wspace audit  audit.jsonl --artifact start.wsp
trit-wspace shard  audit.jsonl --size 512 --out shards/
trit-wspace audit-shards shards/ --artifact start.wsp --out final.wsp
trit-wspace audit-shards shards/ --artifact start.wsp --checkpoint-every 16 --checkpoint-dir cps/
trit-wspace audit-shards shards/ --resume cps/checkpoint-016.json --out final.wsp
trit-wspace replay audit.jsonl --artifact start.wsp --out final.wsp
trit-wspace diff   a.wsp b.wsp
trit-wspace merge  a.wsp b.wsp --aligned --out merged.wsp
trit-wspace list-safetensors model.safetensors
trit-wspace import-safetensors model.safetensors --tensor model.layers.0.mlp.up_proj.weight --out up.wsp
trit-wspace import-safetensors model.safetensors --tensor up_proj.weight,down_proj.weight --out mlp.wsp
trit-wspace export-safetensors up.wsp --out up.safetensors --tensor up_proj.weight
trit-wspace mcp
```

输入 `-` 读 stdin；报告一律 JSON；`--out` 用“临时文件 + 改名”落盘，避免半写。

### 补丁线格式（version=1）

```json
{"version": 1, "edits": [
  {"op": "set_trit", "layer": 0, "row": 0, "col": 0, "to": "+"},
  {"op": "set_bias", "layer": 0, "row": 1, "to": "-"},
  {"op": "swap_rows", "layer": 0, "a": 0, "b": 1}
]}
```

### 验证集两种写法

```json
{"inputs": [["-","+"], ["+","-"]], "outputs": [["+"], ["-"]]}
{"domain": "boolean", "vars": 2, "table": "0110"}
```

紧凑表的顺序是字典序（最后一个变量变化最快），boolean 用 `0/1/x`，ternary 用 `-0+`。

### MCP（stdio）

```json
{
  "mcpServers": {
    "trit-wspace": {
      "command": "C:/path/to/trit-wspace.exe",
      "args": ["mcp"]
    }
  }
}
```

二十一个工具：`wsp_read`、`wsp_verify`、`wsp_patch`、`wsp_apply_candidate`、`wsp_rebase`、`wsp_transplant`、`wsp_dare_drop`、`wsp_repair`、`wsp_propose`、`wsp_explain`、`wsp_probe`、`wsp_localize`、`wsp_audit`、`wsp_diff`、`wsp_merge`、`wsp_shard`、`wsp_audit_shards`、`wsp_run_script`、`wsp_task_merge`、`wsp_task_scale`、`wsp_task_combine`。
每次调用都从 WSP1 文本开始，服务端不存状态；失败返回 `isError=true`，不会返回“差不多对”的工件。

`wsp_propose` 是“模型提议、库复核”的入口：对单点（`depth=1`）、两编辑（`depth=2`）或
三编辑（`depth=3`）组合**实测** `after_loss` / `fixed` / `broken` / `changed`，按 `gain`
排序返回；模型挑一条后走 `wsp_patch`，库仍会重新验证。候选里的数字不是打分估计。
组合枚举超过预算会直接报错，不做静默截断（否则 top-K 就不是真 top-K）。

`wsp_rebase` 把**针对旧工件写的补丁**落到已经前进的工件上：逐条比较 `base` 与 `target`
在同一坐标上的状态，相同则应用，`target` 已是目标值则记为已应用并跳过，不同且不是已应用
就判冲突。默认只要有冲突就整体放弃（`rejected=true`，工件一个字节都不改）；`skip_conflicts`
才只落干净的那部分，冲突仍逐条返回（点编辑给三值字符，scale 给数值，行操作给行指纹）。
CLI 对应 `rebase <target> --base <base> --patch <json>`。

`wsp_transplant` 把一个**隐藏单元整体**从 `source` 搬到 target：第 `l` 层第 `r` 行（入边）、
偏置、scale，以及第 `l+1` 层第 `r` 列（出边）一起改；输出层没有下游列，只搬行。
产物是普通补丁，可以再走事务 / 审计日志 / 三方重放；只生成与目标不同的编辑。
CLI 对应 `transplant <dest> --from <source> --layer L --rows i,j`。

`wsp_dare_drop` 是 DARE 的三值对应物：对 `base → tuned` 的差分 trit 按 `rate` 随机丢弃
（丢回基准值），保留部分照常落位。丢弃决定由 splitmix64 对 `(seed, layer, row, col)`
散列得出，同种子永远同结果。三值写不下 `1/(1-rate)` 重标度，就把它吸收进**有保留差分的行**
的 scale——scale 是正标尺自由度，只改 f32 读数、不改符号行为，对穷举判决是中性记录。
`rate=0` 等价完整差分；`rate` 必须在 `[0, 1)`。产物是普通补丁，可接 `wsp_task_merge` 前先稀疏化。
默认按**坐标**决策；`--by-row`（MCP `by_row`）改成**整行**决策：一行只要有任何 trit/偏置差分，
就整行保留或整行丢回基准值，不会出现“半行保留”。行级决策用 `(seed, layer, row)` 散列
（列标记 `usize::MAX - 1`，与偏置的 `usize::MAX` 区分），只改本层该行的入边与偏置，
不碰下一层的出边列（单元粒度整体搬运见 `wsp_transplant`）。`kept` / `dropped` 仍按差分坐标数计数。
CLI 对应 `dare-drop <base> --tuned <tuned> --rate R [--seed N] [--by-row|--by-layer|--block N]`；
`--block N` 把同一行的列按 `N` 一块整体保留/丢弃（`N=1` 退化为坐标级，`N=0` 判错）；
`--by-layer` 整层（所有行、所有 trit 与偏置）一起保留/丢弃。

`wsp_apply_candidate` 直接吃 `wsp_propose` 的 `candidates[i]`（`edit` 或 `edits`），
可选 `require_after`；给了 `log` 就把这次编辑追加进 JSONL 日志（日志文本随结果返回，不写盘）。

`wsp_explain` 解释单个样本：先对全部候选只做**这一个样本**的前向，算 `flips` / `fixes`；
再对 top-K 做整验证集复核，给 `after_loss` / `gain`。大网只需要线性于权重数的单样本前向。

`wsp_probe` 是大网读取：逐行清零/取反测行为漂移，成本 `2×行数×样本`，与行宽无关；
`budget` 超了默认按固定步长均匀抽样，`adaptive=true` 时每个隐层先 **pilot** 一条静态最高分行，
用消融结果校准层权重、丢掉“高活跃但消融无效”的行，再按活跃度/层权重/静态分填满预算；
报告里明确标 `sampled` / `stride` / `selection` / `pilot_rows` / `pilot_kept`。
没有验证集时用 `Oracle::sampled_behavior` 以网络当前行为生成确定性随机探针集。

`wsp_localize` 做两级定位：先自适应选承重行，再在每行内逐位置（权重 + 偏置）试替代值、
整验证集复核；`max_trits` 超了按位置的**实际活跃次数**选点（权重为零或入参恒为零的位置得 0 分），
报告标 `selection=adaptive` / `sampled` / `stride=0`。

`wsp_task_merge` 做三值任务向量合并：每个任务只对**它改过的坐标**投票，票和取符号、平票
保留基准，全体投 `0` 时取 `0`；任务向量带基准校验和，拿错基准会报错；`aligned=true` / `--aligned` 先消隐藏单元置换再求差分。
scale 按**行**单独记录差分，合并时对同一行取各任务目标值的**中位数**（偶数个取中间两个的平均），
与基准位相同则不写编辑——不再丢弃任务的 scale 调优。
`dare_rate` / `--dare-rate`（配 `dare_seed` / `--dare-seed`）在投票合并**之后**对合并差分做一次
坐标级 DARE 稀疏化：投票仍用完整任务向量，`rate=0` 与不传完全一致；`rate` 非 `[0, 1)` 判错。
`dare_per_task` / `--dare-per-task` 是相反的次序：每个任务**先**用自己的子种子
（`seed ^ index * 常数`）丢坐标，再投票——被丢的票不参与投票，可能改变结果；合并后对有差异的
行统一乘一次 `1/(1-rate)`。两个参数互斥。CLI 对应 `task-merge`。
`weights` / `--weight W[,W...]` 给每个任务一个权重（与任务数等长、有限非负）：trit/偏置的票和
按权重累加，权重 `0` 的任务完全不参与（票与 scale 统计都不进）。scale 用**加权中位数**——排序后
取累计权重达到总权重一半处的值，恰好落在两个值中间时取平均（等权时退化为普通中位数）；
权重全为 1 时与不带权重的合并逐位一致。权重可与 `dare_rate` / `dare_per_task` 同用，
稀疏化的次序不变，投票阶段按权重累加。
`oracle` / `--oracle <json|->` 是可选验证集（格式同 `verify`）：给了就在结果里返回
`verify.before` / `verify.after` 的 `loss` / `exact` / `accuracy`，用来判断这次合并
相对基准是改善还是退化；不传时 `verify: null`、不额外跑前向。
`tune_weights` / `--tune-weights`（配 `tune_rounds` / `--tune-rounds`，默认 1）用 `oracle` 做坐标下降：
从等权出发，每轮对每个任务试 `{0, 0.5, 1, 2}`，只接受验证集 `loss` 严格下降的改动，某轮无改进就停。
结果里的 `tuning` 给出 `rounds` / `evaluations` / `initial_loss` / `tuned_loss`，`weights` 是选出的权重。
评估的是加权投票合并（不含 DARE）；`tune_weights` 与显式 `weights` 互斥且必需 oracle；
`tune_rounds=0` 只算等权基线。
`tune_metric` / `--tune-metric` 选调权目标：`exact`（默认，逐样本完全匹配）、`hamming`
（逐位失配数）、`argmax`（分类错误数；多维按最大 trit，**一维按符号**）或 `readout`
（`Net::readout` 的 f32 连续分数分类错误数，能区分同号不同分）。库层对应
`tune_task_weights` / `tune_task_weights_hamming` / `tune_task_weights_argmax` /
`tune_task_weights_readout`；通用入口是 `tune_task_weights_with(base, tasks, rounds, evaluate)`。
`tune_grid` / `--tune-grid G[,G...]` 换候选权重网格（默认 `[0, 0.5, 1, 2]`）；
`tune_with_dare` / `--tune-with-dare` 让调权评估走最终 DARE 合并路径（配 `dare_rate` 或
`dare_per_task`）。非 DARE 路径还会在结果里返回 `merge_report`（投票数/平票数/忽略任务数/
改动数/scale 行数）。

`task-scale` / `wsp_task_scale` 做三值任务算术：`--lambda L` 单系数（`L>0` 原样、`L=0` 丢弃、
`L<0` 取反 trit/偏置且丢 scale 差分）；`--lambdas L,L...` 批量扫描，配 `--oracle` 时每个 λ
返回 `verify`（loss/exact/accuracy），批量结果在 `results` 数组中（不含 `artifact`）；
`--layer-lambdas L0,L1...` 按层给系数（每层一个；0 丢弃该层、<0 取反该层）；
`--row-lambdas L...`（长度=总行数）与 `--block-lambdas L... --block-size N`（长度=总块数）
把系数扩到行/列块；以上四种与 `--lambda/--lambdas` 互斥。

`--row-lambdas-file <json>` / `--block-lambdas-file <json> --block-size N` 用稀疏键值对
（如 `{"0:1": 0.0}` / `{"0:0:0": -1.0}`；未给出的行/块默认 1）给按行/按块系数；
`--row-lambdas-patterns <json>` / `--block-lambdas-patterns <json> --block-size N` 支持通配/区间/
集合（如 `{"0:*": 0.5}`、`{"0:1-3": -1.0}`、`{"0:1|3": 0.5}`；精确段优先于区间/集合/通配，
未匹配默认 1）。

`task-combine <base> --tasks a,b,... --lambdas L,L...` / `wsp_task_combine` 按 λ 组合多个任务
向量（λ<0 取反、0 丢弃；合并权重取 |λ|，三值没有幅度）。

`task-scale` / `wsp_task_scale` 做三值任务算术：`--lambda L` 单系数（`L>0` 原样、`L=0` 丢弃、
`L<0` 取反 trit/偏置且丢 scale 差分）；`--lambdas L,L...` 批量扫描，配 `--oracle` 时每个 λ
返回 `verify`（loss/exact/accuracy），批量结果在 `results` 数组中（不含 `artifact`）。

真实数据读数见 `examples/cifar_tasks.rs`（CIFAR-10 8×8 灰度 → 三值 MLP → WSP1 → 合并/调权）；
真实训练产物接入见 `examples/trit_train_tasks.rs`（`trit-train::layer::TernaryLinear` 训练 →
写 WSP1 → 读回 → 合并/调权/λ 扫描；支持 `--data` CIFAR 夹具、`--hidden 8,16` 任意层端到端反向、
`--activation tanh|relu`、`--init modulo|xorshift`、每层 Adam（latent/scale 分开 lr）、
`--dare-rate`/`--dare-per-task`/`--by-block`/`--tune-with-dare`；
`trit-train` 作为 dev-dependency，同为 L3；可复算 smoke：`python scripts/trit_train_tasks_smoke.py`）；
它支持 `--size`（8/16/32）、`--color`（16×16×3 或 32×32×3）、`--hidden N`、`--seeds N`（多 seed 均值/方差）
与同架构 f32 基线（连续输入 + 连续权重），需要外部数据夹具，CI 只编译不运行。

`wsp_run_script` 执行组合脚本：`sequence` 默认原子（任一步失败/回滚则整段恢复原样，
`atomic=false` 保留已成功步骤），`when` 按当前行为（完整逐样本输出）选择 `then` / `otherwise`，
报告是树形的 `steps` / `matched` / `chosen`。CLI 对应 `script`。

`wsp_audit` 校验 JSONL 编辑日志的哈希链；给了初始工件就顺带重放并核对最终校验和。
`wsp_shard` 把日志切成带 header 的分片：每片首行声明序号/起止/父子链头，并在 `oracles`
表里存本片用到的验证集（每条只存一次），记录行改用 `oracle_index` 指向表项；
`wsp_audit_shards` 校验跨片 `seq` / `prev` / `head` 与 oracle 表摘要，可选重放。
分片 header 的 `oracle_parent` 指向上一个分片，`oracles` 只写**新增**验证集，后面的分片
直接复用父表（记录行仍用 `oracle_index`）；`audit-shards`/`wsp_audit_shards` 逐片解析时把
父表拼回有效表，检查点也带上这张表以便续放。

CLI 的 `audit-shards` 是**流式**的：`ShardCursor` / `ShardReplay` 读一片、校验一片、应用一片，
不保留分片文本；`--out` 直接落盘流式重放的最终工件。`--checkpoint-every N --checkpoint-dir D`
在分片边界写检查点（工件 + 已消费条数/链头/计数），`--resume <checkpoint>` 先验完前缀链再从
检查点续放，避免重放整段历史。MCP `wsp_audit_shards` 同样接受 `checkpoint` / `checkpoint_every`。`shard` 同时写 `manifest.json`
（每片的文件、序号、条数、链头与文本摘要），`audit-shards` 有清单时按清单读文件并逐片核对
摘要，缺片/多片/错序/内容替换都会直接报错。`wsp_shard` 支持 `start` / `limit` 分页按片返回
（结果带 `total` / `next` / `manifest`），`wsp_audit_shards` 可带 `manifest` 复核。
模板见 [`mcp.example.json`](mcp.example.json)。

> 注册后需要**重启/重载 Qoder App**，工具目录才会出现；当前会话的 `mcp_list` 不会热更新。
> 本地已注册的服务 key 是 `trit-wspace`，命令为 `trit-wspace mcp`。

### 审计日志（JSONL + 哈希链）

`patch --log audit.jsonl` 每次追加一条；每行含 `before` / `after` 校验和、补丁、改动量、
`applied` / `rolled_back`、`prev` / `chain`。`audit` 验链（抓篡改/乱序），`replay` 从初始
工件逐条重放并核对每一步校验和。回滚/未应用条目必须前后同校验和，日志本身自洽。

记录时若给了 `--oracle`，还会存下验证集规范与前后**行为指纹**；`audit` / `replay`
会同时核对权重链与行为链（`behavior_checked` 是实际核对条数）。同一验证集只在第一条
内嵌，后续条目写 `oracle_ref`（向前指向首条序号），日志不重复存；越界/自指引用直接判错。

## 论文与来源

- **Weight-space programming（外部报道，未复现）**：MIT CSAIL，2026-11-02，
  Astra 读写 Qwen3.5-0.8B 原始权重；0.016% 权重、MATH-500 7.9%→48.1%。
- A Survey of Weight Space Learning: Understanding, Representation, and Generation,
  arXiv:2603.10090（2026）——把 WSL 分成理解/表示/生成三支。
- HyperNetworks, Ha/Dai/Le, ICLR 2017, arXiv:1609.09106。
- Learning to Control Fast-Weight Memories, Schmidhuber, *Neural Computation* 1992。
- Using Fast Weights to Attend to the Recent Past, Ba et al., NeurIPS 2016。
- Linear Transformers Are Secretly Fast Weight Programmers, Schlag et al.,
  ICML 2021, arXiv:2102.11174。
- Git Re-Basin: Merging Models modulo Permutation Symmetries, Ainsworth et al.,
  ICLR 2023, arXiv:2209.04836。
- Editing Models with Task Arithmetic, Ilharco et al., ICLR 2023, arXiv:2212.04089；
  TIES-Merging, Yadav et al., NeurIPS 2023, arXiv:2306.01708。
- Ternary Weight Networks, Li/Liu/Wang, arXiv:1605.04711；
  BitNet b1.58（The Era of 1-bit LLMs）, Ma et al., arXiv:2402.17764。
- Estimating or Propagating Gradients Through Stochastic Neurons,
  Bengio/Léonard/Courville, arXiv:1308.3432（STE）。

## 边界（没做的部分）

- 只有符号激活的前馈网络；没有注意力、残差、卷积；
- 验证是**有界穷举**，不是形式化证明；域外没有保证；
- 修补在状态空间 ≤20 万时完备（最小编辑数），超过后是启发式；
- 没有自然语言 → 补丁的模型；本库提供的是供外部程序/模型调用的编辑原语与验证闭环。
