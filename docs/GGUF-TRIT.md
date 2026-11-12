# 三值权重 GGUF 容器（`trit-export`）

状态：**已实现**（ops 层）。只覆盖读写三值 2D 张量所需的最小 GGUF v3 子集，
不是上游 GGUF 的完整实现；未知类型/非 2D/非三值张量一律显式报错。

## 1. 文件布局（小端）

```text
magic "GGUF" | version u32=3 | tensor_count u64 | metadata_kv_count u64
metadata × count: key(string) | type u32 | value
tensor_info × count: name(string) | n_dims u32 | dims u64[n_dims] | type u32 | offset u64
padding 到 32 字节
data: 每个张量按 offset（相对 data 起点）写入，张量之间按 32 字节对齐
```

- `string` = `u64` 长度 + UTF-8 字节（与 GGUF 规范一致）。
- 元数据只支持 `u32`(4) / `f32`(6) / `string`(8)；其他类型报 `MetaType`。
- 张量只支持 2D，`dims = [cols, rows]`（`ne[0] = cols`，与 GGML 的连续维口径一致）。

## 2. 三值张量类型 `TYPE_TRIT = 100`

数据区单张量布局：

```text
trits   : rows×cols 个 trit，trit-codec 5 trits/byte（canonical padding，低位在前）
scales  : rows × f32（小端）
bias    : rows 个 trit，trit-codec 5 trits/byte
```

- `offset` 指向 trits 起点；`scales` 紧跟其后；`bias` 在最后。
- 大小：`ceil(rows×cols/5) + rows×4 + ceil(rows/5)` 字节（`trit_export::tensor_size`）。

## 3. 与 `trit-codec` 的一致性

trits 与 bias 直接调用 `trit_codec::pack::pack`，解码用 `unpack_canonical`：
非法字节（243..=255）与非规范 padding 都会被拒绝。测试用
`pack::pack` 的字节串在 GGUF 输出里逐位查找，钉住「逐位一致」。

## 4. 边界（未证栏）

- 没有写 GGUF 的字符串数组 / bool / u64 元数据；遇到会报错。
- 没有量化元数据（`general.quantization_version` 等）与多文件分片。
- 没有与 llama.cpp 的实测互操作（上游不认识 `TYPE_TRIT`，这是本仓自定义类型）。
- `scales`/`bias` 是必填字段；没有「无 scale」「无 bias」的紧凑变体。
