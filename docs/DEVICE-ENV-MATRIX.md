# 多环境 / 多容器验证矩阵（2026-11-14）

对同一份 `feat/device-adapt` 代码，在原生宿主、WSL Linux 与两个真实容器
（2 CPU / 8 CPU cpuset）里运行：

- `device_adapt`（合成设备仿真 + QAT 策略，64 行 JSON）；
- `device_env_check`（OS/架构/核数/SIMD 后端 + CPU 完整计划延迟 + NPU 模型值）。

**边界**：这些环境共享同一台物理机（原生 Windows / WSL / Docker Desktop），
不是跨硬件；矩阵验证**可移植性与环境检测**，不做跨机器性能结论。

## 1. 结果

| 环境 | 类型 | adapt | status | 核数 | SIMD 后端 | rustc |
|---|---|---|---:|---:|---|---|
| native Windows | host | 64 行 | **PASS** | 24 | popcnt | 1.99.0 |
| WSL Ubuntu-24.04 | Linux | 64 行 | **DRIFT** | 24 | popcnt | 1.98.0-nightly |
| container `cpuset=0-1` | Linux 容器 | 64 行 | **DRIFT** | **2** | popcnt | unknown（scratch 镜像无 rustc） |
| container `cpuset=0-7` | Linux 容器 | 64 行 | **DRIFT** | **8** | popcnt | unknown |

- **结构不变量通过**：4 个环境都是 64 行；前 13 行（7 个 anchor + 4 个
  calibration + 2 个 plan_wins）**逐位一致**，`plan_wins` 计数完全一致
  （train `[13,371,6,51,294,0,384,33]`，test `[0,80,0,2,72,0,80,14]`）。
- **策略训练/评估漂移**：WSL 与两个容器相对 native 的最大指标差 **0.794**
  （`worst` 样本 ratio 1.951 vs 1.157）；训练集准确率 native 0.963 vs
  Linux 0.931。原因混合了 OS、Rust 版本（1.99 vs 1.98 nightly）与训练浮点
  路径；**仿真前置阶段可复现，训练后的策略读数不可 bit 复现**。
- **容器之间一致**：两个容器用同一 Linux 二进制，`device_adapt` 输出
  **逐字节一致**（`cmp` 通过），说明 cpuset 配额不影响合成策略；差异只在
  环境检测的 CPU 计划延迟上。
- **环境检测有效**：容器分别报 `cpus=2/8`；512×4096×512 上 CPU 完整计划
  延迟（24 线程 → 2/8 线程）：

  | 环境 | cpu_u64 | cpu_simd |
  |---|---:|---:|
  | native Windows（24） | 6.707 ms | 17.655 ms |
  | WSL（24） | 6.163 ms | 12.074 ms |
  | container（2） | 34.231 ms | 56.578 ms |
  | container（8） | 10.039 ms | 17.693 ms |

## 2. Docker 网络限制与本地镜像

本机 Docker Hub 直连超时，镜像站拉取也不完整；默认 `rust:1.100-bookworm` /
`rust:1.88-bookworm` 容器本轮**未跑成**。为完成真实容器验证，用 WSL 编译的
Linux 二进制 + glibc loader/libs 构建了本地 scratch 镜像
`trit-device-env:local`（4.9MB），再用`--cpuset-cpus` 起两个容器。

- 官方镜像模式仍保留：网络可达时
  `python scripts/device_matrix.py --docker-image rust:1.100-bookworm` 会
  自动 pull + 运行；
- 本地镜像构建步骤见 `containers/device-env/README.md`；
- 本地镜像无 shell/rustc，因此 `device_env_check` 的 rustc 字段为 `unknown`；
  这是检测结果的一部分。

## 3. 复现

```bash
# 原生 + WSL + 两个本地容器（不进 Docker Hub）
python scripts/device_matrix.py \
  --skip-docker \
  --docker-local-image trit-device-env:local \
  --docker-local-cpusets 0-1,0-7 \
  --out matrix.json

# 网络可用时：官方 Rust 镜像（1.100 + 1.88）
python scripts/device_matrix.py \
  --docker-image rust:1.100-bookworm \
  --docker-image rust:1.88-bookworm \
  --out matrix-official.json
```

`device_matrix.py` 输出 PASS / DRIFT / FAIL：

- **PASS**：64 行、前置阶段与指标全部一致；
- **DRIFT**：结构/前置阶段/plan_wins 一致，但训练后指标有差异（本矩阵的
  WSL/容器都是此类）；
- **FAIL**：退出码非 0、行数/结构/前置阶段/plan_wins 不一致。

## 4. 结论与下一步

1. **仿真器与策略接口跨环境稳定**：设备采样、计划覆盖、plan_wins 在
   Windows/Linux/容器完全一致。
2. **训练不是跨编译器可复现的**：若要把读数当硬契约，需要固定点/整数训练，
   或给 DRIFT 设明确容差；当前矩阵选择显式报告而不是掩盖。
3. **容器隔离检测可用**：cpuset 2 vs 8 被正确识别，CPU 计划延迟随配额变化；
   容器间同一二进制输出一致。
4. **未证**：GPU 容器（nvidia runtime 存在但未跑 wgpu 容器）、不同 CPU 架构
   （ARM64）、不同发行版、跨物理机。下一步是 GPU 容器与 ARM64 矩阵。
