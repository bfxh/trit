# Go 三进制参考实现

这是 `trit` 的 Go 语言**参考实现**，不是 Rust 工作区的移植，也不是稳定 API，不参与默认构建。

## 范围

- 平衡三进制整数：`EncodeInt64` / `DecodeInt64` / `Add` / `Negate`（带溢出检查）。
- Kleene K3：`KAnd` / `KOr` / `Kleene.Not`、`KAll`。
- 依赖准入表：`KGate(required, verified)`——`required=Unknown` 时，即使前驱 `True` 也保持 `Unknown`。
- 5 trits/byte：`Pack5` / `Unpack5`（`3^5 = 243`，13 个码点未用）。
- 演示：`cmd/demo/main.go`。

## 非目标

- 不是完整移植：没有 SAT/BDD、网表、ATPG、STA、WASM 绑定、调度器、训练/推理。
- 不是平衡三进制整数库的官方承诺：Rust 侧 `trit` 的定位是三值**逻辑**工具箱；本目录的整数部分是教学配套。
- 不是跨进程协议：`Pack5` 只是本地 wire format 示例，没有版本头、长度、字节序或校验和。跨语言交换必须显式定义协议。
- 不进入默认构建：不在 Cargo 工作区，不被任何 Rust crate 依赖，不参与默认 CI/发布。

## 与 Rust 语义的映射

| Go | Rust / 文档 | 说明 |
|---|---|---|
| `Kleene` / `KAnd` / `KOr` | `trit-core::Ternary`、K3 algebra | 同一套 K3 强合取/强析取 |
| `KGate` | 编排研究文档中的依赖准入表（不在本仓） | 不是 K3 蕴涵；`required=U, verified=T` 仍为 `U` |
| `Trit` / `EncodeInt64` / `Add` | `trit-codec` 的平衡三进制/5 trits/byte | 教学切片；Rust 侧不是整数库 |
| `Pack5` | `trit-codec` 的 5 trits/byte | 本地格式，不是协议 |

## 运行

```bash
cd other-languages/go
gofmt -l .
go vet ./...
go test ./...
go run ./cmd/demo
```

> 本目录不接入默认 CI；修改 Go 参考实现时请手动跑上面的命令。

本机读数（2026-11-07，Go 1.27.3）：`go test ./...` 通过，库包覆盖率 91.0%（`cmd/demo` 只做演示，未计覆盖率）。

## 许可

与仓库一致：MIT 或 Apache-2.0。
