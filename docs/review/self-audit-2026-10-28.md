# 工具链自审（2026-10-28）：用自己的扫描器挖自己

> 动机：挖漏洞的工具首先要经得起挖自己。这轮把 trit 的四个安全面全部对准
> trit 仓库本身（含 #169 合并的全部新代码），查到的自己的问题照修。

## 扫描面与结果

### 1. `trit-unsafe` 自扫（源码面）

- 扫 280 个 .rs 文件、57 个 unsafe 站点（trit-ops 39 / benchmarks 13 / trit-dot 4 / trit-train 1）。
- **发现 1（已修）**：`trit-dot` 的 2 个 `#[target_feature] unsafe fn`（popcnt 路径）
  有安全契约但没按惯例带 `# Safety` 标记 ⇒ 被自家扫描器计为未文档化签名。
  补上标记后重扫：`undocumented = 0`、`risky = 0`。
- 结论：自身 unsafe 面全绿（0 Risky / 6 Safe / 51 Unknown，Unknown 均为承诺面）。

### 2. `trit-redteam binary` 自检（产物面）——**挖到自己的真问题**

对自家四个扫描器 PE 做硬化/泄密检查：

- trit-unsafe / trit-build / trit-procscan：干净（只有 rust-toolchain 归属的
  std panic 路径，发布构建 `--remap-path-prefix` 可去）。
- **trit-redteam 自己：4 个 project 归属发现**——这就是「检测器嵌了自己的
  检测样本」的自指问题，逐一定位：
  1. `secret-aws-access-key` / `secret-openai-style-key` / `secret-private-key`：
     泄密 needle 表（`AKIA` / `sk-` / `-----BEGIN `）是普通字符串常量，
     编进产物后扫自己必然命中自己；
  2. `secret-private-key` 在 needle 运行时化后仍命中一次：`plausible_secret`
     里的比较字面量 `-----BEGIN ` 自己又是连续样本（自指第二层）；
  3. `build-tree-path`：`path_leaks` 里硬编码了 **`D:\KF\` 魔法路径**——
     维护者本机的检出目录被写进检测逻辑，在任何别的机器上都会漏判/错判。

**修复（全部落在 binary.rs）**：

- needle 表改**运行时拼装**；第一版直接分段仍被链接器的字符串相邻布局
  拼回明文（实测又命中一次），升级为**逆序片段**（`rev()` 还原），
  相邻布局也只能拼出 `IAAK` 这类乱序；
- `plausible_secret` 改按 **name** 分支，needle 文本不再出现在比较里；
- 移除 `D:\KF\` 魔法路径与其 `build-tree-path` finding id；抽取器加
  **通用盘符前缀**（26 个 `X:\`，运行时拼装）+ **≥3 路径段过滤**
  （把「检测器自己的短 needle 常量」滤在门外）；
- 测试 fixture 换成通用路径（`C:\dev\trit\...`），期望 id 改
  `absolute-user-path`（origin 仍为 project）。

**重扫验证**：trit-redteam.exe 自检 → `project: 0`、`secrets: {}`，
只剩工具链归属的路径项。自指闭环。

### 3. `trit-build → trit-redteam` 动态链路（吃自己产出的网表）

`trit-build --emit-verilog`（`(a & b) | !c`，27 组合穷举校验等价）→
`trit-redteam rail / sidechannel` 打自家导出的 `design.v`：

- **rail**：37 组非法输入里 15 组输出 invalid、14 组会被「只看 pos」的消费端
  读成 True（fail-open）。attribution=project，hint 已给出修复方向
  （导出与仿真里加非法态断言）——**记录为已知的导出边界**，修法归入
  Verilog 导出增强，不在本 PR。
- **sidechannel**：双导轨一阶重量模型泄漏 0.975 bit（Unknown 重量为 0，
  天然藏不住）——已知负结果，hint 指向恒定重量编码。

### 4. `trit-procscan` 自观察（运行时面）

`procscan run -- trit-unsafe scan --root .`：1 进程、219ms CPU、
0 模块加载、**0 网络连接**（扫描器不外联）、0 写盘；事件落 JSONL 后
`trace --pid` 追溯成功（ppid=0 = 无存活父进程，按正常链尾处理）。

### 5. Mimosa 深度重扫

覆盖 #169 合并后全部代码的 fresh deep 扫描，结果与处置见
[mimosa-deep-scan-2026-10-28.md](mimosa-deep-scan-2026-10-28.md)。

## 修复清单（本 PR）

| 文件 | 修复 |
|---|---|
| `crates/trit-dot/src/lib.rs` | 2 个 unsafe fn 补 `# Safety` 标记 |
| `crates/trit-redteam/src/binary.rs` | needle 运行时拼装（逆序片段）、移除 `D:\KF\` 魔法路径、通用盘符抽取 + 段数过滤、测试与模块文档同步 |

## 未修、如实记录的边界

1. **导出 Verilog 的 fail-open**（rail 发现）：消费端契约问题，修法在
   导出增强里，另立任务；
2. **双导轨重量侧信道**：编码固有属性，缓解要换恒定重量编码，已记录 hint；
3. **cfg-missing**：GNU 工具链不支持 /GUARD:CF（redteam 自身的 warning 归因
   也验证了这一点——MSVC 构建才报 project 可修），换工具链不属于代码修复；
4. rust-toolchain 归属的 std 路径泄漏：发布构建加
   `RUSTFLAGS=--remap-path-prefix=<HOME>=~` 即可（GitHub Releases 构建建议采纳）。
