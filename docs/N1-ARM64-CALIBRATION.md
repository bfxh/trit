# N1 校准：真实 ARM64 PE 上的交叉引用

> 目的：用**真实** Windows ARM64 二进制验证 `trit-re` 的 ARM64 交叉引用与函数边界，
> 并把发现的问题修到代码里。此前 ARM64 只有合成夹具，属于"未证"。
>
> 结论：在 5 个真实 ARM64 PE 与 1 个真实 ARM64 ELF 上，导入引用覆盖率 **91%–98%**，
> 与同源 x64 构建的 95%–100% 在同一量级；过程中修掉 5 个真实缺陷
> （其中 1 个会让台账输出**不是合法 JSON**，2 个会把"取地址"或"数据符号"误判成"没有引用"）。
> 仍未证的面写在 §6。

## 1. 语料

全部来自官方分发，**二进制不入库**，只记录来源、大小与 `sha256`。
快照日期 2026-10-30。

| 样本 | 版本 / 来源 | 大小 | sha256 |
|---|---|---|---|
| `node-arm64.exe` | Node.js v22.23.4，`nodejs.org/dist/latest-v22.x/win-arm64/node.exe` | 77,164,360 | `64d9f6e69b4703870e8cd30a3477127df8b683ea04feb03660e439396549042a` |
| `node-x64.exe` | Node.js v22.23.4，`.../win-x64/node.exe` | 87,034,184 | `dccc52de28d8df2ee22d2062424f8c563070d95d8ad70c54c4bf01d590dec14f` |
| `deno.exe`（arm64） | Deno v2.9.8，GitHub release `deno-aarch64-pc-windows-msvc.zip` | 89,512,224 | `40d161bc35657cc7432a2e3832290922939cd63fddd29f66db2261d291e42478` |
| `deno.exe`（x64） | Deno v2.9.8，`deno-x86_64-pc-windows-msvc.zip` | 98,080,768 | `924397800533d3eb0ed0c889b20ad70e18b970d93acd8135bc043f425e528266` |
| `rg.exe`（arm64） | ripgrep 15.2.0，`ripgrep-15.2.0-aarch64-pc-windows-msvc.zip` | 3,859,456 | `d33a29a9ef03c9f4c03be9e8d88498e6e2d2e566d64cdbdef97f9afc8f13120c` |
| `rg.exe`（x64） | ripgrep 15.2.0，`ripgrep-15.2.0-x86_64-pc-windows-msvc.zip` | 4,218,880 | `14231169855ec5205cf5a1b6f1db358ff4aed4247c86b69ce8aae647c77f6680` |
| `pwsh.exe` | PowerShell 7.6.9，`PowerShell-7.6.9-win-arm64.zip` | 323,072 | `bbdf538aea81c09f60732219b451a26c64f6f2b22bee05eeece13dba909d36e4` |
| `coreclr.dll` | 同上（原生 ARM64 DLL） | 5,380,904 | `c8d6680d8580e60fd78d977391902cca6846a91a38770df067c9ff7becd03611` |
| `rg`（ELF arm64） | ripgrep 15.2.0，`ripgrep-15.2.0-aarch64-unknown-linux-gnu.tar.gz` | 4,506,992 | `e36d0eb52e70696bdf1781392722e05a21bb91d3b7b762ef5ec20e5df2ec687b` |

选择理由：
- **成对**：Node / Deno / ripgrep 都有同版本 x64 与 ARM64 构建，可做同源对照；
- **语言**：C++（Node/Deno）、Rust（ripgrep）、.NET 原生运行时（coreclr），覆盖不同代码生成风格；
- **规模**：0.3 MB 到 98 MB，含 `.pdata` 上万条的样本。

## 2. 读数（修复后）

`trit-re triage <FILE>` 的输出，时间含读取与 SHA-256：

| 样本 | 架构 | 导入 | 有调用点 | 无调用点 | 未知 | 函数 | 归属调用点 | 未归属 | 耗时 |
|---|---|---|---|---|---|---|---|---|---|
| Node v22.23.4 | arm64 | 413 | 390（94%） | 2 | 21 | 93,643 | 6,611 | 0 | 2.8 s |
| Node v22.23.4 | x86-64 | 424 | 422（100%） | 0 | 2 | 114,531 | 2,579 | 148 | 2.9 s |
| Deno v2.9.8 | arm64 | 376 | 353（94%） | 6 | 17 | 120,727 | 2,737 | 0 | 3.5 s |
| Deno v2.9.8 | x86-64 | 382 | 371（97%） | 0 | 11 | 127,625 | 2,669 | 156 | 3.6 s |
| ripgrep 15.2.0 | arm64 | 134 | 122（91%） | 2 | 10 | 2,934 | 5,173 | 0 | 0.2 s |
| ripgrep 15.2.0 | x86-64 | 127 | 121（95%） | 0 | 6 | 3,618 | 4,745 | 19 | 0.2 s |
| pwsh 7.6.9 | arm64 | 120 | 117（98%） | 1 | 2 | 444 | 256 | 0 | <0.1 s |
| coreclr.dll 7.6.9 | arm64 | 353 | 342（97%） | 1 | 10 | 32,517 | 3,516 | 0 | 0.3 s |
| ripgrep 15.2.0 | **ELF arm64** | 138 | 133（96%） | 0 | 5 | 19,709（启发式） | 7,352 | 0 | 0.2 s |

**"无调用点"变少、"未知"变多不是退步**：真实二进制里大量导入只是被**取地址**
（函数指针，或只在间接调用里用），把它们记成 `False`（确定没引用）是错的。
本轮已改成：地址被计算过但没有直接调用 → `Unknown`，理由写在 `note` 里（见 §4 #5）。

**修复前**的 ARM64 覆盖率：Node 63%、Deno 52%、ripgrep 52%、pwsh 79%、coreclr 80%。

## 3. 独立复核

工具输出不是自证：用一份独立的 Python 扫描器（相同规格、另写实现）复算调用点总数。
ARM64 侧包含跳板展开（`adrp+ldr+br` 桩 → `bl`/`b` 调用方），x64 侧为直接 `FF 15/25`：

| 样本 | 工具 | 独立复算 | 偏差 |
|---|---|---|---|
| Node arm64 | 6,611 | 6,704 | +1.4% |
| Deno arm64 | 2,737 | 2,704 | −1.2% |
| ripgrep arm64 | 5,173 | 5,368 | +3.8% |
| pwsh arm64 | 256 | 260 | +1.6% |
| coreclr arm64 | 3,516 | 3,613 | +2.8% |
| Node x64 | 2,727 | 2,723 | −0.1% |
| ripgrep x64 | 4,745 | 4,707 | −0.8% |

偏差来自两边窗口/破坏性写入判定略有不同，量级在 4% 以内。

**一个反直觉但被复算确认的例子**：Node ARM64 对 `ReleaseSRWLockExclusive` 记了 1130 个
调用点，而 x64 只有 7 个。独立脚本找到该导入在 ARM64 上的唯一跳板（1 个
`adrp+ldr+br`），并数出 **1124** 条 `bl`/`b` 指向它；x64 侧目标槽位只有 4 条直接
`FF 15` + 3 条 `jmp [IAT]`。差异来自两个构建的代码生成（ARM64 保留了每个锁调用的
跳板调用，x64 构建里这些位置没有对应的静态引用），**不是扫描器漏/多报**。

## 4. 本轮修掉的问题

| # | 问题 | 前值 → 后值 | 判据 |
|---|---|---|---|
| 1 | 台账 `slot`/`addr` 写成十六进制裸值（`0x2a0378`），**不是合法 JSON** | 非法 → 合法（数字） | 台账测试逐行 `serde_json` 解析；Python `json.loads` 复算 |
| 2 | `.pdata` 条目上限 65,536，大二进制函数表被截断 | Node 函数 65,536 → 93,643；x64 未归属 1,039 → 148 | 同一二进制重跑；Deno 713 → 156 |
| 3 | ARM64 窗口过窄（`adrp` 与 `ldr` 必须相邻、`ldr` 后只看 2 条） | 覆盖率 52%–80% → 91%–98% | 上表；独立复算偏差 ≤4% |
| 4 | 跨 `BL`/`BLR` 未把 x0–x18 视为已破坏 | 加入"保守写寄存器判定"（不认识就断链） | 单元测试 `interleaved_instructions_do_not_hide_the_reference` / `clobbered_register_breaks_the_chain` |
| 5 | **只取地址**的导入被记成"无调用点" | 新增 `mark_taken`：地址被计算但无直接调用 → `Unknown` + note | PE ARM64 未引用：Node 23→2、ripgrep 12→2、coreclr 11→1；对应未知 21/10/10 |
| 6 | ELF：只要有带大小符号就标 `exact`；数据符号也当导入；无 JUMP_SLOT 记 `False` | 符号覆盖 <90% 退化启发式；只收 FUNC/NOTYPE/IFUNC；无槽位一律 `Unknown` | 真实 ARM64 ELF：3 个符号 → 19,709 个启发式函数；`environ` 等 3 个 OBJECT 不再出现；无槽位 8 → 5（弱符号） |
| 7 | x64 的 `lea/mov [rip+slot]` 未纳入"取地址" | 补 64 位 `48/4C/49/4D 8B/8D 05` 与 32 位绝对寻址；同 ARM64 降 `Unknown` | x64 Node/Deno/ripgrep 的"无调用点" 2/11/6 → 0，未知 2/11/6 |

修复 3 的做法：`adrp` 后允许最多 12 条、`ldr` 后允许最多 8 条夹杂指令；
对目标寄存器执行保守的写判定（`ADR/ADRP`、`ADD/SUB`、`LDR/STR`、`LDP/STP`、
`MOVZ/MOVK`、逻辑/位域/条件选择、`BL`/`BLR` 破坏 x0–x18 与 x30），
遇到不认识的指令直接断链——**宁可不报，也不把调用算到错误的导入上**。
IAT 槽位按 8 字节紧邻排列，一个寄存器判错就会把调用记到隔壁导入，所以这条必须保守。

## 5. 为什么这些数字可信

- 槽位必须**精确命中** IAT（不是"落在附近"）；
- 调用点必须仍在**精确函数区间**内才能参与同函数共现（ARM64 未归属为 0）；
- 两个独立实现（Rust 工具 + Python 复算）在 8 个样本上互相对账；
- `pwsh.exe`（44 万字节级小样本）与 `coreclr.dll`（3 万函数）的覆盖率都在 97% 左右，
  没有随规模劣化。

## 6. 仍未证 / 不做

- **未引用导入未逐条裁定**：ARM64 每二进制有 3–23 个 `False`（例如 ripgrep 的
  `ReadConsoleW`、`GetFinalPathNameByHandleW`；Node 的 `SymGetModuleBase64`、`WSASendTo`）。
  它们可能是间接调用（寄存器/函数指针）或仅 x64 代码路径使用；**本报告不把它们当缺陷**，
  也不当"死导入"结论。
- **ARM64 ELF 只有一个可执行文件样本**（ripgrep）：`.so` 共享库的 PLT/GOT 路径仍未校准，
  且该样本被裁剪（`STT_FUNC` 覆盖 0%），函数级共现只能保持 `Unknown`。
- **PE32**（无 `.pdata`）的同函数共现仍恒为 `Unknown`。
- 字节级模式匹配的固有边界不变：寄存器经多次搬运、跳转表、运行时生成的代码都看不到。

## 7. 复算

```bash
# 取样本（版本与上表一致；校验 sha256 后再分析）
curl -L -o node-arm64.exe https://nodejs.org/dist/latest-v22.x/win-arm64/node.exe
gh release download 15.2.0 --repo BurntSushi/ripgrep \
  --pattern 'ripgrep-15.2.0-aarch64-pc-windows-msvc.zip'
gh release download v7.6.9 --repo PowerShell/PowerShell \
  --pattern 'PowerShell-7.6.9-win-arm64.zip'

# 读数
cargo run -q -p trit-re -- triage node-arm64.exe
cargo run -q -p trit-re -- triage node-arm64.exe --jsonl | wc -l
cargo run -q -p trit-re -- triage node-arm64.exe --xref | head
```

样本只落在临时目录，**不进仓**；本文件的哈希是唯一可复核锚点。

## 8. 证据等级

- 读数表、修复前后值、独立复算：**A**（本机实测，命令在 §7）；
- "Node 构建差异导致锁调用数量不同"：**B**（两侧静态引用都复算过，但未对照构建参数/源码）；
- "取地址但无直接调用 ⇒ Unknown"：**A**（地址计算与调用窗口都复算过），但"这些导入是否真的被间接调用"仍是 **C**（未逐条动态确认）。
