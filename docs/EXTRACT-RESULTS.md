# 提取结果目录（自动生成）

> 由 `scripts/depth_catalogue.py` 从 `runs/extract/*.manifest.json` 生成，**不要手改**。本族只把 **N2 学习式占据补全**接线：清单记语料内容级 SHA-256、模型 SHA、1 cm 体素上的体积 IoU 与网格闭合率；复核器 `parts-extract-verify` 用 `--load-model` 在新进程重跑同一评测，IoU 差 >1e-4 或闭合数不同即判红。N1/N3–N7 仍只有文档读数，**不在本目录**。

## 结果族：S1 提取（N2 占据补全）

| 模型 | 配置 | 语料（场景/文件） | 体积 IoU（均值 / 最低） | 闭合 | 非流形边（债） | 指纹（模型前 12） | 来源 | 时间 |
|---|---|---|---|---|---|---|---|---|
| `sweep-lod-cells-20k-seeds/cell0.04-seed5.occ1` | cell 0.04 m · 20000 步 · 8 视角 | 12 / 300 | 0.5765 / 0.5021 | ✅ 8/8 | 186 | `7916ce660f58` | `4c0f110+dirty` | 2026-11-04 17:08 UTC |
| `sweep-lod-cells-20k-seeds/cell0.04-seed3.occ1` | cell 0.04 m · 20000 步 · 8 视角 | 12 / 300 | 0.6554 / 0.5603 | ✅ 8/8 | 91 | `5c8e0afb0f69` | `4c0f110+dirty` | 2026-11-04 17:07 UTC |
| `sweep-lod-cells-20k-seeds/cell0.04-seed2.occ1` | cell 0.04 m · 20000 步 · 8 视角 | 12 / 300 | 0.3500 / 0.2837 | ✅ 8/8 | 176 | `5d5036b0f445` | `4c0f110+dirty` | 2026-11-04 17:07 UTC |
| `sweep-lod-cells-20k-seeds/cell0.02-seed5.occ1` | cell 0.02 m · 20000 步 · 8 视角 | 12 / 300 | 0.5934 / 0.5174 | ✅ 8/8 | 516 | `13e6b8767399` | `4c0f110+dirty` | 2026-11-04 17:07 UTC |
| `sweep-lod-cells-20k-seeds/cell0.02-seed3.occ1` | cell 0.02 m · 20000 步 · 8 视角 | 12 / 300 | 0.5312 / 0.4484 | ✅ 8/8 | 734 | `3c2a5f10b66a` | `4c0f110+dirty` | 2026-11-04 17:07 UTC |
| `sweep-lod-cells-20k-seeds/cell0.02-seed2.occ1` | cell 0.02 m · 20000 步 · 8 视角 | 12 / 300 | 0.6826 / 0.6128 | ✅ 8/8 | 308 | `a9a823c977b5` | `4c0f110+dirty` | 2026-11-04 17:07 UTC |
| `sweep-lod-cells-seeds/cell0.04-seed5.occ1` | cell 0.04 m · 5000 步 · 8 视角 | 12 / 300 | 0.5640 / 0.4953 | ✅ 8/8 | 129 | `6168f0bb310d` | `4c0f110+dirty` | 2026-11-04 17:04 UTC |
| `sweep-lod-cells-seeds/cell0.04-seed3.occ1` | cell 0.04 m · 5000 步 · 8 视角 | 12 / 300 | 0.5222 / 0.4505 | ✅ 8/8 | 167 | `80ebcbe2d274` | `4c0f110+dirty` | 2026-11-04 17:04 UTC |
| `sweep-lod-cells-seeds/cell0.04-seed2.occ1` | cell 0.04 m · 5000 步 · 8 视角 | 12 / 300 | 0.7287 / 0.6474 | ✅ 8/8 | 92 | `c6592ced9846` | `4c0f110+dirty` | 2026-11-04 17:04 UTC |
| `sweep-lod-cells-seeds/cell0.02-seed5.occ1` | cell 0.02 m · 5000 步 · 8 视角 | 12 / 300 | 0.6218 / 0.5658 | ✅ 8/8 | 427 | `636f4b6169c9` | `4c0f110+dirty` | 2026-11-04 17:04 UTC |
| `sweep-lod-cells-seeds/cell0.02-seed3.occ1` | cell 0.02 m · 5000 步 · 8 视角 | 12 / 300 | 0.4005 / 0.3215 | ✅ 8/8 | 572 | `45340764b767` | `4c0f110+dirty` | 2026-11-04 17:03 UTC |
| `sweep-lod-cells-seeds/cell0.02-seed2.occ1` | cell 0.02 m · 5000 步 · 8 视角 | 12 / 300 | 0.6532 / 0.5828 | ✅ 8/8 | 240 | `7863082fb938` | `4c0f110+dirty` | 2026-11-04 17:03 UTC |
| `sweep-selfsup-seeds/seed5-tau0.02.occ1` | cell 0.01 m · 5000 步 · 8 视角 · selfsup τ0.02 | 12 / 300 | 0.6600 / 0.5841 | ✅ 8/8 | 1506 | `605702fd9f29` | `4c0f110+dirty` | 2026-11-04 17:02 UTC |
| `sweep-selfsup-seeds/seed5-tau0.01.occ1` | cell 0.01 m · 5000 步 · 8 视角 · selfsup τ0.01 | 12 / 300 | 0.6536 / 0.5727 | ✅ 8/8 | 1523 | `2faf286ff1c5` | `4c0f110+dirty` | 2026-11-04 17:01 UTC |
| `sweep-selfsup-seeds/seed3-tau0.02.occ1` | cell 0.01 m · 5000 步 · 8 视角 · selfsup τ0.02 | 12 / 300 | 0.6235 / 0.5356 | ✅ 8/8 | 2769 | `b9f407077757` | `4c0f110+dirty` | 2026-11-04 17:00 UTC |
| `sweep-selfsup-seeds/seed3-tau0.01.occ1` | cell 0.01 m · 5000 步 · 8 视角 · selfsup τ0.01 | 12 / 300 | 0.5917 / 0.5118 | ✅ 8/8 | 2211 | `c7b41d05a14d` | `4c0f110+dirty` | 2026-11-04 16:59 UTC |
| `sweep-selfsup-seeds/seed2-tau0.02.occ1` | cell 0.01 m · 5000 步 · 8 视角 · selfsup τ0.02 | 12 / 300 | 0.7460 / 0.6820 | ✅ 8/8 | 1116 | `a5464fad584c` | `4c0f110+dirty` | 2026-11-04 16:58 UTC |
| `sweep-selfsup-seeds/seed2-tau0.01.occ1` | cell 0.01 m · 5000 步 · 8 视角 · selfsup τ0.01 | 12 / 300 | 0.7430 / 0.6862 | ✅ 8/8 | 1074 | `39e3f86c6086` | `4c0f110+dirty` | 2026-11-04 16:57 UTC |
| `sweep-1cm-seeds/seed5-steps20000.occ1` | cell 0.01 m · 20000 步 · 8 视角 | 12 / 300 | 0.5661 / 0.4842 | ✅ 8/8 | 1693 | `5e523dd35e00` | `241359f+dirty` | 2026-11-04 13:49 UTC |
| `sweep-1cm-seeds/seed5-steps10000.occ1` | cell 0.01 m · 10000 步 · 8 视角 | 12 / 300 | 0.5150 / 0.4521 | ✅ 8/8 | 1734 | `14f6a85c21a9` | `241359f+dirty` | 2026-11-04 13:47 UTC |
| `sweep-1cm-seeds/seed5-steps5000.occ1` | cell 0.01 m · 5000 步 · 8 视角 | 12 / 300 | 0.6363 / 0.5573 | ✅ 8/8 | 1957 | `b242335bfd68` | `241359f+dirty` | 2026-11-04 13:46 UTC |
| `sweep-1cm-seeds/seed3-steps20000.occ1` | cell 0.01 m · 20000 步 · 8 视角 | 12 / 300 | 0.6570 / 0.5914 | ✅ 8/8 | 1606 | `ecf1fdfe4162` | `241359f+dirty` | 2026-11-04 13:44 UTC |
| `sweep-1cm-seeds/seed3-steps10000.occ1` | cell 0.01 m · 10000 步 · 8 视角 | 12 / 300 | 0.5709 / 0.4994 | ✅ 8/8 | 2069 | `e9b4bd190685` | `241359f+dirty` | 2026-11-04 13:43 UTC |
| `sweep-1cm-seeds/seed3-steps5000.occ1` | cell 0.01 m · 5000 步 · 8 视角 | 12 / 300 | 0.5912 / 0.5088 | ✅ 8/8 | 2281 | `504fd3a7755f` | `241359f+dirty` | 2026-11-04 13:42 UTC |
| `sweep-1cm-seeds/seed2-steps20000.occ1` | cell 0.01 m · 20000 步 · 8 视角 | 12 / 300 | 0.5819 / 0.5075 | ✅ 8/8 | 1377 | `2c288196509a` | `241359f+dirty` | 2026-11-04 13:40 UTC |
| `sweep-1cm-seeds/seed2-steps10000.occ1` | cell 0.01 m · 10000 步 · 8 视角 | 12 / 300 | 0.5234 / 0.4406 | ✅ 8/8 | 1494 | `bcedca3c98b5` | `241359f+dirty` | 2026-11-04 13:39 UTC |
| `sweep-1cm-seeds/seed2-steps5000.occ1` | cell 0.01 m · 5000 步 · 8 视角 | 12 / 300 | 0.6643 / 0.5906 | ✅ 8/8 | 1156 | `1b6f8014829f` | `241359f+dirty` | 2026-11-04 13:38 UTC |
| `sweep-lod-40k/cell0.02-steps40000.occ1` | cell 0.02 m · 40000 步 · 8 视角 | 12 / 300 | 0.6092 / 0.5354 | ✅ 8/8 | 666 | `48bb00ab13ca` | `eaa071b+dirty` | 2026-11-04 12:33 UTC |
| `sweep-lod-budget/cell0.04.occ1` | cell 0.04 m · 20000 步 · 8 视角 | 12 / 300 | 0.5101 / 0.4152 | ✅ 8/8 | 175 | `e0b16a2a038c` | `c9af11c+dirty` | 2026-11-04 11:53 UTC |
| `sweep-lod-budget/cell0.02.occ1` | cell 0.02 m · 20000 步 · 8 视角 | 12 / 300 | 0.6631 / 0.5842 | ✅ 8/8 | 800 | `5f67e82337a2` | `c9af11c+dirty` | 2026-11-04 11:53 UTC |
| `sweep-lod/cell0.04.occ1` | cell 0.04 m · 5000 步 · 8 视角 | 12 / 300 | 0.6370 / 0.5613 | ✅ 8/8 | 81 | `2b6c9ec307be` | `5dfbb8a+dirty` | 2026-11-04 11:10 UTC |
| `sweep-lod/cell0.02.occ1` | cell 0.02 m · 5000 步 · 8 视角 | 12 / 300 | 0.5094 / 0.4400 | ✅ 8/8 | 393 | `b21d255806d6` | `5dfbb8a+dirty` | 2026-11-04 11:10 UTC |
| `sweep-lod/cell0.01.occ1` | cell 0.01 m · 5000 步 · 8 视角 | 12 / 300 | 0.7035 / 0.6249 | ✅ 8/8 | 1504 | `bdd078bace72` | `5dfbb8a+dirty` | 2026-11-04 11:09 UTC |
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

