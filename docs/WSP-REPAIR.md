# `repair` 路线尝试：修复已知错误样本（负结果）

## 目标

此前的编辑目标都是“把权重往 tuned 推”（`--tuned` + `delta::diff_weights`）。这一步想把目标换成
**修复已知错误样本**：edit 集是学生判错的样本，用 `repair` 在离散空间里找**精确最小补丁**，
再用 `certify_nets` 同时检查 keep 集是否被破坏。

## 尝试

`examples/editset_repair.rs`（未合并）：任务与 `editset_gate` 相同（`word_embeddings` 行做输入、
f32 `query` 层做教师、三值 `query` + 三值读出层做学生），编辑来源换成：

```rust
repair(&student, &edit, &RepairOptions { max_edits: 1, beam_width: 1, .. })
```

## 结果：不可行

| 预算 | 结果 |
|---|---|
| `edit-limit 4 --max-edits 1 --beam-width 4` | 超过 2 分钟未完成 |
| `edit-limit 1 --max-edits 1 --beam-width 1` | `memory allocation of 147456 bytes failed` |

`147456 = 384×384`：`repair` 的束搜索会为候选克隆整个网络（147k trit），在
384×384（或两层 1536×384 + 384×1536）上内存与时间都不可行。

## 替代读数：`localize` 候选上界（已合并 #465）

`editset_bound` 用 `localize` 两级定位把候选压到 8 行 × 32 trit = 132 个，再单点/双点验证
（带 keep 检查）。真实读数：

| 项 | 值 |
|---|---:|
| 基线 edit_loss / keep_loss | 8 / 0 |
| 单点最优 | `row 262 col 3 → 0`：edit_loss 8→7，keep_loss 0 |
| 双点 | 276 对，全部不优于单点 |
| 候选空间内可达 | **false** |
| keep 破坏率 | 0 |

## 结论

1. **`repair` 的真实束搜索在 384×384 上不可行**（内存/时间），`repair` 路线只能落到
   `localize` 候选上界；
2. 在 132 个候选里，1 格最多修 1/8，2 格一个都修不了第 2 个——**修复已知错误样本在这个
   候选空间内不可达**；
3. 所以“换编辑目标”目前拿不到比“推权重”更强的读数；要真正修复错误样本，需要
   - 更小的网络（候选规模降下来），或
   - `repair` 的候选限制接口（只搜承重行/指定 trit），或
   - 换一种编辑原语（如整行/整块替换）。

## 候选限制接口（scope）

`RepairOptions` 新增 `scope: Option<Vec<Edit>>`：

- `None`：全权重穷举（原行为）；
- `Some(候选)`：束搜索只在给定候选里挑，半径 1 / 半径 2 / BFS 三条路径都走 `scoped_candidates`。

CLI：

```bash
# 只允许改 layer 0 的第 0 行（该行所有 trit + bias，各 2 个目标值）
trit-wspace repair broken.wsp --oracle keep.json --rows 0:0 --max-edits 1

# 只允许改指定的单个 trit（layer:row:col:to）
trit-wspace repair broken.wsp --oracle keep.json --trits 0:0:0:-
```

`--rows` 与 `--trits` 互斥；scope 内无解时返回 `found: false`（不越界改）。

端到端验证（XOR 2-2-1，破坏一格）：

| 命令 | 结果 |
|---|---|
| `--rows 0:0 --max-edits 1` | `found: true`，1 条编辑，只改 `layer 0 row 0 col 0` |
| `--trits 0:0:0:-` | `found: true`，1 条编辑 |
| `--trits 1:0:0:+`（无解） | `found: false` |

测试：`tests/repair_scope.rs` 3 条（scoped 只改允许坐标、scope 无解返回 None、unscoped 仍能修）。

## `localize` 选行 + scoped repair 的读数

`editset_repair_readout`（新 example）：`localize` 在 edit 集上选承重行 → 展开成候选 →
`repair` 带 `scope` 做束搜索 → `certify_nets` 检查 edit/keep。

| edit 集 | max_edits | beam_width | candidates | found | 耗时 |
|---|---|---|---|---|---|
| 8 | 2 | 16 | 6,160 | **false** | 1m49s |
| 2 | 2 | 8 | 6,160 | **false** | 22.7s |
| 1 | 2 | 8 | 6,160 | **false** | 18.6s |

读法：

1. 即使 edit 集缩到**单条样本**，承重行内 2 点内仍不可达；
2. 与 #465 的候选上界一致（单点最多修 1/8、双点无改善）；
3. 所以“修复已知错误样本”在 8 个承重行 × 2 点内**不可行，不是预算问题**。

## 未证栏

- 候选限制接口已实现（scope + `--rows`/`--trits`），但还没在 384×384 上用 `localize` 选行跑完整读数；
- 单点/双点之外（3+ 点）没有搜索；
- 候选空间由 8 行 × 32 trit 限定，不代表全局不可达；
- 任务仍是 `word_embeddings` 输入 + f32 教师，不是 STS pair。
