# 提取结果目录（自动生成）

> 由 `scripts/depth_catalogue.py` 从 `runs/extract/*.manifest.json` 生成，**不要手改**。本族只把 **N2 学习式占据补全**接线：清单记语料内容级 SHA-256、模型 SHA、1 cm 体素上的体积 IoU 与网格闭合率；复核器 `parts-extract-verify` 用 `--load-model` 在新进程重跑同一评测，IoU 差 >1e-4 或闭合数不同即判红。N1/N3–N7 仍只有文档读数，**不在本目录**。

## 结果族：S1 提取（N2 占据补全）

| 模型 | 配置 | 语料（场景/文件） | 体积 IoU（均值 / 最低） | 闭合 | 非流形边（债） | 指纹（模型前 12） | 来源 | 时间 |
|---|---|---|---|---|---|---|---|---|
| `sweep-selfsup/tau0.02.occ1` | cell 0.01 m · 5000 步 · 8 视角 · selfsup τ0.02 | 12 / 300 | 0.4945 / 0.4395 | ✅ 8/8 | 976 | `0227c23b63c4` | `0b6fc42+dirty` | 2026-11-04 10:42 UTC |
| `sweep-selfsup/tau0.01.occ1` | cell 0.01 m · 5000 步 · 8 视角 · selfsup τ0.01 | 12 / 300 | 0.5432 / 0.4745 | ✅ 8/8 | 1095 | `ffa2eb521cb8` | `0b6fc42+dirty` | 2026-11-04 10:41 UTC |
| `sweep-steps/steps20000.occ1` | cell 0.01 m · 20000 步 · 8 视角 | 12 / 300 | 0.5535 / 0.4635 | ✅ 8/8 | 2083 | `9f05847034c6` | `d0ccaee+dirty` | 2026-11-03 23:40 UTC |
| `sweep-steps/steps10000.occ1` | cell 0.01 m · 10000 步 · 8 视角 | 12 / 300 | 0.6981 / 0.6267 | ✅ 8/8 | 1613 | `3bf5408ecb45` | `d0ccaee+dirty` | 2026-11-03 23:38 UTC |
| `sweep-steps/steps5000.occ1` | cell 0.01 m · 5000 步 · 8 视角 | 12 / 300 | 0.7035 / 0.6249 | ✅ 8/8 | 1504 | `bdd078bace72` | `d0ccaee+dirty` | 2026-11-03 23:37 UTC |
| `sweep-steps/steps2500.occ1` | cell 0.01 m · 2500 步 · 8 视角 | 12 / 300 | 0.6858 / 0.5971 | ✅ 8/8 | 1075 | `3ef752ea61ec` | `d0ccaee+dirty` | 2026-11-03 23:36 UTC |
| `extract/n2.occ1` | cell 0.01 m · 20000 步 · 8 视角 | 12 / 300 | 0.5535 / 0.4635 | ✅ 8/8 | 2083 | `9f05847034c6` | `5b1ba6d` | 2026-11-03 23:09 UTC |

## 如何复核一行

```sh
parts-extract-verify --model <n2.occ1>   # 清单缺省 <模型>.manifest.json
```

