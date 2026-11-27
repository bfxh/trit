# trit-tech-stack-mcp

把 bfxh/trit 三进制的技术栈策略暴露为 MCP 工具：**新增 Python 只准测试代码**。

## 构建

```bash
cargo build --release
```

## MCP 配置

```json
{
  "mcpServers": {
    "tech-stack": {
      "command": "<abs>/tools/tech-stack-mcp/target/release/trit-tech-stack-mcp"
    }
  }
}
```

传输是 stdio（一行一个 JSON-RPC 2.0 消息）。

## 工具

- `validate_path {path}` → `{"path": "...", "allowed": true|false, "rule": "...", "reason": "..."}`
- `policy {}` → 规则全文（test-only 扩展名、测试路径、存量豁免、适用对象、例外）

## 规则

- 新增/覆盖 `.py` 只能落在 `tests/`、`test/`、`spec/`、`test_*.py`、`*_test.py`、`conftest.py`；
- 存量豁免，不回溯清理已有 `.py`；
- 其它语言（Rust / TS / Go / …）不受限；
- 例外只来自任务 brief 的 `explicit_override`。

完整说明见仓库根 `docs/tech-stack-policy.md`。
