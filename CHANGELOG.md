# Changelog

## [Unreleased]

### Added（trit-re：ELF entrypoint 接线——语义实证判定后补齐）

- **语义实证**：官方 YARA 的 ELF `entrypoint` = **文件偏移**——yara-python
  判别协议（3 个 ET_EXEC 样本，e_entry vaddr ≠ 文件偏移）实测 vaddr 全不
  匹配、offset 全匹配，与 PE 同语义。不猜，跑出来的。
- **接线**：`ElfInfo::entry_file_offset()`（`e_entry` 过 PT_LOAD 映射，
  BSS/入口为 0/无程序头如实 `None`）+ CLI 的 PE-or-ELF 回退——ELF 样本上
  `$e at entrypoint` 从恒弃权变可评估。
- 测试 166 → 167（PT_LOAD 映射 + 端到端 at-entrypoint 三态）；门禁 fmt /
  clippy / test 全过。

### Added（trit-re：PE32 联合判定测试——方向库最后一条解锁条件收口）

- `tests/pe.rs::coff_and_cfg_jointly_attribute_pe32`：**三路来源同时贡献
  区间**的联合判定——COFF 符号精确区间 + CFG 补隙（符号未覆盖的可执行
  隙）+ 指针表种子（.rdata 连续 ≥2 的 u32 函数指针 → 间接入口）。断言：
  指针表种子 2 被收割、补隙区间照常归属调用点、精确区间不被切分、
  `exact` 不被 Cfg 区间拖回 Unknown（共现保持可判）。
- 方向库「PE32 共现 Unknown 收口」0 项的解锁条件（联合测试）就此满足；
  边界不变：真实 mingw PE32 样本上的 sha256 记录仍归 N1 批次。
- 测试 165 → 166；test_surface 台账同步；budget / surface 门绿。

### Added（trit-re：差分测试规模化——再抓两个语义分歧 + xor 组合校准）

- `scripts/yara_scale.py`：**属性驱动的差分测试生成器**（xorshift64 确定性
  种子，与 fuzz 同款）——核心子集语法 × 随机样本，复用 yara_diff.py 四态
  对拍；判据 DIFF=0。差分测试此前两次抓出真语义偏差（`at`/`@a`），这是
  它的规模化形态。
- **再抓两个真语义分歧并修复**（差分第 3/4 号成果）：
  ① `for-of` 迭代**整个串集**——官方语义：未命中串也在迭代里（`#`=0、
  `$`=假、`@`=弃权），`for all of ($a*) : (# > 2)` 的惯用法正依赖全集迭代；
  此前实现成"只迭代命中串"（实证：针缺席时官方 `for any of … : (# >= 0)`
  仍命中）。
  ② **确定的未定义 ≠ 证据缺失**：零命中串的 `@a[N]`、非截断输入的越界
  `uintN` 读是证据**完整**的确定事实——与官方一致，比较为假；截断输入的
  uintN / 缺上下文的 entrypoint 才是证据缺失 → 弃权。`resolve()` 升级三态
  `Val / DefiniteFalse / Unknown`。
- **xor 组合校准**：官方编译器拒绝 `xor nocase`（探针实测 "invalid modifier
  combination"）→ 解析层对齐拒绝；`xor wide`（展开后逐字节 XOR）经正例 +
  反例与官方逐位一致——最后一条"未与官方对拍"的已知边界清零。
- 规模曲线：10 批（9×40 规则×12 样本 + 1×60×16）**5280 判定对 MATCH 100% /
  DIFF 0 / SUBSET-GAP 0 / OFFICIAL-ERR 0**。
- 测试 163 → 165（全集迭代 / 确定未定义双态钉死）；行数预算再拆
  `sigscan/eval.rs`（三值求值内核，520+309 行）；test_surface 台账
  --update 登记；门禁 fmt / clippy / test / budget / surface 全过。

### Hardened（trit-re：fuzz 种子库扩到新 YARA 语法）

- `tests/fuzz_sigscan.rs` 的真实规则种子 2 → 5（数值比较 / for-of / xor 区间 /
  fullword / ascii wide / @ 下标 / Unscannable 正则——变异器从真实形态出发才
  打得到新语法）；危险字符 alphabet 37 → 56（补 `@ # < > ! . K B M x i n t w
  1 8 6 f o z`——新词元可被随机拼出）。确定性模糊数千次输入零 panic 不变量
  不变（外部输入第一道墙随子集一起长大）。

### Added（trit-re：间隙跳板裁定 + 间接调用数据流负结果入册）

- **实测定位**：trit-re.exe 的全部 8 个残余未归属点（全 `jmp`）= 函数间隙里
  的「`E9 → FF 25`」两级跳板链（增量链接独立跳板）；逐点查证——5/8 无任何
  引用（E8/E9 调用方、`lea r64,[rip+…]`、imm64/RVA 数据引用全查无果）、
  3/8 的引用落在非代码节。**间接调用数据流方向的原假设（"残余 = 漏检的
  数据流"）被否定**：可归因的间接调用早已被跳板展开 + 指针表收割 + 块遍历
  覆盖，残余是死机器——强行解析就是给死代码找主。
- **新裁定 `SiteRuling::ThunkGap`**（短名 `thunk-gap`）：站点是 `E9 rel32`
  且目标是 `FF 25`（rip 相对 jmp 跳板）、且不在任何函数区间 → 如实单列。
  裁定后读数：trit-re.exe / notepad.exe 的**"疑边界漏检"类别双双清零**
  （8→0、1→0，残余全部如实归入 thunk-gap / clip）——裁定是证据分类不洗白，
  方向保守不变。
- 方向库分诊更新（trit-immune SECURITY-SYSTEM.md）：间接调用数据流
  **+1 → 0 挂起**（解锁条件：真样本批次出现"疑边界漏检" > 0）。
- 测试 161 → 162（跳板链判 thunk-gap / 指向普通代码保持 miss 双向钉死）；
  门禁 fmt / clippy / test 全过。

### Added（trit-re：entrypoint 上下文接线 + `at` 语义对齐官方 + 命中表截断如实弃权）

- **entrypoint 可评估**：`SigScan::scan_with_entrypoint(data, truncated, entrypoint)`
  接收入口点上下文（`scan()` 无上下文委托、操作数弃权）；CLI 从 PE 头
  `entry_rva` 过 `rva_to_offset` 取**文件偏移**传入。官方语义实测对拍判定
  （aadtb.dll / AcpiServiceVnA64.dll：RVA≠offset 的样本上官方只认 offset）——
  不猜，跑出来的。ELF 侧留白（机器上无样本可验证语义，如实不接）。
- **`$a at X` 语义修正：首次命中 → 任一命中**（官方 YARA `at` = 在该偏移有
  命中即可）。真 PE 烟测当场揪出：高频序言 `48 89 5C 24` 在 aadtb.dll 入口
  前命中 1753 次，首次命中语义会误判 MISS——修正后与官方一致 HIT。
- **命中表封顶截断 = 扫描不完整**：`MAX_HITS` 1024 → 65536（真样本需要；
  护栏仍在，且预算 1M 步/模式不变）；触顶即 `complete=false`——`at`/`in`/
  `@a[N]` 在记录集不能证明时如实弃权（"截断的沉默不是清白"落到命中表上，
  绝不据残表判假），`#a` 计数按截断值如实下界。
- 测试 158 → 161（entrypoint 三态 + 截断弃权 + 封顶内命中照常证明）；
  CLI 真 PE 烟测 `$e at entrypoint` HIT 与官方一致；门禁 fmt / clippy / test 全过。

### Added（trit-re：YARA 完整子集——三值条件求值 + 数值/循环/修饰符）

- `sigscan` 条件求值升级**三值 Kleene**（True/False/Unknown，弃权不折叠成假）：
  - `Cmp` 数值比较：`filesize`（截断输入上如实弃权）、`uint8/16/32(lit)` 小端读
    （越界 = 弃权）、`#a` 计数、`@a[N]` 偏移、字面量（十进制/`0x`/`KB`·`MB` 后缀）；
  - `ForOf`：`for N|any|all of them/($x*) : ( 体 )`，体内 `$`/`#$`/`@$[N]` 绑定
    当前迭代串；体弃权时 N/any 在「弃权可能补足」时如实上报 Unknown，all 有弃权即 Unknown；
  - `entrypoint` 与模块/未知函数（`pe.number_of_sections`、`math.entropy(…)`）
    **记名弃权**：求值恒 Unknown——不猜、不假未命中；
  - `Const(true/false)`、`CurrentAt`/`CurrentIn`（体内 `$ at entrypoint` 经典形态）。
- 串层面：正则/`base64`/`base64wide`/`private` → `Pattern::Unscannable` 带原因
  （**规则保持解析**，其余串照常扫描，整体裁决 Unknown——不再拖累成整条 Unsupported）；
  `xor`/`xor(k)`/`xor(a-b)` 密钥区间展开（越界/反区间如实报错）、`fullword` 词边界
  （wide 按宽字符判）、`ascii wide` 双变体；文本扫描也纳入预算（256 路 xor 大文件
  如实耗尽为 Unknown，不夸大覆盖）。
- 无 `strings` 的纯数值规则合法（真实规则常见形态）。
- 行数预算拆分（守 800 行纪律）：`sigscan/{matchers,tests}.rs`、`yaraparse/cond.rs`。
- 测试：yara 集成 9 → 18（数值比较/无串规则/for-of/entrypoint 弃权/模块弃权/
  xor/fullword/base64·private Unscannable/算术 bail），全套 49 通过；
  门禁 fmt / clippy(-D warnings) / test 全过。已知边界：xor+wide 组合语义未与官方
  逐位对拍（等校准批次）；`in (a..filesize)` 区间端点仅支持字面量。

### 更正（上一条里的两个读数：盘符债已被 #293 清掉；一次 rc=101 其实是没盘）

- 上一条（`2e7eabf`）写 `docs_hygiene` 拦 1 = `docs/GENERAL-PLAN.md` 盘符 **27** / 基线 24。合完 `origin/main 0b7dd95`
  （#293）后重跑 `python scripts/docs_hygiene.py` ⇒ **绿**。两版分别数：`git show 2842cd3:docs/GENERAL-PLAN.md` 数到 **27**、
  `git show 0b7dd95:` 同一把尺数到 **24** ⇒ 那 3 处是 #293 顺手清掉的，不是我清的；"拦 1"这条撤回。
  §8 里剩下的存量红只有两类：`claims_evidence` 拦 2（`SECURITY-NET` 5 / 基线 4、`CALIBRATION` 2 / 基线 0）与
  `tech_collide` 拦 8（账本编号复用）。
- 同一次 `cargo test --workspace --locked -j 1` 出现过 **两种完全不同的红**，都记下来免得下次再糊：
  1. `rc=101` 且一条 `test result` 都没有，日志里是 `rustc-LLVM ERROR: IO failure on output stream: No space left on device`
     ⇒ 本机 E: 盘实测 **0 字节空闲**。判据"无 cargo/rustc/gcc 进程 + 目录最后写入 ≥24 h"，回收 6 个构建产物目录 **15.4 G**，
     回收后空闲 **16.5 G**，重跑才有结果可读。
  2. `179` 个单位 / **2177 passed / 1 failed**，红的那条是 `trit-wspace` 的
     `cli_read_verify_patch_repair_round_trip`（`crates/trit-wspace/tests/cli.rs:33`）：
     报错里的十进制数 `10565135468733683509` 就是十六进制 `0x929ee6bb79c05735`（`print(hex(...))` 可验），
     而它正是 `/e/tmp/trit-wspace-cli-10096/audit.jsonl`（11-05 05:51 留下）里第一条的 `behavior_before`
     ⇒ 该夹具的临时目录按 `process::id()` 命名又从不删，PID 复用就把上一轮的审计链当成本轮第一条来核对。
     单跑该目标 `6 passed` 三次全绿、`--test-threads=1/8` 都绿；GitHub runner 的 `/tmp` 每次全新 ⇒ CI 不受影响，受影响的只有本机。
     证据与建议改法已回给作者，登记为账本 **A400**（我没有替他们改那片，wsp-pilot 会话仍在跑）。
- 清盘并定位后同一条命令复跑：**224 个单位 / 2292 passed / 0 failed，rc=0** ⇒ 上面两种红都是环境与残留状态，
  产品测试面没有回退；本片合 `origin/main` 后的门线以这一条为准。
- 判据复算尺（逐字）：`python scripts/docs_hygiene.py`；`cargo test --workspace --locked -j 1`；
  `ls -d /e/tmp/trit-wspace-cli-* | wc -l`；`head -c 200 /e/tmp/trit-wspace-cli-10096/audit.jsonl`。
### Added（SCOPE §6 第 6 条 + §8：合过 main 的门线怎么分账）

- `docs/SCOPE.md` §6 加第 6 条：合并 `origin/main` 之后门线判红，要先用
  `git diff --name-only origin/main...HEAD` 确认红项所在文件不在本片改动里，再把红归到债务条目。
  禁区两条：**不许调宽基线让它变绿**、**不许把"本片没红"写成"门全绿"**。
  同时写明 `docs_hygiene` / `claims_evidence` / `tech_collide` 三道门并进 CI 之前必须先清零存量红——
  并进去的那一刻，同一批存量红会拦住所有 PR。
- `docs/SCOPE.md` §8 记 2026-11-06 逐字复跑的存量红：`docs_hygiene` 拦 1（`docs/GENERAL-PLAN.md` 盘符绝对路径
  **27** 处 / 基线 24）、`claims_evidence` 拦 2（`docs/SECURITY-NET.md` 无据能力句 **5** 句 / 基线 4；新建的
  `docs/CALIBRATION.md` **2** 句 / 基线 0，新文件没有豁免）、`tech_collide` 拦 8（交接账本编号复用
  A129/A130/A131/A134/A153/A155/A157/A267）。逐号与复算尺在账本 **A399** / **A367**，都不是这片引入的。

### Measured（watched literals 的靶子：匹配族 µs/节点基线，release、机器空转、5 次重复）

- `cargo run --release -p trit-count --example matching_family` 在当前头（合完 `origin/main`）的读数，
  每档给 **中位数（5 次极差）**：8 对 **10.8（9.8–11.1）**、12 对 **16.3（14.2–18.4）**、16 对 **20.9（18.4–22.4）**、
  20 对 **25.7（24.2–28.2）**、24 对 **31.5（29.8–34.8）**、28 对 **37.1（34.5–40.4）**、32 对 **42.5（39.6–46.3）**、
  40 对 **52.8（50.2–54.9）**；节点数五次逐位不变（8 对 203 … 40 对 1163）⇒ 变动的只有壁钟。
- **20 对起 BDD 一律 refused**（`BDD 节点数超过上限 200000`），8–16 对 `ok` ⇒ 这一族确实是
  "只有 DPLL 出得了数"的靶子。µs/节点从 8 对的 10.8 涨到 40 对的 52.8（**5.0×**）
  ⇒ 单节点开销随子问题变大，watched literals 的判据就是压平这条上升线。
- 判决口径：重复极差在小档更宽（12 对 ±13%、16 对 −12%/+7%、20 对 −6%/+10%、40 对 ±5%），
  所以**判决只看 20 对以上，且改进必须大于当档极差**；8–16 档只用来确认没有回退。
### Fixed（三平台 CI 的 windows `capacity_exec` 栈溢出：根因找到并修掉，另撤回我一次错判）

- **撤回**：我先前把这次红判成"runner 面"（理由是 `size_of::<Commands>()` 只涨了几十字节 + 本机全绿）。
  这个判断是错的：main 最近 5 次 CI 全绿，而我的头**两次**红在同一处 ⇒ 触发点在分支上，不在机器上。
- **本地造出尺子**（不需要 CI）：`RUSTFLAGS="-C link-arg=-Wl,--stack,524288"` 把 `trit.exe` 的主线程栈压到 512 KB，
  `cargo test -p trit-cli --test capacity_exec` 立刻复现同样的 5 条红 + `thread 'main' has overflowed its stack`
  （同一把尺下 Windows 默认 1 MB 只是刚好躲过 ⇒ 任何让解析变大的改动都会顶穿 CI）。
- **根因**：吃掉栈的是 **clap 对 `Commands` 这张 20+ 分支枚举的解析**——debug 构建下每个分支的临时量摊进同一个栈帧。
  我先试"只把分发搬到工作线程"，**没用**（同一把 512 KB 尺下仍然红，报的还是 `'main' has overflowed`），
  因为 `Cli::parse()` 本身就在主线程上；改成把 **解析 + 分发**一起搬进 64 MiB 栈的线程才过。
- 修法：新增 `crates/trit-cli/src/bigtask.rs`（`STACK_BYTES = 64 MiB`，worker panic 以 101 退出，不伪装成功），
  `main()` 现在只做 `bigtask::on_big_stack(|| run(Cli::parse()))`。子命令的多少不再取决于平台默认栈。
- 实测（同一把尺，主线程 512 KB）：修前 `capacity_exec` **5 条全红** ⇒ 修后 **5 passed**；
  整个 `cargo test -p trit-cli` 在 512 KB 下 **105 条全绿**（42 + 35 + 5 + … 共 8 个结果行）。
  常规构建（默认栈）同样全绿；`bigtask` 自己带 2 条测试：一条用 40 层 × 64 KiB 帧（>2.5 MB 递归）
  **证明线程真的拿到配给的栈**，一条证明 `Ok`/`Err` 原样带回。
- 探针那一笔（`3298326`，临时摘掉 `count` 子命令）已用它自己的 revert 撤销（`5ec9e60`），
  过程中 `test_surface` 因"删测试"判红 —— 保留那个红、没有用 `#[ignore]` 糊门。


### Added（再吃掉 4 份手写 xorshift：纹理两条 bench、电路 bench、编解码 example —— 精确尺 60 → 56）

- `crates/trit-texture/benches/sample.rs` 与 `texture.rs`：删掉各自 `struct Rng(u64)`，改吃 `trit_fuzz::SeedRng`。
  **不只是编译过**：`cargo bench -p trit-texture --bench sample` 真跑出来那张表（双线性 62.72 vs 整图 24.57 ns/样点、
  各向异性 taps=8 自由 711.27 / 缓存 714.64 ns/样点、显存 1084 KiB vs 16320 KiB = 15.1×）。
- `crates/trit-circuit/benches/core.rs`：把内联的三行 xorshift 闭包换成 `SeedRng::next_u64`。
  跑过：`by match` 7.587 vs `by lut` 0.837 ns/op（random 档 9.06×），`exact k=12` 514.63 µs/iter。
- `crates/trit-codec/examples/ternary_corpus.rs`：同样改吃 `SeedRng`，跑过（写出 171 行 truth / 85 行 balanced 语料）。
- 四处都是**同一条 13/7/17 移位** ⇒ 语料序列与改动前逐字相同，读数口径没有被悄悄换掉。
- 顺手抓到自己的一个手误：往 `[dev-dependencies]` 里插 `trit-fuzz` 时插重复了一行（`trit-codec` 那份本来就有），
  `cargo check` 直接报"cannot load manifest"——靠编译发现，不是靠眼看。六个 manifest 复查计数现在都是 1。
- 复算尺：`grep -rlE "\^=.*<< 13" --include=*.rs crates/ | wc -l` ⇒ **56**（本会话起点 67）。


### 读数（DPLL 在随机 CNF 上的规模档，release 实测）

- `trit-count --fuzz-cnf --nvars N --cases 1`（N=18/20/22/24 各跑 3 次，同一颗种子 ⇒ 语料逐次相同）：
  **最慢一题** 18–22 变量档 **183–294 µs**（三次分别 189/203/192、183–294、188–192），
  24 变量档 **490–538 µs**（519/538/490）⇒ 6 个变量涨约 **2.6×**；每一档都是"该规模以内全部题目
  与 `2^n` 穷举逐条一致"。复算：`cargo run --release -p trit-count -- --fuzz-cnf --nvars 24 --cases 1`。
- 这不是优化读数，是**地板读数**：以后 DPLL 的任何提速要在同一把尺（同种子、同规模档）上跟它比，
  否则"快了多少"没有意义。


### Added（文档行号指针对账门 `scripts/doc_pointers.py`）

- 仓里大量用 `文件.rs:行` 当复算指针。代码一动行号就漂，而**漂了的那句话仍然通顺**——没人会发现。
  本门逐条按行号真去读文件：① 文件指不到 ⇒ 拦；② 只写文件名且全仓多命中 ⇒ 拦"歧义"（要求写带目录的路径）；
  ③ 行号越界 ⇒ 拦并报出真实行数；④ 指针旁边 40 字符内的反引号标识符必须出现在该行 ±2 行内，
  否则拦"指歪"（这是"行号还在、内容已经不是那件事"唯一可机器判的形式）；⑤ 一条指针都没抓到也判红（失明 ≠ 干净）。
- 第一版按**整句**核标识符，被一处 `crates/trit-parts/Cargo.toml:9` 的引用误伤：同句后半段提到的一个类型名离指针很远，
  却被当成"这行写的是什么"的说明 ⇒ 改成只看指针后面 40 字符 + 前面 12 字符，误报清零（口径写进代码注释）。
- 首跑抓到 **8 条**坏指针，全部当场改对：`CHANGELOG` 两条指向**已被删除的** `random_expr` 副本（改成"已在 `c71314c` 删掉合并"）、
  两条 `tconv/conv.rs` 与一条 model.rs 的引用是歧义/相对路径（改成带目录的全路径）、`matnet.rs:1-4` 这种区间写法不被认（改成 `:3` 指到结构行）。
  现 **19 条指针全绿**；台账基线随首建归零（`doc_pointers_baseline.json` 只在有存量时才需要）。
- 自测 1 正例 + 7 反证 + 1 条"变少允许"；变异登记 6 条，官方门 **断言抓住 6 / 存活 0 / 崩溃 0 / 假杀 0**
  （第一条跑时有一条存活，定位是我把锚点指在 `main()` 的 `--update` 分支而非 `compare()`，换锚后被抓到）。


### Fixed（三平台 CI 的 Clippy 步骤判红：`manual_contains`，以及我本地跑错了口径）

- CI 报红的是**新 lint** `clippy::manual_contains`（rust 1.99）：我在 `crates/trit-bench/tests/bench.rs:170`
  写的 `seq_a.iter().any(|&v| v == 0)` 该用 `seq_a.contains(&0)`；`-D warnings` 下就是错误，
  ubuntu/macos/windows 三条作业同时红在 Clippy 步骤。
- **我本地的复核口径错了**：这轮我只按单个 crate 跑 `cargo clippy -p <crate> --tests`，
  而 CI 的口径是 `cargo clippy --workspace --all-targets --locked -- -D warnings`
  ——`--tests` 不含 examples/benches，逐 crate 也漏掉没点名的包。改成**和 CI 逐字同一条命令**复跑，
  `rc=0` 才算数（本轮实测：修前该命令报 1 条，修后 0 条）。

### Added（`trit-immune/tests/properties.rs` 也改吃 `SeedRng`：重复度 61 → 60）

- 上一轮我写"这件不做，因为要清 20 处 `as usize`"——这轮把它做完了：`below(u64)->u64` ⇒ `SeedRng::below(usize)->usize`，
  顺带去掉 `rng.below(x.len() as u64) as usize` 一类的转换（逐类替换命中 14 处：4 纯数字、4 `.len()`、2 变量、
  4 处 `Rng(seed)` 构造；另把 6 处 `rng.pick_trit()` 改成自由函数调用）。
- `pick_trit` 留在本文件（`Trit` 是 trit-immune 的域概念，不进公共语料件）。
- 口径核对：本地 `below` 一直是 `next() % n`，`SeedRng::below` 也是 ⇒ **探索序列不变**（每步抽样次数一样）。
- 实测：`cargo test -p trit-immune` **23 个结果行全 ok**、`clippy --tests` 无告警；
  精确尺 `grep -rlE "\^=.*<< 13" --include=*.rs crates/` ⇒ **60**（轨迹 67 → 65 → 63 → 61 → 60）。


### Added（再吃掉两份手写 RNG：`trit-float` / `trit-immune` 的测试改吃 `SeedRng`）

- `crates/trit-float/tests/stress.rs`：删掉本地 `struct Rng`，`next()` ⇒ `next_u64()`，
  `unit` / `signed` 留在本文件当自由函数（那是它的进料形状，不是公共件）。测试 **2 条全绿**。
- `crates/trit-immune/tests/fuzz_smoke.rs`：同样删掉本地 `Rng`，7 个取样点改吃 `SeedRng`。测试 **10 条全绿**。
- 重复度读数（同一把精确尺 `grep -rlE "\^=.*<< 13" --include=*.rs crates/`）：**63 → 61**。
  口径注明：把 `benchmarks/` 也算进来是 62，本文引用的都是 `crates/` 口径。
- 同一条 xorshift（13/7/17）⇒ **序列不变**，所以这两份 fuzz/高压测试探索的输入集合跟改之前一致，
  没有"顺手换了语料"的隐蔽差别。
- **没做**：`crates/trit-immune/tests/properties.rs`（它还有本地 `below(u64)->u64` 与 `pick_trit`，
  换成 `SeedRng::below(usize)->usize` 要连带清 20 处 `as usize` 转换，否则 clippy 的 `unnecessary_cast`
  会在 CI 的 `-D warnings` 下判红）、`trit-circuit` 的两份（该目录被 sess 179ea139 认领中，不动别人的面）。
- 门线：`clippy --tests` 对 `trit-float`/`trit-immune` 无告警；`test_surface` / `claims_evidence` /
  `docs_hygiene` / `module_budget` / `layers` / `deps` / `publish_graph` / `ci_integrity` / `hazard_gate` /
  `model_status` 全绿。

### Added + 更正（`trit-dot` / `trit-codec` 的 stress 改吃 `trit-fuzz`；重复度尺子换成精确尺并当众改数）

- `trit-fuzz` 补三个单值取样件：`trit`（`Ternary`）、`ternary_value`（`-1/0/+1`）、`bipolar`（`±1`）。
  断言 1 条新增（`single_draw_helpers_are_balanced_and_typed`：3000 次取样每档 800–1200、`bipolar` 只出 `±1`），
  `trit-fuzz` 现 **11 条**测试；变异登记 7→**9 条**，官方门 **断言抓住 9 / 存活 0 / 崩溃 0 / 假杀 0**。
- 两个消费者改吃它，各自删掉本地 `struct Rng(u64)`：
  `crates/trit-dot/tests/stress.rs`（`ternary`/`bipolar`/`ternary_f32` ⇒ `ternary_value`/`bipolar`，5 条测试全绿）、
  `crates/trit-codec/tests/stress.rs`（`next` ⇒ `next_u64`、`trit` ⇒ `corpus::trit`，5 条测试全绿）。
  _codec_ 那份本地映射是 `0→False, 1→Unknown, 2→True`，`corpus::trit` 是 `0→False, 1→True, 2→Unknown`
  —— 两者都均匀，测试断言的是往返/纠错性质而非具体序列；**这一条差异写在测试文件头顶**，不是悄悄换的。
- **读数更正（我自己的尺子不严谨）**：第一批立项时我用的是宽松尺
  `grep -rl "<< 13" | xargs grep -l ">> 7|>> 17"`，数到 **64**；这条尺会把**散文里提到的模式**也算进去
  ——我自己写的 doc 注释就中招了。换成精确尺 `grep -rlE "\^=.*<< 13"`（同一行必须有 `^=`）后：
  本会话前 **67**（`581d54b`）→ `trit-fuzz` 第一批后 **65** → 本轮两个 stress 改吃后 **63**。
  三个数都用 `git grep` 在同一份路径集（`crates/*.rs` + `benchmarks/*.rs`）上复算过；
  旧条目里"64 个文件"就地改成"读数更正见下条"，不抹掉原文。
- 门线：`claims_evidence` / `test_surface`（更新后见下）/ `docs_hygiene` / `module_budget` / `layers` / `deps` /
  `publish_graph` / `ci_integrity` / `hazard_gate` / `model_status` 全绿；`cargo clippy --tests` 对
  `trit-fuzz`/`trit-dot`/`trit-codec` 无告警。

### Added（`trit-count --fuzz-cnf`：把随机 CNF 对账从测试里搬到工具上）

- 新公开尺 `trit_count::oracle::count_cnf_exhaustive(&Cnf)`（库里那份），与 `tests/cnf_fuzz.rs` 里
  **故意另写**的朴素枚举互为独立复核——两条链共用一把尺就等于没有复核。
- CLI：`trit-count --fuzz-cnf [--cases N] [--nvars MAX] [--len K]`，默认 `nvars=1..=16` 各一条；
  `--nvars` 超过 40 直接用法错误码 2（穷举是 `2^n`，不给它开始等死的机会）。
  release 实测：`--fuzz-cnf --nvars 20` ⇒ **20 例全对，最慢一次 191µs**（读数也写进 README 那行）。
- 新增断言 3 条（`cnf_fuzz.rs` 现 **4 条**）：432 例 DPLL vs 穷举、两把穷举尺逐例相同、CLI 报一致 + 越界守卫。
- 变异登记 `trit-count` 21→**22 条**，新增的 "oracle: CNF 穷举把负文字当正文字"
  用命名过滤实测 **FAILED**（抓住），源码随即恢复。
- `cargo clippy -p trit-count --tests` 无告警、`main.rs` 494 行（离 800 行门还远）。

### Added（`trit-fuzz` 第二批：CNF 语料 + 它的第一条消费者）

- `random_cnf(rng, nvars, nclauses, clause_len)`（DIMACS 口径：文字是 `±1..=nvars`）与 `random_assignment`。
  子句规范化口径写死在生成器里：**无重字、无永真（同变量两符号不同时出现）、无空子句**；
  `clause_len` 是上界（小 `nvars` 时结果可能更短）；`nvars==0` 或 `clause_len==0` 返回空表，
  而不是悄悄造一条空子句把公式判死。
- 新消费者 `crates/trit-count/tests/cnf_fuzz.rs`（**2 条**）：这条链和 `crosscheck.rs` 不同——
  那里从 `Expr` 出发经编码器造 CNF，编码器写错会**同时**骗到两边；这里跳过编码器直接喂随机子句集，
  钉的是 DPLL 本体（规范形整理、单元传播、分量分解、缓存键）。
  实测：**432 个**随机 CNF（`nvars` 1–9 × 长度 1–4 × 12 次）与 `2^nvars` 穷举逐一对账全等。
- 判决性实测：把 `crates/trit-count/src/dpll.rs:78` 的 `let free = k - assigned - m` 改成 `k - assigned`
  ⇒ `dpll_counts_random_cnfs_exactly` 报 **432 个里 96 个不一致**。
- 变异登记从 4 条扩到 **7 条**，官方门 `python scripts/mutation_check.py --pkg trit-fuzz` ⇒
  **断言抓住 7 / 崩溃 0 / 假杀 0 / 存活 0**。其中一条我先登记后**删除**并如实说明：
  "永真子句只撤一半"（`clause.remove(i)` 只做一次）在我这套形状断言下**不可观测**——撤一半后子句仍合法，
  既没有重字也没有永真；它只改变分布强度，而分布不是我要钉的性质。留着自己存活会让门误报"断言太弱"，
  所以删条目 + 写下"这条性质没被锁住"，比假装锁住诚实。
- `test_surface` 台账 94 单位 / **2228** 条（+6）。

### Added（`trit-fuzz`：把测试语料生成收进 base 层第一批）

- 立项判据是一条能量化的尺，不是"感觉重复"。**读数更正见下方"精确尺"那条**：当时我用宽松尺
  `grep -rl "<< 13" | xargs grep -l ">> 7\|>> 17"` 数到 **64 个文件**，后来换成精确尺
  `grep -rlE "\^=.*<< 13"`（要求同一行真有 `^=`）⇒ 本会话前 **67**、这一批合并后 **65**。
  其中 `trit-count` 一个包里有两份**逐字相同**的
  `random_expr`（`trit-count` 的 bin 与 `crosscheck` 测试各一份，两份都已在 `c71314c` 删掉合并）。
- 新 crate `crates/trit-fuzz`（base 层，只依赖 `trit-core` + `trit-bench`）：`random_trits`、`random_expr`、
  `var_names`，RNG 本体复用 `trit_bench::SeedRng`（同 13/7/17 移位 ⇒ 换包不换序列），并在 `trit-bench` 上补
  `SeedRng::below(limit)`（`limit == 0` 直接 panic，不静默回 0）。
- `trit-count` 两份副本删掉、改吃 `trit-fuzz`：**测试逐条不变**（`cargo test -p trit-count` 16 + 10 全绿，
  语料序列口径相同 ⇒ 没有金样被顺手改掉）。
- 断言 6 条（`crates/trit-fuzz/tests/corpus.rs`）：同种子逐条重放、域封闭（不冒出域外变量）、深度上界、
  `depth=0` 只出叶子、3000 个三值每档落在 800–1200、空变量表判红而不是悄悄全出常量、`var_names` 在 26 处换样式。
- 变异 4 条 **抓住 4 / 存活 0 / 崩溃 0 / 假杀 0**（`python scripts/mutation_check.py --pkg trit-fuzz`）；
  `layers.json` 归位 `trit-fuzz = base`，`test_surface` 台账随之更新。
- **还没做**：CNF/子句夹具（下一步从 `trit-codec`、`trit-dot` 的 stress 语料里收），以及那 60 多份手写 RNG 的逐步改吃——
  这两件都没动别人的承重面，不在本轮范围。

### Added（完成度表对账门 `scripts/model_status.py`：表里的数字改成机器量的）

- `docs/STATUS.md` §2 那张"六个像模型的东西"的表一直是**手填**的（内联 `#[test]`、被 bin 用、`tests/` 提及、判定词）。
  手填的表会烂：代码动了表不动，"已跑通 / 组件"就变成假话——正是用户最反感的迷惑性来源。
- 新门逐行复算四列计数并核对判定词：任一不符 ⇒ 判红；判「已跑通」但 `tests/` 提及 0 且"读数在哪"没指到
  文件/§/PR ⇒ 判红；判定词漂出 §1 词表 ⇒ 判红；**表头读不出来也判红**（失明不等于对上了）。
  `--show` 直接打印复算出来的表行，改表不用手数。
- 第一次对账就抓到我自己写的 5 处不符（列口径从"断言条数"变成"类型名在 tests/ 的代码行提及数"）：
  `DepthNet` 4→**11**、`TpmModel` 0→**2**、`DepthUNet` 3→**7**、`TriModel` 5→**0**（它的覆盖经由 `to_tri()`/`RefCpu`
  发生，按类型名数不到）、`TriModel` 被其他模块用 0→**1**。表已按复算口径重写并在表下写明边界与原因。
- 自测 1 正例 + 7 反证（含"注释行也算提及"这类口径放宽的反证）；变异登记 5 条，
  `python scripts/mutation_check.py --pkg scripts/model_status.py` ⇒ **断言抓住 5，存活 0，崩溃 0，假杀 0**。
- 本轮我自己的两个工具性错误（都是老熟人）：**补丁空操作**——用没有 `assert` 的字符串替换改 `planes()` 签名，
  只有调用点生效；**编码**——自测打印用 `⇒`，中文 Windows 上变异门起子进程是 cp936，直接
  `UnicodeEncodeError` 把"门自己崩了"伪装成测试失败；已在 `main()` 里显式把 stdout 定成 UTF-8，
  并把自测失败行改成行首 `FAIL <例名>`（否则门的"未点名判红"口径会把真抓住误读成没点名）。

### 否证（"下一刀省掉分解/拷贝"——消融实测把它打回去了）

- 消融口径：临时把 `crates/trit-count/src/dpll.rs:91` 的 `decompose(renumbered, m)` 强制走 `None`
  （只是测量，不留在树里；表里 `comps=0` 就是它生效的证据），跑同一个 `--example matching_family`。
- 读数：**8 对** 2.2 ms / 203 节点 → **23.1 ms / 10,019 节点**；**12 对** 4.7 ms / 323 节点 →
  **1807 ms / 729,059 节点**（节点数 ×384，时间 ×385）。消融后 µs/节点反而只有 2.3–2.5（子问题变小了）。
- 结论：分量分解的开销与它省下的搜索**不在同一量级**，所以基线里 µs/节点随规模从 10.7 涨到 47.9 µs
  是"子问题本身变大"的正常结果，**不是分解做多了**。之前记的"下一刀该减 `decompose`/拷贝"作废，
  要再提速得往别处找（分支启发式与缓存键；watched literals 的靶子是传播侧，与本条不冲突）。
- 写回代码注释：`dpll.rs` 头部现在带这组数，免得下一个人（或下一个我）再猜一遍。

### Fixed（`count_cmd.rs` 自己找不到二进制：MSRV 作业里五个测试全红，不是计数命令坏）

- CI 读数：**MSRV (1.88)** 失败的是我新加的 `crates/trit-cli/tests/count_cmd.rs` 5 条，报错
  `failed to run trit binary: Os { code: 2, kind: NotFound }`；根因是我写的取径
  `env("CARGO_BIN_EXE_trit").unwrap_or_else(|| format!("trit{}", EXE_SUFFIX))` —— **cargo 在那个作业里没注入该变量**，
  回退成"让 PATH 去找裸 `trit`"必然找不到。同目录的 `tests/cli.rs` / `capacity_exec.rs` 一直是
  "env 取不到就按 `current_exe()` 往上两级推 `target/debug/trit`"，我没照抄。
- 修法：换成同一把尺（`trit_bin()`），并把静默回退改成**显式判红**（`assert!(bin.exists())` +
  `unwrap_or_else(panic)` 点名路径）——"测试自己找不到二进制"不能伪装成"产品不工作"。
- 复现验证：直接跑 `target/debug/deps/count_cmd-*.exe`（**不经 cargo**，因此没有 `CARGO_BIN_EXE_*`）
  ⇒ 改前 5 条全红于同一行，改后 **5 passed**；`cargo test -p trit-cli --test count_cmd` 本地同样 5 passed。
- 同一次 CI 里 **windows-latest** 挂的是既有 `capacity_exec.rs` 5 条，子进程报
  `thread 'main' has overflowed its stack`（退出码 `-1073741571` = `0xC00000FD`）。
  先按证据排除自己：实测 `std::mem::size_of::<Commands>() = 184` 字节（新变体内联只带来十级字节的增量），
  本机 `cargo test -p trit-cli --test capacity_exec` **5 passed** ⇒ 不像本 PR 的成因；
  重跑该作业再看是否复现（复现就按 Windows runner 的栈/内存面查，不动别人的测试）。

### 否证（`MatNet` 不改名——原本列为"命名整改"的欠账）

- `docs/STATUS.md` §4 第 4 条提议"是组件就改名 `MatHead`"。读码把它**否证**了：
  `crates/trit-parts/src/matnet.rs:3` 写的是独立训练的四类分类器（`conv(3→H) → ReLU → conv(H→H) → ReLU → 全局平均 → 线性(H→4)`），
  不是接在骨干后面的头 ⇒ 叫 `MatHead` 比原名更迷惑。
- 外部 pin 实测（决定改名有没有代价）：全机代码面只有 `crates/trit-parts/src/bin/parts_3d/material.rs` 引用，
  其余 `MatNet` 命中都在同一仓的 worktree 副本里；`crates/trit-parts/Cargo.toml:9` 是 `publish = false`
  ⇒ 本来不受 semver 约束。结论：**维持 `MatNet`**，把它在 §2 表里的判定（组件：1 个 bin 用、无仓内复用）写准。
  迷惑性的修法是判定与读数，不是换名字。

### Added（把「计时夹具」登记成分层能力：权威 = `trit-bench`，两份手写档带实测差）

- `scripts/layers.json` 的 `capabilities` 新增 `bench-timing-harness`：权威 `trit-bench`，镜像两份——
  `trit-ops/src/bin/bench.rs`（最完整的手写档：交错 + min/median/max + 轮内轮换，`trit-bench` 的轮换就是从它抄的；
  该文件被 sess 179ea139 认领中，不悄悄合）与 `trit-parts` 的 `parts-bench`（GPU + 仓外 ckpt，本机跑不起来 ⇒ 无法先验证尺子没换）。
  每条都带 `measured_gap`（A357 迁移前后同档：u64 dispatch 8.1–9.3 vs 手写 7.6–12.2 ps/trit 等；小档量化台阶
  100/200 ns → 67.5/72.8/82.8 ns）与 `why_kept`，门报「能力登记 2 条、第二实现 3 份」绿。
- **我自己写坏过一条复算指针**：先填的是 `cargo test -p trit-ops --release bench`（凭印象猜的），
  实际 `crates/trit-ops/Cargo.toml:14` 是 `[[bin]] name = "bench"` ⇒ 正确口径 `cargo run --release -p trit-ops --bin bench`；
  `parts-bench` 的参数也不是位置参数而是 `--dir <parity 目录>`。两处都按 `Cargo.toml` 与 bin 源码逐字改过——
  复算命令写错等于没有复算尺。

### Added（`trit count models`：把计数引擎接到命令行）

- `trit-cli` 新增子命令 `count models <expr>`（`crates/trit-cli/src/commands/count.rs`，经 `#[command(subcommand)]`
  嵌套，main.rs 只长 6 行、离 800 行的上帝模块门还有 12 行）。参数：`--vars` / `--constraints` / `--binary` /
  `--target true|false|unknown` / `--crosscheck` / `--bdd-max-nodes` / `--json`。
- 实测读数（穷举独立复算过）：`a & b` 为真 **1**；`a | b` 为真 **5**，`--binary a` 收窄到 **4**；
  `a & b` 为**未定 3**；`a & b` 在约束 `!a` 下 **0**；`--bdd-max-nodes 1` 时报
  `BDD 拒（BDD 节点数超过上限 1）` 且 `matches=true / three_way=false`（拒绝不等于不一致）。
- 新测试 `crates/trit-cli/tests/count_cmd.rs`（**5 条**，跑真二进制）。判决性实测：把 DPLL 调用里的
  `&binary` 换成 `&HashSet::new()` ⇒ 只有 `binary_vars_narrow_the_domain` 判红。
- 又踩一次**陈旧 exe** 的坑：变异探针把 `trit.exe` 重编译成被改坏的那版，恢复源码后**没重建**就直接跑，
  于是读到 `--binary a` 报 5（其实是变异体在跑）。重新 `cargo build` 后才报 4。
  顺手把这条做成锁：文本模式加断言 `!stdout.contains("BDD BDD ")`，同一个坑要用测试而不是记性来挡。
- `README.md` 加「模型计数」一节（含上面每条读数与复算来源）；`--bdd-max-nodes` 的适用命令列表补 `count`。

### Added（`DepthNet` 的 `tests/` 端到端：三值格不变量、膨胀可达集、留出泛化）

- 第二个 `tests/` 文件 `crates/trit-parts/tests/depthnet_e2e.rs`（**4 条**，全绿）。与内联 7 条**不重叠**：
  内联测"学斜坡 / 学常数 / mv 门 / save-load / 确定性"，这里测必须跨模块才看得见的东西。
- **三值格不变量**（这条是三进制命题的行为锁，不是权重检查）：把 `c1.w[0]` 换成同一量化格内的另一个值
  （`q(w)=round(w).clamp(-1,1)`）⇒ 三值档前向**逐位不变**；`ternary=false` 档同一个扰动**必须改变输出**。
  两条合起来才排除"标志位被忽略"。判决性实测：`crates/trit-parts/src/tconv/conv.rs:113` 的 `q` 改成恒等 ⇒ **只有这条判红**。
- **量到一个非显然的实现性质**：膨胀核抽头在 ±dil 的**整数倍**上，所以孤立冲击的可达集落在 dil 格点上——
  实测 `dil=1` 依赖 16 个像素（{0,1,2,3}²）、`dil=2` 最远 (6,6)、`dil=3` 最远 (9,9)、`dil=5` 最远 (15,15)。
  第一版夹具选了"角落 (15,15)"，理由是"离冲击够远"，结果 15 不在 `dil=3` 的格点上 ⇒ **报红的是夹具不是实现**。
  改成 `(6,6)`（`dil=1` 无关 / `dil=3` 相关）后成对判据成立；`crates/trit-parts/src/tconv/conv.rs:151` 的 `self.dil` 改成常量 `1` ⇒ 这条判红。
- **留出集泛化**（不是训练集）：3 个方向训练、第 4 个方向（反对角）从未出现，1200 步后 MSE **0.1710 → 0.00620**（27.6×）。
- **部署构成**：`cin=6 / pyr=1 / dil=2 / med=-0.75 / iqr=1.25 / 量化档(1,1,0)` 经 `save`→`load` 全字段一致且前向逐位一致；
  未知 magic 的报错点名 `不是 DCN1/DCN2/DCN3/DCN4`（不吞成"读出来了"）。
- `docs/STATUS.md`：§2 表格 `DepthNet` 的 `tests/` 从 0 改 **4**，新增 §2.3 记读数与两条变异；
  §4 第 3 条收口（仍欠 `DepthHead`/`MatNet`，但按 §1 它们是组件，没有端到端可言）。
- 本轮我自己的又一次同型错误，如实记（`docs/STATUS.md` §3 有台账）：改 `planes()` 签名时用了一次没有 `assert` 的字符串替换，
  **补丁空操作**（只有调用点的 `&` 生效、函数签名没改），靠编译报错才发现——判据的输入必须是刚读到的。

### Added（`trit-parts` 第一批 `tests/` 级端到端断言：`DepthUNet` 与 `TriModel` 从"已实现未测"翻篇）

- 新文件 `crates/trit-parts/tests/models_e2e.rs`（**9 条**，`cargo test -p trit-parts --test models_e2e` 全绿）：
  此前 `crates/trit-parts/tests/` **一个文件都没有**，六个"像模型的东西"的断言全在实现文件的 `mod tests` 里。
  这条链选的是**不依赖 `gpu` feature** 的那一段（`tpm::from_net` / `net::Net` 都在 `gpu` 后面，CI 默认特性跑不到），
  改成加载随仓的 `models/parts_id8118.tpm`（13 层 3→32→…→32→16）→ `to_tri()` → `RefCpu::forward`。
- 读数（2026-11-05 实测）：`DepthUNet` 16×16、h=4、301 步 MSE **0.3602 → 0.01051**（34×），
  左右两半均值 **−0.586 / +0.580**（目标 ∓0.6）——只看 MSE 会放过塌成常数的解，所以加区域判据；
  全 false 掩码一步 ⇒ 前向输出**逐位不变**；`save`→`load`→前向**逐位一致**，坏头报 `不是 U1` 不 panic。
- 三值不变量 + 部署语义：13 层每个 `|E|` 要么 0 要么等于该通道 `|scale|`；`to_tri()` 的 E 与 `TpmLayer::effective()`
  **逐位相等**（防止中间混进第二次量化）；`truncate_head(4)` 后留下的 4 个平面逐位不变。
- **金样指纹**（FNV-1a 64）：res=8 `f6de046f0b41c1f6`、res=16 `05f25dc40107c04a`。判决性实测：
  把 `RefCpu::pool` 的 `m.max(v)` 换成 `(m+v)/2.0` ⇒ **只有金样这条判红，其余 8 条全绿**
  ⇒ 没有这条 pin，池化写错根本拦不住。这就是"结构测试冒充端到端"的差别，也是 `docs/STATUS.md` §2.2 写它的原因。
- 尺子自证：`compare_nhwc(a,a)` 报 `(1.0, 0.0)`；翻掉一个像素的 argmax ⇒ 一致率 319/320（既判红又不至于"改一个像素判整帧"）。
- 本轮我自己的两个夹具错（都当场被门/红抓出，写进 `docs/STATUS.md` §3）：① 第一次"抬平面 0"抬的就是本来就是
  argmax 的通道 ⇒ 断言红但其实没测到尺子；② 重算覆盖表时用 `grep -c` 单文件不带文件名，被 `awk -F:` 取空 ⇒
  六个类型全报 0，看着像"补的测试没生效"。
- `docs/STATUS.md` §2 表格随之更新：`DepthUNet`/`TriModel` 判**已跑通**（CPU 参考层），
  当时 `tests/` 覆盖仍是 **0** 的还有 `DepthNet`、`DepthHead`、`MatNet`（`DepthNet` 由下一条补掉）。

### Added（能力句门 + 账本去重判定，把"我以为做完了"变成机器判）

- 新门 `scripts/claims_evidence.py`：能力面（`README.md` + `docs/**.md`）里出现完成类动词
  （支持/已实现/已完成/达标/已跑通/已验证/已落地/已修/已合…）却**既无读数也无复算指针** ⇒ 判红；
  读数必须带单位（"提升 30"不放行）；指针接受反引号命令、`文件.rs:行`、`PR #N`、`§`、`A<编号>`。
  **CHANGELOG 明确不算能力面**（历史叙述，"已修 X"是记录不是声明）。
  存量如实进棘轮基线：18 个文件共 **76 句**（`docs/GENERAL-PLAN.md` 11 句最多），只许变少。
  自测 3 正例 + 3 反证；变异 4 条 **caught 4 / survived 0**。
- 上一轮立的 `scripts/tech_collide.py` 补上**账本一半**（`judge_ledger`）：同一编号出现多次 ⇒ 判红，并区分
  「**编号被两件事复用**」（标题不同，"说 A 完成了指哪一件？"）与「同一件事重复登记」；
  同条目 open 与 `[x]` 并存也判红。实跑在真账本抓到 **8 对复用**（A129/A130/A131/A134/A153/A155/A157/A267），
  已带行号立成 LEDGER 独立条目；去重要由原作者改号，我没有替别人重编号。
- 我这轮三次同类自错，全部当场被自己的门抓出并改正，不写进"已完成"：① `expect` 反向逻辑写错两次
  （正例被当失败、失败被当通过）；② 变异写成 `re.compile(...) and re.compile(...)` 的**等价变异**（存活，换成实削弱）；
  ③ 首版把 CHANGELOG 纳入能力面导致 26 句误报（收窄口径）。
- `docs/SCOPE.md` 的「规矩 ↔ 门」对照表补两行（能力句、同技术撞车）；`docs/STATUS.md` §1 第一条欠账划掉。

### Added（同技术撞车门 + 完成度口径，反"我以为做完了"）

- 新门 `scripts/tech_collide.py`（只读，不改认领表与技术索引）：认领门过去只比**路径**相交，
  所以两个人在不同文件里造同一个轮子永远不会撞车。本门读 `CLAIMS.md` × `TECHMAP.json`，分三档：
  两条在效期声明命中**同一条目** ⇒ **拦**并双方点名（附对方条目的 title/why/实测读数）；
  命中的是**不同条目但共享 ≥2 个标签** ⇒ **只警**，把对方读数与理由印出来，要求 PR 写清适用域差异
  ——**换了域不算重复**（"你这个不行不代表别的领域不行"）；声明命中不到任何条目 ⇒ 提示先登记索引。
  读不到索引/声明 ⇒ 判红（不知道有没有撞车不等于没撞车）。self-test 1 正例 + 4 反证，变异 5 条 caught 5。
- 新 `docs/STATUS.md`：定四个状态词（**已跑通 / 已实现未测 / 组件 / 已否证**），并把"能力句必须带复算命令 + 读数"
  写成口径；随后按口径量了 `trit-parts` 六个"像模型的东西"：只有 `DepthNet`（4 个 bin、7 条内联测试、有清单与复核读数）
  与部分 `TpmModel` 够得上已跑通；`DepthUNet`/`TriModel` 是**已实现未测**，`DepthHead`/`MatNet` 是**组件**。
  最刺眼的一列是 `crates/trit-parts/tests/` 里**0 个文件**提到这六个类型 —— 所有断言都是同文件内联单点测试。
  表里附了可重跑的复算尺。
- 我自己这一轮的两个测量错误也写进 STATUS §3/§4：第一次 grep 漏算内联 `#[test]`（得出"全都没测试"的错结论，
  改正后才有了这张表）；以及"门"字当前只意味着**本地判红 + 变异钉住**，三个新门都还没进 CI（`.github` 被别的会话认领中）。
- 门第一次实跑就抓到 **8 条拦**：交接账本里**同一编号被两件事复用**（`A129` 既是"化学仓 trit-chem M0"
  又是"三值卷积栈 vs MLP"；`A130` 同理）⇒「A130 完成了」无法判定指哪件，这正是"我以为做完了"的最硬来源。
  已写进 `docs/STATUS.md` §3.5，修法与"取号即复验唯一"的纪律也记在那儿；去重是账本侧的活，不由本 PR 代做。
- 明确没做（§4 欠账）：**能力句的门还没写**——"必须带命令 + 读数"现在还是文字。设计已定：扫 README/docs 的能力句，
  缺命令或缺数字判红，存量进棘轮基线只许变少（同 `docs_hygiene` 的做法）。下一步先做它，再谈端到端断言补齐与 `MatNet` 改名。

### Added（`trit-bench`：把 8 份手写 xorshift 与不交错计时收成一份底件）

- 立项判据是清点实测（不是"感觉重复"）：仓内同型 `xorshift64` 手写副本 **8 份**；五处主要 bench 里
  **3 处完全不交错**（热漂移会把 32% 读成 13%）、**2 处只报单次或总均值**（无 min/median 概念），
  多数没有 `black_box`。最完整的是 `trit-ops/src/bench.rs`（交错 + min/median/max + 规模档），
  它的"轮内轮换起始位序"做法被抄进本 crate。
- 新 crate `crates/trit-bench`（base 层，纯 std、零依赖）：`SeedRng`（确定性三值序列，种子 0 防退化）、
  `Bench`（登记实现 → `run(rounds)` 交错 + 每轮轮换位序；`reps(n)` 在计时段内重复并取平均；
  `black_box` 兜住返回值）、`Spread`（min/median/max + `ns_per_unit`/`ps_per_unit`）。
- 复算尺迁过去并且**尺子没变**：`benchmarks/dot_variants` 改用 trit-bench 重跑 A357，读数与手写版同档 ——
  u64 派发 8.1–9.3 ps/trit（手写 7.6–12.2）、u64 便携 12.7–12.8（12.2–12.5）、
  u32 镜像 48.5–52.3（48.1–50.7）、i32 标量 170.8–246（168.6–183.5）。
- **量出一个真问题**：小档位（8192 trits）整个点积只有 1–2 个 tick（本机 `Instant` 分辨率约 100 ns），
  手写版报的 min=100.0 / med=200.0 ns 就是量化台阶本身；加 `reps` 后同一档报出
  min=67.5 / med=72.8 / max=82.8 ns，台阶消失 ⇒ 以后所有 bench 默认要在计时段内重复。
- 归位与文档：`layers.json` 登记 `trit-bench = base`，并把 base 的定义**明确拓宽**为
  「值域/代数/编码 + 不需要理解上层语义的公共底件（测量与夹具）」，`docs/LAYERS.md` §1 同步改（不是悄悄塞）；
  README 加一行；ROADMAP §2 的 `trit-bench` 划掉，下一步迁其余 4 份 bench。
- 测试 9 条（确定性、三值分布、种子 0 不退化、分位数口径、单位换算、轮内交错且轮换位序、
  `reps` 真按次数放大并平均、`reps(0)` 安全、标签对齐）。变异 5 条 **caught 5 / survived 0**。
- 三处被自己的门/现实逼出来的纠正，如实记：① 第一版 `seed_zero_does_not_degenerate` 断言的是"不全为 0"，
  而退化序列其实是**全 -1**（量化偏移）⇒ 变异存活，问题是**我的断言不判决**；改成"不许退化成常数序列"才抓住。
  ② `Bench` 初版用 `Box<dyn FnMut>` 要求 `'static`，逼调用方 clone ⇒ 改成带生命周期 `'a`，允许直接借用。
  ③ 本轮我先在旧分支上做，那片没有上一轮的文档与卫生门、CHANGELOG 还是被覆盖后的 29 字节，
  于是条目被一次没有 assert 的 `replace` 静默丢弃；改动整体搬到 main 之上重做，这次插入前先断言锚点与文件规模。

### Added（trit-wspace：跨片 oracle 表复用 + 三值任务向量）

- 分片 header 新增 `oracle_parent` + 增量 `oracles`：后续分片只写**新增**验证集，前面的直接复用；
  解析时把父表拼成有效表（记录仍用 `oracle_index`），检查点也携带有效表以便续放；
  `verify_shard` 核对父指针、父表前缀与增量摘要。
- `TaskVector`（`task.rs`）：`base → tuned` 的 trit/偏置差分（忽略 scale 标尺），带基准校验和；
  `merge_tasks` 按坐标对**目标 trit**投票：票和取符号、平票保留基准、全体投 0 取 0。
  CLI `task-merge <base> <tuned>...`；MCP `wsp_task_merge`（工具 15 → 16）。
- 读数：3 条共享验证集的日志切 2 片后，第二片 header `oracle_parent=0`、`oracles=[]`，
  文本里不再出现第二份验证集；检查点续放仍解析继承表。disjoint/consensus/tie/majority
  四类投票与基准校验均有断言。
- 测试 137 → 146 + 1 doctest；变异 111 → **123 条，123/123 caught**；test_surface 2,282 → 2,319（合并 main 新增测试后）。
- 门禁：fmt / clippy(-D warnings) / doc(-D warnings) / test / module_budget / publish_graph /
  hazard_gate / test_surface / layers.py / docs_hygiene / MSRV 1.88 全过。

### Added（trit-wspace：分片检查点与续放）

- `Checkpoint`：分片边界处的工件 + 下一个分片号/已消费条数/链头 + 应用/回滚/跳过/行为核对计数；
  `ShardReplay::checkpoint` 取点、`ShardReplay::resume` 续放；续放报告的计数与整段重放一致。
- CLI `audit-shards` 新增 `--checkpoint-every N --checkpoint-dir D`（写 `checkpoint-NNN.json`）
  与 `--resume <checkpoint>`（先流式验前缀链、比对条数/链头，再从检查点工件续放）。
- MCP `wsp_audit_shards` 接受 `checkpoint` 与 `checkpoint_every`，返回 `resumed` 与 `checkpoints`。
- 读数：3 片日志在首片后取点续放，报告逐项等于整段重放；检查点链头/条数/工件被改都会报错；
  CLI 2 片日志写 2 个检查点并从第 1 个续放到同一最终校验和。
- 测试 135 → 137 + 1 doctest；变异 102 → **111 条，111/111 caught**；test_surface 2,371 → 2,373（合并 main 移除 trit-vcs 后总表为 2,282）。
- 门禁：fmt / clippy(-D warnings) / doc(-D warnings) / test / module_budget / publish_graph /
  hazard_gate / test_surface / layers.py / docs_hygiene / MSRV 1.88 全过。

### Added（trit-wspace：程序组合与条件分支脚本）

- `Script`：把 `Program` 组合成 **原子序列**（默认，任一步失败/回滚则整段恢复原样；
  `.atomic(false)` 保留已成功步骤）或 **条件分支**（当前完整行为等于 `when` 时走 `then`，
  否则走可选 `otherwise`）。报告是树形 `ScriptReport`（`steps` / `matched` / `chosen` /
  `rolled_back` / `final_signature`），`failed()` 统一判定单步未应用、序列回滚、分支失败。
- 线格式 `type = step | sequence | when`；`behavior` 用完整逐样本输出表示（不是指纹），
  `wire::script_to_value` / `script_from_value` 往返一致。
- CLI `script <artifact> --script <json> --oracle <json> [--out]`；MCP `wsp_run_script`
  （工具 14 → 15）。
- 读数：break→bad-fix 原子序列失败后两步都回滚（网络逐位等于输入）；`atomic=false` 保留
  break；条件分支在打坏/未打坏网络上分别走 `then`/`otherwise`，无 `otherwise` 时不动。
- 测试 128 → 135 + 1 doctest；变异 95 → **102 条，102/102 caught**；test_surface 2,350 → 2,371。
- 门禁：fmt / clippy(-D warnings) / doc(-D warnings) / test / module_budget / publish_graph /
  hazard_gate / test_surface / layers.py / docs_hygiene / MSRV 1.88 全过。

### Added（trit-wspace：分片 manifest + MCP 分页按片发送）

- `ShardManifest`（`manifest.rs`）：记录每片的文件名、序号、起止、链头与**文本摘要**；
  `verify_texts` 核对数量/顺序/条数/链头/摘要。CLI `shard` 写 `manifest.json`，
  `audit-shards` 有清单时按清单读文件、逐片核对摘要，缺片/清单外分片/内容替换直接报错；
  无清单时退回按文件名排序。
- MCP `wsp_shard` 支持 `start` / `limit` 分页（结果带 `total` / `next` / `manifest`），
  可按片取回；`wsp_audit_shards` 接受 `manifest` 并先核对再验链/重放。
- 读数：2 片日志分页取回后重放与整日志一致；篡改清单摘要、删片、放额外分片、追加空白
  均被拒绝。测试 126 → 128 + 1 doctest；变异 87 → **95 条，95/95 caught**；
  test_surface 台账 2,348 → 2,350。
- 门禁：fmt / clippy(-D warnings) / doc(-D warnings) / test / module_budget / publish_graph /
  hazard_gate / test_surface / layers.py / docs_hygiene / MSRV 1.88 全过。

### Added（trit-wspace：分片流式校验与重放）

- `ShardCursor`：逐个 `push(text)` 解析并验链（序号、父链头、`count`、逐条 `chain`、本片
  `head`），读完即可丢弃分片；`entries` / `head` 随读随记。
- `ShardReplay`：读一片、校验一片、应用一片；内部用增量 `ReplayState`，不保留分片文本，
  `finish()` 给出与原日志一致的 `ReplayReport` 与最终工件。
- CLI `audit-shards` 改为流式逐文件读取，新增 `--out <path>` 直接落盘流式重放的最终工件。
- 读数：2 片/3 条日志流式重放与整日志重放报告逐项相同（applied 2 / skipped 1 / 校验和一致）；
  乱序分片、篡改父链头在游标处即被拒绝。
- 测试 124 → 126 + 1 doctest；变异 84 → **87 条，87/87 caught**；test_surface 台账 2,333 → 2,348。
- 门禁：fmt / clippy(-D warnings) / doc(-D warnings) / test / module_budget / publish_graph /
  hazard_gate / test_surface / layers.py / docs_hygiene / MSRV 1.88 全过。

### Added（trit-wspace：审计日志分片 + 分片内 oracle 表）

- `AuditLog::into_shards(max_entries)` → `ShardedLog`：长日志按条数切分，每片首行是
  `v=2 / kind=wspace-shard` 的 header（`shard` / `first_seq` / `count` / 父 `prev` / 本片
  `head` / `oracles` 表 / `oracles_digest`）。记录行保持 v1 链字段，验证集改成
  `oracle_index` 指向本片表项（同一验证集每片只存一次）。
- `ShardedLog::verify_chain` 做跨片校验：分片序号/序号起止连续、父链头与上一片 `head`
  衔接、每条 `chain` 可重算、表无重复/无未引用项/摘要匹配；`replay_net` 跨片重放并核对
  权重链与行为链。
- CLI `shard <log> --size N --out <dir>`（原子写入 `shard-NNN.jsonl`）、
  `audit-shards <dir> [--artifact <initial>]`；MCP `wsp_shard` / `wsp_audit_shards`
  （工具 12 → 14）。
- 读数：3 条共享同一验证集的日志切 2 片，每片 oracle 表 1 项；记录行不再内嵌 `oracle`；
  分片重放最终校验和与单文件日志一致。篡改 header `prev`/`head`/`count`、oracle 表内容、
  丢片/乱序、`oracle_index` 越界均报错。
- 测试 117 → 124 + 1 doctest；变异 75 → **84 条，84/84 caught**；test_surface 台账 2,323 → 2,333。
- 门禁：fmt / clippy(-D warnings) / doc(-D warnings) / test / module_budget / publish_graph /
  hazard_gate / test_surface / layers.py / docs_hygiene 全过。

### Added（trit-wspace：行内定位按活跃度选点）

- `localize_row` 的 `max_trits` 预算不再按固定步长抽样：先统计每个位置在样本上的
  **实际活跃次数**（权重为零、或该列输入恒为零的位置得 0 分；隐藏层用上一层激活，
  不是原始输入），按分数选点并仍按位置顺序探测；报告新增 `selection=adaptive` /
  `stride=0`，全量时 `selection=all`。
- 读数：权重 `[1,1]`、偏置 0 的两列网络在 `max_trits=2` 下，固定步长只看 `{col0, bias}`
  会漏掉被破坏的 col1；改为活跃度选点后探到 col1 并给出 `after_loss=0` 的修复。
  恒零输入列与隐藏层激活两条反例分别钉住打分输入。
- 测试 115 → 117 + 1 doctest；变异 71 → **75 条，75/75 caught**；test_surface 台账 2,321 → 2,323。
- 门禁：fmt / clippy(-D warnings) / doc(-D warnings) / test / module_budget / publish_graph /
  hazard_gate / test_surface / layers.py / docs_hygiene 全过。

### Added（trit-wspace：pilot 消融校准的选行）

- `probe_rows_adaptive` 升级为 **pilot 校准**：每个隐层先 pilot 一条静态分数最高的行，
  实测清零/取反的 `changed` 作为该层的校准权重；消融无效的 pilot 行直接丢弃，
  其余预算按“活跃优先 → 层权重 → 静态分数”填满；输出层行仍钉住。
- `ProbeReport` 新增 `pilot_rows` / `pilot_kept`（线格式同名）。
- 读数：构造“高活跃但下游无效”的行——pilot 把它丢掉，`budget=2` 选到真正承重的
  layer1 r0；`budget=3` 再补 layer0 r0；全零行不选。
- 测试 114 → 115 + 1 doctest；变异 67 → **71 条，71/71 caught**；test_surface 台账 2,320 → 2,321。
- 门禁：fmt / clippy(-D warnings) / doc(-D warnings) / test / module_budget(679, 0 超) /
  publish_graph / hazard_gate(0) / test_surface / layers.py / docs_hygiene 全过。

### Added（trit-wspace：行内 trit 定位）

- `localize_row`：在单行内逐位置（权重 + 偏置）试替代值，整验证集复核（`after_loss` /
  `gain` / `changed` / `fixed` / `broken`）；`max_trits` 超了按固定步长抽样并标 `sampled` / `stride`。
- `localize`：两级读取——自适应行探针选 top 行，再对每行做行内定位。
- CLI `localize <artifact> --oracle <json> [--rows K] [--row-budget N] [--max-trits N] [--top-trits K]`；
  MCP `wsp_localize`（工具 11 → 12）。
- 读数：打坏的 XOR row0 的 top trit `after_loss=0`；`[6,1]` 的 7 个位置在 `max_trits=3` 下
  `stride=3`、`probed=3`、`evaluated=6`；两级定位选到活跃 row0、排除全零 row1。
- 测试 109 → 114 + 1 doctest；变异 61 → **67 条，67/67 caught**；test_surface 台账 2,315 → 2,320。
- 门禁：fmt / clippy(-D warnings) / doc(-D warnings) / test / module_budget(679, 0 超) /
  publish_graph / hazard_gate(0) / test_surface / layers.py / docs_hygiene 全过。

### Added（trit-wspace：行级探针的自适应选行）

- `probe_rows_adaptive` / CLI `--adaptive` / MCP `probe.adaptive`：预算不足时不再固定步长，
  先用一次前向统计每行 pre-activation 的**活跃次数与裕量**，优先探“真正在工作”的行，
  并**钉住输出层行**；报告 `selection=adaptive`、`stride=0`，仍是明确标注的抽样。
- `Net::pre_activations`：返回每层每行的整数 pre-activation，供选行/后续读取使用。
- 读数：权重非零但在 oracle 上从不激活的行不会被选（专门构造的 activity 用例）；
  全零行被排除；`budget=1` 时只选输出行。
- 测试 103 → 109 + 1 doctest（新增 1 条 pre-activation、4 条自适应选行、1 条 MCP 自适应）；
  变异 56 → **61 条，61/61 caught**；test_surface 台账 2,309 → 2,315。
- 门禁：fmt / clippy(-D warnings) / doc(-D warnings) / test / module_budget(679, 0 超) /
  publish_graph / hazard_gate(0) / test_surface / layers.py / docs_hygiene 全过。

### Added（trit-wspace：行级消融探针 / 大网读取）

- `probe_rows`：逐行清零 / 取反，测整验证集上的行为漂移（`changed` / `fixed` / `broken` /
  `after_loss` / `gain`）。成本 `2×行数×样本`，与行宽无关；`budget>0` 且超预算时按固定步长
  均匀抽样，报告里明确标 `sampled` / `stride`，不做静默截断。
- `Oracle::sampled_behavior`：没有验证集时，用网络当前行为生成确定性随机探针集
  （同一 seed 同结果，`before_loss=0`），供大网读取使用。
- CLI `probe <artifact> --oracle <json> [--budget N] [--top K]`；MCP `wsp_probe`（工具 10 → 11）。
- 读数：XOR 输出行清零 `changed=4`、`after_loss=4`；`[2,5,4]` 的 9 行在 `budget=3` 下
  `stride=3`、`probed_rows=3`，选中 `(0,0)/(0,3)/(1,1)`。
- 测试 97 → 103 + 1 doctest（新增 5 条探针回放/预算/采样验证集 + 1 条 MCP 探针）；
  变异 51 → **56 条，56/56 caught**；test_surface 台账 2,278 → 2,284。
- 门禁：fmt / clippy(-D warnings) / doc(-D warnings) / test / module_budget(679, 0 超) /
  publish_graph / hazard_gate(0) / test_surface / layers.py / docs_hygiene 全过。

### Added（trit-wspace：单样本解释）

- `explain_sample`：先对全部候选只做**这一个样本**的前向，算 `flips` / `fixes`；
  按 `(fixes, flips, 坐标)` 取 top-K，再做整验证集复核，给出 `after_loss` / `gain`。
  `top=0` 表示全部复核（小网）。
- CLI `explain <artifact> --oracle <json> --sample N [--top K]`；MCP `wsp_explain`（工具 9 → 10）。
- 读数：打坏的 XOR 样本 0 的 top 候选 `fixes=true`、`after_loss=0`、`gain=2`；
  `[2,3,1]` 网 `top=4` 时 `evaluated=26`、`fully_evaluated=4`。
- 测试 92 → 97 + 1 doctest（新增 4 条解释不变量 + 1 条 MCP 解释）；变异 46 → **51 条，51/51 caught**；
  test_surface 台账 2,273 → 2,278。
- 门禁：fmt / clippy(-D warnings) / doc(-D warnings) / test / module_budget(679, 0 超) /
  publish_graph / hazard_gate(0) / test_surface / layers.py / docs_hygiene 全过。

### Added（trit-wspace：候选直用）

- `wire::patch_from_candidate` 接受 `{"edit": {...}}`（单点）或 `{"edits": [...]}`（组合），
  把 `wsp_propose` 的候选直接变成补丁；空候选、非法 op 都报错。
- CLI `apply-candidate <artifact> --candidate <json> ...`；MCP `wsp_apply_candidate`（工具 8 → 9）。
  两者都支持 `require_after`（失败原样回滚）；可选把本次编辑追加进 JSONL 审计日志
  （MCP 返回更新后的日志文本，不写盘；CLI 的 `--log` 写盘）。
- `patch` 与 `apply-candidate` 共用同一条执行路径（`perform_patch`），避免两条入口判决分叉。
- 测试 88 → 92 + 1 doctest（新增 1 条候选线格式、3 条 MCP 直用/回滚/日志）；
  变异 42 → **46 条，46/46 caught**；test_surface 台账 2,269 → 2,273。
- 门禁：fmt / clippy(-D warnings) / doc(-D warnings) / test / module_budget(679, 0 超) /
  publish_graph / hazard_gate(0) / test_surface / layers.py / docs_hygiene 全过。

### Added（trit-wspace：三编辑批量提议）

- `analyze_triples`：枚举全部**不同坐标**的三点编辑组合，实测 `after_loss` / `fixed` /
  `broken` / `changed` 并按 `gain` 排序；组合数超过 `max_triples` 报 `CombinationBudget`。
- CLI `propose --depth 1|2|3 [--max-triples N]`（`--pairs` 仍是 `depth=2` 的别名）；
  MCP `wsp_propose` 增加 `depth` / `max_triples`，返回 `depth=1|2|3`。
- 读数：一个真实三点破坏（layer0 三个 trit 置 0）单点与配对都修不到 0，三组合 top 修到 0
  且可回放；`[2,2,1]` 评估 672 组、`[2,3,1]` 评估 2,288 组（均排除同坐标组合）。
- 测试 83 → 88 + 1 doctest（新增 4 条三组合不变量/预算 + 1 条 MCP depth=3 回放）；
  变异 37 → **42 条，42/42 caught**；test_surface 台账 2,264 → 2,269。
- 门禁：fmt / clippy(-D warnings) / doc(-D warnings) / test / module_budget(679, 0 超) /
  publish_graph / hazard_gate(0) / test_surface / layers.py / docs_hygiene 全过。

### Added（trit-wspace：验证集内容寻址）

- 同一验证集只在日志第一条内嵌，后续条目写 `oracle_ref`（向前指向首条序号）；
  解析时解析成完整 oracle 供重放核对，序列化仍写回 ref。不同验证集不去重。
- `oracle_ref` 越界 / 自指 / 与内嵌 `oracle` 同时出现都直接判错；指向没有验证集的条目也判错。
- 测试 80 → 83 + 1 doctest（新增 3 条去重/错引用不变量）；变异 34 → **37 条，37/37 caught**；
  test_surface 台账 2,260 → 2,264。
- 门禁：fmt / clippy(-D warnings) / doc(-D warnings) / test / module_budget(679, 0 超) /
  publish_graph / hazard_gate(0) / test_surface / layers.py / docs_hygiene 全过。

### Added（trit-wspace：两编辑批量提议）

- `analyze_pairs`：枚举全部**不同坐标**的点编辑对，实测 `after_loss` / `fixed` / `broken` /
  `changed` 并按 `gain` 排序；候选对数超过 `max_pairs` 直接报 `PairBudget`，不静默截断。
- CLI `propose --pairs [--max-pairs N]`；MCP `wsp_propose` 增加 `pairs` / `max_pairs`；
  返回里 `depth=1|2` 区分单点与配对。
- 读数：两处 trit 破坏后单点提议全部 `after_loss>0`，配对 top 修到 0 并可回放；
  `[2,3,1]` 的 26 个候选给出 312 对（325 对减去 13 对同坐标）。
- 测试 80 + 1 doctest（新增 5 条配对不变量/预算/同坐标过滤）；变异 29 → **34 条，34/34 caught**；
  test_surface 台账 2,254 → 2,260。
- 门禁：fmt / clippy(-D warnings) / doc(-D warnings) / test / module_budget(679, 0 超) /
  publish_graph / hazard_gate(0) / test_surface / layers.py 全过。

### Added（trit-wspace：审计日志的行为链）

- `record_with_oracle`：记录时存下验证集规范 + 前后**行为指纹**；`replay_net` 同时核对
  权重链（`before`/`after` 校验和）与行为链（`behavior_before`/`behavior_after`），
  报告新增 `behavior_checked`（实际核对条数）。
- 防日志爆掉：单条记录最多存 4,096 条验证集，超过直接报错。
- `recompute_chain`：日志合并/手工修订后重算整条链（不验证已有内容）；
  行为指纹被改但链重算过时，`replay` 仍会按行为链报错。
- CLI：`patch --log --oracle` 记行为链；`audit` / `replay` 输出 `behavior_checked`。
- 测试 74 + 1 doctest（新增 3 条行为链不变量/超限/错配）；变异 24 → **29 条，29/29 caught**；
  test_surface 台账 2,251 → 2,254。
- 门禁：fmt / clippy(-D warnings) / doc(-D warnings) / test / module_budget(679, 0 超) /
  publish_graph / hazard_gate(0) / test_surface / layers.py 全过。

### Added（trit-wspace：可重放的编辑审计链）

- 新模块 `audit`：JSONL + FNV 哈希链。每条记录 `before` / `after` 校验和、补丁、改动量、
  `applied` / `rolled_back`、`prev` / `chain`；`verify_chain` 抓篡改与乱序，
  `replay` / `replay_net` 从初始工件逐条重放并核对每一步校验和。
- 不变量：回滚条目与未应用条目必须前后同校验和（工件不会被留在半改状态），
  `record` 直接拒绝不自洽的记录。
- CLI：`patch --log`、`audit <log> [--artifact]`、`replay <log> --artifact --out`；
  MCP 新增 `wsp_audit`（工具 7 → 8）。
- 测试 71 + 1 doctest（新增 7 条审计链不变量、1 条 MCP 审计回放、CLI 追加日志/验链/重放/篡改）；
  变异 18 → **24 条，24/24 caught**；test_surface 台账 2,233 → 2,251。
- 门禁：fmt / clippy(-D warnings) / doc(-D warnings) / test / module_budget / publish_graph /
  hazard_gate / test_surface / layers.py 全过。
- 本机 release 二进制与 MCP 服务已按新工具清单重装（8 工具）。

### Added（trit-wspace：单点编辑提议器）

- 新模块 `analyze`：枚举全部点编辑（改一个 trit / 偏置），对每条**实测**
  `after_loss` / `fixed` / `broken` / `changed`，按 `gain` 降序、失配数升序、坐标字典序排序。
  候选里的数字不是打分估计，回放补丁必须逐项相等。
- CLI 新增 `propose`；MCP 新增 `wsp_propose`（工具数 6 → 7）。
- 读数：2-2-1 XOR 被单 trit 打坏后，top 候选 `after_loss=0`；
  单层 2→1 网络没有任何单编辑能把 XOR 修到 0，但常量输出能把 4 条失配降到 2 条。
- 测试 59 → **64 + 1 doctest**（新增 4 提议器不变量、1 MCP 提议回放）；
  变异 13 → **18 条，18/18 caught**；test_surface 台账 2,228 → 2,233。
- 门禁：fmt / clippy(-D warnings) / doc(-D warnings) / test / module_budget(672 文件 0 超) /
  publish_graph / hazard_gate(0 findings) / test_surface 全过。
- 本机 release 二进制已重装（`~/.qoder/bin/trit-wspace.exe`），MCP 工具清单已含 `wsp_propose`。

### Added（trit-wspace：模型/脚本工具面）

- 新二进制 `trit-wspace`：`read / verify / patch / repair / diff / merge / mcp` 七个子命令；
  输入 `-` 读 stdin，报告一律 JSON，`--out` 用“临时文件 + 改名”落盘，避免半写。
- 补丁线格式 `version=1`（`set_trit / set_bias / set_scale / swap_rows / negate_row`），
  模型可直接生成、人工可逐条审；验证集支持显式 `inputs/outputs` 与紧凑 `domain/vars/table`
  （boolean 用 `0/1/x`，ternary 用 `-0+`，字典序）。
- MCP stdio 六个工具：`wsp_read`、`wsp_verify`、`wsp_patch`、`wsp_repair`、`wsp_diff`、`wsp_merge`。
  无状态（每次从 WSP1 文本开始）；错误返回 `isError=true`；工具清单 2,381 B / 657 tokens（本机粗估）。
- `ops` 统一 CLI 与 MCP 的判决路径：补丁仍走事务式 `Patch`/`Program`，修补仍只返回精确解。
- 测试 38 → **59 + 1 doctest**（新增 3 CLI 端到端、11 MCP、7 线格式）；
  变异 7 → **13 条，13/13 caught**；test_surface 台账 2,207 → 2,228。
- 门禁：fmt / clippy(-D warnings) / doc(-D warnings) / test / module_budget(670 文件 0 超) /
  publish_graph / hazard_gate(0 findings) / test_surface 全过。
- 本机已把 release 二进制装到 `~/.qoder/bin/trit-wspace.exe` 并注册为 MCP 服务
  （`trit-wspace mcp`，注册 key `trit-wspace`）；工具目录需重启/重载 App 后出现。

### Added（trit-wspace：三值权重空间编程）

- 新 crate `crates/trit-wspace`（实验层，`publish = false`）：把权重当源码的
  **读 / 写 / 验证** 闭环，不做梯度、不改 f32 latent。
- 表示：`Net`/`Layer`（三值权重 + 三值偏置 + 每行 f32 scale），WSP1 规范文本
  （每 trit 一个字符、一行一个神经元、FNV-1a 校验和、解析时允许注释但不进规范形式）；
  `codec_round_trip_is_exact_and_canonical` 钉住逐位往返，改任一 trit 必报校验和错。
- 编辑：`Edit`（改 trit/偏置/scale、换行、整行取反）+ `Patch`（先整体校验再应用）
  + `Journal`（逆序逐位撤销）；`Program` 带前置/后置行为条件，后置失败原样回滚。
- 搜索：`repair` 无梯度求精确补丁。状态空间 `3^权重数 ≤ 20 万` 时按编辑数逐层 **BFS**，
  首次命中的层数就是最小编辑数；超过后走半径 1/2 穷举 + 按 `(失配数, 裕量能量)` 排序的束搜索。
  返回 `None` 而不是“差不多对”。
- 合并：`align` 用贪心行匹配消掉隐藏单元置换对称性，`merge_vote` 做逐点符号投票；
  `merge_aligned` 对“同一模型换了隐藏单元编号”能精确还原。
- 关键负结果：从零网络到 2-2-1 XOR 的汉明距离恰好是 **9**（全部 9 个参数都非零，
  穷举可证），束搜索在 loss 平台上会丢路；改成小状态空间完备 BFS 后才稳定命中。
  这也说明 WSP 的真正优势是“改已有工件”，不是从零训练。
- 读数：测试 **37 条 + 1 doctest**（8 单元 + 21 不变量 + 8 编程语义）；
  2-2-1（带偏置）**穷举 19,683 个配置覆盖全部 16 个二元布尔函数**；
  任意单 trit 破坏都能被**一条编辑**修回；两处破坏在两条内修回。
- 变异：`scripts/mutants/trit-wspace.json` 登记 7 条，
  `mutation_check.py --pkg trit-wspace` **caught 7 / survived 0**（0 崩溃红、0 编译红）。
- 门禁：`cargo fmt --all --check`、`cargo clippy -p trit-wspace --all-targets -D warnings`、
  `cargo test -p trit-wspace`、`module_budget`（662 文件 / 0 超预算）、`publish_graph`、
  `hazard_gate`（0 findings）、`test_surface`（2207 条 / 新增单位已登记）全过。
- 边界（诚实缺口）：只做符号激活前馈网络；验证是有界穷举；大规模网络没有完备搜索，
  也没有“自然语言 → 补丁”的模型；外部 WSP 报道（2026-11-02 MIT CSAIL，Astra 读写
  Qwen3.5-0.8B 原始权重）**未复现**，只借用其接口形态。

### Added（漂移门：校准读数对账，变坏即红）

- `trit-net::netsec::drift`：基线校准读数（`docs/SECURITY-NET-BASELINE.json`，
  Friday-16-02 200k 行、与 M1 §读数逐位一致）与当前读数按 band 逐条对账
  ΔFPR/ΔTPR/Δ弃权，**FPR 升 >0.1 / TPR 降 >0.1 / 弃权升 >0.2 任一即红**
  （阈值可调）；**fail-closed**：缺 band、多 band、坏 JSON、非有限读数、
  负/NaN 阈值一律红——对不上账的「绿」没有含义。
- `trit-net drift <基线.json> <当前.json> [--max-*-delta F]`：JSON 判决输出，
  红则退出码非零。实测：基线自比绿；M1.6 变体对 M1 基线红 rc=1
  （读数变了必须显式重生成基线并在提交说明里给理由）。
- 边界（🔴）：只对账不归因——漂移原因（分布漂移/攻击类型/代码回归）是人的
  工作；CI 接入取决于数据集可得性（335 MB 不入库），流程见
  `docs/SECURITY-NET.md` §M1.6 §漂移门。
- 测试 `tests/drift.rs` 10 条（绿路径/三方向触红/缺 band/多 band/坏 JSON/
  非法阈值/多理由累积）；变异门 +2（FPR 方向写反、结构性问题被忽略）。

### Changed（M1.6 训练器与阈值：mini-batch + 良性尾部，真实读数）

- `trit-net::train`：`TrainOptions.batch_size`——`0/1` = 全批（旧行为）；`k>1` =
  每轮按 `seed ^ epoch` **确定性洗牌**（Fisher–Yates，同参重跑逐位一致），每
  `k` 条累积一次梯度更新。CLI `calibrate --batch N`。
- `trit-net::netsec::calibrate`：`Config.benign_tail: Option<f64>`（CLI
  `--benign-tail F`）——分位数锚在**良性子集**（`Bias::High` 取
  `[良性 P(0.5-b), 良性 P(1-tail)]`，Low 镜像），攻击占比高时不再被攻击分布
  把死区拖走。
- 真实读数（Friday-16-02 DDoS 日，200k 行，基线与 M1 逐位一致）：**mini-batch
  是主收益**——band 0.3 段 TPR 0.9378→**0.9987**（同弃权档），band 0 的 FPR
  0.1446→**0.1101**、精确率 0.8313→0.8645；**良性尾部单独用是负结果**（DDoS 日
  良性分布窄，FPR 全面变差），与 mini-batch 组合才成立（TPR 0.998 档 FPR
  0.156）。跨日依旧失效（漂移负结果成立，照实记录）。详见
  `docs/SECURITY-NET.md` §M1.6。
- 测试 +3（mini-batch 收敛/确定性/超大 batch 钳制 + 洗牌质量断言；良性尾部
  分位数锚定纯函数测试 + 死区恢复机制测试）；变异门 +2、修 1（梯度上升锚点
  随 batch_len 重命名）。

### Added（M3→M2 登记桥：probe 证据半自动进攻击图）

- `trit-net::netsec::probe::attach_probe`：probe 报告 → 攻击图——station 节点（`T`）
  → 主机节点（label = 字面 IP，证据 = **主机应答**：任一探测拿到 `T`/`F` 即在网，
  refused 也是主机回的 RST；全 `X` 才是 `X`）→ 服务节点（`ip:port`，证据 =
  finding 证据，banner 线索跟到 note），边出处 = `tcp-connect`。
  **分工**：桥只登记探测证据；攻击语义由人随后用 LWW 改写。**幂等**：同报告
  重跑图不变，新报告 LWW 刷新。
- `attack::AttackGraph::add_node` 升级为 **LWW**：同 label 重复登记 = 证据与备注
  后写覆盖（与边一致）——否则桥重跑时节点证据会和最新探测结果打架。
- `findings_from_jsonl`：探测报告 JSONL 复原，坏行显式报错不静默丢。
- `trit-net bridge <攻击图.jsonl> <探测报告.jsonl> [--station name]`：更新后的图
  JSONL 走 stdout。
- 测试 +4（结构/证据与 reach 贯通含见证、全 X 主机、幂等 + LWW 刷新 + 人工改写、
  findings JSONL 往返与坏行拒绝）；变异门 +2（主机应答 any→all、服务节点证据
  被升成 T）。
### Fixed（trit-re：N4 加固轮——扫描预算 + 确定性模糊测试，对抗性输入不挂起）

- **审查发现一个真风险**：hex 跳跃模式的回溯在对抗数据上指数爆炸——
  `{ 90 [0-4096] 00 [0-4096] 00 [0-4096] 00 }` 配全 0x90 数据 ≈ 4096³ 条
  回溯路径 = **挂起**（违反"任何输入不 panic 不挂起"不变量）。
- `SCAN_BUDGET`(1M 步/模式)：段尝试 + 首段候选比较合计封顶，耗尽即停止
  搜索。**耗尽的语义 = 扫描不完整 → 规则裁决 `Unknown` 而非 `False`**——
  "未命中只在完整扫描后给出"的组成面（预算耗尽的沉默不是清白；YARA 用
  时间超时，我们用确定性预算——同样的 fail-closed，可复算）。
- `tests/fuzz_sigscan.rs` 新增（确定性模糊，xorshift64 固定种子可复现）：
  随机文本 4000 次 + 真实规则结构变异（删/插/替）3000 次 + 随机 hex spec
  4000 次 + 随机数据×随机模式×截断 800 次 + **对抗性跳跃链 40 次**（无预算
  版本会跑数分钟）——五路夹击 `yaraparse`/`Pattern::hex`/`SigScan`，
  不变量：不 panic、不挂起、命中数封顶纪律。
- 测试 144 全绿（+7：2 预算语义 + 5 模糊）、clippy/fmt/module_budget/
  变异门 31/secret/hazard 全过；sigscan.rs 681 行 ≤ 800。

### Fixed（trit-re：N4 对拍揪出的词法 bug——`.` 不在断字集）

- `yaraparse` 词法器的 word 分支没把 `.` 列入断字集：`in (4..8)` 的 `4..8`
  被整体吞成一个 Word，`parse_int` 直接挂。修复：`.` 加入断字集（`pe.*`
  之类模块路径本就不支持，断开与否只影响报错形状），区间分隔符解析简化为
  「Sym('.')×2 与 Word("..") 两种形态都认」。**真对拍揪出来的**——合成批次
  之前 137 测试全绿也没覆盖到 `4..8` 这种连写形态。

### Added（trit-re：N4 对拍跑通——官方 YARA 引擎真对拍全绿，验收判据达成）

- **环境通路选 uv（最轻一条）**：`uv python install 3.13`（python-build-standalone
  预编译，免 MSVC）+ `uv venv` + `uv pip install yara-python`（cp313 预编译
  轮子 4.5.4）——15ms 装完，绕开 py3.14 无轮子 + 无 MSVC 的双重卡点。
- **真对拍结果**：合成对抗批次 6 样本 × 9 规则（官方 YARA 4.5.4 vs
  trit-re 子集实现）→ **MATCH 6 / DIFF 0 / SUBSET-GAP 4 / OFFICIAL-ERR 0**。
  覆盖：wide 文本、`at 0`、`in (4..8)`、`2 of them`、`all of ($pa*)`、
  `not $ref`、跳跃窗口、正则挂账、filesize 挂账——可评估规则**命中集合
  100% 一致**，不可评估规则全部正确落入 SUBSET-GAP 类（子集边界，不算失败）。
  N4 验收判据「与官方 YARA 在同一批样本上的命中集合一致」在子集范围内达成。
- **吞吐读数**（naive 逐字节扫描的量级，优化非目标）：34 MB 放大样本 ×
  3 规则 **0.80 s ≈ 42.6 MB/s**（`sigscan` CLI 实测；356 KB notepad 单扫
  0.48 s 含进程启动）。
- `trit-re sigscan <样本> --rules <规则.yar>` CLI 子命令：逐规则一行
  `HIT/MISS/UNKNOWN\t<name>`（对拍口径：HIT 行即命中集合）+ 不支持的规则
  另出 `UNSUPPORTED\t<name>\t<原因>`——绝不静默；stderr 一行人读摘要。
- `scripts/yara_diff.py`：**对拍 harness**——yara-python（官方引擎）vs
  trit-re sigscan 逐样本对照命中集合，四态分类：MATCH / DIFF（真差异，退出
  码 1）/ SUBSET-GAP（trit-re 未评估的子集边界规则，单独计数不算失败）/
  OFFICIAL-ERR（官方引擎侧错误）；缺 yara-python 时优雅降级（安装提示 +
  退出码 2）。
- 🔴 **环境受阻的原始记录（2026-11-05 上午）**：本机 Python 3.14 无
  yara-python 预编译轮子，源码构建需要 MSVC（未安装）；`py -0p` 列出的
  3.12/3.11 是残留注册（python.exe 已不存在）——后由 uv 通路解决（见上）。
- 测试 137 全绿、clippy/fmt/module_budget/变异门 31/secret/hazard 全过；
  对拍抓出并修复词法 bug（见上 Fixed）。

### Added（trit-re：N4 第二层——YARA 规则文件解析器）

- `src/yaraparse.rs` 新增：`.yar` 文本 → `sigscan::Rule`。词法器引号感知
  （字符串里的 `//` 是内容不是注释；`//` 行注释 / `/* */` 块注释 / 转义）、
  meta 计数、`import`/`include` 入 notes（模块语义不支持，如实记录）、
  字符串修饰符（nocase/wide/ascii——先收值再收修饰符，一次构造模式）、
  hex 块（含通配/跳跃，复用 `Pattern::hex` 的全部语法检查）。
- 条件递归下降：`and/or/not`（优先级 or < and < not/primary）、括号、
  `$id`、`$id at N`、`$id in (a..b)`、`any/all/N of them`、`any/all/N of
  ($x*)`（前缀集）；整数支持十进制与 `0x` 十六进制。
- **fail-closed 是本层的主语义**：解析不了的规则 → `RuleOutcome::Unsupported`
  带原因 → 扫描时按 `Unknown` 裁决，**绝不静默跳过**（评估不了的规则报
  "没命中"就是说谎）。正则串 / `filesize` / `pe.*` / 未知 `$ref` / 条件尾
  未消化 token 全部走这条路，集成测试逐类钉死。
- `sigscan::Condition` 升级为表达式树（新增 Ref/And/Or/Not/OfAny/OfAll/
  OfCount，`is_none_or` 前缀匹配）；`Rule` 补 `patterns()`/`condition_ref()`
  访问器与 `pub(crate) from_parts` 装配器。
- 测试 +9（`tests/yara.rs` 8 条集成 + 模块烟测）：全字段规则、正则挂账、
  filesize 挂账、未知引用挂账、**Unsupported 规则扫描时 Unknown 绝不 False**、
  表达式树经 SigScan 端到端三值判定、结构错误（未闭合/缺名/注释未闭合）、
  hex 块跳跃；行数预算：测试迁 `tests/yara.rs` 后 yaraparse.rs 689 ≤ 800。
- 🔴 剩余半只如实挂账：与官方 YARA 的命中集合对拍（等真实样本批次与 yara
  就位，N1 口径）。

### Added（trit-re：N4 第一层——sigscan 三值签名匹配）

- `src/sigscan.rs` 新增：YARA 子集的三值匹配器。hex 串（`??` 通配、
  `[n]`/`[n-m]` 跳跃，`[2-4]` 与 `[2 - 4]` 两种写法都认，**跳跃后必须有
  字节段**——以跳跃开头/结尾一律拒绝）、文本串（ascii / nocase=ASCII 折叠 /
  wide=仅 UTF-16LE，对齐 YARA `wide` 语义）、声明式条件
  （All/Any/Count/At/In）。
- **三值纪律与 N4 验收判据逐字对应**：命中 `True`（不依赖输入完整性）、
  完整扫描且未命中 `False`、**输入截断 `Unknown`**——"未命中"只在完整扫描
  后给出（fail-closed：截断的沉默不是清白）。整体裁决 = 各规则 Kleene OR
  （报警语义：任一命中即报警，`False ∨ Unknown = Unknown`）。
- 护栏：每模式命中 ≤ `MAX_HITS`(1024)、跳跃跨度 ≤ `MAX_JUMP`(0x1000)、
  病态模式在收割侧截断不挂起。
- 测试 +8：hex 字面量/通配/跳跃窗口（边界内外）、语法错误 fail-closed
  （未闭合/区间反/以跳跃开头或结尾/非法字节）、nocase/wide、五条件、
  **截断判据本体**（截断未命中 Unknown、截断命中 True、完整未命中 False）、
  Kleene OR 语义、命中封顶。
- 🔴 如实挂账：YARA 规则文件解析器 + 与官方 YARA 命中集合对拍（等真实样本
  批次与 yara 就位，N1 口径）；当前规则经 Rust API 构造。

### Added（trit-re：N2 v4——数据节指针表收割，漏检类清零）

- `functions::pe_pointer_seeds` 新增：**非可执行节里指向可执行域的指针串**
  （vtable/回调表）→ 候选函数起点。全机械过滤：PE32+ 按 u64 / PE32 按 u32
  对齐步进；值 − `image_base` 必须落在可执行域内（NULL 槽位跳过、非可执行
  域值归零连击）；**连续 ≥2 个**指针（表的形状）全收，孤立指针仅 16 字节
  对齐时收（x64 MSVC 函数 16 对齐；不对齐孤立值多半是整数巧合——宁可丢也
  不给假起点）。ELF 不在此列：PIE 下数据节指针要先过重定位，诚实留白。
- `cfg_ranges` 加 `pointer_seeds` 参数：种子只当起点、不进块遍历（零覆盖
  污染风险）；`build_pe` 精确路径接入（note 报告种子数）。
- **验收实测**：trit-re.exe 未归属 15→**12**（**疑边界漏检 1→0 清零**，
  函数 4324→**4591** = +267 个 vtable/回调起点，归属 2662→2665）；
  notepad.exe 未归属 2 不变（漏检 1 不是指针表可解——不是每条漏检都能机械
  找回，如实保留；函数 458→510）；`fn_*` 判定全表不变。测试 118 全绿（+2：
  收割过滤规则单元 + 端到端 vtable 起点）、clippy/fmt/module_budget/变异门
  31/secret/hazard 全过。

### Added（trit-re：N2 v3——取址分析，间接调用叶子的起点信号）

- `x86len::decode` 新增 `addr_taken`：`lea r64,[rip+disp32]`（mod=00 rm=101）
  的落点——**地址被取 = 回调/虚表项，间接调用函数的起点信号**；`mov` 加载、
  非 rip 相对形态、控制流指令恒 `None`。
- `cfg::walk` 把取址目标**当新种子继续走**（不动点：新函数体里再发现取址/
  调用边继续扩），起点合成加第三路：遍历确认调用目标 ∪ 前向无条件跳 ∪
  **取址目标** ∪ 未覆盖层一候选回落。
- **验收实测**：trit-re.exe 未归属 16→**15**（+1 归属，函数 4259→**4324**
  = +65 个取址发现的间接入口）；notepad.exe 不变（残差漏检 1 是数据节表项
  ——vtable 在 `.rdata`，代码内 `lea` 够不着，数据节指针表解析是 v4 范畴）；
  `fn_*` 判定全表不变。测试 116 全绿（+1 取址单元 + +1 端到端）、clippy/fmt/
  module_budget/变异门 31/secret/hazard 全过。

### Added（trit-re：N2 v2——x86-64 最小长度解码器 + 块遍历验证层）

- `functions/x86len.rs` 新增：**最小 x86-64 指令长度解码器**（fail-closed——
  解不出返回 `None` 绝不猜）。覆盖：legacy/REX 前缀（REX.W 压过 `0x66` 的
  立即数宽度）、ModRM/SIB/disp/imm 全寻址形态、主流整数指令、常用 SSE
  （两字节 opcode 默认 ModRM）、全部控制流；15 字节上限 + 缓冲区装不下即停走。
  不支持面如实列出：VEX/EVEX（`C4`/`C5`/`62`）、三字节逃逸（`0F 38`/`0F 3A`）、
  64 位非法的 32 位时代指令（`60`/`61`/`D4`/`D5`/`27`…/`0F A0`/`0F A1`/
  `0F A8`/`0F A9`）、moffs（`A0`-`A3`）——编译器产物里几乎只出现在**数据**
  里，当停走信号恰好正确。`decode(bytes, base, addr) -> (len, kind, target)`
  一次给出长度 + 控制流分类（Call/Jump/JumpCond/Ret/Other）+ 直接边目标。
- `functions/cfg.rs` 接**块遍历验证层**（v2）：递归下降只从**可信种子**
  （入口 + 精确区间起点）起走——从字节候选起走会拿数据当代码、覆盖面撒谎；
  解码失败停链、访问数触顶（200k）停走、未覆盖区域回落层一（fail-closed
  到 v1 行为，绝不静默缩覆盖）。起点合成 = 遍历确认的调用目标 ∪ 前向无条件
  跳目标（尾调用/跳板——遍历归类，比字节猜测准）∪ 未被遍历覆盖的层一候选。
- **验收读数（与 v1 逐字一致，如实记录）**：notepad.exe 未归属 2（漏检 1）、
  trit-re.exe 未归属 16（漏检 1）——遍历验证没有改变归属结果（可达调用目标
  层一字节扫描本来就能看到）；v2 的增量是**语义**（真边验证 + 尾调用归类）
  与**地基**（解码器/遍历器），剩余漏检 = 间接调用的叶子（无静态 E8 源），
  根治在间接调用解析而非块遍历。测试 114 全绿（+8 解码器）、clippy/fmt/
  module_budget/变异门 31 全过。
- 过程披露：`decode` 首版把 target 算在节内偏移空间、与调用方的绝对地址空间
  错位（测试当场判红）；入口被「已覆盖」判定吞掉（测试当场判红）——两处都靠
  层一测试网兜住后修正。

### Added（trit-re：N2 最小 CFG 第一层——CFG 补隙合并）

- `functions/cfg.rs` 新增（N2 路线图第一层）：直接控制流边收割（x86 `E8`/`E9`、
  ARM64 `BL`/`B`，字节模式级）+ 入口/目标定起点 + **补隙合并**——有精确边界源
  （`.pdata`/COFF 符号/STT_FUNC/CFI）时只填充精确区间未覆盖的可执行隙，
  **绝不切分精确区间**（往函数体中间插假起点会劈碎同函数共现——`fn_*` 回归的
  头号来源）；无精确源时覆盖全部可执行节（旧 `heuristic_ranges` 的替换形态，
  起点收割语义逐字节一致）。`FuncSource::Heuristic` 更名 `Cfg`（`"cfg"`），
  `exact` 恒为 false——共现判定照旧要求精确源，剥表 PE32 的共现在此**保持诚实**
  （换个名字不改变证据等级；指令级块遍历留 v2，前提是最小长度解码器）。
- `build_pe`/`build_elf` 精确路径接入补隙（归一化精确区间 → CFG 填隙 → 再归一化）。
- **验收测量（N2 判据，真实样本）**：notepad.exe 未归属 8→**2**（疑边界漏检
  5→**1**，归属调用点 1719→1725）；trit-re.exe（自身）未归属 22→**16**（漏检
  2→**1**，归属 2621→2661）；`fn_*` 判定行与基线构建**逐字一致**（含
  `fn_injection`/`fn_stager`/`staged_loader` 全表对照），测试 106 全绿。
- 测试 +5（`functions/cfg.rs`：无精确源=替换形态平铺、精确区间绝不切分、
  隙内补起点、入口在隙内/在精确区间内两分支、ARM64 BL 目标定起点）；
  `tests/pe.rs` COFF 夹具断言更新（total 2→3：隙里 E8 形字节产 Cfg 候选——
  正是补隙语义，精确区间原样）。
- 变异门 31 条全抓住（本片未新增判决逻辑，登记表不变）。
### Added（仓库边界文档 + 文档卫生门）

- 新 `docs/SCOPE.md`：**主库存什么、不存什么**的一次定案 —— 收进来的四条判据（R1 与具体项目无关、
  R2 有独立判据、R3 可归位、R4 能长期守）、不收的六类（生产链接入、治理工具、一次性笔记、
  不可再生大素材、别的语言生态成品、私有环境与凭据，逐类给真实例子与去处）、
  **数据与二进制政策**（是否判据必需 / 可否再生 / 是否超 1 MiB–8 MiB 三问，超了走"内容级 SHA-256 清单 + 取数脚本 + 最小切片"）、
  构建产物与机器状态（文档只记读数与口径，不记本机路径）、跨仓分工表、进门流程、
  以及**「规矩 ↔ 门」对照表**（哪条机器判、哪条只能人判，人判的要写清为什么门判不了）。
  `README.md`、`docs/LAYERS.md`、`docs/ROADMAP.md` 各挂一处指针，不复制正文。
- 新门 `scripts/docs_hygiene.py` + 棘轮台账 `scripts/docs_hygiene_baseline.json`：
  ① `CHANGELOG.md` 首行必须是 `# Changelog`、必须有 `## [Unreleased]`、字节数不得低于基线；
  ② 仓内可见文档（CHANGELOG / README / docs/**.md）的**盘符绝对路径**按文件计数，只许变少；
  ③ 基线读不出 = 判红（"不知道有多少"不等于"没有"）。
  存量如实登记为债务：7 个文件共 45 处（`GENERAL-PLAN.md` 24、`VISUAL-PLAN.md` 6、`GUARD.md` 5 等），
  我没批量改别人的历史记录，只保证不再变多。
- 自证：`--self-test` 2 正例 + 6 反证绿；活体反证两条都判红（在 `docs/SCOPE.md` 里塞一行指向临时目录的绝对路径 ⇒ rc=1；把 CHANGELOG 截成 tiny ⇒ rc=1），恢复后 rc=0。
  变异 `scripts__docs_hygiene.py.json` 4 条 **caught 4 / survived 0**（python runner 属"未点名/崩溃类"，
  与 `scripts/layers.py` 同一披露）。

### Fixed（CHANGELOG.md 曾被脚本覆盖成一行人机路径；条目按提交信息重建）

- 事实：`CHANGELOG.md` 在某次脚本化追加时把**路径字符串当成内容写回**，文件从 173 760 字节变成 29 字节，
  内容是本机绝对路径；该破坏随 `ba081a3` 提交，并已通过 PR #289 的 squash 进入 main。
  现在从最后一个好版本（`6988d9c`）恢复，并按分支提交信息补回丢失条目。
- 根因与防再犯：脚本里同一个名字先当路径再当内容用，写回时写错了对象。
  ① 这类追加改用 Edit 逐段改，不再整文件写回；② 新门 `scripts/docs_hygiene.py` 判三件事——
  `CHANGELOG.md` 必须以 `# Changelog` 开头且含 `## [Unreleased]`、字节数不得低于棘轮基线、
  仓内文档不得新增**盘符绝对路径**（存量按目录计数进基线，只许变少）。三条都自带反证与变异。
- 借这次恢复一并补回的条目（细节在各次提交信息里）：
  · **能力登记进门**（`layers.json` 的 `capabilities`）：一个能力一个 `authoritative`，第二实现必须带
    含数字的 `measured_gap` + `why_kept` + `bench`；缺字段、写"慢很多"、缺理由、同能力两条、权威不在归位表 ⇒ 判红。
    自测 2 正例 + 11 反证，`scripts/layers.py` 变异 5 → 10 条 caught 10。
  · **A357 点积实测**：`benchmarks/dot_variants/`（因 `cargo package` 不接受对 `publish = false` crate 的
    跨包依赖，从 `trit-dot` 的 dev-dep 挪成 workspace 外独立包）；交错 9 轮读数：
    u64 dispatch 7.6–12.2 / u64 portable 12.2–12.5 / **u32 镜像 48.1–50.7** / i32 标量 168.6–183.5 ps/trit，
    三档无交叉 ⇒ 处置是**定性质**（要 CPU 吞吐用 `trit-dot`，u32 那份是 WGSL 的逐位对拍基准）。
  · **文档拆开写**：新增 `docs/ROADMAP.md`（该搬进 / 该新开 / 该搬出 / 排序 / publish 面）与
    `docs/SCOPE.md`（本文件 §0 说的仓库边界），`docs/LAYERS.md` 与 `README.md` 挂链接。
  · **publish 面**：`trit-count` 去掉 `publish = false`（判据=四条独立判据链 + 26 测试 + 16 变异全抓 +
    依赖面本就可发布），`publish_graph` 不再挡住 `trit-cli count`；名字实测 crates.io 未占用。翻面 ≠ 发布。
  · **DPLL 两刀**（详见下面两条 Perf 小节）。

### Perf（DPLL 两刀：计数器 + CSR 出现表 1.21×，再就地换号 + 复用表格 1.35×，累计 1.56×）

- 第一刀：节点入口不再 `normalize` 全表；`residue()` 用**计数器 + CSR 扁平出现表**一趟做完单位传播闭包与
  残基构造；分支单位子句由 `insert_unit()` 插回有序位置。匹配族八档 min 合计 **260.3 → 215.2 ms（1.21×）**，
  每档 −13%~−19%。第一版只换计数器、出现表仍用 `Vec<Vec<u32>>`（每节点 2(k+1) 次小分配）**只有 1.05×**
  ⇒ 瓶颈是**分配**不是扫描次数。
- 第二刀：`residue` 顺手把出现变量压成 `1..=m` 新号并**就地改写子句**（省掉 `renumber` 的第二次全表拷贝），
  两个子节点**共用同一份表**（插→递归→原位摘→插反面），省掉每个分支节点一次深拷贝；
  `decompose` 之后那次换号是真换号，保留。八档 min 合计 **225.4 → 166.4 ms（1.35×）**，每档 −18%~−28%。
- 等价判据（两刀各做一次，口径相同）：`--random --cases 2`（16 个统计字段）、`--big 24 --cases 2`（8 个字段）、
  匹配族八档的 **nodes / branches / comps / cache 命中数与失败数逐条不变**，且同一二进制两轮自比一致
  （第一轮比对把计时字段算进去，作废重做）。
- 两处自己的错都由门/断言先抓出，不粉饰：① 「单调映射自动保序」的推理被 `debug_assert` 推翻
  （反例 `[3,4,-7]` 排在 `[-1,-2]` 前，符号翻转字典序）⇒ 排序保留，只省拷贝；
  ② 变异表里我自己写过一条**空变异**（只加注释），修成真删 `numbered.remove(at)` 后才被抓住。
  另一条「`sort()` 改 `reverse()`」存活且确认是**等价变异**（结果与命中率都不变），按门规删除并披露。
- 变异：`trit-count` 16 → 20 → **21 条，caught 21 / survived 0**。
- 未做：两级 watched literals。两刀的实测说明真正吃时间的是分配与全表复制，
  下一笔收益更可能在 `decompose` 的分组拷贝——那条同样先测再动。

### Added（分层与归位：算子 / 算法 / 学习 / 生产链）

- 措辞更正：README 标题、`docs/DESIGN.md` 一句话、`sansheng` 的 crate description 里的
  「三值推理**工具箱**」改成「**底座**」——这个仓不是工具箱，是所有三值实现的落点。
  （文档里剩下那些「工具链」指的是编译器工具链，不是自我定位，不动。）
- 新规范 `docs/LAYERS.md`：四层定义 + 「新东西进来放哪」的 5 问判据 + 依赖方向 +
  去重三条硬规则（一个能力一个权威实现、oracle 与被测不同体、性能声明必须带规模档）+
  外部取用口径（生产链按 **git pin + tag** 依赖，不 vendor、不抄源码）+ 改名的固定顺序（现在不改）。
- 新门 `scripts/layers.py` + 登记表 `scripts/layers.json`：35 个 crate 全部归位
  （base 3 / ops 6 / algo 8 / learn 6 / prod 12），依赖边全部向下，唯一的跨层回边
  `trit-graph → trit-learn`（mpnn 要 `Ste`）在 `allows` 里点名并写了删除条件。
  不登记的新 crate、倒着依赖、失效例外、陈旧登记、未知层名都判红。
- 门的自证：`--self-test` 1 正例 + 6 反证全绿；把 `trit-core` 临时登记成 `prod` ⇒ 门立刻报
  4 条以上回边 rc=1（实测，不是推断）；变异 5 条 **caught 5 / survived 0**。
  诚实披露：python runner 的变异属「未点名」（没打印可归属的断言名），只算抓住不算指名。

### Measured（trit-count：BDD 坏顺序族 = 匹配族，给 watched literals 定靶子）

- 新 `crates/trit-count/examples/matching_family.rs`：`(x0∧y0) | … | (x_{n-1}∧y_{n-1})`，
  表达式里 x/y 交错出现、**声明顺序却是「所有 x 再所有 y」**（BDD 的教科书坏顺序，结宽 2^n）。
- 读数（µs/节点 = DPLL 毫秒 ÷ 节点数）：

  | 对数 | rails | 子句 | DPLL ms | 节点 | µs/节点 | BDD |
  |---|---|---|---|---|---|---|
  | 8 | 63 | 320 | 13.7 | 203 | 67.6 | ok |
  | 12 | 95 | 488 | 35.2 | 323 | 109.0 | ok |
  | 16 | 127 | 656 | 66.5 | 443 | 150.1 | ok |
  | 20 | 159 | 824 | 113.6 | 563 | 201.8 | refused(>200k 节点) |
  | 24 | 191 | 992 | 170.3 | 683 | 249.4 | refused |
  | 32 | 255 | 1328 | 305.5 | 923 | 331.0 | refused |
  | 40 | 319 | 1664 | 496.2 | 1163 | 426.7 | refused |

- 结论两条：① **这一族 BDD 会炸、DPLL 扛得住**（节点数只随对数线性涨，20 对起 BDD 被预算拦下）
  ⇒ 上一轮「DPLL 反正比 BDD 慢，不值得优化」的判断在这一族上不成立，watched literals 有靶子了。
  ② DPLL 的 **µs/节点随实例线性变差**（67.6→426.7，随子句数 320→1664 同比涨），
  与每节点 O(子句数) 的三处全表扫描（`normalize` 重排全部子句、`propagate` 全表迭代、residual 重建）同比涨；
  **机制属推断**（没做逐段计时，本机 Instant 分辨率 ~100 ns 也不够切 200 µs 级的三段），
  但它足以否证「DPLL 不值得优化」：这一族只有 DPLL 出得了数。
- 锁：新增 2 条测试（`matching_family_hits_the_closed_form` 对 **闭式解 `4^n − 3^n`**，与两条引擎无关；
  `matching_family_blows_the_bdd_budget` 钉住「20 对是 refused 而不是不一致」）；测试 24 → 26。

### Added（trit-count：BDD 第三方计数）

- 新模块 `crates/trit-bdd/src/count.rs`：`count_models_bdd` 在 dual-rail BDD 上数「全部约束为 `True` ∧ 表达式取目标值」的合法赋值，
  合法域由 `valid()` 并入；未被引用的变量按 `Bdd::exact_cardinality` 的整个变量集自动计入（实测常量式在 2 个二值变量上 = 4、
  2 个三值变量上 = 9，只引用 `a` 的二值式在 `a,b` 皆二值时 = 2）；超过 `u128` 报 `BddError::CountTooLarge`，不环绕。
- 过程中的负结果：先按「BDD 只数到最高判定变量」补了 `2^free` 乘子，实测把 `a`（`b` 未引用）数成 4（正确 2）——
  终端节点记的就是变量总数，乘子是重复计数，已删。
- `trit-count` 侧新增 `count_models_bdd` 包装与 `CROSSCHECK_BDD_LIMITS`（20 万节点软预算）；
  `crosscheck_under` 升级为 DPLL / 穷举 / BDD **三方**对拍，报告多出 `bdd_count`、`bdd_error`、`three_way`；
  `crosscheck_under_with_limits` 让调用方自己给 BDD 预算 —— 被预算拦下时 `matches` 仍可成立、`three_way` 必须为假。
- CLI：`--expr` 多报 `bdd_count` / `three_way` / `bdd_time`；`--policy` 每档变成 `dpll/exhaustive/bdd` 三个数；
  新增 `--big [MAX_N]`：穷举跑不动的规模（n=13..）只做 DPLL vs BDD 两方对拍。
- 读数：`--random --cases 2`（n=1..12，216 个 case）0 失败、0 拒绝；n=12 单例最慢 dpll 20.1 ms / 穷举 872 ms / bdd 4.0 ms。
  `--big 24 --cases 2`（54 个 case）0 失败：n=13 max dpll 33.7 ms vs bdd 2.9 ms、n=21 51.1 vs 13.6 ms
  ⇒ 在这批随机深度 4 的形状上 BDD 不比 DPLL 慢。等价树（`<=>`，全二值，n=8/12/14）与三值 `^` 树（n=8/12/16，
  K3/B3 的 `^` 是强加＝二元合取）三方逐条一致；n=16 那个例子穷举 93.8 s vs dpll 4.1 ms。
  **未证**：会让 BDD 指数爆炸的函数类（隐藏加权位、乘法）本机没构造，「BDD 更快」只对我测过的形状成立。
- 测试：`trit-bdd` 22 → 28、`trit-count` 22 → 24（三方一致、超穷举规模两方一致、预算拒绝不算不一致）。
- 变异：`trit-bdd` 4 → 9、`trit-count` 13 → 16；`mutation_check.py --pkg trit-bdd --pkg trit-count` **caught 25 / survived 0**。
  已知缺口：把 `matches` 里「BDD 那一路」整条删掉的变异杀不掉——三方永远一致，没有注入错误计数的入口；登记时避开了这一条。
- 依赖：`trit-count`（实验层，`publish = false`）新增依赖 `trit-bdd`（可发布），方向合法，`publish_graph` 通过。
- 门禁：workspace test **1959 passed / 0 failed**、fmt / clippy / doc / module_budget / publish_graph /
  test_surface（台账 22→28、22→24）/ ci_integrity / ci_budget / hazard_gate 全过。
  跑第一轮 workspace test 时 E: 盘被 target 目录占满（`No space left on device` 伪装成 `trit-net` 链接错），
  删掉自己已失效的 trit-count 临时 target 目录后重跑才拿到读数——不是代码问题。

### Added（trit-count：策略文件计数）

- 新模块 `crates/trit-count/src/policy.rs`：读取策略 JSON（`algebra` / `binary` / `assumptions` /
  `definitions` / `assertions`），递归展开 definitions 并检测循环；`binary` 与定义同名、空断言集、
  非法名称、未知顶层字段都判错。
- CLI `--policy <path>`：把 assumptions 当约束，逐条断言报 `true/false/unknown` 计数并与穷举对拍；
  三值总数必须等于「前提为真的赋值数」，不一致退出码 1。
- 读数：`examples/policy.json` 两条断言在 12 个赋值上全成立（true=12）；
  `crates/trit-cli/tests/fixtures/bad_policy.json` 暴露 false=1 / true=9 / unknown=2，DPLL 与穷举逐条一致。
- 测试 12 → 22（展开、前提收缩、未使用 binary 并入域、循环拒绝、同名拒绝、空断言拒绝、默认代数、未知字段等）；
  变异 8 → 13，`mutation_check.py --pkg trit-count` **caught 13 / survived 0**。
- 依赖：新增 `serde`(derive) 与 `serde_json`，只用于策略解析；计数与穷举路径不依赖 JSON。

### Added（trit-count：约束计数）

- `count_models_under` / `crosscheck_under` / `Encoder::encode_with_constraints`：在每条约束为 `True` 的赋值上计数；
  CLI `--constraint` 可重复，省略 `--vars` 时自动把约束变量并入变量域。
- 新增 3 个对拍测试（约束 vs 穷举、约束收缩域、约束合并 + 未使用变量），测试 9 → 12。
- 变异新增 2 条（约束不置 True、忽略约束），`mutation_check --pkg trit-count` 应保持全抓。
- 同轮试过“缓存键变量重命名规范化”：实测命中率/节点数不变、耗时略增，已撤（负结果，不落代码）。

### Added（trit-count：三值精确模型计数原型）

- 新实验 crate `trit-count`（`publish = false`）：表达式 → dual-rail CNF → DPLL + 连通分量分解 + 组件缓存，
  计数目标可为 `True` / `False` / `Unknown`；`--random` 对 n=1..12 做 DPLL vs 穷举逐条对拍。
- 编码与 `trit-sat` 的符号化编码**故意各自独立**（计数参考实现不与求解器共享代码），
  并由 `trit-sat::satisfiable` 在 True 目标上做独立交叉验证：`count > 0 ⇔ SAT`，SAT 见证重新 `eval` 为 True。
- 读数（本机 release，3 cases × 3 代数 × 3 目标 × n=1..12）：**324 个随机用例 0 mismatch**；
  n=12 最大 DPLL **3.06 ms** vs 穷举 **389.76 ms**（约 127×），交叉点约 n=8，缓存命中约 46%。
- 测试：`cargo test -p trit-count` **9/9**；登记 `scripts/test_surface_baseline.json` `crate/trit-count: 9`。
- 变异：`scripts/mutants/trit-count.json` 登记 6 条（基字面量极性、binary x 轨、常量 true、自由变量因子、
  分支只数正支、组件乘积覆盖）；`mutation_check.py --pkg trit-count` **caught 6 / survived 0**。
- 边界：计数用 `u128`（自由变量 ≥128 会显式 panic）；未做 watched literals / 变量重命名规范化；
  n≤12 只证明与穷举一致，尚未在真实策略/网表负载上测 crossover。

### Added（门脚本族：全表锚点 sweep `scripts/mutation_sweep.py`）

- 补上一块盲区：`mutation_check.main()` 只对 `--pkg` **选中**的条目调用 `preflight`，
  所以没被选中的目标里，锚点烂掉了没人报。`scripts/mutation_sweep.py` 不复制判据——
  `from mutation_check import load_mutants, preflight` 直接复用，把同一套「文件存在 + 锚点恰好 1 次 +
  不许落在自测代码之后」跑遍整张表；另加两条门本体没有的检查：**跨目标重名判红**
  （同名会让「谁抓住了哪条变异」对不上账），**0 条判红**（空表不等于全绿）。
- 单独成文件而不是改门本体：`scripts/mutation_check.py` 此刻有另一路在飞的改动
  （撤过期豁免），同一文件两个写者必撞；这里对门本体**零行改动**。
- 读数：真表 sweep **743 条 / 55 个目标 / 问题 0 / 0.12s**（成本可忽略，适合每次必跑）；
  `python scripts/mutation_sweep.py --self-test` **12/12**；
  自己登记 5 条变异 ⇒ `python scripts/mutation_check.py --pkg scripts/mutation_sweep.py`
  **断言抓 5、崩溃 0、编译红假杀 0、存活 0**（每条都由点名的断言判红，例如
  「重名检查失效」⇒ `FAIL 重名要判红`，「未知旗标不再拒绝」⇒ `FAIL 走 main：未知旗标 ⇒ rc=2 期望 2，实际 1`）。
- 自测里 `main()` 的过程输出被 `redirect_stdout` 吞掉：绿日志里出现 `FAIL 用法：…` 形状的行
  会让下一个人误判门红——这是自测自身也要守的形状。
- 🔴 **本条只落"脚本 + 登记表"，CI 挂步还没做**：把 sweep 接进 `.github/workflows/` 需要动
  另一路正在声明的面（`.github` 整目录被别的在建 PR 占着），对齐后另开一片；在那之前
  这条门是"可跑、有变异锁、没进 CI"的状态，不许当成已经每次必跑。

### Added（trit-re：PE32 函数边界取自 COFF 符号表）

- `pe::parse` 现在读 COFF 符号表（`PointerToSymbolTable`/`NumberOfSymbols`，18 字节条目）：
  只收 `Type>>8 == IMAGE_SYM_DTYPE_FUNCTION(2)` 且 `SectionNumber > 0` 的函数符号，
  `Value` 按 RVA 解释并自校验「必须落在所指可执行节内」——对 strip 过/非常规的表
  **宁可丢符号也不给假边界**；短名内联、长名走串表（首 4 字节为 0 时读后 4 字节的偏移）。
  新增字段 `PeInfo::coff_functions`（`Option<Vec<CoffFunction>>`）。
- `functions::build_pe` 在没有 `.pdata` 时改用 COFF 符号表当边界源
  （`FuncSource::CoffSymbols`，区间终点 = 下一个函数起点/所在节末尾），并把它算作
  **精确**边界（与 ELF `STT_FUNC` 同等权威）。效果：PE32 的同函数共现从「一律 `Unknown`」
  变成可判——这是路线图 N2 判据里「PE32 共现从 `Unknown` 变为可判」的一半，靠符号表而不是 CFG。
- 测试 +1（`tests/pe.rs::coff_symbols_give_pe32_exact_boundaries`：合成 PE32 夹具带 3 只符号
  ——短名函数、串表长名函数、非函数符号各一只——钉住「过滤 + 逐字节边界 + 归属 + 共现可判」）；
  变异门 +1（`scripts/mutants/trit-re.json`「COFF 函数类型过滤失效」；连同上一片的
  「未归属裁定的非执行判断写反」，登记表现 29 条）。
- 🔴 夹具的一次改法披露：符号表里那只非函数符号原本住在 `.rdata`，结果「节必须可执行」
  替 `Type` 过滤把了关 ⇒ 变异门打「去掉 Type 判断」照样**存活**（防御码空转）。已把它搬到
  可执行节 `.text`（RVA 0x1180），现在只有 `Type` 那道判断能挡它，这条变异改判为断言判红。
- 为守 800 行预算搬家（公开路径不变）：COFF 符号表面 → `crates/trit-re/src/pe/coff.rs`
  （`pe.rs` 878→786 行，照 `pe/overlay.rs` 先例）；带符号表的 PE32 夹具 →
  `crates/trit-re/tests/support/pe_builders.rs`（`support/mod.rs` 802→701 行）。
  `pe::CoffFunction` 与 `support::pe32_with_symbols()` 经 `pub use` 保持原路径。
- 🔴 证据等级：**合成夹具已证，真实 mingw PE32 样本未证**。本机只有 mingw-w64 x86_64
  （`-m32` 没有 i686 工具链），拿不出带符号表的 32 位真样本；按 N1 口径要有真样本后补 `sha256` 记录。

### Added（trit-re collect：procscan 现场模块 → 逆向台账，路线图 N3 落地）

- `trit-re::collect`：吃 `trit-procscan` 的 JSONL 事件流，抽取
  `type:"module"` 的路径——**管道式接入**（`trit-procscan … | trit-re collect`），
  不建 crate 依赖。纪律：坏 JSON 行/非 module 事件**计数上报不静默**；
  同路径**去重保序**（同模块多进程加载只分析一次）；单文件读取失败
  **不中断采集**（计数 + ≤8 条原因）。
- 每模块产出 `ledger::jsonl` 台账行，末尾一条 **`collect-summary`**
  （module_events/modules/deduped/analyzed/unreadable/non_module_events/
  bad_lines/bytes/elapsed_ms/errors）——路线图 N3 判据「一次采集 ≥ N 条
  记录、耗时与文件大小线性」直接从汇总行复算。
- CLI：`trit-re collect [--events <file>] [--summary-only]`（缺省读 stdin）。
- 依赖补 `serde`/`serde_json`（事件流是外部输入，字段级取值）。
- 实测冒烟（真事件流 5 行：同一 `notepad.exe` 两次 + 一只不存在的 dll + 一条 `process`
  事件 + 一行坏 JSON）⇒ 汇总行 `module_events=3 / modules=2 / deduped=1 / analyzed=1 /
  unreadable=1 / non_module_events=1 / bad_lines=1 / bytes=356352 / elapsed_ms=22`，
  `errors` 点名「系统找不到指定的文件。(os error 2)」；stdout 共 **622 行**（621 条台账 + 1 条汇总）。
  同一份流把路径写成单反斜杠（JSON 非法转义）时 ⇒ `module_events=0 / bad_lines=3`，
  一条模块都不解析、但计数照报 ⇒ 坏行上报不是纸面话。
  测试 `tests/collect.rs` 4 条用内存文件系统注入 `read` 闭包，不碰真实文件系统。

### Added（trit-re：未归属调用点的逐条裁定）

- `functions::FunctionReport` 新增 `ruled_sites`（≤64 条明细）与三类计数
  `ruling_clip`/`ruling_miss`/`ruling_nonexec`：每个归不到函数区间的调用点
  按证据分类——疑**边界裁剪**（距最近函数边界 ≤16B）、疑**边界漏检**
  （exec 内、距边界 >16B）、**非执行节**（字节模式误报候选）；
  距离字段（`RuledSite.distance`）可复算。方向保守不变：裁定是证据分类，
  不把调用点洗成「已归属」，共现的 `Unknown` 拖累逻辑不动。
  这也是路线图 N2（最小 CFG）成效的现成度量：边界漏检类应随 N2 显著下降。
- 测试 +2（三类裁定逐一命中 + 明细截断但计数完整）；变异门 +1
  （非执行判断写反）。


### Added（M3 受控探测：allowlist 默认拒绝的实验室端口/服务指纹）

- `trit-net::netsec::probe`：证据生产端——M2 攻击图是登记制，probe 是第一个
  机器化证据来源。授权边界机械化（每条可测）：**只吃字面 IP**（DNS 名拒绝，
  SSRF/重绑定面从协议层排除）；**实验室范围** = 环回/私网/链路本地/`::1`/ULA
  （刻意不含 CGNAT 100.64/10、198.18/15、组播、公网）；**默认拒绝**（allowlist
  显式列出 (ip, port) 才探，主机在列端口没列也拒）；**硬上限防扫全段**（≤64 目标
  /≤32 端口每目标/≤512 总探测，超顶拒绝运行而非缩放，**端口段不支持**——段就是
  扫描的形状）；串行 + 限速 + 全局 deadline。
- 结果只分类不升格：成功=`T`、明确拒绝（RST）=`F`、超时/不可达=`X`（分不清
  filtered/down 不替网络下结论；平台现实：Windows 的 connect_timeout 常把拒绝
  表面化成超时，那时落 `X`——宁可少报信息不多报结论）。banner 摘录与知名端口
  提示只是 note 里的线索，不是判决。每条输出自带 `source: "tcp-connect"`，
  可直接作为攻击图登记的出处。
- `trit-net probe <allowlist.json> [--connect-timeout ms] [--delay ms]`：CLI 入口，
  JSONL 逐行输出 findings，stderr 报 planned/probed/truncated。
- 测试 `tests/probe.rs` 11 条（默认拒绝、DNS/公网拒绝、实验室范围穷举、硬上限、
  配置超顶、分类纯函数、**环回真连接**：开端口 banner=`T`、关端口永不绝不
  `Confirmed`、deadline 截断如实、JSONL 往返；全部只打 127.0.0.1）；
  变异门 +2（拒绝分类被改、allowlist `&&`→`||`）。

### Added（M2 三值攻击图：证据等级 + 弃权语义进攻击侧）

- `trit-net::netsec::attack`：三值攻击图——节点与边都带证据等级（`T` 已证实 /
  `X` 待验证 / `F` 已排除），登记一律带出处（空出处拒绝）。路径折叠 = K3 AND
  （格序 min，`F` 支配；**不是** `strong_and`——⊗ 会让 `F ⊗ X = X`，把已排除的
  跳数洗活）；跨路径合并 = K3 OR（一条已证实路线即可达已证实）；枚举截断
  **一律封顶 `X`**，既不敢确认也不敢排除。`X` 判决带机器可读见证（`open` 逐环节
  点名待验证的边/立场）。修的缺陷：自动渗透文献把「没验证」当「成功」，攻击图的
  边没有证据等级（见 `docs/SECURITY-NET.md` §与论文的关系）。
- `trit-net attack <台账.jsonl> --src <label> --dst <label> [--max-depth N]
  [--max-paths N]`：CLI 判决入口，JSON 输出 verdict/reason/paths/open；
  未知节点如实失败退出。JSONL 台账往返 + 坏行显式报错（不静默丢行）。
- 测试 `tests/attack.rs` 16 条（折叠穷举对拍 K3 AND/OR、截断封顶、LWW 登记、
  幂等复用、环路终止、CLI 端到端）；变异门新增 2 条（fold 换 OR、截断不封顶）。

### Added（无损回退底座 + 依赖脱离体检 + 一句话提交）

- `scripts/autoflow.py run -- <命令…>`：一条命令内**先快照、失败自动回退**——成功保留改动并打印快照 id；
  失败自动补回被删、覆盖被改、**删掉命令新建的文件**，失败码原样返回；`--keep-on-fail` 保留现场。
  实仓演练：改 README + 建探针后 exit 5 ⇒ README 逐字节还原、探针被删、rc=5。
- `scripts/gitflow.py worktree add|list|remove`：**多会话并行**——新分支直接开在仓旁的独立 worktree
  （拒绝放进仓里、拒绝删当前所在树），解决"共用一个工作树被切分支"的老问题。
- `scripts/gitflow.py stack`：按 base→head 串出 PR 链；base 不是 main/master 的 PR 单独告警
  （按 base 过滤的 CI 会零 check，这个坑踩过）。
- `scripts/gitflow.py branch protect`：查主干保护规则（必需检查/审批数/是否对管理员生效）。
- `scripts/gitflow.py branch prune --remote --apply`：删已并入 base **且**有 MERGED PR 记录的远端分支（默认 dry-run）。
- `scripts/gitflow.py branch list|start|sync|prune`：**分支层面的日常**——领先/落后、已并入、陈旧标记、PR 状态；
  `start` 从最新 base 起分支（验名字、拒受保护名）；`sync` 先 fetch 再 rebase，冲突自动 abort 并列出冲突文件；
  `prune` 默认 dry-run，`--apply` 才删已并入 base 的本地分支（不动受保护分支/当前分支）。
- `scripts/gitflow.py publish --title … --files …`：**一句话送 GitHub**——显式点名文件（不做 `git add -A`）、
  提交前密钥扫描（`sk-*`/GitHub/Slack token/私钥头/AKIA/`.env` 一类按文件名拒收）、推送、开 PR、
  `gh pr checks --watch` 绿后 squash 合入；`--dry-run` 只打印将执行的命令。
  自锁时抓到一个真问题：扫描器把**自己源码里的正则字面量**当成密钥 ⇒ 改成运行期拼接模式串。

### Added（`trit-parts` 清单约束 argmax + 评估）

- `parts.rs`：清单解析（k/id/digits，已用真实清单 135 部件验证）、`id↔digits`、
  `decode_free` 与 **`decode_constrained`**（在清单上取 `Σ logit[i][digit_i]` 最大，同分取小 id）。
  约束解码**结构上不可能**输出清单外 id——单测构造「无约束落到 id 8（清单外）」的 logits，约束返回合法 id。
- `evaluate_frame`：平面准确率（仅 GT 部件像素）、id 一致、部件 IoU、清单外 id 率（不同 id 个数）与
  清单外像素占比；`parts-train --eval --manifest` 把无约束/约束两组指标并排打印。
- 单测：4 passed（约束必在清单内 / 约束取最优合法 id / id↔digits 往返 / 指标行为）。
- **注意**：约束的实际收益要等正式模型；当前坏 ckpt 只预测 10 种 id（恰好都在清单内），两组指标相同。

### Added（`trit-parts` 训练循环 + 三值判读口径更正）

- `data.rs`：PNG(RGB)→NHWC f32、`.npy` int32→三进制平面 + valid（编码与 `part_index.encode` 同口径）。
- `loss.rs`：层级加权 CE（k 平面）+ 0.5·BCE(valid)，中心差分逐元素对拍；
  准确率只在 **GT 部件像素**上统计（此前分母错误会少算 k 倍）。
- `net.rs` + `parts-train`：13 层整网前向/反向、STE + Adam/SGD、梯度累加、断点续跑
  （resume→save 逐字节一致，只差 step 字段）。
- 过拟合自检（big-r3.6 4 帧 / 150 步 / lr 1e-3）：loss **0.7005 → 0.0244**，
  valid 0.039 → **0.997**，平面准确率 0.30/0.30/0.30/0.39/0.51 → **0.59/0.65/0.56/0.35/0.38**（仍在升）。
- **口径更正**：「三值 vs f32 平面 argmax 0.9871」被 98% 背景像素灌水。只算部件像素时，
  三值 ckpt 的平面准确率是 **0.28–0.52**，f32 是 **0.74–0.91** ⇒ 事后三值化是崩的，QAT 不是可选项。
- 未做：清单约束 argmax。

### Added（`trit-parts` 反向内核：四类算子 + STE 口径对齐）

- `backward.rs`：卷积 / 转置卷积 / 最大池化 / 通道拼接的 f64 参考实现（前向 + 反向），
  中心差分逐元素对拍（conv 1×1 与 3×3、有/无 ReLU）最大相对误差 < 1e-6。
- `ste_apply`：`E = q·scale` 的梯度经 STE 回落到 latent 与每通道 scale；与 `trit-train`
  的 `Identity / Clipped / Soft` 三种 STE 做单步更新对拍，逐位一致。
- `shaders_bwd.wgsl` + `gpu_bwd.rs`：8 条反向管线（conv_dx/dw/db、pool_dx、cat_dx、convt_dx/dw/db）。
- `parts-gradcheck` 二进制：GPU ↔ f64 CPU 对拍 10 项，判据 `max rel < 2e-3`，实测 ≤ **2.1e-6**。
  变异自检：conv_dx 权重×2 / convt_dw×2 / pool_dx 减半，三个变异全部判红（rel = 0.5）。
- **未做**：训练循环（CE/BCE/优化器）、清单约束 argmax。

### Added（`trit-parts`：部件平面头进仓，模型不再寄居 `.yanagent`）

- 新实验 crate `crates/trit-parts`（`publish = false`，GPU 在 `gpu` feature 后）：
  权重加载 + BN 折叠、WGSL conv/pool/cat/ConvT、UNet 前向（f32 与每通道三值双模），
  以及 `parts-parity` 对拍二进制。按工作区死命令，模型归本仓，训练与推理一律 Rust 三进制。
- 判据（本机 RTX 4060 Ti / Vulkan，384²，与搬迁前逐位一致）：
  f32 `max|Δlogit| = 2.608e-4`、平面 argmax **737280/737280**；
  三值 vs PyTorch 同套量化 `max|Δ| = 1.367e-1`、argmax **0.999688**；
  三值 vs f32 argmax 0.987118、幅值 4.78→1.25 ⇒ 事后三值化不可用，必须 QAT+STE。
- **未做**：反向内核、训练循环、清单约束 argmax。交付模型仍缺，等这三件。
- 夹具不进 git：`D:\KF\trit-data\parts-parity`（22 MB）。
### Added（无损回退底座 + 依赖脱离体检）

- `scripts/snapshot.py`：不依赖 git 的字节级快照（内容寻址对象库）——`save` / `verify` / `restore` / `list`。
  回退补回被删、覆盖被改，**默认不删快照后新写的文件**（`--prune` 才删）；写回后逐文件 SHA-256 校验；
  回退前落 pending 标记，中断后重跑同一条命令即幂等收敛。实测本仓 623 文件 / 5.15 MiB，对象库 6.9 MB。
- `scripts/deps.py`：把「某个 crate 能不能整块摘出去、缺哪几块」逐条列出来（继承字段 / 内部依赖 /
  内部 dev / 越界引用 / build.rs），只读、离线、不改文件。本仓实测：31 个 crate **无一可直接摘出**；
  150 继承字段、54 内部依赖、2 内部 dev（`trit-gpu` / `trit-net` → 不可发布的 `trit-texture`）、
  **0 越界引用**、0 build.rs；7 个 crate 无内部依赖；17 个不可发布。
  （更正：早先报的「5 处越界引用」是**误报**——扫描器只看 `..` 前缀、没解析路径，
  `src/matmul/baseline.rs` 的 `../matmul_f32.wgsl` 其实仍在 `src/` 内。已改为解析后判归属，
  并把「指向不存在文件」单列为缺失引用。）
- `scripts/deps.py detach <crate> --to <dir> --verify`：**copy 模式脱离**——把 crate 与传递内部依赖
  （含 dev 依赖）整块拷到目标目录，实体化 `*.workspace = true`（含点号形式的依赖写法）、
  把 `workspace = true` 依赖展开成 `{ path = "../<名>", version }`，再用 `cargo metadata --offline` 验一遍。
  只写目标目录，源仓一个字节不动；实测 `trit-attn` 脱离后 **`cargo check --offline` 5.07s 通过**，
  `trit-gpu` 连同 `trit-texture` 的 dev 闭包一起脱离也能独立解析。
- `scripts/vendor.py save/restore/status`：**离线自足开关**——`cargo vendor` 整批拷进 `vendor/` 并插入带标记的
  `.cargo/config.toml` source replacement，`restore` 按备份还原并删目录；两个方向都用 `cargo metadata --offline` 验。
  实仓演练：**337.3 MiB**、`cargo check --offline -p trit-attn` 4.87s 通过；restore 后 `git status` 干净。
- 五个工具各带 `--self-test`（21 / 22 / 17 / 10 / 39 条）并进 CI；`bfxh/repo-tools` 另有 **57 个场景测试**
  （边界/不变量/组合矩阵 + 本地 bare 仓端到端 publish），CI 的 `scenarios` job 里跑。
  变异门登记 57 条自锁（12 + 16 + 17 + 5 + 7），登记表 **431 → 488**。
  （POSIX 绝对路径那条不登记：Windows 上才可观测，Linux 上会读成 SURVIVED 造成假红。）
  场景测试当场抓到 4 个真缺陷并修掉：命令不存在不回退（改 127 并回退）、prune 不清空目录、
  POSIX 绝对路径漏拦、`.env.production` 漏网。

### Changed（变异门：三语言口径 + 逐目标 fail-closed + 门自身自测）

- 登记表 **385 → 431 条**、目标 30 → 36：新增 python **39** 条（`module_budget` / `hazard_gate` /
  `publish_graph` / `pr_hygiene` 的 `--self-test` 面）、bash **7** 条（`cli_smoke.sh` 的断言工具）、
  门自身 **12** 条（裁决链 / runner 推断 / 空集 / preflight 边界）。
- `hazard_gate` 自测补两条夹具（**登记了但没写不变量仍要红**、**SAFETY 文档在签名上方且离匹配点超过 8 行**），
  并把内联测试夹具改名 `inline_cfg.rs`——原名以 `tests.rs` 结尾，`rel.endswith("tests.rs")` 已经把它当测试，
  边界判定等于空转，改坏也测不出来。
- `RUNNERS` 三种语言口径：`cargo test` / `<script> --self-test`（python）/ `bash <script> --self-test`；
  登记表没写 `runner` 时按 pkg 后缀（`.py` / `.sh`）推断，脚本不会再落到 `cargo test` 去跑。
- fail-closed 改成**逐目标**：多目标一起选中时，0 条登记的目标不能再被别的目标带绿；
  不带 `--pkg` 且登记表全空同样判红。唯一的松口是 `EXEMPT`，且自证一失效（出现自有 `fn`/自测面）就判红。
- 修复门的恢复路径：写回用 `newline=""`、恢复按字节。此前 Windows 上默认写回会把整篇 `\n` 翻成 `\r\n`，
  `bash` 见到 `\r` 直接语法错 ⇒ "抓住"被误判成崩溃、工作树也被门改坏。
- 新增 `scripts/mutation_check.py --self-test`（**36** 条检查，毫秒级）与 CI job `mutation gate (self-test)`；
  门的判定链（编译红≠抓住、崩溃≠断言、空集≠证据）从此有人盯。
- CI collect 收集 `scripts/*.sh` 与 `crates/*/scripts/*.sh`；字符类必须含 `-`，否则 `crates/trit-immune/...` 一条都收不到。

### Fixed（`trit-re`：GOT 基址按寄存器跟踪——`get_pc_thunk.ax` 等不再漏检）

- 此前只认 `call __x86.get_pc_thunk.bx; add $imm,%ebx`；PIE 构建里 GCC 会按函数挑寄存器
  （实测 `get_pc_thunk.ax` / `.dx` 都出现），`mov 0x10(%eax),%ecx` 这类访问被漏掉。
- 现在 thunk 体按字节识别（`8B 04|0C|14|1C|34|3C 24 C3`）取出寄存器号，`add $imm`
  支持 eax 短编码 `05 imm32` 与 `81 C0|reg imm32`；基址表按寄存器分组；访问侧按 `modrm`
  的 `rm` 取寄存器（`8B/8D/39/3B/85`、`80/81/83`、`FF /2|/4|/6`）。
- **更正上一片的结论**：`c32-pie` 的 `_ITM_deregisterTMCloneTable` / `_ITM_registerTMCloneTable`
  上一片被判 `False`（"本文件不引用"），实际 crt 用 `get_pc_thunk.dx` + `call *%eax` 间接调用它们
  ⇒ 现在正确为 `Unknown`。`c32-fptr-pie` 的 `strcmp`/`memcmp` 同样由 `False` → `Unknown`。
- 其余样本（c32-nopie / x86-64 与 AArch64 glibc / ripgrep）读数不变。
- 测试 **92 → 94**；变异 **25 → 27/27**（新增"GOT 基址只认 EBX""EAX 的短 add 编码不认"）。
- **未证**：不做数据流——寄存器后来被改写成普通指针时仍按 GOT 基址相加，
  方向只会多判 `Unknown`（不会多判 `True`）。

### Fixed（`trit-re`：i386 立即数取址——函数指针不再被判"没有引用"）

- 取导入函数地址时，非 PIC 给的是 **PLT 桩地址**（`push $strcmp@plt` / `movl $…,-0x4(%ebp)`），
  原先只比对 GOT 槽位，打不中 ⇒ 只被取址、没有直接 call 的导入落 `False`。
  新增第三遍扫描：`B8..BF imm32`、`68 imm32`、`C7 45|85|05 imm32` 的立即数同时比对
  GOT 槽位与已知 PLT 桩地址，命中只把该导入降级 `Unknown`。
- 取址识别补 `cmp r/m8|32, imm8|32`：`80/81/83 BB disp32`（EBX 相对）与 `80/81/83 3D disp32`（绝对）。
- 真实样本（`fptr.c`：`f = strcmp; if (argc>2) f = memcmp; return f(...)`）：
  `c32-fptr`（非 PIE）`strcmp`/`memcmp` 由 `False` → **`Unknown`**；
  其余 32 位/x64/ARM64 校准样本读数不变。
- 测试 **90 → 92**；变异 **23 → 25/25**（新增"取址不认 PLT 桩地址""C7 立即数偏移少 1"）。
- **已知假阴性（下一步）**：`c32-fptr-pie` 用 `get_pc_thunk.ax` 把 GOT 基址放进 **EAX**，
  `mov 0x10(%eax),%ecx` 识别不到 ⇒ `strcmp`/`memcmp` 仍为 `False`。
  现在只认 `get_pc_thunk.bx` + EBX 相对形式，需要按寄存器解码 modrm。

### Added（TTX3：万级大纹理档案——瓦片化 + 层级混搭 + 流式解码）

- `trit-texture` 新容器 **TTX3**（新模块 `ttx3`）：把 TT3 推到万级——
  - 基面 **10240×10240 = 1.05 亿纹素**（TTX2 上限的 25 倍；单边上限 65536，
    单级纹素上限 2^30）；
  - **瓦片化**：256×320 纹素/瓦片（64×32 个 4×10 块，与块形状严丝合缝），
    瓦片 = 编码/校验/解码/显存上传的统一单元；
  - **层级混搭**：每级 `LevelPolicy`——细层 `Capacity`（2.4 bpp 纯容量）、
    粗层 `Strength`（瓦片双写冗余，单副本损坏自动恢复；默认混搭 =
    `suggest_chain` 最粗两级 Strength）；
  - **流式**：`Ttx3Archive::decode_tile` 只物化单瓦片 320 KB RGBA；
    `verify` 逐瓦片全档校验；u64 偏移、封闭布局无回填；
  - **校验**：逐瓦片 GF(3) CRC-16 盖三元载荷 ⊕ FNV-1a 盖 RGB565 端点字节
    （只盖载荷的话端点损坏检不出来——开发中发现）；
  - **账**：10240² 全链 6 级存档 ≈ 42.1 MiB vs RGBA 533 MiB（12.7×），
    见 docs/TTX.md 的 TTX3 章节（逐级账可 `level_storage_bytes` 复算）。
- 开发中抓出的三个自伤 bug（各有回归测试）：级目录条目 20 B vs 解析 16 B
  步进错位、解析头部偏移位置错、瓦片按瓦片存储却按整图块序拼装解码。
- 测试：`trit-texture` 40 → 48（往返/混搭/强度恢复/双副本耗尽/Capacity
  检错/尺寸与级数拒绝/魔数与 flags/万级封闭账/与 crate 路径逐位一致）。

### Added（扫描分层：前端/后端识别——产物面 + 网络面 + 组件地图）

- `trit-redteam binary`：新增 `side` / `artifact` 字段——`.wasm`（`\0asm` 魔数）
  与 HTML = **前端产物**，PE / ELF = **后端（原生）产物**；前端产物跳过
  PE/ELF 硬化解析（无此面，报告 hardening=null）但**泄密/路径检查照跑**
  （wasm 里嵌 API key 是真实威胁）。产不出格式的报错信息同步更新。
- `trit-procscan`：报告新增 `network_roles`——采样窗口内每个有网络行为的
  进程的角色：**监听 = 在当后端（server）/ 发起连接 = 在当前端（client）/
  双角色（both）**，带监听端口数与连接目标数（去重）；重复采样去重。
  边界：这是窗口内的行为角色，不是进程的永久身份。
- 新文档 `docs/ATLAS.md`：全部 28 crate + playground 的**分层地图**
  （浏览器前端 / 语言前端 / 编译后端 / 执行后端 / 语义层 / 实验层 /
  横切安全工具 / 入口），逐条给证据（manifest 依赖、产物特征、放行清单），
  并说明三个扫描面（产物 side / 网络 roles / hazard gate 放行范围）怎么消费它。

### Added（`trit-procscan` 网络面检查：地址分类 + 外联/监听/扇出/连接失败）

- 新模块 `net`，接入 `run`/`analyze` 的报告（`network_findings`）：
  - **地址分类器**：RFC 作用域分类（Loopback/Private/CGNAT/LinkLocal/Multicast/
    Unspecified/Public/Invalid），IPv4 严格四段十进制；IPv6 前缀级
    （`::1`、`fe80::/10`、`fc00::/7`、`ff00::/8`、`::`），带括号写法可剥，
    `::ffff:a.b.c.d` 映射回落 v4 分类。采集端拼法 `{addr}:{port}`（IPv6 无括号）。
  - **四类发现**（只陈述证据形态，不下「恶意」结论）：
    - `outbound-connection`（high）：连到 Public / CGNAT（CGNAT 按外部算，口径已写明）；
    - `listen-all-interfaces`（high）：`0.0.0.0` / `::` 上的监听（面向所有网卡；
      只听回环或内网单卡不报）；
    - `egress-fanout`（medium）：同一 pid 对 ≥8 个不同非本机主机发起连接
      （扫描/信标重试的常见形态）；
    - `connect-failures`（medium）：同一 pid ≥4 个不同目标停在 SYN_SENT。
  - **重复采样去重**：同一条连接每个采样间隔重拍一次，先按 (host, port, state)
    去重再判阈值（10 次采样 ≠ 扇出 10 主机，有测试）。
- 修复：原 `is_loopback` 手搓前缀匹配既混了私网、又漏 CGNAT/link-local IPv4，
  IPv6 带括号写法会误判成外联——职责并入分类器后按 RFC 口径重写。
- 边界（不猜）：DNS 未采集 ⇒ 只能对 IP 字面量分类，回答不了域名；
  TCP 表是瞬时快照，看见 LISTEN 不代表一直在听。
- 测试：`trit-procscan` 17 → 26（分类表/畸形地址/四类发现/去重）。
- 变异新增 3 条（私网误判成外联、监听全部网卡不报、扇出阈值失效），全部被抓住。

### Added（`trit-procscan` 动态追溯/追踪：事件流离线重放）

- 新模块 `trace` + 新子命令 `trit-procscan trace <events.jsonl> --pid N`：
  对 `run --events` 采集到的 JSONL 做**离线、确定性**的动态追溯：
  - **追溯**：沿 ppid 链回溯到采样里能看到的根（root → target 方向），
    每环带首末出现时间/采样次数/命令行；父进程不在采样里**显式说明（不猜）**；
    ppid 环（采样伪影或伪装）检测并停止，不死循环；
  - **追踪**：正向收齐目标 pid 的多级后代子树，按首次出现排序；
  - **证据绑定**：模块 / TCP 连接按 pid 绑到链上成员；目录写入没有 pid，
    单列全程时间线；采集器 Note 原样带出。
- 同一份 JSONL 重放结果位相同一（JSONL 往返对拍有测试），可当复算门。
- CLI：`Analyze` 与 `Trace` 共用事件读取；`trace` 不参与 `--strict` 判定
  （它没有发现语义，只有证据链）。
- 测试：`trit-procscan` 3 → 16（追溯/追踪/缺口说明/环/证据绑定/JSONL 往返/JSON 形状）。
- 变异新增 2 条（后代漏间接子树、末次时间不更新），全部被抓住。

### Added（`trit-lsp` 深层三值诊断 + 导航；`trit-unsafe` 扫描面加深；Mimosa 深度扫描复核）

- `trit-lsp`（编辑期「更深层次的三进制扫描」）：
  - **深层三值诊断**（新模块 `semantics`）：
    - 矛盾律 / 排中律陷阱 lint：`a & !a`、`!a | a`（含 `&&`/`∧`/`∨`/括号变体）——
      CONTRIBUTING 红线的「矛盾律在三套语义下都不成立」在写代码时就报警
      （a = X 时结果为 X，不是 False/True）；不同变量、双侧同否定、常量词不误报；
    - **语义分歧对拍**：穷举赋值（≤4 变量）对 K3 与 Ł3 各求值一次，
      结果不同就给出带具体反例的 Information 诊断（如 `a => a`：K3=X、Ł3=1）。
      边界：B3 的毒值吸收使含 X 的求值几乎必然与它们不同，不参与对拍（否则是噪音），
      B3 规则差异靠悬停真值表呈现。
  - **变量导航**（新模块 `nav`）：`definition`（首次出现）/ `references` /
    `documentHighlight` / `rename`（WorkspaceEdit；新名必须是合法标识符且非常量词）。
    `.tri` 是单表达式文件，「定义」= 变量首次出现。
  - 诊断新增 severity 字段（lint=Warning、分歧=Information，原错误仍为 Error）。
  - 测试：lib 21 → 44、wire 1 → 2（definition/references/rename 走真 stdio 会话）。
  - 修复过程记录：律 lint 的词法匹配曾有三处 bug（`(x)` 形态 token 数、
    `!a & !a` 被从 `!` 内部重扫成 `a & !a`、一元算子夹在操作数间死循环），
    各有回归测试；变异门新增 3 条锚点全部被抓住。
- `trit-unsafe`（扫描面加深）：
  - RISKY_APIS 补 `MaybeUninit` / `mem::forget` / `write_bytes`（14 → 17）；
  - `unsafe extern`（edition 2024）声明块纳入扫描面（新 kind `extern`，
    与 fn/impl/trait 同为「承诺面」，不判 Risky 只记账）；
  - 修复存量死模式：`extern "C"` 因 scrub 抹掉字符串内容**从未匹配过**，
    改为 scrub 后仍可命中的 `extern`；
  - GUARDS 补 `is_aligned` / `NonNull` / `size_of`（16 → 19）。测试 12 → 15。
- 安全扫描复核：docs/review/mimosa-deep-scan-2026-10-28.md——Mimosa deep 扫描
  （inconclusive）6 条发现逐条人工复核全部为误报/构建产物（含 `eval` HIGH 是
  变异锚点字符串、`ternary_corpus.rs` 不存在、MongoDB 在 rustdoc 产物里）；
  不宣称项目安全。变异锚点新增 4 条。

### Added（`trit-learn` 神经组件基座 + `trit-attn` 注意力/残差/FFN 训练集成 M0）

- `trit-learn`（可发布）从「量化 + STE」扩成神经组件基座，四个新模块：
  - `dense`：**批量三值 QAT 全连接层**（per-row scale、STE 反向、梯度层内累积、
    外部优化器接口；与 `trit-train` 的单样本探针层互补，面向多层堆叠 + 批训练）；
  - `loss`：MSE / hinge / softmax 交叉熵的解析梯度、数值稳定的 `softmax`
    与注意力反向要用的 `softmax_backward`（Jacobian-向量积，有独立数值对拍）；
  - `opt`：平面向量优化器（带动量 SGD / Adam，含偏差修正）与全局范数裁剪；
  - `activation`：tanh / ReLU / sigmoid 及其「由输出求导」的实现。
  `trit-learn` 测试 6 → 29（新增 23，含 **STE 代理前向梯度检查**：对 latent 的解析
  梯度与光滑代理的有限差分对拍——真前向对 latent 分片常量，直接对拍恒为 0，是伪检查）。
- `trit-attn` 训练集成（ROADMAP §3 项）：新模块 `block` + `train`。
  - `AttentionBlock`：三值 QKV 投影 → `softmax(QKᵀ/√d)` → 残差 → 三值 FFN（tanh/ReLU）
    → 残差；前向始终用三值权重（QAT，无「导出再量化」落差）；反向是全链 STE
    （softmax 用 `softmax_backward`，残差分双路，激活用 `Activation::backward`）。
    权重/尺度梯度由 `BlockOptimizers`（每参数张量一个 SGD/Adam 实例）统一应用。
  - 验证：`dX` 端到端有限差分对拍（接线检查）；块内 `A·V` 与 `scores::attention`
    独立实现逐元素对拍；ReLU 变体同拍；批累积翻倍检查。
  - `train` 模块：关联回忆任务（`[k1, v1, k2, v2, q]`，`q` 可指任一键，值位置 1 或 3，
    24 样本；bipolar one-hot token，置换由种子固定）。**实测（1200 轮、lr=0.3、
    动量 0.9）：loss 0.964 → 0.008，末位 top-1 召回 1.000**，全程三值前向。
    读数 `attention_on_value_position ≈ 0.06`（均匀 0.2）——本配置下 FFN 支路
    主导记忆（配对固定 ⇒ 可背答案），注意力未被逼成内容寻址；要证注意力必要性
    需要「置换随样本变化」的数据集，M0 不做。
  - 死区教训（记入初始化注释）：latent 零均值小扰动 + scale 0.25 ⇒ `q` 全 0，
    前向分片常量，训练空转；改为随机符号、幅值 `U(0.1, 0.3)`（`|w/s| ∈ [0.4, 1.2]`）
    混合三值起步。
- CLI：`trit-attn train [--epochs N] [--lr F] [--hidden N] [--seed N]`（JSON 报告）。
- 跨平台约定（CI 教训）：`softmax` / `tanh` / `sigmoid` / `ln` 改为 **f64 内部求值再落回
  f32**——各平台 libm 的 f32 超越函数差 1 ulp，1200 轮训练把它放大成不同盆地
  （Windows 召回 1.0、Linux/MSRV 0.75）；f64 内部化 + 断言留余量（召回阈值 0.7）
  后三平台一致。
- 测试：`trit-attn` lib 8 → 21、CLI 3 → 7。
- 变异新增 4 条（STE 饱和门失效、softmax 不归一化、注意力丢 1/√d、残差一丢 X），
  见 `scripts/mutation_check.py`。

### Added（`trit-machines` 分级回路 M0：Issue #166）

- 新模块 `graded`：把 `snn` 当场地，四档流水线 `Find → Wait → Exchange → Conclude`。
  - **自主搜寻**：按 `seeds` 切块逐块探针，用**块外被带动的脉冲数**打分，取最高 `groups` 块——
    不需要指定目标；得分全 0 就停在 `Find`，结论 `Unknown`（不猜）。
  - **等待**：命中后静默 `wait_ticks`，末态跨阶段续跑（不是每段从零开始）。
  - **双向 / 散布**：`groups = 2` 轮转对答，`> 2` 散布轮转；可选三值 Hebbian 强化
    （只把空连接 `0` 补成 `±1`，不翻已有符号、不改阈值/泄漏/不应期）。
  - **收敛**：每组按脉冲极性投票，合成三态判决；连续 `quorum` 轮不变才判 `Conclude`，
    否则轮数耗尽判 `Unknown`。
- `snn` 新增 `simulate_with_input` / `simulate_with_input_from`（逐神经元驱动 + 末态续跑）
  与 `Run`（`fired` / `polarity`）；`simulate_with_drive` 改为其薄包装，旧行为逐字不变。
- 测试：`trit-machines` 23 个（新增 6：找不到停在 Find、搜寻选中驱动块、双向收敛、
  散布四组、强化只补空边、非法参数全拒）。
- CLI：`trit-machines graded [--size N] [--seed N] [--groups N] [--rounds N]`。

### Added（`trit-graph` M6：端到端三态节点分类 M0）

- `TernaryClassifier`：一层消息传递 + 三叉头；训练用软松弛（`tanh(latent)` + one-hot/均匀邻居状态），
  推理用硬三票 + 议会；解析梯度（`tanh → mean → 线性头 → softmax → CE`）。
- `ClassifierSample` + `synthetic_samples`：一跳到目标节点的合成任务，标签由恒等规则生成。
- 实测（300 样本、300 轮、lr=0.5）：**loss 0.368、准确率 1.000**；忽略边符号的基线 **0.357**；
  学到的转移表接近恒等（支持 `[+2.1,-0.2,-2.2]`、中立 ≈0、反对 `[-2.1,0.1,2.2]`）。
- 梯度有有限差分对拍；测试共 59 个（新增 4：硬推理、梯度对拍、训练超基线、孤立节点）。
- 变异新增 3 条（标签梯度符号反、头梯度漏特征、聚合梯度少除入度），**25/25 全部被抓住**；
  其中"少除入度"第一次因为梯度恰为 0 而存活，换成有非零梯度的样本才锁住。
- 质量数据 312 条。

### Added（`trit-learn` 可发布 + `trit-graph` TriMPNN M0）

- 新 crate `trit-learn`（**可发布**）：把三值量化、STE 三变体、scale 梯度从 `trit-train`
  下沉；`trit-train` 与 `trit-graph` 共用，避免实验层互相依赖（发布图约束）。
  `trit-train` 保留旧路径 `trit_train::quant` 的 re-export。
- `trit-graph` **TriMPNN M0**：
  - `TransitionTable`：3×3 转移表（边状态 × 邻居状态 → 投票），前向硬三票、死区 ±0.5；
  - `fit_step`：`(边状态, 邻居状态, 目标票)` 监督拟合，反向用 STE 直通到 9 个 latent；
  - `TriMpnn`：收集入边消息 → 议会合成；无入边节点保持原状态；支持多轮。
- 测试：`trit-graph` 55 个（新增 7）、`trit-learn` 6 个（迁移）；`trit-train` 仍 8 个。
- 变异新增 4 条（反对边不取反、死区边界含等号、拟合梯度反向、消息取错端点），
  **22/22 全部被抓住**；质量数据 309 条。

### Added（`trit-graph` M4：三态图核）

- `wl_subtree_kernel(a, b, rounds)`：两图**联合** WL 细化（共用签名→颜色表），
  逐轮累加颜色直方图点积；节点三态与边状态都进签名。
- `path_kernel(a, b, max_len)`：带标签有向路径核（节点码 + 每步边码/终点码），
  长度 `0..=4`；组合爆炸在 M0 之外。
- `normalized(k, kaa, kbb)`：核归一化到 `[0,1]`。
- 测试共 48 个（新增 6）：同构 = 1；边符号/节点状态/结构不同 < 1；空图为 0；
  两个正则环（C3/C6）两种核都为 1（WL 类核的已知上限，写进文档）。
- 变异新增 3 条（WL 核忽略边符号、路径核忽略边符号、归一化分母错误），
  **18/18 全部被抓住**；质量数据 305 条。

### Added（`trit-graph` M3：三通道随机游走 / PageRank）

- `tri_pagerank(view, seeds, damping, iters, tolerance)`：支持/反对/冻结三通道一起传播；
  反对边交换 plus/minus，中立边把质量冻结为 unknown；阻尼向种子先验回流。
- `TriScore { plus, minus, unknown }` + `state()`：未知占优或并列 → 悬置，否则多数通道。
- 测试共 42 个（新增 7）：支持链传播、反对边翻转、中立边冻结、种子质量累加、
  平票悬置、无种子为空、死端保质量与质量守恒（种子总质量不变）。
- 变异新增 4 条（中立边不冻结、反对边不翻转、忽略阻尼、平票判真），**15/15 全部被抓住**。
- 质量数据 302 条。

### Added（`trit-graph` M2：三态超图 + 树分解容器）

- `TritHypergraph`：三态超边（成员位集 + 自身三态 + 关键位）；`incident` 关联查询；
  `verdict(id)` 对成员开议会（关键节点反对票强否决）。
- `TreeDecomposition`：袋（节点位集）+ 袋树；`validate(graph)` 校验节点覆盖、边覆盖、
  运行交（同一节点的袋在袋树上连通）；`bag_verdict` 袋内议会。**不做分解算法**。
- 议会规则抽成公共 `tally(votes)`，图邻居/超边/袋共用一份计票。
- 测试共 35 个（新增 11：超边构造/去重/多数/否决/空超边，分解覆盖/运行交/袋议会/多根 panic）。
- 变异新增 3 条（超边判决忽略成员状态、不校验边覆盖、不校验运行交），**11/11 全部被抓住**。
- 质量数据 298 条。

### Added（`trit-graph` M1：稀疏 CSR、共享视图、三态 1-WL）

- `TriGraphView` 视图 trait：稠密 `TritGraph` 与稀疏 `TritCsr` 只实现四个基本方法，
  出度/三态计数/拓扑序/DFS 树/议会/WL 全部从 trait 派生，避免两套逻辑分叉。
- `TritCsr`：`n + nnz` 稀疏邻接；`from_dense` 保留节点三态与关键位；`neighbors` / `edge`。
- `wl_refine`：三态 1-WL 颜色细化（有向、边符号参与签名）；`color_count` 便于比较划分。
- 实测/测试：路径 vs 星形 5 色 vs 2 色；菱形把一片边改成反对 → 3 色变 4 色；
  C3/C6 同为单色（1-WL 已知上限）；稠密与 CSR 的邻域/拓扑/议会/WL 结果一致。
- 变异：新增 4 条（CSR 偏移、CSR 丢状态、WL 忽略边符号、WL 一轮就停），**8/8 全部被抓住**。
- 限制：仍无超图/树分解容器、无权重/特征/反向索引；议会规则固定不可学习。

### Added（`trit-graph`：三态结构基础层 M0）

- 新 crate `crates/trit-graph`（`publish = false`），给后续 TriMPNN / Tri-TreeOracle /
  Tri-TD-GNN 准备地基：
  - `TritGraph`：三态节点（`+1/0/-1`）+ 四态边（支持/反对/中立/不存在）+ 关键节点掩码，
    位集稠密邻接；`for_each_out` / `out_counts` / `topological_order` / `is_dag` / `dfs_tree`；
  - `Tree`：DFS 生成树的父指针、先序、后序（逆先序）、子节点；
  - `council`：三态议会——支持边投邻居状态、反对边投其反、中立弃权；
    关键节点的反对票触发强否决，否则多数票、平票悬置。
- 限制写明：稠密邻接（n² 内存）、无权重/特征/稀疏/超图、议会规则固定不可学习。
- `docs/ROADMAP.md` 增加"三态结构模型"后期清单（TriMPNN/Tri-TD-GNN/Tri-HyperGNN/BiTri 栈等）。
- 测试 15 个：跨字边界邻居、拓扑序与环、DFS 树父子/后序、议会的多数/否决/平票/弃权。

### Added（`trit-float` M1：舍入模式、同比特范围/精度、锥形精度）

- **舍入模式**：`RoundMode::HalfAway`（默认）/ `HalfEven`；`new_with` / `from_f64_with`。
- **`Format { precision, exponent_digits, taper }`**：指数范围 `3^±((3^E−1)/2)`；
  `quantize` 溢出返回 `None`、下溢为 0；`log10_range` / `log10_error` 用于避免溢出。
- CLI 新增 `range`（16/32/64 比特，匹配尾数位 / 匹配指数位 / 范围优先）与
  `rounding`（平局偏差，HalfAway vs HalfEven）。
- 实测结论：
  - 32 比特匹配指数位：log10 范围 **353.2 vs IEEE single 83.4**，但有效位 6.92 → **5.73**（≈15× 误差）；
  - 16 比特匹配尾数位：范围 10^15 vs half 10^12（1000×），有效位 3.01 → 2.39；
  - 平局偏差：p=4 时 HalfAway **+0.60%**、HalfEven **−0.52%**；p=12 均趋 0。
    **"对称舍入 ⇒ 期望误差为零"在本仓取样口径下未复现**，记为未验证。
- 锥形精度只是旋钮（精度随 `|e|` 递减，单测覆盖）；真正的 Takum 需要锥形指数编码，未做。
- 新增 `crates/trit-float/README.md` 与两张对比图（`docs/assets/float-range.svg`、`float-precision.svg`）。
- 变异名单同步：`trit-float` 11 条（新增锥形精度/格式下溢两条，修正两处因重构失效的锚点）；
  README 的质量数据从 178 修正为脚本实测的 **287** 条。

### Docs（README 重写：定位、可视化对比、代价清单）

- 顶部改为"是什么 + 搜索关键词 + 一眼看强弱"：三张 SVG 对比图（CPU 流式、每参数字节、GPU），
  数据全部来自仓库内实测；`scripts/gen_docs_charts.py` 可重跑生成。
- 新增 [`docs/LESSONS.md`](docs/LESSONS.md)：三进制代价清单、已证负结果、测量经验、工程经验。
- 面向使用者的 README 不再叙述内部检查流程（完整检查矩阵移到 `CONTRIBUTING.md`）。
- 工作区表补齐 `trit-ops` / `trit-texture` / `trit-unsafe` / `trit-procscan` / `trit-lsp` / `trit-train`。
- 更正 `trit-train` 的 STE 来源表述：`trit-net` 已有最小 STE，本层是 per-row scale + 三变体 + 位平面 ALU。
- `docs/DESIGN.md` 去掉本机绝对路径。

### Added（`trit-train`：三进制训练 M0——STE + 位平面 ALU）

- 新 crate `crates/trit-train`（`publish = false`），在 `trit-net` 已有的最小 STE 之外扩成可比较的一层：
  - **STE 三变体**：Identity / Clipped（饱和区归零）/ Soft（`1-(w/scale)^2` 衰减）；
    每行 `scale` 可学习（LSQ 风格），scale 梯度 `∂L/∂scale = dy·Σqᵢxᵢ`；
  - **位平面 ALU**：双导轨按字并行的 `not`/`and`/`or`/`xor`，支持 Kleene / Łukasiewicz /
    Bochvar；9 种操作数组合 × 3 语义 × 长度边界逐元素对拍；
  - **`TernaryLinear`**：前向 `q+scale`、反向 STE，单样本 SGD；2×3 学习任务 14 个测试。
- 实测（24 核，16 M trit，7 轮取最快）：位平面 ALU **4 线程 ~90–100 Gop/s**（峰值），
  8 线程回落到 65–81（带宽/超线程争用）；**软件预取对顺序访问无收益**（±5% 噪声内）；
  1 M trit 规模会把结论测反（线程创建开销压过计算），bench 默认 16 M。
- 边界：不做三进制算术加法（进位串行，已证负结果）；点积是标量参考（2.4 GMAC/s），
  快速掩码内核在 `trit-ops`，受发布图约束暂不能复用。

### Added（`trit-lsp`：`.tri` 的三进制语言服务器 M0）

- 新 crate `crates/trit-lsp`（`publish = false`，独立二进制），stdio + JSON-RPC，
  把 `.tri` 关联到 `target/debug/trit-lsp` 即可用：
  - **诊断**：与 `trit build` 前端同规则（单表达式文件、解析错误行列 → LSP range）；
  - **悬停**：运算符名称/优先级/结合性 + 当前三值语义下**现算**的 3×3 真值表；变量/常量解释；
  - **补全**：运算符（含 `∧∨⊕→≡` 等 Unicode 拼写）、常量词、本文件变量；
  - **文档符号**：文件内变量。
- 语义不复制：解析走 `trit_core::Parser`、真值表走 `trit_eval::eval`；
  运算符表每个拼写都有 `operators_round_trip` 测试保证解析器接受。
- 三值语义由 `initialize.initializationOptions.algebra` 选择（`kleene`/`k3`、
  `lukasiewicz`/`l3`/`luk`、`bochvar`/`b3`），默认 Kleene。
- 位置按 UTF-16 码元换算（中文注释不会错列）；同步方式 Full。
- 验证：26 个单测 + `tests/wire.rs` 起真实子进程走完整会话（initialize → didOpen → hover
  逐格断言 Bochvar 真值表 → completion → documentSymbol → didChange 诊断 → shutdown/exit）；
  clippy/rustdoc `-D warnings`、fmt、模块预算全绿。

### Changed（`trit-ops`：掩码内核行阻塞 2，大行数 −4～10%）

- 掩码内核两行共享一次 `x` 装载（`row_dot_mask2`）：每 8 trit 的装入从 3 次降到 2.5 次，
  两行仍各用 4 路累加器；输出与单行版**逐位一致**，奇数行的「成对 + 尾行」路径有单测。
- 配对实测（`bench --mask-pair`，21 轮交替）：1536×576 → 0.985（15/21）、
  8192×576 → **0.956**（14/21）、49152×576 → **0.896**（16/21）。
  收益随行数增长；小行数无可测变化。绝对值表未重测（测量期有并发负载，只报配对值）。
- **`AUTO_LUT_ROWS` 40960 → 65536**：掩码变快后 tryte 的反超点后移。重测
  （`--pair 掩码 tryte`）：49152 行 tryte 只有 0.945（4/15，已落后）、65536 打平（1.004）、
  81920 起 1.164（9/9）。旧阈值会让输出层走更慢的查表。
- 非 x86_64 的标量回退直接调两次单行版；`gemv_mask_single` 保留为 A/B 基线。

### Changed（`trit-ops`：拆表查表改用映射表，快 13.6–19.1%）

- 拆表的两次查表原用 `code % 9` / `code / 9` 定位，换成 243 项 `u16` 映射
  （`SPLIT_MAP`，486 B）后压成一次装入；两种写法累加顺序相同，结果逐位一致
  （`split_map_matches_divmod_bitwise` 单测锁住）。
- 实测（`bench --split-map`，cols=576，4 个行数档 × 7 轮配对，每档 7/7 胜）：
  中位比 576→0.864、1024→0.809、1536→0.824、2048→0.830。
- **交叉点没动**：`1792 → 0.349×`、`2048 → 1.067×`，`LUT_SPLIT_ROWS = 2048` 不变。
  `gemv_auto` 改走映射；`gemv_with_split_tables`（`%9`/`/9`）保留为 A/B 基线。
- 工具：`bench --split` 增加「拆表 + 映射查表」一行；新增 `bench --split-map` 配对模式。

### Changed（`trit-gpu`：拆掉 655 行的 matmul 驱动函数）

- `crates/trit-gpu/src/matmul.rs`（单函数 `run_async` 约 630 行）拆成
  `matmul/{mod,gpu,inputs,ternary,baseline}.rs`：开设备 / 确定性数据 / 三值内核族 / f32 基线族。
  最大文件 249 行，最长函数 <100 行（`clippy::too_many_lines` 对该目录 0 告警）。
- `bench.rs::bench_variant` 泛化为统一变体执行器：3D grid（split-K 用 z=4）、可选清输出、
  `Check::{Exact,F32}` 两种比对，去掉十份重复的「建管线 → 预热 → 计时 → 读回」。
- 测量顺序刻意保留在 `mod.rs`：同内核换位置读数能差 25%，历史读数都是按这个顺序取的。
- 读数回归（RTX 4060 Ti，512×4096×512）：o3 0.1044、split-K 0.1009、f32 最优 0.2992 ps/MAC，
  与拆前同量级；全部 `matches=true`；GPU 测试 3 passed。

### Fixed（`trit-gpu`：缺 f16 的 adapter 上 f32 基线也起不来）

- #145 把 fp16 内核写进共享的 `matmul_f32.wgsl`，文件头的 `enable f16;` 是**模块级**开关：
  不支持 `SHADER_F16` 的 adapter（CI 的 Windows 软件后端就是）连 f32 基线着色器都建不出来，
  报 `Shader 'matmul-f32' parsing error: the f16 extension is not supported`。
- 把 fp16 内核拆到独立文件 `matmul_f16.wgsl`（只在 `SHADER_F16` 可用时才创建）；
  `matmul_f32.wgsl` 不再带 `enable f16;`。f16 变体的入口/绑定布局不变。
- 由新增的 `trit-gpu --features gpu` CI 门抓到（此前该 feature 从未在 CI 编译/运行）。

### CI（PR 矩阵补 macOS + gpu feature 门）

- PR 矩阵由 ubuntu+windows 扩为 **ubuntu+windows+macOS**：与 main 同矩阵。
  #147 的教训是「PR 门看不见 aarch64 ⇒ 合入后 main 才红」，macOS 的 10× 计费
  低于一次 main 变红的代价。
- 新增 `trit-gpu --features gpu` 的 test + clippy arm（与 `trit-texture --features png` 同形）：
  该 feature 默认关，此前 CI 从未编译 `matmul.rs`/`gpu.rs` 等内核代码。

### Fixed（`trit-ops`：非 x86_64 编译失败）

- #147 合入后 main 的 macOS（arm64）CI 红了：`simd.rs` 只在 `x86_64` 下导出
  `row_dot_{f32,f16,q4,mask}`，ARM 上没有定义。PR 矩阵只跑 ubuntu+windows（x86），
  所以 PR 全绿、main 才炸。
- 补 `scalar` 回退模块（语义与 AVX2 版逐项一致），非 x86_64 导出同一组函数；
  `MASK_LUT` 只在 x86_64 编译，避免死代码告警。
- 新增 3 条对拍测试（x86_64 上 scalar vs AVX2：mask / f32+f16 / q4），
  `cargo test -p trit-ops` 31 passed；`cargo check/clippy --target aarch64-unknown-linux-gnu -D warnings` 干净。

### Changed（`trit-gpu`：2-popcount 点积，实测负结果）

- 新增 `dot_word_2popc` 与入口 `main_tiled2o3` 的对照实现 `main_tiled2o2`
  （tile / 共享内存 / 输出映射与 o3 版完全一致，只换点积算法）。
- **恒等式成立**：`2·popc(a∧b) + popc(a) + popc(b) = n`，对任意位模式无条件成立；
  `tests/dot_identity.rs` 5 条纯 CPU 测试覆盖穷举与反例。
  （反观现有的 3-popc 版本依赖「正负导轨不交叠」，交叠时有 14,911 处不等。）
- **实测更慢**：RTX 4060 Ti，`1×1×1` 74,852.85 → 75,909.91 ps/MAC（−1.4%）、
  `33×100×17` 2,008.20 → 2,046.43 ps/MAC（**−1.9%**）。
- 结论：该内核不是 popcount-bound，少一次 popcount 抵不过额外移位。
  **保留 o3 为默认，o2 只作为 A/B 基线**，不追求上线。
- 验证：`cargo test -p trit-gpu` 5 passed；`--features gpu --test matmul` 3 passed
  （WGSL 合法 + 与 CPU 镜像一致）；fmt 干净；clippy 0 告警。

### Added（`trit-codec::pack` 线格式固化 + canonical 解码）

- **线格式写成稳定契约**（`pack.rs` 模块文档）：数字映射 `F=0/X=1/T=2`、字节内低位 trit 优先、
  `243..=255` 非法、尾部填充必须是 `False`、canonical 字节数 `len.div_ceil(5)`。
- `unpack_canonical(bytes, len)`：字节数必须精确，尾部 padding 必须规范；否则返回
  `NonCanonicalPadding` / `WrongByteCount`。这是唯一能保证「同一逻辑值只有一种字节串」的入口
  （宽松的 `unpack` 会接受非规范输入，破坏哈希/签名稳定性）。
- `Packed5`（单字节，构造期拒绝 `243..=255`）与 `Packed5Vec`（构造期一次校验字节数、非法码、padding，
  之后 `get` 不再重复检查）。
- `CodecError` 新增 `NonCanonicalPadding(u8)` 与 `WrongByteCount { expected, actual }`。
- 测试 +7：243 个合法字节全穷举、非规范 padding 拒绝、字节数拒绝、200 组随机差分（严格 vs 宽松）、
  越界 `get`、canonical 各长度往返。
- **行为不变**：`pack` / `unpack` 语义与字节输出未改，宽松路径保留给既有调用方。
- 验证：`cargo test -p trit-codec` 61 单测 + 9 CLI + 5 stress 全过；`cargo check --workspace --all-targets`
  通过；`cargo fmt --check` 干净；`clippy --all-targets` 0 告警；`module_budget.py` 213 文件全在 800 行内。

### Added（三进制构建工具 M0：`trit build`）

- `trit-build <src.tri> [--out DIR] [--algebra k3] [--force] [--check]`（独立实验 crate）：解析 → `simplify` → **穷举复核**
  （不等价即构建失败、不写产物）→ 产物 `expr.txt` / `truth.csv` / `truth.tritbin` / `lookup.c`
  + `build.json`；内容哈希增量缓存，`--check` 校验产物完整性。
- `TRB1` 容器：魔数 + 版本 + 语义 + 变量数 + 长度 + **双导轨位平面**（2 bit/trit）+ FNV-64 尾；
  基 3 打包/GF(3) CRC-3 因发布图门禁（实验 crate 不许互依）留到 M2。
- `lookup.c` 是刻意边界：三值语义固化为查表，执行交给 cc/rustc（不重写二进制后端）。
- 测试 +4（产物/缓存命中/篡改检测/重建不命中缓存）；变异门 251 → **253**（trit-build 3/3）。
- **双轨并行**（`--manifest jobs.json [--jobs N] [--prefer ternary|binary|balanced] [--dry-run]`）：
  三进制轨优先、二进制轨（外部 `cargo`/`rustc`）只吃剩余槽位，**不抢占**；分配策略是纯函数
  `schedule::pick`（4 条单测），`--dry-run` 先出方案。实测 7 任务/4 槽：wall 171 ms、busy 205 ms，
  6 个 `.tri` 在 cargo 的 163 ms 内全部完成。
- **M1：Verilog 子集读入**（`d.v / --output / --expect`）：复用 `trit-circuit::parse_verilog` +
  `propagate`，与 `.tri` 链路真值表**逐行对拍**，不一致即失败、不写 `build.json`；清单新增
  `{"kind":"verilog"}`（归三进制轨）。测试 +4；变异锚点 +1。
- **TRB2 规范形（加固）**：末字节里没用到的 trit 槽必须为 0；Rust 解码器、C 运行时
  （新增 `TRIT_ERR_CANON`）与 red-team 解析器同一契约。此前同一张表有 27 种写法
  （red-team 实测），内容哈希不等于语义身份；现在严格解码器只接受一种。
- **M2：TRB2 基 3 打包 + 运行时 + 双导轨导出**：`truth.tritbin` 改为 `TRB2`（5 trit/字节 +
  GF(3) CRC-3，27 trit 由 35 B 降到 **17 B**；12 变量 132,883 → **106,300 B**，−20.00%）；
  `--decode` 校验魔数/长度/CRC 并重出 CSV/`lookup.c`；`runtime/trit_runtime.{c,h}` 是
  **独立的 C 解码实现**，逐 trit 与 Rust 端对拍；`--emit-verilog` 导出双导轨 `design.v`
  （仅 kleene，非 kleene 显式报错），由测试内位级解释器与真值表逐行比对。
  测试 +12（含解码-重编码逐字节回环、非法字节、缓存键区分 `--emit-verilog`）；变异锚点 +4。
- **M3：位并行真值表 + e-graph 重写**：12 变量（531,441 行）算表由标量 299 ms 降到
  **23.8 ms**（每轮 64 组赋值；门语义从 `Algebra` 取 3×3 结果表按最小项铺开，K3/Ł3/B3 共用）；
  e-graph 规则**先自证后加载**（≤3 变量穷举，不过就不进图），按节点数最小提取，整链再穷举复核；
  `build.json`/`expr.txt` 记录用到的规则。产物与 M2 **逐字节相同**。
  顺带修正双导轨导出的 `^`：本仓 `Expr::Xor` 是强加法 ⊕（U 传染），按布尔异或导出会与真值表对不上。
  测试 +9；变异锚点 +4（规则自证、重写接入、强加法导出、位并行算子）。
- 文档 `docs/TERNARY-TOOLCHAIN.md`：论文依据（LLVM/MLIR/egg/BitNet/平衡三进制，DOI 已核）、
  分层设计、M0–M3 验收判据、继续靠 LLVM/Rust 的职责表、未验证清单。

### Added（trit-unsafe：三值逻辑审 unsafe）

- `trit-unsafe scan --root . [--deps] [--exclude …]`：识别 `unsafe { }` / `unsafe fn|impl|trait`
  与 transmute、from_raw_parts、get_unchecked、set_len、assume_init、static mut 等高危形态；
  用 `// SAFETY:` / `# Safety` 与断言/边界检查当证据，按 K3（复用 `trit_core::Algebra::Kleene`）
  判 Safe/Unknown/Risky 并按 crate 合成；`--strict` 有 Risky 即退出 1。
- 实测：本仓 209 文件 4 站点 **0 Risky**；含 193 个依赖 crate 时 224,696 站点 / 43,376 Risky，
  其中 87% 来自机器生成绑定（`windows` 38,042、`linux-raw-sys` 3,730、`spirv` 414）；
  排除绑定后 23,969 站点 / 1,138 Risky（`ash` 856 居首）。配合 `cargo audit`：已知漏洞 0。
- 局限：`Safe` 只表示证据齐全，不是 UB 证明；词法级扫描，不展开宏、不做跨函数推理。

### Added（trit-procscan：进程行为扫描 + MCP 体检）

- `trit-procscan run -- <cmd>`：采样进程树/命令行/模块/TCP/目录写入/输出，落 JSONL；
  `analyze <jsonl>` 离线重放（shell、网络工具、`-EncodedCommand`、Temp 模块、写可执行文件、外联等）；
  `mcp <config.json>` 逐个 server 握手，量启动耗时、工具数与 `tools/list` schema 体积（≈ tokens）。
  实测 `rea`：**207 工具 / 3.16 MB schema / ~79 万 tokens / 1682 ms**。
  证据边界：非管理员拿不到内核 ETW，文件读取/注册表/DNS 未采集（报告里明写）。


### Changed（trit-gpu：共享内存 bank 冲突，f32 最优再降 3×）

- **第二个根因**：行距 8 个 `vec4<f32>` = 128 B，正好等于全部 32 个 bank 的宽度
  ⇒ 同一 warp 的 16 行全落在同一组 bank 上。先试「+1 vec4 行填充」
  （reg4-K32 0.908 → 0.508 ps/MAC），再换 `part ^ (row >> k)` 的 XOR swizzle（→ **0.298**，再 1.7×）。
- 三个 f32 内核同法处理（各自按 warp 内行距选 key）：tiled 1.77 → **0.84**、
  reg2-K8 1.16 → **0.76**、reg4-K8 1.00 → **0.40**、reg4-K32 → **0.298**（ps/MAC）。
- 三值侧共享行距 8 word = 32 B 同样让 8 行落同一 bank 组，改成 **9 word**：
  o3 0.110 → **0.102**、split-K 0.112 → **0.101**、tiled 0.178 → 0.15、reg2 0.126 → 0.124。
- split-K 内核同时换成 **3-popc**（与 o3 同一恒等式），0.120 → 0.101，与 o3 并列三值最快。
- 对 cuda-oxide 的四对同结构差距扩大到 2.0× / 2.6× / 1.8× / 3.9×（最优对最优 4.5×）。
  三值最优 0.101 对 f32 最优 0.298 ≈ 3×（此前 8×，因 f32 侧这两步降得多），不同表示不作同尺结论。

### Changed（trit-gpu：128 位向量化装载，四对同结构全部反超 cuda-oxide）

- **根因定位**：此前 naive/tiled/reg2 落后的原因不是线程映射，也不是 FMA 收缩，而是
  **内存访问宽度**——cuda-oxide 的参考内核全用标量 32 位装载，照抄结构后在「每指令带宽」上吃亏。
- f32 改动：全局装载走 `array<vec4<f32>>` 只读视图（同一 buffer，新增绑定 4/5）；共享内存
  直接按 `vec4<f32>` 组织，内层一次 128 位读喂 4 个乘加。`k % 4 != 0` 保留标量回退。
  tiled 还把 A/B 子块都按 `[行][k]` 连续存放（B 的行就是输出列），内层 32 次标量共享读 → 8 次 vec4 读；
  顺手删掉 f32 侧两个已不接线的内核（1D naive、标量 K16 版）。
- 配对复跑（同一时间窗，ps/MAC）：naive 6.205 → **3.118**、tiled 2.225 → **1.774**、
  reg2-K8 1.348 → **1.148**、reg4-K8 1.557 → **1.001**、reg4-K32（共享 8 KB）**0.908**。
  四对全部领先（2.0× / 25% / 17% / 56%），最优对最优 **49%**。
  口径：仍是同算法同分块结构，但向量化装载属于实现级差异，NVIDIA 参考内核未做；
  若同样向量化，差距会缩小（幅度未知）。
- **补上 f32 tiled 与 reg4-K8 的逐元素校验**（此前只计时、不校验）。新校验当场抓出 vec4 版
  tiled 的错位：B 子块按 `[列][k]` 落位，而累加按 `[k][列]` 读。已修，并加 `k=37` 用例覆盖
  标量回退路径（`k=1` 的 tiny 用例只走 `full==0`，碰不到装载循环）。
- 三值侧同样试了 vec4 共享读：reg2 0.126 → 0.128、o3 0.110 → 0.126，均无收益（三值每 MAC
  已由 popc 主导，砍共享读没有回报），已回退；三值继续用 3-popc + split-K。

### Added（trit-gpu：4×4 寄存器阻塞内核）

- `main_tiled4`（三值）与同名 f32 内核：32×32 输出 tile、8×8 线程、每线程 4×4 累加器，
  每个 K-chunk 只搬 32+32 行/列 × 8 word 进共享内存；`matmul` 报告与 `bench` 增加 reg4×4 读数。
- 实测（RTX 4060 Ti，512×4096×512，ps/MAC）：三值 naive 1.52 / tiled 0.18 / reg2×2 **0.119–0.126** /
  reg4×4 0.131–0.134；f32 23.28 / 2.41 / 2.41 / **1.36**。
  **结论**：f32 侧 4×4 快 1.77×；三值侧反而慢约 8% —— 三值每 MAC 已经是 popc 主导，
  再加寄存器阻塞只增压力，最快仍是 reg2×2。最优对最优 ≈ 10.8×。
- 另做四个对照实验：**`vec2<u32>` 打包共享读** 三值慢 ~20%（0.125→0.153），不采用；
  **K 段 16 word（屏障减半）** 慢 ~35%（0.168–0.173）⇒ 屏障不是瓶颈；
  **split-K×4 + atomicAdd** 连测 5 轮稳定 **快 3–5%**（0.120–0.122 vs 0.125–0.126）⇒ 目前最快；
  **纯 `countOneBits` 微基准** 1.33–1.44e12 popc/s，矩阵乘达到 **71–75%**。
  结论：剩余差距在指令混合（共享读/索引/累加），并行度/共享内存宽度/屏障都已排除。
- **3-popc 内核**（`main_tiled2o3`）：利用双导轨不交叠不变量把 `popc(aₚ&bₙ)+popc(aₙ&bₚ)`
  合并成 `popc((aₚ&bₙ)|(aₙ&bₚ))`，每 word-pair 少 1/4 popc。实测 **0.110–0.114 ps/MAC**
  （比 reg2×2 快 ~12%，比 split-K 快 ~5%），popcount 利用率 79–82%。
- **NVIDIA cuda-oxide 同尺对照**（`benchmarks/cuda-oxide-compare/`，cuda-oxide + CUDA 13 + LLVM 21 + Rust 1.99.1，
  WSL2）：同卡 4060 Ti、512×4096×512、B 转置、5×20 中位数。四对同结构内核——naive 6.233 vs 我们 23.80、
  tiled 2.243 vs 2.378、reg2×2 1.328 vs 2.389、reg4×4 1.567 vs **1.349**。
  **如实结论：只赢 reg4×4**；唯一大幅领先是「三值 0.110 对 cuda-oxide 最优 f32 1.328 ≈ 12×」，但属不同表示。
- **f32 基线对齐 cuda-oxide 结构**：新增 `main2d`（2D 16×16 线程块 naive）与 `main_tiled2k8`
  （2×2 + K 段 8）。naive 23.4 → **6.238 ps/MAC**（与 cuda-oxide 6.233 打平，差距全部来自线程映射）；
  reg2 2.36 → **1.431**（K 段 16→8）；tiled 2.372（落后 5.8%）、reg4×4 1.329（**领先 15%**）。
  修了 f32 校验容差未取绝对值的 bug（负期望值时容差变负、判定恒假）。
- **f32 再压一档**：新增 `main_tiled4k16`（4×4 + K 段 16）。读数（ps/MAC）：2D-naive 6.257（cuda-oxide 6.233，
  差 0.4%）、tiled 2.369（落后 5.3%）、reg2-K8 1.452（落后 8.5%）、reg4-K8 1.340（领先 17%）、
  **reg4-K16 1.277（最优对最优领先 cuda-oxide 4%）**。（此档随后被 K 段 32 的 vec4 版取代。）
- 顺手修 `forward.rs` 里 gpu feature 下才可见的 clippy（`chunks_exact` → `as_chunks`）。


### Added（GGUF → MOD 入向导入）

- `trit-texture::gguf::import_gguf`：读 **GGUF v2/v3**，把未量化张量（`F32`/`F16`/`BF16`）
  按 `v > ε → +1 / v < -ε → -1 / 其余 0` 三值化，组装成 `TritModel`（MOD 语义）；
  `*.weight`（2D）+ 同名 `*.bias`（可选，缺省 0）按名排序成层，层间衔接复用 `TritModel::new`。
- CLI：`trit-texture` 新增 bin `gguf-to-mod`（`--threshold` / `--tensor` / `--max-bytes`），
  输出 TTX2 模型容器。
- 硬边界：只支持未量化类型；`TQ1_0`/`TQ2_0`/`Q*_K` 等**显式拒绝**（位布局未在本仓验证，
  猜错会静默产出错误权重）；预算：文件 256 MiB / 权重 5000 万 / KV 10 万 / 张量 10 万。
- 测试 +8（GGUF 头/元数据/张量校验、F16/BF16 解码、阈值稀疏、NaN/截断/超预算、
  层链不匹配、MOD 往返）；实测 232 字节 GGUF → 41 字节 MOD（2 层 8 权重）。
  变异门 244 → **248**。

### Fixed（复审 P3：双导轨封装 / BLIF 导出校验 / 解析方向结构门）

- **L01 `trit-dot::TernaryBits` 封装**：字段改私有，新增 `TernaryBits::new(pos, neg, len)`
  校验「平面长度 = ⌈len/64⌉、正负导轨不重叠、尾部填充位为 0」；`dot`/`dot_portable`/`dot_naive`/
  `dot_sparse` 与 `dot_words*` 统一返回 `Result<i64, DotError>`，长度不一致不再 panic 或静默截断。
  旧路径 `dot` 会索引越界 panic、`dot_portable` 按最短平面静默少算。
- **L03 BLIF 导出校验**：`to_blif` 先跑 `circuit::validate`（元数/重复驱动/悬空/环），再检查
  标识符（空、含空白、`.` 开头逐项拒绝）；新增 `BlifError::Structure` / `BlifError::Name`。
- **同批修正（导出/解析对称）**：`parse_blif` / `parse_netlist` 返回前同样过 `validate`，
  不再返回"能解析但导不回去"的电路；新增 `NetlistError::Structure`；fuzz 往返测试改为
  「基线可解析 + 变异样本只要求不 panic、接受即可往返」。
- 测试 +4（workspace **734/0**）；`cargo check -p trit-gpu --features gpu` 通过（编译门，
  无硬件执行）。变异门 240 → **247**（本批 7 条：双导轨三处、BLIF 导出两处、双向解析各一）。

### Fixed（复审 P2 批次：AIGER 头 / 逻辑运算 / 平衡三进制精度 / 编码校验 / 训练与 SNN / 解析预算）

- **M01 AIGER ASCII 头**：解析只接受 `aag`（标准 ASCII），`aig`（二进制）返回
  `AigerError::BinaryFormat`；导出改为写 `aag`，CLI fixture 更名 `and2.aag`。
  之前标准 ASCII 文件会被拒、二进制头被当文本硬读。
- **M02 逻辑 vs 逐位**：`&&` / `||` 不再被静默当成逐位 `&` / `|`，返回
  `VerilogErrorKind::Unsupported`（逐位运算保留）；向量逻辑表达式不再产出错误位宽。
- **M06 平衡三进制 `range`**：`checked_pow` + 饱和（#113 已修），本批补回归记录。
- **M07 浮点精度**：`digit_count` 改按**平衡三进制位数**（`(3^n-1)/2` 上界），
  舍入改为「相邻指数两个候选、各自 clamp 到合法尾数、取误差小者」——
  `5` 在 `p=1` 时得到 `3`（旧实现给 `9`）；补 `±2/±5/±8` 边界与穷举最近性测试。
- **M08 模型编码**：`format::encode` 改为 `Result`，先 `Model::validate`，再查
  `u16` 范围与乘积；非法模型/权重/空模型都拒绝，不再 `as u16` 截断或把 `2` 静默写成 `+1`。
- **M09 训练输入**：`learning_rate` 必须有限且 > 0、`epochs ≥ 1`，逐轮 `loss` 非有限立即报错，
  不再返回 NaN 报告或改写损坏模型。
- **M10 SNN**：新增 `Network::validate`（方阵/权重 `{-1,0,1}`/阈值/decay/规模），
  `simulate`/`simulate_with_drive` 改为 `Result`，膜电位用 `i64` 中间量并限制 tick 预算。
- **M11 解析预算**：`parse_netlist` / `parse_blif` 增加字节/网络/门数（BLIF 另有 cover 行）
  预算与 `*_with_limits` 入口，默认 1 MiB / 10 万 / 10 万 / 20 万，分配前拒绝。
- 测试 +9（workspace **730/0**）；变异门 228 → **240**（本批 12 条新锚点 + 3 条随迁）。
  兼容性：`aiger` 文本必须是 `aag`；`trit-net`/`trit-machines` 的 encode/simulate 变 fallible
  （两者都是 `publish = false` 实验层，不进稳定 API）。

### Fixed（复审 P0/P1 批次 4：sparse 视图越界 H08 + P1 五项）

- **H08 sparse `View::new` 越界/语义分叉**（`trit-codec/src/plan.rs`）：构造期完整验证
  下标严格递增、域内、符号区未用填充位为 0。修复前 `len=1,index=1` 让 `copy_into`
  在 `out[index]` 越界 panic；`[2,0]` 让 `get`（二分）与 `copy_into`（顺序写）给出
  不同向量。三条读取路径现在同语义。
- **H09 trit-net API 边界**（`infer.rs`/`train.rs`）：`forward`/`analyze`/`train` 入口
  先 `Model::validate()`；训练**逐条**校验样本维数与非有限值（此前只查第一条，第二条
  短样本在 `Shadow::forward` 索引时 panic）。手搓非法模型返回 `ModelError` /
  `TrainError::Model`，不再靠索引 panic。
- **H11 SDF 经典 XOR 映射**：`XOR2`/`XOR3` 此前映射成三值强加 `Xor`，与 Verilog `xor`
  展开的 `XorClassic` 不相等 ⇒ 按实例覆盖静默落空、延迟回退默认 1。改为经典映射后
  复审复现探针（SDF `XOR2` 延迟 100）关键路径 1 → **100**。
- **M03 ATPG 最优性误报**：预算在子调用里耗尽、且还有未试的兄弟分支时置 `exhausted`
  （旧实现只 `break` 不置位）。复审复现集合（full=63、budget=4）不再把 3 个向量冒充
  最优；穷举仍给出 2 且 `proven=true`。
- **M04 检查点索引把锁目录当任务目录**：`index()`/`len()` 只认「偶数长度全十六进制」目录，
  锁目录（`.lock`、隔离目录）与噪声目录自然排除；持锁期间读索引不再 `InvalidState`。
- **M05 锁在进程崩溃后永久残留**：锁目录写 `owner.json`（pid/主机/获取时间，`create_new`
  抢记录，绝不覆盖别人的）；`break_stale_lock` 用 `ps`（unix）/`tasklist`（Windows）探测
  持有进程，只清**确定已死**的锁（探测不可用、pid 无法判定、跨主机记录一律视为占有）；
  清理走「先改名隔离 → 复核 owner」，回滚不覆盖已存在目录；`Drop` 也只删 owner 仍是自己
  的那把。无新依赖、保持 `forbid(unsafe_code)`；已知局限（pid 复用只朝保守方向、命令
  缺失时不恢复、跨主机不自动清理）写进模块文档。
- **顺带封堵三处公开 API 隐患**：空 task id 的十六进制是空串（`task_dir("")` = 版本目录），
  `save/load/remove/prune` 现在一律拒绝，`remove("")` 不再能一次删掉全部任务；`len()` 与
  `index()` 用同一判据（十六进制且可解 UTF-8），不再对「有几个任务」给出不同答案；
  sparse `View::new` 的 `8 + count*4 + …` 改 checked 运算（32 位目标上 count≈2^30 会溢出）。
- 测试 +10；变异门 216 → **228**（新增 12 条：越界下标、填充位、forward/train 校验、
  XOR 映射、最优性、索引判据、陈旧锁回收、无主锁不误杀、Drop 所有权、空 task id）；
  workspace **721/0**；fmt/clippy/模块预算绿。

### Added（TTX 格式转换落地 + GPU 端到端接线——真机测试抓到双轨跨行真 bug）

- **纯转换层 `trit_texture::convert`**（默认构建，零新依赖）：RGBA ↔ TTX1/TTX2、
  TTX1 ↔ TTX2 升降级（降级仅单级 TEX，mip 链显式拒绝）、**盒式 mip 链构造**
  （2×2 滤波、尺寸向下取到块对齐、不能严格递减即停）。
- **PNG 文件 I/O（`png` feature，默认关）+ `ttx-convert` CLI**：png-to-ttx
  （`--v1`/`--mip`）、ttx-to-png、ttx-upgrade、ttx-downgrade；CI 的 check 门补
  `--features png` 的 test+clippy（feature 代码默认构建碰不到，不加门就是死角）。
  **架构如实记**：原计划独立 trit-convert crate 被**发布图门**拦下——实验层之间
  禁止常规路径依赖（政策带自测）；改为可选 feature 收进格式 crate，默认构建
  依旧零图像解码依赖。CLI 真机冒烟：PNG→TTX(mip,96B)→PNG 往返、mip 链降级正确拒绝。
- **GPU 端到端（trit-gpu `forward`，gpu feature）**：`matmul_packed` 预打包入口
  （naive 内核，绑定与 matmul 同一套）——真机测试把 MOD 权重 `to_dual_rail()`
  缓冲直上 GPU，与标量逐元素对拍（inputs=33 跨字边界 + 容器往返重跑一致性）。
  **该测试抓到真 bug**：`to_dual_rail` 曾用平面 `index/32` 定字，`inputs % 32 != 0`
  时第 2 行起点不在字边界（pack_rows 布局是每行整字）——单输出单元测试测不出，
  真机多行立即失配。已按行×字布局修复，多行常量（第 1 行首字 = 字 2）钉死。
- **trit-net 兼容测试**（dev-dep）：`Model`/`Layer` ↔ `TritModelLayer` 经
  `rows()` 无损往返；断链（上层输出 ≠ 下层输入）两边同语义拒绝。
- 测试 +11（trit-texture 26→31、trit-net 16→18、trit-gpu 真机 +1）；
  变异门 216 → **220**（新增 4 条：双轨行界错位〔真 bug 形态〕、降级放行 mip、
  v2 解码取末级、盒式滤波只取首像素），trit-texture 14/14 全灭；
  workspace **710/0**；发布图/预算/docs 门绿。GGUF → MOD 入向转换未做
  （需读 GGUF 元数据层，体量另计）。

### Changed（MOD 载荷按「兼容/强度/三进制原生」重构 + 溢出加固 + 扩展名规范）

- **MOD 重构**（用户三优先级：兼容、强度、适合三进制本身；双轨 2 bit/权重
  的 B 类税方案废弃）：
  - **权重改基 3 打包**（5 trit/字节 = **1.6 bit/权重**，比 Q2_K ≈2.56 省
    37.5%，距理论下限 1.585 仅 +0.9%）——权重/偏置/纹理索引/校验流全套同一
    套打包，格式内没有第二进制制；GPU 改经 `TritModelLayer::to_dual_rail()`
    一次显式转换（位布局手算常量钉死），不再把双轨当存储格式。
  - **容器校验升级 GF(3) CRC-16**（g = **x¹⁶+x⁷+2** 本原，x 模 g 阶 =
    3¹⁶−1，素因子逐一排除，独立 Python 验算）：≤16 连续 trit 突发错**全检**
    （40 权重全部 16-trit 窗口穷举测试），随机错漏检 ≈ 3⁻¹⁶；CRC-3 尾 1 字节
    → CRC-16 尾 4 字节，TEX/MOD 两类同享。TTX1 的 CRC-3 行为冻结（实际除数
    是 x³+2x+1 的互反 x³+2x²+1，注释如实更正）。
  - **层间衔接进容器校验**（上层输出 = 下层输入，读写同一来源，与 trit-net
    `Model::validate` 同语义）；`rows()` 无损取回 `Vec<Vec<i8>>`。
- **溢出加固**（自查发现，32 位目标路径）：TTX1 `from_ttx` 的 `blocks × 12`
  与 TTX2 `level_blocks` 的字节预算改 checked 乘法，解析游标全部 checked——
  恶意超大声明在 64 位上被长度对账挡住、在 32 位上不再回绕放行；两条超大
  声明回归测试（v1/v2 各一，拒绝且不按声明分配）。
- **`docs/TTX.md` §4 文件扩展名规范**：唯一 `.ttx`（小写）、魔数是唯一事实源、
  禁止 .ttx1/.ttx2/.ttm 变体、MIME `application/x-ttx`；**新后缀准入政策**：
  仅当格式主体数据有三进制原生红利（A 类）才注册新后缀，其余复用现有后缀。
  §2/§6 随重构重写（模型侧 1.6 bit/权重、CRC-16 布局、GPU 转换一次如实记）。
- 测试 +5（trit-texture 21 → 26：基 3 打包常量、双轨转换布局、层间衔接、
  16-trit 突发穷举、超大声明×2）；金样字节重铸（CRC-16 尾 [144,81,14,1]
  独立 Python 长除法验算后冻结）；变异门 215 → **216**（ttx2 锚点随迁：
  双轨/字数/轨重叠 5 条退役，基 3 打包/digit 映射/层间衔接/CRC 常数 4 条
  新增 + 等价变异体「空层照收」退役并注明原因）；workspace **704/0**。
- 同轮仓库面：bfxh 名下 17 仓全部转公开（strata 直接翻；归档仓 uiscan 走
  解除归档→翻公开→重新归档；翻前扫过凭据类文件名，strata 的 baseline.env
  为性能基线数字非密钥）。

### Added（TTX v2：多载荷容器——GPU 纹理 mip 链 + 三值模型双轨权重，先写定位文档）

- **`docs/TTX.md`**（先于实现）：TTX 对业界格式的竞争力定位——纹理侧对比
  PNG/QOI/BC1/BC7/ETC2/ASTC/BasisU（TTX 2.40 bpp < BC1 4.0，3 色调色板天花板、
  无硬件采样器如实记），模型侧对比 safetensors/GGUF Q2_K/BitNet b1.58
  （双轨 2 bit/权重 < Q2_K ≈2.56，比理论下限 1.585 多付的 26% 是 B 类税），
  特点清单 + TTX1/TTX2 字节级规范 + 转换路线。
- **TTX2 容器**（trit-texture::ttx2，拆 `mod.rs` 容器 / `model.rs` 模型域）：
  魔数 `TTX2` + kind（TEX/MOD）/flags（非零显式拒绝，前向兼容不猜）/段长头 +
  kind 段 + GF(3) CRC-3 尾（对 kind 的**规范 trit 流**算，与 TTX1 同哲学）。
- **kind=TEX**：纹理 + mip 链——每级自带宽高与块载荷，宽高不增且至少一维
  严格递减（同级/未对齐/空链都是结构化错误）；4×10 块高的 mip 只能降 2–3 级
  是方块格式没有的诚实约束，写进文档。读写共用 `validate_levels` 单一来源。
- **kind=MOD**：三值模型（`TritModel`/`TritModelLayer`）——权重 {-1,0,+1} 存
  **双轨位平面**（行整字、bit = `1<<(i%32)`），与 trit-gpu matmul 内核的
  storage buffer 布局逐位一致（手算常量钉死），**上传零重排**；偏置基 3 打包
  （5 trit/字节）。pos/neg 同字同位重叠 = 结构化拒绝（PR #116 的教训：非法轨
  不许 panic 也不许静默）。层头 words_per_row 与 ceil(inputs/32) 对账。
- 校验流覆盖**权重 + 零填充尾 + 偏置**：偏置字节打翻必被抓（测试钉住）；
  金样字节测试（CRC 尾字节经独立 Python GF(3) 实现交叉验算）杀死任何序列化
  漂移。v1 文件喂给 v2 读器得到显式 `TtxMagic(['T','T','X','1'])`。
- 测试 +9（trit-texture 12 → 21）；workspace **699/0**；变异门 207 → **215**
  （新增 8 条：mip 同级放行、打包/解轨位序错 31、字数对账用 floor、空层照收、
  写/读侧轨重叠失明、偏置打包换基 2）。ttx2.rs 拆 mod/model 后 532/301 行，
  模块预算零债务保持。

### Changed（CI：计费分钟削峰——macOS 门移至 main，不降覆盖）

- GitHub Actions 月度配额 10 月告警 90%（1801/2000 分钟）。按 runner 计费倍率
  （Linux 1× / Windows 2× / macOS 10×）抽样 40 次近期运行：**macOS 一个 job 占
  计费分钟 74%**（UBUNTU=46 / WINDOWS=102 / MACOS=426），另有冷缓存单次 153 分钟离群。
- `check` 矩阵改为按事件分流：**PR 门跑 ubuntu+windows，macOS（aarch64 信号）在
  每次合并后的 main push 上全量跑**——每个合入 main 的提交仍被三平台验证，覆盖不减，
  单次 PR 计费预期从 ~11-14 分钟降到 ~4-6 分钟。
- `mutation.yml` 补并发取消组：同一 PR 的新 push 立即取消被顶掉的变异门运行
  （此前无 concurrency 组，多 push 的 PR 每次都全额跑完）；定时/手动运行用唯一
  run_id 分组不受影响。其余 9 个 Linux 门（module-budget/stress/publish-graph/
  docs/action/msrv/package/wasm/release）与 PR 变异门全部保持原样。

### Added（trit-codec：量化/稀疏/交织/校验四个小编解码组件，PR #116 修后合入）

- `quantize`：f32 ↔ trit 三值量化/反量化（TWN 风格，`beta=0.7` 默认，scale = max(|w|)），
  附 L2 量化误差读数；**有损是设计目的**（crate 文档的无损不变式已随之修订）。
- `sparse`：三值 COO 稀疏容器——只存非 Unknown 的 `(index, value)`，解码校验
  下标严格递增/不越界（结构化错误）。
- `interleave`：块交织/解交织（行写列读），把 burst error 打散配合 hamming 纠错；
  `block=1` 或 `≥ len` 恒等，尾巴 ragged 可逆。
- `checksum`：GF(3) CRC-3（`x³+2x+1`），只检错不纠错；文档声明的**本原性（x 模 g 阶 = 26）
  已钉成测试**（多项式乘法/约减逐位验证，26 步循环）。
- CLI 新增 quantize / dequantize / sparse / interleave / checksum 子命令。
- 测试 +14（codec 52 → 66）；变异门 202 → **207**（新增 5 条：
  量化 scale 折向、阈值边界、稀疏重复下标、交织行列、verify 截断放行）。
- **同 PR 的 `layout` 模块被设计否决剔除**（另设第二套双导轨类型 + panic 策略），
  记录见 `docs/DESIGN.md`「不批的设计提案」。外部贡献（bamboo-echo），ZCode 复审合入。

### Added（三态结构体收尾 + 自有纹理容器格式 TTX）

- **`TriStateDriver`**（trit-core）：三态驱动器——`enable` 时把 `drive` 推上总线、
  未使能呈高阻。这是 TriState 故事缺的最后一块拼图（值/冲突解析/总线有了，
  **使用方**没有）。
- **`resolve_drivers`**（trit-core）：多驱动总线解析——`drivers[lane]` 为挂在该车道上的
  全部驱动器，一次**收集全部冲突车道**（每条列出完整肇事方清单），与
  `TriStateBus::resolve` 的「两个既定总线、首错即断」互补（诊断/复位场景）。
- **自有格式 TTX**（trit-texture::ttx，TritText eXchange）：TT3 块纹理容器——
  魔数 `TTX1` + 尺寸/块数头 + 12 字节块载荷 + **GF(3) CRC-3 校验尾**（对全部索引
  trit 按三进制多项式 `x³+2x+1` 计算，单符号错全检——二进制容器用比特 CRC，
  TTX 按内容自己的基数算校验，这是 A 类设计选择）。加载逐段验证：魔数/尺寸
  对齐/块数/载荷长度/校验和，任何不一致都是结构化错误。
- 测试 +8（trit-core 84、trit-texture 12）；workspace **690/0**；fmt/clippy/
  模块预算/发布图绿。trit-core 新 API：`TriStateDriver`、`resolve_drivers`、`LaneConflict`。

### Changed（trit-gpu：矩阵乘加寄存器阻塞——三值再 1.6×，f32 无收益，最优对最优 21.1×）

- tiled 之上新增**寄存器阻塞入口** `main_tiled2`（两侧同构）：workgroup 8×8、每线程
  2×2 四个累加器——共享内存的 A/B 行段每次读取喂 4 个输出，算术强度再翻倍。
- **真机读数（RTX 4060 Ti，512×4096×512）**：三值 tiled 0.208 → **0.129 ps/MAC**（再 1.6×，
  popcount 内循环的指令级并行吃到了）；f32 tiled 2.641 → 2.712（**无收益**，FMA 已饱和、
  occupancy 下降抵消——如实记负结果）。**最优对最优：14.5× → 21.1×**。
- 修一个真 bug：`row_base` 曾用 `global_invocation_id × 16`（线程号！）——相邻 workgroup
  互相覆盖输出（首差 out[0]/out[1]，真机 `reg_matches=false` 暴露）；改用 `workgroup_id`。
- 正确性：三侧（naive/tiled/reg）各自与 CPU 镜像 `matmul_words` 逐元素一致（真机断言）。
  reg 内核同样在 `gpu` feature 后，默认构建变异门测不到——真机对拍钉住（如实记）。

### Changed（trit-gpu：矩阵乘内核升级到共享内存分块，「最优对最优」同尺）

- naive 之外新增 **tiled 内核**（同 bind group、新入口点 `main_tiled`，2D dispatch）：
  三值侧 16×16 输出 tile、每段协作装载 8-word（256 trit）A 行段 / B 列段进
  workgroup storage——A 段被 16 个列线程复用、B 段被 16 个行线程复用；
  f32 侧经典 16×16 SM tiling。
- **真机读数（RTX 4060 Ti，512×4096×512）**：三值 naive 1.60 → tiled **0.183 ps/MAC**（8.8×）；
  f32 naive 25.89 → tiled **2.641 ps/MAC**（9.8×）。**最优对最优：14.5×**
  （naive 对 naive 是 16.2×）——分块把两边流量各砍一个量级后，比值从 16 收窄到 14.5：
  f32 的 FMA 算力在带宽瓶颈缓解后占比上升，与「三值的红利是流量密度」自洽。
- 正确性：tiled 与 naive 各自与 CPU 位并行镜像 `matmul_words` 逐元素一致（真机断言）。
  开发中修掉一个真 bug：三值 tile 的装载曾写 16 宽、累加读 8 宽——相邻 chunk 的字
  被混入（f32 的 TILE=线程数恰好无此问题；构造测试 + 随机纹理双重对拍暴露）。
  tiled 内核在 `gpu` feature 后，默认构建变异门测不到——正确性由真机对拍钉住（如实记）。

### Added（三进制 GPU 内核研究：三值矩阵乘 + TT3 纹理解码，真机实测）

trit-gpu 新增两个三进制 GPU 内核（`gpu` feature 后），全部与 CPU 镜像真机对拍：

- **三值矩阵乘内核**（`matmul.wgsl` + `matmul_f32.wgsl` + `matmul.rs`）：A 行主序、
  B 转置行主序的双导轨布局，`C[m,n]` = 四组 POPC 点积（复用 dot 内核公式）。
  **真机读数（RTX 4060 Ti / Vulkan，512×4096×512，naive 一线程一输出）**：
  三值 **1.66 ps/MAC** vs 同结构 f32 基线 **25.89 ps/MAC** —— **同设备 15.6×**。
  机理：naive 内核受显存带宽支配，每 MAC 三值搬 2 bit、f32 搬 32 bit，
  16× 流量差兑现成 15.6×（A 类：数据表示的红利）。GPU 结果与 CPU 位并行
  镜像逐元素一致。
- **TT3 块纹理解码内核**（`texture.wgsl` + `texture.rs`）：4×10 块、基 3 取位
  （`byte/3^i % 3`，不是位段）、块网格行主序映射 `(x/4, y/10)`；与 trit-texture
  的 CPU 解码在 256×60 合成纹理上**逐字节一致**（真机验证）。修掉两个真 bug：
  基 3 打包曾被当 3-bit 位段解；块映射曾按平铺扫描序（只有 block 0 的构造
  测试测不出——随机纹理首差 texel 4 暴露）。
- **CPU 镜像进默认构建**（CI 可测）：`pack_columns`（B 转置列打包）、`pack_rows`
  （A 行打包）、`matmul_words`（复用 `dot_words`）；非整字边界（k=100）+ 非方阵
  与标量参考对拍。
- **发布图门修正**：dev-dependencies 不进发布链——`cargo package` 打包时剥离
  path dev-deps（实证：trit-gpu dev-dep 指向 trit-texture，package rc=0）。
  门跳过 dev 边并新增反证例（自测 5 → 6 条全绿）。
- 测试 +2（CPU 镜像对拍、GPU matmul 对拍含域检查）+ GPU 纹理对拍 4 条。
  变异门 200 → **202**（新增 2 条：列打包导轨互换、matmul 行基址），trit-gpu
  **5/5** 实跑全抓。workspace **649/0**；fmt/clippy（默认 + gpu feature）/
  模块预算/发布图绿。真机 11 条 GPU 测试全过。

### Changed（trit-texture 提速/省显存/清税：4×10 零浪费块 + 栈数组 + into 解码）

- **块 4×8 → 4×10（40 纹素）：清掉打包粒度税**。40 恰被 5 整除，trit 载荷 8 字节
  **零槽位浪费**（旧 4×8 的 7 字节容 35 槽只用 32，≈8.6% 税）。等调色板码率差
  **8.3% → 14.3%**（12 vs 14 字节/块；TT3 **2.40 bpp** vs 基线 2.80 bpp），
  合成纹理实测 PSNR 差仍为 **0.000 dB**（22.58 dB 持平）——`cargo run -p trit-texture` 可复算。
  显存账（crate 文档）：4096×4096 RGBA 原图 64 MiB ⇒ TT3 约 5 MiB（12.7×），基线约 5.9 MiB。
- **编码提速 27%**：`encode_rgba`/`encode_rgba_baseline` 逐块的 `Vec<[u8;4]>` 堆分配
  （每块 128 B、4096 块/512×260 图）改**栈数组**。基准（新增 `benches/texture.rs`，
  `cargo bench -p trit-texture`）：encode 628 → **491 µs**（**808 → 1025 MB/s**），
  512×260 合成纹理。
- **免分配解码**：新增 `decode_rgba_into(&texture, &mut out)`——热路径复用调用方缓冲，
  长度不符显式报错不 panic；`decode_rgba` 改为它的分配包装。基准如实记：0.5 MiB 纹理上
  into 仅快 ~1%（371 → 366 µs，分配成本在此规模不显著），价值在复用热路径与大图。
- 测试 8 条（新增 into == 分配逐字节一致 + alpha 通道显式钉 255 + 短缓冲拒绝）。
  变异门 199 → **200**（新增 into alpha 锚点，trit-texture **5/5** 实跑全抓；
  首轮存活——两条路径共用实现导致 alpha 突变对拍不出，补通道断言后判红）。
  workspace **648/0**；fmt/clippy/模块预算/发布图绿。

### Added（三进制块纹理压缩实验层 trit-texture）

- 新 crate `trit-texture`（`publish = false`）：格式 **TT3**——4×8 = 32 纹素块，
  RGB565 端点对（4 字节）+ **每纹素 1 trit** 的 3 级调色板 `{e0, mid, e1}`，
  5 trit/字节打包 7 字节 ⇒ **11 字节/块 = 2.75 bpp**。附 `trit-texture` 二进制
  在合成纹理上打同机对比表（`cargo run -p trit-texture` 可复算）。
- **二进制基线同尺对比**（同块结构、同端点、同 3 级调色板、同最近色搜索，
  唯一变量是索引的进制——2 bit/纹素）：基线 12 字节/块 = 3.0 bpp。
  合成纹理实测：**TT3 2.75 bpp vs 基线 3.00 bpp，PSNR 相同（24.63 dB，差 0.000 dB）**
  ——等调色板下 trit 索引省 1 字节/块（码率 -8.3%）而画质不变，这是可复算的
  A 类（原生红利）结论。
- **边界如实记账**（docs/TERNARY-BOUNDARIES.md 分类，写入 crate 文档）：
  A = 3 级索引 1 trit 装得下、2 bit 装不满；B = 7 字节容 35 槽只用 32（≈8.6%
  打包粒度税）+ RGB565 端点量化税；C = 端点对抄 BC1 生态格式；D = 最近色搜索
  与进制无关。
- 测试 7 条：243 种 5-trit 组合逐位往返、端点色零误差往返（可表示色用 crate 自己的
  pack→unpack 构造，色对须全通道单调——端点是逐通道 min/max，与 BC1 一致）、
  mid 槽位精确落位、并列色取小下标（确定性）、块字节 11/12 与码率代数、
  形状/尺寸/超限/块数错误路径、PSNR 排序。变异门 195 → **199**（新增 4 条，
  trit-texture **4/4** 实跑全抓）。workspace **647/0**；fmt/clippy/模块预算/发布图绿。

### Fixed（复审 P0 批次 3：trit-schedule 三件套，H04/H05/H06）

- **H04 调度延期传播 O(n²)**：不动点传播（每轮全量扫描所有任务，反拓扑链每轮只推进
  一层）改为**工作表传播 O(V+E)**——`propagate_unknown_deferrals` 抽成 `lib.rs` 的
  `pub(crate)` helper，`schedule`/`resumable`/`resumable_threaded`/runtime 四处拷贝
  全部收编。传播闭包与旧实现一致（`BlockedBy` 原因取标记时按声明序首个已延期前驱）。
  10 万项反向链（旧实现 10^10 次内层访问）现在秒过。
- **H05 runtime 资源累计溢出 panic**：`capacity = None` 不做单任务预检，两个
  `cpu = u32::MAX` 需求并发累计曾 `checked_add(...).expect(...)` panic——毒化锁后
  其余 worker 无法收尾、调用方挂死。改为**按任务失败处理**（`failed` + `stop` +
  `cancel_pending` + notify），与任务失败路径同一收尾。
- **H06 可恢复多线程执行器不接 step panic**：`task.step` 的 panic 曾直接穿透
  worker（running/资源不归还、不 notify）→ 其余 worker 与调用方永久等待。现在
  `catch_unwind` 包住并走 `Progress::Failed` 同一收尾路径。
- 测试 +3：10 万项反向链（原因逐点断言 + src 正常排定）、无容量双 MAX 资源溢出
  （确定性构造：先占满者持锁睡 100ms）、step panic 不挂死（boom 失败 + ok 完成）。
- 变异门 192 → **195**（新增 3 条，全部选择**不产生挂起**的变异——挂起变异会把
  变异门本身挂死；另 2 条旧锚点随重构迁移：`不传播延迟` → helper 反向边、
  `资源占用不累计` → `new_used` 赋值）。trit-schedule **34/34** 实跑全抓；
  workspace **640/0**；fmt/clippy/模块预算绿。

### Fixed（复审 P0 批次 2：AIGER 头部预算，H07 panic 封堵）

- **H07**（`aiger.rs`）：头部 `O` 计数此前直接 `Vec::with_capacity(outputs)`——一行
  `aig 0 0 0 18446744073709551615 0` 就触发 capacity overflow panic；`inputs + ands`
  的 `usize` 相加也会溢出 panic。现在：新增 `MAX_AIGER_BYTES = 1 MiB` 字节预算（解析前）；
  `I+O+A` 与 `I+A` 全部 `checked_add`；声明数超过文本行数提前拒绝（每条声明至少占一行，
  超行数必然解析不出来）；`with_capacity` 移除。`AigerError` 新增 `Limit` 变体。
- 测试 +1（四类恶意头部/超大文本全走 `Limit`）；变异门 190 → **192**（新增 2 条，
  trit-circuit **92/92** 实跑全抓）。workspace **637/0**；fmt/clippy/模块预算绿。

### Fixed（复审 P0 批次 1：解析递归护栏，H01–H03 栈溢出封堵）

全面复审（`_handoff/trit-v0.12.0-全面代码复审.md`）的三个**可远程触发的栈溢出**（隔离探针
实测 abort 134）：解析器是递归下降，文本字节上限拦不住深层嵌套——三处分别加护栏，
超限返回 `Limit` 类错误而非进程 abort：

- **H01 Verilog assign 右值**（`verilog/expr.rs`）：拼接 `{a,{a,{…}}}` 此前不计深度
  （只有括号/一元计）→ 纳入 `MAX_RHS_DEPTH`（256）；二元平链构造成左深 AST 后
  `bit_expr`/`synth_v` 递归遍历栈溢出 → 新增 `MAX_RHS_OPS = 256`（每消耗一个运算符计数，
  超 `+1` 即报错）。2 万层拼接、3 万项平链现在都返回 `VerilogErrorKind::Limit`。
- **H02 SDF**（`sdf.rs`）：`parse_node` 对括号嵌套无上限、`collect_sdf` 不拒绝超大文本 →
  新增 `MAX_SDF_DEPTH = 256` 深度护栏与 `MAX_SDF_BYTES = 1 MiB` 字节预算；
  `SdfError` 新增 `Limit` 变体。
- **H03 Verilog 参数表达式**（`verilog/params.rs`）：`ExprParser::factor` 对括号/一元
  递归不设防 → `MAX_PARAM_DEPTH = 256`（`enter_then` 闭包保证两条路径都还原深度）。
- 测试 +5（拼接深度、链上限 +1、预算内链可综合、SDF 深度/字节、参数深括号——其中
  SDF 250 层与预算内链同时验证**护栏不误伤**）。变异门 185 → **190**（新增 5 条，
  trit-circuit **90/90** 实跑全抓）。workspace **636/0**；fmt/clippy/模块预算绿。
- 教训一条：长链测试最初写 3 万项，「阈值 ×40」的变异仍会报 Limit 而存活——改成
  「上限 +1」项后阈值任何放大立即暴露，且不再依赖栈溢出 abort 才判红。

### Fixed（复审修复：平衡三进制 codec 与 trit-float 的溢出/语义缺陷）

外部复审（无 Rust 工具链、按 std 契约推断）+ 本地逐条实测确认后修复；实验层
（`publish = false`）按其建议优先级 H1 → M1 → M2 → M3 落地：

- **`balanced::range(n)` 溢出 panic（H1）**：`3i128.pow(n)` 在 `n ≥ 81` 越界 panic；
  改 `checked_pow`，越界**饱和到 `i128::MAX`**（`range(80) ≈ 7.4e37` 仍精确）。
- **`balanced::decode` 无长度上限（M3）**：超长全零串会按位循环百万次。现在先剥
  前导 `0`（`"00+0"` 与 `"+0"` 同值，行为不变、写入文档），剥后超 41 位直接
  `Overflow`——41 位已覆盖整个 i64，42 位数值必然越界。
- **`TritFloat` 极值 panic（M1，实测比复审多两处）**：`digit_count` 对 `i128::MIN`
  取 `abs()` panic → 改 `unsigned_abs()`；`drop_digit` 的 `(value - remainder)` 对
  `±i128` 极值下溢 → 改 `div_euclid`（数学等价）；指数 `+=` / 相加 / 相减全部
  `saturating`；`Neg` 改 `saturating_neg`。
- **`TritFloat` 加法静默改值（复审 H10，比 panic 更重）**：`x + 0` 对大指数 `x` 曾被
  `saturating_mul` 改写约 72%。对齐溢出时较小项低于任何精度的半个 ulp，**丢弃**
  而不是饱和——`x + 0 == x` 恢复成立。
- **`TritFloat` 的 `Eq` 把 `precision` 算进去且无 `Ord`（M2）**：改为**数值**相等/全序
  （规格化后 `(mantissa, exponent)` 是数值规范形，逐字段比较即数值比较）；
  `from_f64(1.0, 10) == from_f64(1.0, 20)` 成立，补 `PartialOrd + Ord`（对齐溢出按
  符号定序）。
- 测试：codec +2（range 80/81/饱和、剥零/41 位边界/i64::MAX 往返）、float +3
  （`x+0` 恒等、极值不 panic、数值相等与全序含对齐溢出分支）。变异门 178 → **185**
  （1 条随 `saturating` 迁移，新增 7 条；codec 7/7、float 8/8 实跑全抓——其中
  「对齐溢出判成更小」首轮存活，补齐 shift>80 与 `3^40×3^41` 两类断言后判红）。

### Changed（拆最后一个上帝模块：verilog/parse.rs，预算台账清零）

- `verilog/parse.rs` **861 → 471 行**，按职责拆出三个子模块，代码原样搬移（语义零改动）：
  - `build.rs`（119）：`Builder`——assign 右值到门的降级（临时信号、门数预算、表达式综合）；
  - `decl.rs`（195）：端口/线网声明解析（位宽范围、名字列表、`declare_ports`/`mark_used`）；
  - `instance.rs`（108）：模块实例语句（命名/位置连接）。
  `parse.rs` 只留顶层编排（模块扫描、header、语句分发）。
- **模块预算台账清零**：`module_budget_baseline.json` 的 `over_budget` 从 1 件
  （parse.rs 861）变为 **0 件**——全仓 157 个 `.rs` 文件无一超 800 行预算，
  之后任何新上帝对象直接红，不再有登记缓冲。
- 变异门锚点随迁 3 条（`向量声明记成 1 位`/`向量端口只展开 bit 0` → `decl.rs`，
  `assign 的 ^` → `build.rs`），总数不变（178）；`升序 LHS`/`多输入 NAND/NOR` 两条留在
  `parse.rs` 原锚点。顺带把两条双斜杠路径归一化。

### Changed（分支无关化收尾：stride/bmask 编码 + 保守区间 min/max）

- `Simulator` 热循环去掉最后两个结构性分支（`if kind == Const` / `else if arity == 2`）：
  门类型/元数编码进 `CompiledGate.stride`（常量 0、一元 1、二元 3）与 `bmask`
  （二元 3、其余 0），热循环只剩同一条 `lut[a*stride + (b & bmask)]`。ATPG 同机读数
  98.1–102.8 µs（上一组 99.3–106.4 µs 区间内，无回归）。常量槽位只填可寻址的 `lut[0]`、
  不做防御性 `fill`——错编址会立刻读到未填的 `False`，测试抓得住。
- **分支预测失败机制读数**（`benches/core.rs` 新增 branch probe，2×2 表）：
  `match` 求值在随机输入 **7.1–7.3 ns/op** vs 固定输入 **1.6–1.9 ns**（≈4×——分支预测器
  学不会的分支）；查表两种输入持平（0.66–1.05 ns），随机输入下 match/LUT 差 **9–11×**。
  这张表把「图里说的分支预测税」变成了可复算的同机数字。
- `trit-net` 保守区间分析去数据分支：每权重 `if weight >= 0` 改为端点项 `min`/`max`
  各取一次（`w<0` 时区间端点翻转由 min/max 吸收，任意权重都成立）；逐层符号压缩从
  neg/zero/pos 三连分支化成 `sign(x) = (x>0) - (x<0)`（原 `zero_possible` 对 `sign`
  是冗余条件——`high >= low` 恒成立）。同机读数：`analyze 128×128×3`
  **131.96 → 39.9–40.1 µs（≈3.3×）**；`forward` 13.8 → 14.2–14.3 µs（噪声边缘，如实记）。
- `trit-net` 接入首个基准 `benches/net.rs`（前向 + 区间分析的同机读数）。
- 测试：逐点对拍补 Const 门（stride=0 路径）；`trit-net` 新增 2 条（负权重区间必须弃权、
  40 组随机「保守区间不得与精确枚举矛盾」对拍）。变异门 174 → **178**
  （3 条锚点随重构迁移到 stride/bmask 与 min/max 形态，另新增 4 条，逐条实跑判红）。

### Changed（底层：去分支查表 + 硬件 POPCNT 派发）

- `Simulator` 的门求值从「按门类型 9 路 `match`」改成**每门预计算真值表**：
  `CompiledGate` 带上 `lut: [Ternary; 9]`（一元 3 项、二元 9 项），热循环里只剩
  一次下标运算 + 一次查表。ATPG 主路径同机实测 **115.1 / 125.7 / 113.8 µs →
  99.3 / 99.8 / 106.4 µs**（多次取中位，约 13%），`analyze` 在噪声内（~535–558 µs）。
- `Ternary` 加 `#[repr(u8)]` 与 `index()` / `from_index()`，`False = 0`、`Unknown = 1`、
  `True = 2`，LUT 下标 `a*3 + b` 直接来自序号，零转换成本。
- `trit-dot` 的三值/二值点积加**运行时派发**：`std::arch::is_x86_feature_detected!("popcnt")`
  命中时走 `#[target_feature(enable = "popcnt")]` 的 4 路展开内核，否则回退便携单累加器。
  默认构建里 `objdump` 原本**一条 `popcnt` 都没有**（`count_ones` 被展开成约 12 条软件位运算），
  派发后 37 条。同机同构建（Arrow Lake Ultra 7 270K Plus）每 trit 实测：
  1 Mi 派发 **0.0088–0.0092 ns** vs 便携 **0.0128–0.0137 ns**（1.35–1.45×）；
  16 Mi 派发 **0.0103–0.0106 ns** vs 便携 **0.0128–0.0129 ns**（1.22×，已开始吃带宽）。
  纯 4 路展开而不开 `popcnt` 反而慢 12%（软件 popcount 延迟长、展开只增寄存器压力）——
  这条负结果保留在 `dot_words_unrolled` 的 A/B 读数里。
- 二值基线同待遇派发（否则比的不是编码而是谁碰巧拿到指令）：1 Mi **0.0038** vs 便携
  **0.0057–0.0058 ns/元素**；三值/二值比值仍约 2.3×，与「双导轨 2 bit vs 1 bit」相符。
  `f32` 标量 4 累加器 0.14–0.16 ns/元素，三值位并行约 **16–18×** 于它（1 Mi）。
- `trit-dot` 的 `#![forbid(unsafe_code)]` 收窄为 `#![deny(unsafe_code)]`，唯一放开处是
  `popcnt` 模块（两个 `#[target_feature]` 内核），其余代码仍禁止 unsafe。
- 测试：`dot` 新增 3 条（派发 vs 便携逐值一致、各种尾巴长度、4 个 lane 独立贡献），
  `trit-core` 新增 1 条（序号往返 + 9 槽不重叠）。变异门 168 → **174**。

### Changed（精确传播：索引化内循环 + 有上限的并行）

- `propagate_exact` 的枚举不再对每个完成调用 `propagate`（那会为**每个信号**建
  `BTreeMap<String, _>`）；改成复用 `Simulator` 的索引化求值，只分配一份向量。
  同机同电路实测：k=8 **74×**、k=10 **209×**、k=12 **334×**（14.36 ms → 194 µs / 57.8 ms →
  277 µs / 239 ms → 716 µs），`k=8` 的永久基线读数见 `benches/core.rs`。
- 新增 `propagate_exact_with_jobs(..., jobs)`：完成之间独立，按块分给 `jobs` 个线程，
  再按「值不同记 `X`」合并；结果与并行度无关（`sim_tests` 在真分块的 k=12 上对拍串行）。
- **并行度按工作量收敛**：每线程至少 1024 个完成（`MIN_COMPLETIONS_PER_JOB`），
  `jobs = 0` 表示自动。实测 k ≤ 10 串行更快，k=12 四线程 1.6×，k=14 2.2×，k=16 5.4×；
  不设上限时 `available_parallelism` 会在亚毫秒任务上炸出几十个线程，反而更慢（k=12 auto 0.92 ms
  vs 四线程 0.46 ms）。`ExactPropagation` 新增 `jobs` 字段报告**实际**并行度。
- CLI：`trit circuit FILE --assign ... --exact [--jobs N]`，表格输出枚举完成数与实际并行度，
  JSON 增加 `exact` 对象（`exact` / `completions` / `budget` / `jobs` / `unknown_inputs`）。
- 测试：新增 `sim_tests`（串并行等价、**真分块**断言 `jobs == 4`、完成数/预算上报、超预算回退）
  + 3 条 CLI 测试；变异门 139 → **141**（新增 3 条并逐条实跑判红，删 1 条被重写覆盖的旧锚点）。

### Changed（三值拓扑：两趟 BFS + 三值验证证书）

- `TernaryTopology::reachability` 从「逐边松弛到不动点」改为**两趟 BFS**：先沿 `True` 边求
  `True` 集，再从 `True` 集出发沿非 `False` 边求 `Unknown` 集。结果与旧最小不动点逐点一致
  （200 个随机图上与参考实现逐点对拍），但**不再依赖边的插入顺序**。
- 实测（研究探针，未入库）：稀疏随机图 1.7×；长链且边按拓扑序 0.3×（旧实现一轮扫完）；
  同一条链边逆序时旧实现 12,500 轮 / 20.8 ms，新实现 0.10 ms（**199×**）；稠密图 1.1×。
- 邻接表随 `add_edge` 持久维护：第一版每次调用重建邻接表，实测反而慢 3×，已改。

### Added（三值自己的验证结论）

- `TernaryTopology::explain(origin, target) -> ReachEvidence`：三值结论
  `Verdict::{Proved, Undetermined, Refuted}` + `value` + `path` + `unknown_edges`。
  `Proved` 给纯 `True` 路径；`Undetermined` 给一条路径与路径上需要补全的 `Unknown` 边；
  `Refuted` 表示任何补全都不可达。结论是**通过 / 未定 / 否定**，不是二值的「可能」。
- 新增 `trit topology FILE [--origin N] [--to M] [-f json]`：JSON 输入
  （`nodes` / `origin` / `edges`，边值 `0`/`1`/`X`）；不给 `--to` 输出整张传播表，
  给了则输出单点判定与证书。`TernaryTopology` 此前没有任何消费方，现在有了对外入口。
- 测试：9 条库测试（200 组随机图与旧不动点对拍、100 组随机图 explain 与 reachability 一致）
  + 4 条 CLI 集成测试。
- 变异门 136 → **139**：删 1 条已失效的（拆模块后锚点落进测试代码），新增 4 条并逐条实跑判红。
- 变异门 `preflight` 增加规则：锚点落在 `#[cfg(test)] mod ...` 之后直接报错——
  改测试代码不改变产品行为，不该算覆盖。

### Changed（清模块预算债务：5 → 1）

- `verilog/tests.rs` 1094 → **576** + `verilog/hierarchy_tests.rs` 528（层级/参数化测试独立成文件）；
- `trit-cli/tests/cli.rs` 1100 → **459** + `tests/cli_io.rs` 674（每文件自带 `trit_bin`/`run` 助手）；
- `trit-core/src/bits.rs` 801 → **534** + `src/bits/tests.rs` 264；
- `trit-circuit/src/map.rs` 842 → **480** + `src/map/tests.rs` 359；
- 台账从 5 件缩到 **1 件**（`verilog/parse.rs` 861，生产代码，需按职责拆，未做）。
- 测试条数不变（verilog 58、CLI 42+36、core 80、circuit 182）；fmt/clippy 全绿。

### Added（底层补齐：`Ternary` 运算符 + `View::copy_into`）

- `trit-core`：`Ternary` 现在实现 `Default`（`False`）、`From<bool>`、`Not`、
  `BitAnd` / `BitOr`（**Kleene**）、`BitXor`（**经典异或**，任一侧 `X` 则 `X`）。
  Ł3 / B3 的差异仍走 [`Algebra`]，运算符只给三套语义一致的默认值；测试逐条断言。
  这让值层能直接写 `!a | (b & c)`，不必为每个门手写 match。
- `trit-codec`：`View::copy_into(&mut [Ternary])` 顺序批量拷贝——Binary / Packed 走专用循环，
  Sparse 先填 `X` 再回填非零位；`decode` 改用它（不再逐元素 `get`）。
  稀疏 1% 的「整条解码 + 随机取」从 **417 µs → 22.8 µs（18×）**；
  零拷贝视图相对它的优势相应从 242× 收窄到 **11.7×**——视图仍是随机访问最优，物化现在是廉价选项。
- 变异门 166 → **168**，两条新变异按**断言**判红（复核过不是编译错误导致的假红）。

### Added（零拷贝解码 `plan::View`）

- `plan::View::new(encoding, bytes, len)`：只校验、**不分配、不拷贝**；`get(i)` 按需从字节算。
  - `Binary` / `Packed`：O(1) 位/三进制运算；`Sparse`：对升序下标二分，O(log nnz)；
  - `decode` 改为 `View::new(...).iter().collect()`，两条路径共用同一套逻辑。
- 实测（65536 trit，随机取 200 个元素）：

  | 分布 | 整条解码 + 取 | 视图取 | 加速 |
  |---|---|---|---|
  | binary | 57.7 µs | 195 ns | **296×** |
  | packed（1/3 X） | 188 µs | 2.17 µs | 87× |
  | sparse 1% | 417 µs | 1.73 µs | 242× |
  | sparse 4% | 639 µs | 2.13 µs | 300× |
  | sparse 10% | 173 µs | 2.13 µs | 81× |

- **不适用于全量扫描**：视图逐元素 `get` 带边界/分支，整条 `decode` 顺序收集更划算（实测 0.95–1.09×）。
  零拷贝的赢面是「只取少数元素」。
- 测试：视图与 owned 解码逐元素一致、越界返回 `None`、坏缓冲（短/非法字节/长度不符）拒绝。

### Added（按分布选编码：把模拟税按实际情况降下来）

- 新模块 `trit-codec::plan`：给出**字节数最小**的编码，而不是一律 2 bit/trit（双导轨）。
  - `Binary`：整条向量无 `X` → **1 bit/trit**（省 50%）；
  - `Sparse`：非零占比低 → 每个非零 33 bit（u32 下标 + 符号位）；
  - `Packed`：其余 → 1.6 bit/trit（5 trits/byte，利用率 94.9%）。
- 稀疏的赢面是**算术结果**：`33·p < 1.6` ⇒ 非零占比 `< 4.85%` 才选索引；测试按这个阈值双向验证。
- `trit-codec plan` 实测（4096 trit / 双导轨 1024 B）：

  | 分布 | 选中编码 | 字节 | bit/trit | 相对双导轨 |
  |---|---|---|---|---|
  | 纯二值 | binary | 512 | 1.00 | **−50%** |
  | 1/3 是 X | packed | 820 | 1.60 | −20% |
  | 1% 非零 | sparse | 178 | 0.35 | **−83%** |
  | 4% 非零 | sparse | 685 | 1.34 | −33% |
  | 10% 非零 | packed | 820 | 1.60 | −20% |

- 测试 5 条（4 种分布往返、稀疏阈值双向、空向量）；编码/解码可往返。
- 边界：稀疏下标是 u32（大向量可换 varint）；解码是逐元素拷贝，未做原地/借用视图；
  这里只降**存储/带宽**税，不降 CPU 解包税。

### Changed（三值点积优化：CPU 1.65×，GPU 已到带宽上限）

- **CPU**：`pp+nn` 与 `pn+np` 各合成一次 POPC（`(a⁺&b⁺)|(a⁻&b⁻)`、`(a⁺&b⁻)|(a⁻&b⁺)`），
  四组 POPC → 两组。16M 实测 **21.7 → 13.1 ps/trit（1.65×）**；`dot_naive` / `dot_words_naive`
  保留作 A/B 对照。
- **GPU**：同场三内核对比（预热 20 轮 + 5 批中位数）：

  | 变体 | 1M | 16M | 64M |
  |---|---|---|---|
  | A 四组 POPC + 每字 atomic（**默认**） | 21.2 ps | 1.88 ps | 1.069 ps |
  | B 两组 POPC + 每字 atomic | 23.6（−10%） | 1.89（−1%） | 1.070（0%） |
  | C 两组 POPC + workgroup 归约 | 23.4（−10%） | 2.16（−13%） | 1.040（+3%） |

- **结论**：GPU 内核**带宽受限**，合并 POPC 与 workgroup 归约都不带来收益（±10% 内，部分配置更慢）；
  CPU 在同样规模**计算受限**，同一合并直接 1.65×。**同一改动两端结论相反**，必须分设备量。
- **测量方法修正**：GPU 时钟爬坡会让「第一个被测内核」系统性偏慢（同一变体先测 47.6、后测 21.2 ps），
  现在所有内核先预热 20 轮再计时。
- 下一步的唯一有效方向是**减少字节**（1.6 bit/trit 的 5-trit 打包），而不是减少指令；未实现、未测。

### Changed（`trit-gpu`：1 号/2 号编号 + 设备上限 + 四档规模读数）

- `trit-gpu bench` 按约定输出四组：**1 号 = 三值、2 号 = 二进制**，设备分 GPU / CPU，
  并给同设备加速比（1 号 / 2 号）。
- 设备改为按 adapter 的**实际上限**申请：默认 `max_storage_buffer_binding_size = 128MB`，
  64M trit 的 f32 缓冲（256MB）会在 `create_bind_group` 直接 panic。
- 实测（RTX 4060 Ti / Vulkan，5 批中位数）：

  | 规模 | GPU 1 号 | GPU 2 号 | GPU 加速 | CPU 1 号 | CPU 2 号 | CPU 加速 |
  |---|---|---|---|---|---|---|
  | 1M | 33.5 ps/trit | 63.0 ps/元素 | 1.9× | 48.3 | 195.0 | 4.0× |
  | 16M | 5.06 | 36.8 | 7.3× | 51.9 | 528.9 | 10.2× |
  | 32M | 1.76 | 34.4 | 19.5× | 53.4 | 452.4 | 8.5× |
  | 64M | 1.24 | 33.9 | 27.4× | 55.8 | 499.8 | 9.0× |

  （CPU 2 号单位 ps/元素，其余同。）
- **读法**：16M 起三值工作集（0.5 B/trit）开始装进 32MB L2，而 f32（8 B/元素）装不进，
  所以 GPU 比值从 7× 涨到 27×，**超过 16× 的字节比**——这是缓存层级效应，不是指令数变少；
  1M 时两者都在片上、延迟主导，只有 1.9×。

### Added（对比基线规则 + 高压测试门）

- `docs/TERNARY-BOUNDARIES.md` 新增两条硬规则：
  - **§6 任何对比都必须同时给二进制基线**：性能要同机 `f32`/`f64`（或 1-bit 二值）读数，
    精度要同比特预算的二进制上界；**没有基线不算结论**，且必须写明省的是**带宽/存储**还是**指令数**。
  - **§7 高压测试**：位运算 / 编解码 / 浮点边界必须有跨字边界、极端向量
    （全 `+1` / 全 `-1` / 单点 / 交替）与大规模随机的不变量测试；只测小样例不算验证。
- 新增 `tests/stress.rs`：
  - `trit-dot`：极端向量 × 长度 `1/31/32/33/63/64/65/1000`、bipolar 尾部补位、
    3 个种子 × 1M 随机、块级稀疏，以及**二进制基线自检**（朴素 f32 与四累加器互相一致）。
  - `trit-codec`：0..=256 全长往返、10 万 trit 大载荷、256 个字节值分类、2000 个随机 `i64`、
    200 组随机消息 × 4 位置 × 2 幅度的单符号纠错。
  - `trit-float`：6 个精度 × 1000 个值（指数 -40..40）不越 `3^{1-p}`；乘积误差 ≤ 2.5× 上界。
- CI 新增 `stress (high pressure invariants)` job（release 模式跑上述套件）。
- **修掉一个真实竞态**：CLI 集成测试的夹具用 `OnceLock` 的 `get` + `set`，这两步不是原子的——
  两个并发测试会同时看到 `None` 并各自写同一个临时文件，子进程可能读到截断的 JSON。
  改用 `get_or_init`（原子初始化）。这正是此前「ubuntu test 步 exit 101、日志取不到、复跑即过」
  那次 flake 的根因；修复后 CLI 测试连跑 5 轮、每轮 91 条全绿。

### Added（GPU 后端 `trit-gpu`：WGSL `countOneBits`）

- 新 crate `crates/trit-gpu`（`publish = false`）。**GPU 代码在 `gpu` feature 后面**，默认构建不拉
  `wgpu`，CI 的三平台 / MSRV job 不受影响；本地用 `--features gpu` 构建。
- `src/ternary.wgsl`：每 32 个 trit 打成 `u32`（双导轨），`countOneBits` 四组计数累加；
  `src/float.wgsl`：**同一个设备**上的 f32 点积（workgroup 归约），作为公平对照。
- CPU 位并行 `dot_words` 与 GPU 内核逐位一致；测试覆盖 1/31/32/33/100 跨字边界、四种符号组合、
  正负导轨不对称；GPU 测试在没有 adapter 时自动跳过。
- 实测（RTX 4060 Ti / Vulkan，5 批取中位数）：

  | 规模 | GPU 三值 | 同卡 f32 | CPU 位并行 | 加速 |
  |---|---|---|---|---|
  | 16M trit | **2.68 ps/trit** | 35.6 ps/元素 | 50.7 ps/trit | **13.3×** |
  | 1M trit | 40.6 ps/trit | 58.0 ps/元素 | 54.6 ps/trit | 1.4× |

  与内存带宽一致：三值 0.5 B/trit（4 个位平面 / 32 trit）vs f32 8 B/元素 = **16× 字节比**；
  1M 时数据进缓存、dispatch 延迟占主导，GPU 优势消失。
- 测试 4 条 + 1 条 GPU 测试；变异门 163 → **166**（少减一项、`+1` 写错导轨、标量参考改加法）。
- 边界：只测这一个 kernel，不是完整推理；没有与 int8/triton 内核对比；GPU 计时抖动大（已取中位数）。

### Added（发布图门 `scripts/publish_graph.py`）

- 新门：**任何 crate 都不许依赖未发布（`publish = false`）的 crate**。
  - 可发布 → 实验层：发布链断（红）；
  - 实验层 → 实验层：`cargo package --workspace` 会把实验层也一起打包，路径依赖没有版本 ⇒ 同样失败（红）；
  - 实验层 → 可发布：允许（绿）。
- 推论：**实验层必须自包含**；共享代码只能下沉到可发布 crate。`trit-net::format` 里 5-trit 打包
  之所以内联而不是复用 `trit-codec`，就是这条约束。
- 顺带检查可发布 crate 的 crates.io 必填字段（`description` / `license` / `repository`），
  并按依赖打印**发布顺序**（当前：17 个 crate / 11 个可发布）。
- `--self-test` 5 条断言：2 正例（可发布→可发布、实验层→可发布）+ 3 反证（可发布→实验层、
  实验层→实验层、缺 `license`），夹具是真实的两 crate workspace，跑的是真 `cargo metadata`。
- 接入 CI：新 job `publish graph (no dep on unpublished crates)`。
- 由来：本会话真实踩过两次（`trit-net → trit-codec` 让 `cargo package` 失败），现在由门强制。

### Added（三值点积的位并行实现 `trit-dot`）

- 新 crate `crates/trit-dot`（`publish = false`，不属于稳定 API）：
  - **二值（±1）点积**：`dot = n - 2·popcount(a XOR b)`——这才是「XNOR + POPC」能算的东西，
    每个 64 位块 **2 条指令**。
  - **三值点积**：`{-1,0,+1}` 一个 bit 装不下，必须双导轨（2 bit/trit）：
    `dot = Σa⁺b⁺ - Σa⁺b⁻ - Σa⁻b⁺ + Σa⁻b⁻`，每块 **4 AND + 4 POPC = 8 条指令**。
  - **块级稀疏跳过**：整块为 0 才跳过；**元素级稀疏救不了**——64 位块里几乎每块都有非零。
- 实测（1M 元素、同机、20 次平均）：

  | 方式 | 时间 | ns/元素 |
  |---|---|---|
  | 三值位并行 | 22.8 µs | 0.0217 |
  | 二值 XNOR+POPC | 6.4 µs | 0.0061 |
  | f32 朴素（单累加器） | 451 µs | 0.430 |
  | f32 四累加器（公平对照） | 113 µs | 0.108 |
  | 三值 + 块级 90% 稀疏 | 13.8 µs | 0.0132 |
  | 三值 + 元素级 90% 稀疏 | 43.4 µs | 0.0414（**更慢**） |

  三值位并行相对**公平**的 f32 约 **5×**（相对朴素 f32 是 19.8×；「16×」只在拿朴素基线时成立）。
- 结论：**「XNOR+POPC 算三值点积」是误传**。准确说法是「二值点积 2 条指令/64 元素，
  三值 8 条」；GPU 晶体管不是三值的，三值始终是 2 bit 模拟（B 类）。
- 测试 6 条库 + 3 条 CLI；变异门 160 → **163**（二值公式丢系数、三值少减一项、稀疏漏负负项）。

### Added（三值浮点实验层 `trit-float`）

- 新 crate `crates/trit-float`（`publish = false`，不属于稳定 API）：`value = mantissa · 3^exponent`，
  尾数是平衡三进制整数、规格化后最高位 `±1`、按 `precision` 位舍入；实现 `+ - * neg` 与 `f64` 互转。
- 舍入：丢掉最低位时余数只有 `±1`（不到半个 ulp），所以平衡三进制的**最近舍入就是截断**。
- 实测（`trit-float stats`，2000 次随机）：位数 6/12/18/24 的最大相对误差都低于理论上界 `3^{1-p}`；
  **同比特预算下三值的误差上界恒为二进制的 3 倍**（`3^{1-p}` vs `2^{-p·log2 3}`）。
  即三值浮点**没有精度红利**，相同存储白丢约 **1.58 bit**。
- 要匹配 `f64` 的 53 bit 精度，三值需要 **35 位 ≈ 55.47 bit（多 2.47 bit）**。
- 测试 7 条库 + 3 条 CLI；变异门 157 → **160**（舍入改截断、加法恒丢弃小项、乘法指数相减）。
- 边界：浮点精度是**基数无关的 D 类**问题；三值的优势只在整数的基数经济性（`3/ln3 ≈ 2.73` 最小），
  不在浮点精度。未实现次正规数、溢出标志、NaN / ±Inf 的三值表示。

### Added（三值注意力 / 快权重记忆实验层 `trit-attn`）

- 新 crate `crates/trit-attn`（`publish = false`，不属于稳定 API）：
  - **注意力 = 快权重 FFN**：`attention` 与 `ffn_view` 是同一计算的两个写法——第一层权重是 `K`、
    第二层是 `V`，都由输入现场生成，激活是 `exp`，**隐层宽度 = 序列长度**。
    20 组随机用例上逐元素差 `<1e-6`，本机实测最大差 **0.0**。
  - **三值关联记忆（编码器 / 解码器）**：`TernaryMemory` 把「键 → 值」写成整数矩阵 `W += v kᵀ`，
    解码 `W q` 取符号，编码与解码全程没有浮点。
  - `recall_experiment`：32 条键值对、维度 16，**逐分量**召回率
    **三值整数记忆 0.760 / softmax 注意力 0.963 / f32 对照 0.725**。
- CLI：`trit-attn demo [--pairs N] [--dim D] [--seed N]`。
- 测试 7 条库 + 3 条 CLI；变异门 154 → **157**（softmax 不归一化、记忆编码符号写反、三值化不夹边界）。
- 边界：f32 对照**未做能量对齐**（三值键 `|k|²≈2d/3`，f32 均匀键只有 `d/3`），不能据此说三值更好；
  记忆容量随条目数线性退化，没有做归一化/正交化；这是离散玩具模型，不是 Transformer 实现。

### Added（`trit-net`：二进制模型格式 + STE 反向传播）

- **模型格式 `TRN1`**：`magic / version / 层数` + 每层维度 + 权重与偏置按 **5 trits/byte** 打包
  （`{-1,0,+1} ↔ F/X/T` 只是存储映射，不代表 0 是逻辑未知）。113 参数的模型 **26 B**，
  同模型 JSON 是几百字节；非法魔数 / 版本 / 截断 / 尾部多余 / `243..=255` 非法字节全部显式报错。
- **反向传播（直通估计器 STE）**：保留 f32 影子权重，前向用 `clamp(round(w), -1, 1)`，
  梯度直通写回影子权重，训练结束量化回 `{-1,0,+1}`。只支持 1 个隐层的 tanh MLP、`±1` 输入输出。
- CLI：`trit-net train [--epochs N] [--hidden N] [--lr F]`、`trit-net format encode|decode`。
- 读数：majority-of-5（32 样本、6 隐元）300 轮，损失 **0.5 → 0.0027**，
  **三值网络精度 1.00**（影子权重同样 1.00），导出模型 26 B。
- 测试：库 16（格式往返 / 坏输入 / 训练收敛 / 标签与层数校验）+ CLI 7；
  变异门 151 → **154**（不校验魔数、打包长度向下取整、梯度上升）。
- 边界：STE 是近似梯度；只支持单隐层；合成任务上的精度不代表真实数据集。

### Added（三值编解码实验层 `trit-codec`）

- 新 crate `crates/trit-codec`（`publish = false`，不属于稳定 API）：
  - **`pack`：5 trits/byte**。`3^5 = 243 ≤ 256`，密度 **1.6 bit/trit**；1000 trit 实测 200 B
    （双导轨 250 B，省 20%；文本 `0/1/X` 1000 B，省 80%）。`243..=255` 是非法字节，解码显式报错；
    尾部不足 5 个时按长度截断。
  - **`balanced`：平衡三进制整数**（`+0-`）。i64 全域需要 **41 位**
    （`(3^40-1)/2 < i64::MAX < (3^41-1)/2`）；往返覆盖 `i64::MIN/MAX`；空串 / 非法字符 / 溢出分别报错。
  - **`hamming`：GF(3) 上的 `[4,2,3]` 纠错码**。2 个消息 trit → 4 个码字 trit，
    校验位 `p = a + b`、`q = a - b`；穷举 9×4×2 = **72 种单符号错误全部纠正**，最小距离实测 3。
    2 个错误不保证（距离 3 的固有上限），文档写明。
- CLI：`trit-codec pack|unpack|balanced|hamming|stats`。
- 测试 13 条库 + 5 条 CLI；变异门 147 → **151**（打包丢位权、非法字节边界写成 `>`、
  第 4 列误差幅度取错、平衡三进制负号解成 +1）。
- 边界：打包**不是压缩**（固定 1.6 bit/trit，不随分布变化）；Hamming 只保证纠 1 个符号；
  平衡三进制只做整数，不含分数。

### Added（三值小部件实验层 `trit-machines`）

- 新 crate `crates/trit-machines`（`publish = false`，不属于稳定 API），三个独立小机器：
  - **`vm` 三值栈式解释器**：`Jz` 的条件是 `X` 时不猜，两条路都走；结论分成
    「所有路径同值 / 多个可能值 / 截断」，并给出每个输出的路径数。
  - **`ca` 三值一维元胞自动机**（半径 1、总istic）：`3^7 = 2187` 条规则可整片枚举。
  - **`snn` 三值权重脉冲神经元**：`{-1,0,+1}` 突触、整数膜电位、不应期；统计脉冲率与活跃比例。
- CLI：`trit-machines vm|ca|snn`，JSON 输出。
- 研究读数（本机）：CA 在 64 格 × 200 步下 2187 条规则 = 死 274 / 不动点 351 / 周期 690 /
  复杂 872，最大熵规则 #1031（1.5846 bit ≈ log2 3）；SNN 100 神经元时**全局同步驱动会让网络
  退化成一个神经元**（脉冲率只落在离散档位），无驱动时一次 kick 后即静默——要看到传播必须异质驱动。
- 测试 16 条库 + 4 条 CLI 集成；变异门 144 → **147**，三条新变异逐条实跑判红。
- 边界：VM 不做相关性分析（`X & ~X` 仍是 `X`，那是 #90 分析器的活）；CA 只做总istic；
  SNN 是离散时间玩具模型，不能当神经形态硬件的证据。

### Added（三值网络实验层 `trit-net`）

- 新 crate `crates/trit-net`（`publish = false`，**不属于稳定 API**）：三值权重 `{-1,0,+1}`
  的全连接网络，信号是逻辑三值 `{F,X,T}`。`analyze` 枚举 `X` 的所有补全，把每个输出判成
  **确定 / 未定（带两组见证）/ 超预算**，同时给出逐层区间（保守）结论用于对照。
- CLI：`trit-net infer MODEL.json --inputs 1,X,0 [--budget N] [--json]`。
- 研究读数（本机原型，12→8→1，单种子合成任务）：f32 94.75% → 三值权重 84.75%
  （非零输出 85.45%），参数 452 B → 28 B；输入含 `X` 时精确解析 k=1 覆盖 70.3%/确定时精度 90.7%、
  k=4 19.8%/94.9%、k=8 0.7%/100%；而逐层区间只敢确定 0–3%，**假 X 占 95.7–100%**
  （远高于电路的 28.7%，因为区间误差逐层放大）。
- 测试 8 条（取消相关性、见证可复算、预算边界、超预算不猜）+ 4 条 CLI 测试；
  变异门 141 → **144**，三条新变异逐条实跑判红。
- 边界：只做前向解析、不含训练；精确解析是 `2^k` 枚举（k > 12 直接报超预算）；
  权重三值是**量化**（B 类税），只有 `X` 语义是本仓的 A 类能力。

### Added（X-Pessimism 分析）

- `trit_circuit::circuit::analyze_x_pessimism`：把单遍传播给出的 `X` 分成**真 X**与
  **悲观 X**。真 X 给出一对见证赋值（一个补全得 0、一个得 1）；悲观 X 给出可解析的确定值
  （不可满足的那一边就是证明）。
- 用 SAT（`varisat`）查询「输出能否为 0 / 能否为 1」，把 `X` 主输入当自由变量；
  不再需要 `2^k` 次完整传播，`propagate_exact` 的 4096 预算不再限制验证规模。
- `support` = 结构扇入锥里的 `X` 主输入（保守可达集，不是最小反例）。
- `Const X` 无法用二值编码表达，返回 `CircuitError::NonBinaryConstant`。
- CLI：`trit circuit FILE --assign a=X,b=0 --pessimism [-f json]`；JSON 含 `kind`、
  `resolved`、两个 `*_witness`、`support` 与 `sat_calls`。
- 测试：7 条库测试（重汇聚互补、mux 数据相同、见证代入复算、锥隔离、已知输出不发 SAT、
  `Const X` 拒绝、60 组随机电路与 `propagate_exact` 逐输出对拍）+ 3 条 CLI 集成测试。
- 变异门 133 → **136**（单边可满足判成 1、跳过 X 判定、支持集不回溯扇入），三条逐条实跑判红。
- **修变异门的跨平台缺陷**：18 条锚点把 `file` 写成反斜杠路径，Linux 上（周常全量门）
  直接 `FileNotFoundError`。统一为正斜杠，并在跑测试前加 `preflight`：文件缺失、锚点出现
  次数 != 1、路径含反斜杠一次性报全并退出，不再跑一半才崩。
- 实测（研究用探针，未入库）：200 个随机电路 / 800 个输出里，单遍的 383 个 `X` 有
  **110 个（28.7%）是悲观 X**。

### Changed（去上帝模块 + 门禁加强）

- **`trit-schedule` 去上帝模块**：`checkpoint.rs`（1095 行 / 82 个函数）拆成
  `checkpoint/{mod,file,memory,index,lock}.rs` + `tests.rs`；`runtime.rs`（988 行）拆成
  `runtime/{mod,types,execute}.rs` + `tests.rs`。公开路径不变（`trit_schedule::checkpoint::*`、
  `trit_schedule::runtime::*` 与 crate 根重导出均保持），无新增公开 API。
- **新门：模块预算**（`scripts/module_budget.py`）：单文件 800 行预算；超预算必须登记在
  `scripts/module_budget_baseline.json`，且台账**只允许变短**——变胖、变瘦却没收紧、
  文件已降到预算内却没移除、台账里的文件已删除，四种都判红；`--update` 拒绝任何增长。
  当前登记 5 件：`map.rs` 842、`verilog/parse.rs` 861、`verilog/tests.rs` 1094、
  `cli/tests.rs` 1100、`bits.rs` 801。
- **门的自证**：`--self-test` 用脚本自己的 `LIMIT` 生成夹具，10 条断言（正例 5 / 反证 5），
  逐条覆盖上面四种红法与 `--update` 的两条路径。
- **CI**：新增 `module budget (god objects)` job（先跑自测再查台账）；`mutation` 新增 PR 触发，
  只跑改动过的 crate（`mutation_check.py --pkg`），周常全量保留。
- 变异门仍为 133 条：拆分只改路径不改语义，锚点逐条重定位并验证唯一命中。

### Added（0.12 静态三值调度）

- 新增 `trit-schedule`：静态 DAG 调度，支持 `True` / `False` / `Unknown` 依赖；
  `Unknown` 直接延迟并沿必需依赖传播；输出 worker、start/finish、makespan、关键路径。
- 多 worker 事件驱动列表调度；高优先级先排，确定性 tie-break。
- CLI `trit schedule tasks.json --workers N`（table/json/csv/markdown）；`sansheng` 重新导出。
- 测试：链、并行、优先级、未知依赖传播、可选依赖、环/重复/未知引用拒绝、JSON 输入。
- 变异门 96 → 100（未知依赖、延迟传播、关键路径、优先级排序）。
- 这是静态调度 MVP；runtime/抢占/取消/故障恢复不在本轮。

### Added（0.12 runtime）

- `trit_schedule::run_runtime`：依赖驱动的 worker runtime；`RuntimeTask` 闭包、
  `CancelToken` 协作式取消、失败/取消后停止派发、`completed/failed/cancelled/deferred` 报告。
- 不强制抢占运行中的任务；任务需自行轮询 token。
- 测试：依赖顺序、双 worker 重叠、join 等待全部前置、失败后不派发新任务、取消未启动任务、
  未知依赖、panic 转失败、环/零 worker 拒绝。
- 变异门 100 → 103（取消、依赖传播、失败停止）。

### Added（0.12 资源配额）

- `Resources { cpu, memory }` + `RuntimeConfig::with_capacity`；任务 `with_requirements`。
- 严格优先级准入：队首放不下时等待；运行中任务不抢占。
- 超过容量的单任务直接 `Unschedulable`；报告新增 `peak_usage`。
- 测试：6+6 超过 10 时串行、超容量拒绝、配额下高优先级先跑。
- 变异门 103 → 106（容量、占用累计、超容量拒绝）。

### Added（0.12 可恢复任务协议）

- 新增 `ResumableTask::step`：步骤边界 `Yield` + 任务对象内检查点状态。
- `run_resumable` 单线程步进式参考执行器：高优先任务在步骤之间抢占低优先任务，
  低优先任务放回 ready 但状态保留，之后继续；资源配额同样生效。
- 报告 `preemptions`、`steps`、`peak_usage`；取消/失败后不执行新 step。
- 测试：抢占后从原步骤继续、优先级顺序、取消、失败/未知依赖、环/超容量。
- 变异门 106 → 109（抢占状态、优先级过滤、失败清理）。

### Added（0.12 threaded resumable + metrics）

- `run_resumable_threaded`：worker 在 `Yield` 边界抢占，任务对象保留；资源/优先级/取消沿用协议。
- `RuntimeMetrics` 统一指标：`started`（按任务去重）/`completed`/`failed`/`cancelled`/
  `deferred`/`preemptions`/`steps`/`peak_usage`；闭包 runtime 与 resumable runtime 都有。
- 测试：线程抢占后状态续跑、双 worker 重叠、取消/指标、单线程参考仍通过。
- 变异门 109 → 112（线程抢占状态、优先级过滤、started 去重）。

### Added（0.12 metrics export）

- `RuntimeMetrics::to_prometheus()` / `to_prometheus_with(prefix)`：Prometheus/OpenMetrics
  文本导出；报告提供 `metrics_prometheus()` 便捷方法；空前缀回退到 `trit_schedule`。
- 导出字段与 `RuntimeMetrics` 一致；峰值拆为 `peak_cpu` / `peak_memory`。
- 测试 `prometheus_exports_all_fields`；变异门 112 → 114。

### Added（0.12 checkpoint protocol）

- `CheckpointTask::save_state` + `TaskRestorer::restore`：可序列化状态协议，编码格式由任务决定。
- `Checkpoint` 带 `version`，`CheckpointStore` 按任务 id 覆盖保存/读取/删除/恢复。
- `CheckpointedTask` 包装器：每次 `Yield`/`Done` 后写入共享存储，可直接用于现有
  单线程/线程 runtime。
- 测试：检查点往返、存储覆盖/删除、版本拒绝、runtime 每步落盘并恢复。
- 变异门 114 → 117（版本校验、空状态、恢复校验）。

### Added（0.12 checkpoint files）

- `FileCheckpointStore`：版本化目录 `<root>/v1/<hex(task_id)>/<generation>.json`；
  临时文件 + rename 到新代文件（不覆盖），加载取最新代；`prune`/`remove`/`len`。
- 任务 id 十六进制转义，路径分隔符/unicode 不落到文件名；临时文件不参与加载。
- 测试：往返/代号递增、id 转义、prune/remove、版本拒绝；crate 33 → 36。
- 变异门 117 → 120（保存版本校验、加载最新代、prune 保留）。

### Added（0.12 checkpoint index）

- `CheckpointIndex` / `CheckpointEntry`：扫描 `v1/<hex>/`，列出 task id、最新代、状态字节数；
  按 task id 排序，只读元数据，不加载状态。
- `FileCheckpointStore::index()` / `task_ids()` / `index_json()`；`CheckpointIndex::get()`。
- 测试：多任务排序、unicode/slash id、最新代、空索引；crate 36 → 37。
- 变异门 120 → 123（id 解码、排序、state_bytes）。

### Added（0.12 checkpoint lock）

- `CheckpointLock`：`try_lock()` 原子创建 `v1/.lock` 目录，`Drop` 自动释放；
  `lock_timeout()` / `lock()`（默认 5 秒）带超时，占用时返回 `LockBusy`。
- 不自动包裹 `save`/`load`；调用方在临界区显式持有，避免改变现有单写者调用路径。
- 测试：排他、超时、释放后可重新获取；crate 37 → 38。
- 变异门 123 → 125（已存在分支、Drop 释放）。

### Added（0.12 checkpoint resume）

- `CheckpointSource`（内存/文件存储实现）+ `TaskFactory` + `ResumableSpecDef`。
- `restore_specs(defs, source, restorer, factory)`：有检查点则恢复，没有则新建；
  返回值可直接交给 `run_resumable` / `run_resumable_threaded`。
- 测试：内存源从第 3 步恢复只再走 7 步、文件源恢复、无检查点时用 factory。
- 变异门 125 → 127（忽略检查点、无检查点不建任务）。

### Added（0.12 exact propagation）

- `propagate_exact`：相关性感知的精确 X 传播。把 `X` 当未知 0/1，枚举完成并合并可能值；
  `a & ~a`（`a=X`）得到 `0`，修正单遍 Kleene 的假 X。
- 支持部分赋值（缺省主输入按 `X`）；未知输入 `k` 个需要 `2^k` 次完成。
- 超预算时回退到单遍保守结果，`exact=false`、`completions=0`；`DEFAULT_EXACT_BUDGET=4096`。
- 测试：重汇聚相关性、部分赋值、遮蔽、超预算、全已知输入；circuit 166 → 171。
- 变异门 127 → 130（冲突合并、预算、缺省 X）。

### Added（0.12 tri-state / ternary topology）

- `TriState`（0/1/Z）与 `TriStateConflict`：`Z` 是总线单位元，0/1 同时驱动报冲突；
  `TriStateBus` 逐位解析并校位宽；`to_ternary()` 把 `Z -> Unknown`。
- `TernaryTopology`：边带 True/Unknown/False 的有向图三值可达性；路径取 min、节点取 max，
  False 边断开；固定点迭代处理环。
- 测试：解析表/冲突、字符与 Ternary 转换、总线逐位/位宽、True 链、Unknown 传播、False/环；
  `trit-core` 67 → 73。
- 变异门 130 → 133（Z 单位元、总线宽度、拓扑 min/max）。

## [0.11.0] - 2026-10-23

0.10 与 0.11 合并发布：并行/共享 CNF、SDF per-instance、Verilog 参数化、CLI 硬超时，
以及 0.10 的桥接 SAT、映射面积、STA I/O 约束、层级/向量 Verilog。

### Changed（发布工程）

- workspace 版本 0.9.0 → **0.11.0**；新增每周 `quality` 工作流：`cargo audit` +
  8 个 fuzz 目标各 500 次 smoke。

### Added（SDF per-instance）

- **门实例元数据**：`Gate.instance`（`u1` / `u1.u2`），层级展开时写入；JSON 可选字段，
  顶层门为 `None`。
- **SDF 实例对齐**：`parse_sdf_instances` / `timing_model_from_sdf_for`；按 `INSTANCE` 匹配
  `Gate::instance`（容忍 `top.u1` 前缀），实例最坏 IOPATH 优先于 celltype；匹配不到回退 celltype。
- **STA**：`TimingModel::instance_delays` + `arc_delay_for_gate` / `output_arrival_for_gate`；
  同一实例的 rise/fall/条件弧仍合并为一个最坏值，不做引脚级差异。
- 测试：两实例 2/5 周期分别到达 y1/y2；SDF 前缀匹配；CLI `timing --sdf`。
- 变异门 94 → 96（不按实例覆盖、实例路径只做全等）。

### Added（0.11 硬超时）

- **全局 `--timeout-ms N`**：CLI 重新执行自身为子进程，超时 `kill` 并退出 **124**；
  子进程通过 `TRIT_TIMEOUT_CHILD=1` 跳过包装，避免递归。可兜住 ATPG/验证/REPL 等任意子命令。
- 这是进程级硬超时，不是 varisat 单故障取消；库 API 仍无中断句柄。
- 测试：`repl` 挂起时 200 ms 超时，断言退出码 124 且 5 s 内结束。

### Added（0.11 Verilog 参数）

- **参数化模块**：模块头 `#(parameter W = 8, ...)`、实例覆盖 `#(.W(4))` / `#(4)`；
  参数表达式支持 `+ - * / %`、括号与前置负号，用于位宽/下标/实例连接。
- 解析前做源码特化：每个参数组合一个唯一模块副本（上限 256），再由现有解析/层级展开处理；
  根模块保留原名，子模块名带 `__pN`。
- 明确限制：不支持模块体 `parameter`/`localparam`、`signed`/实数/字符串参数、参数化端口类型。
- 测试：默认值、命名/位置覆盖、父参数下传、同模块多参数实例、算术表达式、未知覆盖/体参数拒绝。
- 变异门 90 → 94（忽略覆盖、不替换、不归一化位宽、位置覆盖错位）。

### Added（0.11 共享 CNF）

- **`atpg --sat --shared-cnf`**：每个 worker 只建一份 good/faulty CNF，每个故障一个 selector；
  mux 链注入引脚常量，顺序 at-most-one 保证一次只激活一个故障。按 512 个故障切块，
  避免单 solver selector 过多导致的病态慢例。
- 默认逐故障路径不变（见证稳定）；共享路径覆盖/可测性一致，但见证向量可能不同。
- 10k 门 / 40k 故障、8 线程：**2.6 s**；3000 门 0.26 s；800 门 0.08 s（本机 24 线程）。
- 测试：共享路径与逐故障在随机小电路上对拍覆盖率/可测性/可观测性，并用 `verify_sat_patterns`
  验证测试向量真正检出故障；CLI `--shared-cnf` 覆盖率对拍。
- 变异门 87 → 90（候选不注入、mux 选反、不假设 selector）。

### Added（0.11 SDF-lite 互操作）

- **SDF 读取**：`trit_circuit::sdf::{parse_sdf, timing_model_from_sdf}`；`trit timing` /
  `trit check` 增加 `--sdf FILE`（与 `--library` 互斥）。只取 `CELL/CELLTYPE/DELAY/ABSOLUTE/
  IOPATH`，rise/fall 与 `min:typ:max` 取最坏值、`COND` 合并、向上取整；按 celltype 聚合，
  `TIMESCALE` 不换算，未知 celltype 忽略，无可识别延迟时报错。
- 测试：语法/缺延迟/负值拒绝、跨实例与条件弧最坏值、`min:typ:max`、CLI `timing`/`check`。
- 变异门 83 → 87（新增：取最小值、floor、跨实例取最小、放行负值）。

### Added（0.11 并行 SAT）

- **并行 ATPG / 桥接 SAT**：`generate_tests_sat_with_jobs` 与
  `generate_bridge_tests_sat_with_jobs`；CLI `atpg --sat --jobs N`、`bridge --sat --jobs N`
  （`0` = 自动，`1` = 串行）。按故障/桥接对分块，`std::thread::scope` 独立 CNF/solver，
  按原顺序合并，输出与串行逐字段一致；wasm32 强制串行。
- **策略断言并行**：`trit-core::parallel` 下沉为公共并发原语；`Policy::verify_with_bdd_config_and_jobs`、
  CLI `verify --jobs N`。前提只检查一次，断言顺序不变。
- **减少重复工作**：SAT CNF 复用调用方算好的拓扑序，`Cnf::new_with_order`；不再每个故障重跑
  `validate` / `topological_order`。
- 测速（本机 24 线程）：800 门串行 6.6 s → 2.5 s（拓扑序复用），`jobs=8` 0.37 s（并行 6.97×）；
  3000 门 34.4 s → 4.68 s（7.36×）；2000 条策略断言 0.19 s → 0.07 s。
- 测试：并行/串行逐字段对拍（ATPG、桥接、策略、CLI JSON）、顺序与错误传播。

### Added（0.10 STA I/O 约束）

- **时序 I/O 约束**：`TimingConstraints { input_arrival, output_required }`（JSON，拒绝未知字段/名字）；
  `analyze_timing_with_constraints` 与 `critical_paths_with_constraints`；`trit timing` / `trit check`
  增加 `--constraints FILE`，`--paths` 从输入到达时间起算。未列出的输入按 0、输出按 clock
  （省略 clock 时按关键路径），保持旧行为。
- 测试：到达/required/slack、逐输出 required、未知名字拒绝、JSON 往返、CLI `timing`/`check`。
- 变异门 80 → 83（新增：忽略输入到达、忽略逐输出 required、`--paths` 忽略输入到达）。

### Added（0.10 桥接 SAT + 映射面积）

- **桥接故障 SAT**：`trit bridge --sat` 对每个桥接构造 good / faulty CNF，要求两端在 good 下
  取值不同且至少一个主输出不同；SAT 给见证，UNSAT 证明不可测。支持 **≤64 主输入**；
  默认枚举路径仍为 ≤10。`BridgeReport` 新增公开字段 `sat`；不做 X 压缩。
- **映射面积**：`CellTiming` 支持 `{delay, area}` 与 `{delay, arcs, area}`；
  `CellLibrary::area`；`--area-first` 改为按 area 选择（省略按 1，旧单元库行为等价于单元数最少）。
- 测试：桥接 SAT 与枚举逐桥接对拍、20 输入、CLI `--sat`；映射 area 选择与 JSON 往返。
- 变异门 76 → 80（SAT 锚点迁移到共享 CNF，新增桥接 3 条 + 映射 1 条）。

### Added（0.10 SAT ATPG）

- **SAT-based ATPG**：`trit atpg --sat` 对每个门引脚故障构造 good / faulty CNF，
  要求至少一个主输出不同；SAT 给出测试向量，UNSAT 证明不可测（`UntestableReason::Proven`）。
  上限 64 个主输入（`Fault.witness` 仍为 `u64` 掩码）；`GateKind::Xor` 按代数编码
  （Ł3→OR，K3/B3→AND）。不做 X don't-care 压缩，也不证明测试集最小；未设单故障求解超时。
- `AtpgReport` 新增公开字段 `sat`（外部结构体字面量需补 `..Default::default()` 或改用构造器）；
  `CircuitError` 新增 `Sat` / `NonBinaryConstant`；`trit-circuit` 增加 `varisat` 依赖。
- 测试覆盖：随机小电路与枚举逐故障对拍（三种代数）、20 输入、恒 1 门的 UNSAT、
  常量 X 拒绝、65 输入上限、CLI `--sat`。
- 变异门 72 → 76（新增 4 条 SAT 规则）。

### Added（0.10 层级）

- **Verilog 多模块实例**：`parse_verilog` 现在解析一个文件里的多个 `module` 并做层级展开。
  连接支持 `.port(signal)` 与按端口顺序的位置连接；端口必须全部连接，重复连接报错；
  未知模块、递归、多顶层、重复实例名都返回错误。
- **层级模式支持 `[N:0]` 向量端口**：整条 bus 按位展开；同方向时子模块 `p[i]` 对应父模块 `a[i]`，
  跨方向按位序映射；宽度不匹配、把向量端口接到单比特、把标量端口接到整条 bus 都明确报错。
  单模块且无实例时保留原有完整向量行为。
- **`[0:N]` 升序向量范围**：解析、导出与 JSON 都保留方向；Verilog 位序（左界=MSB）贯通
  `assign`、拼接与层级跨方向连接。其它范围（非零下界）明确拒绝。
  `Bus` 新增公开字段 `ascending`（外部结构体字面量需补 `..Default::default()` 或改用构造器）；
  网表 JSON `format` 1 → 2，旧 `format: 1` 仍接受并按降序解释，更高版本拒绝。
  内部 `wire [N:0]` / `[0:N]` 声明也写入 `Circuit.buses`（此前只记录端口），导出按声明宽度与方向重组。
- 展开后的内部信号名加 `实例名__` 前缀；与用户信号冲突时显式报错，不静默复用。
  门数上限 10 万、嵌套 256 层，均在展开过程中检查。
- `hierarchy` 模块新增 17 条回归，另加 6 条 `[0:N]` 升序回归（解析/导出/JSON、位序、
  跨方向赋值、字面量、内部 wire、层级跨方向）。
- 变异门 62 → 72（新增 10 条：hierarchy 6 条 + 升序位序 4 条）。

### Docs

- **三进制四类边界**：新增 [docs/TERNARY-BOUNDARIES.md](docs/TERNARY-BOUNDARIES.md)，
  把原生三进制红利（A）/ 二进制模拟税（B）/ 生态路径依赖（C）/ 基数无关复杂度（D）
  分开；README「范围」与 DESIGN「非目标」已引用。后续结论、评审与路线图必须先归类。

## [0.9.0] - 2026-10-23

结构模型与 Verilog 前端增量：向量元数据、尺寸常量、JSON 版本；含 `Circuit` 新字段
（minor 允许破坏性调整）。

### Added（0.9 结构模型）

- **向量元数据**：`Circuit.buses` + `Bus { name, width }` 保留 `[width-1:0]` 声明；
  Verilog 解析填充，导出优先按声明宽度重组（允许只建模部分位），JSON 往返保留；
  空映射不序列化，旧 JSON 可直接读取。`CircuitError` 新增 `InvalidBus` / `BusNameConflict`。
  注：`Circuit` 新增公开字段，外部结构体字面量需补 `..Default::default()`。
- **Verilog 尺寸常量**：`<width>'<b|o|d|h><digits>`，支持 `_` 分隔与 b/o/h 的 `x`/`z`，
  不足高位补 0、超出截断高位；十进制常量限 128 位。尺寸常量可参与拼接与按位运算。
  变异门 59 → 62。
- **网表 JSON 版本字段**：`to_json` 写入 `format: 1`；旧 JSON（无 `format`）按 0 接受，
  更高版本明确拒绝。

## [0.8.2] - 2026-10-23

硬化补丁：修掉 Circuit 的静默常量默认，补齐结构校验，收紧 CI 与模块边界；无新增功能。

### Changed（0.8.2 硬化）

- **Circuit 结构不变量**：`validate` 拒绝常量门缺 `const_value`、非常量门带 `const_value`、
  空信号名与重复主输出；`Circuit::validate` 公开。`CircuitError` 新增四个变体
  （`ConstValueMissing` / `ConstValueUnexpected` / `EmptySignalName` / `DuplicateOutput`）。
- **`trit-bdd` 去上帝模块**：`lib.rs` 1059 行拆成 `encode` / `order` / `equiv` / `quantify`；
  公开路径不变。
- 所有 crate 启用 `#![forbid(unsafe_code)]`（当前全仓无 `unsafe`）。
- CI 新增 `cargo package --workspace --no-verify --locked` 可发布性门。
- 变异门 53 → 56（新增 Const 取值/空信号名规则）。

## [0.8.1] - 2026-10-23

评审修复收尾 + 三项芯片侧能力 + 按模块拆分上帝文件；三值语义不变。
本版含 **API 变更**（错误类型与若干返回 `Result` 的入口），升级前请读下面的 Changed 段。

### Changed（评审修复第二批，含 API 变更）

- **Verilog 错误结构化**：`VerilogError` 从 `Message(String)` 改为
  `{ kind, line, message }`，`kind ∈ Syntax / Unsupported / Limit / Export`；
  body 语句错误带 1 起行号（注释剥离保留换行），导出错误归 `Export`。
- **拆掉 circuit 模块环**：`validate` 独立到 `circuit/validate.rs`，`sim` 不再依赖 `analyze`；
  `sim → validate → graph`、`analyze → sim/validate/graph` 单向依赖。
- **ATPG 去上帝模块**：`atpg.rs`（1042 行）拆成 `atpg/{mod,compact}.rs` + `tests.rs`；
  `generate_tests` / `AtpgReport` / `CoverageStat` / `TestPattern` 公开路径不变。
- **Circuit 去上帝模块**：`circuit.rs`（1493 行）拆成 `circuit/{types,sim,analyze,graph}.rs`
  + `tests/test_support`；`Circuit`/`CircuitError`/`analyze`/`propagate` 等公开路径不变。
- **CLI 去上帝模块**：`trit-cli/src/main.rs`（2091 行）拆成 `commands/` 下 15 个命令模块，
  main 只保留 CLI 定义、分发与共享助手（约 590 行）；命令行为与参数不变。
- **Verilog 去上帝模块**：`verilog.rs`（1537 行）拆成 `verilog/{mod,expr,parse,export}.rs`
  + `verilog/tests.rs`；`parse_verilog` / `to_verilog` / `VerilogError` 公开路径不变。
- **设计去重**：ATPG 与桥接故障的精确/贪心集合覆盖抽到私有 `coverage` 模块，
  `TestPattern` 抽到 `pattern` 模块；`bridge` 不再直接调用 `atpg` 的私有实现，
  两套覆盖策略不再各写一份。公开 API 路径保持不变（`atpg::TestPattern` / crate 根导出）。
- 解析器在构造期扣预算：右结合蕴含链改为迭代收集再右折，扁平二元链按实际 AST 深度拒绝；
  新增输入字节上限（1 MiB）与节点预算错误；`Parser::parse_with_limits` 可指定预算。
- `Evaluator::assignment_count` / `chunk_planes` / `evaluate` 改为可失败（`Result`），
  `3^n` 用 checked 计算，新增 `Limits::max_rows` 与 `evaluate_within`。
- `trit_eval::{truth_table, equivalent}`、`trit_belnap::{table, equivalent}`、
  `Circuit::combination_count` 改为返回 `Result`；`analyze` 在 `3^n` 循环前拒绝 >16 个主输入。
- `TriVec` 的 `PartialEq/Eq` 改为按值比较（全零 unknown 平面等同 `None`）；写入清除最后一个
  `Unknown` 时规范化平面；新增 `try_from_binary` / `try_get` / `try_set` / `try_word`。
- `BitPlane` 的 `with_bit` / `bit` 明确为环绕语义，新增 `checked_with_bit` / `checked_bit` /
  `TriBit::checked_get`。
- `Ternary::{is_false,is_true}` 文档改为精确匹配（`X` 两者皆假）；新增 `Expr::try_var` 拒绝
  保留常量 token/非法标识符，`Display` 不再暗示任意变量可无损往返。
- 赋值编号：新增 `try_assignment_index` / `try_decode_assignment`（重复变量、超出 `3^n` 域、
  u64 溢出可区分）；编码不再因末位多余的 `3^n` 计算而过度拒绝。
- 策略：展开时按节点/深度预算（`ExpandBudget` / `DefinitionDepth`），完整定义图（含未引用环）
  先预检；`binary` 只允许原始变量，定义名拒绝保留 token，重复 binary 报错；
  上层错误携带 `LimitError` / `SymbolicError` / `BddError`，不再压成字符串。
- BDD 预算明确为软限额（每次运算返回后检查，峰值可超）；新增 `Quantified::try_is_valid` /
  `try_counterexample`；README 说明硬隔离需进程/worker。
- 策略反例文档改为“完整赋值模型，不做最小化”。
- `CellLibrary.cells` 的值类型改为 `CellTiming`（`Delay(u32)` 或 `Arcs { delay, arcs }`）；
  `TimingModel::from_json` 返回 `TimingModelError`，并做键归一化与弧数校验。

### Fixed

- 评审修复（第一批）：Verilog 括号倒置不再 panic；多输入 NAND/NOR 改为“先合并再反相”；
  XNOR 不再双驱动；临时信号名全模块唯一；强加 `Xor` 导出 Verilog 明确拒绝；
  `check` 无法执行覆盖率检查且用户指定阈值时 FAIL；单元库键规范化并拒绝重复；
  时序/映射累计改用 u64 检查，溢出返回错误；ATPG X 位改为联合填充验证；
  `minimum_cover` 预算耗尽不再宣称最优；`decode_binary` 超过 64 输入返回错误。

### Added

- ATPG X 合并压缩：两个向量若存在一种填充能覆盖“删掉它们后会失去”的唯一故障，就合成一个；
  `inputs` 仍给出具体 0/1 填充，CSV 导出即用该填充。
- `AtpgReport::x_coverage`（含 X 向量覆盖的可测故障比例）与 `merged`（压缩省下的向量数）；
  表格、JSON、WASM 同步输出。
- 多比特 Verilog 子集：`input/output [N:0]` 向量端口按位展开为 `a[3]`… 单比特网；
  支持位选 `a[3]`、拼接 `{a, b}`、等宽逐位向量赋值 `assign y = a & b;`，
  导出时按连续位重新分组为向量端口。只支持 `[N:0]` 组合逻辑；拒绝 `[0:N]`、复制语法、
  位宽不匹配、非连续向量位。CLI `netlist --from verilog --to verilog` 可往返。
- 桥接故障 ATPG：`BridgePair` / `BridgeMode`（wired-AND、wired-OR）→ `generate_bridge_tests`，
  在二进制向量上判定短路可测性并给出覆盖向量；CLI `trit bridge` 支持 `--pairs`/`--max-pairs`/JSON。
  空列表与未知网络直接报错，规模超限拒绝；逻辑级模型，不建模模拟/布局效应。
- Liberty-lite 引脚弧：`TimingModel.arcs`（按 `A1`/`A2` → `Y`）与 `CellLibrary` 的
  `{"delay": N, "arcs": [...]}` 写法；`trit timing` / `check` / `map` 按输入引脚弧计算到达时间，
  慢弧会改变映射选实现。键大小写归一化；未知门、规范化重复、弧数与元数不符都报错。
- 复审测试缺口（§3.3）：`trit-core/tests/structure.rs` 用 proptest 生成不同形状的 AST，
  验证 `to_infix → parse` 的结构/深度/节点数守恒、扁平链与右结合蕴含链的深度边界、
  任意文本不 panic；`trit-policy` 新增展开预算精确性/深度单测与稳定 smoke。
- fuzz 目标补到 8 个：新增 `expr`（解析往返）、`policy`（小策略验证）、`netlist_json`
  （JSON 往返 + 坏电路导出）；`fuzz/Cargo.toml` 的 `trit-circuit` 依赖路径修正为
  `../crates/trit-circuit`。
- ATPG 端到端回归：用 `generate_tests` 实际产出的覆盖向量验证 don't-care 的每个联合填充
  仍检出故障，补上报告 §3.1 指出的“只证明 helper”缺口。
- AIGER ASCII 子集读写：`parse_aiger` / `to_aiger`（`L = 0` 组合逻辑，AND + 取反编码）；
  CLI `netlist --from/--to aiger`；0/1 激励上的往返等价。
- BLIF 子集读写：`parse_blif` / `to_blif`（`.model/.inputs/.outputs/.names`，拒绝 `.latch`）；
  CLI `netlist --from blif` / `--to blif`；往返等价在 0/1 激励上验证。
- Verilog `assign` 支持 `^` / `~^` / `^~`、括号与常量字面量，按 Verilog 优先级解析；
  `^` 映射为经典异或（`XorClassic`），不是强加三值 `Xor`。
- 负载感知时序：`TimingModel.load_delay`（每个扇出追加延迟）进入 `analyze_timing` / `critical_paths`；
  `to_cells_with_load` 在选实现时按扇出计负载；CLI `timing` / `check` / `map --load-delay`。
- fuzz：`fuzz/` 下 cargo-fuzz 目标（netlist / Verilog / ATPG）+ CI 可跑的稳定 smoke；
  `scripts/bench.sh` 与 `trit-circuit` 的 `core` bench（解析/时序/映射/ATPG）。

## [0.8.0] - 2026-10-22

芯片侧能力的一批新入口：结构化 Verilog、STA-lite、技术映射（NAND-only 与时序驱动）、
设计检查流水线、X-aware ATPG；三值语义不变。

### Added

- 时序驱动技术映射：`CellLibrary` + `to_cells`，每门在直接单元与 NAND 展开间按到达时间选实现
  （最长路径 DP，关键路径最优）；`MapStrategy::Area` 优先单元数。CLI `map --cells` / `--area-first`。
- `trit atpg --format csv`：`pattern,<输入…>,covered`，便于导出给外部仿真器。
- `trit_circuit::netlist`：`parse_netlist` / `to_netlist` 从 CLI 下沉到库（可往返）。
- WASM `atpg_js(netlist, algebra)`：浏览器里直接跑 ATPG，返回 JSON。
- `satisfiable_bdd_with_order` / `premise_counterexample_bdd_with_order`，策略验证 `--bdd-order`。
- 网表 JSON 互转：`trit_circuit::netlist::{to_json, from_json}` 与 CLI `trit netlist`。
- `analyze_faults`：ATPG 不再做 `3^n` 输出域枚举；全向量搜索收紧到 ≤8 个主输入。
- 索引化仿真器（`Simulator`）：故障分析/覆盖矩阵不再逐向量建 `BTreeMap`。
- 结构化 Verilog 子集：`parse_verilog` / `to_verilog`，CLI `netlist --from verilog --to verilog`。
- 新增 `GateKind::XorClassic`（真实布尔异或，X 传播）：网表 `XOR2`，Verilog `xor` 原语，NAND 四门映射。
- `trit map`：降到 NAND + 常量；三值强加 `Xor` 因缺 X 检测单元明确拒绝。
- Verilog `assign` 里的 `^` 仍拒绝（请用 `xor` 原语）。
- `trit check`：结构 + 时序 + X 传播 + ATPG 覆盖率，`--clock`/`--min-coverage` 失败即退出 1。
- ATPG X don't-care：每个向量标出可任意填 0/1 的输入位（`dont_care` / `dont_care_bits`）。
- STA-lite：`analyze_timing` / `critical_paths` 与 `trit timing`（required/slack、违例、top-N 路径）；
  `TimingModel` 支持按门类型延迟与 JSON 库。
- `circuit` / `atpg` 支持 `--from text|json|verilog`。
- `BddRequest`（顺序 + 预算）与 `*_with` 系列 API；CLI `equiv/quantify/verify --bdd-max-nodes`。
- `verify --bdd-order best`：四种排序取断言 BDD 节点总数最少者。
- ATPG 随机电路属性测试：每个向量必须真的检出它声称覆盖的故障。

## [0.7.0] - 2026-10-22

ATPG 与 BDD 的一批新能力；`Fault` 结构体有破坏性变更。

### Added

- `trit atpg <netlist>` 与 `trit_circuit::generate_tests`：门引脚 stuck-at 的测试向量生成
  （贪心集合覆盖）+ 覆盖率报告（table / JSON）。
- `AtpgReport::by_gate_kind`：覆盖率按门类型拆分；`exhaustive` 标记是否做了全 `2^n` 搜索；
  `optimal` 标记是否为精确最小覆盖（故障 ≤ 64、向量 ≤ 256 时用 DFS + 下界剪枝）。
- `Fault::witness`（首个二进制区分向量）、`Fault::reason`（`redundant` / `unobserved` / `masked`）
  与 `Circuit::decode_binary`；`trit circuit` 同步输出原因。
- `VarOrder`（appearance / reverse / frequency / frequency-asc）、`order_variables`、
  `equivalent_bdd_with_order`、`equivalent_bdd_best`、`quantify_bdd_with_order`、`quantify_bdd_best`；
  CLI `equiv --bdd-order` 与 `quantify --bdd-order`。
- `BddLimits`（默认 100 万节点/单个 BDD）与 `equivalent_bdd_with_limits`；超限返回
  `BddError::BudgetExceeded`，所有 BDD 构造点都做 O(1) 检查。

### Changed（破坏性）

- `Fault` 新增 `witness` 与 `reason` 字段：用结构体字面量构造 `Fault` 的代码需要补字段。
- stuck-at 可测性判定改用 `2^n` 二进制测试向量；`X` 输入不是物理测试向量，`analyze` 的
  `observable` 与 ATPG 口径一致。
- 主输入超过 16 时故障分析返回 `CircuitError::TooManyInputs`。

## [0.6.0] - 2026-10-22

### Added

- `trit-core::Limits` + `check_expr` / `check_vars`：库级统一预算（深度 / 节点 / 枚举变量数）。
- `trit-eval`：`eval_within` / `truth_table_within` / `equivalent_within`。
- SAT、BDD、策略入口先做深度/节点检查，超限返回 `Limit` 错误；CLI 与 WASM 共用同一套预算。
- `scripts/mutation_check.py`：12 条关键规则的变异门（12/12 被抓）；新增每周 CI。

### Fixed

- 变异门发现 `strong_and` / `strong_add` 的 K3 断言过弱（只测了 `X⊗X` 与 `1⊗1`），
  补上 `1⊗0 = 0`、`0⊗1 = 0` 的固定用例。

## [0.5.0] - 2026-10-22

### Changed（破坏性）

- `CircuitReport::untestable_faults` 替换为 `faults: Vec<Fault>`：逐**门引脚**、逐极性（SA-0/SA-1）。
  `trit circuit` 的 JSON 字段同步为 `faults`（含 `input_index` / `stuck_at` / `observable`）。

### Added

- `trit-core`：解析嵌套上限 256（`ParseError::TooDeep`）。
- CLI 入口规模护栏：`table` / REPL 最多 10 个变量，网表分析最多 10 个主输入。

### Fixed

- 故障注入从整条 net 改为 gate-pin：多扇出不再串扰，SA-0 / SA-1 分开报告。
- `TriVec::from_ternary` / `and_assign` / `or_assign` 的纯二值路径不再分配 Unknown 临时平面。
- GitHub Action 全部固定到 commit SHA（checkout / rust-toolchain / rust-cache / artifact / gh-release），
  MSRV 1.88 同样固定。
- 文档：DESIGN 路线补 v0.5；CONTRIBUTING 的 `tla-core` 路径改为 `trit-core` / `trit-eval`；
  `bulk_bench` 把缓冲区分配移出 move-only 计时；README 明确双导轨只是进程内表示，不是字节协议。

## [0.4.2] - 2026-10-22

### Fixed

- `trit-circuit`：`analyze` / `propagate` / 故障模拟改用真正的拓扑序（此前把全部门输出预置为已解析，
  等于按声明顺序求值）；主输出必须有驱动。
- `trit-eval`：修正三条不保真化简（`a | (a | b)`、K3 `a ⊕ a`、K3/B3 `a ≡ a`）；
  `semantically_equal` 改用真实 `3^n` 等价，不再比较变量顺序敏感的 64 位指纹。
- `trit-bdd`：`is_valid` 相对合法自由变量域判定；`satisfiable_bdd` 与合法域相交，不再把非法导轨 (1,1)
  当成见证；新增 `premise_counterexample_bdd`。
- `trit-policy`：`assumptions` 按“前提确定为 True”过滤（SAT/BDD 新增 `premise_counterexample*`），
  不再用三值 `premise ⇒ assertion` 恒真替代；schema 拒绝未知字段；空断言不再静默通过。
- `trit-core`：`TriVec::from_binary` 规范化多余字与尾部 padding；`TriBit` 的 B3 或不再产生禁止的 `(1,1)`。
- `trit-cli`：CSV/Markdown 每个变量独立成列（此前整行拼成一格、表头拆两行）；REPL `:algebra` 真正切换语义；
  `simplify` 若结果不等价则非零退出，不再 fail-open。
- `action`：`policy` / `engine` 经 env 传入并校验 engine 白名单，不再把输入拼进 shell 脚本。

## [0.4.1] - 2026-10-22

### Fixed

- `trit-sat` / `trit-bdd` 的二值快速路径把 `⊕` 编成经典异或，与解释器不一致：K3/B3 的 `⊕` 在 `0/1` 上是 `∧`，
  Ł3 上是 `∨`。常量（`1 ^ 1`）与 `--binary` 变量都会触发，例如 `trit equiv --symbolic "1 ^ 1" "1"` 误报不等价。
  SAT 新增 `binary_strong_add`，BDD 的二值分支按代数选 `∧`/`∨`，并加回归测试。
- 新增 `trit-sat` 属性测试：随机公式上 SAT 与 `3^n` 穷举逐例一致，且 SAT 给出的反例必须在解释器下区分两边。

## [0.4.0] - 2026-10-22

### Added

- `trit-wasm` + `playground/`：浏览器里跑求值、三值真值表、等价与 Belnap 四值（wasm-bindgen，`--target web`）。
- `trit-belnap`：Belnap 四值 `T/F/B/N` 的 opt-in 语义（双轨 `(t,f)`、`∧/∨/¬/⇒/⇔`、
  4^n 真值表与逐点等价）；`trit belnap eval|table|equiv`；`sansheng` 再导出。
- 网表指令 `const <0|1|X> <out>` 与 `buf <in> <out>`；`examples/x_mask.net` 演示 `X & 0` / `X | 1` 遮蔽。
- 网表校验：同一信号多驱动（含“既是主输入又被门驱动”）与重复主输入分别报
  `MultipleDrivers` / `DuplicateInput`。
- `trit_circuit::propagate` 与 `trit circuit --assign`：给定一条含 `X` 的主输入向量，单遍拓扑传播，
  报告每根信号取值、主输出上的 `X` 来源，以及被遮蔽（输入含 `X`、输出确定）的门；不枚举 `3^n`。
- `trit_bdd::quantify_bdd` 与 `trit quantify`：三值存在/全称量化（`exists` / `forall`），
  量化域是 trit（`{0,1,X}`），可用 `--binary` 收窄到 `{0,1}`；提供自由变量上的见证/反例与 `--json`。
- `sansheng` 再导出 `Quantifier` / `Quantified` / `quantify_bdd`。

### Fixed

- `Gate::constant` 不再把自己的输出当输入，避免 Const 门被判成组合环。

## [0.3.2] - 2026-10-22

### Added

- `trit verify` 策略支持 `assumptions`：先检查前提可满足，再检查 `assumptions ⇒ assertion` 恒真；
  前提不一致返回 `InconsistentAssumptions`。
- `trit_sat::satisfiable` / `trit_bdd::satisfiable_bdd`：返回可满足见证。

## [0.3.1] - 2026-10-22

### Changed

- Release 产物扩到三平台：`trit-linux-x86_64.tar.gz`、`trit-windows-x86_64.zip`、
  `trit-macos-aarch64.tar.gz`；发布改为 build 矩阵 + 单次 publish，避免并发建 release。

## [0.3.0] - 2026-10-22

### Added

- `trit-core`：`TriVec` 位切片三值向量与 `TriBit::from_binary` / `as_binary`；二进制是
  `unknowns` 缺失的零拷贝视图（1M 元素打包 0.65 ms、K3 AND 0.033 ms）。
- `trit-sat` 二值快速路径：`--binary` / `--binary-all`，纯二值子图走经典 CNF
  （100 变量链子句 3866→598，0.028→0.0147 s）；`--stats` 打印编码规模。
- 充分性反例最小化：`minimal_counterexample` / `--minimize`，对偶 `a==b` 检查保证未列出的变量任取仍区分。
- `trit-bdd`：canonical BDD 等价引擎与 CLI `--bdd`；`--stats` 打印差集节点数。
- `trit-policy` + `trit verify`：JSON 决策表、定义展开、全合法赋值断言检查；失败退出码 1 并给最小反例。
- `action/action.yml`：可复用的 `trit verify` GitHub Action。

### Changed

- 仓库由 `ternary-logic-analyzer` 更名 `trit`，拆成 6 个 crate 的 workspace。
- `EquivalenceResult` 新增 `counterexample_assignment`；编号超出 `u64` 时 `counterexample=None`
  （100 变量不再溢出）。

### Fixed

- Release 工作流打包的二进制名由 `tla` 修正为 `trit`。
