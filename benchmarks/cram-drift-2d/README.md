# cram-drift-2d（M0a）

CRAM + 3D 漂移的 **M0a**：2D 障碍网格轨迹数据集 + BFS oracle。
设计见 [`docs/research/CRAM-3D-DRIFT.md`](../../docs/research/CRAM-3D-DRIFT.md)。

- 8×8 网格、20% 障碍、起点 `(0,0)`、终点取 BFS 可达集里最远的点（距离相同取索引最小）。
- 路径由 BFS 给出（邻居顺序固定：上、右、下、左），整个数据集确定性。
- Rust 是交付实现；`python/bfs_ref.py` 是独立 oracle（不交付），用来逐行对拍。

## 复现

```bash
cargo test --release
cargo run --release -- --tasks 1000 --out dataset.jsonl
python python/bfs_ref.py --check dataset.jsonl
# 行数 1000｜DIFF 0
```

## 验收

- `cargo test`：4 条性质测试（路径连续/不踩障碍、长度等于 BFS 距离、不可达返回 None、
  终点是最远可达点）。
- Python oracle 与 Rust 输出 **DIFF 0**（同一 seed、同一邻居顺序）。
- 这是 M0a 的 oracle；M0b 的漂移生成器与 M0c 的能量对照尚未实现。

## 边界

- 只有 2D 网格，没有 3D、没有漂移模型、没有 CRAM 能量。
- 数据集只用于 M0b/M0c 的输入；不代表任何物理可行性结论。
