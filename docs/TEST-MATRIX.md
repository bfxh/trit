# 测试扩面：从 178 到 296，以及为什么是这些格子

## 先说结论

`trit-orch` 的用例从 **178** 涨到 **296**（+118，+66%），全绿，总耗时 **2.15s**。
增量 `--no-run` **0.22s** —— 没有新增测试二进制，全部挂在既有的 4 个 suite 目录里，
所以"每次改 src 多一次链接"的成本一分钱没多花。

但这轮的重点不是数字，是**补哪一格**。堆 300 条重复用例只会让测试变慢而不变强；
所以先做了一次覆盖盘点，只补那些**失效会静默**的面。

## 怎么找的缺口

用符号级统计扫了一遍 `src/` 的公开 API 在测试里的出现次数，找出三类：

| 类型 | 例子 | 为什么危险 |
|---|---|---|
| 零覆盖 | `Budget::from_snapshot`、`obligations_for`、`TraceFold`、`Counts` | 守门函数没人测，改错也不红 |
| 只测了名字 | `may_retry` 被调用过但没断言语义 | 名字对、语义反了也看不出来 |
| 只测了一侧 | 上限类错误只测了命中，没测不命中 | off-by-one 会变成"永远拒绝" |

## 新增的九组，以及每一组盯的那一件事

| 文件 | 条数 | 盯的失效形状 |
|---|---|---|
| `suite_core/budget.rs` | 12 | 超支、超并发、超容量：**不报错、只是多花钱**。含"已结算超支是沉没成本不是永久阻塞"和"在途超支必须拒绝"的区分 |
| `suite_core/counts.rs` | 8 | 分组：在途/已结算若被算进「挂起/拒绝」，面板显示"一堆被拒"，数字全对、含义全错 |
| `suite_core/ternary_matrix.rs` | 11 | **全表**：K3 AND 9 格、依赖门 9 格、四门准入 81 格。三值的 bug 几乎都是"某一格悄悄变成 False/True" |
| `suite_core/plan_matrix.rs` | 12 | 图边界：空 id、重复 id、重复依赖、悬空引用、自环、双节点环、菱形、任务数上限 |
| `suite_trace/store.rs` | 16 | 审计地基：schema 闸、坏行**行号**、乱序按 seq 归位、重复 id 去重、同 seq 冲突报错、旧步骤迁移四条 |
| `suite_coordination/route.rs` | 14 | "不存在无义务的 Unknown"：每个 `BlockReason` 映射到该产出的义务，且未分类原因兜底 |
| `suite_coordination/claim_edges.rs` | 17 | 认领边界：前缀不是边界、尾斜杠、过期≠释放、fence 单调、路径数上限 |
| `suite_coordination/receipt_matrix.rs` | 14 | 四态只能逐级推进；失败后冻结；幂等键重放不多一条 |
| `suite_effects/lifecycle.rs` | 14 | 六态转移：跳态=宣布发了却没发，Confirmed 后回退=重发成功操作 |

合计 **118** 条，全部带 `ORCH_*` 断言 id，失败时能直接定位到"哪一格坏了"。

## 测出来的三个真问题

这轮不是"补测试就完事"，有几条测试一写就红，红得有意义：

1. **`paths_overlap("crates/a/", "crates/a/src/lib.rs") == false`**（真缺陷，已修）
   `covers` 直接比较 `path.as_bytes().get(scope.len()) == b'/'`，`scope` 带尾斜杠时下标错位。
   后果：宿主按习惯写 `crates/a/` 就能对同一目录开出**两张互不冲突的锁**。
   修法是 `covers` 里先 `trim_end_matches('/')`，行为回到文档承诺的"目录前缀覆盖子路径"。

2. **`Phase::Settled` 的 `admission` 是 `Unknown` 而不是 `False`**（不是缺陷，但容易误读）
   状态类原因不通过 `admission` 表达，读 `admission` 会看到默认值，误当成"待补证"。
   已固化成注释 + 断言：判状态必须看 `reasons`。

3. **`may_retry` 与"能不能发"不是一回事**（语义陷阱）
   `Prepared` 的 `may_retry` 是 `false`（再 prepare 会报在途），但它是唯一允许真正发起的状态。
   把两者混用会得到"刚 prepare 完就不能发"的死锁。已固化成断言。

## 边界与不做的部分

- **没有新增测试二进制**。9 个文件全部登记进既有 `suite_core / suite_trace / suite_coordination / suite_effects`。
- **没有测 CLI 之外的 I/O**。文件系统用例只碰 `temp_dir()`，不碰仓库。
- **没有引入覆盖率工具**。`llvm-cov` 在 Windows 上要额外链路，本轮用"符号级零覆盖 + 矩阵表"替代；
  覆盖率数字好看但漏掉的往往是判错对象的那一类 bug（见 `docs/ERRORS.md` 的 B 族）。

## 复现

```bash
cd D:/KF/trit-inv
cargo test -p trit-orch            # 296 条，2.15s
cargo test -p trit-orch --no-run   # 增量 0.22s
```

单组跑：`cargo test -p trit-orch --test kernel suite_core::ternary_matrix`
