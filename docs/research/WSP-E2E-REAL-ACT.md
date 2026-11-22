# 真实激活校准：MiniLM 第 0 层三段线性输入

把 `bias_e2e` 的输入从 `uniform(-1, 1)` 换成 MiniLM 第 0 层三段线性层的**真实输入激活**，
回答一个更具体的问题：随机输入到底低估还是高估了量化误差。

## 1. 协议

- 模型：`sentence-transformers/all-MiniLM-L6-v2`，第 0 层三段：
  `attention.self.query`（384×384）、`intermediate.dense`（1536×384）、
  `output.dense`（384×1536）。
- 文本：`mteb/stsbenchmark-sts` test 的前 512 句（sentence1/sentence2 交替）。
- 激活：tokenizer + 模型 forward，hook 三段线性层的输入；有效 token 向量打乱后取
  **256 条**（seed 7）。导出为 `WSPACT1`（8 字节魔数 + u32 rows + u32 cols + f32le）。
- 随机对照：同一段权重、同一数量、`uniform(-1, 1)`、seed 7。
- 参照：f32 权重 + f64 累加；候选：absmean 逐行三值化 + `zero`/`trit`/`f32` 三种 bias。

导出脚本（只作对齐基线，不参与交付）：

```bash
TRIT_DATA=<数据根> HF_HUB_OFFLINE=1 \
  python crates/trit-wspace/examples/export_bias_activations.py \
  "$TRIT_DATA/miniLM-e2e-activations" 256
```

复算：

```bash
cargo run --release -p trit-wspace --example bias_e2e -- \
  --file "$HF_HOME/hub/models--sentence-transformers--all-MiniLM-L6-v2/snapshots/1110a243fdf4706b3f48f1d95db1a4f5529b4d41/model.safetensors" \
  --layer 0 --inputs 256 \
  --activations-dir "$TRIT_DATA/miniLM-e2e-activations"
```

## 2. 读数

`rel_l2` / `cosine`；`ratio` = 真实激活 / 随机输入。

| 段 | bias | random rel_l2 | random cos | real rel_l2 | real cos | ratio l2 | ratio cos |
|---|---|---:|---:|---:|---:|---:|---:|
| query | zero | 0.5771 | 0.8451 | 0.5935 | 0.8431 | 1.028 | 0.998 |
| query | trit | 0.5527 | 0.8657 | 0.5633 | 0.8699 | 1.019 | 1.005 |
| query | f32 | 0.4943 | 0.8908 | **0.4845** | **0.9020** | 0.980 | 1.012 |
| intermediate | zero | 0.5505 | 0.8632 | 0.4975 | 0.9187 | 0.904 | 1.064 |
| intermediate | trit | 0.5353 | 0.8750 | 0.4818 | 0.9217 | 0.900 | 1.053 |
| intermediate | f32 | 0.5110 | 0.8846 | **0.4464** | **0.9291** | 0.874 | 1.050 |
| output | zero | 0.5799 | 0.8454 | 0.9952 | 0.1002 | 1.716 | 0.119 |
| output | trit | 0.5777 | 0.8472 | 0.9950 | 0.1033 | 1.722 | 0.122 |
| output | f32 | 0.5755 | 0.8481 | **0.9946** | **0.1080** | 1.728 | 0.127 |

## 3. 读法

1. **query 段：随机输入没有低估**。真实 `rel_l2` 与随机几乎相同（1.02 / 1.02 / 0.98），
   cosine 也在 ±1.5% 内。input 分布对这一段的三值化误差不敏感。
2. **intermediate 段：随机输入反而高估了误差 10–13%**（ratio 0.90 / 0.90 / 0.87）。
   真实激活下 cosine 还高 5–6%。
3. **output 段：随机输入明显低估**。真实 `rel_l2` **0.995** vs 随机 **0.58**（1.72×），
   cosine 从随机的 0.85 掉到真实的 0.10。output 段的输入是 GELU 后的 1536 维激活，
   分布比 `uniform(-1, 1)` 重尾/稀疏得多；均匀输入把三值累加误差平均掉了。
4. **结论：路线图里“随机输入低估 1.5–3×”只对 output 段成立，不是全层普遍规律。**
   以后报 `bias_e2e` 读数时必须写明是哪一段、输入分布是什么；query/intermediate
   的随机读数不能外推到真实激活。

f32 bias 的真实激活收益：

| 段 | zero → f32 `rel_l2` | 相对收益 | 随机输入下的相对收益 |
|---|---|---:|---:|
| query | 0.5935 → 0.4845 | **−18.4%** | −14.4% |
| intermediate | 0.4975 → 0.4464 | **−10.3%** | −7.2% |
| output | 0.9952 → 0.9946 | −0.06% | −0.8% |

真实激活下 query/intermediate 的 f32 bias 收益比随机输入更大；output 段仍然没有区分度——
与“bias 幅度越小，收益越接近零”的旧结论一致。

## 4. 未证栏

- 仍是**三段线性**，没有 attention / LayerNorm / GELU / 残差；级联读数仍用随机链，
  不能把 `cosine ~0.44` 当作真实 transformer 的端到端精度。
- 只测第 0 层；没有后续 5 层。
- 只有 STS 句子、没有任务头；不能把 `rel_l2` 直接换算成检索/分类掉点。
- 只测 absmean 逐行三值化；没有 GPTQ / per-group / 残差侧车。
- 真实激活是 token 级（每句最多 64 token），不是句向量；样本 256 条。
- 导出脚本依赖 Python/transformers，仅作对齐基线；Rust 侧只读 `WSPACT1`。

## 5. 接口

- `examples/export_bias_activations.py`：导出三段真实激活 + `manifest.json`（sha256/形状）；
- `crates/trit-wspace/src/activations.rs`：`read_matrix` 读 `WSPACT1`，校验魔数/长度/上限；
- `examples/bias_e2e/main.rs`：`--activations-dir` 替换每层输入，输出
  `readouts_real` 与 `real_vs_random`。
