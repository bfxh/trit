# HANDOFF：位平面反向 `dX` 内核（浏览器前端）

> 状态：dX 后端 **已合并**（main `6b910bc9`，PR #664）；前端 **待实现**；实验录像 **unrecorded**。
> 第二十轮训练步已并入 main（`9d1f0cc6`，#666；读数区间修正 `365a39d5`，#667）：`forward`
> 改位平面 `row_dot`、SGD `dW` 走 `dense::sgd_update`、`pack()` 改 `from_latent`。第二十一轮
> 把 Adam 的 latent 更新也做成行级融合（`layer::adam_update`）；第二十二轮把 `q` 改为
> 惰性缓存，训练步不再写 2.36 M 个 `i8`；第二十三轮让 SGD `train_step` 复用前向的
> `qdot`，不再重扫第二遍双导轨；第二十四轮把 `pack()` 的 8-lane 读改写合并成
> 64 列 word 一次写，并给 `shape_bench` 补了 SGD 多形状表。当前训练步读数
> **1.16–1.22 ms/step（23.9–25.0×）**，`pack()` **0.46 ms**，Adam **2.9–3.1 ms/step
> （2.6–2.8×，逐元素参考 8.0–8.2 ms）**，测试 31（含惰性 `q` 缓存回归）。该读数属于“训练步”区块，
> **不能**写成 dX 内核的倍数；本文 §1 的 39.25× 仍是 dX 单测口径。
>
> 本文的“前端”按 [`reference/ATLAS.md`](reference/ATLAS.md) 的定义：**浏览器 playground / WASM 面**，
> 不是 YARA 的词法/语法前端。若接手的是 YARA 解析前端，请看
> [`YARA-EXPR/02-FRONTEND.md`](YARA-EXPR/02-FRONTEND.md)，本文不覆盖那部分。

```text
TASK:        把位平面反向 dX 的实测结果做成人类可读的静态页/区块
OWNER:       model.backend.primary → model.frontend.collaborator
SURFACE:     human
DELIVERABLE: playground 静态页或区块 + 消费 bitplane-backward-metrics.json
MUST:        基线可见；单位/形状/线程/平台/口径齐全；边界与结论同屏
MUST NOT:    不把 dX 内核读数写成训练步加速；不声称三进制算术加法；不声称浏览器可执行内核
DEFERRED:    实时 JSON 接口、WASM 运行、端到端多形状吞吐、dW 的位平面化（当前是融合外积，不是位平面）
HANDOFF:     后端 owner 交内核、读数、证据；前端 owner 交页面、截图/录像与文案复核
EVIDENCE:    PR #664/#666/#667/#668、commit 6b910bc9/9d1f0cc6/365a39d5/797ad121/d55bbbd3/156e487f/c4cf4e11/0c858924/906fd507、dX 21/21、训练步 L172/C082、L173/C083、L174/C084、L175/C085、L176/C086、31/31、40.96× 峰值复跑
```

## 0. 先读什么

1. [`../crates/trit-train/README.md`](../crates/trit-train/README.md) 的
   「位平面反向 `dX = Wᵀdy`」与「实测」两节：这是给人类面的最短说明。
2. `crates/trit-train/src/plane/backward.rs`：布局、标量参考、AVX2 内核。
3. `crates/trit-train/src/layer.rs` 的 `pack()` / `train_step_with_grad`：
   反向如何接入一层。
4. `crates/trit-train/src/bin/bench.rs` 的 `bench_backward`：
   读数是怎么测的，哪些成本没有算进去。
5. [`handoff/bitplane-backward-metrics.json`](handoff/bitplane-backward-metrics.json)：
   前端直接消费的数据快照。
6. [`engineering/AGENT-MODEL-MANIFEST.md`](engineering/AGENT-MODEL-MANIFEST.md) §5：
   人类面板的固定顺序；本文的页面结构按它排。

**一句话边界**：这是一个**原生 x86_64 AVX2 训练内核**，不是浏览器运行时。
前端只做“讲清楚 + 展示证据”，不要尝试在 playground/WASM 里执行它。

## 1. 前端先记住的三句话

1. **它加速的是反向 `dX = Wᵀdy`，不是三进制算术加法。**
   三值权重打成 `pos` / `neg` 两条导轨（1 trit = 2 bit），反向按掩码选符号，
   把 `coeff = dy[r] * scale[r]` 加/减到 f32 的 `dX` lane 上。
2. **dX 读数只覆盖 `dX`；同分支训练步另有优化。**
   第二十轮把 `train_step` 改成 `pack()` 一次 → 位平面前向 → 融合 `dW` →
   `row_dot` 更新 scale；第二十一轮把 Adam 的 latent 更新也做成行级融合；
   第二十二轮把 `q` 改成惰性缓存；第二十三轮让 SGD `train_step` 复用前向的 `qdot`；
   第二十四轮把 `pack()` 改成按 64 列 word 写合并。
   训练步 23.9–25.0×、Adam 2.6–2.8×，与 dX 的倍数不能互相替代。
3. **读数只代表内核本身。**
   4096×576、单线程、7 轮取最快：标量 `0.24` → 双导轨 `9.44` GMAC/s，
   **39.25×**。这个数**不含** `pack()` 的量化成本，也不等于训练步加速。

## 2. 后端已经交付了什么

| 交付物 | 位置 | 状态 | 前端是否要读 |
|---|---|---|---|
| 位平面反向内核 | `crates/trit-train/src/plane/backward.rs` | 已合并（#664） | 只读 §4 的契约，不必读 SIMD 细节 |
| 位平面打包 | `crates/trit-train/src/plane/pack.rs` | 本轮分支（只写双导轨） | 读 §4 的 `pack()` 成本口径 |
| 惰性 `q` 缓存 | `crates/trit-train/src/plane/q.rs` | 本轮分支 | `q()` 首次访问才解码 |
| SGD `dW` 融合更新 | `crates/trit-train/src/dense.rs` | 已合并（#666） | 只在训练步区块引用，不是 dX 页内容 |
| Adam `dW` 融合 | `crates/trit-train/src/layer/adam.rs` | 本轮分支 | 只在 Adam 训练步区块引用 |
| 模块导出与文档 | `crates/trit-train/src/plane.rs` | 已合并（#666） | 读 §4 |
| 层接入 | `crates/trit-train/src/layer.rs` | 已合并（#666） | 读 §4 的 `pack()` 与反向语义 |
| 反向基准 | `crates/trit-train/src/bin/bench.rs` | 已合并（#666；本轮加 Adam 输出） | 读 §8 的复现口径 |
| 人类说明 | `crates/trit-train/README.md` | 已合并（#666；本轮更新） | 可直接摘写 |
| 缺陷台账 | `docs/ERRORS.md` L171 / L172 / L173 / L174 | L171/#664、L172/#666 已合并；L173/L174 本轮 | 只读结论与读数 |
| 回归门 | `.agent/checks/registry.json` C081/C082/C083/C084/C085/C086 | C081/#664、C082/#666 已合并；C083–C086 本轮 | 只读命令 |
| 数据快照 | `docs/handoff/bitplane-backward-metrics.json` | 本次交接新增 | **要读**，dX 页面数字以它为准 |
| PR / commit | `PR #664/#666/#667/#668` / `6b910bc9`、`9d1f0cc6`、`365a39d5`、`797ad121`、`d55bbbd3` | #664/#666/#667 已合并；#668 open | 页面证据链接 |

**验证读数（dX 同一提交 / 第二十三轮训练步）**

- `cargo test -p trit-train`：dX 交付时 **21 passed / 0 failed**；训练步、Adam、整步对拍与惰性 `q` **31 passed / 0 failed**。
- `cargo clippy -p trit-train --all-targets -- -D warnings`：通过。
- `python scripts/hazard_gate.py`：0 findings。
- `python scripts/module_budget.py`：dX 交付时 1513 files；训练步分支 1527 files，debt=0。
- `python tools/checks.py -Only C081`：PASS；C082 随训练步 PR 一起跑。
- PR CI：最近一次完整三端为 **32 pass / 2 skipping / 0 fail**（macOS、Ubuntu、Windows 测试全过；MSRV 1.89 与 wasm32 也过）；本轮 pack/shape 提交的 CI 待跑。

## 3. 页面结构（按人类面板顺序）

### 3.1 首屏

1. **名字**：`位平面反向 dX 内核`。不要用 `PlaneMatrix`、`C081` 这类内部 ID 当标题。
2. **怎么用 / 是干嘛的**：3–5 行。说明这是一个训练实验层的反向内核说明页，
   不是交互式训练器；页面只展示布局、读数和证据。
3. **身份与状态**：后端 `available`（PR #664 已合并），前端 `planned`（本页），
   录像 `unrecorded`。状态词只用 `available` / `planned` / `unavailable`。
4. **一句话结论**：`4096×576、单线程、7 轮取最快：0.24 → 9.44 GMAC/s，39.25×`。

首屏不要放背景长文，不要先讲三进制历史。先让人知道“这是什么、现在是什么状态”。

**放置建议**：在 `playground/` 新增一个只读 tab，或独立 `playground/bitplane-backward.html`；
不要改现有求值器，也不要把训练内核接进浏览器。前端只需读取
`docs/handoff/bitplane-backward-metrics.json` 的副本或同构 JSON。

### 3.2 第二屏：视觉对比

按 manifest，「凡是对比，必须有图；表格只能作为图后的索引」。

**图 A（必须）**：绝对吞吐柱状图。

- 两条横向柱：`标量参考` = `0.24 GMAC/s`，`双导轨掩码` = `9.44 GMAC/s`。
- 线性坐标轴从 `0` 开始，单位写 `GMAC/s`。基线柱即使很短也必须可见。
- 在双导轨柱上标注 `39.25×`，但不要用这个倍数替代纵轴。
- 图注：`4096×576；单线程；7 轮取最快；仅 dX 内核，不含 pack()`。

**图 B（建议）**：数据流图，见 §5.2。它解释“为什么快”，不是替代图 A。

**表 A（图后）**：只放索引信息。

| 项 | 值 |
|---|---|
| 形状 | 4096×576（2.36 M MAC） |
| 线程 | 1 |
| 轮次 | 7，取最快 |
| 标量参考 | 0.23–0.24 GMAC/s |
| 双导轨掩码 | 8.0–9.5 GMAC/s |
| 相对 | 33–41×（最新单次 39.25×） |
| 平台 | Intel Core Ultra 7 270K Plus，24 核 / 24 线程，Windows 11 |
| 构建 | release；rustc 1.99.0 |

### 3.3 第三屏：方法与边界

按 manifest §5 的论文式方法，必须写清：

- 基线是什么：`PlaneMatrix::input_grad_scalar`，逐行逐列按位选符号后累加。
- 当前是什么：`PlaneMatrix::input_grad`，每 8 列取一个字节的 `pos` / `neg` 掩码，
  AVX2 8 lane 加/减，4 个行累加器打散依赖链，尾部标量补齐。
- 口径：4096×576、单线程、7 轮取最快、release、同一台机器。
- 单位：`GMAC/s` = 每秒十亿次乘加；这里的“乘加”按 `rows × cols` 计。
- 判定：只比内核；`pack()`、量化、latent 更新、内存分配之外的开销不计入倍数结论。
- 限制：只测了一个形状；非 x86_64 或列数 < 8 走标量回退，没有跨平台性能结论。

### 3.4 第四屏：结果与限制

必须同屏写出：

- **强在哪**：同样形状下，dX 内核从 `0.24` 提到 `9.44 GMAC/s`，复跑峰值 `40.96×`。
- **没证明什么**：dX 页本身没有端到端训练吞吐；没有多形状曲线；没有 GPU/WASM 读数。
  训练步的 `forward` / SGD `dW` / Adam `dW` 已在第二十、二十一轮单独优化，见 §3.4b；不要拿 dX 的 39.25× 代替。
- **为什么**：标量 `dX` 每列都要读 `q[c]` 并随机更新 `dX`；掩码内核把 8 列压进一条
  AVX2 加减链，所以差距被放大。
- **噪声**：复跑在 `8.0–9.5 GMAC/s` / `33–41×` 之间；页面必须给区间，不能只报峰值。

### 3.4b 第二十一至二十四轮补充：训练步（可选区块）

如果页面要覆盖“训练步”，**必须单独一块**，不能并进 dX 柱状图：

- 路径：`train_step_scalar`（逐行量化 + 标量点积 + 逐元素 STE）对 `train_step`
  （`pack()` 一次 + `PlaneMatrix::row_dot` 前向 + `qdot` 复用 + `dense::sgd_update`
  融合 `dW`）；Adam 区块用 `train_step_with_grad_adam_scalar` 对
  `train_step_with_grad_adam`（`pack()` 一次 + `dX` + `layer::adam_update` 融合 `dW`）。
- 读数（4096×576，单线程，3 步/轮，25 轮取最快，含 `pack`）：SGD 标量
  **28.8–29.1 ms/step**，位平面步 **1.16–1.22 ms/step（23.9–25.0×）**，其中 `pack()`
  **0.46 ms/step**（只出双导轨，不含 `q()` 首次解码）；Adam 逐元素参考
  **8.0–8.2 ms/step**，融合 **2.9–3.1 ms/step（2.6–2.8×）**。
- 多形状（`shape_bench`，9 轮取最快，3 步/轮）：SGD 4096×576 **0.93–1.03 ms
  / 22–24×**，17×576 **17.0–17.5×**，11×130 **12.5×**，9×65 **9.5×**，5×9 **2.9×**；
  Adam 4096×576 **2.6–3.2×**，17×576 **4.15×**，11×130 **3.16×**。
  `1×1` 不计入证据；`9×65` 的绝对耗时只有 0.4–0.5 µs，倍数会抖。
- 边界：这是单层单样本的端到端口径，不含层构造/输入生成；SGD `train_step`
  复用前向的 `qdot`，Adam 路径没有前向，不存在同一份 `qdot`；`pack()` 仍是每步
  一次全量量化，但只写双导轨、按 64 列 word 写合并，`q` 改为首次 `q()` 解码。
  页面若显示 `q` 内存，必须按“首次访问后才分配”解释。
- 证据：`docs/ERRORS.md` L172/L173/L174/L175/L176、`.agent/checks/registry.json`
  C082/C083/C084/C085/C086、`cargo test -p trit-train` 31/31。

### 3.5 第五屏：录像

当前没有实验录像，标 `unrecorded`。不要用静态截图冒充“论文式录像”。
如果前端补录，录像旁仍要能追到 JSON、commit 和复现命令。

### 3.6 第六屏：原始数据

最后放：

- `docs/handoff/bitplane-backward-metrics.json`（页面数字的唯一快照源）。
- PR：`https://github.com/bfxh/trit/pull/664`。
- commit：`6b910bc9`。
- 复现命令：见 §8。

不要把大表倒到首屏；首屏只留结论和状态。

## 4. 数据契约

### 4.1 `PlaneMatrix` 的位平面布局

`PlaneMatrix` 是行主序的三值权重矩阵。字段是私有的，前端不要依赖 Rust 结构体内存布局；
下面只描述**语义**，用于画图和解释 JSON。

| 概念 | 规则 |
|---|---|
| 行 / 列 | `rows` = 输出维，`cols` = 输入维 |
| 每行字数 | `bpr = max(1, ceil(cols / 64))` |
| 字索引 | `word = r * bpr + c / 64` |
| 位索引 | `bit = 1 << (c % 64)` |
| `+1` | `pos` 位 = 1，`neg` 位 = 0 |
| `-1` | `pos` 位 = 0，`neg` 位 = 1 |
| `0` | 两位都是 0 |
| 非法 | 两位都是 1（构造函数不会产生；前端遇到按无效数据处理） |

```text
一行 64 个 trit = 1 个 u64（pos）+ 1 个 u64（neg）
c:      0 1 2 ... 63
pos:    bit c
neg:    bit c
```

`bytes()` 只算两条导轨：`rows * bpr * 8 * 2`。
如果页面要显示“总内存”，还要加展开后的 `q` 缓存：`rows * cols` 个 `i8`；但该缓存
只在首次 `q()` 时分配，未调用 `q()` 的矩阵没有这部分。

**端序**：Rust 的 `u64` 是宿主端序。前端不要直接解析原始字节；如果后端将来导出
二进制，必须在 schema 里明确 `little-endian` 和字/位顺序。当前只承诺上面的位索引语义。

### 4.2 反向语义

对一层 `y = scale ⊙ (Q ⊙ x)`：

```text
dy[r]    = ∂L/∂y[r]                 长度 rows
scale[r] = 每行独立、可学习的 scale  长度 rows
dX[c]    = Σ_r dy[r] * scale[r] * q[r,c]  长度 cols
```

`input_grad(dy, scale, out)` 就是上面这条式子；`out` 由调用方预分配，函数会覆盖它。
`input_grad_scalar` 是同一式子的标量参考，测试逐尺寸对拍。

### 4.3 API 语义表

| 方法 | 输入 | 输出 / 行为 | 断言与回退 |
|---|---|---|---|
| `from_quantized(q, rows, cols)` | 行主序 `i8`，值域 `-1/0/+1` | 构造 `PlaneMatrix` | `q.len() == rows*cols`；值域越界 panic |
| `from_rows(&[Vec<i8>], cols)` | 每行长度 `cols` | 同上 | 任一行长度不符 panic |
| `rows()` / `cols()` / `blocks_per_row()` | — | 维度 | — |
| `bytes()` | — | 两条导轨字节数 | 不含 `q` 缓存 |
| `q()` | — | 展开的 `i8` 权重 | 行主序；首次调用才解码并缓存 |
| `row_dot(r, x)` | 行号、长度 `cols` 的 `f32` | `Σ q[r,c]*x[c]` | 自动 AVX2；否则标量 |
| `row_dot_scalar(r, x)` | 同上 | 同上 | 纯标量参考 |
| `input_grad(dy, scale, out)` | 三个长度已断言 | 写 `dX` | 自动 AVX2；否则标量 |
| `input_grad_scalar(dy, scale, out)` | 同上 | 写 `dX` | 纯标量参考 |
| `TernaryLinear::pack()` | 当前 latent + scale | 量化一次并打包双导轨 | 每步一次全量量化；`q` 惰性 |

自动内核的启用条件：

```text
target_arch = "x86_64"  且  cols >= 8  且  is_x86_feature_detected!("avx2")
否则：标量回退
```

### 4.4 前端的指标 JSON

文件：[`handoff/bitplane-backward-metrics.json`](handoff/bitplane-backward-metrics.json)

它是**人工整理的快照**，不是运行时接口。字段约定：

| 字段 | 类型 | 用途 |
|---|---|---|
| `schema` | string | 固定 `trit.handoff.bitplane-backward/v1` |
| `kind` | string | `curated-snapshot`，提醒前端不是实时数据 |
| `source.commit` / `source.pr` | string / number | 证据链 |
| `platform` | object | 图表脚注 |
| `benchmark.shape` | object | 坐标轴说明 |
| `benchmark.scalar` / `bitplane` | object | 两条柱的值与区间 |
| `benchmark.speedup` | object | 倍数与区间 |
| `tests` / `ci` | object | 质量状态 |
| `limits` | array | 必须展示的限制句 |
| `provenance.command` | string | 复现命令 |

**页面数字以该 JSON 为准**；不要从旧聊天记录或截图抄数。

### 4.5 不是契约的东西

- `PlaneMatrix` 的私有字段、内部 `Vec` 顺序、分配器行为。
- SIMD 指令顺序、累加器数量、LUT 实现。
- 单次计时值、某一轮的最快值、不同机器的横向比较。
- `pack()` 的成本、端到端训练吞吐、未来优化后的数字。

## 5. 可视化说明

### 5.1 一 trit 两 bit

用三色但不要只靠颜色：

```text
+1  →  pos=1, neg=0     标签 "+1"
-1  →  pos=0, neg=1     标签 "−1"
 0  →  pos=0, neg=0     标签 "0"
```

图里要标出“1 trit = 2 bit”，并说明两条导轨是**并列**的，不是二进制补码。

### 5.2 数据流（建议做成四段横图）

```text
latent w + scale
      │  pack()：每步一次全量量化
      ▼
q (i8)  +  pos/neg (u64 双导轨)
      │  对第 r 行取 coeff = dy[r] * scale[r]
      ▼
每 8 列取 1 字节 pos / neg 掩码
      │  AVX2：8 个 f32 lane 同时加/减 coeff
      ▼
4 个行累加器（r & 3）→ 合并 → dX[c]
```

注意：一个 `u64` 装 64 个 trit，但一条 AVX2 指令只处理 8 个 f32 lane；
不要把图画成“64 lane 一条指令”。

### 5.3 图表规范

- 柱状图从 0 开始，不截断纵轴；不制造“看起来差距更大”的视觉效果。
- 不用双 Y 轴、断轴、对数轴来掩盖基线；如果非要用对数轴，必须在轴标题写明。
- 每个数字带单位；`GMAC/s` 不要写成 `GFLOP/s`。
- 图注必须包含：形状、线程、轮次、口径、平台。
- 表格只做图后的索引，不替代图。
- 移动端保留图 A 和边界句；图 B 可以折叠，但不能默认隐藏边界。

## 6. 可直接使用的文案

### 6.1 标题区

```text
位平面反向 dX 内核
本页只讲 dX 的双导轨掩码；训练步的 forward / dW 另见 §3.4b，不是三进制加法。
```

### 6.2 结论区

```text
4096×576、单线程、7 轮取最快：
标量参考 0.24 GMAC/s → 双导轨掩码 9.44 GMAC/s，39.25×。
这是 dX 内核读数，不含 pack() 的量化成本，也不等于训练步加速。
```

### 6.3 边界区

```text
本页已加速：dX = Wᵀdy
训练步（第二十四轮，单独区块）：forward 走 row_dot、SGD train_step 复用 qdot、dW 走 dense::sgd_update、Adam dW 走 layer::adam_update、pack() 走 from_latent 的 word 写合并、q 惰性解码
平台：原生 x86_64 AVX2；非 x86_64 或 cols < 8 自动回退标量
未证明：端到端多层吞吐、GPU/WASM 读数、多形状 Adam 曲线；pack() 仍每步全量量化
```

### 6.4 状态区

```text
后端：dX available（main 6b910bc9，PR #664 已合并）
      训练步 available（main 9d1f0cc6，PR #666 已合并；区间 #667）
      Adam dW 融合 + 惰性 q available（PR #668，d55bbbd3；open）
前端：planned（本页）
录像：unrecorded
```

## 7. 禁止文案与改写

| 不要写 | 为什么 | 改成 |
|---|---|---|
| “训练快 39 倍” | 只测了 dX 内核，不是训练步 | “dX 内核 39.25×（4096×576，单线程）” |
| “三进制加法被加速” | 这里是选符号 + f32 累加 | “双导轨掩码选符号 + f32 累加” |
| “反向全面加速” | 本页只证明 dX；训练步是另一块 | “只加速 dX；训练步见 §3.4b” |
| “浏览器里能跑” | 没有 WASM 目标，内核是原生 AVX2 | “原生 x86_64；前端只展示静态证据” |
| “所有形状都快 33–41×” | 只测了 4096×576 | “该形状实测 33–41×；其他形状未测” |
| “9.5 GMAC/s 是稳定值” | 复跑有区间 | “最新 9.44；复跑 8.0–9.5” |
| “零拷贝 / 无分配” | 内核内部有累加器分配 | “预分配 out；内部累加器分配未单独计时” |
| “论文级证据” | 没有录像 | “静态读数 + 复现命令；录像 unrecorded” |

## 8. 复现与证据

```bash
# 1. 测试
cargo test -p trit-train

# 2. lint
cargo clippy -p trit-train --all-targets -- -D warnings

# 3. 反向 dX 读数
cargo run -p trit-train --release --bin train-bench -- \
  --mtrits 1 --rounds 7 --rows 4096 --cols 576 --threads 1 --ste-steps 1

# 4. 训练步读数（第二十四轮）
cargo run -p trit-train --release --bin train-bench -- \
  --mtrits 1 --rows 4096 --cols 576 --threads 1 --rounds 25 --ste-steps 1 --train-steps 3

# 5. 多形状矩阵（SGD + Adam）
cargo run -p trit-train --release --bin shape_bench

# 6. 门禁
python scripts/hazard_gate.py
python scripts/module_budget.py
python tools/checks.py -Only C081
python tools/checks.py -Only C082
python tools/checks.py -Only C083
python tools/checks.py -Only C084
python tools/checks.py -Only C085
python tools/checks.py -Only C086
```

预期尾部：

```text
反向 dX：4096×576（2 M MAC，单线程，7 轮取最快）
内核                 GMAC/s         相对
标量参考                 0.24      1.00×
双导轨掩码                9.44     39.25×
```

口径提醒：

- `--mtrits` 只控制前面的位平面 ALU 基准，**不控制**反向 dX 的形状；
  反向形状由 `--rows` / `--cols` 决定。
- `--rounds 7` 表示标量与掩码交替跑 7 轮，各取最快。
- `bench_backward` 使用预分配的 `out`；掩码内核内部的 4 个累加器 `Vec` 分配
  计入本次计时，`pack()` 不计入。
- 换机器、换编译器、换形状都不能直接复用这组倍数；必须重跑并在图注写明。

## 9. 已知缺口

| 缺口 | 状态 | 对前端的影响 |
|---|---|---|
| `dW` 位平面化 | 未做；SGD 走 `dense::sgd_update`、Adam 走 `layer::adam_update`，都是融合外积，不是位平面 | 不能写“dW 是位平面” |
| `forward` 快速内核 | 已改位平面 `row_dot`（第二十轮） | 可写“前向走位平面 row_dot”，不能写“所有形状都快” |
| `pack()` 成本 | 已单独计 0.46 ms/step（只写双导轨，按 64 列 word 写合并）；仍是每步一次全量量化 | 不能把 dX 的 39.25×当训练步加速 |
| `q()` 惰性解码 | 首次调用才分配并解码；训练步不调用 | 不能把 `q` 内存算进每个 `PlaneMatrix` |
| 非 x86_64 性能 | 只保证标量回退正确，未测性能 | 不能给跨平台倍数 |
| 端到端多形状吞吐 | 已有 SGD/Adam 单层多形状矩阵；没有多层端到端 | 不能给“训练快多少” |
| Adam 的 `dW` | 已做行级融合（AVX2 8-lane，标量尾；非 x86 回退），不是位平面化 | 可写“Adam dW 融合”，不能写“Adam 全链路位平面” |
| 浏览器/WASM 运行 | 无目标、无绑定 | 只能做静态说明页 |
| 实时 JSON 接口 | 未实现 | 页面用快照；如需实时，后端另开 Rust `--json` 任务 |
| 实验录像 | unrecorded | 不能称论文级证据 |

如果前端需要“页面上的数字自动更新”，正确做法不是让前端跑内核，而是让后端在
`train-bench` 增加一个 Rust `--json` 输出（当前没有）。这是下一张卡，不在本次交接范围。

## 10. 前端验收清单

- [ ] 页面标题是“位平面反向 dX 内核”，不是内部 ID。
- [ ] 首屏有“是什么 / 怎么用 / 状态 / 一句话结论”。
- [ ] 图 A 两条柱都从 0 起，基线可见，单位 `GMAC/s`。
- [ ] 图注含形状、线程、轮次、口径、平台。
- [ ] 表格在图后，且不替代图。
- [ ] 边界句与结论同屏：只加速 dX；训练步另见 §3.4b；不含 pack；非 x86 回退。
- [ ] 页面数字与 `docs/handoff/bitplane-backward-metrics.json` 一致。
- [ ] 没有出现 §7 的禁止文案。
- [ ] 没有尝试在 playground/WASM 里调用 `trit-train`。
- [ ] 原始数据、PR、commit、复现命令可点击/可复制。
- [ ] 图有 alt 文本；颜色不是唯一编码。
- [ ] 移动端仍能看到图 A 和边界句。
- [ ] 页面路径、截图或录像链接写进交接回执。

## 11. 交接回执

前端完成后回填：

```text
PAGE_PATH:      docs/... 或 playground/...
DATA_SOURCE:    docs/handoff/bitplane-backward-metrics.json
SCREENSHOT:     <链接或仓库路径>
RECORDING:      unrecorded / <链接>
CLAIMS_REVIEW:  pass / fail（逐条对照 §7）
OPEN_QUESTIONS: ...
```

后端 owner 复核：

```text
BACKEND_OWNER:  model.backend.primary
CODE_FREEZE:    6b910bc9
EVIDENCE_OK:    yes / no
NOTES:          只复核数字与边界，不改前端版式
```

## 附录 A：术语

| 术语 | 含义 |
|---|---|
| trit | 三值单位，取值 `-1/0/+1` |
| 双导轨 | `pos` / `neg` 两条位平面，1 trit = 2 bit |
| `dX` | 对输入的梯度 `∂L/∂x`，形状 `cols` |
| `dy` | 对输出的梯度 `∂L/∂y`，形状 `rows` |
| `scale` | 每行独立、可学习的缩放，形状 `rows` |
| `pack()` | 把 latent 量化一次并打成 `PlaneMatrix` |
| GMAC/s | 每秒十亿次乘加；本页按 `rows × cols` 计 |
| 标量参考 | `input_grad_scalar` / `row_dot_scalar`，逐元素参考实现 |
| AVX2 | x86_64 的 256 位 SIMD；本内核一次处理 8 个 f32 lane |

## 附录 B：文件地图

| 路径 | 内容 |
|---|---|
| `crates/trit-train/src/plane/backward.rs` | `PlaneMatrix`、`input_grad`、`input_grad_scalar`、AVX2 内核 |
| `crates/trit-train/src/plane.rs` | `pub use backward::PlaneMatrix`；位平面 ALU 边界 |
| `crates/trit-train/src/layer.rs` | `pack()`；SGD/Adam 反向接入 |
| `crates/trit-train/src/bin/bench.rs` | `bench_backward`；读数输出 |
| `crates/trit-train/README.md` | 人类说明、读数表、边界 |
| `docs/ERRORS.md` L171 | 缺陷台账与读数结论 |
| `.agent/checks/registry.json` C081 | 回归门 |
| `docs/handoff/bitplane-backward-metrics.json` | 前端数据快照 |
