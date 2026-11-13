# 编辑集门禁：真实 384 维输入上的「编辑达成率 / 保持集破坏率」

## 为什么要这一层

`certify` 已有 edit/keep 双集合语义，但此前只在 XOR 上验证过：MiniLM `query` 层没有配套任务，
证书只能证明“行为没变”，证明不了“编辑学对了”。这一层补上编辑侧的**真实数字**。

## 为什么不是 STS 检索对

原计划用 STS 文本对做“句向量相似度”任务，但本地 `torch` 导入失败
（`ModuleNotFoundError: No module named 'torch._utils'`），`transformers` 无法跑 MiniLM 前向，
拿不到真实句向量。改用同一个模型里**已有的真实张量**，协议不需要分词与前向：

| 角色 | 取法 |
|---|---|
| 输入 | `embeddings.word_embeddings.weight` 的随机行（384 维真实词向量），按 absmean 三值化 |
| 教师 | f32 `query` 层 × 原 f32 输入，再与 f32 读出向量 r 点积取符号 |
| 学生 | 三值 `query` 层 + 三值读出层（1×384），输出 1 维三值标签 |
| edit 集 | 学生与教师不一致的样本（编辑必须改对） |
| keep 集 | 学生已经正确的样本（编辑必须保持不变） |

读出向量 r 取 f32 `query` 层的第 `--readout-row` 行——是模型的真实权重，可复现；
换行号就是换一个读出方向，用来检验结论不是单点偶然。

## 用法

```bash
cargo run --release -p trit-wspace --example editset_gate -- \
  --safetensors model.safetensors --tuned tuned.safetensors \
  --samples 512 --edit-limit 8 --keep-limit 64 --readout-row 0
```

- `--tuned`：给 f32 微调后的同一张量，走 `delta::diff_weights` 生成三值补丁（只动 `query` 层），
  再用 `certify_nets` 同时判 edit 达成与 keep 保持；不给则只报基线。
- `--edit-limit` / `--keep-limit`：edit/keep 集大小；`--samples`：最多采多少个带标签样本。
- `--out`：报告落盘（JSON）。

## 真实读数

all-MiniLM-L6-v2，`query` 384×384，`word_embeddings` 30522×384，`--samples 512`，seed 7；
`--tuned` 用合成 Δ（每 10 个元素取反，10,128 trits 三值变化）。

| readout-row | 带标签 | 分歧 | edit 前→后 | fixed | keep 破坏 | certified |
|---|---:|---:|---|---:|---:|---|
| 0 | 108 | 44 | 8 → 5 | 3 | 24 / 64 | false |
| 1 | 92 | 28 | 8 → 5 | 3 | 20 / 64 | false |
| 5 | 108 | 44 | 8 → 3 | 5 | 20 / 64 | false |
| 0（零编辑对照） | 108 | 44 | 8 → 8 | 0 | 0 / 64 | false |

读法：

1. **编辑达成率 37.5%–62.5%**（3–5 / 8），而**保持集破坏率 31%–37.5%**（20–24 / 64）——
   同一份 10,128 trits 的编辑，每修好一个 edit 样本平均要付出 4–8 个 keep 样本的代价；
2. 零编辑对照：`changed=0` 时 edit 与 keep 都不动，证书仍然 `false`（edit 未达成）——
   门禁的 edit 侧不是恒真，keep 侧也不会把“没改”记成破坏；
3. 三组 readout 的结论方向一致（修少、破坏多），不是单点偶然。

## 未证栏

- 读出向量是 `query` 层的一行，不是真实的 STS/检索头；任务的语义强度弱于句向量相似度；
- 输入是**单个 token 的词向量**，不是句向量；没有分词、注意力与池化；
- `--tuned` 是合成 Δ（每 10 个元素取反），不是真实 HF 微调；结论只说明“这类编辑不达标”，
  不能推断真实微调的编辑达成率；
- 三值学生与 f32 教师的读出精度不同（三值读出层 vs f32 点积），分歧包含读出量化误差；
- 没有跑 `repair` 的精确最小补丁（384×384 的束搜索太重），“能达成多少”的上界未测。

## 接口

- `examples/editset_gate.rs`：本页协议的唯一实现，输出 `baseline` / `delta` 两段 JSON；
- 复用 `delta::diff_weights`、`certify_nets`（`KeepReport` 的 `fixed` 字段正好是“编辑达成”计数）。
