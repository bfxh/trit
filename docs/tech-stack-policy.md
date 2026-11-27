# 技术栈策略：新增 Python 只准测试代码

状态：默认规则（2026-11-28），适用于 bfxh/trit 三进制全部项目与所有智能体。

## 1. 规则

- **新增或覆盖** `.py` 只能落在测试位置：
  - 目录：`tests/`、`test/`、`spec/`（含其子目录）
  - 文件名：`test_*.py`、`*_test.py`、`conftest.py`
- 生产路径（`src/`、`tools/`、仓库根等）的 `.py` 写入**直接拒绝**——硬约束，不是评分扣分。
- **存量豁免**：不回溯清理已有 `.py`（`tools/*.py` 等旧脚本保持不动）。
- 其它语言（Rust / TS / Go / …）不受此限，仍按项目现有技术栈。
- 唯一例外：用户在任务 brief 里显式授权（`explicit_override`），且授权是一次性的、写进 brief 的。

## 2. 执行方式

| 场景 | 机制 |
| --- | --- |
| `trit-agent-kernel` 内的写工具 | `write_file` 落盘前调用 `tech_stack::validate_write_path` |
| 其它项目 / 外部智能体 | MCP：`tools/tech-stack-mcp` 的 `validate_path` 工具 |
| 子智能体 / 外部 CLI 子智能体 | 回执的 `stack.added` / `stack.used` 越权由 `validate_stack` 判违约 |
| 批量体检 / 审计 | 调 MCP 的 `policy` 拿规则，再逐路径 `validate_path` |

MCP 用法：

```bash
cd tools/tech-stack-mcp && cargo build --release
# MCP 配置示例
# {"mcpServers": {"tech-stack": {"command": "<abs>/tools/tech-stack-mcp/target/release/trit-tech-stack-mcp"}}}
```

工具：

- `validate_path {path}` → `{"path": "...", "allowed": true|false, "rule": "...", "reason": "..."}`
- `policy {}` → 规则全文（test-only 扩展名、测试路径、存量豁免、适用对象、例外）

## 3. 边界（如实）

- 只拦"经由工具/MCP 的写入"；`bash` 重定向、外部 CLI 子智能体直接改盘不在拦截范围，
  需要额外的 shell 写入检测或事后审计。
- 只约束新增/覆盖，不扫描存量。
- 规则是路径与扩展名层面的，不解析文件内容（不判断 `.py` 里到底是测试还是生产代码）。
