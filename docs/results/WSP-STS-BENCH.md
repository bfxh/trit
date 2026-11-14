# 真实 STS benchmark 上的编辑集门禁

## 数据

`mteb/stsbenchmark-sts` 的 `test` split（1,379 行，官方人工相似度 0–5）。二值化按官方约定：

- `score >= 4` → positive
- `score <= 2` → negative
- `score == 3` → 丢弃（中性对）

取前 400 对（132 正 / 268 负，丢弃 507 个中性对），用本地 all-MiniLM-L6-v2 编码句向量。
协议与 `editset_sts` 完全一致：教师 = 人工标签，学生 = 三值化句向量 → 三值 `query` 层 →
两个 384 维输出取余弦，阈值判定。

```bash
# 隔离 venv（torch / transformers / datasets）
HF_ENDPOINT=https://hf-mirror.com python scripts/encode_sts_bench.py \
  --model <本地 MiniLM 快照> --out sts-bench.json --limit 400
cargo run --release -p trit-wspace --example editset_sts -- \
  --pairs sts-bench.json --safetensors model.safetensors \
  --tuned tuned.safetensors --threshold 0.5
```

> 直连 `huggingface.co` 下载数据集时会报 `missing the 'X-Repo-Commit' header`（代理返回空体）；
> 用 `HF_ENDPOINT=https://hf-mirror.com` 才能拿到文件。

## 真实读数

400 对，`--tuned` = 合成 Δ（10,128 trits）：

| threshold | 基线 对/错 | edit 集 | 补丁后 edit_fixed | 补丁后 keep_broken | 补丁后 对/错 |
|---|---:|---:|---:|---:|---|
| 0.3 | 369 / 31 | 31 | 0 | 0 | 369 / 31 |
| 0.5 | 372 / 28 | 28 | 0 | **4** | 368 / 32 |
| 0.8 | 270 / 130 | 130 | 3 | 0 | 273 / 127 |

读法：

1. **低阈值下三值学生已经 92%–93% 正确**（369–372 / 400），edit 集只有 28–31；
   阈值提到 0.8 时掉到 67.5%（270 / 400），edit 集涨到 130——阈值是这套门禁最敏感的旋钮；
2. **首次出现 keep 被破坏**：0.5 阈值下，合成 Δ 破坏 4 个原本正确的对（`keep_broken=4`），
   而之前的手写句集全是 0——真实 benchmark 上“编辑有代价”第一次可测；
3. 修正能力仍然很弱：`edit_fixed` 只在 0.8 阈值下出现 3 个（130 个里修 2.3%）。

## 与手写集的对照

| 数据集 | 对数 | 三值学生准确率 | edit 集 | edit_fixed | keep_broken |
|---|---:|---|---:|---:|---:|
| 手写同义改写 | 120 | 96%–100% | 0–5 | 0 | 0 |
| 手写细粒度/否定（85 对） | 85 | 55%–69% | 26–45 | 1–6 | 0 |
| STS benchmark（400 对） | 400 | 67.5%–93% | 28–130 | 0–3 | 0–4 |

真实 benchmark 落在两者之间：比同义改写难、比细粒度手写集容易；它是第一个让
`keep_broken` 非零的数据集。

## 未证栏

- 只取前 400 对（benchmark 有 1,379 行），不是完整测试集；
- 二值化阈值（4 / 2）是官方约定，丢弃了 507 个中性对；
- `--tuned` 仍是合成 Δ（每 10 元素取反），不是真实 HF 微调；`edit_fixed` 只能说明“这类编辑弱”，
  不能外推真实微调；
- 学生仍是单层三值 `query`，不是端到端 STS；句向量来自 MiniLM 全精度前向，只压缩了 `query` 层；
- 没有跑多 seed / 多子集，400 对的抽样顺序未随机化。

## 接口

- `scripts/encode_sts_bench.py`：从 `mteb/stsbenchmark-sts` 生成 pairs JSON（官方 score 二值化）；
- `examples/editset_sts.rs`：读 pairs JSON、构造教师/学生判定、跑 delta 补丁前后统计。
