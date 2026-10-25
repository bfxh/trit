# trit-procscan

进程行为扫描 + 自动分析 + MCP 配置体检。

## 边界（先说采不到什么）

本机实测**不是管理员**，内核 ETW 的进程/文件/镜像提供程序起不来。所以 v1 只采这些：

| 采得到 | 采不到（报告里写"未采集"，不猜） |
|---|---|
| 进程树、命令行、CPU、工作集 | 谁读了哪个文件 |
| 每个进程加载的模块（DLL 路径） | 注册表访问 |
| TCP 连接与远端地址 | DNS 查询、UDP |
| 指定目录的写入（mtime/大小差分） | 内核驱动/系统调用级事件 |
| 目标自己写到 stdout/stderr 的行 | 内存内容、命令行之外的行为 |

## 三个子命令

```bash
# 1) 起一个命令，记录它和子进程干了什么；顺便落 JSONL 供离线复算
trit-procscan run --watch ./out --events events.jsonl -- some.exe --flag

# 2) 对已有事件流重放分析
trit-procscan analyze events.jsonl

# 3) MCP 配置体检：逐个 server 做一次 stdio 握手
trit-procscan mcp ~/.qoder/mcp.json --timeout-ms 8000
```

每个子命令都支持 `--strict`：有中/高危发现就退出 1（可当 CI 门）。

## 分析规则（只陈述证据）

`spawns-shell`、`spawns-network-tool`、`encoded-command-line`、`loads-module-from-temp`、
`writes-executable`、`outbound-connection`（回环与内网不报）、`many-processes`。

报告里给进程/模块/连接/写盘计数、CPU 前五、以及每条发现的证据行。
**不下"它在横向移动"这种结论**——那需要文件读取、注册表等本工具拿不到的证据。

## MCP 体检：量的是上下文成本

- 启动耗时：spawn → `initialize` 响应（含解释器/包管理器冷启动）
- 工具数、`tools/list` 的 **schema 字节数**（≈ tokens = 字节/4）
- 最大的 5 个工具（按 schema 体积）
- 建议：`npx-launch`（慢、依赖网络）、`slow-startup`（>2s）、`schema-heavy`（>20 KB）、
  `no-tools`、`duplicate-tool-names`、跨 server 重名工具

实测（本机 `~/.qoder/mcp.json`，2026-10-25）：

| server | 启动 | 工具数 | schema | ≈ tokens |
|---|---|---|---|---|
| `rea` | 1682 ms | **207** | **3,158,358 B** | **~790k** |

单个工具就吃到 4.5 万 token 的：`compare_web_captures`（181 KB）、`inspect_analysis_view`（170 KB）。

## 未做

- 非管理员下拿不到 ETW 全局视图；要文件读写归属得上管理员 + ETW/WPR。
- MCP 体检只做 stdio；远程（`url`）server 跳过。
- 不分析也不攻击 Rust 运行时（后续单独立项）。
