# CLI 并发写锁

状态：**已实现**。`trit-wspace` 的写命令在分发前对 `--out` / `--log` 路径加
advisory 锁文件，防止多进程同时读-改-写同一工件造成覆盖（lost update）。`atomic_write`
仍然负责“不半写”，两者分工不同。

## 1. 锁协议

- 锁文件是 `<target>.lock`（与目标同目录），用 `OpenOptions::create_new`（`O_EXCL`）
  原子创建；创建成功即持有。
- 内容：`pid=<pid>\nunix_ms=<millis>\n`，只用于诊断和 stale 判断。
- 获取顺序按路径字典序，多个锁（`--out` + `--log`）不会互相死锁。
- 默认等待 2 秒（`TRIT_LOCK_WAIT_MS` 环境变量可调），每 50 ms 重试；超时报
  `FAIL ... 被另一个进程锁定（pid=...）：等待 N ms 超时`，不写任何输出。
- 锁文件 mtime 超过 10 分钟视为 stale，可被回收重试；进程正常退出时 `Drop`
  只删除自己 pid 的锁文件。
- 相对路径先经 `std::path::absolute` 归一，避免 `a.wsp` 与 `./a.wsp` 锁不同文件。

## 2. 覆盖范围

`cli::run` 在命令分发前扫描 `--out` / `--log` 并获取锁，覆盖整个读-改-写周期：

- 单工件输出：`patch`、`apply-candidate`、`rebase`、`transplant`、`dare-drop`、
  `repair`、`merge`、`task-*`、`unpack-artifact`、`join-artifact` 等；
- 目录输出：`split-artifact` / `shard` 等锁 `<dir>.lock`（父目录），同一目录的
  并发写同样被串行化；
- 审计日志：`--log` 也加锁，避免两个进程追加同一 JSONL 交错。

没有 `--out` / `--log` 的只读命令不加锁；读取不被阻塞（advisory 锁不阻止读）。

## 3. 真实读数

- 5 个并发 `patch` 写同一 `--out`：**5/5 成功**（锁把它们串行化），输出文件存在，
  `out.wsp.lock` 无残留，5 个 `--log` 各 1 条。
- 人为持锁（`pid=99999`）时，`TRIT_LOCK_WAIT_MS=100` 的 `patch` 在 100 ms 内失败：
  - stderr：`FAIL ... 被另一个进程锁定（pid=99999 unix_ms=0）：等待 100 ms 超时`
  - 报告 0 字节，`out.wsp` 未被写。
- 锁释放后同一 `patch` 成功：`tile_ops=1`、`touched=16`，`out.wsp` 305,989 字节，
  锁文件已删除。

## 4. 边界（未证栏）

- **advisory**：只约束走 CLI `run` 的写入；库调用、mmap、外部编辑器直接写文件
  不会被阻止。
- MCP 工具未接锁：MCP 是单进程多会话，写路径仍只有 `atomic_write`，并发会话
  写同一工件没有这把锁。
- 跨主机 / NFS / 某些网络文件系统不保证 `O_EXCL` 的互斥语义。
- stale 阈值固定 10 分钟：超过 10 分钟的长命令可能被另一个进程误回收；崩溃遗留
  的锁最多等 10 分钟（或手动删除）。
- 锁内容不参与互斥判定；写一半的锁文件会按 stale 超时处理。
- `std::path::absolute` 不解析 `..` / symlink，指向同一 inode 的别名路径仍可能
  锁不同文件。
