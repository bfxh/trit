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

## 未证栏

- 没有实现 `repair` 的候选限制接口，`localize` 候选是唯一可行路径；
- 单点/双点之外（3+ 点）没有搜索；
- 候选空间由 8 行 × 32 trit 限定，不代表全局不可达；
- 任务仍是 `word_embeddings` 输入 + f32 教师，不是 STS pair。
