# 高危门政策：并发 / 内存（比 rustc 更狠，但按设计分级）

> 门禁脚本：`scripts/hazard_gate.py`（CI 每次提交跑 `--self-test` + `--strict`）。
> 本文是那份脚本的**设计依据**：为什么这些规则存在、为什么放行清单里的东西
> 可以通过、为什么清单外一律红。

## 原则：危险能力按「是否提高上限且有防护」分三档

rustc 给的是内存安全的地板；clippy 给风格与常见错误。在这之上，本仓还有一些
**有意为之**的高危原语——它们不是疏忽，是能力上限的来源。所以政策不是
「越狠越好」，而是三档：

| 档 | 判定 | 例子 |
|---|---|---|
| **放行** | 提高能力上限 + 有成文防护 + 已登记设计理由 | `trit-ops` 的裸指针 SIMD GEMV（15.8× f32）、`Ptr<T>` 的 `unsafe impl Send/Sync` |
| **有条件** | 可用，但必须带成文不变量（`# Safety` / SAFETY 注释） | `from_raw_parts`、`target_feature`、`Ordering::Relaxed` |
| **禁止** | 不带来上限收益的高危 | `static mut`、`transmute` |

判断一个高危用法落在哪一档，看三个问题：
1. **拿掉它能实现同样的能力吗？** 能（只是图方便）→ 禁止档。
2. **它的危险面有写下来的不变量吗？** 没有 → 有条件档变红，先写文档。
3. **它让某条关键路径快了多少？** 显著（如 15.8×）→ 走放行档，登记进 ALLOW 表。

## 现行规则（六条）

| 规则 | 面 | 档 | 匹配 |
|---|---|---|---|
| `static-mut` | 内存 | 禁止 | `static mut`（产品代码零放行） |
| `transmute` | 内存 | 禁止 | `mem::transmute`（类型双关走双导轨/查表） |
| `raw-memory-api` | 内存 | 有条件 | `from_raw_parts(_mut)` / `set_len` / `assume_init` / `mem::forget` / `Box::from_raw` |
| `unsafe-send-sync` | 并发 | 有条件 | `unsafe impl Send/Sync`（rustc 只要求 unsafe，本门还要求成文不变量） |
| `target-feature` | 并发 | 有条件 | `#[target_feature]`（无探测的机器上是非法指令） |
| `relaxed-ordering` | 并发 | 有条件 | `Ordering::Relaxed`（只许计数器/配置读取，不许用于同步） |

## 放行清单（现状与理由）

| 文件 | 规则 | 理由（为什么这是「提高上限」） |
|---|---|---|
| `crates/trit-ops/src/gemv.rs` | raw-memory / target-feature | SIMD GEMV 内核，15.8× f32 流式吞吐的上限来源；长度不变量由 padded 布局保证，SAFETY 注释在每个 unsafe 块上方 |
| `crates/trit-ops/src/simd.rs` | raw-memory / target-feature | AVX 行点积内核；调用方 `is_x86_feature_detected` 运行时探测 |
| `crates/trit-ops/src/bench.rs` | raw-memory | 内核对照基准：必须复用与内核相同的内存布局，否则测的不是同一条路径 |
| `crates/trit-dot/src/lib.rs` | raw-memory / target-feature | POPCNT 位并行点积；调用方运行时探测 |
| `crates/trit-ops/src/pool.rs` | unsafe-send-sync / relaxed-ordering | `Ptr<T>`：按行切分、无重叠写、join 屏障；Relaxed 用于线程数配置与完成计数（单变量、无同步依赖） |

**新增高危用法的流程**：先回答上面三个问题；确属放行档 → 在
`hazard_gate.py` 的 ALLOW 表登记（文件 + 设计理由），CI 才会绿；
属于有条件档 → 写好 SAFETY 注释即可；属于禁止档 → 没有后门。

## 边界（不猜）

- 本门是**源码级文本门**：先剥注释/字符串/char 字面量/原始字符串再匹配
  （文档示例与检测 needle 不会「复活」成匹配——trit-redteam 自审的教训）；
- 不是类型系统，不证明任何东西；它补充 rustc / clippy / `trit-unsafe`，
  不替代——rustc 管不了 SAFETY 文档缺失，本门管；
- `#[cfg(test)]` 之后与 `tests/` 目录属于演练区：高危形态在那里只计数不拦截，
  产品代码（`crates/*/src` 的非测试区）才是门。
