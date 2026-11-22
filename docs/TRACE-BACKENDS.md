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

# 物化到独立 SQLite（features = ["tritdb-sql"]）：<wal>.sqlite，可与写入方并发
printf '{"action":"sync","path":"trace.wal"}' \
  | trit-orch trace --trace-backend tritdb

# pgwire 远程查询（features = ["tritdb-pgwire"]）
printf '{"action":"serve","path":"trace.wal","listen":"127.0.0.1:5433"}' \
  | trit-orch trace --trace-backend tritdb

# CAS 链压缩（features = ["tritdb-cas"]）：N 层压成单层，旧 head 记到
# <cas_root>/trit-trace.compacted
printf '{"action":"compact","path":"trace.wal","cas_root":"cas"}'   | trit-orch trace --trace-backend tritdb

# 物化表 GC（features = ["tritdb-sql"]）：保留最近 N 条，--trace-keep-last 覆盖
printf '{"action":"gc","path":"trace.wal"}'   | trit-orch trace --trace-backend tritdb --trace-keep-last 10000

# CAS 快照/恢复（features = ["tritdb-cas"]）
printf '{"action":"snapshot","path":"trace.wal","cas_root":"cas"}' \
  | trit-orch trace --trace-backend tritdb
printf '{"action":"restore","path":"trace.wal","cas_root":"cas","head":"<manifest-hex>"}' \
  | trit-orch trace --trace-backend tritdb
```

`path` 也可以由 `--trace-path <path>` 覆盖。`trace` 的 action：
`append` / `load` / `check` / `sql` / `snapshot` / `restore` / `serve` / `sync` / `compact` / `gc`。

## SQL 物化与远程查询

- `tritdb-sql`：事件先按 `chain_seq` 增量物化进 `trit_trace`（每个 trace 记水位，
  只插更新的行），再执行查询；重复查询不会重复插入，事件只读一遍。
- `trace sync`：把事件增量物化进**独立 SQLite**（`<wal>.sqlite`，`trit_trace` 表）。
  读轨迹 WAL 只短暂持锁，写物化表走 SQLite 自己的锁，所以它能和轨迹写入方并发；
  水位取 `max(chain_seq)`，重复执行不会重复插入。
- `gc` 按 `chain_seq` 保留最近 N 条（默认 10000）：**永不删除最大 `chain_seq`
  那行**——否则 `sync` 的水位会回退，把删掉的行重新插回来。
- `tritdb-pgwire`：`trace serve` 若发现 `<wal>.sqlite`（`trace sync` 的产物），
  就用**自研只读 pgwire 服务**直接查 SQLite——完全不碰轨迹 WAL，因此服务期间
  仍可 `append` 轨迹、跑 `trace sync`，客户端立刻看到新行（实测 3 → 4）。该服务
  支持简单查询（`Q`）与扩展查询（Parse/Describe/Bind/Execute/Close/Sync，
  `$n` 占位符转 SQLite `?n`）、只读、无认证/TLS（`SSLRequest` 回 `N`），
  不支持 COPY。
- 没有 SQLite 物化库时，`trace serve` 回退直读轨迹 WAL：启动时物化一次，服务
  进程持续持有 WAL 排他锁，期间写不了同一个 WAL（`trace sync` 也会被拒）。

## CAS 快照与 anchor 链

- `snapshot` 是**增量**的：manifest 只记相对上一次 anchor 的变化条目，删除用
  `!del:<key>` 标记（值固定为 `Hash::of(b"trit-trace:deleted")`）；WAL 内容没变时
  不写新层，返回的 `parent` 为 `None`。chunk 层本身内容寻址，相同 value 不会重复
  存。新头原子写进 `cas_root/trit-trace.anchor`；设 `TRIT_TRACE_CAS_KEY` 时 anchor
  带 HMAC-SHA256，读取时校验。
- `restore` 不给 `head` 时从 anchor 取头；给了 `head` 且设了 key 时要求它与 anchor
  一致，否则 `trace_anchor_mismatch`。恢复前用 `verify_chain` 验整条 manifest 链，
  再沿 `parent` 把各层合并成完整快照（子层覆盖父层，tombstone 剔除对应键），最后
  用单事务改 WAL。旧的全量 manifest 也能沿链合并（后层覆盖前层）。
- `compact` 把 `head` 所在链压成单层（新 manifest，`parent=None`），并在
  `<cas_root>/trit-trace.compacted` 追加 `<unix 秒> <旧 head>`；旧 manifest 仍在
  CAS chunk 里，凭这条记录可手工恢复。已是单层时不动（幂等）。

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

- `tritdb-*` 依赖已 **vendor** 进仓库（`third_party/trit-db`，固定 rev `aca71888`，
  MIT OR Apache-2.0）；`crates/trit-orch/Cargo.toml` 用 `path` 引用，所以
  `cargo build --offline` 不需要网络或 git 凭据。更新 vendor 用
  `scripts/vendor-tritdb.sh <rev>`（见 `third_party/trit-db/README.md`）。
- **anchor 可被回滚**：不设 `TRIT_TRACE_CAS_KEY` 时 anchor 没有完整性保护，
  能改文件的人可以把 `trit-trace.anchor` 指回一个旧 manifest 头，`restore` 会
  照着旧头恢复——这是「只防改数据库文件的攻击者」的边界，不是 bug。设了 key
  后改 anchor 会被 HMAC 发现，但持有 key 的宿主仍可重写。需要抵抗回滚时，把
  anchor 放到攻击者写不到的位置，或由外部 KMS/透明日志跨系统锚定。
- `TritDbTraceStore` 的 `load` 会重算内容摘要、验证每个 trace 的写入序与元数据；
  元数据 HMAC 只防“改数据库文件”的攻击者，拥有密钥的宿主仍可重写。
- 自研 SQLite 只读服务不实现认证与 TLS：`TRIT_PG_PASSWORD` / `TRIT_PG_TLS_CERT`
  只对「回退直读轨迹 WAL」的旧路径生效。需要认证时请走旧路径，或在前置代理上做。
- mTLS 目前**不可强制**：配 `TRIT_PG_TLS_CLIENT_CA` 后服务端会请求客户端证书，但
  `tritdb-pgwire` 的校验器是「可选客户端证书」——实测不带证书、带非 CA 签发证书、
  带已吊销证书都能连（客户端证书根本没被索取），CRL/OCSP 因此无法作为准入手段。
  要强制客户端证书得改 vendor 的校验器或另做服务。
- 两个后端都建立在“宿主可信”的模型上；执行器/模型自报问题仍由完成闸的
  `Receipt` provenance 负责。

## 验证

```bash
cargo test -p trit-orch --features tritdb --test trace_tritdb
cargo test --offline -p trit-orch --features tritdb-pgwire --lib
cargo test --offline -p trit-orch --features tritdb-cas --test trace_cas
cargo test --offline -p trit-orch --features tritdb-sql --test trace_sql
```
