# trit-parts —— 部件平面头（Rust 三进制 GPU）

`publish = false`，实验层，不属于稳定 API。

按 `D:\KF\AGENTS.md` 的死命令：**模型归本仓，训练与推理一律 Rust 三进制**。
本 crate 是这条规矩的落点——权重加载、BN 折叠、WGSL 前向都在这里，
不再依赖 `.yanagent/` 下的实验目录。

交付模型的能力、指标、局限与负结果见 **[`docs/MODEL-CARD.md`](../../docs/MODEL-CARD.md)**（含换装配的 id 编号约束）。

## 状态（诚实缺口表）

| 能力 | 状态 | 判据 |
|---|---|---|
| 权重加载（BN 折叠进权重与 bias） | ✅ | 与 Python 逐层对拍 |
| WGSL conv 3×3 / 1×1（三值 + f32 双模） | ✅ | `mid_a/b/c` 差 ≤ 2.3e-5 |
| WGSL 2×2 最大池化 / 通道拼接 / ConvT(k=2,s=2) | ✅ | 同上 |
| **f32 前向整体** | ✅ | `max|Δlogit| = 2.6e-4`，平面 argmax 100%（737280/737280） |
| **三值前向整体** | ✅ | vs PyTorch 同套量化 argmax 0.99969（**全像素口径**） |
| 三值在**部件像素**上的判读 | ⚠️ | 平面准确率 **0.28–0.52**（f32 是 0.74–0.91）⇒ 事后三值化是崩的，必须 QAT |
| 反向内核（conv/ConvT 权重与输入梯度、池化、拼接） | ✅ | GPU↔CPU（f64）max rel **≤ 2.1e-6**；3 个变异全判红 |
| STE：`E → latent / scale` | ✅ | 与 `trit-learn`（`trit-train` 的再导出）三种 STE 逐位一致（单步更新对拍） |
| 训练循环（CE(5 平面) + BCE(valid) + Adam/SGD + 累加 + 断点） | ✅ | 4 帧过拟合：loss 0.70→0.024、valid 0.04→0.997；断点往返逐字节一致 |
| **训练出交付模型** | ✅ | 留出 300 帧：**id 0.7580 / IoU 0.5884**（约束解码 0.7585/0.5889）；Python f32 基线 0.7097/0.4343 |
| 部署包 + 单帧推理 | ✅ | `TPM1` 467 KB（q/scale/γ/β/bias）≈ state 的 1/16；`parts-cli export/run/batch`，清单外 id 0 |
| **跨装配布局泛化** | ❌ | 零样本 id 0.0176 ⇒ 换布局必须重训。⚠️ 但「随机置换 id」是最坏情况：8k 同步对照 A **0.196** / B 随机 id **0.088** / B 按 z 序重编号 **0.130**（+47%）⇒ 新装配**保持 id 沿结构密集编号**时效率接近原布局 |
| 清单约束 argmax 解码 | ✅ | 在清单上取 `Σ logit[i][digit_i]` 最大 ⇒ 结构上不可能输出清单外 id（单测钉住） |
| 评估（无约束 vs 约束并排） | ✅ | `parts-train --eval --manifest <parts.json>`：id 一致 / 部件 IoU / 清单外 id 率 |

> ⚠️ 约束解码的**收益**要等训练出正式模型才能量：当前 ckpt 是坏的三值基线，
> 它只预测 10 种 id（恰好都在清单里），所以两组指标相同。机制本身由单测钉住
> （构造「无约束 = 清单外 id 8」，约束返回合法 id）。

> ⚠️ **口径更正（2026-10-29）**：此前写的「三值 vs f32 argmax 0.9871，只掉 1.3%」是被 98% 背景像素
> 灌水的结论。只看 GT 部件像素时，三值 ckpt 的平面准确率是 0.28–0.52，而 f32 是 0.74–0.91。
> 这正是 QAT 存在的理由；训练循环已能把它拉回来（见过拟合读数）。

> 前向对拍的权重来自既有 ckpt（`D:\KF\trit-data\parts-baseline\`），只作**对齐基线**；
> 交付模型已改由本仓的 Rust 三进制训练产出（`_keep/best_id7580.bin` → `best_id7580.tpm`）。
> ⚠️ 模型的适用范围是**单一装配摆放**：换一套部件坐标就得重训（见上表「跨装配布局泛化」）。

## 跑法

```bash
# 夹具不在 git 里（22 MB）：D:\KF\trit-data\parts-parity
CARGO_TARGET_DIR=E:/cargo-target/trit-parts cargo build -p trit-parts --features gpu --release

# 默认目录是 ../trit-data/parts-parity；也可用 --dir 或 TRIT_PARTS_DIR 指定
E:/cargo-target/trit-parts/release/parts-parity --mode f32
E:/cargo-target/trit-parts/release/parts-parity --mode trit

# 反向内核对拍（GPU vs f64 CPU 参考，10 项；任一 > 2e-3 判红）
cargo run -p trit-parts --features gpu --bin parts-gradcheck

# 整网前向自检（对 parity 夹具，应得 0.999688）
cargo run -p trit-parts --features gpu --bin parts-train -- --selfcheck D:/KF/trit-data/parts-parity

# 训练（过拟合自检：4 帧 150 步）
cargo run -p trit-parts --features gpu --bin parts-train -- --data <场景目录> \
  --init D:/KF/trit-data/parts-parity --limit 4 --steps 150 --lr 1e-3 --report 25

# 评估（无约束 / 清单约束并排）
cargo run -p trit-parts --features gpu --bin parts-train -- --eval <场景目录> \
  --manifest <parts.json> --resume <state.bin>
```

夹具文件：

| 文件 | 内容 |
|---|---|
| `weights.bin` + `weights.json` | ckpt 全部 f32 张量（66 个）+ 名字/偏移/形状索引 |
| `input.bin` | 一帧 384²×3 RGB（NHWC，`uint8/255`） |
| `logits_py.bin` | 同帧 PyTorch f32 前向输出（CHW，16×384×384） |
| `logits_trit_py.bin` | 同上，但权重先按每通道 `mean|w|` 三值化 |

## 实现要点

- 激活统一 **NHWC**：`concat` 只是按通道拼，`convT` 就是四个子像素位置的 1×1 卷积。
- **BN 折叠**：`w' = w·γ/√(var+ε)`，`b' = (b−mean)·γ/√(var+ε) + β`，`ε = 1e-5`。
- **三值权重**：每输出通道 `s = mean|w|`，`q = clamp(round(w/s), −1, 1)`，前向用 `q·(s·bn_scale)`。
- 分派 `gx = 256, gy = ceil(total/16384)`，避开单维 65535 工作组上限。


## 与生成管线的联动（2026-10-30 用户指定，必须保留）

**这个部件头是"生成照片/3D 管线"的一个可插拔下游节点**，不是独立玩具：

- **接口**：输入一帧 RGB（NHWC，`res×res×3`，值域 [0,1]）→ 输出 `res×res` 的部件 id 图
  （`u32`，0 = 背景/无效）；加一个清单（`parts.json`）就能把 5 位三进制平面解码成清单内 id。
- **两条调用路径**：
  1. 库内直接调：`trit_parts::api::infer_ids(backend, &tpm, &rgb, res, &parts)`（任何 Rust 管线可嵌）；
  2. 进程外调：`parts-cli run`（单帧）/ `parts-cli batch`（整目录），产物 `ids.bin` + `ids.pgm`。
- **部署包**：`TPM1`（467 KB，只读，跨平台），导出用 `parts-cli export`。
- **约束**：清单决定 id→部件语义；换装配体时必须同时换清单，**不要**用装配体 A 的清单解装配体 B。
- **已知边界**：① 截止 2026-10-30 只在**单个 135 部件装配体**上训练/验证（14 个距离 + 5 种外观）；
  ② 外观随机化是负结果（见 LEDGER A79）；③ 真实照片一次没测过。

> 等以后做"生成照片/3D"那条线时，直接把本 crate 当节点接进去即可——渲染端只要能出 RGB，
> 这边就能出 id 图；反之若要"由 id 图驱动生成"，则本模型可作为条件/损失项。

## 下一步

1. 正式训练（900 帧 384²，2500 步 / batch 6，≈1 小时），读数对照 Python 基线
   （d0 0.9729 / id 0.7097 / IoU 0.4343）；开跑前先报任务与预计耗时。
2. 训练后复跑 `--eval`，量清单约束的实际收益。
