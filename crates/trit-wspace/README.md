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
| L1 原语 | 改 trit/偏置/scale、换行、取反 | `edit`（`Edit`） | 子张量/跨层批量操作 |
| L2 事务 | 整体校验、失败零改动、逐位回滚 | `Patch` + `Journal` | 并发/多版本事务 |
| L3 程序语义 | 前置/后置行为条件、失败原样回滚 | `program` | 程序组合、条件跳转 |
| L4 读取 | 行为穷举、指纹、f32 裕量读数 | `oracle` + `readout` | 大模型探针/解释器 |
| L5 搜索 | 无梯度精确修补、最小编辑数 | `repair`（小网络完备 BFS；大网络半径 1/2 + 束搜索） | 大规模目标、学习式提议器 |
| L6 代数 | 消置换对称性、合并 | `merge`（贪心对齐 + 符号投票） | TIES/DARE 的三值对应物、任务向量 |
| L7 规模 | GPU、外部 LLM 读写协议、模型库 | — | 全部 |

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
trit-wspace patch  xor.wsp --patch break.json --oracle xor.json --require-after --out fixed.wsp
trit-wspace apply-candidate broken.wsp --candidate fix.json --oracle xor.json --require-after --out fixed.wsp --log audit.jsonl
trit-wspace repair broken.wsp --oracle xor.json --max-edits 2 --out fixed.wsp
trit-wspace propose broken.wsp --oracle xor.json --limit 8
trit-wspace propose broken.wsp --oracle xor.json --pairs --limit 8
trit-wspace propose broken.wsp --oracle xor.json --depth 3 --limit 8
trit-wspace audit  audit.jsonl --artifact start.wsp
trit-wspace replay audit.jsonl --artifact start.wsp --out final.wsp
trit-wspace diff   a.wsp b.wsp
trit-wspace merge  a.wsp b.wsp --aligned --out merged.wsp
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

九个工具：`wsp_read`、`wsp_verify`、`wsp_patch`、`wsp_apply_candidate`、`wsp_repair`、`wsp_propose`、`wsp_audit`、`wsp_diff`、`wsp_merge`。
每次调用都从 WSP1 文本开始，服务端不存状态；失败返回 `isError=true`，不会返回“差不多对”的工件。

`wsp_propose` 是“模型提议、库复核”的入口：对单点（`depth=1`）、两编辑（`depth=2`）或
三编辑（`depth=3`）组合**实测** `after_loss` / `fixed` / `broken` / `changed`，按 `gain`
排序返回；模型挑一条后走 `wsp_patch`，库仍会重新验证。候选里的数字不是打分估计。
组合枚举超过预算会直接报错，不做静默截断（否则 top-K 就不是真 top-K）。

`wsp_apply_candidate` 直接吃 `wsp_propose` 的 `candidates[i]`（`edit` 或 `edits`），
可选 `require_after`；给了 `log` 就把这次编辑追加进 JSONL 日志（日志文本随结果返回，不写盘）。

`wsp_audit` 校验 JSONL 编辑日志的哈希链；给了初始工件就顺带重放并核对最终校验和。
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
