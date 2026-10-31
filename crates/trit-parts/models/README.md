# models/ —— 交付权重

`parts_id8118.tpm` 是当前的部件平面头交付包（`TPM1`，13 层，466,656 个三值权重）：

| 项 | 值 |
|---|---|
| 文件 / 大小 | `parts_id8118.tpm` / 479,208 B |
| SHA-256 | `c83bb4e5997975cfe44df2fdc80492c431c69b4b341348e6485d48e33307c5b8` |
| 训练来源 | 本仓 `parts-train`，12 个距离（2.0–4.2）× 150 帧，384²，Adam，约 88k 样本 |
| 留出（r3.6 / r3.9，各 60 帧） | id **0.8118 / 0.7653**，部件 IoU **0.6609 / 0.6142** |
| 3D（GT 深度反投影，10 mm 体素） | 逐部件 IoU **0.6397**，质心误差 30.0 mm |
| 适用边界 | 只对训练过的装配摆放有效；换装配需重训，且 id 必须沿结构密集编号（见 `docs/MODEL-CARD.md`）|

用法：

```bash
# 单帧 id 图
parts-cli run --model crates/trit-parts/models/parts_id8118.tpm --image rgb.png \
              --manifest parts.json --out out/

# 部件点云（深度由外部提供；本模型不预测深度）
parts-3d infer --model crates/trit-parts/models/parts_id8118.tpm --scene <dir> --frame 0 \
               --manifest parts.json --out parts.ply
```

指标口径与负结果见仓库根 `docs/MODEL-CARD.md`。
