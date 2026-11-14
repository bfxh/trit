# 真实模型端到端 bias 读数（all-MiniLM-L6-v2 第 0 层）

把 f32 权重当精确参照，回答两个问题：三值权重到底丢多少、保留 f32 偏置补回多少。
复算入口是 `crates/trit-wspace/examples/bias_e2e.rs`；压缩容器读数另见
[`WSP3-RESULTS.md`](WSP3-RESULTS.md)。

## 1. 设置

模型：`sentence-transformers/all-MiniLM-L6-v2` 的 `model.safetensors`，第 0 层三段线性权重：

| 段 | 张量 | 形状 |
|---|---|---|
| query | `encoder.layer.0.attention.self.query.{weight,bias}` | 384 × 384 |
| intermediate | `encoder.layer.0.intermediate.dense.{weight,bias}` | 1536 × 384 |
| output | `encoder.layer.0.output.dense.{weight,bias}` | 384 × 1536 |

权重按 `trit-learn::ternarize`（absmean，逐行 scale `s_r`）三值化。偏置三种读法：

- `zero`：`y_r = s_r · Σ t·x`；
- `trit`：`y_r = s_r · (Σ t·x + sign(b_r))`；
- `f32`：`y_r = s_r · Σ t·x + b_r`（保留原始偏置幅度）。

输入：256 组 `uniform(-1, 1)` 随机向量，seed 7；f32 参照也用 f64 累加。读数定义：

- `mse` / `rmse`：候选与参照的均方误差（所有样本、所有输出元素汇总）；
- `rel_l2`：`√(Σ‖y−ŷ‖² / Σ‖ŷ‖²)`；
- `cosine`：全局余弦 `Σ y·ŷ / (√Σ‖y‖² · √Σ‖ŷ‖²)`。

复算：

```bash
cargo run --release -p trit-wspace --example bias_e2e -- \
  --file "$HOME/.cache/huggingface/hub/models--sentence-transformers--all-MiniLM-L6-v2/snapshots/1110a243fdf4706b3f48f1d95db1a4f5529b4d41/model.safetensors" \
  --layer 0 --inputs 256 --seed 7
```

不带 `--file` 时跑合成三层（64→128→64），用于快速检查读数值能否复现。

## 2. 逐层读数

每层独立、输入各自随机，参照该层自己的 f32 输出。

| 层 | `bias_abs_mean` | zero `rel_l2` | zero `cos` | trit `rel_l2` | trit `cos` | f32 `rel_l2` | f32 `cos` |
|---|---:|---:|---:|---:|---:|---:|---:|
| query | 0.2618 | 0.5771 | 0.8451 | 0.5527 | 0.8657 | **0.4943** | **0.8908** |
| intermediate | 0.1077 | 0.5505 | 0.8632 | 0.5353 | 0.8750 | **0.5110** | **0.8846** |
| output | 0.0590 | 0.5799 | 0.8454 | 0.5777 | 0.8472 | **0.5755** | **0.8481** |

逐层结论：

- 三值权重本身是主要误差源：即使保留 f32 bias，`rel_l2` 仍在 0.49–0.58，余弦 0.85–0.89。
- f32 bias 的收益与该层偏置幅度正相关：query（`bias_abs_mean` 0.262）把 `rel_l2` 从
  0.5771 降到 0.4943（−14.4%），intermediate（0.108）−7.2%，output（0.059）只有 −0.8%。
- 三值 bias 只补回 f32 bias 收益的一部分：query 拿到 4.2 个百分点的 `rel_l2` 改善（f32 是
  14.4），intermediate 2.8（f32 是 7.2），output 0.4（f32 是 0.8）。
  「f32 bias 值得保留」在这个读数上成立，但只在偏置幅度较大的层上明显。

## 3. 端到端级联

同一组 256 个 384 维输入，三层全部按候选方式前向，再与全 f32 三层链比较。

| bias | `mse` | `rel_l2` | `cosine` |
|---|---:|---:|---:|
| zero | 8.4128 | 0.9458 | 0.3768 |
| trit | 8.3474 | 0.9421 | 0.3937 |
| f32 | **8.0034** | **0.9225** | **0.4419** |

端到端结论：

- 三层线性级联把单层 ~0.5 的 `rel_l2` 累积到 ~0.92：线性链没有纠错，误差近似逐层放大。
- f32 bias 仍然最好：`cosine` 从 0.3768（zero）提到 0.4419（+17.3% 相对），`rel_l2`
  从 0.9458 降到 0.9225（−2.5%）。但绝对读数说明**三值权重本身的误差占主导**，
  保留 f32 bias 不能把级联误差拉回可用区间。

## 4. 单层替换

只把第 i 层三值化，其余两层保持 f32，比较整条链的最终输出。

| 替换层 | zero `rel_l2` | zero `cos` | trit `rel_l2` | trit `cos` | f32 `rel_l2` | f32 `cos` |
|---|---:|---:|---:|---:|---:|---:|
| query | 0.6232 | 0.8235 | 0.5811 | 0.8629 | **0.4498** | **0.9091** |
| intermediate | 0.6672 | 0.7783 | 0.6522 | 0.7942 | **0.6220** | **0.8146** |
| output | **0.8348** | **0.5633** | 0.8354 | 0.5624 | 0.8370 | 0.5600 |

单层替换结论：

- 同一链式输入下，三值化 output 段的余弦下降最大（0.56），query 段最小（0.82–0.91）；
  但这一排序只对「随机输入 + 无 LayerNorm/GELU/残差」的链成立，不能直接外推到真实
  transformer。
- output 段的三档偏置读数差在 ±0.5% 内，视为无区分度。f32 bias 的收益在链式输入上
  不保证出现——这与 §2 中 output 段 bias 幅度最小、逐层收益本来就接近零一致。

## 5. 判定

1. **f32 bias 值得保留**：在 bias 幅度大的层（query）能把逐层相对 L2 降 14.4%，
   三层级联余弦提升 17.3% 相对。
2. **只保留三值 bias 只能拿回部分收益**；零 bias 最差。
3. **收益有上限**：三值权重误差主导，f32 bias 不改变「级联余弦 ~0.44」的量级。
   下一步若要让三值权重进入真实模型，需要先解决权重量化误差（更大 scale 粒度、
   按块/按列 scale、或学习式补齐），而不是继续加厚 bias 表示。
4. 当前实现的 `Layer::bias_f32` 只影响 f32 读数和导出，行为判据仍是三值 bias；
   这些读数与现有语义一致。

## 6. 未证栏

- 输入是随机 `uniform(-1, 1)`，不是真实 tokenizer / 激活统计；读数只反映算子层面的
  量化误差，不代表 MiniLM 在任何真实任务上的精度。
- 只测第 0 层的三段线性权重，没有注意力、LayerNorm、GELU、残差和后续 5 层；
  真实 transformer 的非线性与归一化可能改变误差累积方式。
- 只测 absmean 逐行三值化；没有比较按块/按列 scale、GPTQ 类校准或混合精度。
- 没有在真实任务指标（检索、分类、MATH-500 等）上测；`cosine 0.44` 不能直接换算成
  任务掉点。
- 单层替换的排序依赖随机输入链，未用真实激活复测。
