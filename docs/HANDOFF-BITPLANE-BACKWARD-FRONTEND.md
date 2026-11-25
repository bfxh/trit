# HANDOFF：位平面反向 `dX` 内核（浏览器前端）

> 状态：dX 后端 **已合并**（main `6b910bc9`，PR #664）；前端 **待实现**；实验录像 **unrecorded**。
> 第二十轮训练步已并入 main（`9d1f0cc6`，#666；读数区间修正 `365a39d5`，#667）：`forward`
> 改位平面 `row_dot`、SGD `dW` 走 `dense::sgd_update`、`pack()` 改 `from_latent`。第二十一轮
> 把 Adam 的 latent 更新也做成行级融合（`layer::adam_update`，现已移入
> `layer/adam/support.rs` 作测试参考）；第二十二轮把 `q` 改为
> 惰性缓存，训练步不再写 2.36 M 个 `i8`；第二十三轮让 SGD `train_step` 复用前向的
> `qdot`，不再重扫第二遍双导轨；第二十四轮把 `pack()` 的 8-lane 读改写合并成
> 64 列 word 一次写，并给 `shape_bench` 补了 SGD 多形状表；第二十五轮把 `row_dot`
> 的 `acc[j & 3]` 动态索引改成 4 个固定累加器 + 内联闭包（独立探针 0.2444→0.1428 ms，
> 1.71×）；第二十六轮把 SGD `train_step` 与 `forward()` 都改成 `fused::forward_dot`
> 一遍量化+点积，省掉前向的 `pack()` 双导轨；`forward` 复算 **0.62→0.36 ms（1.71×）**。
> 第二十七轮把反向也改成 `fused::backward` 一遍量化同时算 `dX`/`qdot`，
> `train_step_with_grad` 与 Adam 不再 `pack()`；复算 **0.747–0.759→0.430–0.441 ms
> （1.72–1.74×）**，`dX`/`qdot` 逐位相等。第二十八轮把带梯度 SGD 的 STE `dW`
> 与 scale 更新也并进同一遍（`fused::backward_step`），旧两遍 **0.92–1.32** →
> 新单遍 **0.68–0.76 ms/step（中位 0.98→0.71，约 1.38×）**。macOS aarch64 的标量
> 回退曾因 `dX` 四累加器与旧 `input_grad_scalar` 顺序累加差 1 ULP，已改成单个
> `dx[c]` 顺序累加并补纯标量对拍（`a62dd419`）。第二十九轮把 Adam 的量化、
> `dX`/`qdot` 与 latent/m/v 更新并进 **`fused::adam_step` 单遍**，旧两遍
> **2.12–2.48** → 新单遍 **1.70–1.85 ms/step（中位 2.33→1.76，约 1.32×）**；
> `shape_bench` 4096×576 精确旧基线中位 **2.2746→1.8348 ms（约 1.24×）**，
> `dX`/latent/scale/m/v 在 6 形状 × 3 STE × 3 步下逐位相等，测试 **37**。
> 第三十轮把三条 AVX2 路径的 Soft STE 门控改成 `max(1-ratio², 0)`，靠 `maxps`
> 的 NaN 返回第二操作数语义省掉 `ordered` 掩码：`train_step` **0.80→0.77
> ms/step（1.04×）**，`train_step_with_grad` **0.62→0.58（1.05–1.07×）**，
> Adam **1.73→1.69（1.02–1.03×）**；有限输入逐位不变，测试仍 37/37。
> 第三十一轮把 `train_step` 的 `forward_dot` + `sgd_update` 两遍压成逐行融合
> （`fused::forward_step`）：`train_step` **0.83→0.67 ms/step（1.24×）**，拆分
> `dense/row.rs`、`fused/row.rs` 后 5 组 **0.88→0.69（1.27×）**，`shape_bench`
> 4096×576 **0.7925→0.7007（1.13×）**；latent/scale/loss 逐位相等，测试 38/38。
> 第三十二轮把 Adam 的 `q` 量化除法换成 ±`0.5·s` 阈值比较（`fused/adam_step/q_sign.rs`），
> 只有阈值 ±2^-12 带内或 NaN 回退原除法：Adam **1.72→1.61 ms/step（1.07×）**，
> `shape_bench` **1.7023→1.5682（1.09×）**；`dX`/`qdot` 累加顺序不变，测试 39/39。
> 第三十三轮把同一 `q` 阈值复用到 `fused::row` 与 `fused::backward_step`：
> `train_step` **0.63→0.58–0.60 ms/step（1.05–1.09×）**，带梯度 SGD
> **0.59→0.47–0.49（1.18–1.25×）**；`shape_bench` 4096×576 SGD **0.5319→0.4336
> （1.227×）**，Adam 保持 **1.005×**；测试 40/40。
> 当前融合步读数 **0.58–0.70 ms/step（A/B 与 shape_bench；第二十四轮
> 旧口径 1.16–1.22 / 23.9–25.0×）**，`pack()` **0.46 ms**（不参与任何训练步，仅参考），
> Adam **1.59–1.85 ms/step（约 1.3–1.45×，逐元素参考 8.0–8.2 ms）**。
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
EVIDENCE:    PR #664/#666/#667/#668、commit 6b910bc9/9d1f0cc6/365a39d5/797ad121/d55bbbd3/156e487f/c4cf4e11/0c858924/906fd507/3d74f747/d1dc26d1/09452903/9d1b947e/c64fe0c7/aa92dd1b/a62dd419/cd5e9a00/ba217795/f00d9dcc/be4cb3fe/0309c4ba/f83e83a1/005c2e37/aca268cb/91536a99/d3e041ab/84d4606b、dX 21/21、训练步 L172/C082、L173/C083、L174/C084、L175/C085、L176/C086、L177/C087、L178/C088、L179/C089、L180/C090、L181/C091、L182/C092、L183/C093、L184/C094、L185/C095、40/40、40.96× 峰值复跑
```

## 0. 先读什么

1. [`../crates/trit-train/README.md`](../crates/trit-train/README.md) 的
   「位平面反向 `dX = Wᵀdy`」与「实测」两节：这是给人类面的最短说明。
2. `crates/trit-train/src/plane/backward.rs`：布局、标量参考、AVX2 内核；
   `plane/fused/backward.rs`：训练实际走的量化 + `dX`/`qdot` 单遍；
   `plane/fused/backward_step.rs`：带梯度 SGD 再并上 STE `dW`/scale 更新。
3. `crates/trit-train/src/layer/grad.rs` 的 `train_step_with_grad`：反向如何接入一层；
   `layer.rs` 的 `pack()` 现在只服务公开检查/对拍。
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
   第二十四轮把 `pack()` 改成按 64 列 word 写合并；第二十五轮把 `row_dot` 改成
   4 个固定累加器；第二十六轮把 SGD 前向改成 `fused::forward_dot`，不再构造双导轨；
   第二十七轮把反向也改成 `fused::backward` 单遍，`train_step_with_grad` 与 Adam 不再 `pack()`；
   第二十八轮把带梯度 SGD 的 STE `dW` 并进同一遍（`fused::backward_step`）；
   第二十九轮把 Adam 的量化、`dX`/`qdot` 与 latent/m/v 更新并进 `fused::adam_step` 单遍。
   第三十轮把 Soft STE 门控改成 `max(1-ratio², 0)`，省掉 `ordered` 掩码。
   第三十一轮把 `train_step` 的前向、MSE、`dW` 与 scale 更新压成逐行融合。
   第三十二轮把 Adam 的 `q` 量化除法换成 ±`0.5·s` 阈值比较，阈值附近回退原除法。
   第三十三轮把同一阈值复用到 `fused::row` 与 `fused::backward_step`。
   训练步 0.58–0.70 ms/step、带梯度 SGD 0.47–0.76 ms/step（旧两遍 0.92–1.32）、
   Adam 1.59–1.85 ms/step（旧两遍 2.12–2.48），与 dX 的倍数不能互相替代。
3. **读数只代表内核本身。**
   4096×576、单线程、7 轮取最快：标量 `0.24` → 双导轨 `9.44` GMAC/s，
   **39.25×**。这个数**不含** `pack()` 的量化成本，也不等于训练步加速。

## 2. 后端已经交付了什么

| 交付物 | 位置 | 状态 | 前端是否要读 |
|---|---|---|---|
| 位平面反向内核 | `crates/trit-train/src/plane/backward.rs` | 已合并（#664） | 只读 §4 的契约，不必读 SIMD 细节 |
| 位平面打包 | `crates/trit-train/src/plane/pack.rs` | 本轮分支（只写双导轨） | 只在成本参考里出现；训练不调用 |
| 惰性 `q` 缓存 | `crates/trit-train/src/plane/q.rs` | 本轮分支 | `q()` 首次访问才解码 |
| SGD `dW` 融合更新 | `crates/trit-train/src/dense.rs` | 已合并（#666） | 只在训练步区块引用，不是 dX 页内容 |
| Adam `dW` 融合 + 单遍 | `crates/trit-train/src/layer/adam.rs`、`plane/fused/adam_step.rs` | 本轮分支 | 只在 Adam 训练步区块引用 |
| 反向 `dX`+`qdot` 单遍 | `crates/trit-train/src/plane/fused/backward.rs` | 本轮分支（`c64fe0c7`） | 读 §3.4b 的复算口径 |
| 带梯度 SGD 单遍 | `crates/trit-train/src/plane/fused/backward_step.rs` | 本轮分支（`a62dd419`） | 只在带梯度 SGD 区块引用 |
| 模块导出与文档 | `crates/trit-train/src/plane.rs` | 已合并（#666） | 读 §4 |
| 层接入 | `crates/trit-train/src/layer/grad.rs`、`layer/adam.rs` | 本轮分支 | 读 §4 的反向语义 |
| 反向基准 | `crates/trit-train/src/bin/bench.rs` | 已合并（#666；本轮加 Adam 输出） | 读 §8 的复现口径 |
| 人类说明 | `crates/trit-train/README.md` | 已合并（#666；本轮更新） | 可直接摘写 |
| 缺陷台账 | `docs/ERRORS.md` L171–L185 | L171/#664、L172/#666 已合并；L173–L185 本轮 | 只读结论与读数 |
| 回归门 | `.agent/checks/registry.json` C081–C095 | C081/#664、C082/#666 已合并；C083–C095 本轮 | 只读命令 |
| 数据快照 | `docs/handoff/bitplane-backward-metrics.json` | 本次交接新增 | **要读**，dX 页面数字以它为准 |
| PR / commit | `PR #664/#666/#667/#668` / `6b910bc9`、`9d1f0cc6`、`365a39d5`、`797ad121`、`d55bbbd3`、`c64fe0c7`、`cd5e9a00`、`ba217795` | #664/#666/#667 已合并；#668 open | 页面证据链接 |

**验证读数（dX 同一提交 / 第二十八、二十九轮训练步）**

- `cargo test -p trit-train`：dX 交付时 **21 passed / 0 failed**；训练步、Adam 单遍、反向单遍、带梯度单遍与整步对拍 **40 passed / 0 failed**。
- `cargo clippy -p trit-train --all-targets -- -D warnings`：通过。
- `python scripts/hazard_gate.py`：0 findings / **98 allow**。
- `python scripts/module_budget.py`：dX 交付时 1513 files；第三十轮 **1544 files**；第三十一轮 **1548**；第三十二轮 **1550**；第三十三轮 **1551 files，debt=0**。
- `python tools/checks.py -Only C081,C082,C083,C084,C085,C086,C087,C088,C089,C090,C091,C092,C093,C094,C095`：C081 已合并；C082–C095 随分支跑。
- PR CI：代码+测试 head `84d4606b` 上 **32 pass / 2 skip / 0 fail，mergeState CLEAN**
  （`train_step` 逐行融合、Soft 门控、Adam `q` 符号阈值与 row/backward 复用四条
  AVX2 路径；Ubuntu / Windows / macOS 三端测试全过，module budget 1551/debt=0，
  `agent checks (registry)` 含 C095，mutation changed-crates 通过）；docs-only 提交只记录该结果。

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

### 3.4b 第二十一至二十八轮补充：训练步（可选区块）

如果页面要覆盖“训练步”，**必须单独一块**，不能并进 dX 柱状图：

- 路径：`train_step_scalar`（逐行量化 + 标量点积 + 逐元素 STE）对 `train_step`
  （`fused::forward_step` 逐行融合：`forward_dot_row` 的 `q` 走 ±`0.5·s` 阈值
  + 点积 → 该行 MSE/loss → `dense::sgd_update_row` 的 STE `dW` 与 scale 更新）；
  带梯度 SGD 用 `train_step_with_grad`（`fused::backward_step` 一次量化
  同时出 `dX`/`qdot`、STE `dW` 与 scale，`q` 同样走阈值掩码）；Adam 区块用
  `train_step_with_grad_adam_scalar` 对 `train_step_with_grad_adam`
  （`fused::adam_step` 一次量化同时出 `dX`/`qdot` 并更新 latent/m/v，`q` 符号走
  ±`0.5·s` 阈值、近阈值回退除法；`scale` 仍在 `layer/adam.rs` 行外更新；
  旧 `fused::backward + layer::adam_update` 只留测试对拍，不再 `pack()`）。
- 读数（4096×576，单线程，3 步/轮，25 轮取最快；SGD/Adam 都不含 `pack`）：SGD 标量
  **28.8–29.1 ms/step**，融合前向 **0.81–1.14 ms/step（27–35×）**；`pack()` 参考
  **0.46 ms/step**（训练不再调用，只出双导轨）。Adam 逐元素参考 **8.0–8.2 ms/step**，
  融合 **1.70–1.85 ms/step（约 1.32×）**；同轮 A/B 旧两遍
  **2.12–2.48 ms/step → 1.70–1.85（中位 2.33→1.76，约 1.32×）**。
- Adam 单遍（第二十九轮，同轮交替 A/B 5 组，9 轮取最快）：旧两遍
  `fused::backward` + `layer::adam_update` **2.12–2.48 ms/step** → 单遍
  `fused::adam_step` **1.70–1.85 ms/step（中位 2.33→1.76，约 1.32×）**；
  `dX`/latent/scale/m/v 在 6 形状 × 3 STE × 3 步下逐位相等。
- 带梯度 SGD（第二十八轮，同轮交替 A/B 5 组，9 轮取最快）：旧两遍
  `fused::backward` + `dense::sgd_update` **0.92–1.32 ms/step** → 单遍
  `fused::backward_step` **0.68–0.76 ms/step（中位 0.98→0.71，约 1.38×）**；
  `dX`/latent/scale 在 7 形状 × 3 STE 下逐位相等。
- 多形状（`shape_bench`，9 轮取最快，3 步/轮，SGD/Adam 都不含 `pack`）：SGD 4096×576
  **0.90–1.00 ms / 25.9–27.1×**，17×576 **27–30×**，11×130 **19–20×**，9×65
  **14.5–15.3×**，5×9 **3.0–4.9×**；Adam 4096×576 **4.09×**，17×576 **6.05×**，
  11×130 **4.48×**，9×65 **2.93×**。`1×1` 不计入证据；`9×65` 的绝对耗时只有 0.2–0.4 µs，倍数会抖。
- 反向融合（第二十七轮）：`fused::backward` 复算 example 旧 `pack()+input_grad+row_dot`
  **0.747–0.759 ms** → 新 **0.430–0.441 ms（1.72–1.74×）**，`dX` 576/576、`qdot`
  4096/4096 逐位相等。
- `row_dot`（第二十五轮）：4 个固定累加器替代 `acc[j & 3]` 动态索引，独立探针
  4096 行 **0.2444→0.1428 ms（1.71×）**；它现在只服务 `pack()` 的消费方与标量对拍，
  不参与训练/前向热路径。
- Soft 门控（第三十轮，同轮交替 A/B 7 组，高优先级）：`train_step` **0.80→0.77
  ms/step（1.04×）**，`train_step_with_grad` **0.62→0.58（1.05–1.07×）**，Adam
  **1.73→1.69（1.02–1.03×）**；三条 AVX2 路径省掉 `ordered` 掩码，有限输入逐位不变。
- `train_step` 逐行融合（第三十一轮，同轮交替 A/B 7 组，高优先级）：**0.83→0.67
  ms/step（1.24×）**；拆分 `dense/row.rs`、`fused/row.rs` 后 5 组 **0.88→0.69
  （1.27×）**，`shape_bench` 4096×576 **0.7925→0.7007（1.13×）**；latent/scale/loss
  在 6 形状 × 3 STE × 3 步下逐位相等。
- Adam `q` 符号阈值（第三十二轮，同轮交替 A/B 7 组，25 轮取最快，高优先级）：
  **1.72→1.61 ms/step（1.07×）**，6/7 组更快；`shape_bench` 4096×576 **1.7023→
  1.5682 ms（1.09×）**，7/7 组更快。量化除法只在阈值 ±2^-12 带内或 NaN 回退，
  `dX`/`qdot` 累加顺序不变；m/v 的三次除法 + `sqrt` 仍未改。
- `q` 阈值复用（第三十三轮，同轮交替 A/B 7 组，25 轮取最快，高优先级）：
  `train_step` **0.63→0.58–0.60 ms/step（1.05–1.09×）**，带梯度 SGD **0.59→
  0.47–0.49（1.18–1.25×）**；`shape_bench` 4096×576 SGD **0.5319→0.4336
  （1.227×）**，7/7 更快；Adam **1.6046→1.5966（1.005×）**，属噪声内。
  两条内核只在阈值带内/NaN 回退 `exact`，`fused/tests/threshold.rs` 对拍 ±0.5 边界。
- 边界：这是单层单样本的端到端口径，不含层构造/输入生成；`forward()`、SGD
  `train_step`、`train_step_with_grad`、Adam 四条路径都不再调用 `pack()`，`pack()`
  只保留为公开检查/对拍入口；`train_step_with_grad_adam_scalar` 仍走 `pack()` 作为
  逐元素参考。页面若显示 `q` 内存，必须按“首次访问后才分配”解释。
- 证据：`docs/ERRORS.md` L172/L173/L174/L175/L176/L177/L178/L179/L180/L181/L182/L183/L184/L185、`.agent/checks/registry.json`
  C082/C083/C084/C085/C086/C087/C088/C089/C090/C091/C092/C093/C094/C095、`cargo test -p trit-train` **40/40**。

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
| `TernaryLinear::pack()` | 当前 latent + scale | 量化一次并打包双导轨 | 训练/前向不调用；公开检查/对拍用；`q` 惰性 |

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
      │  fused::backward（Adam）/ backward_step（带梯度 SGD）：每步一次量化（3 个 q 掩码）
      ▼
q ∈ {-1,0,+1}（不物化 i8；训练路径也不建双导轨）
      │  对第 r 行取 coeff = dy[r] * scale[r]；同时累加 qdot[r] += q·x
      ▼
每 8 列：符号掩码选 x，AVX2 8 个 f32 lane 同时加/减 coeff
      ▼
4 个 dX 累加器（r & 3）+ 4 个 qdot 累加器（j mod 4）→ dX[c] / qdot[r]
      ▼
backward_step 分支：同一遍再写 STE dW 与 scale[r]
```

`PlaneMatrix::input_grad` 的旧位平面路径仍在，供 `pack()` 公开 API 与标量对拍；
它的数据流才是“`pack()` → 双导轨 → 每 8 列取 1 字节掩码”。训练步已不再走这条。

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
训练步（第三十一轮，单独区块）：SGD `train_step` 走 `fused::forward_step` 逐行融合（`forward_dot_row` 量化+点积 → 该行 MSE → `dense::sgd_update_row` 的 STE `dW`+scale，一遍且无临时 `qdot/y/dy`）、`forward()` 走 `fused::forward_dot`、Adam 反向走 `fused::adam_step`（量化+dX+qdot+latent/m/v 一遍，scale 行外更新）、带梯度 SGD 走 `fused::backward_step`（dX+qdot+STE dW+scale 一遍）、pack() 只保留公开对拍（训练不调用）、q 惰性解码
平台：原生 x86_64 AVX2；非 x86_64 或 cols < 8 自动回退标量
未证明：端到端多层吞吐、GPU/WASM 读数、多形状 Adam 曲线；SGD/forward/with_grad/Adam 都不再 pack，pack() 仅参考
```

### 6.4 状态区

```text
后端：dX available（main 6b910bc9，PR #664 已合并）
      训练步 available（main 9d1f0cc6，PR #666 已合并；区间 #667）
      Adam dW 融合 + fused backward + 带梯度单遍 + Adam 单遍 available（PR #668，c64fe0c7/aa92dd1b/a62dd419/cd5e9a00/ba217795；open）
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

# 4. 训练步读数（第二十四至二十五轮）
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
python tools/checks.py -Only C087
python tools/checks.py -Only C088
python tools/checks.py -Only C089
python tools/checks.py -Only C090
python tools/checks.py -Only C091
python tools/checks.py -Only C092
python tools/checks.py -Only C093
python tools/checks.py -Only C094
python tools/checks.py -Only C095
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
| `dW` 位平面化 | 未做；SGD `train_step` 走 `fused::forward_step` 的逐行融合（`dense::sgd_update_row`），Adam 走 `fused::adam_step` 的融合外积/单遍，不是位平面 | 不能写“dW 是位平面” |
| `forward` 快速内核 | 已改 `fused::forward_dot`（第二十六轮），不再构造双导轨 | 可写“前向走量化点积单遍”，不能写“所有形状都快” |
| `pack()` 成本 | 已单独计 0.46 ms/step（只写双导轨，按 64 列 word 写合并）；训练/前向已不调用 | 不能把 dX 的 39.25×当训练步加速 |
| 反向 `dX`+`qdot` | 已改 `fused::backward` 单遍（第二十七轮）；旧 `pack()+input_grad+row_dot` 保留对拍 | 可写“反向 1.72–1.74×”，不能写“端到端训练 1.7×” |
| 带梯度 SGD | 已改 `fused::backward_step` 单遍（第二十八轮），`dX`/`qdot`/STE `dW`/scale 同一遍；旧两遍保留对拍 | 可写“带梯度 SGD 中位 1.38×”，不能写“所有训练步都快 1.38×” |
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
| `pack()` | 把 latent 量化一次并打成 `PlaneMatrix`；训练/前向不调用 |
| GMAC/s | 每秒十亿次乘加；本页按 `rows × cols` 计 |
| 标量参考 | `input_grad_scalar` / `row_dot_scalar`，逐元素参考实现 |
| AVX2 | x86_64 的 256 位 SIMD；本内核一次处理 8 个 f32 lane |

## 附录 B：文件地图

| 路径 | 内容 |
|---|---|
| `crates/trit-train/src/plane/backward.rs` | `PlaneMatrix`、`input_grad`、`input_grad_scalar`、AVX2 内核（对拍/公开 API） |
| `crates/trit-train/src/plane/fused/backward.rs` | 训练实际走的量化 + `dX`/`qdot` 单遍（Adam） |
| `crates/trit-train/src/plane/fused/backward_step.rs` | 带梯度 SGD 的 `dX`/`qdot`/STE `dW`/scale 单遍 |
| `crates/trit-train/src/plane.rs` | `pub use backward::PlaneMatrix`；位平面 ALU 边界 |
| `crates/trit-train/src/layer.rs` | `pack()`（公开检查/对拍）；`fused_backward` 适配 |
| `crates/trit-train/src/layer/grad.rs` | `train_step_with_grad` 与 Adam 标量参考 |
| `crates/trit-train/src/bin/bench.rs` | `bench_backward`；读数输出 |
| `crates/trit-train/README.md` | 人类说明、读数表、边界 |
| `docs/ERRORS.md` L171–L185 | 缺陷台账与读数结论 |
| `.agent/checks/registry.json` C081–C095 | 回归门 |
| `docs/handoff/bitplane-backward-metrics.json` | 前端数据快照 |
