# `repair` 上界：候选空间内「最少改几个 trit 能达成 edit 集」

## 问题

`editset_gate` 给出的编辑达成率来自合成 Δ（10,128 trits）——它回答“这类编辑行不行”，
但不回答“**最少**改几格能达成”。`repair` 本来是回答这个的，但 384×384 的全量束搜索
（147,456 个 trit × 8 条 edit 样本）跑不动。

## 做法

先用 `localize` 的两级定位把候选压下来：

1. **行探针**（`probe_rows_adaptive`）：在 8 条 edit 样本上自适应测行级消融，选 top 行；
2. **行内扫描**（`localize_row`）：在承重行内逐位置（权重 + 偏置）试替代值；
3. 同一坐标只保留 `gain` 最大的目标值，得到候选集；
4. 对每个候选单点做完整 `edit ⊕ keep` 验证；单点不可达时，再从前 24 个候选两两组合。

```bash
cargo run --release -p trit-wspace --example editset_bound -- \
  --safetensors model.safetensors --samples 512 \
  --edit-limit 8 --keep-limit 64 --top-rows 8 --top-trits 32
```

任务定义与 `editset_gate` 一致：真实 `word_embeddings` 行做输入、f32 `query` 层做教师、
三值 `query` + 三值读出层做学生。

## 真实读数

all-MiniLM-L6-v2，`query` 384×384，`--samples 512`，`edit-limit 8`，`keep-limit 64`，
`row-budget 64`、`top-rows 8`、`max-trits 64`、`top-trits 32`、`pair-limit 24`：

| 项 | 值 |
|---|---:|
| 基线 edit_loss / keep_loss | 8 / 0 |
| localize 选中行 | 8 |
| 行内探测位置 | 512 |
| 去重后候选 | 132 |
| 单点最优 | `row 262, col 3 → 0`：edit_loss 7、keep_loss 0 |
| 双点评估 | 276 对，全部不优于单点 |
| 候选空间内可达 | **false** |
| 单点 keep 破坏率 | 0 |

读法：

1. **在 132 个候选里，1 格最多修 1/8，2 格一个都修不了第 2 个**——候选空间内不可达；
2. keep 破坏率 0 说明这些单点改动太小，根本碰不到保持集——和 `editset_gate` 里
   10,128 trits 修 3/8、破坏 24/64 形成对照：**改动量与副作用同向增长**；
3. `localize` 选中的 8 行覆盖了 512 个探测位置，但真正有增益的单点只有 1 个——
   行内大部分位置对 edit 集无影响。

## 未证栏

- 候选空间由 `localize` 的 `top-rows × top-trits` 决定（8×32），**不代表全局不可达**；
  把 `--top-rows` 提到 32、`--top-trits` 提到 128 可能找到 2–3 点解，但成本同步上升；
- 没有跑 `repair` 的真实束搜索（`beam_width`/`max_edits` 参数），只用了单点/双点穷举；
- 行探针与行内扫描都是启发式（`localize` 文档已标注），打分最多看 256 条样本；
- 任务本身仍是“真实词向量输入 + f32 query 教师 + 三值读出”，不是端到端 STS。

## 接口

- `examples/editset_bound.rs`：本页协议的唯一实现，输出 `localize` / `best_single` /
  `best_pair` / `reachable` 等字段；
- 复用 `localize::localize`（两级定位）与 `Oracle::loss`（edit/keep 双集合验证）。
