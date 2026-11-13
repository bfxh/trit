# 真句向量上的编辑集门禁（STS 风格 pair 判定）

## 为什么补这一层

`editset_gate` 的输入是 `word_embeddings` 的单 token 行——真实但不是句向量。
装上 `torch` / `transformers` 后，可以用本地 all-MiniLM-L6-v2 真正编码句子，
把这层未证项补掉一部分。

## 协议

| 角色 | 取法 |
|---|---|
| 输入 | `transformers` 编码出的 384 维句向量（`encode_sts.py` 生成 pairs JSON） |
| 教师 | 全精度句向量 `cos(a, b) >= threshold` |
| 学生 | 句向量 absmean 三值化 → 三值 `query` 层 → 两个 384 维输出取余弦 |
| edit 集 | 学生当前判错的对（编辑必须改对） |
| keep 集 | 学生当前判对的对（编辑必须保持不变） |

与 `editset_gate` 的差别：判定是**两个句向量的余弦**，不再依赖“`query` 层某一行”当读出；
但学生仍然只有单层 `query`，不是端到端模型。

### 生成 pairs

```bash
# 隔离 venv，避免污染全局 Python
python -m venv $TMPDIR/sts-venv
$TMPDIR/sts-venv/Scripts/python.exe -m pip install torch transformers
$TMPDIR/sts-venv/Scripts/python.exe encode_sts.py   # 输出 $TMPDIR/sts-pairs.json
```

`encode_sts.py` 用 16 个手写句子（8 组同义改写 + 跨主题对照）取全部 `i<j` 组合，
共 120 对；句向量做 mean pooling + L2 归一化。

### 跑门禁

```bash
cargo run --release -p trit-wspace --example editset_sts -- \
  --pairs sts-pairs.json --safetensors model.safetensors \
  --tuned tuned.safetensors --threshold 0.5
```

## 真实读数

all-MiniLM-L6-v2，16 句 / 120 对，`--tuned` = 合成 Δ（10,128 trits）：

| threshold | 教师正对 | 基线 对/错 | 补丁后 edit_fixed | 补丁后 keep_broken | 补丁后 对/错 |
|---|---:|---|---:|---:|---|
| 0.3 | 8 | 120 / 0 | 0 | 0 | 120 / 0 |
| 0.5 | 8 | 118 / 2 | 0 | 0 | 118 / 2 |
| 0.8 | 5 | 115 / 5 | 0 | 0 | 115 / 5 |

读法：

1. **三值学生在手写句集上接近饱和**（115–120 / 120 正确），edit 集只有 0–5 个样本——
   这个句集太容易，三值化 + 单层 `query` 已经够区分同义改写；
2. 合成 Δ 在任何阈值下都 **修 0、破坏 0**：10,128 trits 的改动没有翻转这 120 对的判定——
   与 `editset_gate`（词向量行输入、破坏 24/64）不同，句向量输入的余弦对这些坐标不敏感；
3. 两个数字说明的是**同一件事**：当前协议下编辑既改不动、也破坏不了，缺的是“更难的任务”。

## 未证栏

- 句集只有 16 个手写句子（8 组改写），不是 STS benchmark；阈值 0.3/0.5/0.8 是人工选的；
- 学生仍是单层三值 `query`，没有 attention / 池化 / 多层；不是端到端 STS；
- `--tuned` 是合成 Δ（每 10 元素取反），不是真实 HF 微调；
- 三值学生接近饱和 → edit 集太小，统计意义弱；需要更难的对（细粒度语义、否定、长句）
  才能让门禁有区分度；
- 没有跑 `--keep`/`--edit` 的 `certify` 证书（pair 判定不是单输出，Oracle 不适用），
  只报了 edit_fixed / keep_broken 两个数。

## 接口

- `examples/editset_sts.rs`：读 pairs JSON、构造教师/学生判定、跑 delta 补丁前后统计；
- `encode_sts.py`（会话临时目录，不入库）：用本地模型生成 pairs JSON 的参考实现。
