# 三进制构建工具链（TTIR → TRB1）——定位、论文依据与边界

> 状态：**M0 已落地并可运行**（`trit-build`），M1/M3 只有设计。
> 规则来源：`docs/TERNARY-BOUNDARIES.md` 的 A/B/C/D 记账；任何"更快/更省"必须给二进制基线。
> 本文里的论文都经过 OpenAlex 核验（DOI 可查），未核验的一律标"未证"。

## 0. 一句话

**不重写 LLVM，重写"三进制独有的那一层"。** LLVM/Rust 负责机器码、链接、OS、包管理；
本工具链负责三值语义（K3/Ł3/B3）、`X` 的传播与精确复核、基 3 打包与 GF(3) 检错、
以及三值产物（`TRB1` / 双导轨 Verilog / GPU 内核）的生成与增量构建。

## 1. 论文与技术（哪些真能借）

| 来源 | DOI / 标识 | 借什么 | 不借什么 |
|---|---|---|---|
| LLVM（Lattner & Adve, CGO 2004） | `10.1109/cgo.2004.1281665` | 分层 IR + pass 契约 + "分析结果可复用" | 二进制后端（SSA、寄存器分配、指令选择） |
| MLIR（CGO 2021） | `10.1109/cgo51591.2021.9370308` | dialect 分层的 lowering 结构：TTIR → 目标方言 | 通用 GPU/张量方言栈 |
| egg（POPL 2021） | `10.1145/3434304` | e-graph 等价饱和做三值代数重写 | 直接套规则：K3 的规则必须逐条枚举 3^n 自证 |
| BitNet b1.58（2024） | `arXiv:2402.17764` | 三值权重的存储与推理形态（MOD/GGUF 入向） | 训练/推理性能结论 |
| 平衡三进制序列压缩（TVLSI 2019） | `10.1109/tvlsi.2019.2906678` | 基 3 打包/解包的码率与解压路径 | 硬件实现细节 |
| 组合平衡三进制数制（2018） | `10.7753/ijcatr0707.1008` | 三进制整数表示的动机 | 性能对比（无同口径基线） |
| Setun（Brusentsov, 1958） | 历史文献，无 DOI | 只作"三值机器曾真实存在"的动机 | **不作性能证据**（器件与今天不可比） |

结论：可借的是**结构**（分层 IR、pass 管线、e-graph、基 3 打包），
不能借的是**二进制后端的优化结论**——那套东西的收益建立在 2 的幂寻址和固定位宽上。

## 2. 三进制独有的是什么（先分类再说话）

- **A 语义**：`X` 是原子值。K3 下 `a & ~a = 0`（确定），B3 下才是 `X`；这个区分是布尔 + 错误码压不出来的。
- **A 复核**：任何"优化"都必须能被 3^n 穷举或精确传播证伪；本工具把"化简后不等价"当**构建失败**，不是警告。
- **A 打包**：5 trit/字节 = 243 ≤ 256（1.6 bit/trit），比双导轨 2 bit/trit 省 20%，比理论下限 1.585 只差 0.9%
  （这项在 `trit-codec` 已实现；M0 因发布图约束先用双导轨，见 §3）。
- **B 税**：宿主是二进制，运行前必须解包/查表；"原生三值器件"不存在于本机。
- **D 无关**：缓存、增量、并行调度、错误恢复——任何进制都要付。

## 3. 架构：TTIR-0（已实现）到 M3（设计）

```text
.tri / Verilog 子集
      │ 前端：解析 + 预算检查（3^n ≤ 上限）
      ▼
   TTIR-0：AST（K3/Ł3/B3 语义可选）
      │ pass 1：化简（代数规则，标出用到的规则）
      │ pass 2：精确复核（穷举等价；未来接 propagate_exact 省 3^n）
      ▼
   产物层
   ├── truth.csv      真值表（人读/对拍）
   ├── truth.tritbin  TRB2：魔数+版本+语义+nvars+长度 + 5 trit/字节 + GF(3) CRC-3
   ├── lookup.c       主机查表函数（交给 cc/rustc 编译 —— 边界示范）
   ├── design.v       双导轨 Verilog（仅 `--emit-verilog`，kleene）
   └── build.json     输入/选项/产物哈希/复核结论
      │
      └── 驱动：内容哈希缓存（FNV-1a）+ `--check` 产物完整性
```

**产物格式（TRB2，v2）**：`TRB2` | version u8 | algebra u8 | nvars u8 | len u32 LE |
载荷 = `len` 个输出 trit ‖ CRC-3（3 个 trit），按 5 trit/字节小端打包（权重 1/3/9/27/81）。

**为什么 M0 用双导轨、M2 才换基 3**：`trit-codec`（5 trit/字节 + GF(3) CRC-3）是
`publish = false` 的实验层，而本仓发布图门禁**禁止实验 crate 互相依赖**（`trit-build` 也是实验层）。
M0 先做"语义/复核/增量"，打包密度留到 M2。

**M2 的取舍（如实）**：既没有把打包下沉 `trit-core`，也没有依赖 `trit-codec`，而是在
`trit-build/src/trb.rs` 写了一份**同构的精简实现**。代价是同一格式有两份代码，必须靠
`runtime/trit_runtime.c`（第三份、独立语言）交叉验证来兜底；codec 稳定后应统一，删掉这里的副本。

## 4. 现在能跑什么（实测，2026-10-25）

```bash
# 源文件：第一行非注释内容 = 表达式
printf '# demo\n(a & b) | !c\n' > demo.tri
trit build demo.tri --out out
trit build demo.tri --out out          # cached=true
trit build demo.tri --out out --check  # 校验产物哈希
```

实测结果（`crates/trit-cli/tests/build.rs`，3 条集成测试绿）：

| 项 | 证据 |
|---|---|
| 3 变量 → 27 行真值表 | `truth.csv` 28 行（含表头） |
| `TRB2` 大小 | 11 字节头 + 6 字节载荷 = **17 B**（27+3 trit，5 trit/字节）；M0 的 TRB1 是 35 B |
| 主机对拍 | `lookup.c` 用 gcc 编译后 27/27 行与 `truth.csv` 一致 |
| 增量 | 同源二次构建 `cached=true`，不重写产物 |
| 完整性 | 篡改 `truth.tritbin` 后 `--check` 退出非 0 |
| 复核 | 化简不等价 ⇒ 构建失败，不写任何产物 |

## 5. 继续靠 LLVM / Rust 的部分（刻意不自己写）

| 职责 | 归谁 | 理由 |
|---|---|---|
| 机器码生成、寄存器分配、指令选择 | LLVM / Cranelift | 与基数无关，重写只是更差 |
| 链接、目标文件格式、调试信息 | LLVM 工具链 | 平台 ABI 由二进制世界定义 |
| 主机侧执行（`lookup.c`、Rust harness） | cc / rustc | 三值语义已固化进表，执行不需要三值硬件 |
| 包管理、CI、测试框架 | Cargo / GitHub Actions | 与进制无关 |
| 加密、网络、GUI | 现成库 | 与进制无关 |

**判据**：如果某项在三进制世界里和二进制世界里"要做的事一样"，就不该自己写。

## 9. 双轨并行：谁先、谁吃剩余（`--manifest`）

用户 2026-10-25 的取舍：**三进制轨（本工具）优先**，二进制轨（cargo/rustc/LLVM）用剩余槽位并发，
双方都别等对方；支持谁由 `--prefer` 决定。

| `--prefer` | 谁先 | 行为 |
|---|---|---|
| `ternary`（默认） | 三进制轨 | 三进制待办且槽位未满 ⇒ 只派三进制；二进制只在三进制排空或槽位已满时上 |
| `binary` | 二进制轨 | 镜像：三进制吃剩余 |
| `balanced` | 清单顺序 | 单 FIFO，按提交顺序 |

**不做抢占**：二进制任务跑起来不会被打断（cargo 没有安全挂起点）；
"优先"的含义只是**不派发**，不是强杀。

清单（路径相对清单文件）：

```json
{"jobs":[
  {"name":"a","kind":"ternary","source":"a.tri"},
  {"name":"lib","kind":"binary","cmd":["cargo","build","-q","-p","trit-core"]},
  {"name":"b","kind":"ternary","source":"b.tri","out":"out/b"}
]}
```

```bash
trit-build --manifest jobs.json --jobs 4 --dry-run        # 只看分配
trit-build --manifest jobs.json --jobs 4                  # 真并行
trit-build --manifest jobs.json --jobs 4 --prefer binary  # 反过来支持二进制
```

实测（本机，7 个任务 / 4 槽位：6 个 `.tri` + 1 个 `cargo build -p trit-core`）：

| 指标 | 值 |
|---|---|
| wall | **171 ms** |
| busy（各任务耗时之和） | 205 ms |
| speedup = busy/wall | 1.2× |
| 三进制单任务 | 4–8 ms（与 163 ms 的 cargo 并行，不被阻塞） |
| 失败 | 0 |

**如实说明**：M0 的三进制任务只有毫秒级，所以"总加速"只有 1.2×——收益体现在
"cargo 跑 163 ms 期间，6 个三进制构建已经全部完成"，而不是把 cargo 变快。
两侧都重（大网表 + 大 crate）时才有明显加速，那要等 M1 的真实工程。

## 10. M1 实测：Verilog 子集进三进制轨

不自己写 Verilog 解析器：直接复用 `trit-circuit::parse_verilog`（与 BLIF/AIGER 共用同一套门模型），
本工具只做"选主输出 → 3^n 仿真 → 与表达式链路对拍 → 出同样的四件产物"。

```bash
trit-build d.v --out out --expect "(a & b) | !c"   # 双链路对拍通过才产出
trit-build multi.v --out out --output z             # 多输出必须点名
```

**对拍判据（硬）**：Verilog 链路（`propagate`，K3/Ł3/B3）与 `.tri` 链路（`simplify` + 穷举）的
真值表必须**逐行相同**；不同即构建失败，不写 `build.json`。

实测（`d.v` = `(a & b) | !c`，3 输入）：

| 项 | 证据 |
|---|---|
| 行数 | 27（`options.assignments`） |
| 两条链路 | `truth.csv` **逐字节相同** |
| 错表达式 | `--expect "a | b | c"` ⇒ 退出 1、`两条链路不一致`、无 `build.json` |
| 多输出 | 不给 `--output` ⇒ 报错并列出可选输出；`--output z` ⇒ 9 行 |
| 并行清单 | `{"kind":"verilog"}` 归三进制轨，可与二进制任务同跑 |

**M1 遗留已由 M2 补上**：位级双导轨 Verilog **导出**（`--emit-verilog`）在 M2 落地，见 §11。

## 11. M2 实测：TRB2 打包 + 解码 + 双导轨导出

```bash
trit-build e.tri --out out                        # truth.tritbin 现在是 TRB2（17 B）
trit-build --decode out/truth.tritbin --out dec   # 校验 CRC-3，重出 truth.csv / lookup.c
trit-build e.tri --out out --emit-verilog         # 多出 design.v（两根位线/信号）
```

| 项 | 证据（本机 `cargo test -p trit-build` + 实跑） |
|---|---|
| 体积 | 27 trit → **17 B**；12 变量（531,441 trit）→ **106,300 B** |
| 与旧格式比 | 按 TRB1 位宽（11 + 16·⌈n/64⌉ + 8）算 **132,883 B** ⇒ **−20.00%**；TRB1 是算的，不是同机测得（M0 实现已被替换） |
| 解码-重编码 | 3 种语义 × nvars 0..=4，`decode → encode` **逐字节相同**（含 5-trit 打包的 padding） |
| 非法输入 | 字节 ≥243、短一字节、翻位后 CRC、`len ≠ 3^nvars` 全部显式报错 |
| C 运行时 | `runtime/trit_runtime.c` **独立**实现解包 + CRC-3；解出的 27 trit 与 `truth.csv` 逐 trit 相同；篡改后退出非 0 |
| 双导轨导出 | `design.v` 用测试内位级解释器求值，与真值表 **27/27** 一致；`(1,1)` 非法组合 **0 行** |
| 门禁 | `trit-build` 26 条测试；变异 **9/9**（新增 4 条锚点：非法字节、语义字节、K3 与门负轨、`--emit-verilog` 缓存键） |

**M2 未做/未证（如实）**
- **没有 HDL 仿真器**：本机无 iverilog / verilator / yosys ⇒ `design.v` 只被测试内的位级解释器验过，
  没进过真正的 Verilog 仿真或综合。
- **GPU 导出未做**：M2 原计划里的 `--emit gpu` 没有目标格式可依（仓里还没有 GPU 三值后端）。
- `--decode` 重建的 CSV 变量名只能是 `v0..vN`：TRB2 不存变量名，**值**逐 trit 相同、**列名**与原表不同。
- 只有 kleene 能导出 Verilog；Ł3/B3 的蕴含/常量规则不同，直接报错而不是猜。
- 格式实现现在有 Rust（`trb.rs`）+ C（`trit_runtime.c`）两份，靠交叉对拍兜底；统一到 `trit-codec` 是后账。

## 12. M3 实测：位并行算表 + e-graph 重写

```bash
trit-build big.tri --out out        # 12 变量 = 531,441 行
```

**位并行**：每轮同时算 64 组赋值。门语义**不手写公式**——先从 `Algebra` 取该门在 3×3
输入组合上的结果表，再按最小项铺到 pos/neg 位平面。K3/Ł3/B3 共用一份代码，
Ł3 的 t-norm、蕴含这些容易写错的地方不需要人工推公式。
行序是硬约束：`code` 以 3 为基、`0→False / 1→Unknown / 2→True`（与 `truth.csv` 的 -1/0/1 对齐）。
`trit-core::bits::Evaluator` 用的是 1↔2 对调的另一套行序，**没有复用它**。

**e-graph**：规则**先自证、后加载**——对规则里出现的 ≤3 个变量穷举 3^k 组取值，
用生产同一份 `trit_eval::eval` 比较左右两边，任何一组不等这条规则就不进图。
因此同一张规则表可以喂三种语义：蕴含消去在 K3 成立、在 Ł3 不成立，会自动只出现在 K3 的规则集里。
重写后用节点数最小提取；整条链再用穷举复核一次（自证只覆盖单条规则，不覆盖实现 bug）。

实测（release，本机）：

| 项 | 读数 |
|---|---|
| 12 变量算表 | 标量 **299 ms** → 位并行 **23.8 ms** |
| 化简 / 重写 / 等价复核 | 28 µs / 169 µs / 11.6 ms |
| `truth.csv` / `lookup.c` 序列化 | 57.8 ms（19.6 MB）/ 18.8 ms |
| CLI 端到端（12 变量） | debug 1.18 s → **0.53 s**；release **0.23 s** |
| 产物 | 与 M2 **逐字节相同**（`truth.csv` / TRB2 / `lookup.c`） |
| 规则集 | 18 条；K3 过 17 条，Ł3 少 `implies-elim`，B3 少吸收律——全部逐条自证 |

**顺带修正的语义坑**：`Expr::Xor` 在本仓是**强加法 ⊕**，K3/B3 下 U 传染
（`F ⊕ U = U`，而格运算 `F ∧ U = F`）。M2 的双导轨导出按布尔异或写，导出的 `design.v`
与 `truth.csv` 对不上；M3 修正并补了 `^` 的导出对拍用例。

**M3 未做/未证**
- 算表已不是瓶颈：剩下 ~150 ms 在 CSV / `lookup.c` / FNV 哈希 / 写 19.6 MB 盘，未优化。
- e-graph 只跑有限轮（32），没有饱和求解；提取只按节点数代价，没有代价模型。
- 规则库 18 条、没有 e-matching 索引，大表达式的重写开销未测。
- 位并行是单线程，按 e-class 切分的多线程未做。

## 6. 阶段与验收

| 阶段 | 内容 | 验收判据 |
|---|---|---|
| **M0（已完成）** | `.tri` → 化简 + 穷举复核 → TRB1/CSV/C + 缓存 | 3 条集成测试；C 主机 27/27 对拍；`--check` 篡改必红 |
| **M1（已完成）** | Verilog 子集**读入** + 双链路对拍 + 并行清单 + 双导轨**导出** | 两条链路逐行一致 ✅；导出与真值表逐 trit 一致 ✅（§11） |
| **M2（已完成，GPU 除外）** | 基 3 打包 + GF(3) CRC-3、TRB2 解码器（`--decode`）+ C 运行时、`--emit-verilog` | 解码-重编码逐字节相同 ✅；非法字节显式报错 ✅；体积基线 20% ↓ ✅；C 运行时逐 trit 对拍 ✅ |
| **M3（已完成）** | e-graph 重写（规则逐条 3^n 自证）+ 位并行算表替掉逐赋值穷举 | 规则自证 100%（不过就不加载）✅；产物与 M2 逐字节相同 ✅；12 变量算表 299→23.8 ms ✅（§12） |

## 7. 不做什么

- **不做二进制后端**：没有三值硬件之前，ILP/寄存器分配/向量化都该留给 LLVM。
- **不做通用语言**：`.tri` 只是一种“可复核的三值表达式”，语法故意最小；扩展要等 M1。
- **不承诺速度**：M0 的收益是“更小、可查错、可复核”，不是“更快”。任何速度声明必须同机基线。
- **不把 `X` 当 `-1`**：这是 `TERNARY-BOUNDARIES.md` 的硬规则，工具层同样适用。

## 8. 未验证清单

- 真实 Verilog 工程的 M1 前端（端口/时序/多驱动）未做，只有设计。
- e-graph 规则在 K3 下的完备性未证（M3）。
- `TRB2` 的 Python/JS 解码器未写；当前只有 Rust（`trb.rs`）与 C（`runtime/trit_runtime.c`）两份实现。
- `design.v` 未过真实 HDL 工具（iverilog / verilator / 综合器），只有测试内位级解释器（§11）。
- 双导轨导出只覆盖 kleene；Ł3/B3 显式拒绝，未证明可无损降成两根位线。
- GPU 导出（M2 原计划的 `--emit gpu`）未做。
- `--decode` 不恢复变量名（TRB2 只有 nvars），重建的 CSV 列名是 `v0..vN`。
- e-graph 没有饱和求解与代价模型（只按节点数提取）；规则库 18 条，没有 e-matching 索引。
- 位并行是单线程；12 变量构建剩下的时间在 CSV/哈希/写盘（19.6 MB），未优化。
- 性能只有构建时间（毫秒级），没有与 LLVM 同口径的对比——本阶段不主张性能优势。
- Setun/平衡三进制的历史细节只引二手文献，未核原始论文。
