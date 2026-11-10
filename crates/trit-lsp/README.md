# trit-lsp：三进制语言服务器（M0）

`.tri`（TTIR-0，单表达式三值逻辑）的 LSP。编辑器把 `.tri` 关联到这个二进制即可。

**没有第二套语法/语义。** 解析走 `trit_core::Parser`，真值表走 `trit_eval::eval`，
运算符表里每个拼写都有测试保证能被解析器接受（`operators_round_trip`）。
语言一改，LSP 自动跟着变；不会出现"编辑器说有错、`trit build` 说没错"。

## 能力（M0）

| 能力 | 说明 |
|---|---|
| 诊断 | 与 `trit build` 前端同规则：文件只能有一条表达式；解析错误的行列直接映射成 LSP range |
| 悬停 | 运算符：名称、优先级、结合性、当前三值语义下的 3×3 真值表（现算）；变量/常量：解释 |
| 补全 | 运算符（含 Unicode 拼写）、常量词、**本文件变量** |
| 文档符号 | 文件内变量（`DocumentSymbol` kind 13） |

三值语义由 `initialize` 的 `initializationOptions.algebra` 选，接受
`kleene`/`k3`、`lukasiewicz`/`l3`/`luk`、`bochvar`/`b3`，默认 `kleene`。

## 跑起来

```bash
cargo build -p trit-lsp
# 二进制：target/debug/trit-lsp（stdio，LSP 帧）
```

Neovim（内置 LSP 客户端）：

```lua
vim.lsp.start({
  name = "trit-lsp",
  cmd = { "/path/to/trit/target/debug/trit-lsp" },
  root_dir = vim.fn.getcwd(),
  init_options = { algebra = "kleene" },
})
```

VS Code 等需要 generic LSP 客户端（例如 `vscode-languageclient` 的最小壳），
本仓不提供编辑器插件；协议标准即可接。

## 设计取舍

- **Full 同步**（`textDocumentSync = 1`）：`.tri` 是单表达式文件，增量同步的复杂度不值。
- **位置按 UTF-16 码元**（LSP 规范）；中文注释不会把列号算错。
- **只做 `.tri`**：不冒充通用语言服务器，也不解析 Verilog 子集（那是 `trit-build` 的 M1 路径）。
- **M0 不做**：重命名/跳转/引用（变量本就文件局部）、格式化（需要独立 printer，容易和语义分叉）。

## 验证

```bash
cargo test -p trit-lsp                              # 26 个单测
cargo test -p trit-lsp --test wire                  # 真机：起子进程走完整 LSP 会话
cargo clippy -p trit-lsp --all-targets -- -D warnings
```

`tests/wire.rs` 用 `Content-Length` 帧和真实进程说话：initialize → didOpen（干净文件）→
hover（Bochvar 真值表逐格断言）→ completion → documentSymbol → didChange（坏输入出诊断）→
shutdown/exit。协议回归靠它，不靠 mock。

## 已知限制

- 只支持的 `.tri` 语法以 `trit-core` 解析器为准；多行表达式目前不支持（与 `trit build` 一致）。
- 诊断只有语法层；`trit build` 的穷举复核/e-graph 重写不在 LSP 里跑（代价太高，且属于构建）。
- 没有 `workspace/didChangeWatchedFiles`：文件在编辑器外被改，需要重开或保存触发。
