#requires -Version 6.0
# 这是一个版本闸，不是一个偏好。Windows PowerShell 5.1 会把 UTF-8 当 ANSI 解码，
# 读和写都坏；它的 ConvertTo-Json 也没有转义选项。让它在入口处报错，比让它安静地
# 产出坏数据便宜得多。bash 入口是默认路径：tools/*.sh
# selftest: prove the safety tooling actually fails when it should.
#
#   pwsh tools/selftest.ps1
#
# Every case here exists because a tool can be "green" for the wrong reason. A guard that
# never fires is indistinguishable from a guard that is always right -- unless you break
# something on purpose and watch it fire. The fixtures are built from scratch in a temp
# directory so the real repo is never touched.

param([string]$Work = (Join-Path $env:TEMP ('costselftest-' + [guid]::NewGuid().ToString('N').Substring(0, 8))))

$ErrorActionPreference = 'Stop'
$repo = Split-Path -Parent (Split-Path -Parent $PSCommandPath)
$tools = Join-Path $repo 'tools'
New-Item -ItemType Directory -Force -Path $Work | Out-Null

$passed = 0
$failed = 0

function Assert-True([bool]$cond, [string]$name, [string]$detail) {
    if ($cond) { "  PASS  $name"; $script:passed++ }
    else { "  FAIL  $name  ($detail)"; $script:failed++ }
}

function New-File([string]$path, [string]$content) {
    New-Item -ItemType Directory -Force -Path (Split-Path -Parent $path) | Out-Null
    [IO.File]::WriteAllText($path, $content, (New-Object System.Text.UTF8Encoding($false)))
}

# =====================================================================================
"== A. affected: 没有 git 历史时必须回退全量 =="
$A = Join-Path $Work 'no-git'
New-File (Join-Path $A 'Cargo.toml') "`n[package]`nname = 'x'`n"
New-File (Join-Path $A 'src\lib.rs') "pub fn a() {}`n"
$outA = Join-Path $Work 'plan-a.json'
& pwsh -NoProfile -File (Join-Path $tools 'affected.ps1') -Path $A -Changed (Join-Path $A 'src\lib.rs') -Lang rust -Out $outA | Out-Null
$planA = Get-Content $outA -Raw | ConvertFrom-Json
Assert-True ($planA.proven -eq $false) 'A1 无 git 时 proven=false' "proven=$($planA.proven)"
Assert-True (@($planA.unproven) -match 'no git history').Count -gt 0 'A2 理由点名 git' ($planA.unproven -join ';')
Assert-True ($planA.commands[0] -match '--workspace') 'A3 回退到全量命令' $planA.commands[0]

# =====================================================================================
"== B. affected(python): 受影响测试被选中，动态导入测试强制纳入 =="
$B = Join-Path $Work 'py-basic'
New-File (Join-Path $B '.git\HEAD') "ref: refs/heads/main`n"
New-File (Join-Path $B 'pkg\core.py') "def f(): return 1`n"
New-File (Join-Path $B 'pkg\__init__.py') "from .core import f`n"
New-File (Join-Path $B 'tests\test_core.py') "from pkg.core import f`ndef test_f(): assert f() == 1`n"
New-File (Join-Path $B 'tests\test_other.py') "def test_other(): assert True`n"
New-File (Join-Path $B 'tests\test_dynamic.py') "import importlib`ndef test_d(): assert importlib`n"
$outB = Join-Path $Work 'plan-b.json'
& pwsh -NoProfile -File (Join-Path $tools 'affected.ps1') -Path $B -Changed (Join-Path $B 'pkg\core.py') -Lang python -Out $outB | Out-Null
$planB = Get-Content $outB -Raw | ConvertFrom-Json
Assert-True ($planB.proven -eq $true) 'B1 有图且可达时 proven=true' ($planB.unproven -join ';')
$cmdB = $planB.commands[0]
Assert-True ($cmdB -match 'test_core\.py') 'B2 选中直接导入它的测试' $cmdB
Assert-True ($cmdB -match 'test_dynamic\.py') 'B3 动态导入测试被强制纳入（用纳入消除未知）' $cmdB
Assert-True ($cmdB -notmatch 'test_other\.py') 'B4 无关测试未被选中' $cmdB

# =====================================================================================
"== C. affected(python): 没人导入的改动判 unproven（不允许 跑 0 个算通过）=="
$C = Join-Path $Work 'py-orphan'
New-File (Join-Path $C '.git\HEAD') "ref: refs/heads/main`n"
New-File (Join-Path $C 'pkg\lonely.py') "def g(): return 2`n"
New-File (Join-Path $C 'tests\test_core.py') "def test_x(): assert True`n"
$outC = Join-Path $Work 'plan-c.json'
& pwsh -NoProfile -File (Join-Path $tools 'affected.ps1') -Path $C -Changed (Join-Path $C 'pkg\lonely.py') -Lang python -Out $outC | Out-Null
$planC = Get-Content $outC -Raw | ConvertFrom-Json
Assert-True ($planC.proven -eq $false) 'C1 proven=false' "proven=$($planC.proven)"
Assert-True (@($planC.unproven) -match 'no test file reached').Count -gt 0 'C2 理由为"一个用例都跑不到"' ($planC.unproven -join ';')

# =====================================================================================
"== D. guard preflight: 少写一行 mod 必须判红 =="
$D = Join-Path $Work 'guard'
New-File (Join-Path $D 'crates\trit-orch\tests\kernel\main.rs') "mod a;`n"
New-File (Join-Path $D 'crates\trit-orch\tests\kernel\a.rs') "#[test] fn t_a() {}`n"
$guard = Join-Path $tools 'refactor-guard.ps1'
& pwsh -NoProfile -File $guard -Mode preflight -Repo $D | Out-Null; $codeGood = $LASTEXITCODE
Assert-True ($codeGood -eq 0) 'D1 全部可达时 exit=0' "exit=$codeGood"
New-File (Join-Path $D 'crates\trit-orch\tests\kernel\b.rs') "#[test] fn t_b() {}`n"
& pwsh -NoProfile -File $guard -Mode preflight -Repo $D | Out-Null; $codeBad = $LASTEXITCODE
Assert-True ($codeBad -eq 1) 'D2 有不可达测试文件时 exit=1（拒绝落盘）' "exit=$codeBad"
New-File (Join-Path $D 'crates\trit-orch\tests\kernel\main.rs') "mod a;`nmod b;`n"
& pwsh -NoProfile -File $guard -Mode preflight -Repo $D | Out-Null; $codeFix = $LASTEXITCODE
Assert-True ($codeFix -eq 0) 'D3 补上 mod 后恢复 exit=0' "exit=$codeFix"
New-File (Join-Path $D 'crates\trit-orch\tests\kernel\main.rs') "mod a;`nmod ghost;`n"
& pwsh -NoProfile -File $guard -Mode preflight -Repo $D | Out-Null; $codeGhost = $LASTEXITCODE
Assert-True ($codeGhost -eq 1) 'D4 声明了不存在的模块也判红' "exit=$codeGhost"

# =====================================================================================
"== E. costprobe 记账：同一个模型写两次不得翻倍 =="
$E = Join-Path $Work 'node-proj'
New-File (Join-Path $E 'package.json') "{`"name`":`"e`",`"type`":`"module`"}`n"
New-File (Join-Path $E 'lib\index.js') "export const x = 1;`n"
New-File (Join-Path $E 'test\a.test.js') "import { test } from 'node:test';`nimport assert from 'node:assert';`ntest('x', () => assert.equal(1,1));`n"
$model = Join-Path $Work 'model.json'
$probe = Join-Path $tools 'costprobe.ps1'
& pwsh -NoProfile -File $probe -Path $E -Model $model -IncrementalRuns 2 | Out-Null
$m1 = Get-Content $model -Raw | ConvertFrom-Json
$incr1 = $m1.components[0].incr_total_s
$rounds1 = @($m1.components[0].rounds).Count
& pwsh -NoProfile -File $probe -Path $E -Model $model -IncrementalRuns 2 | Out-Null
$m2 = Get-Content $model -Raw | ConvertFrom-Json
$incr2 = $m2.components[0].incr_total_s
$rounds2 = @($m2.components[0].rounds).Count
Assert-True ($rounds1 -eq 2) 'E1 第一轮写入 2 个 round' "rounds=$rounds1"
Assert-True ($rounds2 -eq 2) 'E2 重复写入后仍是 2 个 round（按 key 去重）' "rounds=$rounds2"
Assert-True ([math]::Abs($incr1 - $incr2) -lt 0.01) 'E3 派生出的总数不变（没有累加）' "incr1=$incr1 incr2=$incr2"

""
"== F. policy: 两轴权限（配置档 × 网络），拒绝必须真的拒绝 =="
$policy = Join-Path $tools 'policy.ps1'
$F = Join-Path $Work 'policy-root'
New-File (Join-Path $F 'src\a.rs') "fn a() {}`n"

function Invoke-Policy([string]$cmd, [string]$profile, [bool]$net) {
    $a = @('-NoProfile', '-File', $script:policy, '-Profile', $profile, '-Command', $cmd, '-Root', $script:F)
    if ($net) { $a += '-AllowNetwork' }
    & pwsh @a | Out-Null
    return $LASTEXITCODE
}

Assert-True ((Invoke-Policy 'Get-Content src\a.rs' 'read-only' $false) -eq 0) 'F1 read-only 允许读' ''
Assert-True ((Invoke-Policy 'Set-Content src\b.rs hi' 'read-only' $false) -eq 1) 'F2 read-only 拒绝写' ''
Assert-True ((Invoke-Policy 'curl https://example.com' 'read-only' $false) -eq 1) 'F3 网络默认关闭（read-only 里也拒）' ''
Assert-True ((Invoke-Policy 'curl https://example.com' 'workspace-write' $false) -eq 1) 'F4 workspace-write 也不能偷偷联网' ''
Assert-True ((Invoke-Policy 'curl https://example.com' 'workspace-write' $true) -eq 0) 'F5 -AllowNetwork 显式授权后才放行' ''
Assert-True ((Invoke-Policy 'cargo test' 'workspace-write' $false) -eq 0) 'F6 workspace-write 允许构建与跑测' ''
Assert-True ((Invoke-Policy 'Set-Content C:\Windows\x.txt hi' 'workspace-write' $false) -eq 1) 'F7 写工作区之外一律拒绝' ''
Assert-True ((Invoke-Policy 'Get-ChildItem; curl https://x' 'workspace-write' $false) -eq 1) 'F8 前置无害命令不能掩护后面的联网（分段求值）' ''
Assert-True ((Invoke-Policy 'git reset --hard' 'full' $false) -eq 1) 'F9 破坏性命令即使 full 档也拒' ''

""
"== G. journal: 快照能证明改了什么，也能还原 =="
$journal = Join-Path $tools 'journal.ps1'
$G = Join-Path $Work 'journal-root'
New-File (Join-Path $G 'keep.txt') "original`n"
New-File (Join-Path $G 'src\code.rs') "fn f() {}`n"

& pwsh -NoProfile -File $journal -Mode begin -Root $G | Out-Null; $gBegin = $LASTEXITCODE
& pwsh -NoProfile -File $journal -Mode end -Root $G | Out-Null; $gClean = $LASTEXITCODE
Assert-True (($gBegin -eq 0) -and ($gClean -eq 0)) 'G1 未改动时 end exit=0' "begin=$gBegin clean=$gClean"

[IO.File]::WriteAllText((Join-Path $G 'keep.txt'), "changed`n")
New-File (Join-Path $G 'added.txt') "new`n"
$gEnd = & pwsh -NoProfile -File $journal -Mode end -Root $G
$gCode = $LASTEXITCODE
Assert-True ($gCode -eq 1) 'G2 有改动时 end exit=1' "exit=$gCode"
Assert-True (@($gEnd) -match 'modified keep\.txt').Count -gt 0 'G3 能点名被改的文件' ($gEnd -join ';')
Assert-True (@($gEnd) -match 'added\s+added\.txt').Count -gt 0 'G4 能点名新增的文件' ($gEnd -join ';')

$gRestore = & pwsh -NoProfile -File $journal -Mode restore -Root $G
$gCode2 = $LASTEXITCODE
Assert-True ($gCode2 -eq 0) 'G5 restore exit=0' "exit=$gCode2"
Assert-True ((Get-Content (Join-Path $G 'keep.txt') -Raw).Trim() -eq 'original') 'G6 被改的文件内容已还原' (Get-Content (Join-Path $G 'keep.txt') -Raw)
Assert-True (-not (Test-Path (Join-Path $G 'added.txt'))) 'G7 restore 删掉了快照之后新增的文件' ''
& pwsh -NoProfile -File $journal -Mode end -Root $G | Out-Null
Assert-True ($LASTEXITCODE -eq 0) 'G8 还原后 end 回到 exit=0（还原是完整的）' ''

""
"== H. ledger: token 与墙钟分列，重复键不重复计数 =="
$ledger = Join-Path $tools 'ledger.ps1'
$L = Join-Path $Work 'ledger.json'
& pwsh -NoProfile -File $ledger -Mode add -Ledger $L -Task 'split' -Key 'k1' -Tokens 1000 -Wall 2 | Out-Null
& pwsh -NoProfile -File $ledger -Mode add -Ledger $L -Task 'split' -Key 'k1' -Tokens 1000 -Wall 2 | Out-Null
& pwsh -NoProfile -File $ledger -Mode add -Ledger $L -Task 'split' -Key 'k2' -Tokens 500 -Wall 3 | Out-Null
$rep = & pwsh -NoProfile -File $ledger -Mode report -Ledger $L
$ldoc = Get-Content $L -Raw | ConvertFrom-Json
Assert-True (@($ldoc.entries).Count -eq 2) 'H1 重复键只记一条' "entries=$(@($ldoc.entries).Count)"
Assert-True (@($rep) -match 'total tokens:\s*1500').Count -gt 0 'H2 token 合计正确（1000+500，不是 2500）' ($rep -join ';')
Assert-True (@($rep) -match 'total wall:\s*5s').Count -gt 0 'H3 墙钟独立合计' ($rep -join ';')

""
"== I. agent-verify: 单一入口把几步串起来，任一步拒绝就整体拒绝 =="
$av = Join-Path $tools 'agent-verify.ps1'

$I1 = Join-Path $Work 'av-ok'
New-File (Join-Path $I1 '.git\HEAD') "ref: refs/heads/main`n"
New-File (Join-Path $I1 'conftest.py') ''
New-File (Join-Path $I1 'pkg\core.py') "def f(): return 1`n"
New-File (Join-Path $I1 'pkg\__init__.py') "from .core import f`n"
New-File (Join-Path $I1 'tests\test_core.py') "from pkg.core import f`ndef test_f(): assert f() == 1`n"
$rc1 = Join-Path $Work 'rc1.json'
& pwsh -NoProfile -File $av -Root $I1 -Changed (Join-Path $I1 'pkg\core.py') -Lang python -Profile workspace-write -Receipt $rc1 -Store (Join-Path $Work 'store1') | Out-Null
$code1 = $LASTEXITCODE
$doc1 = Get-Content $rc1 -Raw | ConvertFrom-Json
$stepNames = @($doc1.steps | ForEach-Object { $_.step })
Assert-True ($code1 -eq 0) 'I1 全链路通过时 exit=0' "exit=$code1"
$core = ($stepNames | Where-Object { $_ -ne 'memory-brief' -and $_ -ne 'memory-note' }) -join ','
Assert-True ($stepNames[0] -eq 'memory-brief') 'I2 第一步永远是读记忆（新会话先知道项目历史）' ($stepNames -join ',')
# checks 步是无条件 add 的（没初始化也以 skipped 留痕），期望串必须包含它；
# 这条断言曾因漏掉 checks 而永远红——见 docs/ERRORS.md L98。
Assert-True ($core -eq 'timeline-due,preflight,checks,affected,policy,constraints,journal-begin,run,journal-end') 'I2b 核心步骤按顺序' $core
Assert-True ($doc1.plan.proven -eq $true) 'I3 回执记录跑了持证子集' "proven=$($doc1.plan.proven)"
Assert-True ($doc1.verdict -match 'proven subset') 'I4 verdict 说明是持证子集' $doc1.verdict

$I5 = Join-Path $Work 'av-unproven'
New-File (Join-Path $I5 'conftest.py') ''
New-File (Join-Path $I5 'pkg\lonely.py') "def g(): return 2`n"
New-File (Join-Path $I5 'tests\test_core.py') "def test_x(): assert True`n"
$rc5 = Join-Path $Work 'rc5.json'
& pwsh -NoProfile -File $av -Root $I5 -Changed (Join-Path $I5 'pkg\lonely.py') -Lang python -DryRun -Receipt $rc5 | Out-Null
$code5 = $LASTEXITCODE
$doc5 = Get-Content $rc5 -Raw | ConvertFrom-Json
Assert-True ($code5 -eq 0) 'I5 unproven 时 dry-run 仍 exit=0（只说明不拦）' "exit=$code5"
Assert-True ($doc5.plan.proven -eq $false) 'I6 回执记录 proven=false' "proven=$($doc5.plan.proven)"
Assert-True (@($doc5.plan.commands)[0] -eq 'pytest -q') 'I7 回退到全量 pytest' (@($doc5.plan.commands) -join ';')

$I8 = Join-Path $Work 'av-fail'
New-File (Join-Path $I8 'conftest.py') ''
New-File (Join-Path $I8 'pkg\core.py') "def f(): return 1`n"
New-File (Join-Path $I8 'tests\test_core.py') "from pkg.core import f`ndef test_write():`n    open('out.txt','w').write('x')`ndef test_fail(): assert f() == 2`n"
$rc8 = Join-Path $Work 'rc8.json'
& pwsh -NoProfile -File $av -Root $I8 -Changed (Join-Path $I8 'pkg\core.py') -Lang python -Profile workspace-write -Receipt $rc8 -Store (Join-Path $Work 'store8') | Out-Null
$code8 = $LASTEXITCODE
$doc8 = Get-Content $rc8 -Raw | ConvertFrom-Json
Assert-True ($code8 -eq 1) 'I8 用例失败时整体 exit=1（失败不会被吞掉）' "exit=$code8"
Assert-True ($doc8.verdict -match 'command failed') 'I9 verdict 点名 command failed' $doc8.verdict
Assert-True (@($doc8.changes) -match 'added\s+out\.txt').Count -gt 0 'I10 journal 抓到命令实际写的文件（不信退出码，信磁盘）' (@($doc8.changes) -join ';')
Assert-True ($doc8.plan.proven -eq $false) 'I11 无 git 历史时回执也是 unproven' "proven=$($doc8.plan.proven)"
$raw8 = [IO.File]::ReadAllText($rc8, (New-Object System.Text.UTF8Encoding($false)))
Assert-True ((($raw8 -replace '[^\x00-\x7F]', '').Length) -eq $raw8.Length) 'I12 回执纯 ASCII（换任何宿主读都不会把引号读坏）' ''

""
"== J. memory: 项目级记忆，智能体死了也能读；内核不许塞进来 =="
$mem = Join-Path $tools 'memory.ps1'
$J = Join-Path $Work 'mem-proj'
New-File (Join-Path $J 'README.md') "proj`n"

& pwsh -NoProfile -File $mem -Mode init -Root $J | Out-Null
Assert-True ($LASTEXITCODE -eq 0) 'J1 init exit=0' ''
Assert-True (Test-Path (Join-Path $J '.agent\memory\meta.json')) 'J2 init 落盘了 meta' ''

# 没有证据的"结论"直接拒收：记忆里的每句话都要能被复核
& pwsh -NoProfile -File $mem -Mode note -Root $J -Kind decision -Title '无证据的结论' -Detail 'x' 2>&1 | Out-Null
Assert-True ($LASTEXITCODE -eq 1) 'J3 无证据的 decision 被拒收（fail-closed）' ""

& pwsh -NoProfile -File $mem -Mode note -Root $J -Kind pitfall -Title '路径里带 test 会把源码全过滤掉' -Detail '用全路径 notmatch test 过滤，目录名含 test 就全丢' -Evidence 'selftest E1 | exit 1 | commit abc1234' -Tags 'tooling' | Out-Null
Assert-True ($LASTEXITCODE -eq 0) 'J4 带证据的条目写入成功' ''
& pwsh -NoProfile -File $mem -Mode note -Root $J -Kind pitfall -Title '路径里带 test 会把源码全过滤掉' -Detail '用全路径 notmatch test 过滤，目录名含 test 就全丢' -Evidence 'selftest E1 | exit 1 | commit abc1234' -Tags 'tooling' | Out-Null
$entries = @([IO.File]::ReadAllText((Join-Path $J '.agent\memory\log.jsonl')) -split "`n" | Where-Object { $_.Trim() })
Assert-True ($entries.Count -eq 1) 'J5 同一条主张写两次仍是一条（不堆重复）' "entries=$($entries.Count)"

# 智能体"换了一个进程"也要能读到：这里就是新进程，零上下文
$bri = & pwsh -NoProfile -File $mem -Mode brief -Root $J
Assert-True (@($bri) -match '路径里带 test').Count -gt 0 'J6 新进程只靠磁盘就读到了历史坑' ''
Assert-True (@($bri).Count -le 42) 'J7 brief 有硬行数上限（每会话都付的上下文成本）' "lines=$(@($bri).Count)"

$hit = & pwsh -NoProfile -File $mem -Mode recall -Root $J -Query 'test'
Assert-True (@($hit) -match 'pitfall').Count -gt 0 'J8 recall 能按词取回条目' (@($hit) -join ';')
$miss = & pwsh -NoProfile -File $mem -Mode recall -Root $J -Query '不存在的词'
Assert-True (@($miss) -match 'no hit').Count -gt 0 'J9 取不到时明确说没有，而不是空返回' (@($miss) -join ';')

# 手工塞一条无证据的进日志，audit 必须判红
[IO.File]::AppendAllText((Join-Path $J '.agent\memory\log.jsonl'),
    ('{"id":"mFAKE001","at":"2026-01-01T00:00:00","kind":"fact","scope":"project","title":"手写条目","detail":"","evidence":"","tags":"","key":"fact|手写条目|x"}' + "`n"),
    (New-Object System.Text.UTF8Encoding($false)))
& pwsh -NoProfile -File $mem -Mode audit -Root $J | Out-Null
Assert-True ($LASTEXITCODE -eq 1) 'J10 audit 抓到手工塞进来的无证据条目' ''

# 内核不许塞进项目记忆
& pwsh -NoProfile -File $mem -Mode note -Root $J -Kind fact -Scope kernel -Title '内核内部实现' -Detail 'x' -Evidence 'y' 2>&1 | Out-Null
Assert-True ($LASTEXITCODE -eq 1) 'J11 project 仓拒绝 scope=kernel' ''
& pwsh -NoProfile -File $mem -Mode note -Root $J -Kind fact -Title '外部路径' -Detail '见 D:\KF\trit\src\lib.rs' -Evidence 'z' 2>&1 | Out-Null
Assert-True ($LASTEXITCODE -eq 1) 'J12 证据里出现工作区外路径也拒收' ''
& pwsh -NoProfile -File $mem -Mode note -Root $J -Kind todo -Title 'password=abc123 忘了改' 2>&1 | Out-Null
Assert-True ($LASTEXITCODE -eq 1) 'J13 疑似凭据的内容拒收' ''

# 把记忆目录整份复制到另一个项目，必须被 owner 校验挡住
$J2 = Join-Path $Work 'mem-other'
New-File (Join-Path $J2 'x.txt') 'y'
Copy-Item (Join-Path $J '.agent') (Join-Path $J2 '.agent') -Recurse -Force
& pwsh -NoProfile -File $mem -Mode note -Root $J2 -Kind pitfall -Title '搬家过来的记忆' -Detail 'x' -Evidence 'y' 2>&1 | Out-Null
Assert-True ($LASTEXITCODE -eq 1) 'J14 记忆不可跨项目搬运（owner_root 校验）' ''

# J15: 项目开了记忆之后，入口跑完会自动落一条带证据的命令记录（下次会话可读）
$J3 = Join-Path $Work 'av-mem'
New-File (Join-Path $J3 'conftest.py') ''
New-File (Join-Path $J3 'pkg\core.py') "def f(): return 1`n"
New-File (Join-Path $J3 'tests\test_core.py') "from pkg.core import f`ndef test_f(): assert f() == 1`n"
& pwsh -NoProfile -File $mem -Mode init -Root $J3 | Out-Null
$rc3 = Join-Path $Work 'rc3.json'
& pwsh -NoProfile -File $av -Root $J3 -Changed (Join-Path $J3 'pkg\core.py') -Lang python -Receipt $rc3 -Store (Join-Path $Work 'store3') | Out-Null
$doc3 = Get-Content $rc3 -Raw | ConvertFrom-Json
$names3 = @($doc3.steps | ForEach-Object { $_.step })
Assert-True ($names3[-1] -eq 'memory-note') 'J15 开了记忆时最后一步是落账' ($names3 -join ',')
$bri3 = & pwsh -NoProfile -File $mem -Mode brief -Root $J3
Assert-True (@($bri3) -match 'pytest').Count -gt 0 'J16 下次会话的 brief 里能看到上次跑过什么' (@($bri3) -join ';')

""
"== K. constraints: 禁止操作要能被机器判红 =="
$cons = Join-Path $tools 'constraints.ps1'
$K = Join-Path $Work 'cons-proj'
New-File (Join-Path $K 'crates\trit-orch\src\lib.rs') "pub fn a() {}`npub fn b() {}`n"
New-File (Join-Path $K 'crates\trit-orch\src\verdict.rs') "pub fn derive() {}`n"
New-File (Join-Path $K 'crates\trit-orch\tests\kernel\main.rs') "mod a;`n"
New-File (Join-Path $K 'crates\trit-orch\tests\kernel\a.rs') "#[test] fn t_one() {}`n#[test] fn t_two() {}`n"
& git -C $K init -q 2>&1 | Out-Null
& git -C $K -c user.email=s@t -c user.name=s add -A 2>&1 | Out-Null
& git -C $K -c user.email=s@t -c user.name=s commit -qm base 2>&1 | Out-Null
& pwsh -NoProfile -File $cons -Mode init -Root $K | Out-Null
Assert-True ($LASTEXITCODE -eq 0) 'K1 init 写出默认规则' ''
& pwsh -NoProfile -File $cons -Mode check -Root $K | Out-Null
Assert-True ($LASTEXITCODE -eq 0) 'K2 无改动时 pass' ''

function Reset-K {
    & git -C $script:K reset -q --hard 2>&1 | Out-Null
    # -e .agent: the constraint table is untracked in this fixture; a plain clean would
    # delete it and every later case would silently report "not initialised".
    & git -C $script:K clean -fdq -e .agent 2>&1 | Out-Null
}

New-File (Join-Path $K 'crates\trit-orch\tests\kernel\a.rs') "#[test] fn t_one() {}`n"
& pwsh -NoProfile -File $cons -Mode check -Root $K | Out-Null
$k3 = $LASTEXITCODE
Assert-True ($k3 -eq 1) 'K3 删掉一个 #[test] 判红' "exit=$k3"
Reset-K

New-File (Join-Path $K 'crates\trit-orch\src\lib.rs') "pub fn a() {}`npub fn b() {}`nfn c() { unsafe { } }`n"
& pwsh -NoProfile -File $cons -Mode check -Root $K | Out-Null
Assert-True ($LASTEXITCODE -eq 1) 'K4 新增 unsafe 不给 SAFETY 说明判红' ''
Reset-K

New-File (Join-Path $K 'crates\trit-orch\src\lib.rs') "pub fn a() {}`npub fn b() {}`nfn c() { // SAFETY: 边界已校验`n    unsafe { } }`n"
& pwsh -NoProfile -File $cons -Mode check -Root $K | Out-Null
Assert-True ($LASTEXITCODE -eq 0) 'K5 给了 SAFETY 说明就放行' ''
Reset-K

New-File (Join-Path $K 'crates\trit-orch\src\lib.rs') "pub fn a() {}`n"
& pwsh -NoProfile -File $cons -Mode check -Root $K | Out-Null
Assert-True ($LASTEXITCODE -eq 1) 'K6 删掉 pub fn 判红（公开面变化）' ''
& pwsh -NoProfile -File $cons -Mode check -Root $K -Allow 'public-api-change' | Out-Null
Assert-True ($LASTEXITCODE -eq 0) 'K7 显式 -Allow 放行' ''
Reset-K

New-File (Join-Path $K 'crates\trit-orch\src\verdict.rs') "pub fn derive() {}`n// Forecast 只用于提示`n"
& pwsh -NoProfile -File $cons -Mode check -Root $K 2>&1 | Out-Null
Assert-True ($LASTEXITCODE -eq 1) 'K8 Forecast 出现在判定层判红' ''
Reset-K

New-File (Join-Path $K 'crates\trit-orch\src\lib.rs') "pub fn a() {}`npub fn b() {}`nfn c() { dbg!(1); }`n"
& pwsh -NoProfile -File $cons -Mode check -Root $K | Out-Null
Assert-True ($LASTEXITCODE -eq 1) 'K9 dbg! 进 src 判红' ''
Reset-K

New-File (Join-Path $K 'crates\trit-orch\src\lib.rs') "pub fn a() {}`npub fn b() {}`nfn c() { let x = 1i32; x.to_string().unwrap(); }`n"
$k10 = & pwsh -NoProfile -File $cons -Mode check -Root $K
Assert-True ($LASTEXITCODE -eq 0) 'K10 新增 unwrap 只 warn 不拦' "exit=$LASTEXITCODE"
Assert-True (@($k10) -match 'WARN').Count -gt 0 'K11 warn 要被打印出来（不是静默通过）' (@($k10) -join ';')
Reset-K

# J17: 过期结论要能原地更正，而不是留一条旧的在旁边
& pwsh -NoProfile -File $mem -Mode note -Root $J3 -Kind command -Title 'selftest 通过条数' -Detail '64 条' -Evidence 'selftest | exit 0 | 64 passed' -Tags 'verify' | Out-Null
$before = @([IO.File]::ReadAllText((Join-Path $J3 '.agent\memory\log.jsonl')) -split "`n" | Where-Object { $_.Trim() }).Count
$revId = (@([IO.File]::ReadAllText((Join-Path $J3 '.agent\memory\log.jsonl')) -split "`n" | Where-Object { $_ -match 'selftest 通过条数' }) | Select-Object -Last 1 | ConvertFrom-Json).id
& pwsh -NoProfile -File $mem -Mode revise -Root $J3 -Id $revId -Detail '75 条' -Evidence 'selftest | exit 0 | 75 passed' | Out-Null
$revOk = $LASTEXITCODE
$after = @([IO.File]::ReadAllText((Join-Path $J3 '.agent\memory\log.jsonl')) -split "`n" | Where-Object { $_.Trim() }).Count
Assert-True ($revOk -eq 0) 'J17 revise 成功' "exit=$revOk"
Assert-True ($after -eq $before) 'J18 revise 不新增条目（原地更正）' "before=$before after=$after"
$bri3b = & pwsh -NoProfile -File $mem -Mode brief -Root $J3
$rec3b = & pwsh -NoProfile -File $mem -Mode recall -Root $J3 -Query '通过条数'
Assert-True (@($rec3b) -match '75 条').Count -gt 0 'J19 更正后取回的是新细节（brief 只列标题，细节走 recall）' (@($rec3b) -join ';')
Assert-True (@($rec3b | Where-Object { $_ -match '64 条' }).Count -eq 0) 'J20 旧值没有被留在旁边' (@($rec3b) -join ';')

""
"== L. timeline: 问题当时间序列，边界会到期，接受也不等于永久 =="
$tl = Join-Path $tools 'timeline.ps1'
$Lr = Join-Path $Work 'tl-proj'
New-File (Join-Path $Lr 'x.txt') 'y'

$l1 = & pwsh -NoProfile -File $tl -Mode open -Root $Lr -Kind boundary -Title 'Rust 受影响分析只到包级' -ReviewAfterDays 14 -Evidence 'affected.ps1 只按包算' -Now '2026-01-01T00:00:00'
$l1id = ($l1 -join ' ') -replace '.*opened (\w+).*', '$1'
Assert-True ($LASTEXITCODE -eq 0) 'L1 open 成功' "exit=$LASTEXITCODE"
Assert-True ($l1id -match '^b') 'L2 open 返回了可引用的 id' $l1id

$dup = & pwsh -NoProfile -File $tl -Mode open -Root $Lr -Kind boundary -Title 'Rust 受影响分析只到包级' -Evidence 'again' -Now '2026-01-01T00:00:00'
Assert-True ($LASTEXITCODE -eq 3) 'L3 同标题不收第二条（提示去 touch）' "exit=$LASTEXITCODE"

& pwsh -NoProfile -File $tl -Mode touch -Root $Lr -Id $l1id -Evidence '第二次又撞上' -Now '2026-01-03T00:00:00' | Out-Null
$doc = [IO.File]::ReadAllText((Join-Path $Lr '.agent\timeline.json'), (New-Object System.Text.UTF8Encoding($false))) | ConvertFrom-Json
Assert-True (@($doc.items[0].occurrences).Count -eq 2) 'L4 touch 追加一次发生（不是新开条目）' "occurrences=$(@($doc.items[0].occurrences).Count)"

& pwsh -NoProfile -File $tl -Mode due -Root $Lr -Now '2026-01-05T00:00:00' | Out-Null
Assert-True ($LASTEXITCODE -eq 0) 'L5 未到期时 due 不报警' ''
$dueOut = & pwsh -NoProfile -File $tl -Mode due -Root $Lr -Now '2026-01-20T00:00:00'
Assert-True ($LASTEXITCODE -eq 1) 'L6 过了复核期 due 报警（暂时的问题必须回头看一眼）' "exit=$LASTEXITCODE"
Assert-True (@($dueOut) -match '已存活').Count -gt 0 'L7 到期项带上存活天数与发生次数' (@($dueOut) -join ';')

& pwsh -NoProfile -File $tl -Mode resolve -Root $Lr -Id $l1id -Now '2026-01-21T00:00:00' 2>&1 | Out-Null
Assert-True ($LASTEXITCODE -eq 1) 'L8 无证据结案被拒（"修好了"不能只是句话）' ''
& pwsh -NoProfile -File $tl -Mode resolve -Root $Lr -Id $l1id -Evidence '改为按模块计算，实测 3.1s' -Now '2026-01-21T00:00:00' | Out-Null
$tr = & pwsh -NoProfile -File $tl -Mode trend -Root $Lr -Id $l1id
Assert-True (@($tr) -match '结案').Count -gt 0 'L9 trend 里能看到结案事件' (@($tr) -join ';')
Assert-True (@($tr | Where-Object { $_ -match '存活' }).Count -eq 0) 'L10 trend 不再把它当未解决' ''
& pwsh -NoProfile -File $tl -Mode due -Root $Lr -Now '2026-02-01T00:00:00' | Out-Null
Assert-True ($LASTEXITCODE -eq 0) 'L11 已结案的条目不再到期' ''

# 接受 ≠ 永久：accept 之后自己的期限到了，必须重新出现
$l12 = & pwsh -NoProfile -File $tl -Mode open -Root $Lr -Kind limitation -Title 'Node 适配器未在真项目验证' -ReviewAfterDays 7 -Evidence '只在 fixture 上过' -Now '2026-02-01T00:00:00'
$l12id = ($l12 -join ' ') -replace '.*opened (\w+).*', '$1'
& pwsh -NoProfile -File $tl -Mode accept -Root $Lr -Id $l12id -ReviewAfterDays 30 -Evidence '本轮不做，先看 dsh-orch 依赖补齐' -Now '2026-02-02T00:00:00' | Out-Null
& pwsh -NoProfile -File $tl -Mode due -Root $Lr -Now '2026-02-10T00:00:00' | Out-Null
Assert-True ($LASTEXITCODE -eq 0) 'L12 接受后的新期限内不报警' ''
& pwsh -NoProfile -File $tl -Mode due -Root $Lr -Now '2026-03-10T00:00:00' | Out-Null
Assert-True ($LASTEXITCODE -eq 1) 'L13 接受不等于永久：自己的期限到了还会再出现' ''

""
"== M. bash 入口：默认执行面，且与 PS 台账互通 =="
# 5.1 是坏数据的来源（UTF-8 当 ANSI 读），所以默认执行面是 bash。这里既验证 bash 入口
# 的行为与 L 组一致，也验证两边的 JSON 是同一份（读写互通），还验证 5.1 再也进不来。
$bashExe = @('C:\Program Files\Git\bin\bash.exe', 'C:\Program Files\Git\usr\bin\bash.exe', '/usr/bin/bash') |
    Where-Object { Test-Path $_ } | Select-Object -First 1
$skipped = 0
if (-not $bashExe) {
    "  SKIP  M0 这台机器没有 bash，跳过 bash 入口检查（默认执行面缺失本身就是问题）"
    $skipped++
} else {
    function ToPosix([string]$p) {
        $x = $p -replace '\\', '/'
        if ($x -match '^([A-Za-z]):') { $x = '/' + $matches[1].ToLower() + $x.Substring(2) }
        return $x
    }
    $sh = ToPosix (Join-Path $tools 'timeline.sh')
    $Mr = Join-Path $Work 'tl-bash'
    New-File (Join-Path $Mr 'x.txt') 'y'

    $b1 = & $bashExe $sh -Mode open -Root (ToPosix $Mr) -Kind boundary -Title 'Rust 减集只到包级' -Evidence '只按包算' -ReviewAfterDays 14 -Now '2026-01-01T00:00:00'
    $b1code = $LASTEXITCODE
    $bid = (@($b1) -join ' ') -replace '.*opened (\w+).*', '$1'
    Assert-True ($b1code -eq 0) 'M1 bash open 成功' "exit=$b1code"
    Assert-True ($bid -match '^b') 'M2 bash open 返回 id' $bid

    & $bashExe $sh -Mode open -Root (ToPosix $Mr) -Kind boundary -Title 'Rust 减集只到包级' -Evidence dup -Now '2026-01-01T00:00:00' | Out-Null
    Assert-True ($LASTEXITCODE -eq 3) 'M3 bash 同标题不收第二条（与 L3 一致）' "exit=$LASTEXITCODE"

    & $bashExe $sh -Mode touch -Root (ToPosix $Mr) -Id $bid -Evidence '又撞上' -Now '2026-01-03T00:00:00' | Out-Null
    $bdoc = [IO.File]::ReadAllText((Join-Path $Mr '.agent\timeline.json'), (New-Object System.Text.UTF8Encoding($false))) | ConvertFrom-Json
    Assert-True (@($bdoc.items[0].occurrences).Count -eq 2) 'M4 bash touch 追加发生' "occurrences=$(@($bdoc.items[0].occurrences).Count)"

    & $bashExe $sh -Mode due -Root (ToPosix $Mr) -Now '2026-01-05T00:00:00' | Out-Null
    Assert-True ($LASTEXITCODE -eq 0) 'M5 bash 未到期不报警' ''
    & $bashExe $sh -Mode due -Root (ToPosix $Mr) -Now '2026-01-20T00:00:00' | Out-Null
    Assert-True ($LASTEXITCODE -eq 1) 'M6 bash 到期报警' ''

    & $bashExe $sh -Mode resolve -Root (ToPosix $Mr) -Id $bid -Now '2026-01-21T00:00:00' | Out-Null
    Assert-True ($LASTEXITCODE -eq 1) 'M7 bash 无证据结案被拒' ''

    # 双向互通：bash 写的台账，PS 转发器读得懂；PS 写的台账，bash 也读得懂。
    $psList = @(& pwsh -NoProfile -File $tl -Mode list -Root $Mr)
    Assert-True (@($psList | Where-Object { $_ -match [regex]::Escape($bid) }).Count -eq 1) 'M8 bash 写的台账 PS 读得到' (@($psList) -join ' / ')
    $bList = @(& $bashExe $sh -Mode list -Root (ToPosix 'D:\KF\trit-inv'))
    Assert-True (@($bList).Count -ge 2) 'M9 bash 读得懂 PS 写的内核台账（interop）' "rows=$(@($bList).Count)"

    # 5.1 必须进不来：不是偏好，是数据完整性。
    if (Get-Command powershell.exe -ErrorAction SilentlyContinue) {
        $g = & powershell.exe -NoProfile -File (Join-Path $tools 'memory.ps1') -Mode brief -Root $Mr 2>&1
        $gc = $LASTEXITCODE
        Assert-True ($gc -ne 0) 'M10 5.1 直跑工具被版本闸拒绝' "exit=$gc"
        Assert-True ((@($g) -join ' ') -match '#requires|requires') 'M11 拒绝理由说清是 #requires' (@($g) -join ' ' | ForEach-Object { $_.Substring(0, [math]::Min(60, $_.Length)) })
    } else {
        "  SKIP  M10 没有 powershell.exe，跳过 5.1 拒绝检查"
        $skipped++
    }
}

""
$enc = New-Object System.Text.UTF8Encoding($false)

"== N. affected / costprobe：迁移不改行为，且测量不再被 Shell 污染 =="
# 迁移前后落点一致是硬要求（不一致就等于"减集"悄悄换了含义）；同时 costprobe 的每个
# 阶段不再经过 Shell 启动，否则被测项目的成本里会混进测量工具自己的启动成本。
$shAff = Join-Path $tools 'affected.sh'
$shCp = Join-Path $tools 'costprobe.sh'
if (-not $bashExe) {
    "  SKIP  N0 没有 bash，跳过 affected/costprobe 的 bash 入口检查"
    $skipped += 5
} else {
    $Npy = Join-Path $Work 'n-pyfix'
    New-Item -ItemType Directory "$Npy\.git", "$Npy\tests", "$Npy\pkg" -Force | Out-Null
    $nl = [char]10
    [IO.File]::WriteAllText("$Npy\pyproject.toml", (@('[project]', 'name="pyfix"', 'version="0.1"', '') -join $nl), $enc)
    [IO.File]::WriteAllText("$Npy\pkg\core.py", 'VALUE = 1' + $nl, $enc)
    [IO.File]::WriteAllText("$Npy\pkg\registry.py", 'from pkg.core import VALUE' + $nl, $enc)
    [IO.File]::WriteAllText("$Npy\tests\test_core.py", 'from pkg.core import VALUE' + $nl, $enc)
    [IO.File]::WriteAllText("$Npy\tests\test_registry.py", 'from pkg.registry import VALUE' + $nl, $enc)
    [IO.File]::WriteAllText("$Npy\tests\test_other.py", 'def test_x(): assert 1' + $nl, $enc)
    [IO.File]::WriteAllText("$Npy\tests\test_dyn.py", (@('import importlib', 'mod = importlib.import_module("pkg.core")', 'def test_y(): assert mod.VALUE') -join $nl) + $nl, $enc)

    $n1 = @(& $bashExe $shAff -Path (ToPosix 'D:\KF\trit-inv') -Changed 'crates/trit-orch/src/obligation.rs' -Lang rust 2>&1)
    Assert-True (@($n1 | Where-Object { $_ -match 'proven=True' }).Count -eq 1) 'N1 rust 减集仍判 proven' (@($n1) -join ' / ')
    Assert-True (@($n1 | Where-Object { $_ -match 'cargo test -p trit-orch --no-fail-fast' }).Count -eq 1) 'N2 落点仍是 trit-orch 单包' (@($n1) -join ' / ')

    $n3 = @(& $bashExe $shAff -Path (ToPosix $Npy) -Changed 'pkg/core.py' -Lang python 2>&1)
    $planLine = (@($n3) | Where-Object { $_ -match 'plan: pytest' }) -join ' '
    Assert-True ($planLine -match 'test_dyn' -and $planLine -match 'test_registry' -and $planLine -notmatch 'test_other') 'N3 动态导入测试强制纳入、无关测试排除' $planLine

    $Nmulti = Join-Path $Work 'n-multi'
    New-Item -ItemType Directory "$Nmulti\.git", "$Nmulti\src" -Force | Out-Null
    [IO.File]::WriteAllText("$Nmulti\Cargo.toml", (@('[package]', 'name="x"', 'version="0.1.0"', '') -join $nl), $enc)
    [IO.File]::WriteAllText("$Nmulti\package.json", '{"name":"x"}', $enc)
    [IO.File]::WriteAllText("$Nmulti\src\a.rs", 'fn main(){}' + $nl, $enc)
    $n4 = @(& $bashExe $shAff -Path (ToPosix $Nmulti) -Changed 'src/a.rs' -Lang rust 2>&1)
    Assert-True (@($n4 | Where-Object { $_ -match 'proven=False' }).Count -eq 1 -and (@($n4) -join ' ') -match 'workspace') 'N4 多语言未声明隔离时回退全量' (@($n4) -join ' / ')

    $Nrs = Join-Path $Work 'n-rust'
    New-Item -ItemType Directory "$Nrs\src" -Force | Out-Null
    [IO.File]::WriteAllText("$Nrs\Cargo.toml", (@('[package]', 'name="nr"', 'version="0.1.0"', 'edition="2021"', '', '[workspace]', '') -join $nl), $enc)
    [IO.File]::WriteAllText("$Nrs\src\lib.rs", (@('pub fn add(a: i32, b: i32) -> i32 { a + b }', '', '#[cfg(test)]', 'mod tests {', '    use super::*;', '    #[test]', '    fn t() { assert_eq!(add(1, 2), 3); }', '}') -join $nl) + $nl, $enc)
    $cpOut = Join-Path $Work 'n-cost.json'
    $n5 = @(& $bashExe $shCp -Path (ToPosix $Nrs) -Out (ToPosix $cpOut) -IncrementalRuns 2 -WorkRoot (ToPosix (Join-Path $Work 'cp')) -ErrorAction SilentlyContinue 2>&1)
    $cpDoc = Get-Content $cpOut -Raw | ConvertFrom-Json
    Assert-True ($cpDoc.components[0].valid -eq $true -and $null -ne $cpDoc.components[0].cold_total_s) 'N5 costprobe 在可构建仓上报出冷/增量数字' (@($n5) -join ' / ')
    Assert-True (@($cpDoc.components[0].stage_cold).Count -eq 3) 'N6 三段（lint/build/run）都在' ''
    $cmds = (@($cpDoc.components[0].stage_cold) | ForEach-Object { $_.cmd }) -join ' '
    Assert-True ($cmds -notmatch '\$env:') 'N7 记录的 cmd 是人能直接执行的（环境变量不再拼进命令串）' $cmds

    $Nbad = Join-Path $Work 'n-bad'
    New-Item -ItemType Directory "$Nbad\src" -Force | Out-Null
    [IO.File]::WriteAllText("$Nbad\Cargo.toml", (@('[package]', 'name="nb"', 'version="0.1.0"', 'edition="2021"', '', '[workspace]', '') -join $nl), $enc)
    [IO.File]::WriteAllText("$Nbad\src\lib.rs", 'pub fn broken( {' + $nl, $enc)
    $badOut = Join-Path $Work 'n-bad.json'
    & $bashExe $shCp -Path (ToPosix $Nbad) -Out (ToPosix $badOut) -IncrementalRuns 1 -WorkRoot (ToPosix (Join-Path $Work 'cp')) 2>&1 | Out-Null
    $badDoc = Get-Content $badOut -Raw | ConvertFrom-Json
    Assert-True ($badDoc.components[0].valid -eq $false -and $null -eq $badDoc.components[0].cold_total_s) 'N8 构建失败时不报数字（失败看起来很快，快看起来便宜）' "valid=$($badDoc.components[0].valid) cold=$($badDoc.components[0].cold_total_s)"
}

""
"== O. memory 的 bash 入口：裸 mode、去重、预算、拒绝 =="
if (-not $bashExe) {
    "  SKIP  O0 没有 bash，跳过 memory 入口检查"
    $skipped += 7
} else {
    $shMem = Join-Path $tools 'memory.sh'
    $Om = Join-Path $Work 'o-mem'
    New-Item -ItemType Directory $Om -Force | Out-Null

    & $bashExe $shMem init -Root (ToPosix $Om) | Out-Null
    Assert-True ($LASTEXITCODE -eq 0 -and (Test-Path (Join-Path $Om '.agent\memory\meta.json'))) 'O1 裸 mode 也能 init（不用写 -Mode）' "exit=$LASTEXITCODE"

    $o2 = @(& $bashExe $shMem note -Root (ToPosix $Om) -Kind pitfall -Title 'mtime 被保留会让 cargo 判未改动' -Detail 'Copy-Item 保时间戳' -Evidence 'cargo build | noop 56.9s' 2>&1)
    Assert-True (@($o2 | Where-Object { $_ -match 'recorded m' }).Count -eq 1) 'O2 bash note 落一条' (@($o2) -join ' / ')

    & $bashExe $shMem note -Root (ToPosix $Om) -Kind pitfall -Title 'mtime 被保留会让 cargo 判未改动' -Detail 'Copy-Item 保时间戳' -Evidence 'cargo build | noop 56.9s' 2>&1 | Out-Null
    $ol = [IO.File]::ReadAllText((Join-Path $Om '.agent\memory\log.jsonl'), $enc)
    Assert-True ((@($ol -split "`n" | Where-Object { $_.Trim() }).Count -eq 1)) 'O3 同一 claim 不叠加（只刷新时间）' "lines=$(@($ol -split "`n" | Where-Object { $_.Trim() }).Count)"

    & $bashExe $shMem note -Root (ToPosix $Om) -Kind decision -Title '无证据的决策' -Detail 'x' 2>&1 | Out-Null
    Assert-True ($LASTEXITCODE -eq 1) 'O4 bash 拒绝无证据条目' "exit=$LASTEXITCODE"

    & $bashExe $shMem note -Root (ToPosix $Om) -Kind fact -Title '外部路径' -Detail 'ok' -Evidence 'cat D:\Windows\win.ini | exit 0' 2>&1 | Out-Null
    Assert-True ($LASTEXITCODE -eq 1) 'O5 bash 拒绝工作区外路径的证据' "exit=$LASTEXITCODE"

    $o6 = @(& $bashExe $shMem brief -Root (ToPosix $Om) -MaxLines 6 2>&1)
    $o6txt = (@($o6) -join "`n")
    Assert-True ($o6txt -match '已截断') 'O6 brief 超预算时明说被截断（不静默丢内容）' $o6txt.Substring(0, [math]::Min(60, $o6txt.Length))

    & $bashExe $shMem audit -Root (ToPosix $Om) 2>&1 | Out-Null
    Assert-True ($LASTEXITCODE -eq 0) 'O7 bash audit 干净' "exit=$LASTEXITCODE"
}

""
"== P. constraints 的 bash 入口：规则真的在跑，范围真的生效 =="
if (-not $bashExe) {
    "  SKIP  P0 没有 bash，跳过 constraints 入口检查"
    $skipped += 7
} else {
    $shCon = Join-Path $tools 'constraints.sh'
    $Pc = Join-Path $Work 'p-con'
    New-Item -ItemType Directory "$Pc\.agent" -Force | Out-Null
    & $bashExe $shCon init -Root (ToPosix $Pc) 2>&1 | Out-Null
    $p1 = @(& $bashExe $shCon list -Root (ToPosix $Pc) 2>&1)
    Assert-True (@($p1 | Where-Object { $_ -match 'no-test-deletion' }).Count -eq 1 -and @($p1).Count -eq 7) 'P1 六条默认规则 + 表头都在' "rows=$(@($p1).Count)"

    $pd = Join-Path $Work 'p-diffs'; New-Item -ItemType Directory $pd -Force | Out-Null
    $nl = [char]10
    [IO.File]::WriteAllText("$pd\debug.patch", (@('--- a/crates/x/src/a.rs', '+++ b/crates/x/src/a.rs', '@@ -1,2 +1,3 @@', ' fn f() {', '+    println!("hi");', ' }') -join $nl) + $nl, $enc)
    $p2 = @(& $bashExe $shCon check -Root (ToPosix $Pc) -Diff (ToPosix "$pd\debug.patch") 2>&1)
    Assert-True ($LASTEXITCODE -eq 1 -and (@($p2) -join ' ') -match 'BLOCK \[no-debug-output\]') 'P2 调试输出被拦（exit 1）' (@($p2) -join ' / ')

    [IO.File]::WriteAllText("$pd\unsafe-ok.patch", (@('--- a/crates/x/src/a.rs', '+++ b/crates/x/src/a.rs', '@@ -1,2 +1,4 @@', ' fn f() {', '+    // SAFETY: 越界已在外面判过', '+    let x = unsafe { 1 };', ' }') -join $nl) + $nl, $enc)
    & $bashExe $shCon check -Root (ToPosix $Pc) -Diff (ToPosix "$pd\unsafe-ok.patch") 2>&1 | Out-Null
    Assert-True ($LASTEXITCODE -eq 0) 'P3 有 // SAFETY: 时放行（companion 满足）' "exit=$LASTEXITCODE"

    [IO.File]::WriteAllText("$pd\unwrap.patch", (@('--- a/crates/x/src/a.rs', '+++ b/crates/x/src/a.rs', '@@ -1,2 +1,3 @@', ' fn f() {', '+    let v = g().unwrap();', ' }') -join $nl) + $nl, $enc)
    $p4 = @(& $bashExe $shCon check -Root (ToPosix $Pc) -Diff (ToPosix "$pd\unwrap.patch") 2>&1)
    Assert-True ($LASTEXITCODE -eq 0 -and (@($p4) -join ' ') -match 'WARN \[no-new-unwrap\]') 'P4 warn 只报不拦（exit 0）' (@($p4) -join ' / ')

    & $bashExe $shCon check -Root (ToPosix $Pc) -Diff (ToPosix "$pd\unsafe-ok.patch") -Allow 'unsafe-needs-safety-note' 2>&1 | Out-Null
    Assert-True ($LASTEXITCODE -eq 0) 'P5 -Allow 显式放行可用' "exit=$LASTEXITCODE"

    # 这一条是回归测试：历史上的 bug 是"先 escape 再替换 **"，通配符变成字面量后
    # 所有按路径作用域的规则静默不触发——规则表看起来在，实际一条都没跑。
    $probeFile = Join-Path $Work 'p_glob_probe.py'
    [IO.File]::WriteAllText($probeFile, (@(
            'import sys',
            "sys.path.insert(0, r'D:\KF\trit-inv\tools')",
            'import constraints as c',
            "print(c.in_scope('crates/trit-orch/src/a/b.rs', 'crates/**/src/**'),",
            "      c.in_scope('tools/x.py', 'crates/**/src/**'))"
        ) -join $nl) + $nl, $enc)
    $probe = @(& $bashExe -c "python3 '$(ToPosix $probeFile)'" 2>&1)
    Assert-True ((@($probe) -join ' ') -match 'True False') 'P6 路径作用域真的匹配（** 未被 escape 成字面量）' (@($probe) -join ' ')

    [IO.File]::WriteAllText("$pd\outside.patch", (@('--- a/tools/x.py', '+++ b/tools/x.py', '@@ -1,1 +1,2 @@', ' print(1)', '+print(unsafe)') -join $nl) + $nl, $enc)
    & $bashExe $shCon check -Root (ToPosix $Pc) -Diff (ToPosix "$pd\outside.patch") 2>&1 | Out-Null
    Assert-True ($LASTEXITCODE -eq 0) 'P7 范围外的文件不误伤' "exit=$LASTEXITCODE"
}

""
"== Q. journal 的 bash 入口：快照、比对、还原、mtime =="
if (-not $bashExe) {
    "  SKIP  Q0 没有 bash，跳过 journal 入口检查"
    $skipped += 7
} else {
    $shJ = Join-Path $tools 'journal.sh'
    $Jl = Join-Path $Work 'q-jl'
    New-Item -ItemType Directory "$Jl\src" -Force | Out-Null
    New-Item -ItemType Directory "$Jl\node_modules" -Force | Out-Null
    New-Item -ItemType Directory "$Jl\target" -Force | Out-Null
    [IO.File]::WriteAllText("$Jl\README.md", "hi$nl", $enc)
    [IO.File]::WriteAllText("$Jl\src\a.rs", "fn a() {}$nl", $enc)
    [IO.File]::WriteAllText("$Jl\node_modules\x.js", "x=1$nl", $enc)
    [IO.File]::WriteAllText("$Jl\target\y.bin", "bin$nl", $enc)

    $q0 = @(& $bashExe $shJ end -Root (ToPosix $Jl) 2>&1)
    Assert-True ($LASTEXITCODE -eq 2 -and (@($q0) -join ' ') -match 'no snapshot') 'Q0 没有快照时 end 拒绝（exit 2，不静默通过）' "exit=$LASTEXITCODE"

    $q1 = @(& $bashExe $shJ begin -Root (ToPosix $Jl) 2>&1)
    Assert-True (@($q1) -join ' ' -match 'begin -- 2 files') 'Q1 只快照 2 个文件（node_modules/target 被跳过）' (@($q1) -join ' / ')

    $q2 = @(& $bashExe $shJ end -Root (ToPosix $Jl) 2>&1)
    Assert-True ($LASTEXITCODE -eq 0 -and (@($q2) -join ' ') -match '0 changed') 'Q2 无改动时 0 changed（exit 0）' (@($q2) -join ' / ')

    [IO.File]::WriteAllText("$Jl\src\a.rs", "fn a() {} // touched$nl", $enc)
    $q3 = @(& $bashExe $shJ end -Root (ToPosix $Jl) 2>&1)
    # 路径分隔符带平台语义：Windows 上 rel 是反斜杠（与旧实现一致），断言同时接受两种。
    Assert-True ($LASTEXITCODE -eq 1 -and (@($q3) -join ' ') -match 'modified src[\\/]a\.rs') 'Q3 有改动时逐项列出并 exit 1' (@($q3) -join ' / ')

    $q4 = @(& $bashExe $shJ restore -Root (ToPosix $Jl) 2>&1)
    $after = [IO.File]::ReadAllText("$Jl\src\a.rs", $enc)
    Assert-True ($LASTEXITCODE -eq 0 -and $after -eq "fn a() {}$nl") 'Q4 还原内容与快照一致' ($after.Trim())

    # 还原必须刷新 mtime：Copy-Item/copy2 会保留旧时间，cargo 于是认为文件没改过，
    # 树对了、构建还是错的，这种"看起来成功"最贵。
    $age = ((Get-Date) - (Get-Item "$Jl\src\a.rs").LastWriteTime).TotalSeconds
    Assert-True ($age -lt 120) 'Q5 还原后 mtime 是现在（不是快照里的旧时间）' ("age=${age}s")

    $q6 = @(& $bashExe $shJ status -Root (ToPosix $Jl) 2>&1)
    Assert-True ((@($q6) -join ' ') -match 'snapshot of 2 files at \d{4}-\d\d-\d\dT[0-9:.]+') 'Q6 status 打印带偏移的 ISO 时间（不随区域设置变）' (@($q6) -join ' / ')

    & $bashExe $shJ restore -Root (ToPosix $Jl) 2>&1 | Out-Null
    Assert-True ($LASTEXITCODE -eq 0) 'Q7 二次还原是幂等的（已一致就不再复制）' "exit=$LASTEXITCODE"
}

""
"== R. policy 的 bash 入口：两轴判定与根边界 =="
if (-not $bashExe) {
    "  SKIP  R0 没有 bash，跳过 policy 入口检查"
    $skipped += 8
} else {
    $shP = Join-Path $tools 'policy.sh'
    $Rp = Join-Path $Work 'r-pol'
    New-Item -ItemType Directory $Rp -Force | Out-Null

    & $bashExe $shP -Profile read-only -Root (ToPosix $Rp) -Command 'curl https://example.com' 2>&1 | Out-Null
    $r1 = $LASTEXITCODE
    & $bashExe $shP -Profile read-only -Root (ToPosix $Rp) -Command 'curl https://example.com' -AllowNetwork 2>&1 | Out-Null
    $r1b = $LASTEXITCODE
    Assert-True ($r1 -eq 1 -and $r1b -eq 0) 'R1 网络默认关，-AllowNetwork 才放行' "off=$r1 on=$r1b"

    & $bashExe $shP -Profile full -Root (ToPosix $Rp) -Command 'rm -rf /' 2>&1 | Out-Null
    Assert-True ($LASTEXITCODE -eq 1) 'R2 破坏性命令即使 full 也拒' "exit=$LASTEXITCODE"

    & $bashExe $shP -Profile read-only -Root (ToPosix $Rp) -Command 'cargo test && curl https://x.io' 2>&1 | Out-Null
    Assert-True ($LASTEXITCODE -eq 1) 'R3 分段求值，&& 后面的网络调用藏不住' "exit=$LASTEXITCODE"

    & $bashExe $shP -Profile read-only -Root (ToPosix $Rp) -Command 'cargo test' 2>&1 | Out-Null
    Assert-True ($LASTEXITCODE -eq 0) 'R4 read-only 仍允许跑测试（观察项目的方式）' "exit=$LASTEXITCODE"

    # 根边界必须是路径组件比较，不能是字符串前缀：
    # r-pol 的兄弟目录 r-pol-evil 若被判成"根内"，等于边界形同虚设。
    $evil = "$Rp-evil"
    & $bashExe $shP -Profile workspace-write -Root (ToPosix $Rp) -Command "cp a.txt $(ToPosix $evil)\b.txt" 2>&1 | Out-Null
    Assert-True ($LASTEXITCODE -eq 1) 'R5 兄弟目录 r-pol-evil 不算根内（组件级边界）' "exit=$LASTEXITCODE"

    & $bashExe $shP -Profile workspace-write -Root (ToPosix $Rp) -Command 'mkdir sub' 2>&1 | Out-Null
    Assert-True ($LASTEXITCODE -eq 0) 'R6 根内写入放行' "exit=$LASTEXITCODE"

    & $bashExe $shP -Profile read-only -Root (ToPosix $Rp) -Command ' ' 2>&1 | Out-Null
    Assert-True ($LASTEXITCODE -eq 2) 'R7 空命令 exit 2（无法判定不等于放行）' "exit=$LASTEXITCODE"

    $r8 = @(& $bashExe $shP -Profile read-only -Root (ToPosix $Rp) -Command 'cargo test' -Explain 2>&1)
    Assert-True ((@($r8) -join ' ') -match '不是内核沙箱') 'R8 -Explain 明说是分类器不是沙箱' (@($r8) -join ' / ')
}

""
"== S. 错误台账：不许悄悄变薄、不许声称固化却没断言 =="
$ledger = Join-Path $repo 'docs\ERRORS.md'
if (-not (Test-Path $ledger)) {
    "  FAIL  S0 台账不存在 docs/ERRORS.md"
    $failed++
} else {
    $led = [IO.File]::ReadAllText($ledger, $enc)

    $sections = @('## A. ', '## B. ', '## C. ', '## D. ')
    $missing = @($sections | Where-Object { $led -notmatch [regex]::Escape($_) })
    Assert-True ($missing.Count -eq 0) 'S1 平台/语义/流程/欠账 四节齐全' ("missing=" + ($missing -join ','))

    # 条数下限：台账被删行是最可能的"腐化"方式，且删完没人会发现
    $rows = @([regex]::Matches($led, '(?m)^\| L\d\d \|'))
    Assert-True ($rows.Count -ge 40) 'S2 台账条目数不低于 40 行（防悄悄变薄）' ("rows=$($rows.Count)")

    # 台账里写的"固化为 P6 / R5 / Q5"必须在 selftest 里真实存在，否则就是声称已验收。
    # 注意：断言名是源码里的字符串字面量（运行时才变成 PASS 行），所以查源码里的 id 记号。
    $claimed = @([regex]::Matches($led, '(?<![A-Za-z0-9])([A-R]\d{1,2})(?![0-9])') |
        ForEach-Object { $_.Groups[1].Value } |
        Where-Object { $_ -notmatch '^L' } | Sort-Object -Unique)
    $selfText = [IO.File]::ReadAllText($PSCommandPath, $enc)
    $ghost = @($claimed | Where-Object { $selfText -notmatch ('(?<![A-Za-z0-9])' + $_ + '(?![0-9])') })
    Assert-True ($ghost.Count -eq 0) 'S3 台账声称的断言 id 在自测里真实存在（无幽灵引用）' ("claimed=" + ($claimed -join ',') + " ghost=" + ($ghost -join ','))

    $owed = @([regex]::Matches($led, '(?m)^\| L\d\d \|'))
    Assert-True ($led -match '## D\.' -and $led -match '欠账') 'S4 未固化的欠账被单列，不混进已解决里' 'ok'
}

""
"== T. agent-verify 编排层：顺序不可变、拒绝在 run 之前、回执纯 ASCII =="
if (-not $bashExe) {
    "  SKIP  T0 没有 bash，跳过编排层检查"
    $skipped += 9
} else {
    $shAV = Join-Path $tools 'agent-verify.sh'
    $T = Join-Path $Work 't-av'
    New-Item -ItemType Directory "$T\crates\x\src" -Force | Out-Null
    [IO.File]::WriteAllText("$T\crates\x\src\a.rs", "pub fn a() {}$nl", $enc)
    & $bashExe (Join-Path $tools 'memory.sh') -Mode init -Root (ToPosix $T) 2>&1 | Out-Null
    & $bashExe (Join-Path $tools 'timeline.sh') -Mode init -Root (ToPosix $T) 2>&1 | Out-Null
    & $bashExe (Join-Path $tools 'constraints.sh') -Mode init -Root (ToPosix $T) 2>&1 | Out-Null
    Push-Location $T
    & git init -q 2>&1 | Out-Null
    & git add -A 2>&1 | Out-Null
    & git -c user.email=a@b -c user.name=t commit -qm init 2>&1 | Out-Null
    Pop-Location

    # 注入一个必然成功的 plan：不依赖真实构建，但 run / journal-end 两步必须真的发生
    $planT = Join-Path $T 'plan.json'
    [IO.File]::WriteAllText($planT, '{"proven":true,"commands":["echo hello > out.txt"],"unproven":[]}', $enc)
    $rcT = Join-Path $T 'receipt.json'
    $t1 = @(& $bashExe $shAV -Root (ToPosix $T) -Changed 'crates/x/src/a.rs' -Lang rust -Plan (ToPosix $planT) -Receipt (ToPosix $rcT) 2>&1)
    $t1Exit = $LASTEXITCODE
    Assert-True ($t1Exit -eq 0 -and (Test-Path "$T\out.txt")) 'T1 允许路径跑完并真的写盘' "exit=$t1Exit"

    $doc = [IO.File]::ReadAllText($rcT, $enc) | ConvertFrom-Json
    $order = @($doc.steps | ForEach-Object { $_.step })
    $asciiOk = ([IO.File]::ReadAllText($rcT, $enc) -notmatch '[^\x00-\x7F]')
    Assert-True ($asciiOk) 'T2 回执是纯 ASCII（5.1 那样的读者不会读坏）' ("len=$($order.Count)")

    $iMem = [array]::IndexOf($order, 'memory-brief')
    $iJb = [array]::IndexOf($order, 'journal-begin')
    $iRun = [array]::IndexOf($order, 'run')
    $iJe = [array]::IndexOf($order, 'journal-end')
    $iPre = [array]::IndexOf($order, 'preflight')
    $iPol = [array]::IndexOf($order, 'policy')
    Assert-True ($iMem -eq 0 -and $iPre -gt $iMem -and $iPol -gt $iPre -and $iJb -gt $iPol -and $iRun -gt $iJb -and $iJe -gt $iRun) 'T3 步骤顺序固定：memory→preflight→policy→journal-begin→run→journal-end' ($order -join ' > ')

    $jeStep = @($doc.steps | Where-Object { $_.step -eq 'journal-end' })[0]
    Assert-True ($jeStep.verdict -eq 'changed') 'T4 journal-end 看得见 run 写的文件（不是 clean）' $jeStep.verdict

    $mn = @($doc.steps | Where-Object { $_.step -eq 'memory-note' })
    Assert-True ($mn.Count -ge 1 -and $mn[0].verdict -eq 'ok') 'T5 运行结果被写成一条可复核的记忆' ("notes=$($mn.Count)")

    # 拒绝路径：约束表命中必须停在 constraints，run 不能出现
    [IO.File]::WriteAllText("$T\crates\x\src\a.rs", "pub fn a() {}$nl pub fn b() { println!(`"hi`"); }$nl", $enc)
    $rcT2 = Join-Path $T 'receipt2.json'
    $t6 = @(& $bashExe $shAV -Root (ToPosix $T) -Changed 'crates/x/src/a.rs' -Lang rust -Receipt (ToPosix $rcT2) 2>&1)
    $t6Exit = $LASTEXITCODE
    $doc2 = [IO.File]::ReadAllText($rcT2, $enc) | ConvertFrom-Json
    $order2 = @($doc2.steps | ForEach-Object { $_.step })
    Assert-True ($t6Exit -eq 1 -and $doc2.verdict -eq 'refused by constraints' -and ($order2 -notcontains 'run')) 'T6 拒绝时中止在 run 之前，回执照写' "exit=$t6Exit verdict=$($doc2.verdict) steps=$($order2 -join ',')"

    $t7 = @(& $bashExe $shAV -Root (ToPosix $T) -Lang rust 2>&1)
    Assert-True ($LASTEXITCODE -eq 2) 'T7 缺 -Changed 直接 exit 2（参数不全不算通过）' "exit=$LASTEXITCODE"

    [IO.File]::WriteAllText("$T\crates\x\src\a.rs", "pub fn a() {}$nl", $enc)
    $t8 = @(& $bashExe $shAV -Root (ToPosix $T) -Changed 'a.rs' -Lang rust -DryRun -Receipt (ToPosix (Join-Path $T 'rc3.json')) 2>&1)
    $doc3 = [IO.File]::ReadAllText((Join-Path $T 'rc3.json'), $enc) | ConvertFrom-Json
    Assert-True ($LASTEXITCODE -eq 0 -and $doc3.verdict -eq 'dry-run' -and (@($doc3.steps) -notcontains 'run')) 'T8 dry-run 不写盘也不跑（journal/run 都不出现）' "verdict=$($doc3.verdict)"

    Assert-True (Test-Path (Join-Path (Join-Path $env:LOCALAPPDATA 'agent-verify') 'D__KF__parity_av-new')) 'T9 快照落在仓库外（默认 store 不在被保护的树里）' 'ok'

    # 这条锁住一个真实事故：shutil.which("bash") 在 Windows 上拿到的是 WSL 的 bash，
    # 它看到的是另一套文件系统（D:\ 是 /mnt/d），于是"命令找不到"被当成项目失败。
    # 判据不是"哪个 bash"，而是"这个 shell 能不能看见我正要它跑的那个文件"。
    $shRun = Join-Path $tools '_run.sh'
    $t10 = @(& $bashExe $shRun -Root (ToPosix 'D:\KF\trit-inv') -Command 'test -f tools/agent_verify.py && echo SAME-FS' 2>&1)
    Assert-True ($LASTEXITCODE -eq 0 -and (@($t10) -join ' ') -match 'SAME-FS') 'T10 执行面看到的文件系统与工具一致（不是 WSL 那套）' (@($t10) -join ' / ')
}

""
"== U. ledger：token 与墙钟分列、同 key 只能记一次、中位数是数值序 =="
if (-not $bashExe) {
    "  SKIP  U0 没有 bash，跳过 ledger 检查"
    $skipped += 5
} else {
    $shL = Join-Path $tools 'ledger.sh'
    $led = Join-Path $Work 'u-ledger.json'
    & $bashExe $shL add -Ledger (ToPosix $led) -Task split -Key 'a|sha' -Tokens 18400 -Wall 12.6 2>&1 | Out-Null
    & $bashExe $shL add -Ledger (ToPosix $led) -Task split -Key 'b|sha' -Tokens 7200 2>&1 | Out-Null
    & $bashExe $shL add -Ledger (ToPosix $led) -Task verify -Key 'c|run' -Tokens 0 -Wall 1 2>&1 | Out-Null
    $u1 = @(& $bashExe $shL add -Ledger (ToPosix $led) -Task split -Key 'a|sha' -Tokens 99999 2>&1)
    $entries = @(([IO.File]::ReadAllText($led, $enc) | ConvertFrom-Json).entries)
    Assert-True ($entries.Count -eq 3 -and (@($u1) -join ' ') -match 'already recorded') 'U1 同 key 重放只留一条（并说明丢弃了这次的）' "entries=$($entries.Count)"

    $u2 = @(& $bashExe $shL report -Ledger (ToPosix $led) 2>&1)
    Assert-True ((@($u2) -join ' ') -match 'total tokens: 25600.*total wall:\s+13.6') 'U2 report 分列 token 与墙钟，不合并成一个成本' (@($u2) -join ' / ')

    # 旧实现按字典序排（"0" < "18400" < "7200"），中位数会取到 18400；数值中位数应是 7200
    Assert-True ((@($u2) -join ' ') -match 'median per entry: 7200') 'U3 中位数按数值序（不是字符串序）' (@($u2) -join ' / ')

    & $bashExe $shL add -Ledger (ToPosix $led) -Task t 2>&1 | Out-Null
    Assert-True ($LASTEXITCODE -eq 2) 'U4 缺 -Key 拒绝（没有键就无法防重复计数）' "exit=$LASTEXITCODE"

    & $bashExe $shL add -Ledger (ToPosix $led) -Task t -Key k5 2>&1 | Out-Null
    Assert-True ($LASTEXITCODE -eq 2) 'U5 什么都没记也拒绝（空条目会污染中位数）' "exit=$LASTEXITCODE"
}

""
"== V. refactor-guard preflight：没被 mod 声明的测试文件必须判红 =="
if (-not $bashExe) {
    "  SKIP  V0 没有 bash，跳过 preflight 检查"
    $skipped += 3
} else {
    $shG = Join-Path $tools 'refactor-guard.sh'
    $V = Join-Path $Work 'v-guard'
    New-Item -ItemType Directory "$V\crates\trit-orch\tests" -Force | Out-Null
    [IO.File]::WriteAllText("$V\crates\trit-orch\tests\main.rs", "mod suite_a;$nl", $enc)
    [IO.File]::WriteAllText("$V\crates\trit-orch\tests\suite_a.rs", "fn ok() {}$nl", $enc)
    & $bashExe $shG preflight -Repo (ToPosix $V) 2>&1 | Out-Null
    Assert-True ($LASTEXITCODE -eq 0) 'V1 声明齐全时 preflight 干净' "exit=$LASTEXITCODE"

    # 这条是整个 preflight 存在的理由：文件在盘上，但它从未被编译过，套件看起来还是绿的
    [IO.File]::WriteAllText("$V\crates\trit-orch\tests\suite_orphan.rs", "fn ghost() {}$nl", $enc)
    $v2 = @(& $bashExe $shG preflight -Repo (ToPosix $V) 2>&1)
    Assert-True ($LASTEXITCODE -eq 1 -and (@($v2) -join ' ') -match 'unreachable test file') 'V2 没被 mod 声明的测试文件判红（exit 1）' (@($v2) -join ' / ')

    $V2 = Join-Path $Work 'v-notests'
    New-Item -ItemType Directory $V2 -Force | Out-Null
    & $bashExe $shG preflight -Repo (ToPosix $V2) 2>&1 | Out-Null
    Assert-True ($LASTEXITCODE -eq 0) 'V3 没有 tests 目录时放行（不是所有项目都有）' "exit=$LASTEXITCODE"
}

"== Y. resume: 断粮挂起（censored 不是失败）与换会话续跑 =="
if (-not $bashExe) {
    "  SKIP  Y0 没有 bash，跳过 resume 检查"
    $skipped += 21
} else {
    $shRes = Join-Path $tools 'resume.sh'
    $Yr = Join-Path $Work 'y-resume'
    New-Item -ItemType Directory $Yr -Force | Out-Null
    $yroot = ToPosix $Yr

    & $bashExe $shRes begin t1 --budget-token 12000 -Root $yroot 2>&1 | Out-Null
    Assert-True ($LASTEXITCODE -eq 0) 'Y1 begin exit=0' "exit=$LASTEXITCODE"
    $ys = Get-Content (Join-Path $Yr '.agent\resume\t1.json') -Raw | ConvertFrom-Json
    Assert-True ($ys.task -eq 't1' -and $ys.status -eq 'open' -and $ys.budget -eq 12000 `
        -and $ys.created -and $ys.journal_id -eq $null) 'Y2 会话文件字段完整（task/created/status/budget/steps）' ''
    Assert-True (@($ys.steps).Count -eq 0) 'Y2b steps 从空开始' ''

    & $bashExe $shRes begin t1 -Root $yroot 2>&1 | Out-Null
    Assert-True ($LASTEXITCODE -eq 2) 'Y3 已存在的会话再 begin 被拒（fail-closed，不许覆盖）' "exit=$LASTEXITCODE"

    & $bashExe $shRes progress t1 s1 --proven --note 'first step' -Root $yroot 2>&1 | Out-Null
    Assert-True ($LASTEXITCODE -eq 0) 'Y4 progress exit=0' "exit=$LASTEXITCODE"
    $ys = Get-Content (Join-Path $Yr '.agent\resume\t1.json') -Raw | ConvertFrom-Json
    Assert-True ($ys.steps[0].step -eq 's1' -and $ys.steps[0].proven -eq $true `
        -and $ys.steps[0].note -eq 'first step') 'Y4b 步骤记录完整（step/proven/note）' ''

    # journal 联动：只记快照 id（manifest 内容哈希），不复制内容
    New-Item -ItemType Directory -Force (Join-Path $Yr '.journal\latest') | Out-Null
    [IO.File]::WriteAllText((Join-Path $Yr '.journal\latest\manifest.json'), '{"count": 3}', $enc)
    $wantId = (Get-FileHash (Join-Path $Yr '.journal\latest\manifest.json') -Algorithm SHA256).Hash.ToLower().Substring(0, 12)
    & $bashExe $shRes suspend t1 --reason budget -Root $yroot 2>&1 | Out-Null
    Assert-True ($LASTEXITCODE -eq 0) 'Y5 suspend exit=0' "exit=$LASTEXITCODE"
    $ys = Get-Content (Join-Path $Yr '.agent\resume\t1.json') -Raw | ConvertFrom-Json
    Assert-True ($ys.status -eq 'suspended' -and $ys.censor.reason -eq 'budget' -and $ys.censor.at) `
        'Y5b suspend 记录挂起状态与删失原因' ''
    Assert-True ($ys.journal_id -eq $wantId) 'Y6 suspend 时探测到 journal 快照 id（只记 id）' "want=$wantId got=$($ys.journal_id)"

    $y7 = @(& $bashExe $shRes status t1 -Root $yroot 2>&1)
    $y7txt = (@($y7) -join ' ')
    Assert-True ($y7txt -match 'censored' -and $y7txt -match 'not a failure') `
        'Y7 挂起态的措辞是 censored（不是 failed）' $y7txt

    $y8 = @(& $bashExe $shRes resume t1 -Root $yroot 2>&1)
    $y8txt = (@($y8) -join "`n")
    Assert-True ($y8txt -match 'copy verbatim' -and $y8txt -match 'resume\.py progress t1 ' `
        -and $y8txt -match 'resume\.py finish t1') 'Y8 resume 输出逐字可复制的续跑命令块' $y8txt
    Assert-True (@($y8 | Where-Object { $_ -match '[^\x00-\x7F]' }).Count -eq 0) `
        'Y8b 续跑块与状态输出纯 ASCII（任何宿主都不读坏）' ''
    $ys = Get-Content (Join-Path $Yr '.agent\resume\t1.json') -Raw | ConvertFrom-Json
    Assert-True ($ys.status -eq 'suspended') 'Y9 resume 是只读视图（看一眼不改变挂起状态）' ''

    & $bashExe $shRes progress t1 s2 --proven -Root $yroot 2>&1 | Out-Null
    $ys = Get-Content (Join-Path $Yr '.agent\resume\t1.json') -Raw | ConvertFrom-Json
    Assert-True ($ys.status -eq 'open' -and ($ys.history | Where-Object { $_.event -eq 'resume' }).Count -ge 1) `
        'Y10 挂起的会话被 progress 接回 open（历史里记了 resume 事件）' ''

    & $bashExe $shRes finish t1 -Root $yroot 2>&1 | Out-Null
    Assert-True ($LASTEXITCODE -eq 0) 'Y11 finish exit=0' "exit=$LASTEXITCODE"
    $ys = Get-Content (Join-Path $Yr '.agent\resume\t1.json') -Raw | ConvertFrom-Json
    Assert-True ($ys.status -eq 'finished') 'Y11b 状态 finished' ''
    & $bashExe $shRes finish t1 -Root $yroot 2>&1 | Out-Null
    Assert-True ($LASTEXITCODE -eq 2) 'Y12 已完结的会话再 finish 被拒' "exit=$LASTEXITCODE"

    # censored 的完整闭环：挂起后直接收尾 = 删失结案，不是失败
    & $bashExe $shRes begin t2 -Root $yroot 2>&1 | Out-Null
    & $bashExe $shRes suspend t2 --reason deadline -Root $yroot 2>&1 | Out-Null
    $y13 = @(& $bashExe $shRes finish t2 -Root $yroot 2>&1)
    $ys2 = Get-Content (Join-Path $Yr '.agent\resume\t2.json') -Raw | ConvertFrom-Json
    Assert-True ($ys2.status -eq 'censored') 'Y13 挂起后 finish 判 censored（不是 finished）' ''
    Assert-True ((@($y13) -join ' ') -match 'censored' -and (@($y13) -join ' ') -match 'not a failure') `
        'Y13b censored 的措辞明说不是失败' (@($y13) -join ' ')

    $y14 = @(& $bashExe $shRes list -Root $yroot 2>&1)
    Assert-True ((@($y14 | Where-Object { $_ -match '\bt1\b' }).Count -ge 1 `
        -and @($y14 | Where-Object { $_ -match '\bt2\b' }).Count -ge 1) `
        -and (@($y14 | Where-Object { $_ -match 'censored' }).Count -ge 1)) `
        'Y14 list 列出会话与状态（censored 也在列）' (@($y14) -join ' / ')

    & $bashExe $shRes suspend ghost-task --reason budget -Root $yroot 2>&1 | Out-Null
    Assert-True ($LASTEXITCODE -eq 2) 'Y15 不存在的会话 suspend 被拒（fail-closed）' "exit=$LASTEXITCODE"
    & $bashExe $shRes begin '../evil' -Root $yroot 2>&1 | Out-Null
    Assert-True ($LASTEXITCODE -eq 2) 'Y16 非法 task-id（路径形态）被拒' "exit=$LASTEXITCODE"

    # 编排层集成：声明额度 0 时 run 之前整体拒绝，verdict 是 suspended 不是 failed
    $shAVy = Join-Path $tools 'agent-verify.sh'
    $Yav = Join-Path $Work 'y-av'
    New-File (Join-Path $Yav 'conftest.py') ''
    New-File (Join-Path $Yav 'pkg\core.py') "def f(): return 1`n"
    New-File (Join-Path $Yav 'pkg\__init__.py') "from .core import f`n"
    New-File (Join-Path $Yav 'tests\test_core.py') "from pkg.core import f`ndef test_f(): assert f() == 1`n"
    $yrc = Join-Path $Work 'y-gate-rc.json'
    & $bashExe $shAVy -Root (ToPosix $Yav) -Changed 'pkg/core.py' -Lang python `
        --budget-token 0 --task-id t-budget -Receipt (ToPosix $yrc) 2>&1 | Out-Null
    $y17 = $LASTEXITCODE
    $yDoc = Get-Content $yrc -Raw | ConvertFrom-Json
    $yNames = @($yDoc.steps | ForEach-Object { $_.step })
    Assert-True ($y17 -eq 1 -and $yDoc.verdict -eq 'suspended (budget) before run') `
        'Y17 --budget-token 0 在 run 前整体拒绝，verdict 正确' "exit=$y17 verdict=$($yDoc.verdict)"
    Assert-True ($yNames -notcontains 'run' -and $yNames -notcontains 'affected') `
        'Y17b 拒绝发生在 run 之前（连受影响分析都没跑）' ($yNames -join ',')
    Assert-True ($yDoc.budget.declared -eq 0 -and $yDoc.budget.gate -match 'cannot measure model-side token usage') `
        'Y18 回执带着声明制闸门的诚实边界说明' ''
    $yRaw = [IO.File]::ReadAllText($yrc, (New-Object System.Text.UTF8Encoding($false)))
    Assert-True ((($yRaw -replace '[^\x00-\x7F]', '').Length) -eq $yRaw.Length) 'Y18b 回执纯 ASCII' ''
    $ySession = Get-Content (Join-Path $Yav '.agent\resume\t-budget.json') -Raw | ConvertFrom-Json
    Assert-True ($ySession.status -eq 'suspended' -and $ySession.censor.reason -eq 'budget' `
        -and $ySession.budget -eq 0) 'Y19 闸门自动挂起会话（reason=budget，budget=0）' ''

    # 失败自动挂起：verdict 保持 command failed，只允许追加字段
    $Yf = Join-Path $Work 'y-avfail'
    New-File (Join-Path $Yf 'conftest.py') ''
    New-File (Join-Path $Yf 'pkg\core.py') "def f(): return 1`n"
    New-File (Join-Path $Yf 'tests\test_core.py') "from pkg.core import f`ndef test_fail(): assert f() == 2`n"
    $yrc2 = Join-Path $Work 'y-fail-rc.json'
    & $bashExe $shAVy -Root (ToPosix $Yf) -Changed 'pkg/core.py' -Lang python `
        --task-id t-fail -Receipt (ToPosix $yrc2) 2>&1 | Out-Null
    $y20 = $LASTEXITCODE
    $yDoc2 = Get-Content $yrc2 -Raw | ConvertFrom-Json
    Assert-True ($y20 -eq 1 -and $yDoc2.verdict -eq 'command failed') `
        'Y20 失败自动挂起但 verdict 仍是 command failed' "exit=$y20 verdict=$($yDoc2.verdict)"
    Assert-True ($yDoc2.resume_checkpoint -match 't-fail suspended \(manual\)' `
        -and $yDoc2.resume.suspend -eq 0 `
        -and @($yDoc2.steps | ForEach-Object { $_.step }) -contains 'resume-suspend') `
        'Y20b 回执尾部追加了 resume checkpoint（旧字段一个没动）' ''
    Assert-True ($yDoc2.run[0].exit -ne 0 `
        -and (@($yDoc2.steps | ForEach-Object { $_.step }) -contains 'journal-end') `
        -and $yDoc2.plan.proven -ne $null) 'Y20c run/plan/链路原有字段语义不变' ''
    $yFailSession = Get-Content (Join-Path $Yf '.agent\resume\t-fail.json') -Raw | ConvertFrom-Json
    Assert-True ($yFailSession.status -eq 'suspended' -and $yFailSession.censor.reason -eq 'manual' `
        -and $yFailSession.steps[0].proven -eq $false -and $yFailSession.steps[0].step -eq 'run:1') `
        'Y20d 失败步骤记为 unproven 并挂起（reason=manual）' ''

    # 声明额度为正时闸门不拦（声明制：>0 就放行去跑）
    $yrc3 = Join-Path $Work 'y-ok-rc.json'
    & $bashExe $shAVy -Root (ToPosix $Yav) -Changed 'pkg/core.py' -Lang python `
        --budget-token 5 -Receipt (ToPosix $yrc3) 2>&1 | Out-Null
    $yDoc3 = Get-Content $yrc3 -Raw | ConvertFrom-Json
    Assert-True ($LASTEXITCODE -eq 0 -and $yDoc3.verdict -match '^passed') `
        'Y21 声明额度为正时正常跑完（闸门只拦 <= 0）' "exit=$LASTEXITCODE verdict=$($yDoc3.verdict)"
}

""
"== Z. run-caps: 进程资源硬帽（内核帽，不是安全边界）=="
if (-not $bashExe) {
    "  SKIP  Z0 没有 bash，跳过 run-caps 检查"
    $skipped += 12
} else {
    $zPy = $null
    foreach ($c in @($env:PYTHON, 'python', 'python3', 'py')) {
        if (-not $c) { continue }
        $src = (Get-Command $c -ErrorAction SilentlyContinue).Source
        if (-not $src) { continue }
        if ((& $src -c 'print(1)' 2>$null) -eq '1') { $zPy = $src; break }
    }
    if (-not $zPy) {
        "  SKIP  Z0b 找不到可用的 Python，跳过 run-caps 检查"
        $skipped += 12
    } else {
        $shCaps = Join-Path $tools 'run-caps.sh'
        $Zd = Join-Path $Work 'z-caps'
        New-Item -ItemType Directory $Zd -Force | Out-Null
        $znl = [char]10
        [IO.File]::WriteAllText("$Zd\ok.py", "print('caps-ok')$znl", $enc)
        [IO.File]::WriteAllText("$Zd\exit3.py", "import sys; sys.exit(3)$znl", $enc)
        [IO.File]::WriteAllText("$Zd\sleep10.py", "import time; time.sleep(10)$znl", $enc)
        [IO.File]::WriteAllText("$Zd\bigalloc.py", "x = [0] * (1 << 28)$znlprint(len(x))$znl", $enc)
        [IO.File]::WriteAllText("$Zd\spawn3.py", (@(
                'import subprocess, sys',
                'procs = []',
                'try:',
                '    for _ in range(3):',
                '        procs.append(subprocess.Popen([sys.executable, "-c", "import time; time.sleep(2)"]))',
                'except OSError as e:',
                '    print("spawn denied")',
                '    sys.exit(9)',
                'for p in procs:',
                '    p.wait()',
                'print("spawned", len(procs))'
            ) -join $znl) + $znl, $enc)

        $z1 = @(& $bashExe $shCaps --mem-mb 256 --procs 8 --seconds 30 -- $zPy "$Zd\ok.py" 2>&1)
        Assert-True ($LASTEXITCODE -eq 0 -and (@($z1) -join ' ') -match 'caps-ok') `
            'Z1 正常命令透传退出码 0 且输出可见' "exit=$LASTEXITCODE out=(@($z1) -join ' ')"

        & $bashExe $shCaps --mem-mb 256 --procs 8 --seconds 30 -- $zPy "$Zd\exit3.py" 2>&1 | Out-Null
        Assert-True ($LASTEXITCODE -eq 3) 'Z2 子进程退出码原样透传（3）' "exit=$LASTEXITCODE"

        & $bashExe $shCaps --seconds 2 -- $zPy "$Zd\sleep10.py" 2>&1 | Out-Null
        Assert-True ($LASTEXITCODE -eq 124) 'Z3 超时映射为 124（watchdog 杀掉整个 job）' "exit=$LASTEXITCODE"

        & $bashExe $shCaps --mem-mb 256 --seconds 30 -- $zPy "$Zd\bigalloc.py" 2>&1 | Out-Null
        Assert-True ($LASTEXITCODE -ne 0 -and $LASTEXITCODE -ne 124) `
            'Z4 内存帽：超大分配在帽下失败（非 0 退出，不是超时）' "exit=$LASTEXITCODE"

        & $bashExe $shCaps --procs 2 --seconds 60 -- $zPy "$Zd\spawn3.py" 2>&1 | Out-Null
        Assert-True ($LASTEXITCODE -eq 9) 'Z5 进程数帽：第 3 个进程创建被内核拒绝（exit 9）' "exit=$LASTEXITCODE"

        $z6 = @(& $bashExe $shCaps --procs 8 --seconds 60 -- $zPy "$Zd\spawn3.py" 2>&1)
        Assert-True ($LASTEXITCODE -eq 0 -and (@($z6) -join ' ') -match 'spawned 3') `
            'Z6 帽内进程数正常跑完' "exit=$LASTEXITCODE"

        & $bashExe $shCaps -- $zPy "$Zd\ok.py" 2>&1 | Out-Null
        Assert-True ($LASTEXITCODE -eq 2) 'Z7 一个帽都不给就拒绝（不加帽静默跑完等于说谎）' "exit=$LASTEXITCODE"
        & $bashExe $shCaps --mem-mb 256 "$Zd\ok.py" 2>&1 | Out-Null
        Assert-True ($LASTEXITCODE -eq 2) 'Z7b 缺 -- 分隔符拒绝（帽与命令不许含糊）' "exit=$LASTEXITCODE"
        & $bashExe $shCaps --mem-mb 0 -- $zPy "$Zd\ok.py" 2>&1 | Out-Null
        Assert-True ($LASTEXITCODE -eq 2) 'Z7c 非法帽值（0）拒绝' "exit=$LASTEXITCODE"

        & $bashExe $shCaps --mem-mb 256 -- no-such-exe-xyz-probe 2>&1 | Out-Null
        Assert-True ($LASTEXITCODE -eq 127) 'Z8 起不来的命令报 127（不是吞掉）' "exit=$LASTEXITCODE"

        # 编排层集成：--caps 时回执 run[] 追加 caps 参数，verdict 与退出码照旧
        $shAVz = Join-Path $tools 'agent-verify.sh'
        $Zav = Join-Path $Work 'z-av'
        New-File (Join-Path $Zav '.git\HEAD') "ref: refs/heads/main`n"
        New-File (Join-Path $Zav 'conftest.py') ''
        New-File (Join-Path $Zav 'pkg\core.py') "def f(): return 1`n"
        New-File (Join-Path $Zav 'pkg\__init__.py') "from .core import f`n"
        New-File (Join-Path $Zav 'tests\test_core.py') "from pkg.core import f`ndef test_f(): assert f() == 1`n"
        $zrc = Join-Path $Work 'z-caps-rc.json'
        & $bashExe $shAVz -Root (ToPosix $Zav) -Changed 'pkg/core.py' -Lang python `
            --caps --caps-seconds 120 -Receipt (ToPosix $zrc) 2>&1 | Out-Null
        $z9 = $LASTEXITCODE
        $zDoc = Get-Content $zrc -Raw | ConvertFrom-Json
        Assert-True ($z9 -eq 0 -and $zDoc.verdict -match '^passed' `
            -and $zDoc.run[0].caps.seconds -eq 120 -and $zDoc.run[0].caps.mem_mb -gt 0) `
            'Z9 --caps 时 run[] 追加 caps 参数且回执 verdict 照旧' "exit=$z9 verdict=$($zDoc.verdict)"
        Assert-True ($zDoc.caps.note -match 'NOT a security boundary') `
            'Z9b 回执带着"内核帽不是安全边界"的诚实强度声明' ''

        $zrc2 = Join-Path $Work 'z-plain-rc.json'
        & $bashExe $shAVz -Root (ToPosix $Zav) -Changed 'pkg/core.py' -Lang python `
            -Receipt (ToPosix $zrc2) 2>&1 | Out-Null
        $zDoc2 = Get-Content $zrc2 -Raw | ConvertFrom-Json
        Assert-True ($LASTEXITCODE -eq 0 `
            -and ($zDoc2.run[0].PSObject.Properties.Name -notcontains 'caps') `
            -and $zDoc2.PSObject.Properties.Name -notcontains 'caps') `
            'Z10 不开 --caps 时回执与现状完全一致（回归保护）' ''
    }
}

"selftest: passed=$passed failed=$failed skipped=$skipped  (work dir: $Work)"
if ($failed -gt 0) { exit 1 }
exit 0
