# 部件头结果目录（自动生成）

> 由 `scripts/depth_catalogue.py` 从 `runs/parts/*.manifest.json` 生成，**不要手改**。每行的「复核」来自独立进程 `parts-verify`：它重读训练集与评测语料、重算内容级 SHA-256、重跑 `parts-inst eval`；指纹或模型与清单不一致会判红（写 `.verify.reject.json`）。

## 结果族：部件平面头 / 分割与实例

| 模型 | 配置 | 训练数据（场景/帧） | 训练 loss | 留出复核（帧 / mIoU / 前景 IoU / 边界 F1） | 指纹（模型前 12） | 来源 | 时间 |
|---|---|---|---|---|---|---|---|
| `pb50-6k-a1/model.tpm` | 384² · k5 · pbnd · bnd×4 · lr 0.001 · 6000 步 | 50 / 400 | 0.0086 | ✅ 80 帧 / 0.3419 / 0.964 / 0.406 | `75c0f8d93628` | `65216f8+dirty` | 2026-11-03 22:03 UTC |
| `pb50-6k/model.tpm` | 384² · k5 · pbnd · bnd×4 · accum 4 · lr 0.001 · 6000 步 | 50 / 400 | 0.0546 | ✅ 80 帧 / 0.1558 / 0.842 / 0.237 | `b7f7820a1fc6` | `65216f8+dirty` | 2026-11-03 21:18 UTC |

## 如何复核一行

```sh
parts-verify --model <模型.tpm> --data <训练时的场景根目录列表> \
  --seg-sub <seg/pbnd> --renders <留出 renders> --scenes <留出 scenes> \
  --eval-args "--gt part --boundary --k 5 --res 384"
```

复核器会重算 `sha256(模型)`、训练集与评测语料的内容级指纹，与 `<模型>.manifest.json` 比对；一致才写 `.verify.json`，否则写 `.verify.reject.json` 并退出码 1。

