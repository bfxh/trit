# trit 只读审计报告（2026-10-25，基于 `582b6c6`）

> 来源：外部只读审计（4 个并行分片 + OrganizerAgent 回读交叉核对），基线
> `582b6c623df1dd34ab31a2a16a19fd9c9bdc703d`（v0.12.0）。本文是**核实版**：
> 保留全部可执行条目，并标注在当前 main 上的复核结果与修复位置。
> 原报告全文（含代码片段与触发探针）由提交者保留，不在仓库重复。

## 复核结论（2026-10-25，当前 main）

| 条目 | 原级别 | 当前是否仍存在 | 处理 |
|---|---|---|---|
| H1 `trit-float::from_f64` 低精度双舍入 | High | **是**（实测 `from_f64(5.0,1)=9.0`） | 已修 |
| M1 `decode_rgba_baseline` 调色板索引 3 越界 panic | Medium | **是** | 已修 |
| M2 `TernaryMemory::encode/decode` 无长度校验、`i32` 累加 | Medium | **是** | 已修 |
| M3 `trit topology` 节点数无上限 → OOM | Medium | **是** | 已修（CLI 层） |
| Low A1–A4 / B1–B4 / C4–C5 / D2–D6 | Low | 未逐条复核 | 未动（见下表） |

## 已修四项

### H1 · `crates/trit-float/src/lib.rs`（`from_f64`）

- 原问题：先 `round(x·3^p)` 再交给 `normalize` 二次舍入；低精度下尾数越界（`5.0/p=1` 得 `2`，
  超出 `p=1` 的合法区间 `±1`），normalize 又按“更大指数”进位，最终系统性偏大（`5.0 → 9`）。
- 修法：在 `e−(p)..=e+2` 上枚举候选指数，尾数 `clamp` 到 `±(3^p−1)/2`，按绝对误差取最近；
  误差相同（含浮点噪声容差）取更大指数，与 `normalize` 的进位习惯一致。只做一次舍入。
- 回归：`crates/trit-float/tests/from_f64_regression.rs`
  （`5.0/1→3`、`-5.0/1→-3`、`1.5/1→1`、`4.5/1→3`、`2.0/1→3`、`5.0/2→6`、`0.5/1→1/3`；
  以及“尾数不超 limit”和“相对误差不超 `3^{1-p}`”两条性质测试）。

### M1 · `crates/trit-texture/src/lib.rs`（`decode_rgba_baseline`）

- 原问题：`palette_index` 只 `& 0x3`，可取值 3，而 `palette` 只有 3 色 → `palette[3]` panic。
- 修法：新增 `TextureError::InvalidPaletteIndex { block, index }`，改用 `palette.get(...)`，
  越界返回错误。
- 注：正常编码路径只写 0/1/2；该缺陷只对外部构造的 `BinaryTexture` 可达。

### M2 · `crates/trit-attn/src/memory.rs`（`TernaryMemory`）

- 原问题：`encode`/`decode` 不校验长度（`value.len() > dim` 下标 panic、`query.len() < dim` panic），
  `weights += value*key` 是 `i32` 累加无溢出保护。
- 修法：新增 `MemoryError { Dimension, Overflow }`；`encode`/`decode` 返回 `Result`；
  累加改 `checked_mul`/`checked_add`；`decode` 中间量用 `i64` 再 clamp 回 `i32`。
- 回归：`dimension_mismatch_is_an_error`（`[0;5]/[1;9]` 与 `[0;3]` 均返回 `Dimension`）。

### M3 · `crates/trit-cli/src/commands/topology.rs`

- 原问题：`TopologyInput.nodes` 直接来自 JSON，`vec![Vec::new(); nodes]` 按声明值分配；
  `{"nodes": 1000000000}` 可请求 ~24 GB，分配失败直接 abort。
- 修法：反序列化后加 `MAX_NODES = 100_000` 硬上限，超限 `bail!`（库层 `TernaryTopology::new`
  保持通用，不强行限制调用方）。

## Low 项（15 条，原样保留，本次未动）

| # | 文件:行 | 一句话问题 |
|---|---|---|
| A1 | `crates/trit-bdd/src/lib.rs:60-84` | `BddLimits` 是软限额，单次 `apply`/量化内部峰值可突破 `max_nodes`；文档已说明 |
| A2 | `crates/trit-core/src/bulk.rs:317-323` | `TriVec::not` 对 `ones` 清尾填充位，`unknowns` 未 mask；靠不变量保证 |
| A3 | `crates/trit-bdd/src/encode.rs:114` | `Expr::Var(name)` 直接 HashMap 索引；当前入口预注册变量 |
| A4 | `crates/trit-bdd/src/quantify.rs:65-68,107-111` | `Quantified::is_valid`/`counterexample` 不检查节点预算，方法名不带 `try_` |
| B1 | `crates/trit-net/src/format.rs:132-133` | TRN1 的 u16 维度乘积最大 ~860 MB blob 可触发 ~8.6 GB 分配 |
| B2 | `crates/trit-circuit/src/timing.rs:435-437` | 反向 required-time `try_from(...).unwrap_or(u32::MAX)` 静默饱和，与前向报错不一致 |
| B3 | `crates/trit-build/src/egraph.rs:383-410` | `rebuild` 固定点上限 `classes.len()*2`，魔法常数 |
| B4 | `crates/trit-schedule/src/lib.rs:364-374` | 调度兜底返回 `Cycle("内部错误")`，错误类型与语义不匹配 |
| C4 | `crates/trit-codec/src/plan.rs`（Sparse encode） | 逻辑长度写 u32 小端，>4.29e9 trit 静截断 |
| C5 | `crates/trit-codec/src/hamming.rs` | GF(3) [4,2,3] 对 2 个错误必然静默误纠（distance-3 固有） |
| D2 | `crates/trit-cli/src/commands/schedule.rs:15-22` | `workers` 无 CLI 预检，`workers:0` 由库兜底 |
| D3 | `crates/trit-cli/src/commands/schedule.rs:30-33` | 生产 `expect("Schedule 可序列化")` |
| D4 | `crates/trit-cli/src/commands/verify.rs:49` | `best.expect("至少尝试一种排序")` |
| D5 | `crates/trit-gpu/src/lib.rs:117-128` | `matmul_words` 不校验入片尺寸一致性，`b_pos` 不足会越界 |
| D6 | `crates/trit-cli/src/main.rs:711,747-748` 等 | 文件 IO 全量 `read_to_string`，无 `--max-bytes` 闸门 |

## 原报告的未审计区域（沿用）

1. WGSL 着色器源码未做形式化验证，仅核对 Rust 侧生成逻辑。
2. `fuzz/` 按 workspace 约定 exclude，未跑。
3. GPU feature 真机路径在 VM 上仅编译通过。
4. wasm 浏览器运行时未做端到端。
5. 未对 BDD/egraph/SAT 求解器做渐进式性能基准。
6. C5 属 distance-3 码固有特性，未升级为缺陷。
