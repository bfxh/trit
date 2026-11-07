# 三进制互操作（interop）

本目录是 `trit` 的**跨语言互操作规范与 golden vectors**。目标不是再定义一套语义，而是让 Rust、Go、汇编验证、未来的 C/C++/Lean 4/Java 实现能用同一组向量互相判红。

状态：**v0.1.0 冻结**。只允许向后兼容增补；破坏性变更走新 major 或新 profile。

## 目录

| 路径 | 内容 |
|---|---|
| [`spec/v0.md`](spec/v0.md) | 规范正文：K3、KGate、pack5、Belnap4 的语义、编码、错误与版本规则 |
| [`schema/vector.schema.json`](schema/vector.schema.json) | vector 文件的 JSON Schema（draft 2020-12） |
| [`vectors/k3.json`](vectors/k3.json) | K3 NOT/AND/OR 语义向量（21） |
| [`vectors/k3_bitplane.json`](vectors/k3_bitplane.json) | K3 位平面 encode/decode 与 `(1,1)` 非法（7） |
| [`vectors/kgate.json`](vectors/kgate.json) | `KGate` 3×3 准入表（9） |
| [`vectors/pack5.json`](vectors/pack5.json) | pack5 全量组合、尾部、unpack、错误（657） |
| [`vectors/belnap4.json`](vectors/belnap4.json) | Belnap4 NOT/AND/OR（36） |
| [`vectors/belnap4_bitplane.json`](vectors/belnap4_bitplane.json) | Belnap4 位平面 encode/decode（8） |

合计 **738** 个 vector，ID 全局唯一。

## Conformance 规则

1. 实现声明某个 profile，就 MUST 通过该 profile 的全部 vectors；不得跳过失败项。
2. 错误向量 MUST 返回结构化错误类别（`invalid_bitplane` / `invalid_byte` / `short_input` / `invalid_trit`），不得 panic、钳位或返回默认值。
3. `expected` 按 JSON 深度相等比较；数组顺序必须一致。
4. 未知 `op` MUST 拒绝，不得忽略。
5. Vectors 是**规范来源**：不得用被测实现生成期望值来自证；新增 vector 必须能由规范真值表/公式复算。
6. 手写穷举小集合（K3/KGate/Belnap4/pack5）是 normative；未来的随机长向量只能作回归，不得替代穷举。

## 各实现怎么接

- **Rust**：参考实现；conformance runner 下一步加到 `interop/runner`（Rust，独立 workspace）。
- **Go**：`other-languages/go` 的测试读取同一组 JSON，逐条比较；不复制期望值。
- **汇编验证**：`other-languages/asm` 的对拍测试读取同一组 JSON；K3/pack5/GF(3) 各自覆盖对应 profile。
- **未来语言**：先实现 spec 的语义与 pack5，再跑 vectors；不要先写优化。

## 版本与新增 vector

- `spec_version` 当前 `0.1.0`；每个 vector 文件必须与规范版本一致。
- 新增向量：patch/minor 版本；已有 vector 的 `id`、输入、期望值 MUST NOT 改。
- 破坏性变更：新 major 或新 profile id；不得静默改语义。
- 新想法（Belnap 5/9 值、稀疏三值、压缩流、零拷贝 FFI 布局）MUST 使用新 `profile` id，放在 `experimental/` 下，不得污染 stable core。

## 参考优先级

1. **Lean 4**（形式语义目标，落地后优先）
2. **本规范 + golden vectors**
3. **Rust 参考实现**

Lean 4 落地前，规范与 Rust 不一致时以规范为准，修 Rust 并补回归 vector；Lean 4 落地后以 Lean 4 的语义定义为准。

## 下一步

- Rust conformance runner（读 vectors、逐条判红、输出 profile 汇总）。
- Go/asm 测试接同一组 vectors。
- 可选：非默认 `conformance` CI job（不改变主 Rust 工作区默认门禁）。
- 跨进程容器 v0.2（magic/version/profile/trit_count/payload_len/CRC32C）；在 vectors 补齐前不声称支持。

## 许可

与仓库一致：MIT 或 Apache-2.0。
