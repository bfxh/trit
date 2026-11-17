# CRAM + 3D 漂移：真条件生成（M0c-COND）

> 状态：**已实现、已训练，仍是负结果**。读数见
> [`CRAM-3D-DRIFT-RESULTS.md`](CRAM-3D-DRIFT-RESULTS.md) 第六条：5k 步后可行率 **0%**，
> 最优能量 50 → 20（`(0,0)` 首次点亮，8 格连通 + 10 格孤立）。
> 结论：**结构不是瓶颈，目标函数才是**——损失里没有 goal / 能量 / 连通项，条件进不进网络都学不到。

## 0. 目标

把终点（goal）做成生成器的**独立输入通道**，而不是塞进 `z` 的若干维：

- 输入 `[z(128), goal_onehot(64)]`，`fc` 宽 `128 + 64 = 192`；
- 验收（未达成）：5k 步后固定 goal=(7,7) 采样 256 个，可行率 ≥10%（当前 0%）；
- 回归（达成）：`cond = 0` 时无条件训练行为逐位不变。

## 1. 实际改动（比设计便宜得多）

原设计预计要动 `trit-grad` 的前向/反向/权重形状。实际只动了 `crates/qat` 两个文件 + 一个拆分模块：

| 文件 | 改动 |
|---|---|
| `crates/qat/src/train_trit.rs` | `Cfg.cond`（`--cond N`，默认 0）；`init_gen` 里 `GenP::zdim = zdim + cond`、`fc_w/fc_b` 按总宽度初始化；`z_sample` 把 goal one-hot 写到 z 尾部（`cond == 0` 保持老路由）；`meta.json` 记 `cond` |
| `crates/qat/src/main.rs` | `probe` / `select` 从 `meta.json` 读 `cond`，`Gen::new(128 + cond)`，one-hot 写同一位置，DPRB 探针头 zdim 同步 |
| `crates/qat/src/gen_qdmp.rs`（新） | `dump_gen` / `load_gen` 从 `train_trit.rs` 拆出（856 → 769 行，`god_file` 门禁只准减）；`load_gen` 补 fc 宽度断言 |
| `crates/trit-grad` | **一行没改**：`forward_g` / `backward_g` / 三值打包 / 归一化核都只认 `GenP::zdim`，把总宽度放进去就够 |
| `crates/driftnet` | 不用改：`format::pack` 的 zdim 从 `fc.weight` 形状推，`GpuGen` 按同一个数算 |

兼容性：`cond` 缺省 0 → 老模型、老读数照旧；`cond > 0` 的 `weights.bin` 是 192 宽，`load_gen`
会显式报错而不是静默截断。

回归证据：`cond = 0` 与 HEAD 二进制、同一条命令（200 步、ch=1024、ternary+ternary_fc、seed 0）
各跑一遍，两份 `log.jsonl` 除 `sec` 计时外逐位相同。

## 2. 为什么还是 0%

1. **训练时 goal 恒定**：`--goals` 给的 1024 个字节全是 63，所以那 64 维输入在所有样本上
   是同一个常量图案，梯度只会把它塑形成**常量偏置**，没有任何「按 goal 变化」的压力。
2. **损失与 goal 无关**：目标是 patch SWD + hinge 拟合 CIFAR 分布，`traj_energy` 只在
   `select` 里当裁判用。没有一项梯度说「要连到 goal」。
3. 权重侧核对（`artifacts/traj_cond64/weights.bin`）：64 个 cond 列 rms 0.042，128 个 latent 列
   0.0398，三值 0 化率 24% / 26% —— **通路没被训死**；但换 goal 只让 `fc` 输出变化 0.077，
   而潜变量 128 维的总贡献是 0.45（17%）。通路是通的，只是没被要求携带信息。

换结构确实改变了分布（能量 50 → 20、起点第一次点亮），但改不出「按条件生成」。

## 3. 下一步（都在目标函数侧，不在结构侧）

- **多 goal + 能量可微项**：训练时 goal 逐样本采样（不再恒定 63），把「起点亮 + 点亮量」
  做成可微项并扫权重；连通性仍不可微，可微惩罚 #4 已显示会被 SWD 主导。
- **教师轨迹回归**：用 `benchmarks/cram-drift-2d` 的 BFS 参考造「(起点, goal) → 路径」监督对，
  条件回归替代分布匹配。这才是真正测「条件能不能学会」，代价是离开「从零无监督训练」。
- **收尾 M0**：六条负结果作为结论——在 driftnet 现有「SWD + hinge」目标下可行率恒为 0。

## 4. 未证栏

- 只测了单 goal（训练时全 63）的一次 5k 步训练。这次实验**只证明「加一条独立通道会改变样本
  分布，但不会带来条件性」**，没有证明「这个结构学不会条件」——要证后半句必须先有多 goal 的
  监督或损失。
- 没有做 FiLM / AdaIN 条件归一化的对照；没有调学习率、宽度、步数（20k 未跑）。
- 没有 3D、没有多模态；能量仍是手写启发式，不是 CRAM 原文的 Hopfield 能量。
- 代码在 `/d/KF/driftnet` 的 `main`（本地提交，不在本仓）；读数产物在
  `artifacts/traj_cond64/`（`weights.bin` + `log.jsonl` + `meta.json`）。
