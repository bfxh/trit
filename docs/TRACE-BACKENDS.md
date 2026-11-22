# 轨迹后端：JSONL 或 trit-db（可选）

`trit-orch` 默认使用 JSONL 轨迹存储（`TraceEventStore`）。从本轮起，也可以
选择把轨迹事件存进 [trit-db](https://github.com/bfxh/trit-db) 的 `tritdb-txn`
WAL：事件与元数据在同一个事务里提交，不再有日志/水位/锚点的跨 sidecar 半提交。

## 选择方式

默认（不启用 feature）：

```toml
trit-orch = "..."          # 使用 TraceEventStore（JSONL + 哈希链 + 锚点）
```

启用 trit-db 后端：

```toml
trit-orch = { version = "...", features = ["tritdb"] }
```

```rust
use trit_orch::tritdb_backend::TritDbTraceStore;
use trit_orch::{TraceEvent, TraceEventKind};

let store = TritDbTraceStore::open("trace.wal")?;
store.append(&TraceEvent::new("t", 1, TraceEventKind::TaskSelected, "c", 100))?;
let events = store.load()?;
# Ok::<(), trit_orch::Error>(())
```

需要元数据 HMAC（外部锚点）时：

```rust
let store = TritDbTraceStore::with_anchor("trace.wal", b"host-key")?;
```

## CLI

```bash
# JSONL（默认）
printf '{"action":"append","path":"trace.jsonl","events":[...]}' | trit-orch trace --trace-backend jsonl
printf '{"action":"load","path":"trace.jsonl"}' | trit-orch trace --trace-backend jsonl
printf '{"action":"check","path":"trace.jsonl","trace_id":"t"}' | trit-orch trace --trace-backend jsonl

# trit-db（features = ["tritdb"]）
printf '{"action":"append","path":"trace.wal","events":[...]}' | trit-orch trace --trace-backend tritdb

# SQL 查询（features = ["tritdb-sql"]）
printf '{"action":"sql","path":"trace.wal","sql":"SELECT kind, count(*) FROM trit_trace GROUP BY kind"}' \
  | trit-orch trace --trace-backend tritdb

# CAS 快照/恢复（features = ["tritdb-cas"]）
printf '{"action":"snapshot","path":"trace.wal","cas_root":"cas"}' \
  | trit-orch trace --trace-backend tritdb
printf '{"action":"restore","path":"trace.wal","cas_root":"cas","head":"<manifest-hex>"}' \
  | trit-orch trace --trace-backend tritdb
```

`path` 也可以由 `--trace-path <path>` 覆盖。`trace` 的 action：
`append` / `load` / `check` / `sql` / `snapshot` / `restore`。

## 两种后端的分工

| 能力 | `TraceEventStore`（JSONL） | `TritDbTraceStore`（trit-db） |
|---|---|---|
| 存储形态 | 一行一个 JSON + sidecar | `tritdb-txn` WAL 的 KV |
| 原子性 | 日志 + anchor/watermark 尽力回滚 | 事件 + 元数据同一事务 |
| 跨进程写 | `.lock` 文件（advisory） | `tritdb-txn` 单写 WAL |
| 链/摘要 | SHA-256 + `chain_seq` + 可选锚点 | 同样字段；元数据 HMAC 可选 |
| 旧格式迁移 | 全空摘要自动迁移 | 不适用（新后端从空 WAL 开始） |
| 适用 | 单机文件、可读可拷的审计产物 | 需要事务/恢复/WAL 的宿主 |

## 边界

- `tritdb-txn` 是可选 git 依赖（固定 rev）；不启用 feature 时不会编译它，但 Cargo
  仍会解析/拉取该 git 源。离线构建请预先 vendor 或关闭该 feature。若本机 Cargo
  libgit2 在代理下报 `failed to connect ... Invalid argument`，设置
  `CARGO_NET_GIT_FETCH_WITH_CLI=true`（本机 `.cargo/config.toml` 已设置；该文件被
  仓库 `.gitignore` 忽略，不会提交）。
- `TritDbTraceStore` 的 `load` 会重算内容摘要、验证每个 trace 的写入序与元数据；
  元数据 HMAC 只防“改数据库文件”的攻击者，拥有密钥的宿主仍可重写。
- 两个后端都建立在“宿主可信”的模型上；执行器/模型自报问题仍由完成闸的
  `Receipt` provenance 负责。

## 验证

```bash
cargo test -p trit-orch --features tritdb --test trace_tritdb
```
