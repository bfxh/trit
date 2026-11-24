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
| `plane` | **位平面 ALU**：双导轨（1 trit = 2 bit），按字并行的 `not`/`and`/`or`/`xor`，支持 Kleene / Łukasiewicz / Bochvar；`PlaneMatrix` 打包、前向点积与反向 `dX` 掩码内核，`q` 缓存惰性展开 |
| `dense` | `dW` 的 SGD 融合外积更新：`w -= lr · STE(w, scale, dy·scale·x)`，AVX2 8-lane + 标量尾 |
| `layer` | `TernaryLinear`：前向走 `PlaneMatrix`，反向 STE + scale 梯度 + 融合 `dW`，单样本 SGD / Adam；Adam 的行级融合在 `layer/adam.rs` |
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

**`pack()` 除法不做倒数近似**：`w*(1/s)` 表面快 4–6%，但 f32 半边界不保证与
`w/s` 逐位一致；加“距半整数 1e-4 内回退精确除法”后，`pack()` 慢 10–15%、整步同步
变慢，两个版本都判负、未提交。

## 位平面反向 `dX = Wᵀdy`

`TernaryLinear::pack()` 把当前 latent 量化一次，按行打成 `pos`/`neg` 两条导轨
（`PlaneMatrix`，1 trit = 2 bit）。反向不再逐列读 `q[c]`，而是：

1. 对第 `r` 行取 `coeff = dy[r] * scale[r]`；
2. 每 8 列取一个字节的 `pos`/`neg` 掩码，用 AVX2 把 `coeff` 加/减到 `dX` 的 8 个 lane；
3. 4 个行累加器打散依赖链，尾部按标量补齐。

`train_step_with_grad` 复用同一份 `PlaneMatrix`：`dX`、scale 梯度（`row_dot`）和
`dW` 都基于这一次打包，不再二次量化。`train_step_with_grad_adam` 也复用同一份
planes，并把 latent 的 STE 门、一阶/二阶矩和参数更新交给 `layer::adam_update`
的行级融合内核；`train_step_with_grad_adam_scalar` 保留逐元素参考。标量参考
`PlaneMatrix::input_grad_scalar` 保留，测试逐尺寸对拍。

边界：这里只做「双导轨选符号 + f32 累加」，**不是三进制算术加法**；非 x86_64
或列数小于 8 时自动回退标量。`forward()` 仍走打包后的 `row_dot`；SGD
`train_step` 走 `fused::forward_dot`，不再构造双导轨。

## 训练步：SGD 量化点积融合

`train_step` 的 SGD 前向不构造双导轨：`plane::fused::forward_dot` 每 8 列直接做
`clamp(round(w / scale), -1, 1)`，用符号掩码乘 `x` 并累加。它省掉 `pack()` 的
589 KB 分配/写回和 `row_dot` 的第二遍读；累加分组、`hsum` 与 `row_dot` 相同，
测试逐位对拍。

1. `fused::forward_dot` 一次量化 + 点积，返回每行 `qdot`；
2. `dense::sgd_update`：整层 `dW` 融合外积更新，按 8 lane 计算 STE 门控；
3. scale 梯度直接用 `qdot`，不再重算点积。

`pack()` 现在只服务 `forward()`、`train_step_with_grad` 的 `dX` 和 Adam `dX`；
`row_dot` 的 4 个固定累加器也仍用于这些路径。`forward_scalar` /
`train_step_scalar` 保留为标量对拍与基准路径。

**Adam 训练步**：`train_step_with_grad_adam` 仍需一次 `pack()` 和 `dX`；
`layer::adam_update` 每 8 lane 一次算 STE 门、一阶/二阶矩、偏置校正和参数更新，
短行、尾部或非 AVX2 走标量。`train_step_with_grad_adam_scalar` 保留旧的逐元素
Adam 路径，既做对拍也做基准。

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

训练步（4096×576，单线程，3 步/轮，25 轮取最快；SGD 融合前向，不含 `pack`）：

| 路径 | ms/step | 相对 |
|---|---:|---:|
| 标量参考（`train_step_scalar`） | 28.8–29.1 | 1.00× |
| 融合前向（`train_step` + `fused::forward_dot`） | 0.81–1.14 | **27–35×** |
| `pack()` 参考（不参与融合步，只出双导轨） | 0.46 | — |

`fused::forward_dot` 的复算 example（`cargo run -p trit-train --release --example fused_probe`，4096 行）
把 `pack()+row_dot` 的 **0.62–0.68 ms**
压到 **0.35–0.37 ms（1.7–1.9×）**；整步同轮 A/B 的安静样本从 **1.16–1.27 ms**
降到 **0.81–0.92 ms**，负载高时会到 1.1–1.5 ms。上表 0.81–1.14 是安静样本，
不把高载单次当稳定值。

`row_dot` 的 4 个累加器已从 `acc[j & 3]` 动态索引改成固定累加器 + 内联单块闭包：
主循环一次推进 4 个 8-lane 块，余块单路收尾；累加分组仍是 `j mod 4`。独立探针
（4096 行）**0.2444→0.1428 ms（1.71×）**；它现在用于 `forward()`、
`train_step_with_grad` 和 Adam 的 scale 梯度，不参与 SGD 融合步。

Adam 训练步（4096×576，单线程，3 步/轮，25 轮取最快，含 `pack`）：

| 路径 | ms/step | 相对 |
|---|---:|---:|
| 逐元素参考（`train_step_with_grad_adam_scalar`） | 8.0–8.2 | 1.00× |
| 行级融合（`train_step_with_grad_adam`） | 2.9–3.1 | **2.6–2.8×** |

两条路径用同一条基准复算：融合是默认输出，参考路径加 `--adam-scalar`。

多形状矩阵（`shape_bench`，按形状放大步数，9 轮取最快；SGD 不含 `pack`，Adam 含 `pack`）：

SGD `train_step`（融合前向）：

| shape | ms/step | 标量 ms/step | 倍数 |
|---|---:|---:|---:|
| 2×7 | 0.0001 | 0.0002 | 2.0× |
| 5×9 | 0.0001–0.0002 | 0.0005 | 3.0–4.9× |
| 9×65 | 0.0002 | 0.0034–0.0036 | 14.5–15.3× |
| 11×130 | 0.0004 | 0.0081–0.0089 | 19–20× |
| 17×576 | 0.0023–0.0024 | 0.059–0.071 | 27–30× |
| 4096×576 | 0.90–1.00 | 23.2–27.1 | 25.9–27.1× |

Adam `train_step_with_grad_adam`：

| shape | 融合 ms/step | 逐元素 ms/step | 倍数 |
|---|---:|---:|---:|
| 2×7 | 0.0003 | 0.0003 | 1.00× |
| 5×9 | 0.0004 | 0.0005 | 1.14× |
| 9×65 | 0.0009–0.0010 | 0.0022 | 2.3× |
| 11×130 | 0.0014 | 0.0045 | 3.16× |
| 17×576 | 0.0068 | 0.0282 | 4.15× |
| 4096×576 | 2.5–3.1 | 7.8–8.0 | 2.6–3.2× |

SGD 的 `2×7` 只有 2.0×，仍是计时噪声；`5×9` 放大到 3.0–4.9×；`9×65` 到
`17×576` 逐级升高，4096×576 在 25.9–27.1×。Adam 的 `2×7` 没进 SIMD，和逐元素持平；
`5×9` 只有 1.14×，大矩阵回落到 2.6–3.2×。`1×1` 太短，计时分辨率不足，
不作为倍数证据；`9×65` 的绝对时间也只有 0.2–0.4 µs，倍数会抖。
**不能拿 4096×576 的倍数覆盖所有形状**。`shape_bench` 的 9 轮与 `train-bench`
的 25 轮是两次独立复跑，机器负载不同；绝对 ms 不跨表比较，倍数按各自标量参考算。

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
cargo test -p trit-train                       # 31 个（位平面/打包/惰性 q/dW/Adam 对拍；量化/STE 的 6 个测试在 trit-learn）
cargo run -p trit-train --release --bin train-bench -- --mtrits 16 --rounds 7 --ste-steps 3000
cargo run -p trit-train --release --bin train-bench -- --mtrits 1 --rows 4096 --cols 576 --threads 1 --rounds 9 --train-steps 3
cargo run -p trit-train --release --bin train-bench -- --mtrits 1 --rows 4096 --cols 576 --threads 1 --rounds 9 --train-steps 3 --adam-scalar
cargo run -p trit-train --release --bin shape_bench
```

## 边界与下一步

- 前向点积已走**位平面 `row_dot`**（每 8 个 trit 取一个字节的 `pos`/`neg` 掩码）。
  更快的掩码内核在 `trit-ops`，但发布图禁止实验 crate 互相依赖 → M1 要把内核下沉到
  可发布 crate 再复用；本轮的位平面前向/反向没有跨 crate 依赖。
- 反向 `dX`、前向、SGD `dW` 与 Adam `dW` 都已融合；`pack()` 仍是每步一次全量量化，
  但只写双导轨，`q` 缓存改为首次 `q()` 时惰性解码。训练步不读 `q`，因此不付这笔写回。
- 没有 bias/激活/多层/优化器状态（除单层 Adam 探针）；`TernaryLinear` 是探针，不是框架。
- 后续候选：KV cache 的三值压缩、AI-to-AI 三态消息格式（TriCom 类）——都还没做，
  需要先有格式规范与可复算的压缩/延迟读数。
- 前端交接（浏览器面，不是 YARA 解析前端）：`docs/HANDOFF-BITPLANE-BACKWARD-FRONTEND.md`。
