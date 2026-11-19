# 错误台账（ERROR LEDGER）

本文件记录本项目从 P0 到 P6、再到 bash 迁移全过程中**真实发生过的错误与陷阱**。

写它的理由：这些错误里绝大多数属于同一族的——**不报错、只给错结果**。它们不会让测试变红，
只会让判定、边界、缓存、覆盖率悄悄失效，然后在下游以"莫名其妙"的形式出现。散在对话历史里的
经验等于没有经验，所以统一落成台账，每条都标"固化为哪条断言"。

约定：
- **现象** 写可观察到的样子（尽量是原话/原输出），不写"好像有问题"。
- **根因** 写机制，不写"手滑"。
- **固化为** 写 selftest 分组编号，或写 `policy/constraints` 规则 id；没有固化的是**欠账**，明确标出。
- 每条都能被一个新来的人独立复现，不需要读过之前的对话。

---

## A. 平台层：Windows / PowerShell / Git Bash

| # | 现象 | 根因 | 修法 | 固化 |
|---|---|---|---|---|
| L01 | 回执 JSON 在 `ConvertFrom-Json` 报"传入的数组无效" | 会话里的 PowerShell 工具本身是 **5.1**（`5.1.28000.3365`），脚本内部调用的 `pwsh` 是 7。写回执的是 7（UTF-8），读回执的是 5.1（按 ANSI 解码），连 JSON 结尾引号都读坏 | 止血：回执改纯 ASCII（`EscapeHandling EscapeNonAscii`）。根治：默认执行面改 bash，`.ps1` 全部加 `#requires -Version 6.0` 版本闸 | I12、M10、M11 |
| L02 | 读出来的配置文件内容坏、字符串对不上 | `Get-Content -Raw` 在 5.1 **读也**按 ANSI 解码，不只是写坏 | 一律 `[IO.File]::ReadAllText(..., UTF8Encoding($false))`；迁移后统一走 python | 迁移后由 I 组覆盖 |
| L03 | `journal status` 打印的时间没有时区，换台机器文字就变 | `ConvertFrom-Json` 把 ISO 串**自动转成 `[DateTime]`**，打印时套本地区域格式，偏移丢失 | 迁移后原样打印 ISO 带偏移；差异在对比脚本里显式登记为"接受的设计差异" | Q6 |
| L04 | 迁移到 bash 后 `-Now` 不带偏移，naive 与 aware 时间比较直接抛 `TypeError` | PowerShell 版从未暴露，因为 `[DateTime]::Parse` **自动补本地时区**，把问题藏起来了 | python 端统一 `datetime` 带 `astimezone()`，解析失败即报错 | M/N 组（timeline 迁移时新增） |
| L05 | `git diff > x.patch` 出来的补丁无法应用 | PowerShell `>` 重定向写 UTF-16，git 读不了 | 一律 `git diff --output=<变量>`；先赋变量，因为 `--output=(Join-Path ...)` **不展开子表达式**，报 `could not open ''` | 生成补丁脚本内固定 |
| L06 | 迁移后每个构建阶段都退 101，看着像构建失败 | **MSYS 只转换 argv，不转换环境变量**。`-WorkRoot /d/KF/x` 写进 `CARGO_TARGET_DIR` 后，cargo 收到 POSIX 路径 | bash 入口加 `native()` 归一化，环境变量显式转 Windows 形态 | N 组 |
| L07 | `jq` 不存在；`python3` 能 `command -v` 到却不执行任何东西 | Git Bash 无 jq；PATH 上的 `python3` 是 WindowsApps 桩 | `tools/_python.sh` 里 `pick_python` 显式探测真实解释器，`PYTHON=` 可覆盖 | M 组 |
| L08 | 还原后树对了，构建结果还是旧的 | `Copy-Item` / `shutil.copy2` **保留旧 mtime**，cargo 认为文件没改动过 | 还原后 `utime(now)` 刷新 mtime | Q5 |
| L09 | 参数传进去没生效，或报"找不到参数" | PowerShell 变量名**大小写不敏感**，`$kind` 撞上 `-Kind` | python 端键名统一小写映射；PS 只做转发，不持状态 | 迁移后由转发器结构消除 |
| L10 | 命令退出码是个奇怪的数字/字符串 | `exit (函数)` 把函数返回的**提示字符串**当退出码 | python 端 `sys.exit(int)`，入口脚本不拼字符串 | 各组退出码断言 |
| L11 | 断言"无命中"时反而通过 | `@($x) -match '...'` 无命中时 `Count` 仍是 1（空数组包装） | 一律 `Where-Object` 过滤后再数 `Count` | 各组断言写法 |
| L12 | 编译报 `include_str!` 路径解析失败 | PowerShell 双引号内嵌 `include_str!("../../x")` 被当转义 | 用单引号字符串 | — |
| L13 | 生成器脚本读了不该读的目录 | Python 生成器里 `if` 不阻止 `os.listdir()` 求值（表达式先求值再判断） | 拆出 `has_py_tests()`，先判断再调用 | N 组 |
| L14 | 13 步对照"全部一致"，但两边输出都是空的 | `git show HEAD:tools/memory.ps1` 返回 **0 字节**——该文件从未提交，旧版输出全空、退出码 0 | 改用 `git show :tools/memory.ps1`（读索引）。**对照实验第一步是先验证对照物非空且在版本库里** | 迁移流程约定 + 记忆条目 |
| L15 | 只暂存的新增文件在 reset 后消失 | `git reset --hard` 会删掉仅存在于索引的新文件 | 提交链分段做，或先 `git stash` | — |
| L16 | `Read` 直接读 20MB 导出文档失败 | 工具对超大文件不可用 | 先用结构/目录定位，再按段落取 | — |

| L44 | 迁移到 bash 后 `agent-verify` 报 `command failed`，可同一命令在 PowerShell 里跑得好好的 | `shutil.which("bash")` 在 Windows 上解析到 **System32\bash.exe = WSL 的 bash**。WSL 看到的是另一套文件系统（Windows 盘符挂在 `/mnt/<盘符>` 下），PATH 里是 `/mnt/c/...`，Windows 的 `pytest.exe` 在那边根本不可执行 | `pick_shell()` 按**路径**优先 Git Bash（`BASH` 环境变量 → 两个 Program Files → 由 `git` 反推 `../bin/bash.exe`），只剩别的才用并打印警告 | T10 |
| L45 | dry-run 的输出里凭空多一行"警告: 生成的 JSON 已截断，因为序列化已超过设置的深度 8"，回执字段跟着缺 | 旧实现用 `ConvertTo-Json -Depth 8`，plan 的嵌套一旦变深就被**静默截断** | 回执改用 python `json.dump`；对比脚本把这条警告当噪声剔除，并断言新实现**不再出现** | T2、cmp_agentverify |
| L46 | `ledger report` 判红：期望 `wall: 5s`，实得 `wall: 5.0s` | `[math]::Round(5.0,2)` 打印 `5`，python `round(5.0,2)` 打印 `5.0`。**同一个数，两种文字** | 墙钟总时长也走 `fmt_num()`（整数值输出整数形态），与 token 侧一致 | H3 |
| L47 | 故意写进陷阱行，`constraints` 仍报 `clean` | 它只看 **git diff**；fixture 不是真仓库 → 永远没有 diff → 永远 clean。这是"规则在、日志在、返回 0，但判断对象不是它以为的那个"的典型样本 | 拒绝路径的 fixture 先 `git init` + 提交，再改文件 | T6、I 组 |
| L48 | `bash -c` 里 `command -v pytest` 与登录 shell 结果不一致，同一台机器两种答案 | 非登录 shell 不读 profile，PATH 完全取决于父进程给什么；父进程是工具层拉起的 python，不是一个用户的终端 | 执行面固定为 Git Bash 并按"能否看见目标文件"验证（T10），不去猜 PATH | T10 |
---

## B. 语义层：判定、边界、缓存——"不报错，只给错结果"

这一族是台账的核心。它们共同的特征：**失效是静默的**。规则在、日志在、命令返回 0，
只是它判断的对象不是它以为的那个。

| # | 现象 | 根因 | 修法 | 固化 |
|---|---|---|---|---|
| L17 | `constraints check` 对任何改动都打印 `pass (0 warning, 0 block)` | glob 转正则时**先 `escape` 再替换 `**`**，`**` 变成 `\*\*` 字面量，所有按路径作用域的规则静默不匹配 | 先占位符、再 escape、最后换回 `.*` / `[^/]*` | P6 |
| L18 | 同上，但规则表、`-Allow` 都正常，就是拦不住 | `in_scope(path, globs)` 收到**字符串**时 `for g in globs` 逐字符迭代，`crates/**/src/**` 拆成 `c`、`r`、`a`… 永远 False | 函数同时接受单条 glob 与列表；并**直接断言 `in_scope` 本身**，不靠"有违规时被拦"间接体现 | P6 |
| L19 | workspace-write 下能写到根目录的**兄弟目录** | 根边界用 `startswith(root)`，`pol-outside` 以 `pol` 开头就被判成根内 | 按路径组件比较：相等，或 `root + 分隔符` 前缀 | R5 |
| L20 | bash 下的 `cp a.txt /e/tmp/x` 在 workspace-write 里直接放行 | 分类器正则只认 `[A-Za-z]:\…` 与 UNC，**POSIX 绝对路径一个都不匹配**，等于不做判定 | 加 POSIX 路径识别 + 盘符映射（`/e/tmp/x` 映射成 Windows 的 `E` 盘 `tmp/x`）；映射不出来的绝对路径**算根外**（无法判定视为越界） | R5 |
| L21 | 还原后源码对了，构建还是旧的 | 快照/还原保留 mtime（见 L08） | 还原后刷新 mtime | Q5 |
| L22 | 合并测试二进制的方案，反而更慢 | 4 个二进制 4.45s **慢于** 1 个二进制 2.60s；只有**严格交替测量**才看得出（先测的吃冷缓存） | 单二进制 + 按职责分目录（`tests/kernel/main.rs` + `suite_*/mod.rs`） | 性能基线记录 |
| L23 | 关掉 debug 信息以为能大幅降链接成本 | 实测 10.9s vs 10.7s，只省磁盘 | 证伪并回滚配置，不要留"看起来合理"的假优化 | 已证伪，写入 BUILD-PERF |
| L24 | `cargo metadata` 在非 workspace 目录直接失败 | 该目录不是 workspace 根 | 在 workspace 根执行，或用 `--manifest-path` | N 组 |
| L25 | `node --test` 在 `dsh-orch` 退出 1；`node --test test/` 什么也不跑 | 依赖缺失的 Rust CLI 产物；目录语义要求显式 glob | 用 `"test/*.test.js"`；缺依赖时记"不可测"而不是记失败 | N 组 |
| L26 | pytest 收集到用例但导入失败 | 根目录不在 `sys.path` | 加根 `conftest.py` | 减集脚本内 |
| L27 | 复制 Rust 工作区来测编译成本，22 个 target 解析错误 | 用复制代替隔离，路径与 target 目录互相污染 | 用独立 `CARGO_TARGET_DIR` / `git worktree` | BUILD-PERF |
| L28 | 成本数字比实际偏高约 0.3s/段 | 旧 `costprobe` 每段经 `pwsh -NoProfile -Command`，**把测量工具的启动成本算进被测项目** | 直接起子进程；同时把结论改成"旧数字偏高"而不是"新实现更快" | N5/N6 |
| L29 | 减集在热点文件上几乎减不动 | 改 `registry.py` 时 affected 195、选入 130 ≈ 全量 | 减集只对长尾有效；热点文件老实跑全量 | 记忆条目 |
| L30 | 同一改动两次测量差很多 | 冷/热缓存交替污染 | 严格交替、同 target、同参数，并且记录"第几轮" | BUILD-PERF |

---

## C. 流程层：对照实验、编排、记忆

| # | 现象 | 根因 | 修法 | 固化 |
|---|---|---|---|---|
| L31 | 迁移"零差异"，但其实是两边都没输出 | 空对照物（L14）。**空对照物比没有对照物更危险，它会安静地一致** | 先断言对照物字节数 > 0 且来自版本库（`git show :path`） | 记忆条目 + 迁移流程 |
| L32 | 迁移后输出不一致，第一反应是改新实现去"看起来像" | 差异背后往往是旧实现的**语义分叉**（rust 分支把被改包自身算进 affected，python/node 只算导入者） | 把差异提成**显式签名**（`reverse_closure(..., include_seed=True/False)`），而不是模糊对齐 | N3 |
| L33 | `agent-verify` 回执里 steps 是空的 | `$receiptDoc.steps = $steps` 赋值位置在序列化之后 | steps 必须**最后赋值**；回执用 `Write-Receipt` 统一出口 | I 组 |
| L34 | 记忆写入被拒：三条不同的拒绝文案 | 证据里出现工作区外路径；`-Kind` 不在允许集合（`constraint` 不是合法值）；`-Detail` 取值以 `-` 开头被当参数 | 这是护栏**按设计**拒收，不是故障；改写措辞/换 kind 后重录 | J 组（4 条拒绝文案） |
| L35 | `docs/BUILD-PERF.md` 被 `Cargo.toml` 引用却不存在 | 文档路径失效，引用还在 | 要么补文件，要么删引用；失效引用比缺文档更误导 | BUILD-PERF 检查 |
| L36 | 记忆里写"79 条全绿"，实际早已 126 条 | 条目过期没有机制感知 | 用 `-Mode revise` 就地更正，并让自测打印真实条数 | 记忆 audit |

| L49 | 拿整套旧脚本当对照跑，dry-run 文本对不上，看不出是`编排层`还是`affected`的问题 | 两边 `affected` 的 python 分支强弱不同（旧实现给全量、新实现给持证子集），直接比会把**上一轮的改进**混进这一轮差异里 | 给新实现加 `-Plan` 注入接口，两边喂**同一份 plan**，只比编排层自己的输出；差异项归零 | cmp_agentverify 三条路径 |

| L50 | `crates/a/` 认领锁不住 `crates/a/src/lib.rs`：同一目录能开出两张互不冲突的锁 | `claim::covers` 用 `path.as_bytes().get(scope.len()) == Some(&b'/')` 判子路径，scope 自带尾斜杠时下标指向内层首字符而不是分隔符，判 false | `covers` 对 scope 与 path 都先 `trim_end_matches('/')` 再比较；尾斜杠是宿主写法，不是语义。断言 `ORCH_CLAIM_TRAILING_SLASH` | 扩测 `claim_edges` |
| L51 | 写求值测试时三处报 `no method named single`/`value_at`：`TriBit` 没有单点取值方法 | 单点读是 `checked_get(bit)`（`bit >= 64` 给 `None`）；`value_at` 属于 `TruthTable`，不是 `TriBit`；`TriBit` 的公开面只有 `get`/`checked_get`/`constant`/`from_binary` | 断言统一用 `checked_get(0).expect(..)`；不要把"表级方法"记为"位级方法" | Rust 断言 `ORCH_EVAL_BITWISE`（`trit-eval/tests/eval_semantics.rs`） |
| L52 | 化简前后两张真值表 `values` 不等，看着像"化简不保语义" | 表的**行号由变量出现顺序决定**，而重写会改变顺序（De Morgan 展开、操作数交换）。直接比 `values` 等于拿两份不同编号的表对照 | 判等用 `equivalent`（它取变量并集后对齐）或逐点 `eval`；只有同一表达式才比 `values` | Rust 断言 `ORCH_EVAL_SIMPLIFY_SEMANTIC`、`ORCH_EVAL_EXPAND_IMPLIES` |
| L53 | 断言"全 0 数据 CRC 为 0"，实测返回 `[-1,0,0]` | GF(3) 的"零余数"是 digit 0，而 digit↔trit 的映射是 `digit = value + 1`，所以零余数写成 trit 是 **-1**。默认"0 就是 0"的实现会与写入端错开一半 | 把映射钉进断言：`crc3(&[0,0,0]) == [-1,0,0]`；两份独立实现必须同口径 | Rust 断言 `ORCH_REDTEAM_CRC_ZERO_DIGIT`（`trit-redteam/src/blob.rs`） |
| L54 | 网表缺 `y_p/y_n` 时 `analyze` 直接 panic，而不是返回 `Err` | `env["y_p"]` 是 `BTreeMap` 的索引，缺键即 panic。对"选错文件"这种常见手滑，panic 会把审计进程带走 | **已修**（`cargo test -p trit-redteam`）：`env.get(...)` + `bail!("网表缺少 y_p/y_n 输出")`；原 `#[should_panic]` 测试改为 `is_err()`（panic 不再是契约） | C001、C002（复算：`python tools/checks.py run -Only C001`） |
| L55 | 已知条件（`True`）走 `Jz` 后 `Halt` 输出 `Unknown`，看着像解释器错了 | `Jz` **会弹出条件**（条件是被消费掉的），之后栈顶是原来的下一层；空栈 `Halt` 只能报 `X` | 记进语义：`X` 是"确定地不知道"，不是失败；写断言 `Jz` 弹栈后空栈停机给 `X` | Rust 断言 `ORCH_VM_JZ_POPS`（`trit-machines/src/vm.rs`） |
| L56 | 一份畸形网表就能让 `trit-redteam` 的审计进程 panic；更糟的是该样本被跳过，审计等于被绕过 | 解析器把**外部输入**当内部不变量：`rail.rs` 74 行 `assert_eq!`、101 行 `assert!`、106 行 `panic!`、134 行 `expect`；`sidechannel.rs` 用 `env["y_p"]` 索引 | 解析器全链改 `Result`（错误带位置）；`eval_expr` 拒绝尾部残留（`a b` 不再静默只算 `a`）；缺 `y_p/y_n` 改 `bail!`；5 类畸形输入各一条断言；原 `#[should_panic]` 测试改 `is_err()` | C001、C002 |
| L57 | 反向探针把违规代码追加到文件末尾，检查仍判绿，误以为"这条检查是假的" | `cut` 在 `#[cfg(test)]` 处 `break`，之后的内容不看。**过滤是对的**（forbid 规则不该被测试里的正常断言打红）；错的是验证方法——注入点必须落在被检查的区段内，且要正反两次都验 | 注入到 `cut` 之前再验：判红 exit 1；还原后判绿；`diff` 为空才算验证成立 | C001（反向自证） |
| L58 | 版权门禁把**远端早已存在**的历史提交当成"待推送"，于是任何从 main 分出的新分支都被旧 trailer 误伤；修的时候又写成 `rev-list --not --remotes <sha>`，`--not` 把 `<sha>` 一起否定，结果恒为空集，门禁变成"永不拦截" | ①新分支用 `rev-list -n 50` 而非"远端还没有的 commit"；②git 的 `--not` 翻转其**之后所有** revision 的极性，包含项必须写在 `--not` 之前 | 两次都改对：`rev-list "$local_sha" --not --remotes`；四场景自证（干净新分支放行 / 违规新分支拦住 / 违规历史分出的干净分支放行 / 已有分支增量拦住） | C003、C004 |
| L59 | 检查器把字面串当正则，规则**永不命中**；又把"解释为什么禁止"的注释当违规，规则**永远命中**。两个相反的错误产生同一个后果：这条检查等于没有 | `grep_files` 一律 `re.compile(pattern)`：`"$local_sha"` 里的 `$` 是行尾锚点，`rev-list "$local_sha" --not --remotes` 因此匹配不到任何行；而注释行里写了被禁字样，照样计入命中 | 加 `literal`（`re.escape`）与 `exclude_line`；`cut` 在 `#[cfg(test)]` 截断；三种情形各有自测（`test_regex_dollar_is_an_anchor`、`test_literal_matches_the_same_text`、`test_exclude_line_skips_comments`） | C005 |
| L60 | `trit-lsp` 的 `next_char(text, i)` 用 `text[i..]`，隐含"`i` 必在 char 边界"；这是一个**没有任何断言支撑的假设** | 当前恰好成立：所有推进都用 `len_utf8()`（`spelling.len()` 也是字节长度），`i += 1` 只出现在 `'0'/'1'/'?'` 与 `(`/`)` 这些 ASCII 分支。但任一未来 `i += 1` 落到非 ASCII 分支，就变成用户编辑器文本上的 panic（LSP 进程直接崩） | 把假设变成断言：`every_token_lands_on_char_boundaries` 遍历 33 组刁钻样本（CJK/emoji/组合字符/多字节操作符/BOM/CRLF/零宽空格），要求每个 token 的 `start`/`end` 都在 char 边界、字节切片等于 token 文本、token 不重叠；违约时切片本身就会 panic | C006、C007 |
| L61 | 同一个库函数有三个调用点，**校验强度却不一致**：`light-fit` 校验了 `nshape[0]==h && nshape[1]==w && sshape[0]==h && sshape[1]==w`，而 `light-relight-normals` 只校验 `nshape.len() == 3`（只看秩），`meshtex --light-out` 连 seg 的 shape 都直接丢弃（`let (seg, _)`） | 库函数 `fit_from_normals` / `relight_normals` 把长度契约完全交给调用者，体内裸索引 `mask[i]` / `n[i*3+2]` / `rgb[i*3+c]`；任一处漏校验，用户给一张分辨率不同的 normal/seg 图就越界 panic。`w * h` 也没有 checked，32 位目标上溢出会先算出一个偏小的上界 | ①库层自己守契约：`fit_from_normals` 用 `checked_mul` + 长度检查（不足返回 `None`，签名不变）；`relight_normals` 改返回 `Option<Vec<f32>>`（唯一调用点已更新为 `let Some(...) else fail`）。②CLI 层补齐：`light_relight_normals_cmd` 与 `meshtex` 都改成与 `light-fit` 同口径（含 seg 的 2 维 shape）。③回归测试 `short_buffers_are_rejected_instead_of_panicking`（短缓冲、`usize::MAX` 溢出、正常长度走通三组） | C008、C009 |
| L62 | `parse_obj` 解析外部 OBJ 时对索引不做任何上界检查：`f 0 1 2` 让 `0 - 1` 在 u32 上溢出（debug panic，release 绕成 4_294_967_295），`f 1/9 ...` 直接 `uvs[98]` 越界，`f 1 2 9` 的越界号要到下游 `m.verts[8]` 才炸 | OBJ 顶点号是 1-based，代码在解析阶段就 `- 1` 并立刻拿结果索引 `uvs`；同时 `f` 允许引用在文本更靠后定义的 `v` / `vt`，边读边查上界会误拒合法文件 | ①解析阶段只拒绝 `0`，UV 缺省用 `raw_v` 而不是 `vi + 1`（后者会溢出）；②上界统一放到整份文本读完再校验（顶点号对 `verts.len()`、UV 号对 `uvs.len()`）；③`tri_uv` 延后到校验之后构造，`uvs[...]` 不再是裸索引；④回归测试 `obj_indices_out_of_range_are_errors_not_panics` 覆盖 0 号、UV 越界、顶点越界、缺 vt 四组 | C010、C011 |
| L63 | CI 的 `agent checks (registry)` 里凡 `shell: bash` 的检查全红，报 `exit 126 / Permission denied` | Windows 上的 git 默认不记 exec bit，`.sh` 一律以 100644 提交；clone 到 linux 后文件不可执行，`tools/checks.sh selftest` 这种直接执行的写法当场被拒。报错点离真因很远（看着像检查器坏了，实际是文件权限） | `git update-index --chmod=+x` 把 21 个脚本补回 100755 —— 权限位是脚本的一部分，不该依赖运行它的那台机器 | C005 自测新增 `test_tool_scripts_are_executable_in_the_index`（读 git index，跨平台一致） |
| L64 | 清单里的 `obj` / `png` / `path` 直接 join 到包目录，`../` 能让打包器去读包外的任意文件，再把字节原样塞进输出的 `.glb` | 清单是外部文本（第三方资源包场景），路径没做任何约束；读进来的 PNG 直接进 BIN chunk，等于一条信息外带通道 | `resolve` 改成词法折叠 + 边界判定：相对路径折叠后必须仍在包目录的上一级之内（真实产物的清单就写在 `../objects.manifest.json`，所以不能一律禁 `..`），绝对路径显式放行；`e.obj` / `e.png` 也从裸 join 改走 `resolve` | C012 |
| L65 | 读外部 `.npy` 的头解析在 `data::read_npy_i32` 与 `geo::read_npy_f32` 各写一份，四处边界同时缺：v2 头只按 `len >= 10` 守卫却去读 `bytes[10..12]`；`header_len` 不校验就切 `bytes[start..start+hlen]`；shape 连乘与 `n * 4` 都是裸 usize 乘法 | 头字段是外部二进制：10 字节的 v2 头、声称 65535 字节的头、巨大 shape，都能让解析刷成越界 panic（debug）/ 绕成小值（release）；两份实现意味着只修一处等于没修 | 统一到 `crate::npy`：一份 `header` + `body`，i32 / f32 只差描述符与解码；v2 要求 ≥12 字节、`checked_add` 后验 `end <= len`、shape 用 `try_fold`、`n * 4` 走 `checked_mul`；`data` / `geo` 各留一层 `pub use` 保持入口名 | C013、C014 |
| L66 | 断点文件里的 `n` / `slots` / `n_layers` 是外部二进制声明，`state::load` 却先 `vec![0.0f32; n]` / `Vec::with_capacity(n_layers)` 再逐项读 | `n` 是 u32，`vec![0f32; u32::MAX]` 要 16GB，分配失败直接 abort；`len` 校验发生在分配**之后**，等于让文件自己决定进程的内存预算；`Vec::with_capacity(n_layers)` 同理（`Layer` 结构很大） | 取数原语独立到 `state/tpn.rs`，一律 `need(b, at, count)` 先 `checked_mul(4)` + `checked_sub` 比剩余字节，通过后才分配；`with_capacity(n_layers.min(64))`；v2 的 6 个数组改 `take_f` | C015 |
| L67 | `tex::quilt` 的 `src` 是交错 RGB，契约 `src.len() == sw×sh×3` 只写在文档里；`out` 的 `out_w * out_h * 3` 是裸 usize 乘法，而 `out_w` 来自 CLI `--size` | `src.chunks(3)` 末尾短块让 `p[c]` 越界 panic；`--size 100000` 让 `out_w * out_h * 3` 溢出（release 绕成小值后索引越界）或直接 40GB 分配 abort | 开头 `checked_mul` 验 `sw×sh×3` 与 `src.len()`，输出改分配 `cells`（`checked_mul` 后的值）；`sm64` / `luma_of` / `fit_plane` 服从预算顶格下沉到 `tex/util.rs` | C016 |
| L68 | `fuse::View` 的 `ids` / `depth` 契约（都该等于 `w×h`）没有任何校验；四个构造点里 `depth` 的 `.npy` 形状被 `_` 丢弃，`w/h` 只取自 `seg` 的 shape | `seg.npy` 与 `depth.npy` 尺寸不一致时，`occ::selfsup_solid` / `occ::features` 里的 `v.depth[i]`（`i < w×h`）直接越界 panic；`occ.rs` 538 行顶格，加一行都触发 module_budget | `occ/mod.rs` 加 `check_views`（`ids.len() == w*h && depth.len() == w*h`），`selfsup_solid` / `features` 开头各调一次；`Mlp` 156 行下沉到 `occ/mlp.rs` 腾预算 | C017 |
| L69 | `mesh::Grid::from_points` 直接用点云包围盒 × `cell` 算 `dims`，`vec![0u8; dims[0]*dims[1]*dims[2]]` 是裸乘、没有上限；同为栅格构造的 `occ::grid_for` 有 `saturating_mul` + 60M 门，这里什么都没有 | `cell` 最小 1e-4（函数自己 `max(1e-4)`），1 米的包围盒就是 10005³ ≈ 1e15 体素 → 1PB 分配直接 abort；`margin` 还能再乘进去；9 个调用点里 `cell` 多半来自 CLI `--cell` | 与 `grid_for` 对齐：`margin.min(1<<20)`、`total` 走 `saturating_mul`、超过 60M 返 `None`、`occ` 按 `total` 分配；`origin` 改 `[0,1,2].map(..)` 把增加的行压回预算 | C018 |
---

## D. 还没固化的欠账（明确标出，不假装已解决）

| # | 欠账 | 现状 | 打算 |
|---|---|---|---|
| L37 | Rust 受影响分析只到**包**级 | `trit-orch` 内 21 个模块不区分，改一个文件就当整包受影响 | 下沉到模块级（时序条目 `b27761469`） |
| L38 | Node 适配器只在 fixture 验证过 | 没在真项目跑通（时序条目 `lac1ec29f`） | 找真仓库验证 |
| L39 | 跨语言调用边未建模 | python 调 node、rust 调 python 的边不在图里（`r4c77dcdf`） | 先做 fail-safe 全量，再补边 |
| L40 | `journal` 只覆盖 `-Root` 下 | 树外写入不在范围（`l4839d875`） | 由 `policy` 事前拒 + 明确声明 |
| L41 | `policy` 是分类器不是沙箱 | 越界写入只能事后证明 | 已用 `-Explain` 明说（R8），不假装是沙箱 |
| L42 | `Calibrated` 的 `coverage_bp` 是声明不是证据 | 要等 P6.3 conformal 实测才能判红 | 保留为"声明"，不许当证据用 |
| L43 | 记忆条目 `m3389ae79` 写"79 条全绿" | 严重过期（实际 142） | `-Mode revise` 更正 |

---

## E. 这份台账怎么用

1. **新错误先进台账，再进记忆。** 台账是给人看的完整机制，记忆是给智能体看的最小触发条件。
2. **每条静默失效必须配一条直接断言。** 只靠"有违规时会拦"不够——那只证明了规则能拦，
   没证明规则在看着正确的文件与路径。
3. **"看起来合理"的优化先证伪再采纳。** L23、L29 都是先做了再被测量推翻的例子。
4. **能力边界要出现在输出里，不能只写在注释里。** 注释不会被人读，输出会被。
