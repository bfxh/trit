# trit-train：三进制训练实验层（M0）

`publish = false`，不属于稳定 API。

> 与 `trit-net` 的关系：那边已有一个**最小 STE**（`q = clamp(round(w))`、identity 梯度、
> 无 scale、两层 MLP、全批 GD）。本层在同一台机器上把它扩成可比较的三件事：
> **per-row 可学习 scale**、**三种 STE 变体**、**位平面 ALU**，并给出并发/预取读数。
> 两个实验层不共享代码（发布图禁止），数值口径可以直接对照。

| 模块 | 内容 |
|---|---|
| `quant` | absmean 三值量化；**STE** 三变体（Identity / Clipped / Soft）；scale 梯度 |
| `plane` | **位平面 ALU**：双导轨（1 trit = 2 bit），按字并行的 `not`/`and`/`or`/`xor`，支持 Kleene / Łukasiewicz / Bochvar |
| `layer` | `TernaryLinear`：前向 `q + scale`，反向 STE + scale 梯度，单样本 SGD |
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
cargo test -p trit-train                       # 8 个（plane + layer；量化/STE 的 6 个测试在 trit-learn）
cargo run -p trit-train --release --bin train-bench -- --mtrits 16 --rounds 7 --ste-steps 3000
```

## 边界与下一步

- 点积是**标量参考**（2.4 GMAC/s）。快速掩码内核在 `trit-ops`，但发布图禁止实验 crate
  互相依赖 → M1 要把内核下沉到可发布 crate 再复用。
- 没有反向的位平面加速（`dX = Wᵀdy` 也走标量）——这是训练侧最大的一块欠账。
- 没有 bias/激活/多层/优化器状态；`TernaryLinear` 是探针，不是框架。
- 后续候选：KV cache 的三值压缩、AI-to-AI 三态消息格式（TriCom 类）——都还没做，
  需要先有格式规范与可复算的压缩/延迟读数。
