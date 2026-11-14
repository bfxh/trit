# 更大模型与 GPU 落点

状态：**M0 读数已补**。两部分：

1. `bias_e2e --layers N`：真实 MiniLM 6 层（18 个线性段）的误差累积读数。
2. `trit-gpu/examples/wspace_gpu.rs`：真实 WSP1 权重的 WF32 GPU vs CPU 对拍/计时。

## 1. 6 层线性堆叠（真实 MiniLM）

- 模型：all-MiniLM-L6-v2，`encoder.layer.0..5`，每层 query / intermediate / output。
- 输入：256 组 `uniform(-1,1)`，seed 7；逐层读数每段独立随机输入，级联用同一组 384 维输入。
- 逐层（18 段）：`rel_l2` 在 0.49–0.58；f32 bias 一致最好（例如 layer0.query
  0.5771→0.4943，layer5.intermediate 0.5223→0.5179）。
- 级联：

| bias | `rel_l2` | `cosine` | `mse` |
|---|---:|---:|---:|
| zero | 0.99997 | 0.1256 | 5,587,911 |
| trit | 0.99997 | 0.1305 | 5,587,886 |
| f32 | **0.99989** | **0.2901** | **5,586,936** |

对照第 0 层 3 段（第 1 项）：级联 cosine 0.3768 / 0.3937 / 0.4419。层数从 3 段增到
18 段后，三值权重的误差几乎完全累积，f32 bias 也只能把 cosine 从 0.13 拉到 0.29。

**结论**：去掉 LayerNorm / GELU / 残差后，线性链的量化误差随层数迅速放大；三值权重
不能直接堆叠成“更大模型”，真实 transformer 的归一化与非线性是必需的。

复算：

```bash
cargo run --release -p trit-wspace --example bias_e2e -- \
  --file model.safetensors --layer 0 --layers 6 --inputs 256 --out bias6.json
```

## 2. GPU：真实 WSP1 权重

```bash
cargo run -p trit-gpu --features gpu --release --example wspace_gpu -- \
  --file query.weight.wsp --inputs 512
```

wf32 内核（三值权重 × f32 激活）与 CPU 标量对拍，最大绝对误差 **~2.5e-5**（f32 累加
顺序差异）。本机 Windows / release 读数：

| 层 | m×k | n | CPU ms | GPU ms | 加速 |
|---|---|---:|---:|---:|---:|
| query | 384×384 | 64 | 3.40 | 3.71 | 0.92× |
| query | 384×384 | 512 | 27.52 | 4.17 | **6.60×** |
| intermediate | 1536×384 | 64 | 14.01 | 3.43 | **4.09×** |

小批量（9.4M MAC）端到端被上传/回读吃掉；75M MAC 以上有 4–6.6×。GPU 时间含建缓冲、
上传、派发与回读，是端到端口径，不是纯内核。

## 3. 依赖边界

`trit-wspace` 与 `trit-gpu` 都是 `publish = false` 的实验层，`publish_graph.py` 禁止
实验层互相依赖。所以 GPU 示例放在 `trit-gpu` 的 **dev-dependencies** 里（发布图允许
dev-deps 跨实验层），不进入 `trit-wspace` 的运行时。

要正式把 GPU 接进 `analyze` / `repair` 的候选评估，需要先把 `trit-gpu` 转可发布（或把
WF32 内核下沉到可发布 crate）；这是发布面决策，未在本 PR 做。

## 4. 未证栏

- 6 层只测线性三段，没有注意力 / LayerNorm / GELU / 残差；**不是**真实 transformer 精度。
- 输入是随机向量，不是真实 tokenizer/激活统计。
- GPU 只测 WF32（三值权重 × f32 激活），不是三值×三值；没有测训练/反向。
- 本机 GPU 读数，不同后端/驱动会变；CI 默认不启用 `gpu` feature，未跑。
- 没有把 GPU 接进 `analyze` / `repair` 的候选评估（依赖边界 + 需要批量 API）。
