# trit-gamescan

游戏环境扫描、受控故障注入与恢复的 Phase 0 核心。只依赖 `trit-core`，不依赖任何游戏引擎或 MCP SDK。

## 模块

- `Ternary` 三值判定：复用 `trit-core` 的 Kleene 语义；`True` 通过、`False` 失败、`Unknown` 证据不足。
- `WorldSnapshot`：规范化实体/资源/collider 快照，带稳定哈希。
- 结构规则：重复 ID、悬空 parent、层级环、非有限 Transform、零 scale、非归一化 quaternion、重复资源键。
- `PenetrationOracle`：sphere / capsule / box 两两 collider 穿透检测（含 OBB SAT、capsule-box 三元搜索最近点）。
- `Mutation` / `MutationPlan` / `Mutator`：受控状态污染。
- `RecoveryCoordinator`：checkpoint → 注入 → 扫描 → 回滚 → 验证；失败降级到 pristine，再失败 quarantine。
- `ReplayWorld`：Phase 0 replay adapter；后续 Bevy/Unity/Unreal/Godot adapter 放在独立 crate。

## 验证

```bash
cargo fmt -p trit-gamescan -- --check
cargo clippy -p trit-gamescan --all-targets -- -D warnings
cargo test -p trit-gamescan
python scripts/module_budget.py
```

当前测试：2 个 snapshot 单测 + 9 个 penetration 集成测试 + 15 个 Phase 0 集成测试，共 26 条。

## 边界

- 当前没有 MCP server / agent airlock / 真实引擎 adapter。
- 穿模是 collider 级解析检测；mesh/visual bounds、CCD/swept、通用凸体 GJK/EPA 未实现。
- 非均匀缩放的 sphere/capsule 返回 `Unknown`，不伪造 `Pass`。

设计说明见 [`docs/GAME-SCAN.md`](../../docs/GAME-SCAN.md)。
