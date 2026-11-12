# WSP1 tile 编辑：任意矩形块的事务编辑与审计

状态：**已实现**。新增两个原子编辑 `set_tile` / `fill_tile`，走现有 `Patch` 事务、
`Journal` 逐位回滚和 `AuditLog` 哈希链；CLI `patch` 直接接受 JSON。

## 1. 编辑语义

- `SetTile { layer, row_start, rows, col_start, cols, values }`：把 `rows×cols` 矩形块
  按行主序设为 `values`。
- `FillTile { layer, row_start, rows, col_start, cols, to }`：用同一个 trit 填充。
- 只改 trits，不动 bias/scale/f32 bias；越界、零行列、`values` 长度不符都整体拒绝，
  网络保持原样（`Patch::apply` 先整体 validate 再应用）。

## 2. JSON

```json
{"version":1,"edits":[
  {"op":"fill_tile","layer":0,"row_start":0,"rows":4,"col_start":0,"cols":4,"to":"+"},
  {"op":"set_tile","layer":0,"row_start":8,"rows":2,"col_start":8,"cols":2,
   "values":"+-0+"}
]}
```

`values` 可以是字符串（每字符一个 trit）或数组。

## 3. 事务、回滚、审计

```rust
let patch = Patch::new(vec![Edit::FillTile { /* ... */ }]);
let journal = patch.apply(&mut net)?;   // 失败不改
journal.undo(&mut net);                  // 逐位恢复
let mut log = AuditLog::new();
log.record("tile", &before, &after, patch.clone(), true, false)?;
let (report, replayed) = log.replay_net(&before)?;
```

`Undo::Tile` 保存整块旧值（不是重算），回滚后 `Net` 逐位相等。审计条的 `cost`
新增 `tile_ops`，`touched` 计入 `rows*cols`。

## 4. CLI

```bash
trit-wspace patch model.wsp --patch tile.json --log audit.jsonl --out edited.wsp
trit-wspace replay audit.jsonl --artifact model.wsp --out replayed.wsp
```

`patch` 报告 `report.cost.{tile_ops,touched}`；`replay` 报告 `applied/final_checksum`。

## 5. 真实读数（all-MiniLM-L6-v2 第 0 层 query，384×384）

- 4×4 `fill_tile`：`tile_ops=1`、`touched=16`、`trits=0`。
- 工件 checksum `7f30c57cda546db8` → `80d1b189cb7cf44b`。
- 审计日志 1 条 383 字节；`replay` 1 条 applied，`final_checksum=80d1...`，
  输出与 `edited.wsp` **逐字节一致**。
- WSP1 文件大小不变（305,989 字节，等长文本替换）。

## 6. 边界（未证栏）

- 只支持矩形块；没有非矩形 mask / 稀疏 tile。
- 只改 trits；没有 tile 级 bias/scale 编辑。
- 没有跨层 copy/swap tile；`transplant` 仍是单元级。
- `Patch::diff` 仍输出逐点 `set_trit`，没有 tile 级差分压缩。
- `analyze` / `propose` 的候选空间仍是单点编辑，不生成 tile。
- `patch --log` 是追加模式；对同一工件重复应用同一补丁会产生重复审计条，
  重放时第二条会因 `before` 不匹配而失败（审计的预期行为，不是 bug）。
