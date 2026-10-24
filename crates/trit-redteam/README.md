# trit-redteam

三进制产物（TRB2 blob、双导轨 `design.v`）的**红队/加固检查器**。

## 边界

- 只读**本仓自己的产物**：`trit-build` 生成的 `truth.tritbin` 与 `--emit-verilog` 的 `design.v`。
- 不发网络请求、不做目标发现、不做规避检测；没有"打某个外部系统"的入口。
- 目的是把"我们的格式有哪些可被利用的性质"变成**可复算的判据**，用来加固自己的格式与消费端。

## 三个检查

| 子命令 | 打的是什么 | 实测（`a & b` 的真实产物，2026-10-25） |
|---|---|---|
| `encode <blob>` | TRB2 末字节的闲置 trit 槽 ⇒ 同一张表多个字节串、可夹带数据 | 14 字节 blob 有 **3 个闲置 trit ⇒ 27 种写法**、隐写容量 **4.75 bit**；**加固后严格解码器只接受 1 种**（`accepted_by_strict_decoder=1`），其余 26 种被拒 |
| `crc --trits 9` | CRC-3 的检错边界（GF(3) 线性码，用全零载荷即可枚举全部错误模式） | 重量 1/2 **100%** 检出；**重量 3 有漏检**（2184 个模式里 30 个漏检，例子：位置 `[0,1,5]` 各 +1）；最小不可检重量 **3** |
| `rail <design.v>` | 双导轨 `(1,1)` 非法态的传播：fail-open / 粘滞 | 27 个合法输入 → 输出非法态 **0**；37 个非法输入里 **14 个输出成了有效 True**（只看 `pos` 的消费端会读成 True）；全 `(1,1)` 输入锁死在非法态 |
| `fuzz <blob>` | TRB2 **差分** fuzz：Rust 解析器 vs C 运行时（`trit-build/runtime/trit_runtime.c`） | 5 个种子 × 5000 个确定性变异 = **25,000 例，0 分歧**。第一轮就抓到本工具自己的解析器**比生产工具宽松**（漏了 `len == 3^nvars` 校验），已按契约收紧 |
| `chain <jobs.json>` | `--manifest` 的执行面静态清点（`kind: binary` 会被直接执行，无 allowlist/沙箱） | 标出 shell 包装、网络工具、绝对路径、`cwd` 越界；ternary-only 清单无发现 |
| `sidechannel <design.v>` | 双导轨的一阶静态模型：`True/False` 各拉高 1 根线、`Unknown` 0 根 | 真实导出 27 个合法输入：`H(输出)=1.506 bit`、`H(输出｜重量)=0.531 bit` ⇒ **泄漏 0.975 bit**；重量 0 的 11 行**全是 Unknown**（重量 1 的 16 行里 true/false 各 11/5，所以重量只能判"是否未知"，判不出取值） |
| `binary <file>` | PE/ELF 硬化（ASLR/NX/执行栈/W+X/RELRO/BIND_NOW）+ 密钥形态 + 构建路径泄漏 | 自家 `trit-build.exe`（PE64）：ASLR ✓、NX ✓、无 W+X、CFG ✗；**嵌了 `C:\Users\<用户名>\.rustup\...`**（std 的 panic 路径）；修法：`--remap-path-prefix` |

```bash
trit-build e.tri --out out --emit-verilog
trit-redteam encode out/truth.tritbin
trit-redteam crc --trits 9 --max-weight 4
trit-redteam rail out/design.v
trit-redteam fuzz out/truth.tritbin --iterations 5000 --seed 1
trit-redteam chain jobs.json
trit-redteam sidechannel out/design.v
trit-redteam binary target/release/trit-build.exe
# 每个子命令都支持 --strict：有发现就退出 1，可直接当 CI 门
```

## 归属：是我们的问题，还是 Rust 工具链本身的问题

每条发现都带 `origin`（`project` / `rust-toolchain`）、`area` 与 `hint`，报告顶层给计数。

| 判据（证据） | 归属 |
|---|---|
| 硬化位缺失：ASLR / NX / 可执行栈 / W+X / RELRO / BIND_NOW | `project` — 链接参数能改 |
| PE 上 CFG 缺失且是 MinGW 运行时（无 `VCRUNTIME140`） | `rust-toolchain` — binutils 版 GNU ld 不支持 `/GUARD:CF` |
| PE 上 CFG 缺失且是 MSVC 运行时 | `project` — 加 `/GUARD:CF` 即可 |
| 产物里有 `.rustup` / `/rustc/` / `library/core` 路径 | `rust-toolchain` — std 的 panic 路径，可用 `--remap-path-prefix` 抹掉 |
| 产物里有构建树 / 绝对路径（非 std 来源） | `project` — 产物内容 |
| 密钥/令牌形态字符串 | `project` — 产物内容 |
| TRB2 可塑 / CRC-3 检出边界 / 双导轨 fail-open / 侧信道 / manifest 执行面 | `project` — 格式与工具设计 |
| 差分 fuzz 出现分歧 | `project` — 两份实现里至少一边与格式契约不符 |

修复前（`target/release/trit-build.exe`，PE64）：`findings` 只有 1 条 `rust-toolchain-path`
（`C:\Users\<用户名>\.rustup\...\library\core\...`），CFG 缺失在 `warnings` 里记为
`rust-toolchain` ⇒ **没有属于我们代码的硬化/泄密发现**。

**已修**：Release 工作流与本地发布构建都加 `RUSTFLAGS=--remap-path-prefix=$HOME=~`
（`.github/workflows/release.yml` + README「开发」）。修完后同一个产物：
`findings` **空**，只剩 `warnings: [cfg-missing(rust-toolchain), toolchain-path-only]`
——路径降级成 `~/.rustup/...`，不再含用户名。

**限制**：归属是启发式（字符串证据 + 头字段），它回答"这段字节来自哪里"，不回答"风险该谁负责"。

## 为什么这些是"三进制独有"的

- **5 trit/字节** 不是 2 的幂：每个字节都可能剩 1–4 个 trit 槽，闲置槽既不改语义也不进 CRC。
- **第三个真值** 让 `(pos, neg)` 有两根线而合法组合只有三种，`(1,1)` 是第四种状态；
  门级网表会传播它，而"只看 pos"的消费端会把它读成 True（fail-open）。
- **GF(3) CRC** 的检错能力由最小距离决定，与二进制 CRC 的直觉不同：冗余 3 个符号时
  Singleton 上界 `d ≤ 4`，实测 `d = 3`，所以"3 个 trit 的错误"并不保证检出。

## 未做

- 双导轨分析只覆盖 `trit-build --emit-verilog` 生成的 `~ ! & |` 子集；`^`（强加法）与蕴含
  目前不会出现在导出里，未分析。
- `fuzz` 只做单字节/短结构变异，没有覆盖感知（coverage-guided）反馈；语料是确定性的。
- `sidechannel` 是**静态一阶模型**：不含噪声、不含动态功耗/时序，不能推出"攻击成功率"。
- `binary` 只看头字段与字符串，不做反汇编、不找漏洞；ELF 只认小端 32/64，PE 只认 32/64。
- `chain` 只做静态清点，不执行、不沙箱；它回答"会跑什么"，不回答"跑了会怎样"。
