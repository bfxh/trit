# trit-train：三进制训练实验层（M0）

`publish = false`，不属于稳定 API。

> 与 `trit-net` 的关系：那边已有一个**最小 STE**（`q = clamp(round(w))`、identity 梯度、
> 无 scale、两层 MLP、全批 GD）。本层在同一台机器上把它扩成一组可比较的原语：
> **per-row 可学习 scale**、**三种 STE 变体**、**位平面 ALU**、**位平面反向 `dX`**、
> **位平面训练步 + `dW` 融合更新**，并给出并发/预取/反向/训练步吞吐读数。
> 两个实验层不共享代码（发布图禁止），数值口径可以直接对照。

| 模块 | 内容 |
|---|---|
| `quant` | absmean 三值量化；**STE** 三变体（Identity / Clipped / Soft）；scale 梯度 |
| `plane` | **位平面 ALU**：双导轨（1 trit = 2 bit），按字并行的 `not`/`and`/`or`/`xor`，支持 Kleene / Łukasiewicz / Bochvar；`PlaneMatrix` 打包、前向点积与反向 `dX` 掩码内核 |
| `dense` | `dW` 融合外积更新：`w -= lr · STE(w, scale, dy·scale·x)`，AVX2 8-lane + 标量尾 |
| `layer` | `TernaryLinear`：前向走 `PlaneMatrix`，反向 STE + scale 梯度 + 融合 `dW`，单样本 SGD / Adam |
| `prefetch` | `_mm_prefetch` 提示（非 x86_64 空操作），用于 A/B |

## STE 做了什么

```text
前向:  q = clamp(round(w / scale), -1, 1)      y = scale · Σ qᵢxᵢ
反向:  ∂L/∂w = ∂L/∂q · g(w/scale)              ∂L/∂scale = dy · Σ qᵢxᵢ
门控:  Identity → 1
       Clipped  → |w/scale| ≤ 1 ? 1 : 0
       Soft     → max(0, 1 − (w/scale)²)
```

scale 是**每行独立、可学习**的参数（LSQ 风格），不是每步从 latent 重算——否则
scale 的梯度会绕回量化门控，训练不稳。

## 位平面 ALU

双导轨布局：`pos` 位 = `+1`、`neg` 位 = `-1`、两位皆 0 = `X`。按字位运算：

| 运算 | Kleene / Łukasiewicz | Bochvar |
|---|---|---|
| `and` | `pos = ap&bp`，`neg = an\|bn` | `pos = ap&bp`，其余非 `pos` 且含 `X` → `X` |
| `or` | `pos = ap\|bp`，`neg = an&bn` | `neg = an&bn`，其余非 `neg` 且含 `X` → `X` |
| `xor`（强加法） | Kleene：`X` 传播；Łukasiewicz：`X⊕X = +1`、`T⊕F = +1` | 同 Kleene |

9 种操作数组合 × 3 套语义 × 长度边界（0/1/63/64/65/130/200）都有逐元素对拍测试。

**算术加法不做**：三进制进位在打包表示上只能逐 lane 串行，此前 `trit-balanced`
实测比 i64 慢 2–3 个数量级——已证负结果，不重复踩。

## 位平面反向 `dX = Wᵀdy`

`TernaryLinear::pack()` 把当前 latent 量化一次，按行打成 `pos`/`neg` 两条导轨
（`PlaneMatrix`，1 trit = 2 bit）。反向不再逐列读 `q[c]`，而是：

1. 对第 `r` 行取 `coeff = dy[r] * scale[r]`；
2. 每 8 列取一个字节的 `pos`/`neg` 掩码，用 AVX2 把 `coeff` 加/减到 `dX` 的 8 个 lane；
3. 4 个行累加器打散依赖链，尾部按标量补齐。

`train_step_with_grad` 复用同一份 `PlaneMatrix`：`dX`、scale 梯度（`row_dot`）和
`dW` 都基于这一次打包，不再二次量化。`train_step_with_grad_adam` 的 `dX`/scale 也复用，
但 latent 的 Adam 更新仍是逐元素循环——这是明确的未覆盖项。标量参考
`PlaneMatrix::input_grad_scalar` 保留，测试逐尺寸对拍。

边界：这里只做「双导轨选符号 + f32 累加」，**不是三进制算术加法**；非 x86_64
或列数小于 8 时自动回退标量。前向也已改为同一份打包后的 `row_dot`，不再逐行
`quantize_with_scale`。

## 位平面训练步

`train_step` 每步只做一次 `pack()`：

1. `PlaneMatrix::from_latent(latent, scale, rows, cols)`：AVX2 每 8 列做一次除法、
   round、clamp，用 `movemask` 直接写 `pos`/`neg` 字节；不再分配每行临时 `q`；
2. `forward_with_planes`：每行 `scale[r] * planes.row_dot(r, x)`；
3. `dense::sgd_update`：整层 `dW` 融合外积更新，按 8 lane 计算 STE 门控；
4. scale 梯度用同一份 planes 的 `row_dot`，不再重算点积。

`forward_scalar` / `train_step_scalar` 保留为标量对拍与基准路径。

## 实测（本机 24 核，16 M trit，7 轮取最快；`bench` 可复算）

位平面 ALU（Kleene，Gop/s）：

| 线程 | 预取 | AND | OR | XOR |
|---:|---|---:|---:|---:|
| 1 | off | 50.38 | 49.11 | 57.35 |
| 2 | off | 86.25 | 90.86 | 90.86 |
| **4** | off | **90.60** | **89.94** | **97.44** |
| 8 | off | 68.35 | 74.49 | 74.73 |
| 4 | on | 90.70 | 95.92 | 95.29 |

三值点积（4096×576，标量参考实现）：

| 线程 | 1 | 2 | 4 | 8 |
|---|---:|---:|---:|---:|
| GMAC/s | 2.36 | 4.41 | 6.90 | 8.13 |

反向 `dX`（4096×576，单线程，7 轮取最快，多次复跑）：

| 内核 | GMAC/s | 相对标量 |
|---|---:|---:|
| 标量参考 | 0.23–0.24 | 1.00× |
| 双导轨掩码 | 8.0–9.5 | **33–41×** |

标量 `dX` 比标量点积慢一个量级，是因为它每列都要读 `q[c]` 并随机更新 `dX`；
掩码内核把 8 列压进一条 AVX2 加减链，差距被放大。这个读数只算内核本身，
不含 `pack()` 的量化成本；训练步的真实开销见下一张表。

训练步（4096×576，单线程，3 步/轮，7 轮取最快，含 `pack`）：

| 路径 | ms/step | 相对 |
|---|---:|---:|
| 标量参考（`train_step_scalar`） | 27.2–27.6 | 1.00× |
| 位平面步（`train_step`） | 1.9–2.1（偶发 2.4） | **13.4–14.1×** |
| 其中 `pack()` | 0.85–0.89 | — |

同一轮 `dX` 读数在 8.0–9.5 GMAC/s；训练步是端到端口径，含打包、前向、`dW`
和 scale 更新，不含层构造/输入生成。2.4 ms 是热降频单次，不当作稳定值。

三条结论：

1. **并发到 4 线程见顶**（约 90–100 Gop/s），8 线程回落 15–30%：工作集 16 MB 左右，
   已经压到内存带宽，再叠超线程只是争用。
2. **软件预取对顺序访问没有收益**（多次运行在 ±5% 噪声内）：硬件预取器已经覆盖。
   它只在跨步/间接访问上才有机会——这里不是。
3. **小规模会把结论测反**：1 M trit 时线程创建开销压过计算，8 线程只有 5.4 Gop/s
   （比单线程慢 3×）。bench 默认 16 M trit 就是为了避开这个陷阱。

## STE 收敛

2×3 线性层、无噪目标、3000 步：

| 变体 | 最终 MSE | 达标（1e-4）步数 | 三值权重 |
|---|---:|---:|---|
| Identity | 0.000000 | 99 | `[[1,0,-1],[0,1,1]]` |
| Clipped | 0.000000 | 99 | 同上 |
| Soft | 0.000000 | 99 | 同上 |

**这个任务太容易，区分不了变体**——三个都收敛到同一组权重。要区分得换带噪/更大模型
（M1），当前只证明"三条路径都通"。

## 跑

```bash
cargo test -p trit-train                       # 27 个（plane/反向/打包/dW/layer；量化/STE 的 6 个测试在 trit-learn）
cargo run -p trit-train --release --bin train-bench -- --mtrits 16 --rounds 7 --ste-steps 3000
cargo run -p trit-train --release --bin train-bench -- --rows 4096 --cols 576 --rounds 7 --train-steps 3
```

## 边界与下一步

- 前向点积已走**位平面 `row_dot`**（每 8 个 trit 取一个字节的 `pos`/`neg` 掩码）。
  更快的掩码内核在 `trit-ops`，但发布图禁止实验 crate 互相依赖 → M1 要把内核下沉到
  可发布 crate 再复用；本轮的位平面前向/反向没有跨 crate 依赖。
- 反向 `dX`、前向与 `dW` 已融合；`train_step_with_grad_adam` 的 latent 更新仍是逐元素
  Adam，`pack()` 仍是每步一次全量量化。
- 没有 bias/激活/多层/优化器状态（除单层 Adam 探针）；`TernaryLinear` 是探针，不是框架。
- 后续候选：KV cache 的三值压缩、AI-to-AI 三态消息格式（TriCom 类）——都还没做，
  需要先有格式规范与可复算的压缩/延迟读数。
- 前端交接（浏览器面，不是 YARA 解析前端）：`docs/HANDOFF-BITPLANE-BACKWARD-FRONTEND.md`。
