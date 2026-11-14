# 行为级硬件仿真 × 设备适配策略：CPU / GPU / NPU 计划选择（2026-11-14）

> 状态：**M0 仿真 + 学习策略机制验证 + 本机 GPU 实测校准**。主体读数来自
> 行为级模拟器与合成设备档案；§5.1 补上本机 RTX 4060 Ti + CPU 的真实采集，
> 但仍是**单机、少量形状**，不构成多机迁移结论。

## 1. 一句话

给一次三值矩阵乘，模拟器按设备档案（有效峰值/带宽/缓存/启动开销/并行单元/
占用率）估算 8 个内核计划的延迟；用 `trit-learn` 的 QAT 三值分类器从
「设备档案 + 形状」**零 shot** 选计划，和穷举模拟 oracle、按类静态最优、
手写规则、随机对照。目标不是证明某个计划最快，而是验证：
**不做逐设备 profiling，只凭档案能否逼近 oracle，并能被更多训练设备增强。**

## 2. 行为级模型（`examples/device_adapt/`）

- 设备档案：`device.rs`。CPU / GPU / NPU 三类；字段为有效吞吐（trit/int4/f32
  MAC/s）、带宽、cache、启动 µs、并行单元、占用率。**非周期级**：不模拟
  流水线、缓存行、指令发射。
- 计划：`plan.rs`。8 个计划 = CPU u64 / CPU SIMD / GPU naive / GPU tiled(16×16) /
  GPU reg 2×2(32×32) / GPU split-K4 / NPU int4(16×16) / GPU f32 tiled。
- 延迟：
  `max(计算时间, 内存时间) / (设备占用率 × 计划占用率 × 并行利用率) + 启动开销`。
  内存流量在工作集超过 cache 时按 tile 重复读；split-K 计入部分和与额外启动。
- 可行性：设备大类 + 精度支持硬 mask；学习只负责同类计划间的取舍。

## 3. 锚点与校准

- GPU 锚点 `gpu_rtx4060ti_anchor` 由 `trit-gpu` 真机读数反推
  （512×4096×512：ternary reg2x2 **0.129 ps/MAC**、tiled 0.208、naive 1.60；
  f32 tiled 2.641）。峰值即实测最好内核的**有效吞吐**，计划占用率不再二次折减。
- 构造性一致性（模型/实测）：reg **1.007**、tiled **1.005**、naive **0.996**、
  f32 tiled **1.000**。这是常数反推的必然结果，**不是独立验证**。
- CPU 锚点用 trit-dot 的 u64 读数（7.6–12.5 ps/trit）与假定的 SIMD 增益；
  NPU 锚点为纯合成档案。其余训练/测试设备全部由 log-uniform 采样生成。

## 4. 协议

- 训练设备：48/类（CPU/GPU/NPU 各 48）+ 训练形状 8 个 = **1152 样本**；
  搜索/标签都在模拟器内完成，无真机 profiling。
- 测试设备：24 个留出采样设备（8/类）+ **7 个 OOD 锚点**（含真机校准的 4060 Ti、
  数据中心/弱 popcount/边缘 NPU 等）；测试形状 8 个，与训练形状不重叠。
- 策略：QAT 三值 MLP（21 维特征 → 32 tanh → 8 类，每步 `zero_grad`，120 epochs）；
  标签 = oracle 计划。两个模型：`learned_clean`（干净标签）与
  `learned_noisy`（训练时设备参数 ±10% 抖动）。
- 对照：oracle（穷举模拟）/ `class_fixed`（每类一个静态最优计划）/
  `heuristic`（只看大类 + 形状的手写规则）/ `random`。
- 指标：`mean_ratio = 选中计划延迟 / oracle 延迟`（1.0 = 最优）、p90、exact
  （≤1+1e-9）、within_10（≤1.10）。3 个搜索种子（7/11/23）平均。
- 测试条件：`sim_clean` / `sim_noise`（留出采样设备，后者参数 ±10% 抖动）与
  `anchor_clean` / `anchor_noise`（OOD 锚点）分开报。

## 5. 读数（3 seeds 平均，train-per-class=48）

| 条件 | 策略 | mean | p90 | exact | within10 |
|---|---|---:|---:|---:|---:|
| `sim_clean` | **learned_clean** | **1.011** | **1.000** | 0.939 | 0.976 |
| | **learned_noisy** | **1.010** | **1.000** | **0.945** | **0.979** |
| | class_fixed | 1.034 | 1.042 | 0.899 | 0.936 |
| | heuristic | 1.130 | 1.569 | 0.740 | 0.783 |
| | random | 2.378 | 3.349 | 0.585 | 0.610 |
| `sim_noise` | learned_clean | 1.010 | 1.001 | 0.932 | 0.976 |
| | **learned_noisy** | **1.008** | **1.000** | **0.939** | **0.985** |
| | class_fixed | 1.029 | 1.049 | 0.894 | 0.943 |
| | heuristic | 1.125 | 1.566 | 0.742 | 0.783 |
| `anchor_clean` | **learned_clean** | **1.069** | **1.021** | **0.917** | **0.934** |
| | learned_noisy | 1.094 | 1.463 | 0.881 | 0.911 |
| | heuristic | 1.077 | 1.331 | 0.768 | 0.839 |
| | class_fixed | 1.147 | 1.697 | 0.839 | 0.857 |
| `anchor_noise` | **learned_clean** | **1.068** | **1.022** | **0.905** | **0.940** |
| | learned_noisy | 1.107 | 1.458 | 0.863 | 0.911 |
| | heuristic | 1.077 | 1.322 | 0.786 | 0.851 |
| | class_fixed | 1.172 | 1.706 | 0.821 | 0.857 |

- **同分布设备**：学习策略基本贴住 oracle（mean 1.01，p90 1.00），明显优于
  按类静态（1.034）与手写规则（1.130）；种子里 3/3 胜 heuristic。
- **OOD 锚点**：learned_clean 均值 1.069 略优于 heuristic 1.077，但**种子相关**
  （seed 7 为 1.110 输给 1.077；seed 11/23 为 1.048 胜）；p90 1.021 远好于
  heuristic 1.331。结论是「互有胜负」，不是全面胜出。
- **训练规模消融**（同测试集）：24/类 → 48/类，
  `sim_clean` 1.031→1.011，`anchor_clean` 1.112→1.069（learned_clean）；
  训练集准确率 0.947→0.953。**更多设备样本确实增强适配，但收益集中在 OOD 尾部**。
- **噪声增强不可靠**：n=48 时 `learned_noisy` 在 sim 上与 clean 持平
  （1.010 vs 1.011），在 anchor 上反而更差（1.094 vs 1.069）。n=24 时方向又
  翻转。结论：**±10% 噪声副本不是稳定的增强手段**（负结果）。
- **计划支配**：3 个测试集的 oracle 计数合计 `[0,240,2,21,179,0,240,62]`；
  `gpu_split_k4`/`cpu_u64` 在留出集为 0，`gpu_naive` 仅 2（大形状 + 低带宽
  单例）。有效动作集收缩到 `cpu_simd / gpu_tiled / gpu_reg / gpu_f32 / npu_int4`。
- **残余坏例**：弱 popcount GPU 锚点上策略仍选 ternary reg（2.2–2.8×，oracle 要
  f32）；另有 1 个 4096³ 低带宽 GPU 单例 oracle 是 naive、策略选 reg（3.4×）。
  p90 1.02 说明这类是尾部而非普遍现象。

## 5.1 真实设备校准（本机，2026-11-14）

`crates/trit-gpu/examples/device_calibrate.rs`（需 `--features gpu`）：

- CPU：`popcount_words`（SIMD 后端 **popcnt**，24 逻辑核）单线程 **38.5e9
  trit/s**、`dot_words` 19.9e9、f32 点积 2.69e9；128MB 单线程拷贝
  **22.2 GB/s**（共享带宽的下界）。档案：ternary 924e9、f32 64.6e9、
  bandwidth 22.2GB/s；cache=32MB、launch=0.5µs、occupancy=0.9 为假设。
- GPU：**NVIDIA RTX 4060 Ti / Vulkan**；`nvidia-smi` 8.586GB 总显存 /
  1.148GB 已用；wgpu copy 带宽 **232–249 GB/s**（多次运行）；34 SM 为查表。
  校准形状（512×4096×512）三值最优 **reg2x2 0.113–0.122 ps/MAC**，
  split-K 0.148–0.155、tiled 0.146–0.157、naive 0.851–0.854；f32 tiled
  **0.817–0.828 ps/MAC**。档案：ternary_ops **8.2–8.8e12**、f32_ops
  **1.21–1.22e12**、occupancy=1（峰值即实测有效吞吐）。
- 模型保真（model/measured，越接近 1 越好；含多次运行）：
  - 校准形状：**1.056–1.068**（合成锚点 1.87–1.94）；
  - 768³：**1.05–1.23**（合成 2.00–2.21）；
  - 320³：**0.44–0.51**（合成 0.80–0.88）——小形状仍被显著低估，
    启动/占用项需要下一轮校准；
  - 64×2048×2048：0.70–0.76（合成 1.33–1.39），且模型 best 恒为 reg，
    真机 best 是 **split-K**。
- 真机计划延迟（seed 7 一次运行）：512×4096×512 reg **0.121ms**；
  768³ reg **0.066ms**；320³ reg **0.025ms**；64×2048×2048 split-K
  **0.050ms** vs reg 0.056ms。注意 320³ 附近计划差距 <10%，运行间噪声
  会让 best 在 reg/tiled 之间翻转。
- 策略真机零 shot（合成设备训练，实测延迟评估；3 个策略种子）：
  - `learned_clean` 均值 **1.027 / 1.032 / 1.024**，exact 0.75 / 0.75 / 0.50；
  - `heuristic` 均值 **1.056 / 1.065 / 1.057**，exact 0.50 / 0.50 / 0.50；
  - `class_fixed` / 默认模型 oracle 与 learned 相同（本机都选 reg）。
  - learned 在 512×4096×512、768³、320³ 上多数命中；在瘦长
    64×2048×2048 上固定选 reg，损失 **0.7%–12.9%**（真机 best 是 split-K）。
    heuristic 会选 split-K，但会在 768³/320³ 上选 tiled，单次损失可达
    19.5%。另一次同 seed 运行 heuristic 1.013 < learned 1.020，**排序会被
    测量噪声翻转**；两者互有胜负，不能宣称学习策略全面胜出。
- **关键发现**：合成训练里 split-K 在留出集 0 次获胜（被 tiled/reg 支配），
  但真机瘦长形状上 split-K 最优——这正是需要真实校准的原因。下一步应把
  校准档案加入训练，或给 split-K 建形状相关特征；本轮未做（避免用一台
  机器的 4 个形状反向拟合策略）。

## 6. 过程修正（对后续有用）

1. **第一版特征没有比值**：策略在弱 popcount GPU 上把 ternary reg 选成 5–7×
   误差，整体退化成 `class_fixed`。加入 ternary/f32、ops/带宽、cache/字节
   三个显式比值后（21 维特征），整体（采样+锚点）mean 1.195→1.055（seed 7，
   24/类，拆分测试集之前的读数）。
2. **锚点不能二次折减占用率**：GPU 峰值已经是实测最好内核的有效吞吐；
   第一版再乘计划 occupancy=0.9 让 reg 校准点偏 1.118×。改为锚点/GPU 计划
   不额外折减后，校准比回到 1.00 附近。
3. **训练/测试采样 RNG 分开**：否则改 `train-per-class` 会连带改变测试设备，
   训练规模消融无法同集对比。
4. **`zero_grad` 每步清零**（与 `trit-count` 的同一条教训）：`TernaryDense::update`
   不清累积梯度，漏掉会让逐样本 SGD 变成跨样本梯度累积。

## 7. 未证栏与边界

- **仿真不是真机**：主体设备参数是合成的；§5.1 只校准了本机 RTX 4060 Ti
  + CPU 一次，CPU/NPU 的完整计划延迟、多 GPU、多机迁移均未验证。
  校准档案写盘（`--out`）了，但 `device_adapt` 尚未接 `--profile` 消费它。
- 只做**单算子 matmul**；DAG 融合、流水线、多设备异构调度、功耗/热约束未做。
- 计划集固定 8 个，未做 tile/精度/线程数的细粒度 autotuning。
- 校准只有 1 个 GPU 形状点（512×4096×512），CPU/NPU 没有实测锚点。
- 策略是 21→32→8 的小 MLP；序列模型/图模型/在线自适应未试。

## 8. 复现

```bash
# 默认：48 设备/类，120 epochs，测试噪声 ±10%
cargo run --release -p trit-gpu --example device_adapt -- \
  --seed 7 --train-per-class 48 --epochs 120 --hidden 32 --noise 0.10

# 训练规模消融：24 设备/类（同一测试集）
cargo run --release -p trit-gpu --example device_adapt -- \
  --seed 7 --train-per-class 24 --epochs 120 --hidden 32 --noise 0.10

# 真实设备校准（需要 GPU；会跑本机 4 个形状的全部内核）
cargo run --release -p trit-gpu --features gpu --example device_calibrate -- \
  --seed 7 --out device-profile.json
```

每行一个 JSON：`anchor` / `calibration` / `plan_wins` / `train` / `eval` /
`worst` / `final`。3 seeds 的读数用 `--seed 7/11/23` 重复后按 `policy+condition`
聚合；测试集由独立种子生成，改训练规模不影响测试设备。
