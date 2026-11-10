# 互操作 conformance runner

独立实现 `interop/spec/v0.md` 的 Rust runner；不依赖主 `trit` Cargo 工作区，用来逐条跑 `interop/vectors/`。

## 运行

```bash
cd interop/runner
cargo run --release
# 或指定向量目录
cargo run --release -- --vectors ../vectors
```

默认读取 `../vectors`；输出每个文件的通过数/总数和总计。任何失败都会打印 vector `id` 与期望/实际，并以退出码 `1` 结束。

## 覆盖

- `k3` / `k3_bitplane`
- `kgate`
- `pack5`
- `belnap4` / `belnap4_bitplane`

## 边界

- 这是规范的独立实现，不是主 Rust 参考实现的封装；两边不一致时以规范/vectors 为准，修一边并补回归。
- 该 crate 是独立 workspace，不进入主 Cargo 工作区，不参与默认 CI。
