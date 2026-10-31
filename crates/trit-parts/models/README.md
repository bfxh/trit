# models/ —— 交付权重

`parts_id8118.tpm` 是当前的部件平面头交付包（`TPM1`，13 层，466,272 个三值权重，head 已裁到 16 通道）：

| 项 | 值 |
|---|---|
| 文件 / 大小 | `parts_id8118.tpm` / 466,272 个三值权重（head 已裁到 16 通道） |
| SHA-256 | `7716ba9e819f57bc3ad0312c41f5f4bf970142e995ebe3b5f49319b600611bea` |
| 训练来源 | 本仓 `parts-train`，12 个距离（2.0–4.2）× 150 帧，384²，Adam，约 88k 样本 |
| 留出（r3.6 / r3.9，各 60 帧） | id **0.8118 / 0.7653**，部件 IoU **0.6609 / 0.6142** |
| 3D（GT 深度反投影，10 mm 体素） | 逐部件 IoU **0.6397**，质心误差 30.0 mm |
| 适用边界 | 只对训练过的装配摆放有效；换装配需重训，且 id 必须沿结构密集编号（见 `docs/MODEL-CARD.md`）|
| 取景带 | `parts_id8118.framing`（TPF1，两条统计的接受区间）|
| ⚠️ 域外行为 | 对**任何**图像都会把 96–99% 像素判成部件（域内只有 1.7%），**没有 OOD 拒识**；生产必须外挂「是不是这个物体」的闸门 |
| ⚠️ 拒识 | **没有**。域外图像会被标成 96–99% 部件（见模型卡 §4c），管线需自己加检测门槛 |

用法（一条命令：闸门 → 部件 id 图 → 可选点云）：

```bash
# 一条命令：闸门 + id 图 (+ 点云)
parts-cli pipeline --model crates/trit-parts/models/parts_id8118.tpm   --manifest parts.json --bank crates/trit-parts/models/parts_id8118.ood   --framing crates/trit-parts/models/parts_id8118.framing   --image photo.png --out out/ [--depth d.npy --meta m.json]
#   域外 ⇒ 退出码 4；取景不可用 ⇒ 退出码 5；两者都不写输出（--force 可强制）

# 只做域外闸门（域外返回退出码 4）
parts-ood check --model crates/trit-parts/models/parts_id8118.tpm   --manifest parts.json --bank crates/trit-parts/models/parts_id8118.ood --image photo.png

# 单帧 id 图
parts-cli run --model crates/trit-parts/models/parts_id8118.tpm --image rgb.png \
              --manifest parts.json --out out/

# 部件点云（深度由外部提供；本模型不预测深度）
parts-3d infer --model crates/trit-parts/models/parts_id8118.tpm --scene <dir> --frame 0 \
               --manifest parts.json --out parts.ply
```

指标口径与负结果见仓库根 `docs/MODEL-CARD.md`。
