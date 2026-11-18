# WSP 内部逻辑读取能给 driftnet 什么（以及不能给什么）

> 状态：**结论文档**，无代码。上游是 `feat/wsp-internal-logic`（PR #510）与
> `feat/trit-circuit-sequential-aiger`（PR #511）。
> 本文回答两个问题：WSP 的方法能不能一直用；能给 driftnet 什么。
> 所有数字来自 PR #510 的 `WSP-INTERNAL-LOGIC-RESULTS.md`。

## 1. 能不能一直用：不能

M0/M3 的全部判据都建立在**输入可枚举**上：

| 判据 | 依赖 |
|---|---|
| `DIFF=0`（逐串等价） | 穷举完整输入域 |
| 最小 DFA（状态数 = Myhill–Nerode 等价类） | 穷举所有前缀 × 后缀 |
| Tracr 闭环（恢复率 100%） | 穷举 + 编译展开 |

实测边界：

- **域**：预算到 `max_len = 14`（32767 个串），词表 `2^(n+1) − 1` 指数增长；
  再往上成本全在穷举次数上；
- **规模**：20 状态 / 9 转移源，`and_w = 3k + dim_in + c` 线性增长；
- **任务**：提取的是状态机，要求「序列 → 有限状态」形态。

driftnet 三条全不满足：

1. 输入是 32×32×3 连续像素（3072 维），穷举不可能；
2. 任务是「噪声 z → 图像」，不是序列识别；
3. 网络是卷积 + 浮点激活（`F32Conv` / `ConvBn`），不是离散小网。

**结论：这套方法不能一直用，它只回答「小离散网络在算什么算法」这一个问题。**

可以继续搬的只有两条不依赖穷举的部分：

- **逐层干预**：`intervene::sweep(net, max_len, layer_idx, units, encode)` 已通用化到任意层、
  任意单元数，内部用「改一个单元 + 尾部网络重算」，只需要固定样本集；
- **信息瓶颈检查**：同一层激活是否决定输出（`hidden_to_out_conflicts`）。

但结论强度会从「完全等价」退化成「ablation 统计」——这是方法本身的损失，不是实现问题。

## 2. 给 driftnet 的三件审计

先纠正一个预期：**WSP 救不了 CRAM 的可行率问题**。
`CRAM-3D-DRIFT-RESULTS.md` 的九条负结果指向「SWD + hinge 这个目标不奖励可行性」，
那是损失函数/监督的问题，不是读取问题。WSP 套上去不会让可行率从 0 变正。

按真实增量排序：

### 2.1 量化一致性审计（有增量）

driftnet 的 artifacts 里有 `A_f32_dense`（浮点参考）与 `B_trit_dense` / `C_trit_dend`（三值）。
做法：固定样本集（同一批 z seed），比较两者的输出差异，给出：

- 逐样本差异清单（哪些样本被量化改变、改变幅度）；
- 差异是否集中在某类样本上（例如高对比度/低对比度）。

增量：把「三值量化损伤了什么」从 FID 一个总数，变成可定位的清单。
`equiv` 的思路（差异 + 反例）可以用，但**只能抽样，不是等价判定**——3072 维输入无法穷举。

### 2.2 树突分支承重（有增量）

`C_trit_dend` 是树突变体。对每个分支做「清零 / 取反」，统计 FID 或输出变化。
这正是 `intervene` 现在的形态（逐单元翻转率）。

增量：把「树突结构有没有在贡献」从直觉变成逐分支读数；如果某些分支翻转率恒为 0，
那它们就是装饰，可以删。

### 2.3 状态轨迹（不适用）

1-NFE 只有一步前向，没有多步「思考链」可读。L-D 的轨迹层在 driftnet 上没有载体。
只有把生成改成多步迭代（looped / 逐步精化）之后，轨迹才有意义。

## 3. 我的判断：该做的不是 WSP

`CRAM-3D-DRIFT-RESULTS.md` 自己列的下一步里，**E1 掩码解码**是性价比最高的一条：

- 不动训练；
- 可行率由解码保证（不是靠损失函数学出来）；
- 尚未开工。

在它之前还有一件必须先做的：**口径一致性**。训练侧（trit 后端 + batch BN）与评估侧
（burn + running stats）的读数不能直接比——第十条已实测同一份策略在两种口径下
是 9.4%/11.7% vs 0%。不先统一，任何「变好了」都可能只是口径差。


## 5. 真实接入 driftnet 的三道障碍（2026-11-18 查明）

想直接跑「改权重 → 声明域验证」，卡在三处，都已确认到代码级：

| 障碍 | 事实 | 出处 |
|---|---|---|
| artifacts 格式不匹配 | `artifacts/*/ckpt.pt` 是 PyTorch checkpoint；`Gen::from_packed` 要 `DPK1` | `format.rs::read_packed` |
| f32 模型加载不了 | `conv_bn()` 强制 `tern_mat`，`ConvBn.w: TernMat`；`A_f32_dense` 的 conv 是 f32 | `model.rs::from_packed` |
| 无 `lib.rs` | `crates/driftnet` 只有 bin，写不了 `examples/` | `crates/driftnet/` |

结果：**最多只能接 `B_trit_dense`**（三值 conv），`A_f32_dense` 作为浮点参考走不进 `Gen`。

### 转换脚本的最小设计（`tools/fixtures/pt_to_dpk1.py`）

1. `torch.load("ckpt.pt")` 取 state_dict（torch 2.15 已装）；
2. 键名对齐 `from_packed`：`fc.weight/bias`、`net.{0,1,2}.{1,2}.*`、`net.3.weight/bias`；
3. conv/fc 的三值权重按 2-bit 编码（`Trit::POS=1, NEG=2, ZERO=0`），`scale` 取每行
   absmean（与 `trit-learn::quant` 同口径）；
4. 按 `DPK1` 写出——`format.rs::read_packed` 是格式的权威来源（`magic + ch + zdim +
   ternary_fc + n + [name, kind, ndim, dims, ...]`）。

估计 100 行，与 `tools/fixtures/prepare_cifar.py` 同位置、同性质（一次性夹具，不参与训练）。


## 6. 第一个真实读数：改一个权重，声明域完全验证

**先纠正一个前提**：`artifacts/ch1024_20k/weights.bin` 的 magic 是 **`QDMP`**（f32 tensors），
不是 `.pt`——所以 §5 的 `pt_to_dpk1.py` **不需要**，`qat select --dump` 直接能加载它。

实验（一次性夹具，未入库）：

1. 用 QDMP 解析器把 `net.3.bias[0]` 从 `-0.03165` 改成 `-0.02165`（`+0.01`），写出 `weights_mod.bin`；
2. 同一批 64 个 z（`--seed 0 --goal 63`）各跑一次 `qat select --dump-out`；
3. 逐样本比较 3072 维输出。

读数：

```text
samples=64  changed=64  max_abs=0.009999
```

- **changed = 64/64**：改动传播到每一条样本；
- **max_abs = 0.009999**：最大差异约等于 delta（0.01），线性传播，没有放大或抑制。

这是「声明域完全验证」（`declared::verify`）的第一个真实读数——不是估计，是 64 条全部跑过。

**但它是正向读数**（改了一定会变）。真正的强判据是**反向**：改一个「应该无关」的权重，
`changed == 0`。那需要先找出死单元（ReLU 后恒 0 的通道），留作下一步。

## 4. 未证栏

- 没有在 driftnet 上实际跑过任何一条审计；本文只有方法映射，没有读数。
- §5 的三道障碍已确认到代码级；但 §6 发现 `weights.bin` 是 `QDMP`，`pt_to_dpk1.py` 对
  这条路径**不需要**——转换脚本只对 `.pt` 走 `DPK1` 的场景有意义。
- §6 只有**正向**读数（改一定会变）；反向（改无关权重 → `changed == 0`）**未做**，
  需要先分析死单元。
- 「量化差异集中在某类样本」是假设，未验证。
- `intervene` 用在卷积层需要先决定「单元」是通道还是空间位置，本文没有定。
- E1 的可行率保证是 CRAM 文档的推断，本文未复核其推导。
