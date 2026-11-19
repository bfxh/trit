#requires -Version 6.0
# 这是一个版本闸，不是一个偏好。Windows PowerShell 5.1 会把 UTF-8 当 ANSI 解码，
# 读和写都坏；它的 ConvertTo-Json 也没有转义选项。让它在入口处报错，比让它安静地
# 产出坏数据便宜得多。bash 入口是默认路径：tools/*.sh
# seed-memory-trit: the pitfalls this workspace actually hit, recorded as evidence.
#
#   pwsh tools/memory.ps1 -Mode init -Root D:\KF\trit -Kernel     # once
#   pwsh tools/seed-memory-trit.ps1 -Root D:\KF\trit              # idempotent
#
# Every entry below happened in this repo, with a command behind it. That is the rule:
# memory is not a diary, it is a set of claims each with a way to re-check them. If an
# entry cannot name the command that produced it, it does not belong here.

param([Parameter(Mandatory = $true)][string]$Root)

$ErrorActionPreference = 'Stop'
$mem = Join-Path $PSScriptRoot 'memory.ps1'

$entries = @(
    # --- tooling failures we actually hit, each with its reproduction -----------------
    @{
        kind = 'pitfall'; title = 'Get-Content -Raw 回写脚本会把 UTF-8 读成 ANSI，整份源码变乱码'
        detail = '用 Get-Content -Raw 读 UTF-8 无 BOM 文件再 WriteAllText 回写，中文变成「閫変腑」这类乱码，selftest 直接 ParserError。读文件一律用 [IO.File]::ReadAllText + UTF8Encoding($false)。'
        evidence = 'selftest.ps1 回写后 ParserError at line 56 | exit 1 | 用 git checkout -- tools/selftest.ps1 恢复'
        tags = 'tooling,encoding'
    },
    @{
        kind = 'pitfall'; title = 'exit (函数调用) 会把提示字符串当成退出码，导致守卫假绿'
        detail = 'preflight 里写 exit (Invoke-Preflight)，函数输出的提示行和整数一起进退出码，D2/D4 明明该红却退出 0。改为 Write-Host 输出提示、$code = 函数调用 后再 exit $code。'
        evidence = 'selftest D2 期望 exit=1 实得 exit=0 | exit 0 | refactor-guard.ps1'
        tags = 'tooling,ps'
    },
    @{
        kind = 'pitfall'; title = '按全路径 -notmatch test 过滤源码，目录名含 test 就全丢'
        detail = 'fixture 目录叫 costselftest-xxx，Get-TouchTarget 用 FullName -notmatch ''test'' 过滤，所有源码被当测试文件，touched 为空、rounds 全丢，脚本一声不响输出空模型。按路径分段过滤。'
        evidence = 'selftest E1 rounds=0 | exit 1 | costprobe.ps1 -Path node-proj'
        tags = 'tooling,path'
    },
    @{
        kind = 'pitfall'; title = 'import importlib 是语句不是调用，动态导入检测漏掉整类文件'
        detail = 'import_graph.py 只在 ast.Call 里找 importlib，import importlib 的测试文件没被标成动态导入，未知伪装成已知，减集会静默漏用例。Import 分支也要判 importlib/runpy/pkgutil/imp。'
        evidence = 'selftest B3 期望含 test_dynamic.py 实得只含 test_core.py | exit 1 | import_graph.py'
        tags = 'tooling,python'
    },
    @{
        kind = 'pitfall'; title = 'Copy-Item 还原会保留旧 mtime，cargo 认为没改动'
        detail = 'journal restore 用 Copy-Item 拷回文件，修改时间还是快照时的旧值，cargo 判定未变、跳过重编译，还原成功但构建用旧缓存。还原后必须重置 LastWriteTime。'
        evidence = 'journal restore 后 cargo test 无重编译 | exit 0 | tools/journal.ps1'
        tags = 'tooling,cargo'
    },
    @{
        kind = 'pitfall'; title = 'PowerShell 变量不区分大小写，局部 $kind 撞参数 $Kind'
        detail = 'memory.ps1 里 $kind = ''project'' 实际赋值给了 -Kind 参数，ValidateSet 直接报「project 不是 Kind 的有效值」。局部变量要起不撞参数的名字。'
        evidence = 'selftest J1 报 memory.ps1: project 不是 Kind 的有效值 | exit 1 | memory.ps1'
        tags = 'tooling,ps'
    },
    @{
        kind = 'pitfall'; title = '提前把数组赋给对象，后续 += 重新赋值导致回执漏步'
        detail = 'agent-verify 先 $receiptDoc.steps = $steps，之后 Add-Step 用 $steps += 生成新数组，回执里就少了 memory-note 步。收尾时再赋。'
        evidence = 'selftest J15 期望最后一步 memory-note 实得 journal-end | exit 1 | agent-verify.ps1'
        tags = 'tooling,ps'
    },
    @{
        kind = 'pitfall'; title = '同一份补丁留两份版本会漂移，宁可只留一份累积补丁'
        detail = 'p0-tooling.patch 交付时是 16 条自测，实际已到 36 条；两份内容重叠的补丁会让后来者应用错。删掉旧的，只留 p0-p1 累积版。'
        evidence = 'git diff --stat 显示 selftest 11125→14251 字节 | exit 0 | 删除 p0-tooling.patch'
        tags = 'release,patch'
    },
    @{
        kind = 'pitfall'; title = 'pytest 的 rootdir 不在 sys.path，项目根没有 conftest.py 就 import 失败'
        detail = 'fixture 里 from pkg.core import f 在 pytest 下报 ModuleNotFoundError，用例一条没跑却显示失败。根目录放一个空 conftest.py，pytest 才把根插入 sys.path。'
        evidence = 'selftest I1 期望 exit=0 实得 exit=1 | exit 1 | tests/test_core.py'
        tags = 'python,pytest'
    },

    # --- decisions that later sessions must not silently reverse ----------------------
    @{
        kind = 'decision'; title = '成本记账派生不累加：rounds 按 key 去重，总时长取中位数'
        detail = '每次增量测量存一个 round，key=文件名+内容 SHA256；合并模型时同 key 保留旧值丢弃新值；incr_total_s=median(rounds)。绝不 sum。'
        evidence = 'selftest E1-E3 三次写同一模型 rounds 恒为 2 | exit 0 | tools/costprobe.ps1'
        tags = 'accounting'
    },
    @{
        kind = 'decision'; title = '依赖图不全一律回退全量，并且理由要写进计划里'
        detail = '无 git、cargo metadata 失败、多语言清单未 -AssumeIsolated 三种情况都 proven=false 并输出全量命令。全量永远 sound，猜的子集不是。'
        evidence = 'selftest A1-A3 C1-C2 | exit 0 | tools/affected.ps1'
        tags = 'safety,affected'
    },
    @{
        kind = 'decision'; title = '守卫从事后 check 改成事前 preflight：没编译就算出模块可达性'
        detail = '少写一行 mod 会静默丢整块用例。preflight 从 tests/**/main.rs 解析 mod 递归解析文件，任何未被引用的 .rs 判红并拒绝落盘。'
        evidence = 'selftest D1-D4 | exit 0 | tools/refactor-guard.ps1 -Mode preflight'
        tags = 'safety,guard'
    },
    @{
        kind = 'decision'; title = '权限两轴：配置档 × 网络开关，命令分段求值'
        detail = 'read-only/workspace-write/full 与网络默认关是两个独立轴；命令按 ; | && 切段逐段判定，防止 Get-Content; curl 这种掩护写法。破坏性命令即使 full 也拒。'
        evidence = 'selftest F1-F9 | exit 0 | tools/policy.ps1'
        tags = 'safety,perm'
    },
    @{
        kind = 'decision'; title = '证据来自磁盘不来自退出码：journal 存哈希与副本'
        detail = '权限只是承诺，journal 才是证据。用内容哈希不用 mtime，begin/end/restore 三段；还原后重置 mtime 防 cargo 假阴性。'
        evidence = 'selftest G1-G8 | exit 0 | tools/journal.ps1'
        tags = 'safety,evidence'
    },
    @{
        kind = 'decision'; title = '单一入口 agent-verify：顺序本身就是安全属性'
        detail = 'memory-brief → preflight → affected → policy → journal-begin → run → journal-end → memory-note。任一步拒绝整体拒绝，六步顺序被断言在 selftest 里。'
        evidence = 'selftest I2/I2b 断言步骤序列 | exit 0 | tools/agent-verify.ps1'
        tags = 'entry'
    },
    @{
        kind = 'decision'; title = '内核记忆与项目记忆物理隔离：owner_root + scope 双闸'
        detail = '记忆目录记录 owner_root，搬运到别的项目直接拒收；scope=kernel 只在 -Kernel 初始化的仓里接受。证据里出现工作区外路径也拒收。'
        evidence = 'selftest J11 J12 J14 | exit 0 | tools/memory.ps1'
        tags = 'memory,isolation'
    },

    # --- verified commands, so a later session does not re-measure from scratch --------
    @{
        kind = 'command'; title = 'pwsh tools/selftest.ps1 → 64 条全绿'
        detail = '安全工具自测总账：A 无 git 回退全量、B 动态导入强制纳入、C 零用例判红、D preflight、E 记账去重、F 权限、G 快照还原、H 分账、I 入口、J 记忆。'
        evidence = 'pwsh tools/selftest.ps1 | exit 0 | 通过 64 失败 0'
        tags = 'verify'
    },
    @{
        kind = 'command'; title = 'agent-verify 在 trit-inv 上端到端：proven subset，10.05s，changed=0'
        detail = '改 crates/trit-orch/src/obligation.rs，跑 cargo test -p trit-orch --no-fail-fast，退出 0；journal 证明除缓存目录外零写入。'
        evidence = 'tools/agent-verify.ps1 -Root D:\KF\trit-inv | exit 0 | cargo test -p trit-orch 10.05s'
        tags = 'verify,baseline'
    },
    @{
        kind = 'command'; title = '全工作区 cargo test 约 59s，单包约 10s'
        detail = 'costprobe 实测：根组件冷 61.8s / 增量派生 59.8s；-p trit-orch 10.05s。判断减集是否有收益以这两个数为基线。'
        evidence = 'tools/costprobe.ps1 -Path D:\KF\trit-inv -IncrementalRuns 1 | exit 0 | cold 61.8s incr 59.8s'
        tags = 'perf,baseline'
    }
)

$ok = 0; $fail = 0
foreach ($e in $entries) {
    & pwsh -NoProfile -File $mem -Mode note -Root $Root -Kind $e.kind -Title $e.title -Detail $e.detail -Evidence $e.evidence -Tags $e.tags | Out-Null
    if ($LASTEXITCODE -eq 0) { $ok++ } else { $fail++; "seed: FAILED -> $($e.title)" }
}
"seed: $ok recorded, $fail failed"
if ($fail -gt 0) { exit 1 }
exit 0
