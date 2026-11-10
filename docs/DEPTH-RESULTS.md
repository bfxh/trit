# 深度头结果目录（自动生成）

> 由 `scripts/depth_catalogue.py` 从 `runs/depth/*.manifest.json` 生成，**不要手改**。每行的「复核」来自独立进程 `parts-depth-verify`：它重读数据、重算模型/数据指纹（数据是**内容级** SHA-256）并在留出集上重跑评测；指纹与清单不一致会判红。

## 结果族：单目深度头

| 模型 | 配置 | 数据（目录/文件） | 训练 MSE | 留出复核（形状对齐 / δ<1.25 / 常数基线） | 晋级 | 指纹（模型前 12） | 来源 | 时间 |
|---|---|---|---|---|---|---|---|---|
| `hidden128-lr0.001-dil2.dcn` | h128 · dil 2 · accum 8 · lr 0.001 · 3 ep | 60 / 720 | 1.0254 | ✅ 9.71% / 0.919 / 7.91% | — | `6239de027ba5` | `f881cac+dirty` | 2026-11-03 17:36 UTC |
| `hidden128-lr0.002-dil2.dcn` | h128 · dil 2 · accum 8 · lr 0.002 · 3 ep | 60 / 720 | 0.9146 | ✅ 8.38% / 0.931 / 7.91% | ✓ | `9b1a5ae5a917` | `f881cac+dirty` | 2026-11-03 17:36 UTC |
| `hidden64-lr0.001-dil2.dcn` | h64 · dil 2 · accum 8 · lr 0.001 · 3 ep | 60 / 720 | 0.9495 | ✅ 9.01% / 0.927 / 7.91% | — | `a71e8b249891` | `f881cac+dirty` | 2026-11-03 17:35 UTC |
| `hidden64-lr0.002-dil2.dcn` | h64 · dil 2 · accum 8 · lr 0.002 · 3 ep | 60 / 720 | 0.9215 | ✅ 8.85% / 0.931 / 7.91% | — | `7f5994534c6e` | `f881cac+dirty` | 2026-11-03 17:35 UTC |
| `hidden32-lr0.001-dil2.dcn` | h32 · dil 2 · accum 8 · lr 0.001 · 3 ep | 60 / 720 | 0.9436 | ✅ 8.73% / 0.931 / 7.91% | — | `9ee20496c8c8` | `f881cac+dirty` | 2026-11-03 17:35 UTC |
| `hidden32-lr0.002-dil2.dcn` | h32 · dil 2 · accum 8 · lr 0.002 · 3 ep | 60 / 720 | 0.9160 | ✅ 8.53% / 0.933 / 7.91% | ✓ | `e552d505a7f6` | `f881cac+dirty` | 2026-11-03 17:35 UTC |
| `demo_h128.dcn` | h128 · dil 2 · accum 8 · lr 0.002 · 3 ep | 60 / 720 | 0.9146 | ✅ 8.38% / 0.931 / 7.91% | — | `9b1a5ae5a917` | `fff3840+dirty` | 2026-11-03 15:17 UTC |
| `demo_h64.dcn` | h64 · dil 2 · accum 8 · lr 0.002 · 3 ep | 60 / 720 | 0.9215 | ✅ 8.85% / 0.931 / 7.91% | — | `7f5994534c6e` | `fff3840+dirty` | 2026-11-03 15:17 UTC |
| `demo_h32.dcn` | h32 · dil 2 · accum 8 · lr 0.002 · 3 ep | 60 / 720 | 0.9160 | ✅ 8.53% / 0.933 / 7.91% | — | `e552d505a7f6` | `fff3840+dirty` | 2026-11-03 15:17 UTC |

## 如何复核一行

```sh
parts-depth-verify --dcn <模型> --data <训练时的 --data 列表> \
  --holdout <清单里的 holdout> --size <config.size>
```

复核器会重算 `sha256(model)` 与数据集指纹，与 `<模型>.manifest.json` 比对；一致才写 `.verify.json`，不一致写 `.verify.reject.json` 并退出码 1。

