# 测试扩面：为什么是这些格子

全仓 45 个 crate、**3247 条 `#[test]`**。这轮把四个 crate 补到下表，全绿，
`clippy -D warnings` 与 `fmt --check` 都干净。

| crate | 动手前 | 现在 | 本轮 + |
|---|---:|---:|---:|
| `trit-orch`（编排内核） | 178 | **335**（296 集成 + 39 单元） | +157 |
| `trit-graph`（三态图） | 59 | **86**（59 + 27 集成） | +27 |
| `trit-ops`（三值算子） | 37 | **66**（37 + 29 集成） | +29 |
| `trit-unsafe`（unsafe 审计） | 15 | **41** | +26 |
| `trit-eval`（求值/化简） | 19 | **46**（18 集成 + 19 单元 + 9 属性） | +18 |
| `trit-redteam`（攻方审计） | 42 | **65**（56 单元 + 9 CLI） | +23 |
| `trit-machines`（三值机器） | 26 | **34**（30 单元 + 4 CLI） | +8 |

数字不是重点。**堆重复用例只会让测试变慢而不变强**，所以先做了一次符号级盘点：
统计每个 crate 的 `pub fn/struct/enum/const` 在测试文本里的命中率，找出"公开面有、
测试文本没有"的格子。命中率最低的几处正好是安全/数值这两个面：

```
trit-ops      31.0%    trit-unsafe  33.3%    trit-parts   46.3%
trit-eval     50.0%    trit-train   50.0%    trit-redteam 52.5%
```

## 补的九组（`trit-orch`）

| 文件 | 条数 | 盯的失效形状 |
|---|---|---|
| `suite_core/budget.rs` | 12 | 超支/超并发/超容量：不报错、只是多花钱；已结算超支是沉没成本、在途超支必须拒 |
| `suite_core/counts.rs` | 8 | 在途/已结算若被算进"挂起/拒绝"，数字全对、含义全错 |
| `suite_core/ternary_matrix.rs` | 11 | **全表**：K3 AND 9 格、依赖门 9 格、四门准入 **81 格**（True 1 / False 65 / Unknown 15） |
| `suite_core/plan_matrix.rs` | 12 | 空 id、重复 id、重复依赖、悬空引用、自环、双节点环、菱形、任务数上限 |
| `suite_trace/store.rs` | 16 | schema 闸、坏行 **1-based 行号**、乱序按 seq 归位、同 seq 冲突、旧步骤迁移四条 |
| `suite_coordination/route.rs` | 14 | "不存在无义务的 Unknown"：每个 `BlockReason` 映射到该产出的义务 |
| `suite_coordination/claim_edges.rs` | 17 | 前缀不是边界、尾斜杠、过期≠释放、fence 单调、路径数上限 |
| `suite_coordination/receipt_matrix.rs` | 14 | 四态逐级推进；失败冻结；幂等键重放不多一条 |
| `suite_effects/lifecycle.rs` | 14 | 六态转移：跳态=宣布发了却没发；Confirmed 后回退=重发成功操作 |

## 另外三个 crate 补的是什么

**`trit-eval`（新增 `tests/eval_semantics.rs`，18 条）**
求值层是"结论来自穷举"的执行者，所以三条契约必须钉住：
**位并行 = 逐点求值**（3 变量 × 27 赋值 × 3 语义 × 4 算子，逐位比对）；
**真值表第 i 行 = `decode_assignment(i)` 那份赋值**（行号就是三进制编码，
False=0/True=1/Unknown=2）；**化简保语义**（10 个式子 × 3 语义，逐点比对而不是比表）。
顺带钉住：编解码在 3^3 全域上双射、重复变量被拒、越域报 `OutOfDomain`、
未绑定变量编码成 `Unknown` 而不是 `False`。

**`trit-redteam`（`rail.rs` +102、`blob.rs` +106、`sidechannel.rs` +118，共 23 条）**
攻方视角的三个面：
- `rail.rs`：门级网表分析的**优先级**（`a | b & c` 必须是 `a | (b & c)`，反了会把"有条件与"读成"无条件或"）、
  `~`/`!` 同义、`1'b0/1'b1` 字面量、选错文件必须报错而不是给出"零发现"、
  超过 `MAX_SIGNALS` 拒绝穷举、位线落单拒绝、缺 `y_p/y_n` 的 panic 契约被钉住。
- `blob.rs`：CRC-3 只在 GF(3) 字母表内取值、**单个 trit 错误必须被抓**（6 位载荷 × 每位的两个方向全试）、
  空/截断/未知语义字节/坏版本全部拒绝、`nvars=40` 走 `checked_pow` 拒绝而不是分配天文表、
  27 种非规范写法的**头字节必须完全相同**（差别只在末字节）。
- `sidechannel.rs`：熵的**手算值**（单类 0、对半 1、三类等概率 log2(3)）、
  重量 0 桶里只能有 `unknown`、`leakage = H(类别) − H(类别|重量)` 必须是差而不是另一个估计、
  JSON 里泄漏量 round 到 3 位（过细的有效位会让人以为这是精确物理量）、6 信号 729 行的实际上限。

**`trit-machines`（`vm.rs` +169，8 条）**
三值栈机：**三张 9 格真值表全穷举**（AND/OR/XOR/NOT，27 组）；
栈下溢（`Not` 空栈、`And` 单元素、`Dup` 空栈、`Jz` 空栈）一律判**截断**而不是 panic——
它跑的是外部字节码，一份坏程序不该把审计进程带走；
空栈 `Halt` 报 `X`（"确定地不知道"）；**截断优先于"只有一个输出"**：
已经见过的路径全指向 `T`，但没走到的路径可能指向别的，此时给结论就是作弊。

## 本轮又测出来的五个口径陷阱

1. **真值表的行号跟"变量出现顺序"走**（`L52`）。重写会改顺序，所以"化简前后比 `values`"
   会把等价误判成不等价。判等只能用 `equivalent`（它取并集后对齐）或逐点 `eval`。

2. **`TriBit` 没有单点取值方法**（`L51`）。单点读是 `checked_get(bit)`；`value_at` 是 `TruthTable` 的。

3. **CRC-3 的"零余数"写成 trit `-1`**（`L53`）。digit↔trit 是 `digit = value + 1`，
   所以"0 就是 0"的实现在逐 trit 对拍时会红一半。

4. **`Jz` 会弹栈**（`L55`）。已知条件分支之后 `Halt` 只能看到剩余栈，空栈给 `X`——
   这不是解释器错了，而是"确定地不知道"。

5. **三值里排中律与矛盾律都不成立**。`a | !a`、`a & !a` 在 `a = X` 时都是 `X`，
   所以 `fold_constants` 对含变量的式子几乎永远返回 `None`——它的实际作用面很窄，
   而 `a | 1 → 1` 属于吸收律的**局部重写**，与这个开关无关。

## 成本

**`trit-graph`（新增 `tests/graph_props.rs`，27 条）**
稠密位集图与稀疏 CSR 是同一语义的两套存储，最容易分叉。断言把它们逐项对齐：
节点/关键位/出度/三态计数/每一条边/拓扑序必须**逐点相等**。另外钉住：
`set_edge(…, None)` 必须把 pos/neg 位一起清掉（只清 valid 位会让下一次 Unknown 读成 True）、
`roots()` 是 forest 语义（不可达节点也算 root，不能当可达集合用）、
关键节点的反对票是强否决、PageRank 的质量守恒与 sink 自环、
`normalized(_,0,0)` 必须是 0 而不是 NaN。

**`trit-ops`（新增 `tests/ops_numeric.rs`，29 条）**
三值算术的全部价值是"乘法消失但结果不变"，所以每条快路径都要和标量参考**给同一个数**：
`Rail` 长度 0..=130 全往返、长度 2 的 81 对点积穷举、"4 组 AND+popcount"没有暗格、
`Trytes` 位置编码 = `2·3^k`、`TernaryMatrix` 三个 kernel 与标量对拍、
tryte 整表/拆表/映射三条路径互相对拍（映射只是省掉 `%9`/`/9`，结果必须逐位一致）、
量化的 rel_err 手工重算、四类 scale 的退化输入。

**`trit-unsafe`（`scan.rs` +316 行、`lexer.rs` +107 行）**
这个 crate 没有 lib，断言只能内联。测的是**扫描器自己**：
`unsafe_fn` 不是关键字（下划线不能当分隔符）、高危调用窗口是后 3 行、
`SAFETY` 只在注释里算说明（字符串里的不算）、
`tests/benches/examples` 路径单列不计入 strict、
`counts` 的五元组口径、`collect_rust_files` 跳过 `target/.git/node_modules/.qoder` 与文件上限、
`scan_tree` 按 crate 归组且 K3 合成、
非 UTF-8 的 `.rs` 必须报"读失败"而不是静默跳过。
词法层单独一组：注释/原始字符串/转义引号/嵌套块注释/未闭合字符串，全部断言"行数不变"。

## 测出来的三个真问题

1. **`claim::covers` 尾斜杠锁不住子路径**（真缺陷，已修，台账 `L50`）
   `path.as_bytes().get(scope.len()) == Some(&b'/')` 在 `scope` 带尾斜杠时下标错位。
   后果：宿主写 `crates/a/` 就能对同一目录开出**两张互不冲突的锁**。

2. **`Phase::Settled` 的 `admission` 是 `Unknown` 而不是 `False`**（不是缺陷，但容易误读）
   状态类原因走 `reasons`，不走 `admission`；读 `admission` 会当成"待补证"。

3. **`may_retry` ≠ "能不能发"**（语义陷阱）
   `Prepared` 的 `may_retry` 是 `false`（再 prepare 会报在途），而它恰恰是唯一允许真正发起的状态。

另有几处实现口径被固化下来（不是 bug，但换个人就会理解反）：
`fence` 单调向前（给更大的值是允许的）、`ReceiptLedger::record` 对重复幂等键是幂等返回而不是报错、
`ternary_weights` 的 `seed|1` 会把奇偶 seed 合并、`trit-unsafe` 的 `crate_label` 对树外路径归 `root`。

## 成本

- `trit-orch` 仍是**单个测试二进制**（`tests/kernel/main.rs`），全跑 1.9s、增量 `--no-run` 0.22s。
- 新增的三个集成文件各是一个二进制：`graph_props` 0.00s、`ops_numeric` 0.00s、`trit-unsafe` 并入既有 bin。
  它们只在对应 crate 的测试目标里链接，不改变 `trit-orch` 的增量成本。

## 复现

```bash
cd <repo-root>
cargo test -p trit-orch                                  # 296 + 39
cargo test -p trit-graph -p trit-ops -p trit-unsafe      # 27 / 29 / 41
cargo clippy -p trit-ops -p trit-unsafe -p trit-graph -p trit-orch --all-targets -- -D warnings
```

单组跑：`cargo test -p trit-orch --test kernel suite_core::ternary_matrix`
