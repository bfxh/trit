# GAME-SCAN：游戏环境扫描、故障注入与恢复（Phase 0）

## 范围

`crates/trit-gamescan` 是一个引擎无关的游戏环境扫描核心：

- 扫描运行中世界的结构/物理不变量；
- 在隔离测试实例里做受控状态污染；
- 每次污染都是可回滚事务：checkpoint → 注入 → 扫描 → 回滚 → 验证；
- 恢复失败时降级到 pristine checkpoint，再失败就 quarantine，绝不带病继续；
- 通过 `trit-core::Ternary` 表达通过/失败/未知，`Unknown` 不冒充 `Pass`。

当前**不包含**：MCP server、agent airlock、Bevy/Unity/Unreal/Godot adapter、mesh 级穿模、CCD/swept、通用凸体 GJK/EPA。引擎和协议依赖属于 adapter crate，不属于本 crate。

## 模块

| 模块 | 内容 |
|---|---|
| `snapshot` | `WorldSnapshot`、实体/资源/collider 快照、稳定哈希、规范化排序 |
| `findings` | `Ternary` 结论、`Finding`、`RuleOutcome`、`ScanReport` |
| `rules` | 结构规则：重复 ID、悬空 parent、层级环、非有限 Transform、零 scale、非归一化 quaternion、重复资源键 |
| `penetration` | `PenetrationOracle`：sphere/capsule/box 两两穿透，OBB SAT，capsule-box 三元搜索最近点 |
| `chaos` | `Mutation`、`MutationPlan`、`Mutator` |
| `recovery` | `CheckpointStore`、恢复验证、checkpoint → pristine → quarantine |
| `replay` | Phase 0 replay adapter |

## 三值判定

扫描结论直接复用 `trit-core::Ternary`（Kleene 语义）：

- `True` = 通过；
- `False` = 失败；
- `Unknown` = 证据不足/能力缺失/几何不支持。

聚合使用 `&`（Kleene 合取）：任一 `False` 即失败；否则任一 `Unknown` 即未知；全部 `True` 才通过。
这个映射避免把“没测到”当成“没问题”，也是后续 MCP/agent 输出必须保留的语义。

## 穿模 oracle

支持：

- sphere-sphere；
- sphere-capsule；
- capsule-capsule；
- sphere-box（OBB）；
- capsule-box（slab 求交 + 三元搜索最近点）；
- box-box（15 轴 OBB SAT，支持旋转）；
- trigger 默认跳过，layer/mask 双向匹配，disabled 跳过；
- 输出 depth/normal/contact points，按深度分 Medium/High/Critical；
- 非法 collider 或非均匀缩放 sphere/capsule 返回 `Unknown`，不伪造结果。

未实现：mesh/visual bounds 穿模、CCD/swept 高速隧穿、通用凸体 GJK/EPA。

## 故障注入与恢复

```text
baseline scan
  -> checkpoint + verify
  -> apply mutations (receipts)
  -> scan polluted state
  -> always rollback
  -> verify recovery (hash / counts / tick / collider count / rule parity)
  -> resume or quarantine
```

- 默认所有注入都 abort；只有显式提升为持久测试夹具才 commit。
- checkpoint 恢复失败 → pristine fallback。
- 恢复验证失败或恢复后规则签名不等于基线 → 继续降级。
- 全部失败 → `Quarantined`；调用方必须销毁/重启实例，不得 resume。
- 恢复本身也要做混沌测试：restore 失败、verify 注入失败、apply 中途失败。

## 与 OpenAI FUSF 论文的关系

参考：

> OpenAI. *The free uniform spanning forest is a factor of IID.* 2026-09-25.  
> <https://github.com/openai/math/blob/main/preprints/The-free-uniform-spanning-forest-is-a-factor-of-IID-September-25-2026/The-free-uniform-spanning-forest-is-a-factor-of-IID-September-25-2026.pdf>

只引用三个设计原则：

1. 观测与规则分离：adapter 提供快照，core 规则做判定，agent 不直接对原始观测下结论；
2. 无根/重标记等变：规则不依赖某个特权实体、相机或迭代顺序；
3. Borel 因子不等于 finitary 局部算法：扫描器必须自己声明 scope/timeout/采样范围，证据不足返回 `Unknown`。

该论文研究 free uniform spanning forest（FUSF），不是游戏扫描器；本 crate 不是 FUSF 的采样实现，也不声称论文证明了本设计。

## 验证

```bash
cargo fmt -p trit-gamescan -- --check
cargo clippy -p trit-gamescan --all-targets -- -D warnings
cargo test -p trit-gamescan
python scripts/module_budget.py
```

Phase 0 测试：2 个 snapshot 单测 + 9 个 penetration 集成测试 + 15 个 Phase 0 集成测试，共 26 条。
