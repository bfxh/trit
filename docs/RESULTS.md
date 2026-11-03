# 结果目录（自动生成）

> 由 `scripts/depth_catalogue.py` 生成，**不要手改**。只有 `live` 的族有机器可读清单 + 独立复核；`registered` 的族只登记协议与证据位置，文档读数**不算复核**。

## 已接线（live）

- **单目深度头**（live）：清单 + 独立复核已接线；完整表见 `docs/DEPTH-RESULTS.md`

  本次扫描：9 条运行，9 条通过独立复核。

## 已登记、产物未接（registered）

- **部件平面头 / 分割与实例**（registered，产物未接）：parts-train 写清单（commit / config / 数据指纹 / 模型 SHA），并把 `parts-inst eval` 包成 parts-verify 做独立复核
  - 文档证据：GENERAL-PLAN §6.96 / §6.99（文档读数：mIoU 0.4962、边界 F1 0.432、3D 部件 IoU 0.5177、计数±1 36.2%——**非本目录复核**）
- **纹理**（registered，产物未接）：先定义可复核的产物（ttx 包 + 采样一致性指标）与留出协议
  - 文档证据：docs/TTX.md（协议草案，无机器可读清单）
- **S1 提取（场景/文字→3D）**（registered，产物未接）：S1 / N1–N7 门的机器可读读数 + 每场景资产清单（2,400 帧见 A108）
  - 文档证据：GENERAL-PLAN 通用提取各节 / LEDGER A108（**非本目录复核**）

