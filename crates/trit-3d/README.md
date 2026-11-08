# trit-3d — 三值 3D 资产神经组件

全 Rust、三值权重的 3D 资产实验层。目标不是再训练一个大模型，而是把
「体素/真实 mesh → 三值编码 → 3DGS 参数 → 可看资产」这条链拆成可验证的组件。

## 模块

| 文件 | 内容 |
|---|---|
| `src/lib.rs` | `VoxelGrid`（-1/0/+1 体素）、`AttentionEncoder`（三值 AttentionBlock）、`DenseEncoder`（TernaryDense 堆叠）、`VoxelHead`（体素回归） |
| `src/gaussian.rs` | 完整 3DGS 头：`pos / scale / quat / opacity / 1–3 阶 SH`；多视角 SH 颜色监督；TGPU 无关的 PLY 导出（scale log / opacity logit / 法线） |
| `src/mesh.rs` | `TriMesh`（torus / OBJ 读取 / 表面体素化 / 面积加权采样 / 分层尺度 / 薄壁压薄 / 体积） |
| `src/lut.rs` | 三值 LUT 层：解析 trit-build `--emit-gpu` 的 TGPU v1（pos/neg 2-bit 位平面），CPU 查表 + 体素规则后处理 |
| `examples/` | `voxel_ae` / `gaussian_head` / `mesh_asset` / `mesh_gt` / `real_asset` / `mesh_compare` / `lut_layer` |
| `scripts/asset_pipeline.sh` | 一键：OBJ → 体素 → 高斯 → PLY → gauss2mesh → OBJ → UV |

## 实测读数

### 编码器 / 高斯头

| 项 | 读数 |
|---|---|
| 体素 8³ AE（DenseEncoder） | MSE 0.711 → 0.000000，重建 IoU **1.0** |
| 体素 8³ AE（AttentionBlock） | MSE 0.987 → 0.320，IoU **0.41**（负结果：单头/无位置编码/容量不足） |
| 64 高斯、1 阶 SH | 参数 MSE 0.119510 → **0.000242**；Chamfer² 0.194702 → **0.004441** |
| 多视角颜色（8 方向） | 0.047456 → **0.000035**；高阶 SH 绝对值和 **78.68**（非零监督） |
| 解析 torus（512/2048 高斯） | χ=2（128³ surfel），体积 0.6118；GT 2048 的 χ=0 |

### 真实资产（`aa776b2f…obj`，249,951 v / 500,014 f）

| 高斯数 | 训练 MSE | Chamfer²（中心） | 128³ 重建 χ | 体积 |
|---:|---:|---:|---:|---:|
| 512 | 1.66e-4 | — | −995 | 0.532 |
| 2048 | 1.55e-4 | — | −699 | 0.282 |
| **4096** | **1.96e-4** | **0.000933** | **−176** | **0.650** |

4096 高斯的法线点积 **0.9986**；但重建 mesh 对照原始 mesh：
Chamfer² **0.051365**、法线点积 **0.5128**、体积比 **9.04×**（0.650 / 0.072）——
**真实资产重建不合格**，问题在 gauss2mesh 的尺度/覆盖/膨胀与开放薄壁表面，不是高斯头位置。

负结果：分层尺度 + 法向压薄 0.05 让 χ 从 −176 变成 **−468**、体积 0.312；
8192 高斯 debug 训练超 5 分钟中止；分层目标下预测 scale 相对误差 **3.95**。

### 三值 LUT 层（trit-build PR #373）

- 解析 `truth.gpu.bin`（TGPU v1，16 B 头 + pos/neg 位平面）；
- 对 `truth.csv` 27 行逐行对拍：**0 mismatch**；
- 同 LUT 对 8³ cube 做规则后处理：true 484 / false 28 / unknown 0。

### UV（`LSSMJ-mat/crates/conv-core`，GPL 侧）

接缝（二面角 45° + 边界 + 八分体补切 + 小三角形 35° 曲率自适应）→ 每岛 LSCM →
扫描线精确检测三角形重叠 → 局部拆重叠 → shelf 打包。

| mesh | 三角形 | 岛数 | fill | tri_overlaps | 耗时 |
|---|---:|---:|---:|---:|---:|
| 真实资产 64³ | 25,556 | 22,055 | **0.4187** | 0 | 1.8 s |
| 真实资产 128³ | 234,012 | 27,313 | **0.4174** | 0 | 3.6 s |
| 解析 torus | 43,284 | 260 | 0.4845 | 0 | 1.7 s |

负结果：更大分块（grid=3）fill 0.4123；shelf 90° 旋转前后 0.4173 不变；
单三角形 AABB 的 fill 理论上限约 0.5。

## 一键管线

```bash
bash crates/trit-3d/scripts/asset_pipeline.sh \
  "D:/KF/我的/glass-keyboard-20260904/obj源几何/glass_keyboard_obj/aa776b2f9fa456894508b0963d47cc34.obj" \
  "E:/cargo-target/LSSMJ-mat/asset/pipeline" 4096 128
```

四步：训练 4096 高斯 → `gauss2mesh` 128³ → `uvunwrap`（LSCM）→ `asset_uv.obj`。
外部二进制路径可用 `GAUSS2MESH` / `UVUNWRAP` 覆盖。

## 测试

```bash
cargo test -p trit-3d --lib          # 18 passed
cargo clippy -p trit-3d --all-targets -- -D warnings
```

## 三值账（`POLICY.md` §1）

- **A 原生红利**：三值权重/Attention/FFN、三值体素、三值 LUT（TGPU 2 bit/trit）。
- **B 模拟税**：latent、体素坐标、高斯位置/尺度/四元数/颜色/SH 必须 f32；
  三值化会毁掉几何与颜色。
- **C 生态路径依赖**：PLY / OBJ / WGSL 是外部格式，必须按生态约定。
- **D 基数无关**：矩阵乘、LSCM、打包、重叠检测的复杂度与进制无关。

## 边界

- 单样本/小网格为主，不是可泛化的 3D 生成大模型；
- 真实资产重建仍不合格（体积 9×、法线 0.51），不能当交付资产；
- UV 大 mesh 走分块 LSCM，fill 0.42 是“零重叠”下的当前值，不是电影级；
- GPU LUT 路径由 trit-build 的 `truth.wgsl` 负责，本 crate 只做 CPU 参考与体素规则。
