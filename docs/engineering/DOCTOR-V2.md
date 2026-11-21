# DOCTOR-V2：体检开销治理、多值编码评估与编排待办

> 定位：给在本仓跑 `tools/doctor.py`、维护护栏工具链的人读。它回答三件事：doctor 的时间花在哪、
> 缓存怎么修（读数见 §1）、Rust / 三进制 / 四进制与编排下一步该怎么走（未实测，只给判据）。
> 边界：只覆盖 doctor 的 gates / simchain 面与 journal 的 L124 修复；不覆盖 cargo / 测试 / CI 的墙钟；
> 不承诺跨机缓存、也不承诺防篡改。Rust 与多值编码**没有替换任何东西**，本文不做"更快"的声称。

## 1. 读数：1.26 s → 0.54 s（同机同命令）

| 档 | 命令 | 次数 | 中位 | 范围 |
|---|---|---|---|---|
| 门重验 | `python -I tools/doctor.py --refresh` | 3 | **1.257 s** | 1.230–1.257（首跑冷盘 2.155） |
| 命中缓存 | `python -I tools/doctor.py` | 4 | **0.536 s** | 0.516–0.560 |

比值 **2.3×**（1.257 / 0.536）。分量（同机单跑）：`scripts/privacy_scan.py` 0.675 s、
`scripts/docs_hygiene.py` 0.068 s、解释器启动 `python -I -c pass` 0.024 s。两个门合计 0.743 s，
占强制重验的 **59%**。首轮口径（privacy_scan 0.91 s / 总计 1.55 s）是另一天、另一进程负载下的读数，
不与本表混算——本仓规矩：不同机的数字分开列（`docs/specs/DOCUMENTATION.md` §2 规则 10）。

2026-11-22 复测（同机同命令，加入 simchain 第七面之后）：强制重验中位 **1.312 s**
（n=3，1.303–1.336）、命中中位 **0.538 s**（n=4，0.533–0.553），比值 2.4×。
与首测的差异在门脚本负载波动内，两档读数分开列、不合并平均。simchain 面只读
83.8 KB 报告，单跑中位 **0.34 ms**（n=7），不是开销来源。

缓存命中时 gates 面的 detail 带 `[cached fp=<前 8 位> --refresh to re-verify]`，可用
`python -I tools/doctor.py --json` 直接读；表格视图不打印 OK 面的 detail，只看表格会误判成"没命中"。

## 2. 机制：stat 指纹当验证轨迹

- 指纹（`tools/doctor.py:297`）：对 `-Root` 下每个文件取 `(相对路径, mtime_ns, size)`，排序后
  SHA-256，取前 16 位十六进制。目录跳过集与 `tools/journal.py` 同纪律（`.git`、`target`、
  `__pycache__` 等本地产物不算门输入——否则门自己写 `__pycache__` 就会把自己的缓存作废）。
- 缓存（`tools/doctor.py:288`、`:346`）：`.agent/doctor-cache.json` 只存 gates 面
  `{status, evidence, detail}` + 指纹；指纹一致才复用，`--refresh`（`:453`）强制重跑并重写。
  落盘前过 `tools/doctor.py:331` 的 `cache_allowed`：树能忽略这个文件（`git check-ignore`）才写，
  没有 `.gitignore` 的仓库不写——体检工具不许把体检对象弄脏。
- simchain 面（`tools/doctor.py:390`）：只读 `.agent/last-simchain.json`，`violations > 0` 判 FAIL；
  文件缺失 / 坏 JSON / 自相矛盾（顶层计数与 `scenario_details` 的行数或违规数不一致）判
  UNAVAILABLE 并给重跑命令。不判报告新鲜度，只把 `at` 打进 evidence/detail——与 selftest 面
  同一纪律；doctor 不跑模拟器（200 场景是分钟级，不是秒级）。
- 语义：**只缓存 gates 面**。其余六面（selftest / ledger / registry / backlog / git / simchain）
  每次现算，所以"缓存命中"不会让台账、分支、自检记录、模拟报告的新鲜度失真。
- 这就是 build system 文献里的 *verifying trace*：存依赖的哈希而不是结果，验证通过就跳过任务；
  dirty-bit 方案做不到 early cutoff，验证轨迹可以（Mokhov, Mitchell & Peyton Jones,
  *Build Systems à la Carte*, ICFP 2018, DOI `10.1145/3236774`）。

## 3. 边界：它是节省，不是安全边界

- `mtime_ns + size` 理论上会漏掉"同尺寸、同时间戳"的改写——Git 的 index 也接受同一取舍，
  并把这种条目叫 racy（`git-scm.com/docs/racy-git`）。逃生舱是 `--refresh`；要更强的保证就每次重验。
- 缓存文件与被检查的树在同一信任域：能写 `.agent/` 的人也能改门脚本。它防的是**重复开销**，
  不防篡改。
- 跨机 / 跨 worktree 不共享缓存；一次 `git checkout` 改变 mtime 就会整体重验（正确性优先）。
- 缓存文件已进 `.gitignore`（与 `.agent/last-selftest.json`、`.agent/last-simchain.json` 同纪律），
  doctor 不会把自己的产物弄成 dirty。

## 4. Rust / 三进制 / 四进制：先算账，再谈替换

**Rust**。剩余 0.538 s 的大头不是解释器启动（启动只占 0.024 s），而是采集本身：
同机单跑中位 `git` 面 **252 ms**（n=5，git 子进程是主体）、`selftest` 面 **0.16 ms**（n=5）、
树指纹 **113 ms**（n=3）、ledger 面 **104 ms**（n=7；其中 `parse_ledger` 只有 0.28 ms，
开销在 300.8 KB selftest 源码的断言号扫描）、registry 面 **32 ms**（n=7）、
simchain 面 **0.34 ms**（n=7）。所以 Rust 化的收益上限在"0.5 s 级"，不在"启动开销"。Pereira et al. 的跨语言基准里 Rust 的
时间/能耗约为 C 的 1.04×/1.03×，Python 为 71.9×/75.9×（*Energy efficiency across programming
languages*, SLE 2017, DOI `10.1145/3136014.3136031`；2021 年复验见 *Science of Computer
Programming* 205:102609）。这是计算密集基准的口径，不是启动、不是 I/O——只能当动机，不能当承诺。

- 提议（**未落地**，遵守"先不替换"）：起一个 `trit-doctor` 试点 crate，复用本仓 `trit-core`
  的表示与序列化，输出与现在**同一份** `--json` schema；同机 A/B，验收线是"暖态中位 < 0.15 s
  且除 volatile 字段外逐字节同 JSON"。达不到就把负结果写进 `docs/results/LESSONS.md` 并关线，
  不做"部分替换"的夹生状态。

**三进制**。平衡三进制的数位密度优势是信息论的：同样范围只需 `log3 2 ≈ 63%` 的位数
（Knuth 的结论，Setun 实践见 Brousentsov et al., *Development of ternary computers at Moscow
State University*）。落到本工具：64 位指纹 = 41 个 trit，按 5 trit/字节（3⁵=243）打包是 9 字节，
现在写成 16 个十六进制字符。省 7 字节/次，收益是噪声；而 SHA-256 与文件系统都是二进制域，
转换是新开销。归类按 `docs/positioning/TERNARY-BOUNDARIES.md`：哈希与 I/O 属 **[D] 基数无关**，
强行三值化是 **[B] 模拟税**。

**四进制**。2 bit/位，与二进制天然兼容；多值逻辑的硬件红利（连线数、面积、功耗）出现在
器件层（Chaudhuri, *Beyond Bits: A Quaternary FPGA Architecture Using Multi-Vt Multi-Vdd
FDSOI Devices*, ISMVL 2018, DOI `10.1109/ISMVL.2018.00015`；MTJ + CNTFET 的非易失版本见
*IEEE Access* 13:46810, 2025, DOI `10.1109/ACCESS.2025.3550778`），x86 软件里"四进制"就是
2-bit 打包，等于 base4 文本编码，对 SHA-256 没有加速。归类 **[C] 生态路径依赖 + [D]**。

**结论**：只保留 Rust 试点这一条候选；三进制 / 四进制**不采纳**到 doctor（不是"已否证"——
没有做过端到端实验，见 §7）。

## 5. 编排待办（1 项已接，4 项未接）

1. **doctor 的 simchain 面已接（2026-11-22）**。`.agent/last-simchain.json` 由 50 种子 × 4 故障族
   的模拟跑落盘；`faces` 清单现在有第七面，实现是 `face_simchain`
   （`tools/doctor.py:390`）：`violations > 0` 判 FAIL，文件缺失 / 坏 JSON / 自相矛盾判
   UNAVAILABLE 并给重跑命令，`at` 只打进 detail、不判新鲜度——照 selftest 面同一纪律。
   selftest AI30–AI35 六条钉子（AI1b 同步从六面改成七面）。未做：报告过期判据。
2. **缓存只覆盖 gates：量过，决定不扩**。原假设"88 KB 台账解析贵"被证伪：`parse_ledger`
   只有 **0.28 ms**；真正的开销是 face_ledger 把 300.8 KB selftest 源码逐断言号扫描
   （**104 ms**），registry 的存在性探测再占 **32 ms**，两面合计 136 ms，占命中档 0.538 s 的
   25%。但 registry 面的语义是"当前机器能不能跑"（PATH 上有没有命令、scope 文件在不在），
   缓存会在装上缺失命令 / 建好 scope 文件后继续报旧 WARN——这是会撒谎的缓存，不接。
   ledger 面单独缓存只省约 104 ms（19%），却要新增一套输入指纹与失效面，收益不抵复杂度。
   结论：缓存保持只覆盖 gates；ledger 面若将来变重，先重新量再议。
3. **journal 的跨平台路径语义未测**。`safe_rel`（`tools/journal.py:94`）在 Windows 上由
   G5/G8/AM25 钉住；POSIX 上反斜杠是合法文件名字符，两种语义没有实机读数。
4. **ECC 帧在编排默认路径仍是关闭**。`crates/trit-orch/src/trace_store.rs:37` 的 `ecc: None`
   是默认构造；开启要显式走 opt-in 构造函数（同文件 `crates/trit-orch/src/trace_store.rs:43`）。
   默认开不开、开了对既有账本字节的影响，没有决策记录。
5. **断粮仍要人接**。`tools/resume.py` 只负责挂起/续跑账本，跨会话把活捡起来仍靠人读账；
   这不是缺陷，是协议三的边界，但没有"自动接续"的承诺。

## 6. 复现

```bash
# 读数（同一台机器，先跑 --refresh 再跑命中档）
python -I tools/doctor.py --refresh
python -I tools/doctor.py
python -I tools/doctor.py --json        # gates.detail 看 [cached fp=...]，simchain 面看 violations

# 分量
python -I scripts/privacy_scan.py
python -I scripts/docs_hygiene.py

# L124 的三条钉子（fixture 内自建，不碰真仓）
pwsh tools/selftest.ps1                 # AM25 / AM25b / AM26 + AI30–AI35
```

## 7. 未证栏

- Rust 试点的墙钟、内存、二进制体积：**没有读数**；验收线写在 §4，未开工。
- 三进制 / 四进制版本的端到端对照：**没有实验**；本文只给信息论与归类，不给倍数。
- 缓存的真实命中率（连续开发会话里 mtime 抖动多大）：**没有统计**；本机只测了稳态与强制重验两档。
- racy 漏检（同尺寸同 mtime 改写）：**没有构造攻击样例**；只有 Git 的同类边界作参照。
- POSIX 上 journal 的 `safe_rel` 行为：**没有实机验证**。
- simchain 报告的新鲜度：**没有过期判据**；doctor 只把 `at` 打进 detail，多久没跑算过期、
  要不要判 WARN 未定义。
- simchain 面的违规分支只在 fixture 里验过（AI32/AI33）；真仓最近一份报告是 0 违规，
  没有“真实违规报告进 doctor”的实机读数。
