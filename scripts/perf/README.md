# 性能与运行时验证脚本

这些脚本产出 `docs/plans/` 与评审记录里引用的性能数字，**都可复现**：
同一台机器、同一二进制、同一脚本生成的数据集。

## 前提

```bash
# 服务端要带 tritdb-pgwire（自研只读 pgwire）与 tritdb-cas
cargo build --release --features tritdb-pgwire,tritdb-cas -p trit-orch

# 可选：Node 客户端
node --version
```

> **注意**：`cargo test` 会用**默认 feature** 重新构建 `target/debug/trit-orch.exe`，
> 覆盖带 feature 的二进制。长期压测时建议把二进制复制到独立路径再用 `--bin` 指定，
> 否则会看到「trace serve 需要编译 feature tritdb-pgwire」然后连接被拒。

## 脚本

| 脚本 | 测什么 | 典型命令 |
|---|---|---|
| `matrix.py` | 延迟分位 p50/p95/p99/**p999** + 写放大（WAL / SQLite） | `python scripts/perf/matrix.py --sizes 1000,10000 --queries 50000` |
| `scale.py` | 并发阶梯（8/32/64 连接）QPS、分位、RSS | `python scripts/perf/scale.py --clients 8,32,64 --seconds 30` |
| `soak.py` | 长稳 + RSS 采样（是否缓慢增长） | `python scripts/perf/soak.py --duration 3600 --clients 8 --interval 30` |
| `deep.py` | 大 payload（默认 2000 行 × 10KB）+ 复杂 SQL 分位 | `python scripts/perf/deep.py --rows 2000 --payload 10000 --rounds 500` |
| `node_client.js` | 非 Python 客户端（Node 手写 pg 协议） | `node scripts/perf/node_client.js --clients 8 --seconds 30` |

共享件 `pg_client.py`：最小 pg 协议客户端（startup + 简单查询 `Q`）、RSS 采样、
分位计算、`append`/`sync` 准备。**只依赖 python 标准库**。

## 口径（读数字前必须知道）

- 延迟都是**热态**（服务已启动、页缓存已预热），不含冷启动。
- `matrix.py` / `deep.py` 是 **1 连接串行**；`scale.py` 是单进程多线程；
  `node_client.js` 是单进程 Node 事件循环。
- 客户端语言会显著影响吞吐：python 多线程受 GIL 限制，Node 8 连接即可 4.9 万 QPS。
  **不要把 python 的吞吐当作服务端上限**。
- 写放大 = 物理字节 / 逻辑事件 JSON 字节。WAL 侧约 2.07×（追加日志 + 记录头/CRC），
  SQLite 物化侧约 0.31–0.35×（结构化列，去掉 JSON 字段名）。
- `sync` 是批量事务（每 64 条一组 INSERT），不是每条事件一次 fsync；
  `append_step` 是每事件一次 `sync_all`（所以库基准里只有 ~600 e/s）。

## 参考读数（本机：Intel Core Ultra 7 270K Plus，24 核，15.4 GB，SSD，Windows 11）

| 场景 | 结果 |
|---|---|
| `matrix.py --sizes 1000,10000` | ~16k QPS；p50 0.054ms、p999 0.61ms；写放大 2.07× / 0.31–0.35× |
| `scale.py --clients 64`（python） | 24,655 QPS（GIL 限制） |
| `node_client.js --clients 8` | 48,939 QPS、p999 0.596ms |
| `soak.py --duration 120` | 397 万请求 0 失败，RSS 8876 → 8828 KB |
| `soak.py --duration 3600` | **1.135 亿请求 0 失败**，qps 31,417；p50 0.241ms / p99 0.496ms / p999 0.726ms；RSS 8864 → 2800 KB（峰值 8864，120 次采样，无缓慢增长） |
| `deep.py --rows 2000 --payload 10000` | 点查 0.1ms；全表扫描 2–4ms |

## 已知边界（没测的别当测过）

- **慢盘**：本机只有 SSD，HDD/网络存储下的 `append_step` 与写放大未测。
- **跨机网络**：只测了同机 LAN 地址（与 loopback 无显著差异），真实跨机未测。
- **复杂 SQL**：只覆盖 `count(*)`、`WHERE`、`GROUP BY`、`ORDER BY`、`LIMIT`；
  没有 JOIN、子查询、窗口函数。
- **小时级 RSS**：2026-11-23 已实测 `soak.py --duration 3600`（8 并发、3613s）：1.135 亿请求 0 失败，RSS 8864 → 2800 KB，无缓慢增长。
- 服务端是**只读** pgwire：写语句返回 `0A000`，且不支持扩展查询协议（Parse/Bind/Execute）。
