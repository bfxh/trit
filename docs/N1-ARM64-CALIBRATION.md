# N1 校准：真实 ARM64 PE 上的交叉引用

> 目的：用**真实** Windows ARM64 二进制验证 `trit-re` 的 ARM64 交叉引用与函数边界，
> 并把发现的问题修到代码里。此前 ARM64 只有合成夹具，属于"未证"。
>
> 结论：在 5 个真实 ARM64 PE 与 2 个真实 ELF（ripgrep 可执行文件、glibc 2.44 共享库）上，
> 导入引用覆盖率 **91%–100%**，
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
| `aarch64-libc.so.6` | Debian glibc 2.44-3，`libc6_2.44-3_arm64.deb` | 1,788,312 | `3d9fb822f7126eab39dfe0c0a5abe79043d02086c88cb5f2a63eabe09e95b2be` |
| `x86_64-libc.so.6` | 同上 `amd64` 包 | 2,046,960 | `6e8481bffbeca764ca114dfd130c67ff1fc745a3abc624bc7c86025f9ca08813` |

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
| ripgrep 15.2.0 | **ELF arm64** | 138 | 133（96%） | 0 | 5 | 3,480（`.eh_frame`） | 5,856 | 1,496 | 0.2 s |
| glibc 2.44 | **ELF arm64** | 13 | 13（100%） | 0 | 0 | 3,500（`.eh_frame`） | 40 | 0 | <0.1 s |
| glibc 2.44 | ELF x86-64 | 13 | 13（100%） | 0 | 0 | 3,858（`.eh_frame`） | 40 | 0 | <0.1 s |

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
| 8 | 共享库把自己导出的函数（`malloc`/`free`）的 JUMP_SLOT 也当"导入" | 只保留未定义 + FUNC/NOTYPE/IFUNC | glibc ARM64 行数 18→13、x64 15→13，与导入数一致；两架构读数对称（各 13 导入 / 40 调用点） |
| 9 | 裁剪 ELF 没有可用函数边界（3 个符号 → 启发式） | 解析 `.eh_frame` 的 CIE/FDE，来源标 `eh_frame` | ripgrep ELF：19,709 启发式 → 3,480 个 FDE 函数；glibc arm64/x64：3,500/3,858 |

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
- **ELF 只有两个样本**（ripgrep 可执行文件、glibc 共享库）。函数边界现在来自 `.eh_frame`，
  但 `panic=abort` 构建（如 ripgrep）缺 FDE 的函数仍落在未归属里（1,496 个调用点），
  相关能力组保持 `Unknown`；IFUNC 密集的库（如 libm）未覆盖。
- **ELF 槽位表收 `JUMP_SLOT` + `GLOB_DAT`**，但只对 NOTYPE/FUNC/IFUNC 符号建行；
  纯数据导入（`OBJECT`）不进台账，因此"没看到引用"不会落到它们头上。
  字节比较类取址尚未计入（见 §10 的未证项）。
- **PE32**（无 `.pdata`）的同函数共现仍恒为 `Unknown`。
- x64 的未归属调用点已按类别拆开：Node 148 = call 25 / jmp 123，Deno 156 = call 24 / jmp 132。
  其中 jmp 绝大多数是无 `.pdata` 的导入跳板（结构性）；**call 的 24–25 个尚未逐条裁定**，
  已列为下一步（见 `docs/SECURITY-RE.md` 的 N2 前置）。
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

## 9. 真实 `.debug_frame` 校准（2026-10-30）

发行版二进制（ripgrep、glibc）只带 `.eh_frame`，所以这条路径改用**现场编译**：

```bash
# WSL Ubuntu 24.04 + gcc 13.3.0
gcc -O0 -g -fno-asynchronous-unwind-tables -o c-df dbg.c
# CRT 仍会写入 .eh_frame；只移除它，保留真实的 .debug_frame
objcopy --remove-section=.eh_frame --remove-section=.eh_frame_hdr c-df c-df-debugframe
```

| 样本 | 大小 | sha256 | 内容 |
|---|---|---|---|
| `c-df` | 19,408 | `66aa4a46fe5f42288e9eb05ae46f39a6bae37e3b3ec654dffab7b079158d3e2c` | `.eh_frame`（CRT 4 FDE）+ `.debug_frame`（用户 3 FDE） |
| `c-df-debugframe` | 19,176 | `32ae2f988585d29f63bb07a470b53baafed0c70d0d18665c4f03a5abcd5c218f` | 只剩 `.debug_frame` |
| `c32-df-only` | 17,980 | `ed0f43d59304fd222ec189ac5ad50fbf11878e9f685e3df81fcf679892001532` | ELF32（`gcc -m32`），CIE v1，`address_size` 按 ELF class 推断为 4 |
| `c-arm64-df-only` | 73,560 | `07b3362a956395a88d8c026cea85765a0cfa0a141d8ddef7d89422e5de436940` | AArch64（`aarch64-linux-gnu-gcc`），CIE v1 |
| `c-clang-df-only` | 19,328 | `76dfa0f2ed20c8708afa09c54b3a8e202e3bef77bb4cd6e913c6fa96bd551ccb` | x86-64 clang 21，CIE **v4**（`Pointer Size: 8 / Segment Size: 0` 紧跟 augmentation） |

独立对账（`readelf --debug-dump=frames`）与 trit-re 的函数区间**逐条相同**：

```text
c-df/c-df-debugframe  (x86-64, gcc)  0x11a9..0x11fa  0x11fa..0x1233  0x1233..0x12cc
c32-df-only           (ELF32, gcc)   0x11bd..0x120f  0x120f..0x1240  0x1240..0x12e9
                                     0x12e9..0x12ed  0x10c0..0x10c4
c-arm64-df-only       (AArch64)      0x858..0x8c0    0x8c0..0x8f8    0x8f8..0x998
c-clang-df-only       (x86-64, clang) 0x1170..0x1216  0x1220..0x1281  0x1290..0x12c2
```

工具读数（`trit-re triage --functions`）：

| 样本 | 函数数 | 边界 | 归属调用点 / 调用点总数 | 未归属 |
|---|---|---|---|---|
| `c-df-debugframe` | 3 | exact | 4 / 4 | 0 |
| `c32-df-only` | 5 | exact | 4 / 6 | 2（都在 CRT，该样本只保留用户函数 FDE） |
| `c-arm64-df-only` | 3 | exact | 4 / 8 | 4（全在 PLT/CRT 与库函数间隙） |
| `c-clang-df-only` | 3 | exact | 4 / 4 | 0 |

`c32-df-only` 的交叉引用是 `complete=true`：9 个导入里 6 个有调用点、2 个确证无引用、
1 个 `Unknown`（`__gmon_start__` 被 `mov 0x24(%ebx)` 取过地址）。函数归属 4 / 6，
2 个未归属都在 CRT（该样本只保留用户函数的 FDE）。见 §10。

**发现的缺陷**：原先只要 `.eh_frame` 非空就选它。`c-df` 的 `.eh_frame` 只覆盖 CRT，
4 个用户调用点全部落在外面（未归属 4）；改为按**调用点覆盖率**选择后改选 `.debug_frame`，
4 个调用点全部归属、未归属 0。`c-df-debugframe` 与 `c-df` 现在读数一致。

**v4 字段顺序**：clang 的 `.debug_frame` 把 `address_size`/`segment_selector_size` 放在
**augmentation 之后、`code_alignment_factor` 之前**。原先按"返回地址寄存器之后"读，
clang 样本上 3 个 FDE 全部解析失败；改正后与 readelf 逐条一致。单元测试
`address_size_eight_reads_full_width_addresses` 用 `0x1_0000_1100` 这种超出 32 位的地址，
把"写死 4 字节"的变异打红（此前用 0x1100 的夹具打不红——值落在 32 位内，读 4 字节也对）。

### 9.1 augmentation（`zR` 等）

CIE 的 augmentation 现在按 DWARF 规范解释，不再"遇到就跳过整条"：

- `z` 后跟 uleb 长度，按长度裁剪后续字段；
- `R` 读 1 字节指针编码，`L` 跳过 1 字节，`P` 读编码字节 + 按编码跳过 personality 指针，
  `S`/`B`/`G` 无数据；其余字母**返回失败**而不是猜；
- 指针编码支持格式 `absptr`/`uleb`/`sleb`/`udata2,4,8`/`sdata2,4,8`，应用位支持绝对（`0x00`）
  与 `pcrel`（`0x10`）；`indirect`/`datarel`/`textrel` 一律失败；
- `parse_fde` 的初址用完整编码（含 `pcrel`，基准是节地址 + 字段偏移），范围字段只取低 4 位格式
  （按无符号读）。

**未证（诚实边界）**：本机四个真实 `.debug_frame` 样本的 augmentation 全是空串
（`readelf` 的 `Augmentation: ""`），真实 `zR` 只在 `.eh_frame` 里见到；
所以 `zR`/`pcrel`/`P` 这条路径只有**合成夹具**覆盖，尚未在真实 `.debug_frame` 上校准。


## 10. i386（ELF32）交叉引用校准（2026-10-30）

32 位 ELF 与 64 位路径有三处结构差异，都是实测确认的：

- 重定位是 `SHT_REL`（8 字节/条）：`r_offset` 4 字节，`r_info` 是「符号:类型 = 24:8」；
- PIE（Ubuntu 的 `gcc -m32` 默认产物）的 PLT 桩是 `FF A3 disp32`（`jmp *disp(%ebx)`），
  槽位 = GOT 基址 + disp；基址来自 `call __x86.get_pc_thunk.bx; add $imm,%ebx`，
  两条都要按字节确认（thunk 体必须是 `8B 1C 24 C3`），否则不采信；
- 非 PIE 的桩是 `FF 25 abs32`，地址就是槽位。

```bash
# WSL Ubuntu 24.04 + gcc 13.3（已装 gcc-multilib）
gcc -m32 -O0 -o c32-pie dbg.c
gcc -m32 -O0 -no-pie -fno-pic -o c32-nopie dbg.c
gcc -m32 -O1 -shared -fPIC -o libdbg32.so dbg.c
```

| 样本 | 大小 | sha256 | 结构 | 工具读数 |
|---|---|---|---|---|
| `c32-pie` | 15,092 | `87b986d4…` | ET_DYN（PIE），`FF A3` 桩 + thunk | `complete=true`，5 个 JUMP_SLOT 全有调用点 |
| `c32-nopie` | 14,972 | `5a4a5077…` | ET_EXEC，`FF 25 abs32` 桩 | `complete=true`，5 个全有调用点 |
| `libdbg32.so` | 14,688 | `2d842a13…` | ET_DYN 共享库 | `complete=true`，4 个 JUMP_SLOT 全有调用点 |

逐条对账（`objdump -d` 的调用地址 vs 工具给的 `sites`）：

```text
c32-pie      __libc_start_main 0x10b2   fclose 0x12cd   strlen 0x11fa
             fprintf 0x1232            fopen 0x1297
c32-nopie    __libc_start_main 0x80490a3  fclose 0x8049272  strlen 0x80491c7
             fprintf 0x80491ed            fopen 0x804923e
libdbg32.so  fclose 0x120f   strlen 0x11b1   fopen 0x11e3   __fprintf_chk 0x1206
```

三个样本**逐条相同**（同一行的地址集合完全一致），没有多报也没有漏报。

**语义**：`(%ebx)` 引用只在"基址 + disp 恰好等于已知 JUMP_SLOT"时才算数；
`.plt` 里的 `FF A3` 只登记为跳板、不算调用点（调用点归到 `E8` 的调用方）。
`mov`/`lea`/`push` 只取地址 ⇒ 该导入降级 `Unknown`（夹具里 `printf` 就是这条路）。

**未证 / 不做**：

- 字节比较类取址（`80/81/83 BB disp32`、`B8 imm32` 搬常量等）不计入"取地址"：
  若某个导入只被这类指令碰过，仍可能落 `False`。目前覆盖的是
  `mov`/`lea`/`cmp`/`test`/`push` 的 `[ebx+disp32]` 与 `[abs32]` 形式（有单元测试逐个锁）。
- 基址解析只认 `get_pc_thunk.bx` + `add $imm,%ebx` 这一种序言；`pop %ebx` 变体、
  `-mcmodel` 其它写法未覆盖（遇到时 `.plt` 有 `FF A3` 但无基址 ⇒ `complete=false`）。
- 32 位 PE（无 `.pdata`）与本主题无关，函数共现仍恒 `Unknown`。

### 10.1 `GLOB_DAT` 槽位（2026-10-30 补）

槽位表现在同时收 `JUMP_SLOT`（PLT 桩用）与 `GLOB_DAT`（直接取函数地址用；x86-64/i386 = 类型 6，
ARM64 = 1025）。此前只收前者，`__cxa_finalize` 这类"有 PLT 桩、也被调用"的导入会被记成
"没有槽位" ⇒ `Unknown`。

读数变化（同一批样本，改前 → 改后）：

| 样本 | 有调用点 | 无调用点 | 未知 |
|---|---|---|---|
| `c32-pie` | 5 → **6** | 0 → **2** | 4 → 1 |
| `libdbg32.so` | 4 → **5** | 0 → **2** | 4 → 1 |
| `c32-nopie` | 5 → 5 | 0 → 0 | 1 → 1 |
| x86-64 glibc | 13 → 13 | 0 → 0 | 0 → 0 |
| AArch64 glibc / ripgrep | 13 / 133 → 不变 | 0 / 0 | 0 / 5 |

`c32-pie` 里变成 `False` 的两个是 `_ITM_deregisterTMCloneTable` / `_ITM_registerTMCloneTable`：
槽位已知、可执行节扫全、函数体内确实没有对 `0x20(%ebx)` / `0x30(%ebx)` 的任何引用
（objdump 计数 0）⇒ "本文件不引用它"是有证据的结论，而不是缺省。

**注意**：这会让"数据型 GLOB_DAT 导入"从 `Unknown` 变成 `False`，前提是上面那组取址形式
已经覆盖；新增形式必须同时补单元测试，否则会把"漏检"伪装成"没有引用"。
