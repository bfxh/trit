# 其他语言参考实现

本目录存放 `trit` 的跨语言**参考实现**与**互操作契约草案**，目的是让其他语言直接读懂三值语义、平衡三进制、5 trits/byte 与准入表，并保证不同实现之间能互相验证，而不是把 Rust 工作区整体移植一遍。

## 硬规则

1. **非默认**：不进入 Cargo 工作区，不被任何 Rust crate 依赖，不参与默认 CI、发布和版本号。
2. **非稳定 API**：参考实现的 API 没有兼容性承诺，可以随时改写或删除。
3. **统一互操作**：稳定语义与字节格式必须遵守 [interop 契约](../interop/spec/v0.md)；前沿想法只能进实验 profile，不得污染稳定核心。
4. **零外部依赖**：参考实现只依赖标准库；不引入模型、训练框架或二进制现成件。
5. **结论仍按 A/B/C/D**：性能/精度结论必须带二进制基线，并按 [`docs/TERNARY-BOUNDARIES.md`](../docs/TERNARY-BOUNDARIES.md) 归类。
6. **保持最小**：每个实现只覆盖“能读懂语义”的最小切片；完整能力以 Rust 工作区为准。

## 语言优先级（2026-11-08 定稿）

| 层级 | 语言 | 定位 | 做什么 | 不做什么 |
|---|---|---|---|---|
| **Tier 0** | **Rust** | 主内核 / 参考实现 | 三值逻辑、SAT/BDD、codec、CLI/WASM；唯一默认实现 | 不被 Java/Go 重写 |
| **Tier 0** | **Assembly** | 三进制/多值热点与验证 | 位切片 K3/Ł3/B3、5 trits/byte、popcount/点积、GF(3)、Belnap 4 值位平面 | 不替代 Rust 的可维护参考实现；不无基线地宣称“更快” |
| **Tier 1** | **C** | 可移植 ABI / 嵌入式 / WASM | header-only K3 + 5 trits/byte + 门表；FFI 基线 | 不做 SAT/BDD |
| **Tier 1** | **C++** | 生态集成 | 游戏、EDA、HFT、Unreal 适配 | 不重复 C 的活 |
| **Tier 1** | **Lean 4** | 形式语义 | K3/Ł3/B3 定律、准入表、编解码不变量；接 `openai/math` 的 Lean 生态 | 不做运行时 |
| **Tier 2** | **Java** | JVM 集成层 | Minecraft、Android/多设备、企业适配 | 不重写内核；不和 Rust 拼性能 |
| **Tier 3** | **Go** | 智能体 / 基础设施 | DeepSeek-Reasonix、Docker/CLI/控制面 | 不扩三进制内核；只保留最小参考 |

> Assembly 与 Rust 同属 Tier 0，但职责不同：Rust 是可维护、可审计的参考实现；Assembly 是**热点/多值路径的性能与可行性验证**。任何“汇编更快”的结论，必须与 C/Rust 同算法、同数据、同基线对比。

Lean 4 形式化入口：[`interop/lean/`](../interop/lean/)（K3/KGate/Belnap4/pack5 核心定理；Lean 4.35.0，无 Mathlib）。`openai/math` 外置评测见 [`interop/lean/openai_math/`](../interop/lean/openai_math/)。

## Java 选型

> 以下按 **MC = Minecraft Java Edition** 理解；如果 MC 指 Mission Critical，选型会变。

**主方案：Java 17 字节码基线 + Java 21 LTS 工具链，纯 Java 核心 + 两个薄适配器。**

- 核心：K3 + 5 trits/byte + `KGate`，零 native 依赖；SAT/BDD/ATPG 等重活通过 JNI/FFM/sidecar 调 Rust。Java 21 上 FFM 仍是 preview，稳定用 JNI 或 sidecar；Java 22+ 才建议 FFM。
- 为什么不是 Java 8/11：新 MC 与企业侧都已经在 17/21；8/11 只维护，不新做。
- 为什么不一上来 Java 25：Android 与多版本 MC 的兼容面更重要；Java 21 是更稳的交集，25 可作为企业侧可选构建。
- 适配器优先级：
  1. **Minecraft（Fabric + NeoForge）**：这是 Rust 三进制最替代不了的表面——mod 必须跑在 JVM，分发走 Modrinth/CurseForge。核心不依赖 Minecraft 类，adapter 只做入口映射，降低版本 churn。
  2. **Android / 多设备**：Java 17 + Gradle；UI 可以用 Kotlin，核心保持 Java；桌面 JavaFX/CLI，游戏可走 libGDX。
  3. **企业后端**：Spring Boot 3 / Quarkus starter，有客户再做；不要把 Spring 塞进核心。
- **“企业级”是工程标准，不是框架**：JUnit 5 + jqwik、JMH、Jazzer、Revapi/japicmp、OWASP dependency-check、CycloneDX SBOM、JPMS、Javadoc、Java 17/21/25 × OS 的 CI、SemVer、签名发布。
- 如果只能选一个适配器：**MC 优先**。多设备可以复用同一核心；企业后端最不“三进制特有”，Rust sidecar 也能服务。

**Rust 能替掉三进制数学，替不掉 JVM 宿主面**——MC mod、Android 运行时、企业中间件，这才是 Java 值得做的那部分。

## 汇编路线（Tier 0，优先级仅次于 Rust）

汇编**一定要做三进制/多值**。这里的“汇编”不是把整个库用汇编写一遍，而是：

1. **热点内核**：位切片 K3/Ł3/B3、5 trits/byte 打包/解包、popcount/点积、GF(3)、Belnap 4 值位平面。
2. **目标 ISA**：x86-64（AVX2/AVX-512/BMI2）、ARM64（NEON/SVE）；RISC-V 作为后续。
3. **验证纪律**：
   - 标量参考 + 穷举真值表 + 随机向量 + 跨字边界 + 全 `+1`/`-1`/交替/单点；
   - 与 C/Rust intrinsics 同算法、同数据对比 cycles/byte；
   - 每条汇编规则必须有差分测试/变异测试；编译错、崩溃、零断言都不算检出；
   - 不得绕过边界检查或引入未定义行为；需要侧信道审查。
4. **互操作**：汇编实现必须通过统一 golden vectors；不得自创语义。
5. **结论口径**：汇编在二进制 ISA 上是 **B 类（二进制模拟三态）优化**，不是 A 类原生红利；SAT/BDD/ATPG 是 D 类算法瓶颈，不作为汇编优先目标。

## 三进制互操作契约

稳定核心、实验 profile 与 golden vectors 已拆分到 [`interop/`](../interop/README.md)：

- 规范：[`interop/spec/v0.md`](../interop/spec/v0.md)
- 向量：[`interop/vectors/`](../interop/vectors/)
- 格式：[`interop/schema/vector.schema.json`](../interop/schema/vector.schema.json)

实现必须通过所声明 profile 的全部 vectors；手写穷举 vectors 是规范，随机长向量只作回归。不得用被测实现生成期望值自证。

## 当前实现

| 语言 | 路径 | 范围 | 运行 |
|---|---|---|---|
| C | [`c/`](c/) | K3、KGate、5 trits/byte（含尾部/错误），header-only | `cd c && make test`；conformance 见 [`c/README.md`](c/README.md) |
| Go | [`go/`](go/) | 平衡三进制整数、K3、5 trits/byte、KGate | `cd go && go test ./...` |

## 其余语言评估

| 语言 | 定位 | 理由 | 边界 |
|---|---|---|---|
| **C++** | Tier 1 | 游戏/EDA/HFT/Unreal 有 FFI 需求；长期语言 | header-only；不重复 C |
| **Zig** | 可选 | 现代 C 替代；显式分配器、comptime、C ABI 友好 | 与 C 同范围；有消费方再做 |
| **C#** | 可选 | .NET 生态、`Span<T>`/SIMD、企业集成 | 有 .NET 消费方再做 |
| **Python** | 教学/对齐 | 教学、原型、NumPy 位并行演示；纯 Python 不适合性能路径 | 按本仓 `AGENTS.md` 只作对齐基线，不交付生产实现 |
| **TypeScript/JS** | 演示/胶水 | 浏览器演示与 WASM 绑定；`BigInt` 慢 | 不手写高性能内核 |
| **Swift** | 可选 | Apple 生态、值类型和 SIMD 友好 | 有 iOS/macOS 消费方再做 |
| **Julia** | 研究 | 数值/多重派发适合三值浮点与数组实验 | 只做研究原型，不作为生产内核 |
| **Haskell/OCaml/F#** | 语义/教学 | ADT 表达三值语义自然 | 只做语义与真值表，不做性能 |
| **PHP/Ruby/Perl/Lua/R** | 不建议 | 除非有明确宿主嵌入需求，维护成本大于收益 | 优先通过 C/WASM 复用 |

## 新增语言的最低要求

- 一个可读的 `README.md`：范围、非目标、与 Rust/Lean 语义的映射、运行方式。
- 一个最小测试集：K3 真值表、平衡三进制往返、5 trits/byte、边界与溢出。
- 必须通过互操作 golden vectors；不得自创语义或字节顺序。
- 不引入外部依赖；不复制 Rust 的内部实现细节。
- 在 [`docs/TERNARY-BOUNDARIES.md`](../docs/TERNARY-BOUNDARIES.md) 的 A/B/C/D 框架下写结论；没有二进制基线不算性能结论。

## 一句话排序

**Rust 内核 + Assembly 热点/多值验证；C/C++/Lean 4 长期；Java 做 JVM 宿主；Go 做智能体/基础设施。**
