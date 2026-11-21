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
# 控制台编码闸：中文 Windows 的 OEM 页是 936(GBK)，管道里的中文断言靠运气——
# 子进程按 locale 写、PowerShell 按 [Console]::OutputEncoding 读，两边不一致就 mojibake 伪红。
# 双侧钉死 UTF-8：本进程的读和写，加上 Python 子进程的写（PYTHONUTF8/PYTHONIOENCODING）。
[Console]::OutputEncoding = [System.Text.UTF8Encoding]::new($false)
$OutputEncoding = [System.Text.UTF8Encoding]::new($false)
$env:PYTHONUTF8 = '1'
$env:PYTHONIOENCODING = 'utf-8'
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

# 跑完落盘：汇总行 + UTC 时间戳写进 .agent/last-selftest.json（gitignore，不进公开树）。
# doctor.py 的 selftest 面只读这份记录（doctor 不自己跑 selftest）；AI 组用同一函数做
# 落盘往返，保证写与读同源。汇总串刻意不含 work 目录——本机路径不留痕（L93/L107 同族纪律）。
function Write-SelftestRecord([string]$Root, [int]$Passed, [int]$Failed, [int]$Skipped) {
    $dir = Join-Path $Root '.agent'
    New-Item -ItemType Directory -Force -Path $dir | Out-Null
    $at = (Get-Date).ToUniversalTime().ToString('yyyy-MM-ddTHH:mm:ssZ')
    $rec = [ordered]@{
        tool = 'selftest'; at = $at; passed = $Passed; failed = $Failed; skipped = $Skipped
        summary = ('selftest: passed={0} failed={1} skipped={2}' -f $Passed, $Failed, $Skipped)
    }
    $line = $rec | ConvertTo-Json -Compress
    [IO.File]::WriteAllText((Join-Path $dir 'last-selftest.json'), ($line + "`n"),
        (New-Object System.Text.UTF8Encoding($false)))
}

# =============================================================================="== A. affected: 没有 git 历史时必须回退全量 =="
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
Assert-True ($planB.PSObject.Properties.Name -notcontains 'targets') 'B5 python 计划不新增 targets 字段（字段只追加，不改旧形态）' (($planB.PSObject.Properties.Name) -join ',')

# =====================================================================================
"== B(续). affected(rust): 减集下沉到 cargo target 级（ERRORS L37 的欠账） =="
# 每个夹具是独立 cargo 工程（[workspace] 兜底）；cargo metadata 只解析清单，不编译源码，
# 所以夹具的 .rs 只需要真实到"能表达 mod 结构"。断言钉死 soundness 行为本身：
# 可达才选 target；cfg/path/macro/include/文件数上限 不可证就回退包级并留下 unproven 理由。
$nl = [char]10
$R1 = Join-Path $Work 'rs-targets'
New-File (Join-Path $R1 '.git\HEAD') "ref: refs/heads/main`n"
New-File (Join-Path $R1 'Cargo.toml') "[package]`nname = 'bx'`nversion = '0.1.0'`nedition = '2021'`n`n[workspace]`n"
New-File (Join-Path $R1 'src\lib.rs') ((@(
    'pub fn f() -> i32 { 1 }', '',
    '#[cfg(feature = "gpu")]',
    'mod gpu;', '',
    '#[cfg(test)]',
    'mod tests;') -join $nl) + $nl)
New-File (Join-Path $R1 'src\gpu.rs') "pub fn g() {}`n"
New-File (Join-Path $R1 'src\tests.rs') "#[test] fn t_lib() { assert_eq!(super::f(), 1); }`n"
New-File (Join-Path $R1 'src\bin\only.rs') "fn main() {}`n"
New-File (Join-Path $R1 'src\bin\other.rs') "fn main() {}`n"
New-File (Join-Path $R1 'tests\t.rs') "#[test] fn t_int() { assert_eq!(bx::f(), 1); }`n"

$outR1 = Join-Path $Work 'plan-r1.json'
& pwsh -NoProfile -File (Join-Path $tools 'affected.ps1') -Path $R1 -Changed (Join-Path $R1 'src\bin\only.rs') -Lang rust -Out $outR1 | Out-Null
$planR1 = Get-Content $outR1 -Raw | ConvertFrom-Json
Assert-True ($planR1.proven -eq $true) 'B6 rust 单 bin 可达→proven 且只跑该 bin' ($planR1.unproven -join ';')
Assert-True ($planR1.commands[0] -match '--bin only' -and $planR1.commands[0] -notmatch '--lib|--bin other|--test') 'B7 单 bin 命令不含 lib/其他 bin/test 选择器' $planR1.commands[0]
Assert-True ($planR1.targets[0].mode -eq 'target-level' -and $planR1.targets[0].selected[0].kind -eq 'bin' -and $planR1.targets[0].selected[0].name -eq 'only' -and $planR1.targets[0].total -eq 4) 'B8 targets 数组形状：mode/selected/total' (($planR1.targets | ConvertTo-Json -Compress))

$outR2 = Join-Path $Work 'plan-r2.json'
& pwsh -NoProfile -File (Join-Path $tools 'affected.ps1') -Path $R1 -Changed (Join-Path $R1 'src\lib.rs') -Lang rust -Out $outR2 | Out-Null
$planR2 = Get-Content $outR2 -Raw | ConvertFrom-Json
Assert-True ($planR2.proven -eq $true -and $planR2.commands[0] -eq 'cargo test -p bx --no-fail-fast') 'B9 lib 变更→包级写法不退化（tests 对 lib 是 extern，全包受影响）' "$($planR2.commands[0]) / $($planR2.unproven -join ';')"
Assert-True ($planR2.targets[0].mode -eq 'package-level' -and $planR2.targets[0].reason -match 'lib') 'B9b targets 数组写明包级原因' (($planR2.targets | ConvertTo-Json -Compress))

$outR3 = Join-Path $Work 'plan-r3.json'
& pwsh -NoProfile -File (Join-Path $tools 'affected.ps1') -Path $R1 -Changed (Join-Path $R1 'src\tests.rs') -Lang rust -Out $outR3 | Out-Null
$planR3 = Get-Content $outR3 -Raw | ConvertFrom-Json
Assert-True ($planR3.proven -eq $true -and $planR3.commands[0] -match '--lib' -and $planR3.commands[0] -notmatch '--bin|--test') 'B10 cfg(test) 单测文件变更→只选 --lib（不进 rlib，集成测试不受影响）' "$($planR3.commands[0]) / $($planR3.unproven -join ';')"

$outR4 = Join-Path $Work 'plan-r4.json'
& pwsh -NoProfile -File (Join-Path $tools 'affected.ps1') -Path $R1 -Changed (Join-Path $R1 'src\gpu.rs') -Lang rust -Out $outR4 | Out-Null
$planR4 = Get-Content $outR4 -Raw | ConvertFrom-Json
Assert-True ($planR4.proven -eq $true -and $planR4.commands[0] -eq 'cargo test -p bx --no-fail-fast') 'B11 cfg(feature) mod 文件按可达计（不冒进剔除，任何 feature 组合下都健全）' "$($planR4.commands[0]) / $($planR4.unproven -join ';')"

$R2 = Join-Path $Work 'rs-cfgmiss'
New-File (Join-Path $R2 '.git\HEAD') "ref: refs/heads/main`n"
New-File (Join-Path $R2 'Cargo.toml') "[package]`nname = 'cx'`nversion = '0.1.0'`nedition = '2021'`n`n[workspace]`n"
New-File (Join-Path $R2 'src\lib.rs') ((@('#[cfg(feature = "nope")]', 'mod ghost;', 'pub fn f() -> i32 { 1 }') -join $nl) + $nl)
New-File (Join-Path $R2 'src\bin\only.rs') "fn main() {}`n"
New-File (Join-Path $R2 'src\bin\other.rs') "fn main() {}`n"
$outR5 = Join-Path $Work 'plan-r5.json'
& pwsh -NoProfile -File (Join-Path $tools 'affected.ps1') -Path $R2 -Changed (Join-Path $R2 'src\bin\only.rs') -Lang rust -Out $outR5 | Out-Null
$planR5 = Get-Content $outR5 -Raw | ConvertFrom-Json
Assert-True ($planR5.proven -eq $true -and $planR5.commands[0] -match '--bin only') 'B12 cfg 关闭时 mod 文件可不存在：不算走查失败' ($planR5.unproven -join ';')

$R3 = Join-Path $Work 'rs-pathmod'
New-File (Join-Path $R3 '.git\HEAD') "ref: refs/heads/main`n"
New-File (Join-Path $R3 'Cargo.toml') "[package]`nname = 'dx'`nversion = '0.1.0'`nedition = '2021'`n`n[workspace]`n"
New-File (Join-Path $R3 'src\lib.rs') "pub fn f() -> i32 { 1 }`n"
New-File (Join-Path $R3 'src\bin\big.rs') ((@('#[path = "big/asset.rs"]', 'mod asset;', 'fn main() { asset::run(); }') -join $nl) + $nl)
New-File (Join-Path $R3 'src\bin\big\asset.rs') "pub fn run() {}`n"
New-File (Join-Path $R3 'src\bin\other.rs') "fn main() {}`n"
New-File (Join-Path $R3 'tests\t.rs') "#[test] fn t_int() { assert_eq!(dx::f(), 1); }`n"
$outR6 = Join-Path $Work 'plan-r6.json'
& pwsh -NoProfile -File (Join-Path $tools 'affected.ps1') -Path $R3 -Changed (Join-Path $R3 'src\bin\big\asset.rs') -Lang rust -Out $outR6 | Out-Null
$planR6 = Get-Content $outR6 -Raw | ConvertFrom-Json
Assert-True ($planR6.proven -eq $true -and $planR6.commands[0] -match '--bin big' -and $planR6.commands[0] -notmatch '--lib|--bin other|--test') 'B13 #[path] 字面量按静态路径解析→只选该 bin' "$($planR6.commands[0]) / $($planR6.unproven -join ';')"
$outR7 = Join-Path $Work 'plan-r7.json'
& pwsh -NoProfile -File (Join-Path $tools 'affected.ps1') -Path $R3 -Changed (Join-Path $R3 'src\bin\big.rs') -Lang rust -Out $outR7 | Out-Null
$planR7 = Get-Content $outR7 -Raw | ConvertFrom-Json
Assert-True ($planR7.proven -eq $true -and $planR7.commands[0] -match '--bin big' -and $planR7.commands[0] -notmatch '--lib') 'B14 target root 自身变更直接判可达（无需走查）' ($planR7.unproven -join ';')

$R4 = Join-Path $Work 'rs-macro'
New-File (Join-Path $R4 '.git\HEAD') "ref: refs/heads/main`n"
New-File (Join-Path $R4 'Cargo.toml') "[package]`nname = 'mx'`nversion = '0.1.0'`nedition = '2021'`n`n[workspace]`n"
New-File (Join-Path $R4 'src\lib.rs') "macro_rules! m { () => { mod gen; } }`n"
New-File (Join-Path $R4 'src\bin\only.rs') "fn main() {}`n"
New-File (Join-Path $R4 'src\bin\other.rs') "fn main() {}`n"
$outR8 = Join-Path $Work 'plan-r8.json'
& pwsh -NoProfile -File (Join-Path $tools 'affected.ps1') -Path $R4 -Changed (Join-Path $R4 'src\bin\only.rs') -Lang rust -Out $outR8 | Out-Null
$planR8 = Get-Content $outR8 -Raw | ConvertFrom-Json
Assert-True ($planR8.proven -eq $false -and $planR8.commands[0] -eq 'cargo test -p mx --no-fail-fast' -and (@($planR8.unproven) -match 'macro').Count -gt 0) 'B15 宏体声明 mod→回退包级 unproven（宏生成面不可证）' "$($planR8.commands[0]) / $($planR8.unproven -join ';')"

$R5 = Join-Path $Work 'rs-include'
New-File (Join-Path $R5 '.git\HEAD') "ref: refs/heads/main`n"
New-File (Join-Path $R5 'Cargo.toml') "[package]`nname = 'ix'`nversion = '0.1.0'`nedition = '2021'`n`n[workspace]`n"
New-File (Join-Path $R5 'src\lib.rs') "include!(`"extra.rs`");`n"
New-File (Join-Path $R5 'src\bin\only.rs') "fn main() {}`n"
New-File (Join-Path $R5 'src\bin\other.rs') "fn main() {}`n"
$outR9 = Join-Path $Work 'plan-r9.json'
& pwsh -NoProfile -File (Join-Path $tools 'affected.ps1') -Path $R5 -Changed (Join-Path $R5 'src\bin\only.rs') -Lang rust -Out $outR9 | Out-Null
$planR9 = Get-Content $outR9 -Raw | ConvertFrom-Json
Assert-True ($planR9.proven -eq $false -and (@($planR9.unproven) -match 'include').Count -gt 0 -and $planR9.commands[0] -eq 'cargo test -p ix --no-fail-fast') 'B16 include!() 粘贴条目→回退包级 unproven' "$($planR9.commands[0]) / $($planR9.unproven -join ';')"
$outR10 = Join-Path $Work 'plan-r10.json'
& pwsh -NoProfile -File (Join-Path $tools 'affected.ps1') -Path $R5 -Changed (Join-Path $R5 'src\lib.rs') -Lang rust -Out $outR10 | Out-Null
$planR10 = Get-Content $outR10 -Raw | ConvertFrom-Json
Assert-True ($planR10.proven -eq $true -and $planR10.commands[0] -eq 'cargo test -p ix --no-fail-fast') 'B17 root 自身变更无需走查→包级 proven（毒标记不误伤）' ($planR10.unproven -join ';')

$outR11 = Join-Path $Work 'plan-r11.json'
& pwsh -NoProfile -File (Join-Path $tools 'affected.ps1') -Path $R1 -Changed (Join-Path $R1 'src\bin\only.rs') -Lang rust -MaxFiles 2 -Out $outR11 | Out-Null
$planR11 = Get-Content $outR11 -Raw | ConvertFrom-Json
Assert-True ($planR11.proven -eq $false -and (@($planR11.unproven) -match 'cap').Count -gt 0 -and $planR11.commands[0] -eq 'cargo test -p bx --no-fail-fast') 'B18 文件访问数超上限→fail-closed 回退包级并注明原因' "$($planR11.commands[0]) / $($planR11.unproven -join ';')"

$R6 = Join-Path $Work 'rs-deps'
New-File (Join-Path $R6 '.git\HEAD') "ref: refs/heads/main`n"
New-File (Join-Path $R6 'Cargo.toml') "[workspace]`nresolver = '2'`nmembers = ['a', 'b']`n"
New-File (Join-Path $R6 'a\Cargo.toml') "[package]`nname = 'ax'`nversion = '0.1.0'`nedition = '2021'`n"
New-File (Join-Path $R6 'a\src\lib.rs') "pub fn f() -> i32 { 1 }`n"
New-File (Join-Path $R6 'a\src\bin\only.rs') "fn main() {}`n"
New-File (Join-Path $R6 'b\Cargo.toml') "[package]`nname = 'bxx'`nversion = '0.1.0'`nedition = '2021'`n`n[dependencies]`nax = { path = '../a' }`n"
New-File (Join-Path $R6 'b\src\lib.rs') "pub fn g() -> i32 { ax::f() }`n"
$outR12 = Join-Path $Work 'plan-r12.json'
& pwsh -NoProfile -File (Join-Path $tools 'affected.ps1') -Path $R6 -Changed (Join-Path $R6 'a\src\bin\only.rs') -Lang rust -Out $outR12 | Out-Null
$planR12 = Get-Content $outR12 -Raw | ConvertFrom-Json
Assert-True ($planR12.proven -eq $true -and $planR12.commands[0] -match '--bin only' -and $planR12.commands.Count -eq 2 -and $planR12.commands[1] -eq 'cargo test -p bxx --no-fail-fast') 'B19 目标级命令与依赖包命令分列（-p 选择器不能跨包混用）' (($planR12.commands) -join ' | ')

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
    # PS 转发器写一笔，bash 再读——互通必须两边都验过才算（曾写死本机仓库根，
    # 在别的机器上退化成"仓库根恰好有台账就绿"的环境彩票，见 L99）。
    & pwsh -NoProfile -File $tl -Mode touch -Root $Mr -Id $bid -Evidence 'ps-interop' | Out-Null
    Assert-True ($LASTEXITCODE -eq 0) 'M9a PS 转发器 touch 成功' "exit=$LASTEXITCODE"
    $bList = @(& $bashExe $sh -Mode list -Root (ToPosix $Mr))
    Assert-True ($LASTEXITCODE -eq 0 -and (@($bList) -join ' ') -match [regex]::Escape($bid)) 'M9 PS 写的台账 bash 读得懂（interop）' "rows=$(@($bList).Count)"

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

    $n1 = @(& $bashExe $shAff -Path (ToPosix $repo) -Changed 'crates/trit-orch/src/claim.rs' -Lang rust 2>&1)
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
    Assert-True (@($p1 | Where-Object { $_ -match 'no-test-deletion' }).Count -eq 1 -and @($p1).Count -eq 9) 'P1 八条默认规则 + 表头都在（main-tree-raw-delete 入表）' "rows=$(@($p1).Count)"

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
    $t10 = @(& $bashExe $shRun -Root (ToPosix $repo) -Command 'test -f tools/agent_verify.py && echo SAME-FS' 2>&1)
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

""
"== W. trash: 删除必须先进回收站，清单就是账，恢复逐字节一致 =="
if (-not $bashExe) {
    "  SKIP  W0 没有 bash，跳过 trash 入口检查"
    $skipped += 5
} else {
    $shT = Join-Path $tools 'trash.sh'
    $shP2 = Join-Path $tools 'policy.sh'
    $Wt = Join-Path $Work 'w-trash'
    New-Item -ItemType Directory "$Wt\src" -Force | Out-Null
    New-Item -ItemType Directory "$Wt\dir1" -Force | Out-Null
    [IO.File]::WriteAllText("$Wt\src\a.txt", "alpha bytes`n", $enc)
    [IO.File]::WriteAllText("$Wt\keep.txt", "keep`n", $enc)
    [IO.File]::WriteAllText("$Wt\dir1\d.txt", "in dir`n", $enc)
    $origA = [IO.File]::ReadAllBytes("$Wt\src\a.txt")
    $sha256sp = [System.Security.Cryptography.SHA256]::Create()
    $origSha = [BitConverter]::ToString($sha256sp.ComputeHash($origA)).Replace('-', '')

    # W1: 说不清为什么删，就不许删——--reason 是必填，不是礼貌
    & $bashExe $shT put (ToPosix "$Wt\src\a.txt") -Root (ToPosix $Wt) 2>&1 | Out-Null
    Assert-True ($LASTEXITCODE -eq 1) 'W1 put 缺 --Reason 拒绝（说不清为什么删就不许删）' "exit=$LASTEXITCODE"

    # W2: 不存在的路径明确报错（校验先于任何移动，一个都不动）
    $w2 = @(& $bashExe $shT put nope.txt -Reason x -Root (ToPosix $Wt) 2>&1)
    Assert-True ($LASTEXITCODE -ne 0 -and (@($w2) -join ' ') -match 'does not exist') 'W2 不存在的路径明确报错' (@($w2) -join ' / ')

    # W3: put 成功 = 原位清空 + 回收站里有 entry
    $w3 = @(& $bashExe $shT put (ToPosix "$Wt\src\a.txt") -Reason cleanup -Root (ToPosix $Wt) 2>&1)
    $w3id = ''
    foreach ($l in $w3) { if ($l -match 'put (t\d{8}-\d{6}-\d+) ') { $w3id = $Matches[1]; break } }
    Assert-True ($LASTEXITCODE -eq 0 -and $w3id -ne '' -and -not (Test-Path "$Wt\src\a.txt")) 'W3 put 成功且原位清空' "id=$w3id exit=$LASTEXITCODE"

    # W4: 清单记账：sha256/bytes 与被收走的原件一致（记账不是装饰）
    $w4raw = (@(& $bashExe $shT list --json -Root (ToPosix $Wt) 2>&1) -join "`n")
    $w4 = $w4raw | ConvertFrom-Json
    $w4e = @($w4.entries | Where-Object { $_.id -eq $w3id })[0]
    Assert-True ($null -ne $w4e -and $w4e.sha256 -eq $origSha -and $w4e.bytes -eq $origA.Length -and
                 $w4e.path -eq 'src/a.txt' -and $w4e.state -eq 'kept') 'W4 清单 sha/bytes/路径记账与原件一致' "sha=$($w4e.sha256) bytes=$($w4e.bytes)"

    # W5: restore 往返逐字节一致，且恢复时复核 sha
    $w5 = @(& $bashExe $shT restore $w3id -Root (ToPosix $Wt) 2>&1)
    $backA = [IO.File]::ReadAllBytes("$Wt\src\a.txt")
    $backSha = [BitConverter]::ToString($sha256sp.ComputeHash($backA)).Replace('-', '')
    Assert-True ($LASTEXITCODE -eq 0 -and (@($w5) -join ' ') -match 'sha256=\w+ bytes=\d+ ok') 'W5 restore 报告 sha 复核通过' (@($w5) -join ' / ')
    Assert-True ((Compare-Object $backA $origA).Count -eq 0 -and $backSha -eq $origSha) 'W6 往返逐字节一致且 sha 记账' "sha=$backSha want=$origSha"

    # W7: 占用冲突拒绝——还原不悄悄改名、更不覆盖
    $w7 = @(& $bashExe $shT put (ToPosix "$Wt\src\a.txt") -Reason again -Root (ToPosix $Wt) 2>&1)
    $w7id = ''
    foreach ($l in $w7) { if ($l -match 'put (t\d{8}-\d{6}-\d+) ') { $w7id = $Matches[1]; break } }
    [IO.File]::WriteAllText("$Wt\src\a.txt", "occupied`n", $enc)
    & $bashExe $shT restore $w7id -Root (ToPosix $Wt) 2>&1 | Out-Null
    $w7code = $LASTEXITCODE
    Assert-True ($w7code -ne 0 -and ([IO.File]::ReadAllText("$Wt\src\a.txt", $enc)) -eq "occupied`n") 'W7 目标被占用时拒绝且不改名覆盖' "exit=$w7code"

    # W8: -Into 指定目录还原
    $w8 = @(& $bashExe $shT put (ToPosix "$Wt\keep.txt") -Reason keep-moved -Root (ToPosix $Wt) 2>&1)
    $w8id = ''
    foreach ($l in $w8) { if ($l -match 'put (t\d{8}-\d{6}-\d+) ') { $w8id = $Matches[1]; break } }
    & $bashExe $shT restore $w8id -Into (ToPosix "$Wt\restored") -Root (ToPosix $Wt) 2>&1 | Out-Null
    Assert-True ($LASTEXITCODE -eq 0 -and ([IO.File]::ReadAllText("$Wt\restored\keep.txt", $enc)) -eq "keep`n") 'W8 -Into 指定目录还原' "exit=$LASTEXITCODE"

    # W9/W10: purge 的显式双重确认，缺一拒绝
    [IO.File]::WriteAllText("$Wt\purge.txt", "gone soon`n", $enc)
    $w9 = @(& $bashExe $shT put (ToPosix "$Wt\purge.txt") -Reason purge-test -Root (ToPosix $Wt) 2>&1)
    $w9id = ''
    foreach ($l in $w9) { if ($l -match 'put (t\d{8}-\d{6}-\d+) ') { $w9id = $Matches[1]; break } }
    & $bashExe $shT purge $w9id -Root (ToPosix $Wt) 2>&1 | Out-Null
    $w9a = $LASTEXITCODE
    & $bashExe $shT purge $w9id -Confirm wrong-id -Root (ToPosix $Wt) 2>&1 | Out-Null
    $w9b = $LASTEXITCODE
    Assert-True ($w9a -eq 1 -and $w9b -eq 1) 'W9 purge 缺 --Confirm / 确认串不匹配都拒绝' "no=$w9a mismatch=$w9b"
    & $bashExe $shT purge $w9id -Confirm $w9id -Root (ToPosix $Wt) 2>&1 | Out-Null
    $w10raw = (@(& $bashExe $shT list --json -Root (ToPosix $Wt) 2>&1) -join "`n")
    $w10e = @(($w10raw | ConvertFrom-Json).entries | Where-Object { $_.id -eq $w9id })[0]
    Assert-True ($LASTEXITCODE -eq 0 -and $w10e.state -eq 'purged' -and -not (Test-Path "$Wt\purge.txt")) 'W10 双重确认后 purge 成功且记账为 purged' "state=$($w10e.state)"

    # W11/W12: policy 拦裸删、只放行经 trash.py 的调用——放行口只有这一条
    & $bashExe $shP2 -Profile workspace-write -Root (ToPosix $Wt) -Command 'rm src/keep.txt' 2>&1 | Out-Null
    $w11 = $LASTEXITCODE
    & $bashExe $shP2 -Profile workspace-write -Root (ToPosix $Wt) -Command 'trash put src/keep.txt -Reason r' 2>&1 | Out-Null
    $w12 = $LASTEXITCODE
    Assert-True ($w11 -eq 1 -and $w12 -eq 0) 'W11/W12 policy 拦 rm、放行 trash put（唯一放行口）' "deny=$w11 allow=$w12"

    # W13: journal restore 不再直接 unlink——快照期间新增的文件进回收站，回执尾部带 entry id
    $Wj = Join-Path $Work 'w-journal'
    New-Item -ItemType Directory $Wj -Force | Out-Null
    [IO.File]::WriteAllText("$Wj\keep.txt", "original`n", $enc)
    & $bashExe (Join-Path $tools 'journal.sh') begin -Root (ToPosix $Wj) 2>&1 | Out-Null
    [IO.File]::WriteAllText("$Wj\added.txt", "born after snapshot`n", $enc)
    $w13 = @(& $bashExe (Join-Path $tools 'journal.sh') restore -Root (ToPosix $Wj) 2>&1)
    $w13raw = (@($w13) -join "`n")
    $w13man = "$Wj\.journal\trash\manifest.json"
    $w13e = $null
    if (Test-Path $w13man) {
        $w13e = @(((Get-Content $w13man -Raw | ConvertFrom-Json).entries) |
            Where-Object { $_.path -eq 'added.txt' })[0]
    }
    Assert-True ($LASTEXITCODE -eq 0 -and -not (Test-Path "$Wj\added.txt") -and $w13raw -match 'journal: trash t\d{8}') 'W13 journal restore 把新增文件收进回收站（原位清空 + 回执带 entry id）' (@($w13) -join ' / ')
    Assert-True ($null -ne $w13e -and $w13e.reason -match '^journal-restore:' -and $w13e.state -eq 'kept') 'W14 回收站记账 reason=journal-restore、状态 kept' "reason=$($w13e.reason)"

    # W15: 目录整进整出逐字节一致（dir 是一个 entry，不是一次逐文件遍历）
    $w15 = @(& $bashExe $shT put (ToPosix "$Wt\dir1") -Reason whole-dir -Root (ToPosix $Wt) 2>&1)
    $w15id = ''
    foreach ($l in $w15) { if ($l -match 'put (t\d{8}-\d{6}-\d+) ') { $w15id = $Matches[1]; break } }
    & $bashExe $shT restore $w15id -Root (ToPosix $Wt) 2>&1 | Out-Null
    Assert-True ($LASTEXITCODE -eq 0 -and ([IO.File]::ReadAllText("$Wt\dir1\d.txt", $enc)) -eq "in dir`n") 'W15 目录整进整出、内容逐字节一致' "exit=$LASTEXITCODE"

    # W16: 跨卷分支在单卷机器上也要被跑到——TRASH_FORCE_COPY 强制走 copy+verify
    [IO.File]::WriteAllText("$Wt\keep.txt", "keep again`n", $enc)
    $env:TRASH_FORCE_COPY = '1'
    $w16 = @(& $bashExe $shT put (ToPosix "$Wt\keep.txt") -Reason forcecopy -Root (ToPosix $Wt) 2>&1)
    Remove-Item Env:TRASH_FORCE_COPY -ErrorAction SilentlyContinue
    $w16id = ''
    foreach ($l in $w16) { if ($l -match 'put (t\d{8}-\d{6}-\d+) ') { $w16id = $Matches[1]; break } }
    & $bashExe $shT restore $w16id -Root (ToPosix $Wt) 2>&1 | Out-Null
    Assert-True ((@($w16) -join ' ') -match 'via=copy\+verify' -and $LASTEXITCODE -eq 0 -and
                 ([IO.File]::ReadAllText("$Wt\keep.txt", $enc)) -eq "keep again`n") 'W16 copy+verify 分支往返一致（跨卷路径的替身）' (@($w16) -join ' / ')

    # W17: 站点没被 gitignore 时 put 必须 fail-closed（不脏 git status 是硬约束）
    $Wg = Join-Path $Work 'w-git'
    New-Item -ItemType Directory $Wg -Force | Out-Null
    [IO.File]::WriteAllText("$Wg\g.txt", "g`n", $enc)
    & git -C $Wg init -q 2>&1 | Out-Null
    & git -C $Wg add -A 2>&1 | Out-Null
    & git -C $Wg -c user.email=s@t -c user.name=s commit -qm base 2>&1 | Out-Null
    & $bashExe $shT put (ToPosix "$Wg\g.txt") -Reason x -Root (ToPosix $Wg) 2>&1 | Out-Null
    $w17a = $LASTEXITCODE
    [IO.File]::WriteAllText("$Wg\.gitignore", ".journal/`n", $enc)
    & $bashExe $shT put (ToPosix "$Wg\g.txt") -Reason x -Root (ToPosix $Wg) 2>&1 | Out-Null
    $w17b = $LASTEXITCODE
    Assert-True ($w17a -eq 1 -and $w17b -eq 0) 'W17 站点未被 gitignore 拒收；补上 ignore 后放行' "before=$w17a after=$w17b"

    # W18/W19: constraints 的 no-raw-delete 命令侧——拦 rm，放行 trash put
    # 夹具必须是 worktree（.git 文件形态）：主检出上的整删现在由 main-tree-raw-delete 管
    # （审批语义、回收站账不抵扣，见 AD 组）；no-raw-delete 的原语义只在 worktree/无仓库上
    # 适用——夹具形态是断言的一部分，断言说什么，夹具就得是什么（L108）。
    $Wcm = Join-Path $Work 'w-con-main'
    New-Item -ItemType Directory "$Wcm\src" -Force | Out-Null
    [IO.File]::WriteAllText("$Wcm\src\x.txt", "x content`n", $enc)
    [IO.File]::WriteAllText("$Wcm\src\y.txt", "y content`n", $enc)
    [IO.File]::WriteAllText("$Wcm\.gitignore", ".journal/`n.agent/`n", $enc)
    & git -C $Wcm init -q 2>&1 | Out-Null
    & git -C $Wcm add -A 2>&1 | Out-Null
    & git -C $Wcm -c user.email=s@t -c user.name=s commit -qm base 2>&1 | Out-Null
    $Wc = Join-Path $Work 'w-con'
    & git -C $Wcm worktree add $Wc -b w-con-wt 2>&1 | Out-Null
    # 形态断言用 .NET 存在性检查，不用 Get-Item：git 给 worktree 的 .git 文件打了
    # Windows Hidden 属性，Get-Item 不带 -Force 看不见（Test-Path 却看得见）——
    # 无 EAP=Stop 时 $null.PSIsContainer 会让这条断言静默假通过（L107）。
    Assert-True (-not [IO.Directory]::Exists("$Wc\.git") -and [IO.File]::Exists("$Wc\.git")) `
        'W18f 夹具自证为 worktree（.git 文件形态，主库审批闸不在此点火）' ''
    & $bashExe (Join-Path $tools 'constraints.sh') init -Root (ToPosix $Wc) 2>&1 | Out-Null
    & $bashExe (Join-Path $tools 'constraints.sh') check -Root (ToPosix $Wc) -Commands 'rm src/x.txt' 2>&1 | Out-Null
    $w18 = $LASTEXITCODE
    & $bashExe (Join-Path $tools 'constraints.sh') check -Root (ToPosix $Wc) -Commands 'trash put src/x.txt -Reason r' 2>&1 | Out-Null
    Assert-True ($w18 -eq 1 -and $LASTEXITCODE -eq 0) 'W18/W19 constraints 命令侧：拦 rm、放行 trash put' "deny=$w18 allow=$LASTEXITCODE"

    # W20: 清单里有 entry 的删除放行（companion 不是注释，是回收站的账）
    & $bashExe $shT put (ToPosix "$Wc\src\y.txt") -Reason con-test -Root (ToPosix $Wc) 2>&1 | Out-Null
    & $bashExe (Join-Path $tools 'constraints.sh') check -Root (ToPosix $Wc) 2>&1 | Out-Null
    $w20 = $LASTEXITCODE
    # W21: 清单里没有 entry 的整文件删除判红——git 对整删打印 `+++ /dev/null`，
    # 旧解析器只认 `+++ b/`，整删过的文件从来进不了规则视野
    Remove-Item "$Wc\src\x.txt"
    $w21 = @(& $bashExe (Join-Path $tools 'constraints.sh') check -Root (ToPosix $Wc) 2>&1)
    Assert-True ($w20 -eq 0 -and $LASTEXITCODE -eq 1 -and (@($w21) -join ' ') -match 'BLOCK \[no-raw-delete\]') 'W20/W21 有 entry 放行、裸删判红（整文件删除也被看见）' "with-entry=$w20 raw=$LASTEXITCODE"
}

""
"== X. git_safe: checkpoint 不动 HEAD/index/工作区，rescue 给出逐字可复制的恢复命令 =="
if (-not $bashExe) {
    "  SKIP  X0 没有 bash，跳过 git_safe 入口检查"
    $skipped += 4
} else {
    $shG = Join-Path $tools 'git-safe.sh'
    $Xg = Join-Path $Work 'x-git'
    New-Item -ItemType Directory "$Xg\src" -Force | Out-Null
    [IO.File]::WriteAllText("$Xg\src\a.rs", "fn a() {}`n", $enc)
    & git -C $Xg init -q 2>&1 | Out-Null
    & git -C $Xg add -A 2>&1 | Out-Null
    & git -C $Xg -c user.email=s@t -c user.name=s commit -qm base 2>&1 | Out-Null
    [IO.File]::WriteAllText("$Xg\src\a.rs", "fn a() {} // touched`n", $enc)
    [IO.File]::WriteAllText("$Xg\new.txt", "untracked`n", $enc)
    $headBefore = (& git -C $Xg rev-parse HEAD)
    $stBefore = (@(& git -C $Xg status --porcelain)) -join "`n"

    # X1/X2: 没有仓库、没有任何提交，都要 fail-closed 并说清
    $Xn = Join-Path $Work 'x-nogit'
    New-Item -ItemType Directory $Xn -Force | Out-Null
    & $bashExe $shG checkpoint -Root (ToPosix $Xn) 2>&1 | Out-Null
    $x1 = $LASTEXITCODE
    $Xe = Join-Path $Work 'x-nocommit'
    & git -C $Xe init -q 2>&1 | Out-Null
    & $bashExe $shG checkpoint -Root (ToPosix $Xe) 2>&1 | Out-Null
    $x2 = $LASTEXITCODE
    Assert-True ($x1 -ne 0 -and $x2 -ne 0) 'X1/X2 无仓库 / 无提交时 fail-closed' "nogit=$x1 nocommit=$x2"

    # X3: checkpoint 成功，落到 refs/recycle/checkpoint/<ts>-<label>
    $x3 = @(& $bashExe $shG checkpoint -Label wip -Root (ToPosix $Xg) 2>&1)
    $cid = ''
    foreach ($l in $x3) { if ($l -match 'checkpoint ([0-9a-f]{40})') { $cid = $Matches[1]; break } }
    Assert-True ($LASTEXITCODE -eq 0 -and $cid -ne '' -and (@($x3) -join ' ') -match 'refs/recycle/checkpoint/') 'X3 checkpoint 落到 refs/recycle/checkpoint 并输出 id' "cid=$cid"

    # X4: HEAD 一步没动
    $headAfter = (& git -C $Xg rev-parse HEAD)
    Assert-True ($headAfter -eq $headBefore) 'X4 HEAD 未被 checkpoint 移动' "$headBefore -> $headAfter"

    # X5: index 与工作区一步没动（临时索引只写自己的文件，未跟踪的还是未跟踪）
    $stAfter = (@(& git -C $Xg status --porcelain)) -join "`n"
    $staged = @(& git -C $Xg diff --cached --name-only).Count
    Assert-True ($stAfter -eq $stBefore -and $staged -eq 0) 'X5 index/工作区未被触碰（porcelain 前后一致、staged 为空）' "staged=$staged"

    # X6: checkpoint 真的装下了工作区：未跟踪文件与已改内容都在 ref 的树里
    $tree = (@(& git -C $Xg ls-tree -r --name-only $cid)) -join ' '
    $captured = (& git -C $Xg show "${cid}:src/a.rs")
    Assert-True ($tree -match 'new\.txt' -and $captured -match 'touched') 'X6 ref 树里含未跟踪文件与已改内容' "$tree / $captured"

    # X7: list 列出 checkpoint
    $x7 = @(& $bashExe $shG list -Root (ToPosix $Xg) 2>&1)
    Assert-True ((@($x7) -join ' ') -match 'refs/recycle/checkpoint/.*wip') 'X7 list 列出 checkpoint ref' (@($x7) -join ' / ')

    # X8: verify 对可解析的 ref 放行，对不存在的拒绝（fail-closed）
    & $bashExe $shG verify $cid -Root (ToPosix $Xg) 2>&1 | Out-Null
    $x8a = $LASTEXITCODE
    & $bashExe $shG verify ghost -Root (ToPosix $Xg) 2>&1 | Out-Null
    Assert-True ($x8a -eq 0 -and $LASTEXITCODE -eq 1) 'X8 verify 可解析放行、不存在拒绝' "ok=$x8a ghost=$LASTEXITCODE"

    # X9: rescue 打印 reflog 尾部与逐字可复制的恢复命令
    $x9 = @(& $bashExe $shG rescue -Root (ToPosix $Xg) 2>&1)
    $x9raw = (@($x9) -join "`n")
    Assert-True ($x9raw -match 'restore --source=' -and $x9raw -match [regex]::Escape($cid) -and $x9raw -match '\[1\] reflog') 'X9 rescue 含 reflog 与恢复命令块' ($x9raw.Substring(0, [math]::Min(120, $x9raw.Length)))

    # X10: 第二个 checkpoint 带不同 label，list 里两条都在
    & $bashExe $shG checkpoint -Label again -Root (ToPosix $Xg) 2>&1 | Out-Null
    $x10 = @(& $bashExe $shG list -Root (ToPosix $Xg) 2>&1)
    $refs = @($x10 | Where-Object { $_ -match 'refs/recycle/checkpoint/' })
    Assert-True ($LASTEXITCODE -eq 0 -and $refs.Count -ge 2 -and ((@($x10) -join ' ') -match 'again')) 'X10 第二个 checkpoint 带 label，list 可见两条' "refs=$($refs.Count)"
}

""
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

""
"== AA. signal: 分诊 / 惩罚信号 / 记忆 brief / 研究导出（规则分诊，不是模型判断）=="
if (-not $bashExe) {
    "  SKIP  AA0 没有 bash，跳过 signal 检查"
    $skipped += 35
} else {
    $shSig = Join-Path $tools 'signal.sh'
    $As = Join-Path $Work 'a-signal'
    New-Item -ItemType Directory $As -Force | Out-Null
    $asroot = ToPosix $As

    # 分诊：task / emotion / mixed，单行 ASCII JSON，毫秒级（不调任何 API）
    $c1 = @(& $bashExe $shSig classify --text 'fix the failing test in src/core.py and rerun cargo test' --root $asroot 2>&1)
    $c1doc = (@($c1)[0] | ConvertFrom-Json)
    Assert-True ($LASTEXITCODE -eq 0 -and $c1doc.kind -eq 'task' -and $c1doc.emotion_score -eq 0) `
        'AA1 纯任务文本判 task、score=0' "exit=$LASTEXITCODE out=$((@($c1) -join ' '))"
    Assert-True ((@($c1doc.signals) -contains 'imperative') -and (@($c1doc.signals) -contains 'tooling')) `
        'AA2 命中的规则名进 signals' (@($c1doc.signals) -join ',')
    Assert-True ((@($c1doc.task_hints) -contains 'fix') -and (@($c1doc.task_hints) -contains 'src/core.py') `
        -and (@($c1doc.task_hints) -contains 'cargo')) 'AA3 祈使动词/文件名/工具名进 task_hints' (@($c1doc.task_hints) -join ',')

    $c2 = @(& $bashExe $shSig classify --text 'this is hopeless, always fails, I want to give up' --root $asroot 2>&1)
    $c2doc = (@($c2)[0] | ConvertFrom-Json)
    Assert-True ($LASTEXITCODE -eq 0 -and $c2doc.kind -eq 'emotion' -and $c2doc.emotion_score -eq 2) `
        'AA4 绝望措辞判 emotion、score 按权重累计' "score=$($c2doc.emotion_score) kind=$($c2doc.kind)"

    $c3 = @(& $bashExe $shSig classify --text 'frustrated -- fix src/core.py now' --root $asroot 2>&1)
    $c3doc = (@($c3)[0] | ConvertFrom-Json)
    Assert-True ($c3doc.kind -eq 'mixed' -and $c3doc.emotion_score -ge 1) `
        'AA5 情绪与任务并存判 mixed' "kind=$($c3doc.kind) score=$($c3doc.emotion_score)"

    $c4 = @(& $bashExe $shSig classify --text '很好的需求，让我 very frustrated，重做' --root $asroot 2>&1)
    Assert-True ((@($c4) -join ' ') -match '"emotion_score": 1' `
        -and (@($c4) | Where-Object { $_ -match '[^\x00-\x7F]' }).Count -eq 0) `
        'AA6 中文输入也输出单行纯 ASCII JSON（ensure_ascii）' (@($c4) -join ' ')

    # 规则表被 --rules 整体覆盖（不是合并）：覆盖后默认规则不再命中
    $aRules = Join-Path $Work 'a-rules.json'
    [IO.File]::WriteAllText($aRules, '{"emotion":{"joy":{"weight":3,"any":["great"]}},"task":{"imperative":{"any":["ship"]}}}', $enc)
    $c5 = @(& $bashExe $shSig classify --text 'great hopeless ship it' --rules (ToPosix $aRules) --root $asroot 2>&1)
    $c5doc = (@($c5)[0] | ConvertFrom-Json)
    Assert-True ($c5doc.emotion_score -eq 3 -and $c5doc.kind -eq 'mixed') `
        'AA7 规则覆盖生效（自定义 weight=3 累计到 score）' "score=$($c5doc.emotion_score) kind=$($c5doc.kind)"
    Assert-True (@($c5doc.signals) -notcontains 'despair') `
        'AA8 --rules 是整体替换不是合并（默认 despair 不再命中）' (@($c5doc.signals) -join ',')

    & $bashExe $shSig classify --text 'x' --rules '{"emotion": {}}' --root $asroot 2>&1 | Out-Null
    Assert-True ($LASTEXITCODE -eq 2) 'AA9 缺 task 键的规则表被拒（fail-closed）' "exit=$LASTEXITCODE"
    & $bashExe $shSig classify --root $asroot 2>&1 | Out-Null
    Assert-True ($LASTEXITCODE -eq 2) 'AA9b 无输入源（--text/--file 都缺）被拒' "exit=$LASTEXITCODE"

    # 分诊入账：classify 自动进台账（含 text，供 --with-text 导出）。
    # ts 一律在原始行上断言：ConvertFrom-Json 会把 ISO 串转成 [DateTime]（L03 同族），
    # 解析对象上的 -match 走本机区域格式，永远对不上。
    $asStore = Join-Path $As '.agent\signals.jsonl'
    $asLines = @([IO.File]::ReadAllLines($asStore, $enc) | Where-Object { $_.Trim() })
    $asDocs = @($asLines | ForEach-Object { $_ | ConvertFrom-Json })
    $asTask = @($asDocs | Where-Object { $_.kind -eq 'task' })[0]
    Assert-True ($asDocs.Count -eq 5 -and $asTask.text_len -gt 0 -and $asTask.text -ne '' `
        -and (@($asLines | Where-Object { $_ -match '"ts": "\d{4}-\d\d-\d\dT' }).Count -eq $asDocs.Count)) `
        'AA10 classify 自动入账（ts/text_len/text 齐全）' "rows=$($asDocs.Count)"

    # 惩罚入账与复读
    & $bashExe $shSig penalty --score 2 --reason 'argued with user intent' --now '2026-11-21T10:00:00' --root $asroot 2>&1 | Out-Null
    Assert-True ($LASTEXITCODE -eq 0) 'AA12 penalty 入账 exit=0' "exit=$LASTEXITCODE"
    & $bashExe $shSig penalty --score 4 --reason x --root $asroot 2>&1 | Out-Null
    Assert-True ($LASTEXITCODE -eq 2) 'AA12b score 越界（4）被拒' "exit=$LASTEXITCODE"
    & $bashExe $shSig penalty --score 0 --reason x --root $asroot 2>&1 | Out-Null
    Assert-True ($LASTEXITCODE -eq 2) 'AA12c score=0 被拒（不带信号的惩罚不是惩罚）' "exit=$LASTEXITCODE"
    & $bashExe $shSig penalty --score 2 --root $asroot 2>&1 | Out-Null
    Assert-True ($LASTEXITCODE -eq 2) 'AA12d 缺 --reason 被拒（一个数不是信号）' "exit=$LASTEXITCODE"

    & $bashExe $shSig penalty --score 3 --reason '骂了评审' --now '2026-11-21T11:00:00' --root $asroot 2>&1 | Out-Null
    Assert-True ($LASTEXITCODE -eq 0) 'AA13 中文 reason 的 penalty 入账 exit=0' "exit=$LASTEXITCODE"
    $asRaw = [IO.File]::ReadAllText($asStore, $enc)
    Assert-True ((($asRaw -replace '[^\x00-\x7F]', '').Length) -eq $asRaw.Length) `
        'AA11 台账文件纯 ASCII（中文以 \uXXXX 落盘，任何宿主都读不坏）' ''
    $asLines2 = @([IO.File]::ReadAllLines($asStore, $enc) | Where-Object { $_.Trim() })
    $asDocs2 = @($asLines2 | ForEach-Object { $_ | ConvertFrom-Json })
    $pen = @($asDocs2 | Where-Object { $_.kind -eq 'penalty' })
    $penRaw = @($asLines2 | Where-Object { $_ -match '"kind": "penalty"' })
    Assert-True ($pen.Count -eq 2 -and $pen[0].score -eq 2 -and $pen[0].reason -eq 'argued with user intent' `
        -and $penRaw[0] -match [regex]::Escape('"ts": "2026-11-21T10:00:00')) `
        'AA13b penalty 记录字段完整（ts/kind=penalty/score/reason）' "pen=$($pen.Count)"

    # brief：无事件=neutral；窗口内按峰值定标签、带计数；窗口外不说话
    $Ab = Join-Path $Work 'a-brief'
    New-Item -ItemType Directory $Ab -Force | Out-Null
    $abroot = ToPosix $Ab
    $b1 = @(& $bashExe $shSig brief --root $abroot 2>&1)
    Assert-True ($LASTEXITCODE -eq 0 -and (@($b1) -join '') -eq 'mood: neutral') `
        'AA14 无事件（连台账都没有）时 brief=mood: neutral，不是报错' (@($b1) -join ' ')
    & $bashExe $shSig penalty --score 1 --reason 'a' --now '2026-11-21T10:00:00' --root $abroot 2>&1 | Out-Null
    & $bashExe $shSig penalty --score 2 --reason 'b' --now '2026-11-21T10:30:00' --root $abroot 2>&1 | Out-Null
    & $bashExe $shSig penalty --score 1 --reason 'c' --now '2026-11-21T11:00:00' --root $abroot 2>&1 | Out-Null
    $b2 = @(& $bashExe $shSig brief --now '2026-11-21T11:30:00' --root $abroot 2>&1)
    Assert-True ((@($b2) -join '') -match [regex]::Escape('mood: frustrated(score=2, 3 events in last day) -- reflect harder, do not argue user intent')) `
        'AA15 窗口内三条惩罚的汇总形态：峰值定标签、括号里带计数与窗口' (@($b2) -join ' ')
    Assert-True ((@($b2).Count -eq 1) -and (@($b2) | Where-Object { $_ -match '[^\x00-\x7F]' }).Count -eq 0) `
        'AA15b brief 单行纯 ASCII（em-dash 之类的非 ASCII 一律不出现）' ''
    $b3 = @(& $bashExe $shSig brief --now '2026-11-23T11:31:00' --root $abroot 2>&1)
    Assert-True ((@($b3) -join '') -eq 'mood: neutral') `
        'AA16 窗口外（>24h）的事件不再说话' (@($b3) -join ' ')

    # export：默认脱敏的研究语料；--with-text 显式带原文
    $aExp = Join-Path $Work 'a-export.jsonl'
    & $bashExe $shSig export --jsonl (ToPosix $aExp) --root $asroot 2>&1 | Out-Null
    Assert-True ($LASTEXITCODE -eq 0) 'AA17 export exit=0' "exit=$LASTEXITCODE"
    $expLines = @([IO.File]::ReadAllLines($aExp, $enc) | Where-Object { $_.Trim() })
    $expDocs = @($expLines | ForEach-Object { $_ | ConvertFrom-Json })
    $penRows = @($expDocs | Where-Object { $_.kind -eq 'penalty' })
    $clsRows = @($expDocs | Where-Object { $_.kind -ne 'penalty' })
    Assert-True ($expDocs.Count -eq 7 -and $penRows.Count -eq 2 -and $clsRows.Count -eq 5) `
        'AA17b 语料行数=台账事件数（classify 与 penalty 都在）' "rows=$($expDocs.Count) pen=$($penRows.Count) cls=$($clsRows.Count)"
    Assert-True ((@($expDocs | Where-Object { $_.PSObject.Properties.Name -contains 'text' }).Count -eq 0) `
        -and (@($expDocs | Where-Object { $_.PSObject.Properties.Name -contains 'task_hints' }).Count -eq 0)) `
        'AA18 默认导出脱敏：无 text、无 task_hints（抽出的原文片段也是原文）' ''
    Assert-True ($penRows[0].penalty -eq 2 -and $penRows[0].emotion_score -eq 2 `
        -and $penRows[0].text_len -gt 0 -and @($penRows[0].signals).Count -eq 0) `
        'AA19 penalty 行：penalty=emotion_score=score，reason 只留 text_len' ''
    $expJoined = (@($expLines) -join "`n")
    Assert-True ((($expJoined -replace '[^\x00-\x7F]', '').Length) -eq $expJoined.Length) `
        'AA19b 语料文件纯 ASCII' ''

    $aExp2 = Join-Path $Work 'a-export-wt.jsonl'
    & $bashExe $shSig export --jsonl (ToPosix $aExp2) --with-text --root $asroot 2>&1 | Out-Null
    Assert-True ($LASTEXITCODE -eq 0) 'AA20a --with-text export exit=0' "exit=$LASTEXITCODE"
    $wtDocs = @(([IO.File]::ReadAllLines($aExp2, $enc) | Where-Object { $_.Trim() }) | ForEach-Object { $_ | ConvertFrom-Json })
    $wtCls = (@($wtDocs | Where-Object { $_.kind -eq 'task' })[0]).text
    $wtPen = (@($wtDocs | Where-Object { $_.kind -eq 'penalty' })[0]).text
    Assert-True ($wtCls -eq 'fix the failing test in src/core.py and rerun cargo test' `
        -and $wtPen -eq 'argued with user intent') `
        'AA20 --with-text 显式带原文（classify 的 text 与 penalty 的 reason）' ''
    & $bashExe $shSig export --jsonl (ToPosix (Join-Path $As 'inside.jsonl')) --with-text --root $asroot 2>&1 | Out-Null
    Assert-True ($LASTEXITCODE -eq 1) 'AA21 with-text 语料写进 owner root 被拒（防误提交，fail-closed）' "exit=$LASTEXITCODE"
    & $bashExe $shSig export --jsonl (ToPosix $aExp) --root $asroot 2>&1 | Out-Null
    Assert-True ($LASTEXITCODE -eq 1) 'AA21b 覆盖已存在的语料被拒' "exit=$LASTEXITCODE"
    $Ac = Join-Path $Work 'a-empty'
    New-Item -ItemType Directory $Ac -Force | Out-Null
    & $bashExe $shSig export --jsonl (ToPosix (Join-Path $Work 'a-none.jsonl')) --root (ToPosix $Ac) 2>&1 | Out-Null
    Assert-True ($LASTEXITCODE -eq 1) 'AA21c 无台账时 export 拒绝（不产出空语料冒充数据）' "exit=$LASTEXITCODE"

    # 台账损坏必须 fail-closed：绝不带着坏账继续出货
    [IO.File]::AppendAllText($asStore, "garbage`n", $enc)
    & $bashExe $shSig brief --root $asroot 2>&1 | Out-Null
    Assert-True ($LASTEXITCODE -eq 1) 'AA22 台账坏行让 brief 拒绝（不跳过坏行继续）' "exit=$LASTEXITCODE"
    & $bashExe $shSig export --jsonl (ToPosix (Join-Path $Work 'a-exp3.jsonl')) --root $asroot 2>&1 | Out-Null
    Assert-True ($LASTEXITCODE -eq 1) 'AA22b 台账坏行让 export 拒绝' "exit=$LASTEXITCODE"
    & $bashExe $shSig penalty --score 1 --reason x --root $asroot 2>&1 | Out-Null
    Assert-True ($LASTEXITCODE -eq 1) 'AA22c 台账坏行让 penalty 拒绝（坏账不许再追加）' "exit=$LASTEXITCODE"
}

"== AB. declutter: 密度报表、LANDMINE 豁免、导出不动源码树、clean 先进回收站、预算棘轮 =="
# declutter 的剥注释器是三态扫描（代码/字符串/注释），这里每条"字符串里的注释形态"用例
# 都是给它钉的：错一次就是吃掉用户代码或静默留垃圾。LANDMINE 用例钉豁免通道。
if (-not $bashExe) {
    "  SKIP  AB0 没有 bash，跳过 declutter 检查"
    $skipped += 20
} else {
    $shDec = Join-Path $tools 'declutter.sh'
    $abnl = [char]10
    $Ab = Join-Path $Work 'ab-declutter'
    New-Item -ItemType Directory "$Ab\src" -Force | Out-Null
    [IO.File]::WriteAllText("$Ab\src\a.py", (@(
        '#!/usr/bin/env python3',
        '# a normal comment',
        'VALUE = 1  # trailing comment',
        'TEXT = "string with # hash and // slashes"',
        '# LANDMINE: 三态切错字符串整个系统就瘫 -- 保留',
        'MORE = 2') -join $abnl) + $abnl, $enc)
    [IO.File]::WriteAllText("$Ab\src\a.rs", (@(
        '// rust line comment',
        '/* block /* nested */ still */',
        'pub fn f() -> i32 { 1 } // trailing',
        'pub const S: &str = "not // a comment";') -join $abnl) + $abnl, $enc)
    [IO.File]::WriteAllText("$Ab\src\a.js", (@(
        '// js comment',
        'const s = "also // not";',
        '/* block */') -join $abnl) + $abnl, $enc)
    [IO.File]::WriteAllText("$Ab\src\c.js", 'let t = `x${ /* } brace */ y }`;' + $abnl, $enc)
    [IO.File]::WriteAllText("$Ab\src\b.json", (@(
        '{',
        '  // jsonc comment',
        '  "a": 1',
        '}') -join $abnl) + $abnl, $enc)
    $sha = [System.Security.Cryptography.SHA256]::Create()
    $shaPyBefore = [BitConverter]::ToString($sha.ComputeHash([IO.File]::ReadAllBytes("$Ab\src\a.py"))).Replace('-', '')
    $shaJsBefore = [BitConverter]::ToString($sha.ComputeHash([IO.File]::ReadAllBytes("$Ab\src\a.js"))).Replace('-', '')

    $abAudit = @(& $bashExe $shDec audit --json -Root (ToPosix $Ab) 2>&1)
    $abDoc = (@($abAudit) -join "`n") | ConvertFrom-Json
    $abPy = @($abDoc.files | Where-Object { $_.path -eq 'src/a.py' })[0]
    Assert-True ($null -ne $abPy -and $abPy.comments -eq 4 -and $abPy.code -eq 3) `
        'AB1 audit 密度正确（a.py comments=4 code=3，纯注释行不计代码）' "comments=$($abPy.comments) code=$($abPy.code)"
    Assert-True ($abDoc.files[0].path -eq 'src/a.js' -and $abDoc.files[-1].path -eq 'src/b.json') `
        'AB2 按密度降序排序（密者在前）' ((@($abDoc.files | ForEach-Object { $_.path }) -join ','))
    Assert-True ($abDoc.totals.files -eq 5 -and $abDoc.totals.comments -eq 11 -and $abDoc.totals.code -eq 10) `
        'AB3 库级汇总（files/comments/code）' "files=$($abDoc.totals.files) comments=$($abDoc.totals.comments) code=$($abDoc.totals.code)"
    $abLm = @($abDoc.landmines)[0]
    Assert-True ($null -ne $abLm -and $abLm.path -eq 'src/a.py' -and $abLm.line -eq 5 -and $abLm.text -match 'LANDMINE') `
        'AB4 LANDMINE 单列（路径/行号/文本），不带标记的一律视为可清理' "path=$($abLm.path) line=$($abLm.line)"

    $AbOut = Join-Path $Work 'ab-export-out'
    & $bashExe $shDec export --strip --out (ToPosix $AbOut) -Root (ToPosix $Ab) 2>&1 | Out-Null
    $abExp = $LASTEXITCODE
    Assert-True ($abExp -eq 0 -and (Test-Path "$AbOut\src\a.py")) 'AB5 export --strip exit=0 且产物落盘' "exit=$abExp"
    $abOutPy = [IO.File]::ReadAllText("$AbOut\src\a.py", $enc)
    Assert-True (($abOutPy -notmatch '# a normal comment') -and ($abOutPy -match 'VALUE = 1') `
        -and ($abOutPy -notmatch 'trailing comment')) 'AB6 普通注释被剥、代码行原样保留' ($abOutPy.Trim())
    Assert-True (((@($abOutPy -split $abnl))[0] -eq '#!/usr/bin/env python3') -and ($abOutPy -match 'LANDMINE')) `
        'AB7 LANDMINE 注释与 shebang 原样保留' (($abOutPy -split $abnl) -join ' | ')
    Assert-True (($abOutPy -match [regex]::Escape('TEXT = "string with # hash and // slashes"')) `
        -and (([IO.File]::ReadAllText("$AbOut\src\a.js", $enc)) -match [regex]::Escape('const s = "also // not";')) `
        -and (([IO.File]::ReadAllText("$AbOut\src\b.json", $enc)) -match '"a": 1') `
        -and (([IO.File]::ReadAllText("$AbOut\src\b.json", $enc)) -notmatch 'jsonc comment')) `
        'AB8 字符串里的注释形态不被误剥（py/js/json），jsonc 注释照剥' ''
    $abOutRs = [IO.File]::ReadAllText("$AbOut\src\a.rs", $enc)
    Assert-True (($abOutRs -notmatch [regex]::Escape('still */')) `
        -and ($abOutRs -match [regex]::Escape('pub fn f() -> i32 { 1 }'))) `
        'AB9 rust 嵌套块注释整段剥净（残片即错）' (($abOutRs -split $abnl) -join ' | ')
    $abOutCjs = [IO.File]::ReadAllText("$AbOut\src\c.js", $enc)
    Assert-True ($abOutCjs -match [regex]::Escape('let t = `x${  y }`;')) `
        'AB10 模板插值里的注释被剥且插值结构完好（内含 } 的注释不提前闭合）' ($abOutCjs.Trim())
    $shaPyAfter = [BitConverter]::ToString($sha.ComputeHash([IO.File]::ReadAllBytes("$Ab\src\a.py"))).Replace('-', '')
    $shaJsAfter = [BitConverter]::ToString($sha.ComputeHash([IO.File]::ReadAllBytes("$Ab\src\a.js"))).Replace('-', '')
    Assert-True ($shaPyAfter -eq $shaPyBefore -and $shaJsAfter -eq $shaJsBefore) `
        'AB10b export 不碰源码树（sha 前后一致）' ''
    & $bashExe $shDec export --strip --out (ToPosix "$Ab\inside") -Root (ToPosix $Ab) 2>&1 | Out-Null
    Assert-True ($LASTEXITCODE -eq 1 -and -not (Test-Path "$Ab\inside")) `
        'AB11 --out 落在 --root 内拒绝（导出永不写进源码树边界）' "exit=$LASTEXITCODE"

    $AbC = Join-Path $Work 'ab-clean'
    New-Item -ItemType Directory "$AbC\src" -Force | Out-Null
    $origPy = (@('#!/usr/bin/env python3', '# keep me', '# LANDMINE: 地雷保留', 'X = 1  # trail') -join $abnl) + $abnl
    [IO.File]::WriteAllText("$AbC\src\c.py", $origPy, $enc)
    & $bashExe $shDec clean -Root (ToPosix $AbC) 2>&1 | Out-Null
    $abNoGate = $LASTEXITCODE
    Assert-True ($abNoGate -eq 1 -and ([IO.File]::ReadAllText("$AbC\src\c.py", $enc)) -eq $origPy) `
        'AB12 clean 缺 --in-place 直接拒绝且文件未动（破坏档默认关）' "exit=$abNoGate"
    & $bashExe $shDec clean --in-place -Root (ToPosix $AbC) 2>&1 | Out-Null
    $abClean = $LASTEXITCODE
    $cleanedPy = [IO.File]::ReadAllText("$AbC\src\c.py", $enc)
    $abMan = Join-Path $AbC '.journal\trash\manifest.json'
    $abEntry = $null
    if (Test-Path $abMan) {
        $abEntry = @(((Get-Content $abMan -Raw | ConvertFrom-Json).entries) |
            Where-Object { $_.path -eq 'src/c.py' })[0]
    }
    Assert-True ($abClean -eq 0 -and $cleanedPy -notmatch '# keep me' -and $cleanedPy -match 'LANDMINE' `
        -and $cleanedPy -match '#!/usr/bin/env python3' `
        -and $null -ne $abEntry -and $abEntry.reason -eq 'declutter-clean' -and $abEntry.state -eq 'kept') `
        'AB13 in-place 逐文件先 trash.put（reason=declutter-clean）再剥注释写回' "exit=$abClean reason=$($abEntry.reason)"
    Move-Item "$AbC\src\c.py" "$AbC\cleaned-aside.py"
    & $bashExe (Join-Path $tools 'trash.sh') restore $abEntry.id -Root (ToPosix $AbC) 2>&1 | Out-Null
    $abRest = $LASTEXITCODE
    $abBack = [IO.File]::ReadAllText("$AbC\src\c.py", $enc)
    Assert-True ($abRest -eq 0 -and $abBack -eq $origPy) `
        'AB14 已进回收站的原件可 restore 且逐字节一致' "exit=$abRest same=$($abBack -eq $origPy)"

    $AbB = Join-Path $Work 'ab-budget'
    New-Item -ItemType Directory "$AbB\src" -Force | Out-Null
    $abBasePy = (@('# base one', '# base two', 'V = 1') -join $abnl) + $abnl
    [IO.File]::WriteAllText("$AbB\src\m.py", $abBasePy, $enc)
    [IO.File]::WriteAllText("$AbB\src\m2.py", (@('# only', 'W = 2') -join $abnl) + $abnl, $enc)
    & $bashExe $shDec budget init -Root (ToPosix $AbB) 2>&1 | Out-Null
    $abInit = $LASTEXITCODE
    $bud = [IO.File]::ReadAllText("$AbB\.agent\comment-budget.json", $enc) | ConvertFrom-Json
    Assert-True ($abInit -eq 0 -and $bud.files.'src/m.py' -eq 2 -and $bud.files.'src/m2.py' -eq 1) `
        'AB15 budget init 建基线（每文件注释行数，.agent/comment-budget.json）' "exit=$abInit m=$($bud.files.'src/m.py')"
    & $bashExe $shDec budget check -Root (ToPosix $AbB) 2>&1 | Out-Null
    Assert-True ($LASTEXITCODE -eq 0) 'AB15b 基线未变时 check 绿' "exit=$LASTEXITCODE"
    [IO.File]::AppendAllText("$AbB\src\m.py", '# extra line' + $abnl, $enc)
    & $bashExe $shDec budget check -Root (ToPosix $AbB) 2>&1 | Out-Null
    $abChk = $LASTEXITCODE
    & $bashExe $shDec budget init -Root (ToPosix $AbB) 2>&1 | Out-Null
    $abInit2 = $LASTEXITCODE
    $bud2 = [IO.File]::ReadAllText("$AbB\.agent\comment-budget.json", $enc) | ConvertFrom-Json
    Assert-True ($abChk -eq 1 -and $abInit2 -eq 1 -and $bud2.files.'src/m.py' -eq 2) `
        'AB16 只紧不松：新增注释 check 红、init 拒绝抬高、台账未动' "check=$abChk init=$abInit2 cap=$($bud2.files.'src/m.py')"
    [IO.File]::WriteAllText("$AbB\src\m.py", $abBasePy, $enc)
    Remove-Item "$AbB\src\m2.py"
    & $bashExe $shDec budget init -Root (ToPosix $AbB) 2>&1 | Out-Null
    $abInit3 = $LASTEXITCODE
    $bud3 = [IO.File]::ReadAllText("$AbB\.agent\comment-budget.json", $enc) | ConvertFrom-Json
    $abKeys = @($bud3.files.PSObject.Properties.Name) -join ','
    & $bashExe $shDec budget check -Root (ToPosix $AbB) 2>&1 | Out-Null
    Assert-True ($abInit3 -eq 0 -and $abKeys -eq 'src/m.py' -and $LASTEXITCODE -eq 0) `
        'AB17 收紧：孤儿随 init 移除，收紧后 check 绿' "init=$abInit3 keys=$abKeys check=$LASTEXITCODE"
    [IO.File]::WriteAllText("$AbB\.agent\comment-budget.json", '{oops', $enc)
    $abBad = @(& $bashExe $shDec budget check -Root (ToPosix $AbB) 2>&1)
    Assert-True ($LASTEXITCODE -eq 1 -and ((@($abBad) -join ' ') -match 'unreadable')) `
        'AB18 坏 JSON fail-closed（不通过、明说读不了）' "exit=$LASTEXITCODE"

    $abPs = @(& pwsh -NoProfile -File (Join-Path $tools 'declutter.ps1') audit --json -Root $Ab 2>&1)
    $abPsDoc = (@($abPs) -join "`n") | ConvertFrom-Json
    Assert-True ($LASTEXITCODE -eq 0 -and $abPsDoc.totals.files -eq 5) `
        'AB19 PS 转发器与 bash 入口互通（同一份 JSON）' "exit=$LASTEXITCODE files=$($abPsDoc.totals.files)"
}

""
"== AC. watch: 事件账本、衰减分析（行为证据推断）、整盘健康、污染面 =="
# watch 的四个面都在这里：record 自由标签 + 隐私闸；report 分桶失败率与疲劳 WARN
# （触发与不触发两侧、分母只吃认识的标签）；health 四个报警面各带后果说明；
# pollution 三种污染模式只列不拦。夹具全部走临时目录，git 夹具就地 init。
$acTotal = 44
if (-not $bashExe) {
    "  SKIP  AC0 没有 bash，跳过 watch 检查"
    $skipped += $acTotal
} else {
    $shW = Join-Path $tools 'watch.sh'
    $acnl = [char]10

    $AcR = Join-Path $Work 'ac-watch'
    New-Item -ItemType Directory $AcR -Force | Out-Null
    $acroot = ToPosix $AcR
    $acFile = Join-Path $AcR '.agent\watch.jsonl'

    & $bashExe $shW record cmd -AgentId sub-1 -Turn 1 -Ok -Note 'first command' -Root $acroot 2>&1 | Out-Null
    Assert-True ($LASTEXITCODE -eq 0) 'AC1 record exit=0' "exit=$LASTEXITCODE"
    $ac1 = Get-Content $acFile -Raw | ConvertFrom-Json
    Assert-True ($ac1.event -eq 'cmd' -and $ac1.agent_id -eq 'sub-1' -and $ac1.turn -eq 1 `
        -and $ac1.verdict -eq 'ok' -and $ac1.note -eq 'first command' -and $ac1.ts) `
        'AC1b 一行六字段 {ts,event,agent_id,turn,verdict,note} 齐全' ''
    $acRaw = [IO.File]::ReadAllText($acFile, $enc)
    Assert-True ((($acRaw -replace '[^\x00-\x7F]', '').Length) -eq $acRaw.Length) `
        'AC1c 事件文件纯 ASCII（要随仓提交，任何宿主都不读坏）' ''

    & $bashExe $shW record model-thought -Note 'free label' -Root $acroot 2>&1 | Out-Null
    Assert-True ($LASTEXITCODE -eq 0) 'AC2 自由标签照收（事件类型不设白名单）' "exit=$LASTEXITCODE"
    $ac2 = (Get-Content $acFile | Select-Object -Last 1) | ConvertFrom-Json
    Assert-True ($ac2.event -eq 'model-thought' -and $ac2.verdict -eq '') 'AC2b 无 verdict 的记号事件 verdict 为空' ''

    # 事件文件要随仓提交，所以本机绝对路径在 record 门口就拒；拒收不落行。
    $acCount = @(Get-Content $acFile).Count
    & $bashExe $shW record cmd -Note 'see D:\elsewhere\note.py' -Root $acroot 2>&1 | Out-Null
    $ac3a = $LASTEXITCODE
    $acPosix = '/home/' + 'tester' + '/x.py'
    & $bashExe $shW record cmd -AgentId $acPosix -Root $acroot 2>&1 | Out-Null
    Assert-True ($ac3a -ne 0 -and $LASTEXITCODE -ne 0) 'AC3 盘符/家目录形态的本机路径拒收（fail-closed）' "a=$ac3a b=$LASTEXITCODE"
    Assert-True (@(Get-Content $acFile).Count -eq $acCount) 'AC3b 拒收时不落任何行' ''

    # AC4: 分桶失败率上行且两个桶样本都够 → WARN。early=1..10（1 失败，10%），
    # late=30..34（3 失败，60%），上行 50 个百分点。
    $AcF = Join-Path $Work 'ac-fatigue'
    New-Item -ItemType Directory $AcF -Force | Out-Null
    $froot = ToPosix $AcF
    foreach ($t in 1..10) {
        if ($t -eq 3) { & $bashExe $shW record cmd -AgentId main -Turn $t -Fail -Root $froot 2>&1 | Out-Null }
        else { & $bashExe $shW record cmd -AgentId main -Turn $t -Ok -Root $froot 2>&1 | Out-Null }
    }
    & $bashExe $shW record cmd -AgentId main -Turn 30 -Ok -Root $froot 2>&1 | Out-Null
    & $bashExe $shW record cmd -AgentId main -Turn 31 -Fail -Root $froot 2>&1 | Out-Null
    & $bashExe $shW record cmd -AgentId main -Turn 32 -Fail -Root $froot 2>&1 | Out-Null
    & $bashExe $shW record cmd -AgentId main -Turn 33 -Fail -Root $froot 2>&1 | Out-Null
    & $bashExe $shW record cmd -AgentId main -Turn 34 -Ok -Root $froot 2>&1 | Out-Null
    $ac4 = @(& $bashExe $shW report -Root $froot 2>&1)
    Assert-True ($LASTEXITCODE -eq 1) 'AC4 分桶失败率上行且样本足够时 report exit=1' "exit=$LASTEXITCODE"
    Assert-True ((@($ac4) -join ' ') -match 'fatigue suspected at turn 34 \(fail-rate 60\.0% vs 10\.0% early\)') `
        'AC4b WARN 行原文：轮位与两个分桶失败率都对' (@($ac4) -join ' / ')
    Assert-True ((@($ac4) -join ' ') -match 'not mind-reading') 'AC4c 边界句在输出里（行为证据推断不是读心）' ''

    # AC5: 后期桶样本不足 4 不触发——把单个失败当疲劳会让 WARN 失去意义
    $AcN = Join-Path $Work 'ac-thin'
    New-Item -ItemType Directory $AcN -Force | Out-Null
    $nroot = ToPosix $AcN
    foreach ($t in 1..5) { & $bashExe $shW record cmd -Turn $t -Ok -Root $nroot 2>&1 | Out-Null }
    & $bashExe $shW record cmd -Turn 30 -Fail -Root $nroot 2>&1 | Out-Null
    & $bashExe $shW report -Root $nroot 2>&1 | Out-Null
    Assert-True ($LASTEXITCODE -eq 0) 'AC5 后期桶样本不足时不触发 WARN' "exit=$LASTEXITCODE"

    # AC6: 上行不足 20 个百分点不触发——early 20% vs late 30% 只是波动
    $AcM = Join-Path $Work 'ac-margin'
    New-Item -ItemType Directory $AcM -Force | Out-Null
    $mroot = ToPosix $AcM
    foreach ($t in 1..10) {
        if ($t -eq 3 -or $t -eq 7) { & $bashExe $shW record cmd -Turn $t -Fail -Root $mroot 2>&1 | Out-Null }
        else { & $bashExe $shW record cmd -Turn $t -Ok -Root $mroot 2>&1 | Out-Null }
    }
    foreach ($t in 30..39) {
        if ($t -eq 31 -or $t -eq 32 -or $t -eq 35) { & $bashExe $shW record cmd -Turn $t -Fail -Root $mroot 2>&1 | Out-Null }
        else { & $bashExe $shW record cmd -Turn $t -Ok -Root $mroot 2>&1 | Out-Null }
    }
    $ac6 = & $bashExe $shW report -Root $mroot 2>&1
    Assert-True ($LASTEXITCODE -eq 0 -and (@($ac6) -join ' ') -notmatch 'fatigue suspected') `
        'AC6 上行不足阈值（20 个百分点）不触发 WARN' "exit=$LASTEXITCODE"

    # AC7: 自由标签不进分母——noise 标签全失败，分桶失败率也一分不动
    $AcL = Join-Path $Work 'ac-freelabel'
    New-Item -ItemType Directory $AcL -Force | Out-Null
    $lroot = ToPosix $AcL
    foreach ($t in 1..10) {
        if ($t -eq 3) { & $bashExe $shW record cmd -Turn $t -Fail -Root $lroot 2>&1 | Out-Null }
        else { & $bashExe $shW record cmd -Turn $t -Ok -Root $lroot 2>&1 | Out-Null }
    }
    & $bashExe $shW record cmd -Turn 30 -Ok -Root $lroot 2>&1 | Out-Null
    & $bashExe $shW record cmd -Turn 31 -Fail -Root $lroot 2>&1 | Out-Null
    & $bashExe $shW record cmd -Turn 32 -Fail -Root $lroot 2>&1 | Out-Null
    & $bashExe $shW record cmd -Turn 33 -Fail -Root $lroot 2>&1 | Out-Null
    & $bashExe $shW record cmd -Turn 34 -Ok -Root $lroot 2>&1 | Out-Null
    foreach ($t in 31..35) { & $bashExe $shW record noise -Turn $t -Fail -Note 'not an outcome label' -Root $lroot 2>&1 | Out-Null }
    $ac7 = ((& $bashExe $shW report --json -Root $lroot) -join "`n") | ConvertFrom-Json
    Assert-True ($ac7.late.obs -eq 5 -and $ac7.late.fails -eq 3 -and $ac7.late.fail_rate -eq 60.0) `
        'AC7 noise 标签不进分桶（late 仍是 3/5=60%）' "late=$($ac7.late | ConvertTo-Json -Compress)"
    Assert-True ($ac7.counted -eq 15 -and $ac7.events_total -eq 20) 'AC7b counted 与总数分开报（15 计入 / 20 落盘）' "counted=$($ac7.counted) total=$($ac7.events_total)"

    # AC8: json 口径的字段全部对得上
    $ac8 = ((& $bashExe $shW report --json -Root $froot) -join "`n") | ConvertFrom-Json
    Assert-True ($ac8.early.obs -eq 10 -and $ac8.early.fails -eq 1 -and $ac8.early.fail_rate -eq 10.0 `
        -and $ac8.late.obs -eq 5 -and $ac8.late.fails -eq 3 -and $ac8.late.fail_rate -eq 60.0) `
        'AC8 json 分桶数字精确' ''
    Assert-True ($ac8.fail_streak.longest -eq 3 -and $ac8.fail_streak.trailing -eq 0) `
        'AC8b 连续失败串：最长 3（31-33）、尾部 0（34 是 ok）' ''
    Assert-True ($ac8.commands_per_turn.max -eq 1 -and $ac8.agents.main -eq 15 `
        -and $ac8.warn.turn -eq 34 -and $ac8.warn.fired) 'AC8c 每轮命令数、按 agent 分组、warn 轮位' ''

    # AC9: 按 agent_id 分组是免费能力——过滤后口径只算该子代理的
    $AcA = Join-Path $Work 'ac-agents'
    New-Item -ItemType Directory $AcA -Force | Out-Null
    $aroot = ToPosix $AcA
    foreach ($t in 1..10) { & $bashExe $shW record cmd -AgentId alpha -Turn $t -Ok -Root $aroot 2>&1 | Out-Null }
    foreach ($t in 30..33) { & $bashExe $shW record cmd -AgentId beta -Turn $t -Fail -Root $aroot 2>&1 | Out-Null }
    & $bashExe $shW report -Root $aroot 2>&1 | Out-Null
    Assert-True ($LASTEXITCODE -eq 1) 'AC9 全量口径：alpha 全 ok + beta 全失败 → 触发' "exit=$LASTEXITCODE"
    $ac9 = ((& $bashExe $shW report --json -AgentId beta -Root $aroot) -join "`n") | ConvertFrom-Json
    Assert-True ($LASTEXITCODE -eq 0 -and $ac9.counted -eq 4 -and $ac9.agent_filter -eq 'beta') `
        'AC9b --agent-id 过滤后只算 beta 的（early 桶空 → 不触发）' "exit=$LASTEXITCODE counted=$($ac9.counted)"

    # health: 四个报警面，每个都带"再这么干会发生什么"
    $AcC = Join-Path $Work 'ac-clean'
    New-Item -ItemType Directory $AcC -Force | Out-Null
    & $bashExe $shW record test -Ok -Root (ToPosix $AcC) 2>&1 | Out-Null
    $ac10 = & $bashExe $shW health -Root (ToPosix $AcC) 2>&1
    Assert-True ($LASTEXITCODE -eq 0 -and (@($ac10) -join ' ') -match 'alarms=0') 'AC10 干净根 exit=0' "exit=$LASTEXITCODE"

    $AcS = Join-Path $Work 'ac-streak'
    New-Item -ItemType Directory $AcS -Force | Out-Null
    foreach ($i in 1..3) { & $bashExe $shW record cmd -Fail -Note "step $i failed" -Root (ToPosix $AcS) 2>&1 | Out-Null }
    $ac11 = & $bashExe $shW health -Root (ToPosix $AcS) 2>&1
    Assert-True ($LASTEXITCODE -eq 1 -and (@($ac11) -join ' ') -match 'fail-streak') 'AC11 连续失败串报警' (@($ac11) -join ' / ')
    Assert-True ((@($ac11) -join ' ') -match 'checkpoint and fix') 'AC11b 报警带后果说明（继续跑会怎样）' ''

    $AcD = Join-Path $Work 'ac-dirty'
    New-Item -ItemType Directory $AcD -Force | Out-Null
    New-File (Join-Path $AcD 'base.txt') "base$acnl"
    & git -C $AcD init -q 2>&1 | Out-Null
    & git -C $AcD -c user.email=s@t -c user.name=s add -A 2>&1 | Out-Null
    & git -C $AcD -c user.email=s@t -c user.name=s commit -qm base 2>&1 | Out-Null
    foreach ($i in 1..30) { New-File (Join-Path $AcD "dirty$i.txt") "d$acnl" }
    $ac12raw = ((& $bashExe $shW health --json -Root (ToPosix $AcD)) -join "`n")
    $ac12 = $ac12raw | ConvertFrom-Json
    Assert-True ($LASTEXITCODE -eq 1 -and @($ac12.alarms).Count -eq 1 -and $ac12.alarms[0].kind -eq 'dirty-drift') `
        'AC12 30 个脏路径 → dirty-drift 且只报这一条' "alarms=$($ac12raw.Substring(0, [math]::Min(160, $ac12raw.Length)))"
    Assert-True ($ac12.alarms[0].consequence -match 'cannot be restored cleanly') `
        'AC12b 脏漂移的后果说明在场（断粮时现场无法干净恢复）' ''

    $AcG = Join-Path $Work 'ac-giant'
    New-Item -ItemType Directory $AcG -Force | Out-Null
    New-File (Join-Path $AcG 'small.txt') ("l1$acnl" + "l2$acnl")
    & git -C $AcG init -q 2>&1 | Out-Null
    & git -C $AcG -c user.email=s@t -c user.name=s add -A 2>&1 | Out-Null
    & git -C $AcG -c user.email=s@t -c user.name=s commit -qm base 2>&1 | Out-Null
    [IO.File]::WriteAllText((Join-Path $AcG 'small.txt'), ((1..720 | ForEach-Object { "line$_" }) -join $acnl) + $acnl, $enc)
    $ac13 = ((& $bashExe $shW health --json -Root (ToPosix $AcG)) -join "`n") | ConvertFrom-Json
    Assert-True ($LASTEXITCODE -eq 1 -and @($ac13.alarms).Count -eq 1 -and $ac13.alarms[0].kind -eq 'giant-diff') `
        'AC13 700+ 行 diff → giant-diff 且不误报其他面' ''

    $AcU = Join-Path $Work 'ac-uncommitted'
    New-Item -ItemType Directory $AcU -Force | Out-Null
    New-File (Join-Path $AcU 'f.txt') "x$acnl"
    & git -C $AcU init -q 2>&1 | Out-Null
    & git -C $AcU -c user.email=s@t -c user.name=s add -A 2>&1 | Out-Null
    $acOld = (Get-Date).AddDays(-10).ToString('yyyy-MM-ddTHH:mm:ss')
    $env:GIT_AUTHOR_DATE = $acOld; $env:GIT_COMMITTER_DATE = $acOld
    & git -C $AcU -c user.email=s@t -c user.name=s commit -qm old 2>&1 | Out-Null
    Remove-Item Env:GIT_AUTHOR_DATE -ErrorAction SilentlyContinue
    Remove-Item Env:GIT_COMMITTER_DATE -ErrorAction SilentlyContinue
    [IO.File]::AppendAllText((Join-Path $AcU 'f.txt'), "y$acnl", $enc)
    $ac14 = ((& $bashExe $shW health --json -Root (ToPosix $AcU)) -join "`n") | ConvertFrom-Json
    Assert-True ($LASTEXITCODE -eq 1 -and $ac14.alarms[0].kind -eq 'uncommitted' -and $ac14.alarms[0].detail -match 'days ago') `
        'AC14 十天前的提交 + 脏文件 → uncommitted 报警' ''

    # pollution: 三种污染模式，只列不拦
    $AcP = Join-Path $Work 'ac-pollution'
    New-Item -ItemType Directory $AcP -Force | Out-Null
    $proot = ToPosix $AcP
    & $bashExe $shW record policy -Fail -Note 'refused by policy' -Root $proot 2>&1 | Out-Null
    & $bashExe $shW record cmd -Note 'retry with workaround' -Root $proot 2>&1 | Out-Null
    & $bashExe $shW record commit -Note 'git commit --no-verify quick' -Root $proot 2>&1 | Out-Null
    & $bashExe $shW record commit -Note 'changed-lines=912 across 12 files' -Root $proot 2>&1 | Out-Null
    & $bashExe $shW record test -Ok -Root $proot 2>&1 | Out-Null
    $ac16 = @(& $bashExe $shW pollution -Root $proot 2>&1)
    Assert-True ($LASTEXITCODE -eq 1) 'AC16 污染面命中时 exit=1' "exit=$LASTEXITCODE"
    $ac16j = ((& $bashExe $shW pollution --json -Root $proot) -join "`n") | ConvertFrom-Json
    $acKinds = @($ac16j.findings | ForEach-Object { $_.kind })
    Assert-True ((@($acKinds) -contains 'no-verify') -and (@($acKinds) -contains 'refused-then-bypass') `
        -and (@($acKinds) -contains 'giant-diff')) 'AC16b 三种模式都挑出来（--no-verify/被拒后绕行/巨型 diff）' ($acKinds -join ',')
    Assert-True ((@($ac16) -join ' ') -match 'hooks were skipped' -and (@($ac16) -join ' ') -match 'boundary, not an obstacle') `
        'AC17 每条污染带后果说明' ''
    $ac18 = & $bashExe $shW pollution -Root (ToPosix $AcC) 2>&1
    Assert-True ($LASTEXITCODE -eq 0 -and (@($ac18) -join ' ') -match 'division of labor') `
        'AC18 干净根 pollution exit=0，分工声明在输出里（只观测不拦）' "exit=$LASTEXITCODE"

    # AC19: 参数校验 fail-closed，且什么都没落
    $acCount2 = @(Get-Content $acFile).Count
    & $bashExe $shW record cmd -Turn -1 -Root $acroot 2>&1 | Out-Null
    $ac19a = $LASTEXITCODE
    & $bashExe $shW record cmd -Turn abc -Root $acroot 2>&1 | Out-Null
    $ac19b = $LASTEXITCODE
    & $bashExe $shW record cmd -Ok -Fail -Root $acroot 2>&1 | Out-Null
    Assert-True ($ac19a -ne 0 -and $ac19b -ne 0 -and $LASTEXITCODE -ne 0) 'AC19 负轮位/非整数轮位/--ok --fail 冲突都拒' "a=$ac19a b=$ac19b c=$LASTEXITCODE"
    Assert-True (@(Get-Content $acFile).Count -eq $acCount2) 'AC19b 校验拒绝时不落行' ''

    # AC20: 入口互通——PS 转发器写的台账，bash 读得懂
    $AcI = Join-Path $Work 'ac-interop'
    New-Item -ItemType Directory $AcI -Force | Out-Null
    & pwsh -NoProfile -File (Join-Path $tools 'watch.ps1') record test -Ok -Root $AcI 2>&1 | Out-Null
    Assert-True ($LASTEXITCODE -eq 0) 'AC20 pwsh 转发器 record 成功' "exit=$LASTEXITCODE"
    $ac20 = ((& $bashExe $shW report --json -Root (ToPosix $AcI)) -join "`n") | ConvertFrom-Json
    Assert-True ($ac20.events_total -eq 1) 'AC20b PS 写的事件账本 bash 读得懂（互通）' "events=$($ac20.events_total)"

    # AC21: git worktree 形态——.git 是文件不是目录，affected 不得误判 no git history（L105）
    $AcW2 = Join-Path $Work 'ac-worktree'
    New-Item -ItemType Directory "$AcW2\pkg", "$AcW2\tests" -Force | Out-Null
    New-File (Join-Path $AcW2 '.git') "gitdir: ../.git/worktrees/ac$acnl"
    New-File (Join-Path $AcW2 'pkg\core.py') "def f(): return 1$acnl"
    New-File (Join-Path $AcW2 'pkg\__init__.py') "from .core import f$acnl"
    New-File (Join-Path $AcW2 'tests\test_core.py') ("from pkg.core import f" + $acnl + "def test_f(): assert f() == 1" + $acnl)
    $ac21 = @(& $bashExe (Join-Path $tools 'affected.sh') -Path (ToPosix $AcW2) -Changed 'pkg/core.py' -Lang python 2>&1)
    Assert-True ((@($ac21) -join ' ') -match 'proven=True' -and (@($ac21) -join ' ') -notmatch 'no git history') `
        'AC21 worktree 形态（.git 是文件）不误判 no git history，proven 照判' (@($ac21) -join ' / ')

    # AC22: ledger 每行可选 agent_id——追加式，旧行没有该字段照常读
    $shL2 = Join-Path $tools 'ledger.sh'
    $AcLed = Join-Path $Work 'ac-ledger.json'
    & $bashExe $shL2 add -Ledger (ToPosix $AcLed) -Task split -Key 'k|old' -Tokens 100 -Wall 1 2>&1 | Out-Null
    & $bashExe $shL2 add -Ledger (ToPosix $AcLed) -Task split -Key 'k|sub' -Tokens 200 -Wall 2 --agent-id 'sub-7' 2>&1 | Out-Null
    $ac22 = @(([IO.File]::ReadAllText($AcLed, $enc) | ConvertFrom-Json).entries)
    Assert-True (@($ac22).Count -eq 2 -and ($ac22[0].PSObject.Properties.Name -notcontains 'agent_id')) `
        'AC22 旧条目形状不变（无 agent_id 键）' ''
    Assert-True ($ac22[1].agent_id -eq 'sub-7') 'AC22b 新条目带 agent_id（子代理参与记账）' ''
    $ac22rep = @(& $bashExe $shL2 report -Ledger (ToPosix $AcLed) 2>&1)
    Assert-True ((@($ac22rep) -join ' ') -match 'total tokens:\s*300') 'AC22c 旧数据 + 新字段混读，合计照算' (@($ac22rep) -join ' / ')

    # AC23/AC24: agent-verify --agent-id——回执顶层与 run[] 追加 agent_id，旧字段一个不动；
    # 不带 flag 时与现状完全一致
    $AcV = Join-Path $Work 'ac-av'
    New-File (Join-Path $AcV '.git\HEAD') "ref: refs/heads/main`n"
    New-File (Join-Path $AcV 'conftest.py') ''
    New-File (Join-Path $AcV 'pkg\core.py') "def f(): return 1`n"
    New-File (Join-Path $AcV 'pkg\__init__.py') "from .core import f`n"
    New-File (Join-Path $AcV 'tests\test_core.py') "from pkg.core import f`ndef test_f(): assert f() == 1`n"
    $shAv2 = Join-Path $tools 'agent-verify.sh'
    $acRc = Join-Path $Work 'ac-av-rc.json'
    & $bashExe $shAv2 -Root (ToPosix $AcV) -Changed 'pkg/core.py' -Lang python --agent-id 'sub-9' `
        -Receipt (ToPosix $acRc) -Store (Join-Path $Work 'ac-av-store') 2>&1 | Out-Null
    Assert-True ($LASTEXITCODE -eq 0) 'AC23 带 --agent-id 全链路 exit=0' "exit=$LASTEXITCODE"
    $ac23 = Get-Content $acRc -Raw | ConvertFrom-Json
    $ac23core = (@($ac23.steps | ForEach-Object { $_.step }) |
        Where-Object { $_ -ne 'memory-brief' -and $_ -ne 'memory-note' }) -join ','
    Assert-True ($ac23.agent_id -eq 'sub-9' -and $ac23.run[0].agent_id -eq 'sub-9') `
        'AC23b 回执顶层与 run[0] 都带 agent_id' ''
    Assert-True ($ac23core -eq 'timeline-due,preflight,checks,affected,policy,constraints,journal-begin,run,journal-end') `
        'AC23c 步骤序列与 I2b 完全一致（只追加不改旧）' $ac23core

    $acRc2 = Join-Path $Work 'ac-av-rc2.json'
    & $bashExe $shAv2 -Root (ToPosix $AcV) -Changed 'pkg/core.py' -Lang python `
        -Receipt (ToPosix $acRc2) -Store (Join-Path $Work 'ac-av-store') 2>&1 | Out-Null
    $ac24 = Get-Content $acRc2 -Raw | ConvertFrom-Json
    $ac24runIds = @($ac24.run | ForEach-Object { $_.PSObject.Properties.Name -contains 'agent_id' })
    Assert-True ($LASTEXITCODE -eq 0 -and ($ac24.PSObject.Properties.Name -notcontains 'agent_id') `
        -and ($ac24runIds -notcontains $true)) 'AC24 不带 flag 时回执没有任何 agent_id 键（回归保护）' "exit=$LASTEXITCODE"
    Assert-True ($ac24.verdict -match 'proven subset') 'AC24b verdict 与 I4 同口径' $ac24.verdict

    & $bashExe $shAv2 -Root (ToPosix $AcV) -Changed 'pkg/core.py' -Lang python --agent-id 'D:\elsewhere\x' `
        -Receipt (ToPosix (Join-Path $Work 'ac-av-rc3.json')) 2>&1 | Out-Null
    Assert-True ($LASTEXITCODE -eq 2 -and -not (Test-Path (Join-Path $Work 'ac-av-rc3.json'))) `
        'AC25 agent-id 带盘符路径 exit=2 拒收且不落回执' "exit=$LASTEXITCODE"
}

""
"== AD. 主库删除审批 / 污染 WARN 族 / pitfalls 痛点语料 =="
if (-not $bashExe) {
    "  SKIP  AD0 没有 bash，跳过 AD 组检查"
    $skipped += 24
} else {
    $shPolAD = Join-Path $tools 'policy.sh'
    $shConAD = Join-Path $tools 'constraints.sh'
    $shTrAD = Join-Path $tools 'trash.sh'
    $shPfAD = Join-Path $tools 'pitfalls.sh'

    # 夹具：主检出与 worktree 两种形态。判定不写死路径——git 在主检出的根留 .git 目录、
    # 在链接 worktree 的根留 .git 文件。判错方向（把主库当 worktree）会让主库删除绕过审批，
    # 所以两形态本身先自证（AD0）。
    $ADm = Join-Path $Work 'ad-main'
    New-Item -ItemType Directory "$ADm\src" -Force | Out-Null
    [IO.File]::WriteAllText("$ADm\src\a.txt", "alpha`n", $enc)
    [IO.File]::WriteAllText("$ADm\.gitignore", ".journal/`n.agent/`n", $enc)
    & git -C $ADm init -q 2>&1 | Out-Null
    & git -C $ADm add -A 2>&1 | Out-Null
    & git -C $ADm -c user.email=s@t -c user.name=s commit -qm base 2>&1 | Out-Null
    $ADw = Join-Path $Work 'ad-wt'
    & git -C $ADm worktree add $ADw -b ad-wt 2>&1 | Out-Null
    # 形态判定同样走 .NET 存在性检查（.git 有 Windows Hidden 属性，Get-Item 看不见）
    Assert-True ([IO.Directory]::Exists("$ADm\.git") -and [IO.File]::Exists("$ADw\.git")) `
        'AD0 夹具两形态自证：主检出 .git=目录、worktree .git=文件（判错方向的锚）' ''

    # AD1/AD2: 主检出上 DELETION 家族 → DENY，拒绝文案本身就是审批协议
    & $bashExe $shPolAD -Profile workspace-write -Root (ToPosix $ADm) -Command 'rm src/a.txt' 2>&1 | Out-Null
    $ad1 = $LASTEXITCODE
    $ad2 = @(& $bashExe $shPolAD -Profile workspace-write -Root (ToPosix $ADm) -Command 'rm src/a.txt' 2>&1)
    Assert-True ($ad1 -eq 1) 'AD1 主检出（.git 目录形态）上裸删 DENY' "exit=$ad1"
    Assert-True ((@($ad2) -join ' ') -match 'main tree delete needs human approval') `
        'AD2 主库拒绝文案带审批语义（-Allow main-delete --approval-note）' (@($ad2) -join ' / ')

    # AD3/AD4: 没有说明的审批不算审批；审批+说明逐次放行且大声打印
    & $bashExe $shPolAD -Profile workspace-write -Root (ToPosix $ADm) -Command 'rm src/a.txt' -Allow main-delete 2>&1 | Out-Null
    $ad3 = $LASTEXITCODE
    Assert-True ($ad3 -eq 1) 'AD3 -Allow main-delete 缺 --approval-note 仍拒（没有说明不算审批）' "exit=$ad3"
    $ad4 = @(& $bashExe $shPolAD -Profile workspace-write -Root (ToPosix $ADm) -Command 'rm src/a.txt' `
        -Allow main-delete -ApprovalNote 'human ok: one-off cleanup of a stray file' 2>&1)
    Assert-True ($LASTEXITCODE -eq 0 -and ((@($ad4) -join ' ') -match 'per-invocation grant') `
        -and ((@($ad4) -join ' ') -match 'human ok')) `
        'AD4 审批+说明放行且大声打印（逐次放行，note 原文留痕）' (@($ad4) -join ' / ')

    # AD5/AD6: worktree（.git 文件形态）维持现状——分支不怎么管（回归）
    $ad5 = @(& $bashExe $shPolAD -Profile workspace-write -Root (ToPosix $ADw) -Command 'rm src/a.txt' 2>&1)
    Assert-True ($LASTEXITCODE -eq 1 -and ((@($ad5) -join ' ') -notmatch 'main tree delete')) `
        'AD5 worktree 裸删照旧 DENY、文案不带主库审批语义（分支不受影响回归）' (@($ad5) -join ' / ')
    & $bashExe $shPolAD -Profile workspace-write -Root (ToPosix $ADw) -Command 'trash put src/a.txt -Reason r' 2>&1 | Out-Null
    Assert-True ($LASTEXITCODE -eq 0) 'AD6 worktree 上 trash 放行回归（分支只要求回收站）' "exit=$LASTEXITCODE"

    # AD7: diff 侧整删已跟踪文件在主检出 → BLOCK main-tree-raw-delete（要人批）
    $ADc = Join-Path $Work 'ad-con-main'
    New-Item -ItemType Directory "$ADc\src" -Force | Out-Null
    [IO.File]::WriteAllText("$ADc\src\a.txt", "tracked`n", $enc)
    [IO.File]::WriteAllText("$ADc\.gitignore", ".journal/`n.agent/`n", $enc)
    & git -C $ADc init -q 2>&1 | Out-Null
    & git -C $ADc add -A 2>&1 | Out-Null
    & git -C $ADc -c user.email=s@t -c user.name=s commit -qm base 2>&1 | Out-Null
    & $bashExe $shConAD init -Root (ToPosix $ADc) 2>&1 | Out-Null
    Remove-Item "$ADc\src\a.txt"
    $ad7 = @(& $bashExe $shConAD check -Root (ToPosix $ADc) 2>&1)
    Assert-True ($LASTEXITCODE -eq 1 -and ((@($ad7) -join ' ') -match 'BLOCK \[main-tree-raw-delete\]')) `
        'AD7 diff 侧整删已跟踪文件在主检出 BLOCK（main-tree-raw-delete，要人批）' (@($ad7) -join ' / ')

    # AD8: worktree 上同样的整删仍走 no-raw-delete 原规则（回归）
    $ADcm = Join-Path $Work 'ad-con-wt-main'
    New-Item -ItemType Directory "$ADcm\src" -Force | Out-Null
    [IO.File]::WriteAllText("$ADcm\src\a.txt", "tracked`n", $enc)
    [IO.File]::WriteAllText("$ADcm\.gitignore", ".journal/`n.agent/`n", $enc)
    & git -C $ADcm init -q 2>&1 | Out-Null
    & git -C $ADcm add -A 2>&1 | Out-Null
    & git -C $ADcm -c user.email=s@t -c user.name=s commit -qm base 2>&1 | Out-Null
    $ADwt = Join-Path $Work 'ad-con-wt'
    & git -C $ADcm worktree add $ADwt -b ad-con-wt 2>&1 | Out-Null
    & $bashExe $shConAD init -Root (ToPosix $ADwt) 2>&1 | Out-Null
    Remove-Item "$ADwt\src\a.txt"
    $ad8 = @(& $bashExe $shConAD check -Root (ToPosix $ADwt) 2>&1)
    Assert-True ($LASTEXITCODE -eq 1 -and ((@($ad8) -join ' ') -match 'BLOCK \[no-raw-delete\]') `
        -and ((@($ad8) -join ' ') -notmatch 'main-tree-raw-delete')) `
        'AD8 worktree 整删仍走 no-raw-delete 原规则（主库闸不在此点火，回归）' (@($ad8) -join ' / ')

    # AD9/AD10: constraints 侧的审批同样要求说明；flag 缺 note 不算放行
    & $bashExe $shConAD check -Root (ToPosix $ADc) -Allow main-tree-raw-delete 2>&1 | Out-Null
    $ad9 = $LASTEXITCODE
    Assert-True ($ad9 -eq 1) 'AD9 constraints -Allow 缺 -ApprovalNote 仍 BLOCK（没有说明不算审批）' "exit=$ad9"
    & $bashExe $shConAD check -Root (ToPosix $ADc) -Allow main-tree-raw-delete -ApprovalNote 'human ok: retire dead module' 2>&1 | Out-Null
    Assert-True ($LASTEXITCODE -eq 0) 'AD10 constraints 审批+说明放行（exit 0）' "exit=$LASTEXITCODE"

    # AD11: 主库上回收站有账也不放行——bin 留内容，不代购审批
    $ADc2 = Join-Path $Work 'ad-con-main2'
    New-Item -ItemType Directory "$ADc2\src" -Force | Out-Null
    [IO.File]::WriteAllText("$ADc2\src\b.txt", "beta`n", $enc)
    [IO.File]::WriteAllText("$ADc2\.gitignore", ".journal/`n.agent/`n", $enc)
    & git -C $ADc2 init -q 2>&1 | Out-Null
    & git -C $ADc2 add -A 2>&1 | Out-Null
    & git -C $ADc2 -c user.email=s@t -c user.name=s commit -qm base 2>&1 | Out-Null
    & $bashExe $shConAD init -Root (ToPosix $ADc2) 2>&1 | Out-Null
    & $bashExe $shTrAD put (ToPosix "$ADc2\src\b.txt") -Reason ad-test -Root (ToPosix $ADc2) 2>&1 | Out-Null
    $ad11put = $LASTEXITCODE
    $ad11 = @(& $bashExe $shConAD check -Root (ToPosix $ADc2) 2>&1)
    Assert-True ($ad11put -eq 0 -and -not (Test-Path "$ADc2\src\b.txt") -and $LASTEXITCODE -eq 1 `
        -and ((@($ad11) -join ' ') -match 'BLOCK \[main-tree-raw-delete\]')) `
        'AD11 主库上回收站有账仍 BLOCK（bin 保内容，不抵扣审批）' "put=$ad11put out=$(@($ad11) -join ' ')"

    # AD12-AD15: 污染 WARN 族各触发一次，全部只提醒不拦截（exit 0）
    $ad12 = @(& $bashExe $shPolAD -Profile workspace-write -Root (ToPosix $ADm) -Command 'git commit --no-verify -m x' 2>&1)
    Assert-True ($LASTEXITCODE -eq 0 -and ((@($ad12) -join ' ') -match 'pollution\[no-verify\]')) `
        'AD12 WARN commit --no-verify：绕门禁进坏基线的后果提醒且不拦截' (@($ad12) -join ' / ')
    $ad13 = @(& $bashExe $shPolAD -Profile workspace-write -Root (ToPosix $ADm) -Command 'git add -A' 2>&1)
    Assert-True ($LASTEXITCODE -eq 0 -and ((@($ad13) -join ' ') -match 'pollution\[add-all\]')) `
        'AD13 WARN git add -A：无关改动混一提交的后果提醒且不拦截' (@($ad13) -join ' / ')
    $ADbig = Join-Path $Work 'ad-bigdiff'
    New-Item -ItemType Directory "$ADbig\src" -Force | Out-Null
    & git -C $ADbig init -q 2>&1 | Out-Null
    for ($i = 0; $i -lt 35; $i++) { [IO.File]::WriteAllText("$ADbig\src\f$i.txt", "v1`n", $enc) }
    & git -C $ADbig add -A 2>&1 | Out-Null
    & git -C $ADbig -c user.email=s@t -c user.name=s commit -qm base 2>&1 | Out-Null
    for ($i = 0; $i -lt 35; $i++) { [IO.File]::WriteAllText("$ADbig\src\f$i.txt", "v2 changed`nmore`nhere`n", $enc) }
    $ad14 = @(& $bashExe $shPolAD -Profile workspace-write -Root (ToPosix $ADbig) -Command 'git commit -m bulk' 2>&1)
    Assert-True ($LASTEXITCODE -eq 0 -and ((@($ad14) -join ' ') -match 'pollution\[wide-diff\]')) `
        'AD14 WARN 巨型混合 diff（35 文件 > 阈值 30）：review 与二分失效提醒且不拦截' (@($ad14) -join ' / ')
    $ad15 = @(& $bashExe $shPolAD -Profile workspace-write -Root (ToPosix $ADm) -Command 'git add .env' 2>&1)
    Assert-True ($LASTEXITCODE -eq 0 -and ((@($ad15) -join ' ') -match 'pollution\[secret-path\]')) `
        'AD15 WARN 密钥形态文件（.env）：凭据进公开树的提醒且不拦截' (@($ad15) -join ' / ')

    # AD16-AD20: pitfalls 痛点语料库
    $ADp = Join-Path $Work 'ad-pit'
    New-Item -ItemType Directory $ADp -Force | Out-Null
    $pfRoot = ToPosix $ADp
    & $bashExe $shPfAD add --cat git --trigger 'staged-only file lost by git reset --hard' `
        --consequence 'work is gone without a trace' --prevention 'stash or segment the commit chain' `
        --source 'docs/ERRORS.md L15' -Root $pfRoot 2>&1 | Out-Null
    Assert-True ($LASTEXITCODE -eq 0) 'AD16a pitfalls add exit=0' "exit=$LASTEXITCODE"
    & $bashExe $shPfAD add --cat nosuch --trigger t --consequence c --prevention p -Root $pfRoot 2>&1 | Out-Null
    Assert-True ($LASTEXITCODE -eq 2) 'AD16b 未知 cat 拒绝（exit 2）' "exit=$LASTEXITCODE"
    $pfList = @(& $bashExe $shPfAD list -Root $pfRoot 2>&1)
    Assert-True ((@($pfList) -join ' ') -match 'staged-only file lost') `
        'AD16c list 读回 trigger（ASCII 转义渲染，不丢内容）' (@($pfList) -join ' / ')

    & $bashExe $shPfAD seed -Root $pfRoot 2>&1 | Out-Null
    $ad17a = $LASTEXITCODE
    & $bashExe $shPfAD seed -Root $pfRoot 2>&1 | Out-Null
    $ad17b = $LASTEXITCODE
    $pfCount = (@(& $bashExe $shPfAD count -Root $pfRoot 2>&1) -join ' ')
    $n17 = if ($pfCount -match 'count=(\d+)') { [int]$Matches[1] } else { -1 }
    Assert-True ($ad17a -eq 0 -and $ad17b -eq 0 -and $n17 -ge 8) `
        'AD17 seed 两次幂等且 ≥8 条（种子提炼自真实台账）' "rc=$ad17a/$ad17b count=$n17"

    $pfExp = Join-Path $Work 'ad-pit-export.jsonl'
    & $bashExe $shPfAD export --jsonl (ToPosix $pfExp) -Root $pfRoot 2>&1 | Out-Null
    $ad18 = $LASTEXITCODE
    $expLines = @([IO.File]::ReadAllLines($pfExp, $enc) | Where-Object { $_.Trim() })
    Assert-True ($ad18 -eq 0 -and $expLines.Count -eq $n17) `
        'AD18 export --jsonl 行数 == count（ML 训练就绪格式）' "lines=$($expLines.Count) count=$n17"
    $expRaw = [IO.File]::ReadAllText($pfExp, $enc)
    $ghN = @([regex]::Matches($expRaw, '"cat":\s*"github"')).Count
    Assert-True ($ghN -ge 2) 'AD19 种子含 ≥2 条 github 域痛点（不只 git）' "github=$ghN"

    $pfCountBefore = $n17
    # 伪造的用户路径在运行时拼出来：源码里不出现完整的"盘符+Users+反斜杠"形态，
    # 免得 privacy_scan 的硬规则打红这条夹具本身（路径是假的，闸是真的）
    $upFake = ('C:' + '\' + 'Users' + '\' + 'somebody' + '\' + 'box' + '\todo.txt')
    & $bashExe $shPfAD add --cat git --trigger "see notes at $upFake" `
        --consequence c --prevention p -Root $pfRoot 2>&1 | Out-Null
    $ad20 = $LASTEXITCODE
    $pfCount2 = (@(& $bashExe $shPfAD count -Root $pfRoot 2>&1) -join ' ')
    Assert-True ($ad20 -eq 1 -and $pfCount2 -match "count=$pfCountBefore") `
        'AD20 带本机用户路径的条目拒收且不落盘（隐私门，语料要进公开树）' "rc=$ad20 $pfCount2"

    # AD21: 内核规则不许从表里悄悄消失——旧表缺 no-raw-delete 时 check 现场补回并大声说明
    $ADt = Join-Path $ADwt '.agent\constraints.json'
    [IO.File]::WriteAllText($ADt, '[{"id":"no-new-unwrap","severity":"warn","scope":["**"],"on":"added","pattern":"unwrap","message":"x"}]' + "`n", $enc)
    $ad21 = @(& $bashExe $shConAD check -Root (ToPosix $ADwt) 2>&1)
    $ad21j = (@($ad21) -join ' ')
    Assert-True ($LASTEXITCODE -eq 1 -and $ad21j -match 'restored for this run' `
        -and $ad21j -match 'no-raw-delete' -and $ad21j -match 'BLOCK \[no-raw-delete\]') `
        'AD21 旧表缺内核规则时 check 现场补回并判红（约束不许静默变窄）' (@($ad21) -join ' / ')
}

""
"== AE. cognition/collect: 独立训练层收集器（语料契约见 cognition/SCHEMA.md）=="
# 独立层的硬约束在这里钉住：stdlib 直跑（不经 tools/ 任何入口）、corpus 永远无原文
# （没有 --with-text 后门）、knowledge 是可入库的公开快照（幂等 + 用户路径隐私闸）、
# 全部 store 缺失时骨架可收集。夹具全在临时目录；AE17 故意用默认输出写进仓库的
# cognition/corpus/，验证 gitignore 后 git status 无感，跑完即清理。
$aePy = $null
foreach ($c in @($env:PYTHON, 'python', 'python3', 'py')) {
    if (-not $c) { continue }
    $aeSrc = (Get-Command $c -ErrorAction SilentlyContinue).Source
    if (-not $aeSrc) { continue }
    if ((& $aeSrc -c 'print(1)' 2>$null) -eq '1') { $aePy = $aeSrc; break }
}
if (-not $aePy) {
    "  SKIP  AE0 找不到可用的 Python，跳过 cognition 收集器检查"
    $skipped += 32
} else {
    $col = Join-Path $repo 'cognition\collect.py'
    $aenl = [char]10
    $Ae = Join-Path $Work 'ae-cognition'
    New-Item -ItemType Directory $Ae -Force | Out-Null

    # AE1-AE3: 全部 store 缺失 = 骨架可收集（没有数据不是错误），且不写空文件冒充数据
    $Ae0 = Join-Path $Ae 'empty-root'
    New-Item -ItemType Directory $Ae0 -Force | Out-Null
    $AeOut0 = Join-Path $Ae 'out-empty'
    $ae1 = @(& $aePy $col --root $Ae0 --out $AeOut0 2>&1)
    Assert-True ($LASTEXITCODE -eq 0) 'AE1 全部 store 缺失时骨架可收集 exit=0' "exit=$LASTEXITCODE"
    Assert-True ((@($ae1) -join ' ') -match 'collect: corpus=0 events knowledge=0 pitfalls \(skipped: signals absent, watch absent, pitfalls absent\)') `
        'AE2 摘要点名三家缺席（跳过是明说的，不是静默）' (@($ae1) -join ' ')
    Assert-True (-not (Test-Path (Join-Path $AeOut0 'corpus\events.jsonl')) -and -not (Test-Path (Join-Path $AeOut0 'knowledge'))) `
        'AE3 缺席的源不写 0 行文件冒充数据' ''

    # AE4-AE9: corpus 投影 = signal export 形状 + watch 行加 event/verdict/agent_id/turn；无原文
    $AeB = Join-Path $Ae 'owner-root'
    New-Item -ItemType Directory (Join-Path $AeB '.agent') -Force | Out-Null
    [IO.File]::WriteAllText((Join-Path $AeB '.agent\signals.jsonl'), (@(
            '{"ts": "2026-11-21T09:00:00+08:00", "kind": "task", "emotion_score": 0, "signals": ["imperative"], "task_hints": ["fix"], "text_len": 20, "text": "fix the thing please"}',
            '{"ts": "2026-11-21T09:05:00+08:00", "kind": "penalty", "score": 2, "reason": "argued with user intent"}'
        ) -join $aenl) + $aenl, $enc)
    [IO.File]::WriteAllText((Join-Path $AeB '.agent\watch.jsonl'), (@(
            '{"ts": "2026-11-21T09:10:00+08:00", "event": "cmd", "agent_id": "sub-1", "turn": 1, "verdict": "ok", "note": "first"}',
            '{"ts": "2026-11-21T09:11:00+08:00", "event": "test", "agent_id": "", "turn": null, "verdict": "fail", "note": ""}'
        ) -join $aenl) + $aenl, $enc)
    $AeOutB = Join-Path $Ae 'out-owner'
    $ae4 = @(& $aePy $col --root $AeB --out $AeOutB 2>&1)
    Assert-True ($LASTEXITCODE -eq 0 -and ((@($ae4) -join ' ') -match 'collect: corpus=4 events knowledge=0 pitfalls \(skipped: pitfalls absent\)')) `
        'AE4 signals+watch 合并出 4 行、pitfalls 缺席被点名' (@($ae4) -join ' ')
    $aeCorpus = Join-Path $AeOutB 'corpus\events.jsonl'
    $aeDocs = @(([IO.File]::ReadAllLines($aeCorpus, $enc) | Where-Object { $_.Trim() }) | ForEach-Object { $_ | ConvertFrom-Json })
    Assert-True ($aeDocs.Count -eq 4) 'AE5 corpus 行数=4（signals 2 + watch 2，账本序）' "rows=$($aeDocs.Count)"
    $aeKeys = @($aeDocs | ForEach-Object { (@($_.PSObject.Properties.Name | Sort-Object) -join ',') })
    Assert-True ($aeKeys[0] -eq 'emotion_score,kind,penalty,signals,text_len,ts') 'AE6 signals 行字段与 SCHEMA 完全一致（export 投影六字段）' $aeKeys[0]
    Assert-True ($aeKeys[1] -eq $aeKeys[0]) 'AE6b penalty 行同形（同一份投影契约）' $aeKeys[1]
    Assert-True ($aeDocs[1].penalty -eq 2 -and $aeDocs[1].emotion_score -eq 2 -and $aeDocs[1].text_len -eq 23) `
        'AE6c penalty 行把 score 双写进 penalty/emotion_score，reason 只留长度' ''
    Assert-True ($aeDocs[0].text_len -eq 20) 'AE6d classify 行 text_len 原样来自台账（collect 不重算）' ''
    Assert-True ($aeKeys[2] -eq 'agent_id,emotion_score,event,kind,penalty,signals,text_len,ts,turn,verdict') `
        'AE7 watch 行在六字段之外加 event/verdict/agent_id/turn' $aeKeys[2]
    Assert-True ($aeDocs[2].kind -eq 'watch' -and $aeDocs[2].agent_id -eq 'sub-1' -and $aeDocs[2].turn -eq 1 -and $aeDocs[2].verdict -eq 'ok') `
        'AE7b watch 行字段值逐项正确' ''
    Assert-True ($null -eq $aeDocs[3].turn -and $aeDocs[3].verdict -eq 'fail') 'AE7c 无轮位 watch 行 turn=null 照写（不是丢字段）' ''
    $aeRaw = [IO.File]::ReadAllText($aeCorpus, $enc)
    Assert-True ($aeRaw -notmatch '"text"|"task_hints"|"reason"|"note"' `
        -and $aeRaw -notmatch 'fix the thing|argued with user intent|first') `
        'AE8 corpus 无原文（无 text/task_hints/reason/note 键，也无原文片段）' ''
    Assert-True ((($aeRaw -replace '[^\x00-\x7F]', '').Length) -eq $aeRaw.Length) 'AE8b corpus 纯 ASCII（任何宿主都读不坏）' ''
    Assert-True (-not (Test-Path (Join-Path $AeOutB 'knowledge\pitfalls.jsonl'))) 'AE9 pitfalls 缺席时 knowledge 不写（快照不冒充数据）' ''

    # AE10-AE11: knowledge 快照（公开知识、入库）与幂等
    $AeC = Join-Path $Ae 'pit-root'
    New-Item -ItemType Directory (Join-Path $AeC '.agent') -Force | Out-Null
    [IO.File]::WriteAllText((Join-Path $AeC '.agent\pitfalls.jsonl'), (@(
            '{"id": "pfix-001", "cat": "git", "trigger": "staged-only file lost", "consequence": "work gone", "prevention": "stash first", "source": "docs/ERRORS.md L15", "ts": "2026-11-21T00:00:00+08:00"}',
            '{"id": "pfix-002", "cat": "api", "trigger": "win32 常量凭记忆写", "consequence": "错误指错层", "prevention": "全零 buffer 扫描", "source": "", "ts": "2026-11-21T00:00:00+08:00"}'
        ) -join $aenl) + $aenl, $enc)
    $AeOutC = Join-Path $Ae 'out-pit'
    & $aePy $col --root $AeC --out $AeOutC 2>&1 | Out-Null
    Assert-True ($LASTEXITCODE -eq 0) 'AE10 knowledge 快照落盘 exit=0' "exit=$LASTEXITCODE"
    $aeK = Join-Path $AeOutC 'knowledge\pitfalls.jsonl'
    $aeKDocs = @(([IO.File]::ReadAllLines($aeK, $enc) | Where-Object { $_.Trim() }) | ForEach-Object { $_ | ConvertFrom-Json })
    Assert-True ($aeKDocs.Count -eq 2 -and $aeKDocs[0].id -eq 'pfix-001' -and $aeKDocs[1].cat -eq 'api') 'AE10b 快照逐行保真（id/cat 原样）' ''
    Assert-True (([IO.File]::ReadAllText($aeK, $enc)) -match '凭记忆写') 'AE10c 非 ASCII 内容按 UTF-8 原样保留（不转义不丢字）' ''
    $aeKBefore = [IO.File]::ReadAllBytes($aeK)
    & $aePy $col --root $AeC --out $AeOutC 2>&1 | Out-Null
    $aeKAfter = [IO.File]::ReadAllBytes($aeK)
    Assert-True ($LASTEXITCODE -eq 0 -and (Compare-Object $aeKBefore $aeKAfter).Count -eq 0) `
        'AE11 重跑幂等：覆盖写且字节一致' "exit=$LASTEXITCODE"

    # AE12/AE13: 坏行 fail-closed 拒收并指出行号（半读的语料教一半的真相）
    $AeD = Join-Path $Ae 'bad-sig'
    New-Item -ItemType Directory (Join-Path $AeD '.agent') -Force | Out-Null
    [IO.File]::WriteAllText((Join-Path $AeD '.agent\signals.jsonl'), (@(
            '{"ts": "2026-11-21T09:00:00+08:00", "kind": "task", "emotion_score": 0, "signals": [], "task_hints": [], "text_len": 1, "text": "x"}',
            'not json at all'
        ) -join $aenl) + $aenl, $enc)
    $ae12 = @(& $aePy $col --root $AeD --out (Join-Path $Ae 'out-bad') 2>&1)
    Assert-True ($LASTEXITCODE -eq 1 -and ((@($ae12) -join ' ') -match 'signals line 2')) `
        'AE12 signals 坏行拒收且带行号' (@($ae12) -join ' ')
    $AeE = Join-Path $Ae 'bad-watch'
    New-Item -ItemType Directory (Join-Path $AeE '.agent') -Force | Out-Null
    [IO.File]::WriteAllText((Join-Path $AeE '.agent\watch.jsonl'), (@(
            '{"ts": "2026-11-21T09:00:00+08:00", "event": "cmd", "agent_id": "a", "turn": "1", "verdict": "ok", "note": ""}'
        ) -join $aenl) + $aenl, $enc)
    $ae13 = @(& $aePy $col --root $AeE --out (Join-Path $Ae 'out-bad2') 2>&1)
    Assert-True ($LASTEXITCODE -eq 1 -and ((@($ae13) -join ' ') -match 'watch line 1')) `
        'AE13 watch 形状漂移拒收且带行号（turn 必须是 null 或整数）' (@($ae13) -join ' ')

    # AE14/AE15: knowledge 是入库面——用户路径与坏条目都在快照门口拦。
    # 夹具输入运行期拼接（privacy_scan 不打红本文件）；反斜杠按 JSON 编码双写——
    # 磁盘上的真实泄漏行永远是合法 JSON，单反斜杠会让 json.loads 先死、断言以错误的
    # 理由变绿（见 L110）。
    $AeF = Join-Path $Ae 'pit-leak'
    New-Item -ItemType Directory (Join-Path $AeF '.agent') -Force | Out-Null
    $upFake = ('C:' + '\' + 'Users' + '\' + 'somebody' + '\' + 'notes.txt')
    $upFakeJson = $upFake.Replace('\', '\\')
    [IO.File]::WriteAllText((Join-Path $AeF '.agent\pitfalls.jsonl'), (@(
            '{"id": "pok-001", "cat": "git", "trigger": "t", "consequence": "c", "prevention": "p", "source": "s", "ts": "2026-11-21T00:00:00+08:00"}',
            ('{"id": "pleak-001", "cat": "git", "trigger": "see notes at ' + $upFakeJson + '", "consequence": "c", "prevention": "p", "source": "s", "ts": "2026-11-21T00:00:00+08:00"}')
        ) -join $aenl) + $aenl, $enc)
    $ae14 = @(& $aePy $col --root $AeF --out (Join-Path $Ae 'out-leak') 2>&1)
    Assert-True ($LASTEXITCODE -eq 1 -and ((@($ae14) -join ' ') -match 'pitfalls line 2') -and ((@($ae14) -join ' ') -match 'user path')) `
        'AE14 用户路径进公开知识被拦（拒收带行号，且理由是隐私而不是坏 JSON）' (@($ae14) -join ' ')
    Assert-True (-not (Test-Path (Join-Path $Ae 'out-leak\knowledge\pitfalls.jsonl'))) 'AE14b 拒收时不落快照（fail-closed，先于任何写盘）' ''
    [IO.File]::WriteAllText((Join-Path $AeF '.agent\pitfalls.jsonl'), (@(
            '{"id": "pok-001", "cat": "git", "trigger": "t", "consequence": "c", "prevention": "p", "source": "s", "ts": "2026-11-21T00:00:00+08:00"}',
            '{"id": "", "cat": "git"}'
        ) -join $aenl) + $aenl, $enc)
    $ae15 = @(& $aePy $col --root $AeF --out (Join-Path $Ae 'out-mal') 2>&1)
    Assert-True ($LASTEXITCODE -eq 1 -and ((@($ae15) -join ' ') -match 'pitfalls line 2')) `
        'AE15 坏条目（缺 trigger 等）拒收带行号' (@($ae15) -join ' ')

    # AE16: 没有 --with-text 后门（原文要用户自己手动交付，工具不代劳）
    & $aePy $col --root $AeB --out (Join-Path $Ae 'out-wt') --with-text 2>&1 | Out-Null
    Assert-True ($LASTEXITCODE -eq 2) 'AE16 --with-text 不是本工具的旗标（exit 2，原文不走工具）' "exit=$LASTEXITCODE"

    # AE17: corpus 落点被 gitignore——默认输出写进仓库 cognition/corpus/，git status 无感
    $stBefore = @(& git -C $repo status --porcelain -- cognition/)
    & $aePy $col --root $AeB 2>&1 | Out-Null
    $ae17code = $LASTEXITCODE
    $stAfter = @(& git -C $repo status --porcelain -- cognition/)
    Assert-True ($ae17code -eq 0 -and (Test-Path (Join-Path $repo 'cognition\corpus\events.jsonl'))) `
        'AE17 默认输出落在 cognition/corpus/（跟文件夹走，无硬编码路径）' "exit=$ae17code"
    & git -C $repo check-ignore -q cognition/corpus/events.jsonl 2>&1 | Out-Null
    Assert-True ($LASTEXITCODE -eq 0) 'AE17b corpus 被 gitignore 命中（克隆不带私有语料）' "check-ignore=$LASTEXITCODE"
    Assert-True ((@($stBefore) -join "`n") -eq (@($stAfter) -join "`n")) `
        'AE17c collect 产出后 git status（限 cognition/）零变化（语料不进 status）' ''
    Remove-Item (Join-Path $repo 'cognition\corpus') -Recurse -Force -ErrorAction SilentlyContinue

    # AE18: 调用面 fail-closed
    & $aePy $col 2>&1 | Out-Null
    Assert-True ($LASTEXITCODE -eq 2) 'AE18 缺 --root exit=2' "exit=$LASTEXITCODE"
    & $aePy $col --root (Join-Path $Ae 'no-such-root') 2>&1 | Out-Null
    Assert-True ($LASTEXITCODE -eq 2) 'AE18b --root 不是存在的目录 exit=2（不猜 cwd）' "exit=$LASTEXITCODE"
    & $aePy $col --root $AeB --wat x 2>&1 | Out-Null
    Assert-True ($LASTEXITCODE -eq 2) 'AE18c 未知旗标拒绝' "exit=$LASTEXITCODE"

    # AE19: corpus 也是快照语义——重跑覆盖、行数一致
    & $aePy $col --root $AeB --out $AeOutB 2>&1 | Out-Null
    Assert-True ($LASTEXITCODE -eq 0 -and @(([IO.File]::ReadAllLines((Join-Path $AeOutB 'corpus\events.jsonl'), $enc) | Where-Object { $_.Trim() })).Count -eq 4) `
        'AE19 corpus 重跑覆盖且行数一致' "exit=$LASTEXITCODE"
}

# =====================================================================================
"== AF. 编排链稳定性：并发锁、崩溃恢复、畸形存储、超大计划、compact/并行一致性 =="
# 每条断言都对着一个真实失效模式：并发链竞争 journal-begin、被杀进程留下的脏状态、
# 半截存储被读成空账、500 条命令的计划、同一 run 两种回执/两种门序必须可比。
$afPy = $null
foreach ($c in @($env:PYTHON, 'python', 'python3', 'py')) {
    if (-not $c) { continue }
    $afSrc = (Get-Command $c -ErrorAction SilentlyContinue).Source
    if (-not $afSrc) { continue }
    if ((& $afSrc -c 'print(1)' 2>$null) -eq '1') { $afPy = $afSrc; break }
}
if (-not $afPy) {
    "  SKIP  AF0 找不到可用的 Python，跳过编排链稳定性检查"
    $skipped += 38
} else {
    $afEnc = New-Object System.Text.UTF8Encoding($false)
    $avPy = Join-Path $tools 'agent_verify.py'
    $jrPy = Join-Path $tools 'journal.py'
    $ldPy = Join-Path $tools 'ledger.py'
    $rsPy = Join-Path $tools 'resume.py'

    function New-AvFixture([string]$root) {
        New-Item -ItemType Directory -Force -Path (Join-Path $root 'pkg'), (Join-Path $root 'tests') | Out-Null
        New-File (Join-Path $root 'pkg\core.py') "def f(): return 1$nl"
        New-File (Join-Path $root 'pkg\__init__.py') ''
        New-File (Join-Path $root 'tests\test_core.py') "from pkg.core import f${nl}def test_f(): assert f() == 1$nl"
        New-File (Join-Path $root 'conftest.py') ''
        Push-Location $root
        & git init -q 2>&1 | Out-Null
        & git add -A 2>&1 | Out-Null
        & git -c user.email=a@b -c user.name=t commit -qm init 2>&1 | Out-Null
        Pop-Location
    }
    function Invoke-Av([string]$root, [string]$store, [string]$receipt, [string[]]$extra,
                       [string]$lang = 'python', [string]$changed = 'pkg\core.py') {
        $plan = if (Test-Path (Join-Path $root 'plan.json')) { Join-Path $root 'plan.json' } else { '' }
        $argsAv = @($avPy, '-Root', $root, '-Changed', (Join-Path $root $changed), '-Lang', $lang)
        if ($plan) { $argsAv += @('-Plan', $plan) }
        $argsAv += @('-Store', $store, '-Receipt', $receipt) + $extra
        & $afPy @argsAv 2>&1
    }
    function Read-Receipt([string]$path) {
        return [IO.File]::ReadAllText($path, $afEnc) | ConvertFrom-Json
    }

    # --- AF1: 同 root 并发两条链，第二条 fail-closed（行为钉死：不竞争 journal-begin）---
    $AfA = Join-Path $Work 'af-conc'
    New-AvFixture $AfA
    New-File (Join-Path $AfA 'plan.json') '{"proven":true,"commands":["sleep 6"],"unproven":[]}'
    $storeA = Join-Path $Work 'af-store-conc'
    $avArgs = @($avPy, '-Root', $AfA, '-Changed', (Join-Path $AfA 'pkg\core.py'), '-Lang', 'python',
                '-Plan', (Join-Path $AfA 'plan.json'), '-Store', $storeA)
    $p1 = Start-Process -FilePath $afPy -ArgumentList ($avArgs + @('-Receipt', (Join-Path $Work 'af-c1.json'))) `
        -NoNewWindow -PassThru -RedirectStandardOutput (Join-Path $Work 'af-c1.out') -RedirectStandardError (Join-Path $Work 'af-c1.err')
    $p2 = Start-Process -FilePath $afPy -ArgumentList ($avArgs + @('-Receipt', (Join-Path $Work 'af-c2.json'))) `
        -NoNewWindow -PassThru -RedirectStandardOutput (Join-Path $Work 'af-c2.out') -RedirectStandardError (Join-Path $Work 'af-c2.err')
    $null = $p1.WaitForExit(); $null = $p2.WaitForExit()
    Assert-True (($p1.ExitCode -eq 0 -and $p2.ExitCode -eq 1) -or ($p1.ExitCode -eq 1 -and $p2.ExitCode -eq 0)) `
        'AF1 同 root 并发两链：恰好一胜一拒（fail-closed，不竞争 journal-begin）' "e1=$($p1.ExitCode) e2=$($p2.ExitCode)"
    $afLoserOut = [IO.File]::ReadAllText((Join-Path $Work ("af-c{0}.out" -f ($(if ($p1.ExitCode -eq 1) { 1 } else { 2 })))), $afEnc)
    Assert-True ($afLoserOut -match 'chain\.lock' -and $afLoserOut -match 'journal\.py (status|restore)') `
        'AF1b 拒者点名 chain lock 脏状态与恢复命令' (($afLoserOut -split "`r?`n" | Select-Object -First 2) -join ' / ')

    # --- AF2/AF3: 崩溃恢复：杀进程 → 锁与快照留下 → 下次运行诊断 → 清锁后可还原 ---
    $AfB = Join-Path $Work 'af-crash'
    New-AvFixture $AfB
    New-File (Join-Path $AfB 'plan.json') '{"proven":true,"commands":["sleep 8"],"unproven":[]}'
    $storeB = Join-Path $Work 'af-store-crash'
    $pB = Start-Process -FilePath $afPy -ArgumentList @($avPy, '-Root', $AfB, '-Changed', (Join-Path $AfB 'pkg\core.py'), `
        '-Lang', 'python', '-Plan', (Join-Path $AfB 'plan.json'), '-Store', $storeB, '-Receipt', (Join-Path $Work 'af-crash.json')) `
        -NoNewWindow -PassThru -RedirectStandardOutput (Join-Path $Work 'af-crash.out') -RedirectStandardError (Join-Path $Work 'af-crash.err')
    Start-Sleep -Seconds 4
    $afWasAlive = (-not $pB.HasExited)
    if (-not $pB.HasExited) { $pB.Kill() }
    $null = $pB.WaitForExit()
    Assert-True ($afWasAlive -and (Test-Path (Join-Path $AfB '.journal\chain.lock'))) `
        'AF2 run 中途杀进程：锁留下（故意成为脏状态标记）' "alive=$afWasAlive"
    Assert-True (Test-Path (Join-Path $storeB 'latest\manifest.json')) `
        'AF2b 崩溃后 journal 快照还在（可诊断可还原）' ''
    $afNextOut = @(Invoke-Av $AfB $storeB (Join-Path $Work 'af-crash2.json'))
    Assert-True ($LASTEXITCODE -eq 1 -and ((@($afNextOut) -join ' ') -match 'chain\.lock')) `
        'AF2c 下次运行检测出脏状态并给清晰诊断（exit 1）' "exit=$LASTEXITCODE"
    Remove-Item (Join-Path $AfB '.journal\chain.lock') -Force
    & $afPy $jrPy -Mode restore -Root $AfB -Store $storeB 2>&1 | Out-Null
    Assert-True ($LASTEXITCODE -eq 0) 'AF3 清锁后快照可还原（崩溃不留死账）' "exit=$LASTEXITCODE"

    # --- AF4: journal manifest 畸形 → 全链 fail-closed，不静默不抛栈 ---
    $AfM = Join-Path $Work 'af-manifest'
    New-AvFixture $AfM
    New-File (Join-Path $AfM 'plan.json') '{"proven":true,"commands":["echo m > m.txt"],"unproven":[]}'
    $storeM = Join-Path $Work 'af-store-manifest'
    $null = Invoke-Av $AfM $storeM (Join-Path $Work 'af-m1.json')
    [IO.File]::WriteAllText((Join-Path $storeM 'latest\manifest.json'), '{corrupt json', $afEnc)
    & $afPy $jrPy -Mode end -Root $AfM -Store $storeM 2>&1 | Out-Null
    Assert-True ($LASTEXITCODE -eq 2) 'AF4 journal end 对坏 manifest exit=2（不是 traceback 不是 changed）' "exit=$LASTEXITCODE"
    & $afPy $jrPy -Mode status -Root $AfM -Store $storeM 2>&1 | Out-Null
    Assert-True ($LASTEXITCODE -eq 2) 'AF4b journal status 对坏 manifest exit=2' "exit=$LASTEXITCODE"
    $null = Invoke-Av $AfM $storeM (Join-Path $Work 'af-m2.json')
    $afM2 = Read-Receipt (Join-Path $Work 'af-m2.json')
    $afM2steps = @($afM2.steps | ForEach-Object { $_.step })
    Assert-True ($LASTEXITCODE -eq 1 -and $afM2.verdict -eq 'refused by journal' -and ($afM2steps -notcontains 'run')) `
        'AF4c 全链拒绝：journal-begin 判红就不许 run（没有证据链的 run 不发生）' "verdict=$($afM2.verdict)"

    # --- AF5: ledger 畸形 → 闸 fail-closed，不读成空账 ---
    $AfL = Join-Path $Work 'af-ledger'
    New-AvFixture $AfL
    New-File (Join-Path $AfL 'plan.json') '{"proven":true,"commands":["echo l > l.txt"],"unproven":[]}'
    $storeL = Join-Path $Work 'af-store-ledger'
    New-Item -ItemType Directory -Force -Path $storeL | Out-Null
    [IO.File]::WriteAllText((Join-Path $storeL 'cost-ledger.json'), '{bad', $afEnc)
    & $afPy $ldPy -Mode add -Ledger (Join-Path $storeL 'cost-ledger.json') -Task t -Key k -Wall 1 2>&1 | Out-Null
    Assert-True ($LASTEXITCODE -eq 2) 'AF5 ledger add 对坏账 exit=2（半截账不读成空账）' "exit=$LASTEXITCODE"
    $null = Invoke-Av $AfL $storeL (Join-Path $Work 'af-l1.json') @('--budget-token', '5000', '--task-id', 'afl')
    $afL1 = Read-Receipt (Join-Path $Work 'af-l1.json')
    Assert-True ($LASTEXITCODE -eq 1 -and $afL1.verdict -eq 'refused by budget ledger' `
        -and (@($afL1.steps | ForEach-Object { $_.step }) -notcontains 'run')) `
        'AF5b 预算闸遇坏账 fail-closed（不降级成弱闸不静默）' "verdict=$($afL1.verdict)"

    # --- AF6: resume 会话文件坏 → 直接拒绝 + 链上检查点失败被点名 ---
    $AfR = Join-Path $Work 'af-resume'
    New-AvFixture $AfR
    New-Item -ItemType Directory -Force -Path (Join-Path $AfR '.agent\resume') | Out-Null
    [IO.File]::WriteAllText((Join-Path $AfR '.agent\resume\badtask.json'), '{bad', $afEnc)
    & $afPy $rsPy status badtask -Root $AfR 2>&1 | Out-Null
    Assert-True ($LASTEXITCODE -eq 2) 'AF6 resume status 对坏会话 exit=2（干净拒绝不是 traceback）' "exit=$LASTEXITCODE"
    & $afPy $rsPy begin badtask -Root $AfR 2>&1 | Out-Null
    Assert-True ($LASTEXITCODE -eq 2) 'AF6b resume begin 拒绝覆盖坏会话（fail-closed）' "exit=$LASTEXITCODE"
    New-File (Join-Path $AfR 'plan.json') '{"proven":true,"commands":["false"],"unproven":[]}'
    $storeR = Join-Path $Work 'af-store-resume'
    $null = Invoke-Av $AfR $storeR (Join-Path $Work 'af-r1.json') @('--task-id', 'badtask')
    $afR1 = Read-Receipt (Join-Path $Work 'af-r1.json')
    $afR1steps = @($afR1.steps | ForEach-Object { "$($_.step)=$($_.verdict)" })
    Assert-True ($LASTEXITCODE -eq 1 -and $afR1.verdict -eq 'command failed' `
        -and ($afR1steps -contains 'resume-progress=failed') -and ($afR1steps -contains 'resume-suspend=failed')) `
        'AF6c 命令失败+坏会话：检查点失败被回执点名（不静默吞掉）' ($afR1steps -join ',')

    # --- AF7/AF8: 超大计划 500 条（并行门）：不崩、顺序保持、逐条过 policy ---
    $AfBig = Join-Path $Work 'af-big'
    New-AvFixture $AfBig
    $afBigCmds = 0..499 | ForEach-Object { "echo b$_ > b$_.txt" }
    [IO.File]::WriteAllText((Join-Path $AfBig 'plan.json'),
        (@{ proven = $true; commands = $afBigCmds; unproven = @() } | ConvertTo-Json -Compress), $afEnc)
    $storeBig = Join-Path $Work 'af-store-big'
    $null = Invoke-Av $AfBig $storeBig (Join-Path $Work 'af-big.json') @('--parallel-gates')
    $afBigDoc = Read-Receipt (Join-Path $Work 'af-big.json')
    $afRunCmds = @($afBigDoc.run | ForEach-Object { $_.command })
    $afPolCmds = @($afBigDoc.policy | ForEach-Object { $_.command })
    Assert-True ($LASTEXITCODE -eq 0 -and @($afRunCmds).Count -eq 500 `
        -and $afRunCmds[0] -eq 'echo b0 > b0.txt' -and $afRunCmds[250] -eq 'echo b250 > b250.txt' `
        -and $afRunCmds[499] -eq 'echo b499 > b499.txt') `
        'AF7 500 条计划不崩、run 逐条执行且顺序锚点保持' "exit=$LASTEXITCODE run=$(@($afRunCmds).Count)"
    $afAlignOk = $true
    for ($i = 0; $i -lt 500; $i++) { if ($afPolCmds[$i] -ne $afRunCmds[$i]) { $afAlignOk = $false; break } }
    Assert-True ($afAlignOk -and @($afPolCmds).Count -eq 500 `
        -and (@($afBigDoc.policy | Where-Object { $_.exit -ne 0 }).Count -eq 0)) `
        'AF8 policy 500 条逐条过且与 run 顺序逐位对齐（按链序记账不是完成序）' "pol=$(@($afPolCmds).Count)"

    # --- AF9-AF12: compact 与 full 回执字段一致性 + 单行 ASCII + stdout 零感知 ---
    $AfC = Join-Path $Work 'af-cmp'
    New-AvFixture $AfC
    New-File (Join-Path $AfC 'plan.json') '{"proven":true,"commands":["echo a > a.txt","echo b > b.txt","echo c > c.txt"],"unproven":[]}'
    $storeC = Join-Path $Work 'af-store-cmp'
    $rcF = Join-Path $Work 'af-rc-full.json'; $rcCp = Join-Path $Work 'af-rc-compact.json'
    $afOutFull = @(Invoke-Av $AfC $storeC $rcF)
    $afExitFull = $LASTEXITCODE
    Remove-Item (Join-Path $AfC 'a.txt'), (Join-Path $AfC 'b.txt'), (Join-Path $AfC 'c.txt') -Force
    $afOutCmp = @(Invoke-Av $AfC $storeC $rcCp @('--compact'))
    $afExitCmp = $LASTEXITCODE
    $afFullDoc = Read-Receipt $rcF
    $afCmpDoc = Read-Receipt $rcCp
    $afFullSteps = @($afFullDoc.steps | ForEach-Object { "$($_.step)|$($_.verdict)|$($_.exit)" })
    $afCmpSteps = @($afCmpDoc.steps | ForEach-Object { "$($_.step)|$($_.verdict)|$($_.exit)" })
    $afFullRun = @($afFullDoc.run | ForEach-Object { "$($_.command)|$($_.exit)" })
    $afCmpRun = @($afCmpDoc.run | ForEach-Object { "$($_.command)|$($_.exit)" })
    Assert-True ($afExitFull -eq 0 -and $afExitCmp -eq 0 `
        -and $afFullDoc.verdict -eq $afCmpDoc.verdict -and $afFullDoc.exit -eq $afCmpDoc.exit `
        -and $afFullDoc.plan.proven -eq $afCmpDoc.proven) `
        'AF9 同一 run 两种回执：verdict/exit/proven 逐项相等' "v=$($afCmpDoc.verdict)"
    Assert-True ((@($afFullSteps) -join ';') -eq (@($afCmpSteps) -join ';') `
        -and (@($afFullRun) -join ';') -eq (@($afCmpRun) -join ';')) `
        'AF10 步骤序列与 run 序列逐项相等（compact 只砍文本不砍判定）' ''
    $afCmpRunKeys = (@($afCmpDoc.run[0].PSObject.Properties.Name | Sort-Object) -join ',')
    Assert-True ($afCmpRunKeys -eq 'command,exit,seconds') `
        'AF11 compact 的 run 段只留 command+exit+seconds（契约钉死）' $afCmpRunKeys
    $afCpRaw = [IO.File]::ReadAllText($rcCp, $afEnc)
    Assert-True ($afCpRaw.Trim().IndexOf("`n") -lt 0 -and $afCpRaw -notmatch '[^\x00-\x7F]') `
        'AF12 compact 单行且纯 ASCII（任何解码器都读不坏）' ''
    $afNormFull = (@($afOutFull) -join "`n") -replace '[0-9]+\.[0-9]+', 'N'
    $afNormCmp = (@($afOutCmp) -join "`n") -replace '[0-9]+\.[0-9]+', 'N'
    Assert-True ($afNormFull -eq $afNormCmp) `
        'AF13 compact 的 stdout 与 full 一致（规整计时后逐字符相等，默认路径零变化）' ''

    # --- AF13: compact 与 full 的体积比（有记忆留痕的代表性 run）---
    $AfC2 = Join-Path $Work 'af-cmp2'
    New-AvFixture $AfC2
    & $afPy (Join-Path $tools 'memory.py') -Mode init -Root $AfC2 2>&1 | Out-Null
    & $afPy (Join-Path $tools 'timeline.py') -Mode init -Root $AfC2 2>&1 | Out-Null
    $af20 = 0..29 | ForEach-Object { "echo e$_ > e$_.txt" }
    [IO.File]::WriteAllText((Join-Path $AfC2 'plan.json'),
        (@{ proven = $true; commands = $af20; unproven = @() } | ConvertTo-Json -Compress), $afEnc)
    $storeC2 = Join-Path $Work 'af-store-cmp2'
    $null = Invoke-Av $AfC2 $storeC2 (Join-Path $Work 'af-rc2-full.json')
    $null = Invoke-Av $AfC2 $storeC2 (Join-Path $Work 'af-rc2-compact.json') @('--compact')
    $fb = (Get-Item (Join-Path $Work 'af-rc2-full.json')).Length
    $cb = (Get-Item (Join-Path $Work 'af-rc2-compact.json')).Length
    Assert-True ($cb * 4 -lt $fb) 'AF14 compact 显著小于 full（≥4 倍；典型 run 实测由 bench 报数）' "full=$fb compact=$cb"

    # --- AF15-AF18: cost 记账往返与预算闸打通 ---
    $AfCost = Join-Path $Work 'af-cost'
    New-AvFixture $AfCost
    New-File (Join-Path $AfCost 'plan.json') '{"proven":true,"commands":["echo c1 > c1.txt","echo c2 > c2.txt"],"unproven":[]}'
    $storeCost = Join-Path $Work 'af-store-cost'
    $afCostOut = @(Invoke-Av $AfCost $storeCost (Join-Path $Work 'af-cost1.json') `
        @('--agent-id', 'sub-a', '--spent-token', '12345', '--task-id', 'cost1'))
    $afCost1 = Read-Receipt (Join-Path $Work 'af-cost1.json')
    Assert-True ($LASTEXITCODE -eq 0 -and $afCost1.cost.tokens -eq 12345 -and $afCost1.cost.recorded -eq $true) `
        'AF15 cost 记账：回执 cost 字段如实（tokens=声明值 recorded=true）' "tokens=$($afCost1.cost.tokens)"
    Assert-True ((@($afCostOut) -join "`n") -match 'cost: wall=') 'AF15b verdict 尾部有 cost: 行' ''
    $afLed1 = Read-Receipt (Join-Path $storeCost 'cost-ledger.json')
    $afCostEntries = @($afLed1.entries)
    Assert-True (@($afCostEntries).Count -eq 4 `
        -and (@($afCostEntries | Where-Object { $_.agent_id -ne 'sub-a' }).Count -eq 0)) `
        'AF16 台账 4 条（2 命令 + overhead + spent）且全部带 agent-id' "n=$(@($afCostEntries).Count)"
    Assert-True ((@($afCostEntries | Where-Object { $_.tokens -eq 12345 }).Count -eq 1)) `
        'AF16b tokens 只落在 spent entry（wall 不冒充 token）' ''
    $null = Invoke-Av $AfCost $storeCost (Join-Path $Work 'af-cost2.json') @('--budget-token', '100000', '--task-id', 'cost1')
    $afCost2 = Read-Receipt (Join-Path $Work 'af-cost2.json')
    Assert-True ($LASTEXITCODE -eq 0 -and $afCost2.budget.recorded_tokens -eq 12345 `
        -and $afCost2.budget.remaining -eq 87655) `
        'AF17 剩余预算 = 声明 - 台账已记（87655 = 100000 - 12345）' "remaining=$($afCost2.budget.remaining)"
    $afLed2 = Read-Receipt (Join-Path $storeCost 'cost-ledger.json')
    Assert-True (@($afLed2.entries).Count -eq 7) `
        'AF17b 第二跑 wall 继续上账且键唯一不去重（4+3=7 条）' "n=$(@($afLed2.entries).Count)"
    $null = Invoke-Av $AfCost $storeCost (Join-Path $Work 'af-cost3.json') @('--budget-token', '100', '--task-id', 'cost1')
    $afCost3 = Read-Receipt (Join-Path $Work 'af-cost3.json')
    Assert-True ($LASTEXITCODE -eq 1 -and $afCost3.verdict -eq 'suspended (budget) before run' `
        -and (@($afCost3.steps | ForEach-Object { $_.step }) -notcontains 'run')) `
        'AF17c 剩余不足挂起在 run 前（censored 不是失败）' "verdict=$($afCost3.verdict)"
    $storePure = Join-Path $Work 'af-store-pure'
    $null = Invoke-Av $AfCost $storePure (Join-Path $Work 'af-cost4.json') @('--budget-token', '0', '--task-id', 'pure1')
    $afCost4 = Read-Receipt (Join-Path $Work 'af-cost4.json')
    Assert-True ($LASTEXITCODE -eq 1 -and $afCost4.verdict -eq 'suspended (budget) before run' `
        -and ($null -eq $afCost4.budget.recorded_tokens)) `
        'AF18 无台账记录维持纯声明制（recorded_tokens 如实为空）' "verdict=$($afCost4.verdict)"

    # --- AF19: 并行门与串行门 verdict 逐项相等 ---
    # 与 AF13 同理：清掉输出文件让两条 run 看到同一棵干净基树——journal-end 的
    # verdict 才可比（clean/changed 是树状态的函数，不是门序的函数）
    $rcPar = Join-Path $Work 'af-rc-parallel.json'
    Remove-Item (Join-Path $AfC 'a.txt'), (Join-Path $AfC 'b.txt'), (Join-Path $AfC 'c.txt') -Force
    $null = Invoke-Av $AfC $storeC $rcPar @('--parallel-gates')
    $afParDoc = Read-Receipt $rcPar
    $afParSteps = @($afParDoc.steps | ForEach-Object { "$($_.step)|$($_.verdict)|$($_.exit)" })
    Assert-True ($LASTEXITCODE -eq 0 -and ((@($afParSteps) -join ';') -eq (@($afFullSteps) -join ';'))) `
        'AF19 并行门与串行门步骤 verdict 逐项相等（按链序记账，不是完成序）' ''

    # --- AF20: 两种拒绝在并行下语义与回执形状不变 ---
    $AfP = Join-Path $Work 'af-polref'
    New-AvFixture $AfP
    New-File (Join-Path $AfP 'plan.json') '{"proven":true,"commands":["echo p > p.txt"],"unproven":[]}'
    $storeP = Join-Path $Work 'af-store-polref'
    $null = Invoke-Av $AfP $storeP (Join-Path $Work 'af-p-s.json') @('-Profile', 'read-only')
    $afPs = Read-Receipt (Join-Path $Work 'af-p-s.json')
    $afPsSteps = @($afPs.steps | ForEach-Object { $_.step })
    $null = Invoke-Av $AfP $storeP (Join-Path $Work 'af-p-p.json') @('-Profile', 'read-only', '--parallel-gates')
    $afPp = Read-Receipt (Join-Path $Work 'af-p-p.json')
    $afPpSteps = @($afPp.steps | ForEach-Object { $_.step })
    Assert-True ($LASTEXITCODE -eq 1 -and $afPs.verdict -eq 'refused by policy' `
        -and $afPp.verdict -eq 'refused by policy' `
        -and ((@($afPsSteps) -join ',') -eq (@($afPpSteps) -join ','))) `
        'AF20 policy 拒绝：并行与串行 verdict/步骤形状一致（并行跑完的 constraints 不记账）' "v=$($afPp.verdict)"
    $AfCon = Join-Path $Work 'af-conref'
    New-Item -ItemType Directory -Force -Path (Join-Path $AfCon 'crates\x\src') | Out-Null
    [IO.File]::WriteAllText((Join-Path $AfCon 'crates\x\src\a.rs'), "pub fn a() {}$nl", $afEnc)
    & $afPy (Join-Path $tools 'constraints.py') -Mode init -Root $AfCon 2>&1 | Out-Null
    Push-Location $AfCon
    & git init -q 2>&1 | Out-Null
    & git add -A 2>&1 | Out-Null
    & git -c user.email=a@b -c user.name=t commit -qm init 2>&1 | Out-Null
    Pop-Location
    New-File (Join-Path $AfCon 'plan.json') '{"proven":true,"commands":["true"],"unproven":[]}'
    [IO.File]::WriteAllText((Join-Path $AfCon 'crates\x\src\a.rs'),
        "pub fn a() {}$nl pub fn b() { println!(`"hi`"); }$nl", $afEnc)
    $storeCon = Join-Path $Work 'af-store-conref'
    $null = Invoke-Av $AfCon $storeCon (Join-Path $Work 'af-con-s.json') @('-Lang', 'rust', '-Changed', 'crates\x\src\a.rs')
    $afConS = Read-Receipt (Join-Path $Work 'af-con-s.json')
    $null = Invoke-Av $AfCon $storeCon (Join-Path $Work 'af-con-p.json') @('-Lang', 'rust', '-Changed', 'crates\x\src\a.rs', '--parallel-gates')
    $afConP = Read-Receipt (Join-Path $Work 'af-con-p.json')
    Assert-True ($LASTEXITCODE -eq 1 -and $afConS.verdict -eq 'refused by constraints' `
        -and $afConP.verdict -eq 'refused by constraints' `
        -and (@($afConP.steps | ForEach-Object { $_.step }) -notcontains 'run')) `
        'AF20b constraints 拒绝：并行语义不变，run 不出现' "v=$($afConP.verdict)"

    # --- AF21: 并发不同 root 两链（compact + agent-id）互不干扰 ---
    $AfSA = Join-Path $Work 'af-sub-a'; New-AvFixture $AfSA
    $AfSB = Join-Path $Work 'af-sub-b'; New-AvFixture $AfSB
    New-File (Join-Path $AfSA 'plan.json') '{"proven":true,"commands":["echo sa > sa.txt"],"unproven":[]}'
    New-File (Join-Path $AfSB 'plan.json') '{"proven":true,"commands":["echo sb > sb.txt"],"unproven":[]}'
    $pA = Start-Process -FilePath $afPy -ArgumentList @($avPy, '-Root', $AfSA, '-Changed', (Join-Path $AfSA 'pkg\core.py'), `
        '-Lang', 'python', '-Plan', (Join-Path $AfSA 'plan.json'), '-Store', (Join-Path $Work 'af-store-sa'), `
        '-Receipt', (Join-Path $Work 'af-sa.json'), '--compact', '--agent-id', 'sub-a') `
        -NoNewWindow -PassThru -RedirectStandardOutput (Join-Path $Work 'af-sa.out') -RedirectStandardError (Join-Path $Work 'af-sa.err')
    $pB = Start-Process -FilePath $afPy -ArgumentList @($avPy, '-Root', $AfSB, '-Changed', (Join-Path $AfSB 'pkg\core.py'), `
        '-Lang', 'python', '-Plan', (Join-Path $AfSB 'plan.json'), '-Store', (Join-Path $Work 'af-store-sb'), `
        '-Receipt', (Join-Path $Work 'af-sb.json'), '--compact', '--agent-id', 'sub-b') `
        -NoNewWindow -PassThru -RedirectStandardOutput (Join-Path $Work 'af-sb.out') -RedirectStandardError (Join-Path $Work 'af-sb.err')
    $null = $pA.WaitForExit(); $null = $pB.WaitForExit()
    Assert-True ($pA.ExitCode -eq 0 -and $pB.ExitCode -eq 0) `
        'AF21 并发不同 root 两链都成功（链锁不误伤独立 root）' "a=$($pA.ExitCode) b=$($pB.ExitCode)"
    $afSaDoc = Read-Receipt (Join-Path $Work 'af-sa.json')
    $afSbDoc = Read-Receipt (Join-Path $Work 'af-sb.json')
    Assert-True ($afSaDoc.agent_id -eq 'sub-a' -and $afSbDoc.agent_id -eq 'sub-b') `
        'AF21b 各自 compact 回执带各自 agent-id（互不干扰）' ''
    $afSaLed = Read-Receipt (Join-Path (Join-Path $Work 'af-store-sa') 'cost-ledger.json')
    Assert-True ((@($afSaLed.entries | Where-Object { $_.agent_id -ne 'sub-a' }).Count -eq 0) `
        -and @($afSaLed.entries).Count -eq 2) `
        'AF21c 各自台账行 agent-id 归属正确（1 命令 + overhead）' "n=$(@($afSaLed.entries).Count)" 

    # --- AF22: chain.lock 不进快照不进 diff（SKIP 集成员）---
    Assert-True ((@($afFullDoc.changes) | Where-Object { $_ -match 'chain\.lock' }).Count -eq 0 `
        -and (@($afFullDoc.changes) -match 'added\s+a\.txt').Count -gt 0) `
        'AF22 chain.lock 不进 changes（.journal 在 SKIP 集里），changes 仍看得见命令写的文件' `
        (@($afFullDoc.changes) -join ';')
}

# =====================================================================================
"== AG. 反造假核心层：attest 哈希链（防篡改防重复）/ claim_audit 三态 / 编排链集成 =="
# 每条断言对着一个造假面：账改不了（payload 复算）、丢不了（断链即报）、数不了两遍
# （去重键结构性拒绝）；声明不许当真（无证据=UNVERIFIED）、与现实矛盾=FABRICATED 最响
# 警报且全链拒绝。夹具全部现建，真实仓零接触。
$agPy = $null
foreach ($c in @($env:PYTHON, 'python', 'python3', 'py')) {
    if (-not $c) { continue }
    $agSrc = (Get-Command $c -ErrorAction SilentlyContinue).Source
    if (-not $agSrc) { continue }
    if ((& $agSrc -c 'print(1)' 2>$null) -eq '1') { $agPy = $agSrc; break }
}
if (-not $agPy) {
    "  SKIP  AG0 找不到可用的 Python，跳过反造假核心层检查"
    $skipped += 30
} else {
    $agEnc = New-Object System.Text.UTF8Encoding($false)
    $atPy = Join-Path $tools 'attest.py'
    $caPy = Join-Path $tools 'claim_audit.py'
    $avPy = Join-Path $tools 'agent_verify.py'
    $ldPy = Join-Path $tools 'ledger.py'

    function Invoke-At([string[]]$atArgs) {
        $script:agOut = @(& $agPy $atPy @atArgs 2>&1)
        return $LASTEXITCODE
    }
    function Invoke-Ca([string[]]$caArgs) {
        $script:agOut = @(& $agPy $caPy @caArgs 2>&1)
        return $LASTEXITCODE
    }
    function New-AgFixture([string]$root) {
        New-Item -ItemType Directory -Force -Path (Join-Path $root 'pkg'), (Join-Path $root 'tests') | Out-Null
        New-File (Join-Path $root 'pkg\core.py') "def f(): return 1$nl"
        New-File (Join-Path $root 'pkg\__init__.py') ''
        New-File (Join-Path $root 'tests\test_core.py') "from pkg.core import f${nl}def test_f(): assert f() == 1$nl"
        New-File (Join-Path $root 'conftest.py') ''
        Push-Location $root
        & git init -q 2>&1 | Out-Null
        & git add -A 2>&1 | Out-Null
        & git -c user.email=a@b -c user.name=t commit -qm init 2>&1 | Out-Null
        Pop-Location
    }
    function Invoke-AgAv([string]$root, [string]$store, [string]$receipt, [string[]]$extra) {
        $argsAv = @($avPy, '-Root', $root, '-Changed', (Join-Path $root 'pkg\core.py'), '-Lang', 'python',
                    '-Plan', (Join-Path $root 'plan.json'), '-Store', $store, '-Receipt', $receipt) + $extra
        & $agPy @argsAv 2>&1 | Out-Null
        return $LASTEXITCODE
    }
    function Read-AgReceipt([string]$path) {
        return [IO.File]::ReadAllText($path, $agEnc) | ConvertFrom-Json
    }

    # --- AG1-AG3: 链 append/verify/balance 往返 ---
    $AgS1 = Join-Path $Work 'ag-chain1'
    $ag1a = Invoke-At @('append', '--store', $AgS1, '--kind', 'spend', '--task-id', 't1', '--agent-id', 'ag1', '--runid', 'r1', '--claimed-tokens', '100')
    $ag1b = Invoke-At @('append', '--store', $AgS1, '--kind', 'grant', '--task-id', 't1', '--agent-id', 'ag1', '--runid', 'r2', '--claimed-tokens', '50')
    $ag1c = Invoke-At @('append', '--store', $AgS1, '--kind', 'correction', '--task-id', 't1', '--agent-id', 'ag1', '--runid', 'r3', '--claimed-tokens', '-20')
    Assert-True ($ag1a -eq 0 -and $ag1b -eq 0 -and $ag1c -eq 0) `
        'AG1 链 append x3 全成（spend/grant/correction 各一）' "e=$ag1a/$ag1b/$ag1c"
    $ag2 = Invoke-At @('verify', '--store', $AgS1)
    Assert-True ($ag2 -eq 0 -and ($agOut -join ' ') -match 'chain OK \(3 entries') `
        'AG2 verify 全链复算逐条验 OK 且条数锚点' (($agOut -join ' '))
    $ag3 = Invoke-At @('balance', '--store', $AgS1, '--task', 't1')
    $ag3txt = $agOut -join ' '
    Assert-True ($ag3 -eq 0 -and $ag3txt -match 'spent: 100' -and $ag3txt -match 'granted: 50' `
        -and $ag3txt -match 'corrections: -20' -and $ag3txt -match 'spent_net \(budget-gate facing\): 80') `
        'AG3 balance --task 逐数正确（spent_net=spent+corrections 是闸面口径）' $ag3txt
    $ag3b = Invoke-At @('balance', '--store', $AgS1, '--agent', 'ag1')
    Assert-True ($ag3b -eq 0 -and (($agOut -join ' ') -match 'spent: 100')) `
        'AG3b balance --agent 认同一本账' (($agOut -join ' '))

    # --- AG4: 断链检测报第一条断点（篡改/删行/重放三种断法）---
    $agLines = [IO.File]::ReadAllLines((Join-Path $AgS1 'attest-ledger.jsonl'), $agEnc)
    $AgS1t = Join-Path $Work 'ag-chain1-tamper'
    New-Item -ItemType Directory -Force -Path $AgS1t | Out-Null
    [IO.File]::WriteAllLines((Join-Path $AgS1t 'attest-ledger.jsonl'),
        @($agLines[0], $agLines[1].Replace('"claimed_tokens":50', '"claimed_tokens":999'), $agLines[2]), $agEnc)
    $ag4 = Invoke-At @('verify', '--store', $AgS1t)
    Assert-True ($ag4 -eq 1 -and ($agOut -join ' ') -match 'payload_hash mismatch at seq 2') `
        'AG4 篡改中段字段→verify 报第一条断点 seq 2（payload 复算不符，改过的账藏不住）' (($agOut -join ' '))
    $AgS1g = Join-Path $Work 'ag-chain1-gap'
    New-Item -ItemType Directory -Force -Path $AgS1g | Out-Null
    [IO.File]::WriteAllLines((Join-Path $AgS1g 'attest-ledger.jsonl'), @($agLines[0], $agLines[2]), $agEnc)
    $ag4b = Invoke-At @('verify', '--store', $AgS1g)
    Assert-True ($ag4b -eq 1 -and ($agOut -join ' ') -match 'seq 3 out of order at position 2') `
        'AG4b 删中段→seq 断报第一条断点（账凭空消失即报）' (($agOut -join ' '))
    $AgS1r = Join-Path $Work 'ag-chain1-replay'
    New-Item -ItemType Directory -Force -Path $AgS1r | Out-Null
    [IO.File]::WriteAllLines((Join-Path $AgS1r 'attest-ledger.jsonl'),
        @($agLines[0], $agLines[1], $agLines[2], $agLines[0]), $agEnc)
    $ag4c = Invoke-At @('verify', '--store', $AgS1r)
    Assert-True ($ag4c -eq 1 -and ($agOut -join ' ') -match 'position 4') `
        'AG4c 重放旧行（重复 payload）→拒并点名断点' (($agOut -join ' '))

    # --- AG5: 同一 (agent,task,runid,kind) 重复 spend 拒 ---
    $ag5 = Invoke-At @('append', '--store', $AgS1, '--kind', 'spend', '--task-id', 't1', '--agent-id', 'ag1', '--runid', 'r1', '--claimed-tokens', '100')
    $ag5lines = [IO.File]::ReadAllLines((Join-Path $AgS1 'attest-ledger.jsonl'), $agEnc)
    Assert-True ($ag5 -eq 1 -and @($ag5lines).Count -eq 3) `
        'AG5 同一 spend 重复 append 拒且账面行数不变（重复记账结构性不可能）' "e=$ag5 lines=$(@($ag5lines).Count)"

    # --- AG6: import 收编 cost-ledger 已有 spent 账 ---
    $AgLed = Join-Path $Work 'ag-import-cost.json'
    & $agPy $ldPy -Mode add -Ledger $AgLed -Task 'av|t9' -Key 'av|rimp1|spent' -Tokens 4321 -Agent-Id 'imp9' 2>&1 | Out-Null
    & $agPy $ldPy -Mode add -Ledger $AgLed -Task 'av|t9' -Key 'av|rimp2|cmd1|aa' -Wall 2.5 2>&1 | Out-Null
    $AgS3 = Join-Path $Work 'ag-import'
    $ag6 = Invoke-At @('import', '--store', $AgS3, '--from-ledger', $AgLed)
    Assert-True ($ag6 -eq 0 -and ($agOut -join ' ') -match 'imported=1 skipped=0') `
        'AG6 import 收编 cost-ledger spent 行（wall-only 行不是 token 账不收）' (($agOut -join ' '))
    $ag6b = Invoke-At @('balance', '--store', $AgS3, '--task', 't9')
    Assert-True ($ag6b -eq 0 -and (($agOut -join ' ') -match 'spent: 4321')) `
        'AG6b balance 认收编后的旧账（已有账不凭空消失）' (($agOut -join ' '))
    $ag6c = Invoke-At @('import', '--store', $AgS3, '--from-ledger', $AgLed)
    $ag6lines = [IO.File]::ReadAllLines((Join-Path $AgS3 'attest-ledger.jsonl'), $agEnc)
    Assert-True ($ag6c -eq 0 -and ($agOut -join ' ') -match 'imported=0 skipped=1' -and @($ag6lines).Count -eq 1) `
        'AG6c 重导入全跳过（同一笔账不数两遍）' "e=$ag6c lines=$(@($ag6lines).Count)"

    # --- AG7: 坏 JSONL fail-closed ---
    $AgS1c = Join-Path $Work 'ag-chain1-corrupt'
    New-Item -ItemType Directory -Force -Path $AgS1c | Out-Null
    [IO.File]::WriteAllLines((Join-Path $AgS1c 'attest-ledger.jsonl'),
        @($agLines[0], '{bad', $agLines[2]), $agEnc)
    $ag7 = Invoke-At @('append', '--store', $AgS1c, '--kind', 'spend', '--task-id', 'x', '--agent-id', 'y', '--runid', 'z', '--claimed-tokens', '5')
    Assert-True ($ag7 -eq 1) 'AG7 坏 JSONL：append 拒（不在半截账上记新账）' "e=$ag7"
    $ag7b = Invoke-At @('balance', '--store', $AgS1c)
    Assert-True ($ag7b -eq 1) 'AG7b 坏 JSONL：balance 拒（破账上没有数字）' "e=$ag7b"
    $ag7c = Invoke-At @('verify', '--store', $AgS1c)
    Assert-True ($ag7c -eq 1 -and ($agOut -join ' ') -match 'line 2') `
        'AG7c 坏 JSONL：verify 点名坏行行号' (($agOut -join ' '))

    # --- AG8-AG10: claim_audit 三态各至少 2 条 ---
    $AgC = Join-Path $Work 'ag-claims-root'
    New-Item -ItemType Directory -Force -Path (Join-Path $AgC 'pkg'), (Join-Path $AgC 'tests'), `
        (Join-Path $AgC 'artifacts'), (Join-Path $Work 'ag-rc') | Out-Null
    New-File (Join-Path $AgC 'pkg\core.py') "def f(): return 1$nl"
    New-File (Join-Path $AgC 'tests\test_core.py') "def test_f(): assert 1$nl"
    New-File (Join-Path $AgC 'tests\test_other.py') "def test_o(): assert 1$nl"
    New-File (Join-Path $AgC 'tests\test_red.py') "def test_r(): assert 0$nl"
    New-File (Join-Path $AgC 'artifacts\bench.txt') "guard_overhead_ratio 2.35x receipt_full_bytes 4800$nl"
    New-File (Join-Path $Work 'ag-rc\rc-green.json') '{"at":"fixture","run":[{"command":"python -m pytest tests/test_core.py -q","exit":0}]}'
    New-File (Join-Path $Work 'ag-rc\rc-red.json') '{"at":"fixture","run":[{"command":"python -m pytest tests/test_red.py -q","exit":1}]}'
    $agRcGlob = Join-Path $Work 'ag-rc\*.json'

    New-File (Join-Path $Work 'ag-claims-v.jsonl') ((@(
        '{"claim":"pkg/core.py exists on disk","kind":"file-exists"}',
        '{"claim":"tests/test_core.py all green","kind":"test-pass"}') -join $nl) + $nl)
    $ag8 = Invoke-Ca @('-Claims', (Join-Path $Work 'ag-claims-v.jsonl'), '-Root', $AgC, '-Receipts', $agRcGlob)
    $ag8v = @($agOut | Where-Object { $_ -match '\] VERIFIED' }).Count
    Assert-True ($ag8 -eq 0 -and $ag8v -eq 2) `
        'AG8 VERIFIED x2（file-exists 带内容哈希 + test-pass 有绿 receipt）exit 0' "e=$ag8 v=$ag8v"

    New-File (Join-Path $Work 'ag-claims-u.jsonl') ((@(
        '{"claim":"session spent 777 tokens","kind":"number"}',
        '{"claim":"tests/test_other.py all green","kind":"test-pass"}') -join $nl) + $nl)
    $ag9 = Invoke-Ca @('-Claims', (Join-Path $Work 'ag-claims-u.jsonl'), '-Root', $AgC, '-Receipts', $agRcGlob)
    $ag9u = @($agOut | Where-Object { $_ -match '\] UNVERIFIED' }).Count
    Assert-True ($ag9 -eq 1 -and $ag9u -eq 2) `
        'AG9 UNVERIFIED x2（数字无出处 + 测试存在但 receipt 无 verdict）exit 1（不许当真）' "e=$ag9 u=$ag9u"

    New-File (Join-Path $Work 'ag-claims-f.jsonl') ((@(
        '{"claim":"tests/ghost_rs.rs all green","kind":"test-pass"}',
        '{"claim":"tests/test_red.py all green","kind":"test-pass"}') -join $nl) + $nl)
    $ag10 = Invoke-Ca @('-Claims', (Join-Path $Work 'ag-claims-f.jsonl'), '-Root', $AgC, '-Receipts', $agRcGlob)
    $ag10f = @($agOut | Where-Object { $_ -match '\] FABRICATED' }).Count
    Assert-True ($ag10 -eq 2 -and $ag10f -eq 2 -and (($agOut -join ' ') -match 'is RED')) `
        'AG10 FABRICATED x2（测试文件不存在 + 声称绿实际最近 verdict 是红）exit 2（最响警报）' "e=$ag10 f=$ag10f"

    # --- AG11: claims 文件自身 fail-closed ---
    New-File (Join-Path $Work 'ag-claims-bad.jsonl') ((@(
        '{"claim":"ok line","kind":"file-exists"}', '{bad') -join $nl) + $nl)
    $ag11 = Invoke-Ca @('-Claims', (Join-Path $Work 'ag-claims-bad.jsonl'), '-Root', $AgC)
    Assert-True ($ag11 -eq 3) 'AG11 claims 坏行→exit 3 拒整份（半读的声明集不许有一半被当真）' "e=$ag11"
    # 机器路径形态运行期拼接（L106：夹具原文不许出现隐私闸认识的连续命中串）
    $agMp = 'X' + ':/Us' + 'ers/someone/notes.md'
    New-File (Join-Path $Work 'ag-claims-mp.jsonl') ('{"claim":"see ' + $agMp + ' for detail","kind":"file-exists"}' + $nl)
    $ag11b = Invoke-Ca @('-Claims', (Join-Path $Work 'ag-claims-mp.jsonl'), '-Root', $AgC)
    Assert-True ($ag11b -eq 3) 'AG11b claims 带机器路径→exit 3 拒（审计可能随仓提交，盘符形态不进门）' "e=$ag11b"

    # --- AG12/AG13: agent_verify --claims-file 集成 ---
    $AgA = Join-Path $Work 'ag-av'
    New-AgFixture $AgA
    New-File (Join-Path $AgA 'plan.json') '{"proven":true,"commands":["echo ag > ag.txt"],"unproven":[]}'
    $storeA = Join-Path $Work 'ag-store-av'
    $storeB = Join-Path $Work 'ag-store-decl'
    $storeC = Join-Path $Work 'ag-store-import'
    $storeCon = Join-Path $Work 'ag-store-con'
    New-File (Join-Path $Work 'ag-av-f.jsonl') ('{"claim":"tests/ghost_rs.rs all green","kind":"test-pass"}' + $nl)
    $ag12 = Invoke-AgAv $AgA $storeA (Join-Path $Work 'ag-rc-fab.json') @('--claims-file', (Join-Path $Work 'ag-av-f.jsonl'))
    $ag12doc = Read-AgReceipt (Join-Path $Work 'ag-rc-fab.json')
    $ag12steps = @($ag12doc.steps | ForEach-Object { $_.step })
    Assert-True ($ag12 -eq 1 -and $ag12doc.verdict -eq 'rejected: fabricated claims' -and ($ag12steps -notcontains 'run')) `
        'AG12 --claims-file 含 FABRICATED→全链拒绝在 run 前（模型说什么不算什么）' "e=$ag12 v=$($ag12doc.verdict)"
    New-File (Join-Path $Work 'ag-av-ok.jsonl') ('{"claim":"pkg/core.py exists","kind":"file-exists"}' + $nl)
    $ag13 = Invoke-AgAv $AgA $storeA (Join-Path $Work 'ag-rc-ok.json') @('--claims-file', (Join-Path $Work 'ag-av-ok.jsonl'))
    $ag13doc = Read-AgReceipt (Join-Path $Work 'ag-rc-ok.json')
    Assert-True ($ag13 -eq 0 -and $ag13doc.claims.total -eq 1 -and $ag13doc.claims.verified -eq 1) `
        'AG13 --claims-file 全 VERIFIED→链通过且回执带 claims 汇总（appended 字段）' "e=$ag13 v=$($ag13doc.verdict)"
    $ag13b = Invoke-AgAv $AgA $storeA (Join-Path $Work 'ag-rc-ok-c.json') @('--claims-file', (Join-Path $Work 'ag-av-ok.jsonl'), '--compact')
    $ag13bdoc = Read-AgReceipt (Join-Path $Work 'ag-rc-ok-c.json')
    Assert-True ($ag13b -eq 0 -and ($ag13bdoc.PSObject.Properties.Name -contains 'claims') -and $ag13bdoc.claims.verified -eq 1) `
        'AG13b compact 回执同样携带 claims 汇总（compact 只砍文本不砍判定）' "e=$ag13b"

    # --- AG14/AG15: --spent-token 上链 + 预算闸走链余额 ---
    $ag14 = Invoke-AgAv $AgA $storeA (Join-Path $Work 'ag-rc-sp1.json') @('--task-id', 'agt1', '--spent-token', '777')
    $ag14doc = Read-AgReceipt (Join-Path $Work 'ag-rc-sp1.json')
    Assert-True ($ag14 -eq 0 -and $ag14doc.attest.appended -eq $true -and $ag14doc.attest.seq -eq 1 `
        -and (Test-Path (Join-Path $storeA 'attest-ledger.jsonl'))) `
        'AG14 --spent-token 落认证链（回执 attest.appended=true、seq=1、链文件在 store）' "e=$ag14 seq=$($ag14doc.attest.seq)"
    $ag15 = Invoke-AgAv $AgA $storeA (Join-Path $Work 'ag-rc-bg1.json') @('--task-id', 'agt1', '--budget-token', '5000')
    $ag15doc = Read-AgReceipt (Join-Path $Work 'ag-rc-bg1.json')
    Assert-True ($ag15 -eq 0 -and $ag15doc.budget.recorded_tokens -eq 777 `
        -and $ag15doc.budget.remaining -eq 4223 -and $ag15doc.budget.source -eq 'attest-chain') `
        'AG15 预算闸消费认证链余额（777 已认证，4223=5000-777，source=attest-chain）' "src=$($ag15doc.budget.source) rem=$($ag15doc.budget.remaining)"
    $ag15b = Invoke-AgAv $AgA $storeB (Join-Path $Work 'ag-rc-bg2.json') @('--task-id', 'agt2', '--budget-token', '5000')
    $ag15bdoc = Read-AgReceipt (Join-Path $Work 'ag-rc-bg2.json')
    Assert-True ($ag15b -eq 0 -and $ag15bdoc.budget.source -eq 'declared-only' -and $null -eq $ag15bdoc.budget.recorded_tokens) `
        'AG15b 无链维持声明制并如实标注 declared-only（recorded 如实为空）' "src=$($ag15bdoc.budget.source)"

    # --- AG16: 并发 append 链不断 ---
    $ag16procs = @()
    for ($i = 1; $i -le 6; $i++) {
        $ag16procs += Start-Process -FilePath $agPy -ArgumentList @($atPy, 'append', '--store', $storeCon, `
            '--kind', 'spend', '--task-id', 'tc', '--agent-id', 'ac', '--runid', "r$i", '--claimed-tokens', '10') `
            -NoNewWindow -PassThru -RedirectStandardOutput (Join-Path $Work "ag-con$i.out") -RedirectStandardError (Join-Path $Work "ag-con$i.err")
    }
    foreach ($p in $ag16procs) { $null = $p.WaitForExit() }
    $ag16exits = @($ag16procs | ForEach-Object { $_.ExitCode })
    $ag16b = Invoke-At @('verify', '--store', $storeCon)
    Assert-True (@($ag16exits | Where-Object { $_ -ne 0 }).Count -eq 0) `
        'AG16 并发 6 append 全成功（锁窗口内排队，不拒不丢）' "exits=$($ag16exits -join ',')"
    Assert-True ($ag16b -eq 0 -and ($agOut -join ' ') -match 'chain OK \(6 entries') `
        'AG16b 并发产物链完整（seq 连续、无重键、逐条复算过）' (($agOut -join ' '))

    # --- AG17: import 收编后预算闸认旧账 ---
    & $agPy $ldPy -Mode add -Ledger (Join-Path $storeC 'cost-ledger.json') -Task 'av|agt3' -Key 'av|rimp1|spent' -Tokens 600 -Agent-Id 'imp9' 2>&1 | Out-Null
    $ag17imp = Invoke-At @('import', '--store', $storeC, '--from-ledger', (Join-Path $storeC 'cost-ledger.json'))
    $ag17 = Invoke-AgAv $AgA $storeC (Join-Path $Work 'ag-rc-imp.json') @('--task-id', 'agt3', '--budget-token', '1000')
    $ag17doc = Read-AgReceipt (Join-Path $Work 'ag-rc-imp.json')
    Assert-True ($ag17imp -eq 0 -and $ag17 -eq 0 -and $ag17doc.budget.recorded_tokens -eq 600 `
        -and $ag17doc.budget.remaining -eq 400) `
        'AG17 import 收编旧账→预算闸认（600 已认证，400=1000-600，旧账不算 air）' "imp=$ag17imp e=$ag17 rem=$($ag17doc.budget.remaining)"

    # --- AG18: 多次 spend 累计入闸 ---
    $ag18 = Invoke-AgAv $AgA $storeA (Join-Path $Work 'ag-rc-sp2.json') @('--task-id', 'agt1', '--spent-token', '888')
    $ag18b = Invoke-AgAv $AgA $storeA (Join-Path $Work 'ag-rc-bg3.json') @('--task-id', 'agt1', '--budget-token', '9999')
    $ag18doc = Read-AgReceipt (Join-Path $Work 'ag-rc-bg3.json')
    Assert-True ($ag18 -eq 0 -and $ag18b -eq 0 -and $ag18doc.budget.recorded_tokens -eq 1665 `
        -and $ag18doc.budget.remaining -eq 8334) `
        'AG18 两次 spend 累计（1665=777+888，链余额逐次累加不丢）' "rem=$($ag18doc.budget.remaining)"

    # --- AG15c: 链被篡改→预算闸 fail-closed（放最后：改坏 storeA 的账）---
    $agChain = Join-Path $storeA 'attest-ledger.jsonl'
    [IO.File]::WriteAllText($agChain,
        [IO.File]::ReadAllText($agChain, $agEnc).Replace('"claimed_tokens":777', '"claimed_tokens":111'), $agEnc)
    $ag15c = Invoke-AgAv $AgA $storeA (Join-Path $Work 'ag-rc-bg4.json') @('--task-id', 'agt1', '--budget-token', '5000')
    $ag15cdoc = Read-AgReceipt (Join-Path $Work 'ag-rc-bg4.json')
    Assert-True ($ag15c -eq 1 -and $ag15cdoc.verdict -eq 'refused by attest ledger') `
        'AG15c 链被篡改→预算闸 fail-closed 拒（闸不信任自己的账就不放行）' "e=$ag15c v=$($ag15cdoc.verdict)"
}


# =====================================================================================
"== AH. terminology: 概念契约（防偷换概念）——锚点漂移/混用/孤儿三类漂移机器可查 =="
# 术语表是概念契约：同一个词在权威定义、回执、断言之间悄悄换义是造假的温床。每条断言
# 对着一个真实失效模式：权威行被改写（ANCHOR-LOST）、代码挪行使锚点错位（ANCHOR-MOVED）、
# 某词与它不许混用的词在同一行出现（MIX）、登记处比词表活得久（ORPHAN）、棘轮被绕过
# （改定义不带 --reason）。纪律：本段里凡枚举术语或正反对照的行必须自带守卫词
# （严禁/不是/不许），否则本组会被自己钉住的 MIX 规则拦下——这正是被检验的行为。
$ahPy = $null
foreach ($c in @($env:PYTHON, 'python', 'python3', 'py')) {
    if (-not $c) { continue }
    $ahSrc = (Get-Command $c -ErrorAction SilentlyContinue).Source
    if (-not $ahSrc) { continue }
    if ((& $ahSrc -c 'print(1)' 2>$null) -eq '1') { $ahPy = $ahSrc; break }
}
if (-not $ahPy) {
    "  SKIP  AH0 找不到可用的 Python，跳过术语契约检查"
    $skipped += 26
} else {
    $termPy = Join-Path $tools 'terminology.py'
    $ahEnc = New-Object System.Text.UTF8Encoding($false)

    function Invoke-Terms {
        param([string]$root, [string]$registry, [string]$files)
        $argz = @($termPy, 'check', '--root', $root)
        if ($registry) { $argz += @('--registry', $registry) }
        if ($files) { $argz += @('--files', $files) }
        & $ahPy @argz 2>&1
    }

    # --- 夹具：按注册表 anchors 把权威文件逐字复制到临时根（复制真实文件来测漂移检出）---
    $ahTermsDoc = [IO.File]::ReadAllText((Join-Path $repo '.agent\terms.json'), $ahEnc) | ConvertFrom-Json
    $AhFx = Join-Path $Work 'ah-fixture'
    $ahFiles = @('docs\ERRORS.md', 'tools\selftest.ps1')
    foreach ($t in $ahTermsDoc.terms) {
        foreach ($a in $t.anchors) {
            $ahFiles += (($a -split ':', 2)[0] -replace '/', '\')
        }
    }
    foreach ($f in ($ahFiles | Select-Object -Unique)) {
        $dst = Join-Path $AhFx $f
        New-Item -ItemType Directory -Force -Path (Split-Path -Parent $dst) | Out-Null
        Copy-Item (Join-Path $repo $f) $dst -Force
    }
    $ahRegFx = Join-Path $AhFx 'reg.json'
    Copy-Item (Join-Path $repo '.agent\terms.json') $ahRegFx -Force
    function Copy-AhFixture([string]$dst) {
        New-Item -ItemType Directory -Force -Path $dst | Out-Null
        Copy-Item (Join-Path $AhFx '*') $dst -Recurse -Force
    }

    # AH1 全绿路径：真仓 check exit 0（默认面=护栏手册+ERRORS+tools/*.py+selftest 断言名）
    $null = Invoke-Terms $repo ''
    Assert-True ($LASTEXITCODE -eq 0) 'AH1 真仓 check 全绿（17 术语 35 锚点全部仍在权威位置）' "exit=$LASTEXITCODE"

    # AH2/AH3 契约形状
    Assert-True ($ahTermsDoc.terms.Count -ge 12) 'AH2 注册表至少 12 条术语（概念契约不许缩水）' "n=$($ahTermsDoc.terms.Count)"
    $ahShapeOk = $true
    foreach ($t in $ahTermsDoc.terms) {
        if (-not $t.term -or -not $t.definition -or -not $t.status -or @($t.anchors).Count -lt 1) { $ahShapeOk = $false }
        if ($t.mix -notin @('co', 'forms', 'exclusive', 'none')) { $ahShapeOk = $false }
    }
    Assert-True $ahShapeOk 'AH3 每条术语 term/definition/status/anchors/mix 齐全（登记处形状钉死）' ''

    # AH4/AH4b 核心概念逐一在册（枚举行自带守卫词，防自吞 MIX）
    # 词与词严禁混用，所以下面两行枚举同线出现不构成混用——守卫词就是给枚举行的。
    $ahCore = @('proven', 'censored', 'failed', 'sound', 'declared', 'wall', 'token', 'caps', 'owner_root', 'change', 'verdict', 'fail-closed', 'ratchet', 'gate', 'triage', 'worktree')  # 词表本体：词与词严禁混用，同线枚举不是偷换
    $ahMissing = @($ahCore | Where-Object { $ahTermsDoc.terms.term -notcontains $_ })
    Assert-True ($ahMissing.Count -eq 0) 'AH4 核心概念逐一在册（16 个，词与词严禁混用：proven、censored、failed、sound、declared、wall、token、caps、owner_root、change、verdict、fail-closed、ratchet、gate、triage、worktree）' ($ahMissing -join ',')
    Assert-True ($ahTermsDoc.terms.term -contains '已跑通') 'AH4b 完成度状态词（已跑通/已实现未测/组件/已否证）的代表词在册——声明审计词族' ''

    # AH4c 锚点逐一真实存在：文件在、行号对、引文逐字落在登记的行上（登记处不说谎）
    $ahAnchorBad = @()
    foreach ($t in $ahTermsDoc.terms) {
        foreach ($a in $t.anchors) {
            $p1 = $a -split ':', 2
            $p2 = $p1[1] -split ':', 2
            $apath = Join-Path $repo ($p1[0] -replace '/', '\')
            if (-not ([IO.File]::Exists($apath))) { $ahAnchorBad += $a; continue }
            $aline = [IO.File]::ReadAllLines($apath, $ahEnc)
            if ([int]$p2[0] -gt $aline.Count -or -not $aline[[int]$p2[0] - 1].Contains($p2[1])) { $ahAnchorBad += $a }
        }
    }
    Assert-True ($ahAnchorBad.Count -eq 0) 'AH4c 每条锚点：文件存在且引文逐字落在登记的行上' ($ahAnchorBad -join ' | ')

    # AH5/AH6 混用正反例：夹具文本把 censored 冒充 failed——不许（MIX 必中）；带守卫的对照行放行
    $ahTheft = 'resume ledger status: censored means the task already ' + 'fai' + 'led completely'
    New-File (Join-Path $AhFx 'ah-bad.md') ($ahTheft + $nl)
    $ahBadOut = @(Invoke-Terms $AhFx $ahRegFx 'ah-bad.md')
    Assert-True ($LASTEXITCODE -eq 1 -and ((@($ahBadOut) -join "`n") -match 'MIX censored')) `
        'AH5 混用正例：censored 不许写成 failed——这样的文本必中（exit 1）' "exit=$LASTEXITCODE"
    New-File (Join-Path $AhFx 'ah-good.md') "censored is not a failure; the resume ledger has no failed status$nl"
    $null = Invoke-Terms $AhFx $ahRegFx 'ah-good.md'
    Assert-True ($LASTEXITCODE -eq 0) 'AH6 混用反例：对照行（censored 不是 failed）不误报' "exit=$LASTEXITCODE"

    # AH7 锚点失效：把权威文件里的定义行改坏（正文里把"不是失败"改成"是失败"的真实偷换形态）
    $AhLost = Join-Path $Work 'ah-lost'
    Copy-AhFixture $AhLost
    $ahRp = Join-Path $AhLost 'tools\resume.py'
    $ahOldTxt = [IO.File]::ReadAllText($ahRp, $ahEnc)
    $ahNewTxt = $ahOldTxt.Replace('explicitly not a failure', 'explicitly a failure')
    [IO.File]::WriteAllText($ahRp, $ahNewTxt, $ahEnc)
    $ahLostOut = @(Invoke-Terms $AhLost (Join-Path $AhLost 'reg.json'))
    Assert-True ($LASTEXITCODE -eq 1 -and ((@($ahLostOut) -join "`n") -match 'ANCHOR-LOST censored')) `
        'AH7 锚点失效：权威行被改写必报 ANCHOR-LOST（exit 1）' "exit=$LASTEXITCODE"

    # AH8 锚点移位：引文还在但代码挪行，锚点行号错位必报
    $AhMoved = Join-Path $Work 'ah-moved'
    Copy-AhFixture $AhMoved
    $ahRp2 = Join-Path $AhMoved 'tools\resume.py'
    $ahKept = @([IO.File]::ReadAllLines($ahRp2, $ahEnc) | Where-Object { $_ -notmatch '^Why censoring is not failure' })
    [IO.File]::WriteAllLines($ahRp2, $ahKept, $ahEnc)
    $ahMovedOut = @(Invoke-Terms $AhMoved (Join-Path $AhMoved 'reg.json'))
    Assert-True ($LASTEXITCODE -eq 1 -and ((@($ahMovedOut) -join "`n") -match 'ANCHOR-MOVED censored')) `
        'AH8 锚点移位：引文仍在但不在登记行必报 ANCHOR-MOVED（exit 1）' "exit=$LASTEXITCODE"

    # AH9 孤儿术语：登记处收下死词，check 审出"登记处比词表活得久"。
    # 死词与匹配串都拆形构造（L106 口径）——夹具里的 selftest.ps1 副本若含连续词面，
    # 孤儿检查会把它当成"词还活着"，检验就被检验者自己放走了。
    $AhOrp = Join-Path $Work 'ah-orphan'
    Copy-AhFixture $AhOrp
    $ahOrpReg = Join-Path $AhOrp 'reg.json'
    $ahDeadWord = 'zzz-orphan-' + 'qx'
    $null = & $ahPy $termPy add --root $AhOrp --registry $ahOrpReg --term $ahDeadWord --definition 'a word the vocabulary forgot' --anchor 'tools/resume.py:23:censored   closed without finishing -- explicitly not a failure' 2>&1
    Assert-True ($LASTEXITCODE -eq 0) 'AH9a add 死词收下（锚点仍真实，先登记再由 check 审腐烂）' "exit=$LASTEXITCODE"
    $ahOrpOut = @(Invoke-Terms $AhOrp $ahOrpReg)
    Assert-True ($LASTEXITCODE -eq 1 -and ((@($ahOrpOut) -join "`n") -match ('ORPHAN ' + $ahDeadWord))) `
        'AH9 孤儿术语：词在代码/扫描面/家都不存在必报（登记处腐烂机器可查）' "exit=$LASTEXITCODE"

    # AH10 棘轮：概念契约只紧不松
    $AhRat = Join-Path $Work 'ah-ratchet'
    Copy-AhFixture $AhRat
    $ahRatReg = Join-Path $AhRat 'reg.json'
    $null = & $ahPy $termPy update --root $AhRat --registry $ahRatReg --term censored --definition 'new meaning, same word' 2>&1
    Assert-True ($LASTEXITCODE -eq 2) 'AH10 definition 改动缺 --reason 直接拒绝（概念不许悄悄换义）' "exit=$LASTEXITCODE"
    $null = & $ahPy $termPy update --root $AhRat --registry $ahRatReg --term censored --forbidden 'failure' 2>&1  # 收窄禁词不许：基线只紧不松
    Assert-True ($LASTEXITCODE -eq 2) 'AH10b forbidden 收窄缺 --reason 直接拒绝（基线只紧不松）' "exit=$LASTEXITCODE"
    $null = & $ahPy $termPy update --root $AhRat --registry $ahRatReg --term censored --definition 'new meaning, same word' --reason '口径由 owner 明示更新' 2>&1
    Assert-True ($LASTEXITCODE -eq 0) 'AH10c definition 改动带 --reason 放行' "exit=$LASTEXITCODE"
    $ahHist = [IO.File]::ReadAllText($ahRatReg, $ahEnc) | ConvertFrom-Json
    Assert-True (@($ahHist.history | Where-Object { $_.term -eq 'censored' -and $_.reason -match 'owner' }).Count -ge 1) `
        'AH10d 改动理由落进 history（概念变动可审计）' ''

    # AH11 add 的两道 fail-closed
    $AhAdd = Join-Path $Work 'ah-add'
    Copy-AhFixture $AhAdd
    $ahAddReg = Join-Path $AhAdd 'reg.json'
    $null = & $ahPy $termPy add --root $AhAdd --registry $ahAddReg --term censored --definition dup --anchor 'tools/resume.py:23:censored   closed without finishing -- explicitly not a failure' 2>&1
    Assert-True ($LASTEXITCODE -eq 2) 'AH11 add 重复术语拒绝（术语不许静默重定义）' "exit=$LASTEXITCODE"
    $null = & $ahPy $termPy add --root $AhAdd --registry $ahAddReg --term never-was --definition x --anchor 'tools/resume.py:999:this line does not exist' 2>&1
    Assert-True ($LASTEXITCODE -eq 2) 'AH11b add 假锚点 fail-closed 拒绝（登记处永不记谎）' "exit=$LASTEXITCODE"

    # AH12/AH13 exit 2 两面：坏注册表 fail-closed、扫描文件缺失
    $AhBadReg = Join-Path $Work 'ah-badreg'
    New-Item -ItemType Directory -Force -Path (Join-Path $AhBadReg '.agent') | Out-Null
    [IO.File]::WriteAllText((Join-Path $AhBadReg '.agent\terms.json'), '{bad json', $ahEnc)
    $ahBadRegOut = @(Invoke-Terms $AhBadReg '')
    Assert-True ($LASTEXITCODE -eq 2 -and ((@($ahBadRegOut) -join ' ') -match 'fail closed') -and ((@($ahBadRegOut) -join ' ') -notmatch 'Traceback')) `
        'AH12 坏注册表 JSON fail-closed：干净拒绝不是崩溃栈（exit 2）' "exit=$LASTEXITCODE"
    $null = Invoke-Terms $AhFx $ahRegFx 'ah-nope.md'
    Assert-True ($LASTEXITCODE -eq 2) 'AH13 扫描文件缺失 exit 2（文件缺失与漂移分开记账）' "exit=$LASTEXITCODE"

    # AH14/AH15 机读面与精确计数
    $ahJsonOut = (& $ahPy $termPy check --root $repo --json 2>&1 | Out-String).Trim()
    Assert-True ($LASTEXITCODE -eq 0 -and $ahJsonOut.IndexOf("`n") -lt 0 -and $ahJsonOut -notmatch '[^\x00-\x7F]') `
        'AH14 --json 单行纯 ASCII 机读（任何解码器都读不坏）' ''
    $ahCleanOut = @(Invoke-Terms $repo '')
    Assert-True ($LASTEXITCODE -eq 0 -and (@($ahCleanOut | Where-Object { $_ -match 'terms=17 anchors=35 drift=0' }).Count -eq 1)) `
        'AH15 全绿报表精确计数行 terms=17 anchors=35 drift=0（数字是契约的一部分）' (@($ahCleanOut | Select-Object -Last 2) -join ' / ')

    # AH16/AH16b 认证与声明：外来词形态必中；严禁混用的对照行放行
    $ahFw = 'the budget is ' + 'att' + 'ested by the ledger'
    New-File (Join-Path $AhFx 'ah-foreign.md') ($ahFw + $nl)
    $ahForOut = @(Invoke-Terms $AhFx $ahRegFx 'ah-foreign.md')
    Assert-True ($LASTEXITCODE -eq 1 -and ((@($ahForOut) -join ' ') -match 'MIX declared')) `
        'AH16 认证账本冒充声明制：声明制条目的禁词形态必中（exclusive）' "exit=$LASTEXITCODE"
    New-File (Join-Path $AhFx 'ah-foreign-ok.md') "attested 与 declared 严禁混用：声明是声明，认证是认证$nl"
    $null = Invoke-Terms $AhFx $ahRegFx 'ah-foreign-ok.md'
    Assert-True ($LASTEXITCODE -eq 0) 'AH16b 严禁混用的对照行不误报' "exit=$LASTEXITCODE"

    # AH17 wall 被写成模型侧消耗的形态必中（秒与 token 不许互相冒充）
    $ahWtTheft = 'the cost note: wall is ' + 'mod' + 'el-side usage, tokens included'
    New-File (Join-Path $AhFx 'ah-wall.md') ($ahWtTheft + $nl)
    $ahWtOut = @(Invoke-Terms $AhFx $ahRegFx 'ah-wall.md')
    Assert-True ($LASTEXITCODE -eq 1 -and ((@($ahWtOut) -join ' ') -match 'MIX wall')) `
        'AH17 wall 被写成模型侧消耗的形态必中（wall 与 token 不许互相冒充）' "exit=$LASTEXITCODE"

    # AH18 默认扫描面宽度（护栏手册+ERRORS+tools/*.py+selftest 断言名）
    $ahFilesN = if (@($ahCleanOut | Where-Object { $_ -match 'terms: files=(\d+)' })) { [int]$Matches[1] } else { 0 }
    Assert-True ($ahFilesN -ge 25) 'AH18 默认扫描面至少 25 个文件（文档+工具 usage/docstring+selftest 断言名都在面上）' "files=$ahFilesN"
}

""
"== AI. doctor: 项目自感知快照 -- 聚合有出处的证据面，不是闸 =="
# doctor.py 读的是落盘证据：selftest 汇总记录（本文件尾部写入）、台账固化列 vs selftest
# 断言号（双向对账：台账声称的断言号必须真实存在；selftest 引用的 L 号必须还在台账里）、
# registry 可跑性、欠账+git、privacy/docs 门。任何一面 FAIL 都不改变 doctor 的退出码——
# 它是观测面，判定闸在 policy/constraints。台账回归清点的真实缺口数记录在 docs/ERRORS.md L118。
$aiPy = $null
foreach ($c in @($env:PYTHON, 'python', 'python3', 'py')) {
    if (-not $c) { continue }
    $aiSrc = (Get-Command $c -ErrorAction SilentlyContinue).Source
    if (-not $aiSrc) { continue }
    if ((& $aiSrc -c 'print(1)' 2>$null) -eq '1') { $aiPy = $aiSrc; break }
}
$aiTotal = 30
if (-not $aiPy) {
    "  SKIP  AI0 找不到可用的 Python，跳过 doctor 检查"
    $skipped += $aiTotal
} else {
    $aiEnc = New-Object System.Text.UTF8Encoding($false)
    $aiDocRoot = Join-Path $Work 'ai-doctor'
    $aiG = Join-Path $aiDocRoot 'g-clean'
    $aiD = Join-Path $aiDocRoot 'd-debt'
    $aiX = Join-Path $aiDocRoot 'x-ghost'
    $aiF = Join-Path $aiDocRoot 'f-record'
    $aiP = Join-Path $aiDocRoot 'p-gates'
    # 夹具 selftest 基础内容只含 W13/W14；反漂移夹具（$aiX 专用）才引用一个 fixture 台账里
    # 不存在的 L 号——用运行期拼接写出 'L77'，真 selftest.ps1 源码里不出现该字面量，
    # 真仓反向对账不会把它当成真的引用（L106 同法）。
    $aiSelfFx = ("Assert-True (`$true) 'W13 sample pin' ''`n" +
                 "Assert-True (`$true) 'W14 sample pin two' ''`n")
    $aiSelfFxDrift = ($aiSelfFx + "# reverse pin: docs/ERRORS.md L" + "77`n")
    function New-AiRoot([string]$root) {
        New-Item -ItemType Directory -Force -Path $root | Out-Null
    }
    function New-AiLedgerRoot([string]$root, [string]$ledgerBody, [string]$registryJson) {
        New-Item -ItemType Directory -Force -Path `
            (Join-Path $root 'docs'), (Join-Path $root 'tools'), (Join-Path $root '.agent\checks') | Out-Null
        [IO.File]::WriteAllText((Join-Path $root 'docs\ERRORS.md'), $ledgerBody, $script:aiEnc)
        [IO.File]::WriteAllText((Join-Path $root 'tools\selftest.ps1'), $script:aiSelfFx, $script:aiEnc)
        if ($registryJson) {
            [IO.File]::WriteAllText((Join-Path $root '.agent\checks\registry.json'),
                ($registryJson + "`n"), $script:aiEnc)
        }
    }
    function Invoke-AiDoctor([string]$root) {
        return @(& $script:aiPy (Join-Path $script:tools 'doctor.py') --root $root --json 2>&1)
    }
    function Get-AiFace($doc, [string]$name) {
        return @($doc.faces | Where-Object { $_.face -eq $name })[0]
    }

    # 三种台账形态：无债务 / 有债务 / 断言号被删（任务要求的形态各验一次）
    $aiLedGood = @(
        '# fixture: clean ledger', '',
        '## A. fixture', '',
        '| # | a | b | c | pin |',
        '|---|---|---|---|---|',
        '| L01 | x | y | z | W13 |',
        '| L02 | x | y | z | W14 |') -join "`n"
    $aiLedDebt = @(
        '# fixture: debt ledger', '',
        '## A. fixture', '',
        '| # | a | b | c | pin |',
        '|---|---|---|---|---|',
        '| L01 | x | y | z | - |',
        '| L02 | x | y | z | N 组 |', '',
        '## D. fixture backlog', '',
        '| # | debt | now | plan |',
        '|---|---|---|---|',
        '| L37 | x | y | z |') -join "`n"
    $aiLedGhost = @(
        '# fixture: ghost ledger', '',
        '## A. fixture', '',
        '| # | a | b | c | pin |',
        '|---|---|---|---|---|',
        '| L01 | x | y | z | W99 |',
        '| L02 | x | y | z | C999 |') -join "`n"

    New-AiRoot $aiF | Out-Null
    New-AiLedgerRoot $aiG $aiLedGood '[{"id":"C901","kind":"require","scope":["docs/ERRORS.md"]}]'
    New-AiLedgerRoot $aiD $aiLedDebt ('[{"id":"C901","kind":"require","scope":["docs/ERRORS.md"]},' +
        '{"id":"C902","kind":"cmd","run":"ai-missing-cmd-xyz --flag"}]')
    New-AiLedgerRoot $aiX $aiLedGhost '[{"id":"C901","kind":"require","scope":["docs/ERRORS.md"]}]'
    # $aiX 专用：selftest 引用 fixture 台账里不存在的 L 号 → 反向漂移（AI9）
    [IO.File]::WriteAllText((Join-Path $aiX 'tools\selftest.ps1'), $aiSelfFxDrift, $aiEnc)

    # --- UNAVAILABLE 形态：空根，什么证据都没有，六个面全部如实缺席 ---
    $aiE = Join-Path $aiDocRoot 'e-empty'
    New-AiRoot $aiE | Out-Null
    $aiOut1 = Invoke-AiDoctor $aiE
    $aiExit1 = $LASTEXITCODE
    $aiDoc1 = ($aiOut1 -join "`n") | ConvertFrom-Json
    Assert-True ($aiExit1 -eq 0 -and $null -ne $aiDoc1.faces) 'AI1 空根：exit=0 且 JSON 可解析（缺证据不崩）' "exit=$aiExit1"
    $aiUnav = @($aiDoc1.faces | Where-Object { $_.status -eq 'UNAVAILABLE' })
    Assert-True (@($aiDoc1.faces).Count -eq 6 -and $aiUnav.Count -eq 6) 'AI1b 空根：六面全部 UNAVAILABLE（缺出处不许冒充绿）' (($aiDoc1.faces | ForEach-Object { "$($_.face)=$($_.status)" }) -join ',')
    $aiSelf1 = Get-AiFace $aiDoc1 'selftest'
    Assert-True ($aiSelf1.detail -match 'rerun') 'AI2 selftest 面缺记录时点名重跑命令' $aiSelf1.detail

    # --- last-selftest.json 落盘往返：写入用与文件尾部同一个函数（写读同源） ---
    Write-SelftestRecord -Root $aiG -Passed 447 -Failed 0 -Skipped 3
    $aiOutG = Invoke-AiDoctor $aiG
    $aiExitG = $LASTEXITCODE
    $aiDocG = ($aiOutG -join "`n") | ConvertFrom-Json
    $aiSelfG = Get-AiFace $aiDocG 'selftest'
    Assert-True ($aiExitG -eq 0 -and $aiSelfG.status -eq 'OK' -and
        $aiSelfG.detail -match 'passed=447 failed=0' -and $aiSelfG.evidence -match 'last-selftest\.json') `
        'AI3 落盘往返：Write-SelftestRecord 写入后 doctor 读回计数与证据指针' ("exit=$aiExitG $($aiSelfG.status) $($aiSelfG.detail)")

    Write-SelftestRecord -Root $aiF -Passed 4 -Failed 3 -Skipped 0
    $aiOutF = Invoke-AiDoctor $aiF
    $aiExitF = $LASTEXITCODE
    $aiDocF = ($aiOutF -join "`n") | ConvertFrom-Json
    $aiSelfF = Get-AiFace $aiDocF 'selftest'
    Assert-True ($aiSelfF.status -eq 'FAIL' -and $aiExitF -eq 0) 'AI4 selftest FAIL 面成立但 exit 仍 0（FAIL 不阻塞 exit，观测面不是闸）' "exit=$aiExitF status=$($aiSelfF.status)"

    New-Item -ItemType Directory -Force -Path (Join-Path $aiX '.agent') | Out-Null
    [IO.File]::WriteAllText((Join-Path $aiX '.agent\last-selftest.json'), '{bad', $aiEnc)
    $aiDocX1 = ((Invoke-AiDoctor $aiX) -join "`n") | ConvertFrom-Json
    $aiSelfX1 = Get-AiFace $aiDocX1 'selftest'
    Assert-True ($aiSelfX1.status -eq 'UNAVAILABLE') 'AI5 坏 JSON 的 selftest 记录：UNAVAILABLE（不是崩也不是绿）' $aiSelfX1.detail

    # --- 台账三形态 ---
    $aiLedG = Get-AiFace $aiDocG 'ledger'
    Assert-True ($aiLedG.status -eq 'OK' -and $aiLedG.detail -match 'entries=2 pinned=2 unpinned=0 ghost=0') `
        'AI6 无债务台账：ledger OK（固化列里的断言号在 selftest 里真实存在）' $aiLedG.detail

    $aiDocD = ((Invoke-AiDoctor $aiD) -join "`n") | ConvertFrom-Json
    $aiLedD = Get-AiFace $aiDocD 'ledger'
    Assert-True ($aiLedD.status -eq 'WARN' -and $aiLedD.detail -match 'unpinned=2 ghost=0' -and $aiLedD.detail -match 'group_only=1') `
        'AI7 有债务台账：空固化与组号引用都算缺口（组号不是断言钉住，缺口是已知债务不是失败）' $aiLedD.detail

    $aiDocX = ((Invoke-AiDoctor $aiX) -join "`n") | ConvertFrom-Json
    $aiExitX = $LASTEXITCODE
    $aiLedX = Get-AiFace $aiDocX 'ledger'
    Assert-True ($aiLedX.status -eq 'FAIL' -and $aiExitX -eq 0 -and $aiLedX.detail -match 'ghost=L01:W99;L02:C999') `
        'AI8 断言号被删：selftest 与 registry 都不认领的幽灵号让 ledger FAIL，exit 仍 0' ("exit=$aiExitX $($aiLedX.detail)")
    Assert-True ($aiLedX.detail -match 'reverse_missing=1' -and $aiLedX.detail -match 'reverse=L77') `
        'AI9 反向漂移：selftest 引用的 L 号不在台账里同样 FAIL（台账不许变薄）' $aiLedX.detail

    # --- registry 面 ---
    $aiRegG = Get-AiFace $aiDocG 'registry'
    Assert-True ($aiRegG.status -eq 'OK' -and $aiRegG.detail -match 'checks=1 runnable=1') 'AI10 registry 面：全部可跑时 OK' $aiRegG.detail
    $aiRegD = Get-AiFace $aiDocD 'registry'
    Assert-True ($aiRegD.status -eq 'WARN' -and $aiRegD.detail -match 'checks=2' -and $aiRegD.detail -match 'C902:command missing') `
        'AI11 registry 面：命令缺失的检查被点名（存在性探测，不执行任何检查）' $aiRegD.detail
    New-Item -ItemType Directory -Force -Path (Join-Path $aiF '.agent\checks') | Out-Null
    [IO.File]::WriteAllText((Join-Path $aiF '.agent\checks\registry.json'), '{oops', $aiEnc)
    $aiDocF2 = ((Invoke-AiDoctor $aiF) -join "`n") | ConvertFrom-Json
    $aiRegF = Get-AiFace $aiDocF2 'registry'
    Assert-True ($aiRegF.status -eq 'UNAVAILABLE') 'AI12 坏 JSON 的 registry：UNAVAILABLE（半份数据不冒充全貌）' $aiRegF.detail

    # --- backlog / git / gates 面 ---
    $aiBkD = Get-AiFace $aiDocD 'backlog'
    Assert-True ($aiBkD.status -eq 'WARN' -and $aiBkD.detail -match 'd_section=1' -and $aiBkD.detail -match 'L37') `
        'AI13 欠账节单列的条目进 backlog 面（D 节不许混进已固化）' $aiBkD.detail
    $aiBkG = Get-AiFace $aiDocG 'backlog'
    Assert-True ($aiBkG.status -eq 'OK') 'AI14 无欠账台账：backlog 面 OK' $aiBkG.detail
    $aiGitD = Get-AiFace $aiDocD 'git'
    Assert-True ($aiGitD.status -eq 'UNAVAILABLE') 'AI15 无仓库：git 面 UNAVAILABLE' $aiGitD.detail

    # gates OK 形态与 git 仓库一起备好：门脚本 + 合规 CHANGELOG 先落盘并提交；两道门各自
    # 就地建档（docs_hygiene --update 建棘轮基线、privacy --init 冻结存量——夹具里的
    # docs_hygiene.py 盘符命中在真仓是被 privacy 基线冻结的，夹具同口径）；建档产物二次
    # 提交，之后 doctor 才能断言 dirty=0 与两门皆绿。
    New-Item -ItemType Directory -Force -Path (Join-Path $aiG 'scripts') | Out-Null
    Copy-Item (Join-Path $repo 'scripts\privacy_scan.py') (Join-Path $aiG 'scripts\privacy_scan.py')
    Copy-Item (Join-Path $repo 'scripts\docs_hygiene.py') (Join-Path $aiG 'scripts\docs_hygiene.py')
    [IO.File]::WriteAllText((Join-Path $aiG 'CHANGELOG.md'), ("# Changelog`n`n## [Unreleased]`nfixture`n"), $aiEnc)
    Push-Location $aiG
    & git init -q 2>&1 | Out-Null
    & git add -A 2>&1 | Out-Null
    & git -c user.email=s@t -c user.name=s commit -qm base 2>&1 | Out-Null
    & $aiPy (Join-Path $aiG 'scripts\docs_hygiene.py') --update 2>&1 | Out-Null
    & $aiPy (Join-Path $aiG 'scripts\privacy_scan.py') --init 2>&1 | Out-Null
    & git add -A 2>&1 | Out-Null
    & git -c user.email=s@t -c user.name=s commit -qm baselines 2>&1 | Out-Null
    Pop-Location
    $aiOutG2 = Invoke-AiDoctor $aiG
    $aiDocG2 = ($aiOutG2 -join "`n") | ConvertFrom-Json
    $aiGitG = Get-AiFace $aiDocG2 'git'
    Assert-True ($aiGitG.status -eq 'WARN' -and $aiGitG.detail -match 'dirty=0' -and $aiGitG.detail -match 'unpushed=1') `
        'AI16 无远端的本地分支算未推送（tip 不在任何 remote ref 上，事实口径不是猜测）' $aiGitG.detail
    $aiGatesG = Get-AiFace $aiDocG2 'gates'
    Assert-True ($aiGatesG.status -eq 'OK' -and $aiGatesG.evidence -match 'privacy exit=0' -and $aiGatesG.evidence -match 'docs exit=0') `
        'AI17 privacy/docs 门现场快跑：都绿则 gates OK（秒级，证据就是退出码）' $aiGatesG.evidence

    [IO.File]::WriteAllText((Join-Path $aiG 'dirty.txt'), "d`n", $aiEnc)
    $aiOutG3 = Invoke-AiDoctor $aiG
    $aiDocG3 = ($aiOutG3 -join "`n") | ConvertFrom-Json
    $aiGitG3 = Get-AiFace $aiDocG3 'git'
    Assert-True ($aiGitG3.detail -match 'dirty=1') 'AI18 未跟踪文件进 dirty 计数（git 采集，不是状态文件）' $aiGitG3.detail

    New-Item -ItemType Directory -Force -Path (Join-Path $aiP 'scripts'), (Join-Path $aiP 'docs\plans') | Out-Null
    Copy-Item (Join-Path $repo 'scripts\privacy_scan.py') (Join-Path $aiP 'scripts\privacy_scan.py')
    [IO.File]::WriteAllText((Join-Path $aiP 'docs\plans\violation.md'), "task doc`n", $aiEnc)
    Push-Location $aiP
    & git init -q 2>&1 | Out-Null
    & git add -A 2>&1 | Out-Null
    & git -c user.email=s@t -c user.name=s commit -qm base 2>&1 | Out-Null
    Pop-Location
    $aiDocP = ((Invoke-AiDoctor $aiP) -join "`n") | ConvertFrom-Json
    $aiGatesP = Get-AiFace $aiDocP 'gates'
    Assert-True ($aiGatesP.status -eq 'FAIL' -and $aiGatesP.evidence -match 'privacy exit=1') `
        'AI19 privacy 门红时 gates FAIL（doctor 如实转述门禁退出码，不加自己的意见）' $aiGatesP.evidence
    $aiGatesD = Get-AiFace $aiDocD 'gates'
    Assert-True ($aiGatesD.status -eq 'UNAVAILABLE') 'AI20 门脚本缺失：gates UNAVAILABLE（不是绿）' $aiGatesD.detail

    # --- --json 机器可读契约 ---
    $aiLine = @($aiOutG3)[0]
    $aiFaceKeysOk = $true
    $aiStatusOk = $true
    foreach ($f in $aiDocG3.faces) {
        $fn = @($f.PSObject.Properties.Name)
        if (($fn -notcontains 'face') -or ($fn -notcontains 'status') -or
            ($fn -notcontains 'evidence') -or ($fn -notcontains 'detail')) { $aiFaceKeysOk = $false }
        if (@('OK', 'WARN', 'FAIL', 'UNAVAILABLE') -notcontains $f.status) { $aiStatusOk = $false }
    }
    Assert-True ((@($aiOutG3).Count -eq 1) -and ((($aiLine -replace '[^\x00-\x7F]', '').Length) -eq $aiLine.Length)) `
        'AI21 --json 单行且纯 ASCII（机器可读契约，任何宿主都读不坏）' "lines=$(@($aiOutG3).Count)"
    Assert-True ((@($aiDocG3.PSObject.Properties.Name) -contains 'tool') -and
        (@($aiDocG3.PSObject.Properties.Name) -contains 'at') -and
        (@($aiDocG3.PSObject.Properties.Name) -contains 'faces') -and
        $aiFaceKeysOk -and $aiStatusOk) `
        'AI21b --json 形状：tool/at/faces，每面 face/status/evidence/detail，status 取值封闭' ''

    # --- 真仓一次实跑：先落一份当前真实计数快照（与文件尾部同一函数），再读回清点 ---
    $aiP0 = $passed; $aiF0 = $failed; $aiS0 = $skipped
    Write-SelftestRecord -Root $repo -Passed $aiP0 -Failed $aiF0 -Skipped $aiS0
    $aiSw = [System.Diagnostics.Stopwatch]::StartNew()
    $aiOutR = Invoke-AiDoctor $repo
    $aiSw.Stop()
    $aiExitR = $LASTEXITCODE
    $aiDocR = ($aiOutR -join "`n") | ConvertFrom-Json
    Assert-True ($aiExitR -eq 0) 'AI22 真仓 doctor exit=0' "exit=$aiExitR"
    $aiLedR = Get-AiFace $aiDocR 'ledger'
    $aiPinned = if ($aiLedR.detail -match 'pinned=(\d+)') { [int]$Matches[1] } else { -1 }
    $aiEntries = if ($aiLedR.detail -match 'entries=(\d+)') { [int]$Matches[1] } else { -1 }
    Assert-True ($aiLedR.detail -match 'ghost=0' -and $aiLedR.detail -match 'reverse_missing=0') `
        'AI23 真仓台账：无幽灵断言号、selftest 引用的 L 号全部存在（双向不漂移）' $aiLedR.detail
    Assert-True ($aiPinned -ge 73 -and $aiEntries -ge 108) `
        'AI24 台账回归清点棘轮：有固化断言号的条目只增不减（首次真实清点数见 docs/ERRORS.md L118）' "pinned=$aiPinned entries=$aiEntries"
    $aiRegR = Get-AiFace $aiDocR 'registry'
    $aiRegCount = if ($aiRegR.detail -match 'checks=(\d+)') { [int]$Matches[1] } else { -1 }
    Assert-True (($aiRegR.status -eq 'OK' -or $aiRegR.status -eq 'WARN') -and $aiRegCount -ge 45) `
        'AI25 真仓 registry 检查可清点（本机缺依赖的条目 WARN，不算崩不算 UNAVAILABLE）' "$($aiRegR.status) $($aiRegR.detail)"
    $aiSelfR = Get-AiFace $aiDocR 'selftest'
    Assert-True ($aiSelfR.detail -match ('passed=' + $aiP0 + ' failed=' + $aiF0)) `
        'AI26 真仓 selftest 记录往返：doctor 读到的计数与落盘时刻的快照一致' $aiSelfR.detail
    $aiBkR = Get-AiFace $aiDocR 'backlog'
    Assert-True ($aiBkR.status -eq 'WARN' -and $aiBkR.detail -match 'L37') `
        'AI27 真仓欠账节在场且被清点（D 节不许悄悄消失）' $aiBkR.detail
    Assert-True ($aiSw.Elapsed.TotalSeconds -lt 60) `
        'AI28 doctor 秒级（不跑 cargo/selftest，只读落盘 + 轻量门）' ("seconds=" + [math]::Round($aiSw.Elapsed.TotalSeconds, 1))
}


# =====================================================================================
"== AJ. 声明分解 + 独立复核：聚合声明过闸前必须拆条，同一声明两条独立证据路径都要走通 =="
# 出处写进 claim_audit usage（台账 L120）：①过程监督优于结果监督（OpenAI "Let's Verify Step
# by Step", 2023）——聚合判定只吃逐原子判定，"全绿"一句话没有资格过闸；②独立复推
# （arXiv:2610.12234 式）——第三方不是"帮看看"，是重新推导再 diff：最近回执一条路、
# 盘上现实 + 台账运行痕迹另一条路，两源一致才 dual-source，矛盾降级 FABRICATED。
$ajPy = $null
foreach ($c in @($env:PYTHON, 'python', 'python3', 'py')) {
    if (-not $c) { continue }
    $ajSrc = (Get-Command $c -ErrorAction SilentlyContinue).Source
    if (-not $ajSrc) { continue }
    if ((& $ajSrc -c 'print(1)' 2>$null) -eq '1') { $ajPy = $ajSrc; break }
}
$ajTotal = 27
if (-not $ajPy) {
    "  SKIP  AJ0 找不到可用的 Python，跳过声明分解与独立复核检查"
    $skipped += $ajTotal
} else {
    $caPy = Join-Path $tools 'claim_audit.py'
    $avPy = Join-Path $tools 'agent_verify.py'
    $ldPy = Join-Path $tools 'ledger.py'
    $ajEnc = New-Object System.Text.UTF8Encoding($false)

    function Invoke-AjCa([string[]]$caArgs) {
        $script:ajOut = @(& $script:ajPy $caPy @caArgs 2>&1)
        return $LASTEXITCODE
    }
    function Get-AjSha8([string]$s) {
        $h = [System.Security.Cryptography.SHA256]::Create().ComputeHash([Text.Encoding]::UTF8.GetBytes($s))
        return ([BitConverter]::ToString($h) -replace '-', '').Substring(0, 8).ToLowerInvariant()
    }
    function New-AjFixture([string]$root) {
        New-Item -ItemType Directory -Force -Path (Join-Path $root 'pkg'), (Join-Path $root 'tests') | Out-Null
        New-File (Join-Path $root 'pkg\core.py') "def f(): return 1$nl"
        New-File (Join-Path $root 'pkg\__init__.py') ''
        New-File (Join-Path $root 'tests\test_core.py') "from pkg.core import f${nl}def test_f(): assert f() == 1$nl"
        New-File (Join-Path $root 'conftest.py') ''
        Push-Location $root
        & git init -q 2>&1 | Out-Null
        & git add -A 2>&1 | Out-Null
        & git -c user.email=a@b -c user.name=t commit -qm init 2>&1 | Out-Null
        Pop-Location
    }
    function Invoke-AjAv([string]$root, [string]$store, [string]$receipt, [string[]]$extra) {
        $argsAv = @($avPy, '-Root', $root, '-Changed', (Join-Path $root 'pkg\core.py'), '-Lang', 'python',
                    '-Plan', (Join-Path $root 'plan.json'), '-Store', $store, '-Receipt', $receipt) + $extra
        & $script:ajPy @argsAv 2>&1 | Out-Null
        return $LASTEXITCODE
    }
    function Read-AjReceipt([string]$path) {
        return [IO.File]::ReadAllText($path, $script:ajEnc) | ConvertFrom-Json
    }

    $AjC = Join-Path $Work 'aj-root'
    New-Item -ItemType Directory -Force -Path (Join-Path $AjC 'pkg'), (Join-Path $AjC 'tests'), `
        (Join-Path $Work 'aj-rc'), (Join-Path $Work 'aj-rc-both'), (Join-Path $Work 'aj-rc-orch'), `
        (Join-Path $Work 'aj-rc-del') | Out-Null
    New-File (Join-Path $AjC 'pkg\core.py') "def f(): return 1$nl"
    New-File (Join-Path $AjC 'tests\test_core.py') "def test_f(): assert 1$nl"

    # 回执夹具：同一命令的三种最近判词（绿/红）与 journal changes 两种（added/deleted）。
    New-File (Join-Path $Work 'aj-rc\rc-green.json') '{"at":"fixture","run":[{"command":"python -m pytest tests/test_core.py -q","exit":0}]}'
    Start-Sleep -Milliseconds 20
    New-File (Join-Path $Work 'aj-rc\rc-red.json') '{"at":"fixture","run":[{"command":"python -m pytest tests/test_core.py -q","exit":1}]}'
    [IO.File]::Copy((Join-Path $Work 'aj-rc\rc-green.json'), (Join-Path $Work 'aj-rc-both\rc-green.json'))
    Start-Sleep -Milliseconds 20
    New-File (Join-Path $Work 'aj-rc-both\rc-red.json') '{"at":"fixture","run":[{"command":"python -m pytest tests/test_core.py -q","exit":1}]}'
    New-File (Join-Path $Work 'aj-rc-orch\rc-full.json') '{"at":"fixture","run":[{"command":"python -m pytest tests/test_core.py -q","exit":0}],"changes":["journal: added    pkg/core.py"]}'
    New-File (Join-Path $Work 'aj-rc-del\rc-del.json') '{"at":"fixture","run":[],"changes":["journal: deleted    pkg/core.py"]}'
    New-File (Join-Path $Work 'aj-rc\rc-ghostgreen.json') '{"at":"fixture","run":[{"command":"python -m pytest tests/ghost_rs.rs -q","exit":0}]}'

    # 台账夹具：cost-ledger 里那条运行痕迹的 key 带 sha8(command)，与 agent_verify 写账同源推导。
    $ajCmd = 'python -m pytest tests/test_core.py -q'
    $ajLed = Join-Path $Work 'aj-ledger'
    New-Item -ItemType Directory -Force -Path $ajLed | Out-Null
    & $ajPy $ldPy -Mode add -Ledger (Join-Path $ajLed 'cost-ledger.json') -Task 'av|ajt' `
        -Key ('av|ajr1|cmd1|' + (Get-AjSha8 $ajCmd)) -Wall 1.5 2>&1 | Out-Null
    Assert-True ($LASTEXITCODE -eq 0) 'AJ0a 台账夹具落盘（key 含 sha8(command)，独立复核的第二证据源）' ''
    $ajLedEmpty = Join-Path $Work 'aj-ledger-empty'
    New-Item -ItemType Directory -Force -Path $ajLedEmpty | Out-Null
    & $ajPy $ldPy -Mode add -Ledger (Join-Path $ajLedEmpty 'cost-ledger.json') -Task 'av|other' `
        -Key 'av|other|cmd1|deadbeef00' -Wall 0.5 2>&1 | Out-Null
    $ajLedBad = Join-Path $Work 'aj-ledger-bad'
    New-Item -ItemType Directory -Force -Path $ajLedBad | Out-Null
    [IO.File]::WriteAllText((Join-Path $ajLedBad 'cost-ledger.json'), '{bad', $ajEnc)

    # --- AJ1-AJ4: 聚合判定三态规则（全 V→V / 一 F→F / 有 U→U / 缺分解→U）---
    New-File (Join-Path $Work 'aj-agg-v.jsonl') ('{"claim":"fixture: pkg and tests all green","kind":"aggregate","decomposition":[' +
        '{"claim":"pkg/core.py exists on disk","kind":"file-exists"},' +
        '{"claim":"tests/test_core.py all green","kind":"test-pass"}]}' + $nl)
    $aj1 = Invoke-AjCa @('-Claims', (Join-Path $Work 'aj-agg-v.jsonl'), '-Root', $AjC, '-Receipts', (Join-Path $Work 'aj-rc\rc-green.json'))
    $aj1v = @($ajOut | Where-Object { $_ -match '\] VERIFIED' }).Count
    Assert-True ($aj1 -eq 0 -and $aj1v -eq 3 -and (($ajOut -join ' ') -match 'conjunction of 2 atom')) `
        'AJ1 全原子 VERIFIED→聚合 VERIFIED exit 0（合取语义，逐原子判词都在输出里）' "e=$aj1 v=$aj1v"

    New-File (Join-Path $Work 'aj-agg-f.jsonl') ('{"claim":"one fake atom inside","kind":"aggregate","decomposition":[' +
        '{"claim":"pkg/core.py exists","kind":"file-exists"},' +
        '{"claim":"tests/ghost_rs.rs all green","kind":"test-pass"}]}' + $nl)
    $aj2 = Invoke-AjCa @('-Claims', (Join-Path $Work 'aj-agg-f.jsonl'), '-Root', $AjC, '-Receipts', (Join-Path $Work 'aj-rc\rc-green.json'))
    Assert-True ($aj2 -eq 2 -and (($ajOut -join ' ') -match 'any FABRICATED atom makes the aggregate FABRICATED')) `
        'AJ2 一原子 FABRICATED→聚合 FABRICATED exit 2（任一 F 即 F，最响警报不因包装降级）' "e=$aj2"

    New-File (Join-Path $Work 'aj-agg-u.jsonl') ('{"claim":"one unverifiable atom inside","kind":"aggregate","decomposition":[' +
        '{"claim":"pkg/core.py exists","kind":"file-exists"},' +
        '{"claim":"overhead 9.99x","kind":"number"}]}' + $nl)
    $aj3 = Invoke-AjCa @('-Claims', (Join-Path $Work 'aj-agg-u.jsonl'), '-Root', $AjC, '-Receipts', (Join-Path $Work 'aj-rc\rc-green.json'))
    Assert-True ($aj3 -eq 1 -and (($ajOut -join ' ') -match 'unevidenced atom is UNVERIFIED')) `
        'AJ3 有 UNVERIFIED 原子→聚合 UNVERIFIED exit 1（缺证据的部分不许被结论句吸收）' "e=$aj3"

    New-File (Join-Path $Work 'aj-agg-nd.jsonl') ('{"claim":"all green everywhere, trust me","kind":"aggregate"}' + $nl)
    $aj4 = Invoke-AjCa @('-Claims', (Join-Path $Work 'aj-agg-nd.jsonl'), '-Root', $AjC, '-Receipts', (Join-Path $Work 'aj-rc\rc-green.json'))
    Assert-True ($aj4 -eq 1 -and (($ajOut -join ' ') -match 'cannot be VERIFIED')) `
        'AJ4 聚合缺 decomposition→UNVERIFIED exit 1（聚合级没有分解就不许 VERIFIED——"全绿"没有拆条没有资格过闸）' "e=$aj4"

    # --- AJ5/AJ6: 空分解等同缺分解；原子级判词逐条可见 ---
    New-File (Join-Path $Work 'aj-agg-e.jsonl') ('{"claim":"empty decomposition","kind":"aggregate","decomposition":[]}' + $nl)
    $aj5 = Invoke-AjCa @('-Claims', (Join-Path $Work 'aj-agg-e.jsonl'), '-Root', $AjC)
    Assert-True ($aj5 -eq 1) 'AJ5 空 decomposition 数组→UNVERIFIED exit 1（空与缺同罪，不许拿空列表冒充拆条）' "e=$aj5"
    $aj6 = Invoke-AjCa @('-Claims', (Join-Path $Work 'aj-agg-v.jsonl'), '-Root', $AjC, '-Receipts', (Join-Path $Work 'aj-rc\rc-green.json'))
    $aj6a = @($ajOut | Where-Object { $_ -match 'atom\[1\]' }).Count + @($ajOut | Where-Object { $_ -match 'atom\[2\]' }).Count
    Assert-True ($aj6 -eq 0 -and $aj6a -ge 2) `
        'AJ6 分解原子逐条打印判词（atom[1]/atom[2] 各带三态之一，过程可见而不是只给结论）' "e=$aj6 atoms=$aj6a"

    # --- AJ7-AJ9: 分解形状坏/原子缺 kind/原子带机器路径 → 整份 fail-closed exit 3 ---
    New-File (Join-Path $Work 'aj-agg-bad1.jsonl') ('{"claim":"atom not a dict","kind":"aggregate","decomposition":["just-a-string"]}' + $nl)
    $aj7 = Invoke-AjCa @('-Claims', (Join-Path $Work 'aj-agg-bad1.jsonl'), '-Root', $AjC)
    Assert-True ($aj7 -eq 3) 'AJ7 decomposition 原子不是对象→exit 3 拒整份（半读的声明集不许有一半被当真）' "e=$aj7"
    New-File (Join-Path $Work 'aj-agg-bad2.jsonl') ('{"claim":"atom missing kind","kind":"aggregate","decomposition":[{"claim":"only claim"}]}' + $nl)
    $aj8 = Invoke-AjCa @('-Claims', (Join-Path $Work 'aj-agg-bad2.jsonl'), '-Root', $AjC)
    Assert-True ($aj8 -eq 3) 'AJ8 原子缺 kind→exit 3（没有 verifier 名字的原子不能作证）' "e=$aj8"
    $ajMp = 'X' + ':/Us' + 'ers/someone/n.md'
    New-File (Join-Path $Work 'aj-agg-mp.jsonl') ('{"claim":"mp in atom","kind":"aggregate","decomposition":[{"claim":"see ' + $ajMp + ' for detail","kind":"file-exists"}]}' + $nl)
    $aj9 = Invoke-AjCa @('-Claims', (Join-Path $Work 'aj-agg-mp.jsonl'), '-Root', $AjC)
    Assert-True ($aj9 -eq 3) 'AJ9 机器路径藏在原子里→exit 3（隐私闸扫进 decomposition，拆条不许成为盘符路径的密道）' "e=$aj9"

    # --- AJ10: 原子不许再是聚合（无递归 verifier→UNVERIFIED）---
    New-File (Join-Path $Work 'aj-agg-nest.jsonl') ('{"claim":"nested aggregate","kind":"aggregate","decomposition":[' +
        '{"claim":"inner agg","kind":"aggregate","decomposition":[{"claim":"pkg/core.py exists","kind":"file-exists"}]}]}' + $nl)
    $aj10 = Invoke-AjCa @('-Claims', (Join-Path $Work 'aj-agg-nest.jsonl'), '-Root', $AjC)
    Assert-True ($aj10 -eq 1 -and (($ajOut -join ' ') -match 'cannot serve as its own atom')) `
        'AJ10 原子嵌套 aggregate→UNVERIFIED exit 1（分解不递归，嵌套层没有 verifier 就不许作证）' "e=$aj10"

    # --- AJ11/AJ12: 旧格式回归——纯原子声明文件行为与 summary 行形状不变 ---
    New-File (Join-Path $Work 'aj-old-v.jsonl') ((@(
        '{"claim":"pkg/core.py exists on disk","kind":"file-exists"}',
        '{"claim":"tests/test_core.py all green","kind":"test-pass"}') -join $nl) + $nl)
    $aj11 = Invoke-AjCa @('-Claims', (Join-Path $Work 'aj-old-v.jsonl'), '-Root', $AjC, '-Receipts', (Join-Path $Work 'aj-rc\rc-green.json'))
    Assert-True ($aj11 -eq 0 -and (($ajOut -join "`n") -match 'summary: total=2 verified=2 unverified=0 fabricated=0') `
        -and (@($ajOut | Where-Object { $_ -match 'aggregates=' }).Count -eq 0)) `
        'AJ11 旧格式回归：纯原子两 VERIFIED exit 0，summary 行逐字不变且不出现 aggregates 行（追加不改建）' "e=$aj11"
    New-File (Join-Path $Work 'aj-old-f.jsonl') ((@(
        '{"claim":"tests/ghost_rs.rs all green","kind":"test-pass"}',
        '{"claim":"tests/test_red.py all green","kind":"test-pass"}') -join $nl) + $nl)
    $aj12 = Invoke-AjCa @('-Claims', (Join-Path $Work 'aj-old-f.jsonl'), '-Root', $AjC, '-Receipts', (Join-Path $Work 'aj-rc\rc-green.json'))
    Assert-True ($aj12 -eq 2) `
        'AJ12 旧格式回归：幽灵测试照旧 FABRICATED exit 2（三态 exit 语义 0/1/2 保持）' "e=$aj12"

    # --- AJ13-AJ18: second-opinion 独立复核（同一声明两条独立证据路径都要走通）---
    New-File (Join-Path $Work 'aj-tp.jsonl') ('{"claim":"tests/test_core.py all green","kind":"test-pass"}' + $nl)
    New-File (Join-Path $Work 'aj-fe.jsonl') ('{"claim":"pkg/core.py exists on disk","kind":"file-exists"}' + $nl)
    $aj13 = Invoke-AjCa @('second-opinion', '-Claims', (Join-Path $Work 'aj-tp.jsonl'), '-Root', $AjC,
        '-Receipts', (Join-Path $Work 'aj-rc\rc-green.json'), '-Ledger', (Join-Path $ajLed 'cost-ledger.json'))
    Assert-True ($aj13 -eq 0 -and (($ajOut -join ' ') -match 'dual-source') `
        -and (($ajOut -join "`n") -match 'summary: dual-source=1')) `
        'AJ13 双源一致→VERIFIED 升级标注 dual-source exit 0（receipt 一路 + 盘上文件与台账运行痕迹一路）' "e=$aj13"

    New-File (Join-Path $Work 'aj-tp-ghost.jsonl') ('{"claim":"tests/ghost_rs.rs all green","kind":"test-pass"}' + $nl)
    $aj14 = Invoke-AjCa @('second-opinion', '-Claims', (Join-Path $Work 'aj-tp-ghost.jsonl'), '-Root', $AjC,
        '-Receipts', (Join-Path $Work 'aj-rc\rc-ghostgreen.json'), '-Ledger', (Join-Path $ajLed 'cost-ledger.json'))
    Assert-True ($aj14 -eq 2 -and (@($ajOut | Where-Object { $_ -match 'src1:' }).Count -ge 1) `
        -and (@($ajOut | Where-Object { $_ -match 'src2:' }).Count -ge 1)) `
        'AJ14 双源矛盾→降级 FABRICATED exit 2 且附双证据（src1 回执判词与 src2 盘上现实并列打印）' "e=$aj14"

    $aj15 = Invoke-AjCa @('second-opinion', '-Claims', (Join-Path $Work 'aj-tp.jsonl'), '-Root', $AjC,
        '-Receipts', (Join-Path $Work 'aj-rc\rc-green.json'), '-Ledger', (Join-Path $ajLedEmpty 'cost-ledger.json'))
    Assert-True ($aj15 -eq 1 -and (($ajOut -join ' ') -match 'left no ledger trace')) `
        'AJ15 台账无该命令的运行痕迹→UNVERIFIED exit 1（两源都要走通，缺一路不许升级）' "e=$aj15"

    $aj16 = Invoke-AjCa @('second-opinion', '-Claims', (Join-Path $Work 'aj-tp.jsonl'), '-Root', $AjC,
        '-Receipts', (Join-Path $Work 'aj-rc-both\*.json'), '-Ledger', (Join-Path $ajLed 'cost-ledger.json'))
    Assert-True ($aj16 -eq 2 -and (($ajOut -join ' ') -match 'is RED')) `
        'AJ16 最近回执判红→FABRICATED exit 2（src1 取 mtime 最新回执，最新证据压过挑出来的旧绿）' "e=$aj16"

    $aj17 = Invoke-AjCa @('second-opinion', '-Claims', (Join-Path $Work 'aj-tp.jsonl'), '-Root', $AjC,
        '-Receipts', (Join-Path $Work 'aj-rc\rc-green.json'), '-Ledger', (Join-Path $ajLedBad 'cost-ledger.json'))
    Assert-True ($aj17 -eq 3) `
        'AJ17 台账损坏→exit 3 拒（复核不信任自己的第二证据源就整体拒绝，不是降级继续）' "e=$aj17"

    New-File (Join-Path $Work 'aj-agg-dual.jsonl') ('{"claim":"fixture: pkg and tests all green","kind":"aggregate","decomposition":[' +
        '{"claim":"pkg/core.py exists on disk","kind":"file-exists"},' +
        '{"claim":"tests/test_core.py all green","kind":"test-pass"}]}' + $nl)
    $aj18 = Invoke-AjCa @('second-opinion', '-Claims', (Join-Path $Work 'aj-agg-dual.jsonl'), '-Root', $AjC,
        '-Receipts', (Join-Path $Work 'aj-rc-orch\rc-full.json'), '-Ledger', (Join-Path $ajLed 'cost-ledger.json'))
    Assert-True ($aj18 -eq 0 -and (($ajOut -join ' ') -match 'all VERIFIED \(dual-source\)')) `
        'AJ18 聚合在独立复核下逐原子双源全过→聚合 VERIFIED（dual-source）exit 0（分解规则在复核层同样成立）' "e=$aj18"

    # --- AJ19/AJ19b: file-exists 的第二证据源 = 回执 journal changes（added↔deleted）---
    $aj19 = Invoke-AjCa @('second-opinion', '-Claims', (Join-Path $Work 'aj-fe.jsonl'), '-Root', $AjC,
        '-Receipts', (Join-Path $Work 'aj-rc-orch\rc-full.json'))
    Assert-True ($aj19 -eq 0 -and (($ajOut -join ' ') -match 'dual-source')) `
        'AJ19 file-exists 双源：盘上 stat 一路 + 回执 journal 记 added 一路→dual-source VERIFIED exit 0' "e=$aj19"
    $aj19b = Invoke-AjCa @('second-opinion', '-Claims', (Join-Path $Work 'aj-fe.jsonl'), '-Root', $AjC,
        '-Receipts', (Join-Path $Work 'aj-rc-del\rc-del.json'))
    Assert-True ($aj19b -eq 2 -and (($ajOut -join ' ') -match 'DELETED')) `
        'AJ19b 回执 journal 记 deleted 而声称存在→两源矛盾 FABRICATED exit 2' "e=$aj19b"

    # --- AJ20/AJ21: --second-opinion 编排集成（claims 步骤自动跑两源）---
    $AjO = Join-Path $Work 'aj-orch'
    New-AjFixture $AjO
    New-File (Join-Path $AjO 'plan.json') '{"proven":true,"commands":["echo aj > aj.txt"],"unproven":[]}'
    $storeAj = Join-Path $Work 'aj-store-orch'
    New-Item -ItemType Directory -Force -Path $storeAj | Out-Null
    & $ajPy $ldPy -Mode add -Ledger (Join-Path $storeAj 'cost-ledger.json') -Task 'av|ajorch' `
        -Key ('av|ajorch|cmd1|' + (Get-AjSha8 $ajCmd)) -Wall 1.0 2>&1 | Out-Null
    New-File (Join-Path $Work 'aj-orch-claims.jsonl') ((@(
        '{"claim":"pkg/core.py exists on disk","kind":"file-exists"}',
        '{"claim":"tests/test_core.py all green","kind":"test-pass"}') -join $nl) + $nl)
    $aj20 = Invoke-AjAv $AjO $storeAj (Join-Path $Work 'aj-rc-orch.json') `
        @('--claims-file', (Join-Path $Work 'aj-orch-claims.jsonl'),
          '--claims-receipts', (Join-Path $Work 'aj-rc-orch\*.json'), '--second-opinion')
    $aj20doc = Read-AjReceipt (Join-Path $Work 'aj-rc-orch.json')
    $aj20steps = @($aj20doc.steps | ForEach-Object { $_.step })
    Assert-True ($aj20 -eq 0 -and ($aj20steps -contains 'claim-second-opinion') `
        -and $aj20doc.claims.second.dual_source -eq 2 -and $aj20doc.claims.second.fabricated -eq 0) `
        'AJ20 编排 --second-opinion：claims 步骤自动跑两源，双源全过→链通过且回执 claims.second 带 dual_source（appended 字段）' "e=$aj20 second=$($aj20doc.claims.second | ConvertTo-Json -Compress)"
    New-File (Join-Path $Work 'aj-orch-fe.jsonl') ('{"claim":"pkg/core.py exists on disk","kind":"file-exists"}' + $nl)
    $aj21 = Invoke-AjAv $AjO $storeAj (Join-Path $Work 'aj-rc-orch2.json') `
        @('--claims-file', (Join-Path $Work 'aj-orch-fe.jsonl'),
          '--claims-receipts', (Join-Path $Work 'aj-rc-del\*.json'), '--second-opinion')
    $aj21doc = Read-AjReceipt (Join-Path $Work 'aj-rc-orch2.json')
    $aj21steps = @($aj21doc.steps | ForEach-Object { $_.step })
    Assert-True ($aj21 -eq 1 -and $aj21doc.verdict -eq 'rejected: fabricated claims (second opinion)' `
        -and ($aj21steps -contains 'claim-second-opinion') -and ($aj21steps -notcontains 'run')) `
        'AJ21 独立复核抓到矛盾→全链拒绝在 run 前（主审计过了不等于复核过——两条证据路径都要走通）' "e=$aj21 v=$($aj21doc.verdict)"
    $aj22 = Invoke-AjAv $AjO $storeAj (Join-Path $Work 'aj-rc-orch3.json') @('--second-opinion')
    Assert-True ($aj22 -eq 2) `
        'AJ22 --second-opinion 不带 --claims-file→exit 2（复核没有对象就是坏调用）' "e=$aj22"

    # --- AJ23/AJ23b: 1000 原子大文件不崩；同文件双进程并发不崩 ---
    $ajAtoms = 1..1000 | ForEach-Object { '{"claim":"pkg/core.py exists on disk","kind":"file-exists"}' }
    New-File (Join-Path $Work 'aj-big.jsonl') ('{"claim":"all 1000 parts exist","kind":"aggregate","decomposition":[' + ($ajAtoms -join ',') + ']}' + $nl)
    $aj23 = Invoke-AjCa @('-Claims', (Join-Path $Work 'aj-big.jsonl'), '-Root', $AjC)
    Assert-True ($aj23 -eq 0 -and (($ajOut -join "`n") -match 'summary: aggregates=1 atoms=1000') `
        -and (($ajOut -join "`n") -match 'summary: total=1 verified=1 unverified=0 fabricated=0')) `
        'AJ23 1000 原子聚合一次核验完成（大文件不崩，判定与规模都如实上报）' "e=$aj23"
    $ajC1 = Start-Process -FilePath $ajPy -ArgumentList @($caPy, '-Claims', (Join-Path $Work 'aj-big.jsonl'), '-Root', $AjC) `
        -NoNewWindow -PassThru -RedirectStandardOutput (Join-Path $Work 'aj-c1.out') -RedirectStandardError (Join-Path $Work 'aj-c1.err')
    $ajC2 = Start-Process -FilePath $ajPy -ArgumentList @($caPy, '-Claims', (Join-Path $Work 'aj-big.jsonl'), '-Root', $AjC) `
        -NoNewWindow -PassThru -RedirectStandardOutput (Join-Path $Work 'aj-c2.out') -RedirectStandardError (Join-Path $Work 'aj-c2.err')
    $null = $ajC1.WaitForExit(); $null = $ajC2.WaitForExit()
    Assert-True ($ajC1.ExitCode -eq 0 -and $ajC2.ExitCode -eq 0) `
        'AJ23b 同一 1000 原子文件双进程并发核验都成功（claim_audit 只读无共享状态，并发不崩不互伤）' "c1=$($ajC1.ExitCode) c2=$($ajC2.ExitCode)"

    # --- AJ24: 对抗自测——30 条"聚合糊弄"声明，每条大声明包 3 条假原子 ---
    $ajFool = 1..30 | ForEach-Object {
        '{"claim":"wave ' + $_ + ': all green, all checked, nothing missing","kind":"aggregate","decomposition":[' +
        '{"claim":"tests/ghost_a_' + $_ + '.rs all green","kind":"test-pass"},' +
        '{"claim":"tests/ghost_b_' + $_ + '.rs all green","kind":"test-pass"},' +
        '{"claim":"pkg/ghost_c_' + $_ + '.py exists","kind":"file-exists"}]}'
    }
    New-File (Join-Path $Work 'aj-fool.jsonl') (($ajFool -join $nl) + $nl)
    $aj24 = Invoke-AjCa @('-Claims', (Join-Path $Work 'aj-fool.jsonl'), '-Root', $AjC)
    $aj24top = @($ajOut | Where-Object { $_ -match '^\s*\[\d+\] FABRICATED' }).Count
    Assert-True ($aj24 -eq 2 -and $aj24top -eq 30 -and (($ajOut -join ' ') -match 'fabricated=30')) `
        'AJ24 对抗自测：30 条聚合糊弄声明（大声明包 3 条假原子）分解后 30/30 FABRICATED 捕获，exit 2（糊弄句没有拆条豁免权）' "e=$aj24 top=$aj24top"
}


# =====================================================================================
"== AK. 数学推理引擎 mre：推导图重执行——附推导的数字声明要么算得通，要么点名失配步 =="
# 出处写进 mre usage 与 cognition/SCHEMA.md §7（台账 L121）：①业界过程监督实证（"Let's
# Verify Step by Step", 2023）——每步带 note 的推导轨迹就是过程监督的训练数据形状；
# ②业界把数学声明变成机器可核验对象的探索（NS 奇性声明配 Lean 4 形式化，假设态探索，
# 工程不依赖该声明真假）。mre 验收的是推导一致性，不是前提真实性——工件出处仍是
# number 的第一证据路径，推导重执行是补上来的第二路径（number 单源孤儿的缺口，L121）。
$akPy = $null
foreach ($c in @($env:PYTHON, 'python', 'python3', 'py')) {
    if (-not $c) { continue }
    $akSrc = (Get-Command $c -ErrorAction SilentlyContinue).Source
    if (-not $akSrc) { continue }
    if ((& $akSrc -c 'print(1)' 2>$null) -eq '1') { $akPy = $akSrc; break }
}
$akTotal = 34
if (-not $akPy) {
    "  SKIP  AK0 找不到可用的 Python，跳过数学推理引擎检查"
    $skipped += $akTotal
} else {
    $mrPy = Join-Path $tools 'mre.py'
    $caPy = Join-Path $tools 'claim_audit.py'
    $avPy = Join-Path $tools 'agent_verify.py'
    $akEnc = New-Object System.Text.UTF8Encoding($false)

    function Invoke-AkMr([string]$path) {
        $script:akOut = @(& $script:akPy $script:akMrPy '-Derivation' $path 2>&1)
        return $LASTEXITCODE
    }
    function Invoke-AkCa([string[]]$caArgs) {
        $script:akOut = @(& $script:akPy $script:akCaPy @caArgs 2>&1)
        return $LASTEXITCODE
    }
    function Invoke-AkAv([string]$root, [string]$store, [string]$receipt, [string[]]$extra) {
        $argsAv = @($script:akAvPy, '-Root', $root, '-Changed', (Join-Path $root 'pkg\core.py'),
                    '-Lang', 'python', '-Plan', (Join-Path $root 'plan.json'), '-Store', $store,
                    '-Receipt', $receipt) + $extra
        & $script:akPy @argsAv 2>&1 | Out-Null
        return $LASTEXITCODE
    }
    function Read-AkReceipt([string]$path) {
        return [IO.File]::ReadAllText($path, $script:akEnc) | ConvertFrom-Json
    }

    # 夹具生成器：全部推导 JSON / 声明集 / 对抗样本由一段 Python 生成（Windows 路径经
    # argv 传入，避免 MSYS 路径翻译歧义）。
    $akGen = Join-Path $Work 'ak-gen.py'
    New-File $akGen @'
import json, os, sys

work, akr, ako = sys.argv[1], sys.argv[2], sys.argv[3]

def w(path, doc):
    os.makedirs(os.path.dirname(path), exist_ok=True)
    with open(path, "w", encoding="utf-8", newline="\n") as fh:
        json.dump(doc, fh, ensure_ascii=True)

def wt(path, text):
    os.makedirs(os.path.dirname(path), exist_ok=True)
    with open(path, "w", encoding="utf-8", newline="\n") as fh:
        fh.write(text)

D = os.path.join(work, "ak-der")

w(os.path.join(D, "roundtrip.json"), {
    "id": "ak-roundtrip", "claim": "exact rational chain", "target": {"value": "17/35"},
    "steps": [
        {"op": "add", "inputs": [1, 2], "output_id": "s1", "output": "3", "note": "1+2"},
        {"op": "div", "inputs": [1, 3], "output_id": "s2", "output": "1/3", "note": "one third"},
        {"op": "mul", "inputs": ["s1", "s2"], "output_id": "s3", "output": "1", "note": "3 * 1/3"},
        {"op": "sub", "inputs": ["s3", "1/35"], "output_id": "s4", "output": "34/35", "note": "minus 1/35"},
        {"op": "mul", "inputs": ["s4", "1/2"], "output_id": "s5", "output": "17/35", "note": "half"}]})

w(os.path.join(D, "mismatch.json"), {
    "id": "ak-mismatch", "claim": "one step lies", "target": {"value": "999"},
    "steps": [
        {"op": "add", "inputs": [2, 3], "output_id": "s1", "output": "5", "note": "2+3"},
        {"op": "mul", "inputs": ["s1", 4], "output_id": "s2", "output": "20", "note": "5*4"},
        {"op": "sub", "inputs": ["s2", 1], "output_id": "s3", "output": "999", "note": "declared wrongly"}]})

w(os.path.join(D, "sqrt-ok.json"), {
    "id": "ak-sqrt-ok", "claim": "sqrt(2) to declared tolerance",
    "target": {"value": "1.4142136", "tol": "0.00001"},
    "steps": [{"op": "sqrt", "inputs": [2], "output_id": "r", "output": "1.4142136",
               "tol": "0.00001", "note": "bracketed in exact rational arithmetic"}]})

w(os.path.join(D, "sqrt-lie.json"), {
    "id": "ak-sqrt-lie", "claim": "sqrt(2) claimed as 1.5",
    "target": {"value": "1.5", "tol": "0.00001"},
    "steps": [{"op": "sqrt", "inputs": [2], "output_id": "r", "output": "1.5",
               "tol": "0.00001", "note": "outside the bracket"}]})

w(os.path.join(D, "pow-int.json"), {
    "id": "ak-pow-int", "claim": "2^10 = 1024", "target": {"value": "1024"},
    "steps": [{"op": "pow", "inputs": [2, 10], "output_id": "p", "output": "1024",
               "note": "integer exponent is exact"}]})

w(os.path.join(D, "pow-frac.json"), {
    "id": "ak-pow-frac", "claim": "2^(1/2) to declared tolerance",
    "target": {"value": "1.4142136", "tol": "0.00001"},
    "steps": [{"op": "pow", "inputs": [2, "1/2"], "output_id": "p", "output": "1.4142136",
               "tol": "0.00001", "note": "fractional exponent bracketed"}]})

w(os.path.join(D, "pow-neg.json"), {
    "id": "ak-pow-neg", "claim": "cube root of -8", "target": {"value": "2"},
    "steps": [{"op": "pow", "inputs": [-8, "1/3"], "output_id": "p", "output": "2",
               "tol": "0.001", "note": "negative base fractional power"}]})

w(os.path.join(D, "divzero.json"), {
    "id": "ak-divzero", "claim": "divide by zero", "target": {"value": "1"},
    "steps": [
        {"op": "add", "inputs": [1, 1], "output_id": "z", "output": "2", "note": "2"},
        {"op": "sub", "inputs": ["z", "z"], "output_id": "zero", "output": "0", "note": "0"},
        {"op": "div", "inputs": [1, "zero"], "output_id": "boom", "output": "5", "note": "1/0"}]})

w(os.path.join(D, "sqrtneg.json"), {
    "id": "ak-sqrtneg", "claim": "sqrt of a negative", "target": {"value": "2"},
    "steps": [
        {"op": "sub", "inputs": [1, 3], "output_id": "d", "output": "-2", "note": "1-3"},
        {"op": "sqrt", "inputs": ["d"], "output_id": "r", "output": "2", "tol": "0.01",
         "note": "sqrt(-2) is undefined"}]})

w(os.path.join(D, "cycle.json"), {
    "id": "ak-cycle", "claim": "cyclic references", "target": {"value": "1"},
    "steps": [
        {"op": "add", "inputs": ["s2", 1], "output_id": "s1", "output": "2", "note": "needs s2"},
        {"op": "add", "inputs": ["s1", 1], "output_id": "s2", "output": "3", "note": "needs s1"}]})

w(os.path.join(D, "ghost.json"), {
    "id": "ak-ghost", "claim": "unresolved reference", "target": {"value": "2"},
    "steps": [{"op": "add", "inputs": ["ghost", 1], "output_id": "s1", "output": "2",
               "note": "ghost is neither a literal nor an output_id"}]})

w(os.path.join(D, "tolband.json"), {
    "id": "ak-tolband", "claim": "exact op within declared band",
    "target": {"value": "3.0005", "tol": "0.001"},
    "steps": [{"op": "add", "inputs": [1, 2], "output_id": "s1", "output": "3.0005",
               "tol": "0.001", "note": "|3 - 3.0005| <= 0.001"}]})

w(os.path.join(D, "tolband-bad.json"), {
    "id": "ak-tolband-bad", "claim": "exact op beyond declared band",
    "target": {"value": "3.0005", "tol": "0.0001"},
    "steps": [{"op": "add", "inputs": [1, 2], "output_id": "s1", "output": "3.0005",
               "tol": "0.0001", "note": "|3 - 3.0005| > 0.0001"}]})

w(os.path.join(D, "nonote.json"), {
    "id": "ak-nonote", "claim": "a step without note", "target": {"value": "3"},
    "steps": [{"op": "add", "inputs": [1, 2], "output_id": "s1", "output": "3"}]})

wt(os.path.join(D, "nosteps.json"),
   '{"id":"ak-nosteps","claim":"no derivation graph at all","target":{"value":"1"}}')
wt(os.path.join(D, "bad.json"), '{bad')

def big(n):
    return {"id": "ak-big", "claim": str(n) + " exact adds", "target": {"value": "2"},
            "steps": [{"op": "add", "inputs": [1, 1], "output_id": "s" + str(i),
                       "output": "2", "note": "1+1"} for i in range(1, n + 1)]}
w(os.path.join(D, "big10000.json"), big(10000))
w(os.path.join(D, "big10001.json"), big(10001))

# claims root: bench artifact carries 2.35 and the 20 adversarial lie numbers
lies = ["9.99", "9.98", "9.97", "9.96", "9.95", "9.94", "9.93", "9.92", "9.91", "9.90",
        "8.99", "8.98", "8.97", "8.96", "8.95", "8.94", "8.93", "8.92", "8.91", "8.90"]
wt(os.path.join(akr, "artifacts", "bench.txt"),
   "guard_overhead_ratio 2.35x wave numbers " + " ".join(lies) + "\n")

w(os.path.join(akr, "derivations", "overhead.json"), {
    "id": "overhead", "claim": "the ratio derives as 47/20", "target": {"value": "2.35"},
    "steps": [{"op": "add", "inputs": [40, 7], "output_id": "s1", "output": "47", "note": "40+7"},
              {"op": "div", "inputs": ["s1", 20], "output_id": "s2", "output": "2.35",
               "note": "47/20 = 2.35"}]})

w(os.path.join(akr, "derivations", "liar.json"), {
    "id": "liar", "claim": "the ratio is 9.99 (a lie)", "target": {"value": "9.99"},
    "steps": [{"op": "add", "inputs": [40, 7], "output_id": "s1", "output": "47", "note": "40+7"},
              {"op": "div", "inputs": ["s1", 20], "output_id": "s2", "output": "9.99",
               "note": "47/20 misreported as 9.99"}]})

fool = []
for k, lie in enumerate(lies, 1):
    fool.append(json.dumps({
        "claim": "wave " + str(k) + " overhead " + lie + "x",
        "kind": "number", "value": float(lie),
        "source": {"path": "artifacts/bench.txt"},
        "derivation": {"id": "liar-" + str(k), "claim": "wave " + str(k) + " ratio",
                       "target": {"value": lie},
                       "steps": [{"op": "add", "inputs": [40, 7], "output_id": "s1",
                                  "output": "47", "note": "40+7"},
                                 {"op": "div", "inputs": ["s1", 20], "output_id": "s2",
                                  "output": lie, "note": "47/20 misreported"}]}}))
wt(os.path.join(work, "fool-claims.jsonl"), "\n".join(fool) + "\n")

inline_ok = {"id": "inline-ok", "claim": "47/20", "target": {"value": "2.35"},
             "steps": [{"op": "add", "inputs": [40, 7], "output_id": "s1", "output": "47",
                        "note": "40+7"},
                       {"op": "div", "inputs": ["s1", 20], "output_id": "s2", "output": "2.35",
                        "note": "47/20"}]}
inline_liar = {"id": "inline-liar", "claim": "9.99", "target": {"value": "9.99"},
               "steps": [{"op": "add", "inputs": [40, 7], "output_id": "s1", "output": "47",
                          "note": "40+7"},
                         {"op": "div", "inputs": ["s1", 20], "output_id": "s2", "output": "9.99",
                          "note": "misreported"}]}

# orchestration root: file-based derivation + inline liar
wt(os.path.join(ako, "artifacts", "bench.txt"), "guard_overhead_ratio 2.35x wave 9.99x\n")
w(os.path.join(ako, "derivations", "ok.json"), {
    "id": "ok", "claim": "47/20", "target": {"value": "2.35"},
    "steps": [{"op": "add", "inputs": [40, 7], "output_id": "s1", "output": "47", "note": "40+7"},
              {"op": "div", "inputs": ["s1", 20], "output_id": "s2", "output": "2.35",
               "note": "47/20"}]})
num_ok = {"claim": "overhead 2.35x", "kind": "number", "value": 2.35,
          "source": {"path": "artifacts/bench.txt"}, "derivation": "derivations/ok.json"}
num_ok_ca = {"claim": "overhead 2.35x", "kind": "number", "value": 2.35,
             "source": {"path": "artifacts/bench.txt"}, "derivation": "derivations/overhead.json"}
num_bad = {"claim": "wave 9.99x", "kind": "number", "value": 9.99,
           "source": {"path": "artifacts/bench.txt"}, "derivation": inline_liar}
fe = {"claim": "pkg/core.py exists on disk", "kind": "file-exists"}
wt(os.path.join(ako, "claims-ak-ok.jsonl"),
   json.dumps(dict(num_ok, kind="number")) + "\n" + json.dumps(fe) + "\n")
wt(os.path.join(ako, "claims-ak-bad.jsonl"), json.dumps(num_bad) + "\n")
wt(os.path.join(ako, "claims-ak-so.jsonl"), json.dumps(dict(num_ok, kind="number")) + "\n")
wt(os.path.join(ako, "claims-ak-legacy.jsonl"), json.dumps(fe) + "\n")
wt(os.path.join(work, "ak-ca-ok.jsonl"), json.dumps(num_ok_ca) + "\n")
wt(os.path.join(work, "ak-ca-bad.jsonl"),
   json.dumps({"claim": "wave 9.99x", "kind": "number", "value": 9.99,
               "source": {"path": "artifacts/bench.txt"},
               "derivation": "derivations/liar.json"}) + "\n")
wt(os.path.join(work, "ak-ca-inline.jsonl"),
   json.dumps({"claim": "overhead 2.35x", "kind": "number", "value": 2.35,
               "source": {"path": "artifacts/bench.txt"}, "derivation": inline_ok}) + "\n")
wt(os.path.join(work, "ak-ca-missing.jsonl"),
   json.dumps({"claim": "overhead 2.35x", "kind": "number", "value": 2.35,
               "source": {"path": "artifacts/bench.txt"},
               "derivation": "derivations/ghost.json"}) + "\n")
wt(os.path.join(work, "ak-ca-legacy.jsonl"),
   json.dumps({"claim": "overhead 2.35x", "kind": "number", "value": 2.35,
               "source": {"path": "artifacts/bench.txt"}}) + "\n")
print("ak-gen ok")
'@

    $AkC = Join-Path $Work 'ak-root'
    $AkO = Join-Path $Work 'ak-orch'
    New-Item -ItemType Directory -Force -Path $AkC, (Join-Path $Work 'ak-der'), `
        (Join-Path $AkO 'pkg'), (Join-Path $Work 'ak-store') | Out-Null
    New-File (Join-Path $AkO 'pkg\core.py') "def f(): return 1$nl"
    New-File (Join-Path $AkO 'conftest.py') ''
    Push-Location $AkO
    & git init -q 2>&1 | Out-Null
    & git add -A 2>&1 | Out-Null
    & git -c user.email=a@b -c user.name=t commit -qm init 2>&1 | Out-Null
    Pop-Location
    New-File (Join-Path $AkO 'plan.json') '{"proven":true,"commands":["echo ak > ak.txt"],"unproven":[]}'
    $akGenRun = & $akPy $akGen $Work $AkC $AkO 2>&1
    Assert-True ($LASTEXITCODE -eq 0 -and ($akGenRun -join ' ') -match 'ak-gen ok') `
        'AK0a 夹具生成器落盘（推导 JSON/声明集/对抗样本全部现建，真实仓零接触）' (($akGenRun -join ' '))

    $AkD = Join-Path $Work 'ak-der'
    $script:akMrPy = $mrPy
    $script:akCaPy = $caPy
    $script:akAvPy = $avPy

    # --- AK1-AK3: 精确有理数 DAG 往返（5 步链，核对清单逐步可见）---
    $ak1 = Invoke-AkMr (Join-Path $AkD 'roundtrip.json')
    Assert-True ($ak1 -eq 0 -and (($akOut -join ' ') -match 'mre: VERIFIED')) `
        'AK1 精确有理数 DAG 往返：5 步 add/div/mul/sub/mul 全部重算吻合→VERIFIED exit 0' "e=$ak1"
    Assert-True ((($akOut -join "`n") -match 'step 2 div -> s2  declared 1/3  computed 1/3')) `
        'AK2 核对清单按十进制字面量语义精确比对（1/3 就是 1/3，不是 0.333 的浮点影子）' (($akOut -join ' | '))
    Assert-True ((($akOut -join "`n") -match 'target matches \(17/35\)')) `
        'AK3 最终 target 与最后一步重算值精确相等（17/35）' (($akOut -join ' | '))

    # --- AK4: 失配步定位---
    $ak4 = Invoke-AkMr (Join-Path $AkD 'mismatch.json')
    Assert-True ($ak4 -eq 2 -and (($akOut -join ' ') -match 'step 3 \(s3\): declared 999 vs computed 19')) `
        'AK4 重算不符→REJECTED exit 2 并点名第一个失配步（step 3 (s3)：声明 999 重算 19）——比 FABRICATED 更具体' "e=$ak4"

    # --- AK5/AK6: sqrt 保守区间夹逼---
    $ak5 = Invoke-AkMr (Join-Path $AkD 'sqrt-ok.json')
    Assert-True ($ak5 -eq 0) `
        'AK5 sqrt(2)≈1.4142136（tol 1e-5）→有理数区间夹逼后声明值落在区间内→VERIFIED exit 0（无浮点）' "e=$ak5"
    $ak6 = Invoke-AkMr (Join-Path $AkD 'sqrt-lie.json')
    Assert-True ($ak6 -eq 2 -and (($akOut -join ' ') -match 'outside the re-executed interval')) `
        'AK6 sqrt(2) 声明成 1.5→落在夹逼区间外→REJECTED exit 2（区间由纯有理数二分夹出，谎言无处藏）' "e=$ak6"

    # --- AK7/AK8/AK9: pow 三态---
    $ak7 = Invoke-AkMr (Join-Path $AkD 'pow-int.json')
    Assert-True ($ak7 -eq 0 -and (($akOut -join ' ') -match 'computed 1024')) `
        'AK7 整数指数 pow 精确重算（2^10=1024）→VERIFIED exit 0' "e=$ak7"
    $ak8 = Invoke-AkMr (Join-Path $AkD 'pow-frac.json')
    Assert-True ($ak8 -eq 0) `
        'AK8 分数指数 pow（2^(1/2)）非精确→按声明容差区间夹逼→VERIFIED exit 0' "e=$ak8"
    $ak9 = Invoke-AkMr (Join-Path $AkD 'pow-neg.json')
    Assert-True ($ak9 -eq 2 -and (($akOut -join ' ') -match 'fractional power of a negative')) `
        'AK9 负底数分数指数→fail-closed REJECTED exit 2（不定值不开闸，(-8)^(1/3) 也不放行）' "e=$ak9"

    # --- AK10/AK11: 运行期无定义---
    $ak10 = Invoke-AkMr (Join-Path $AkD 'divzero.json')
    Assert-True ($ak10 -eq 2 -and (($akOut -join ' ') -match 'step 3 \(boom\).*division by zero')) `
        'AK10 除零→REJECTED exit 2 点名步骤（除零是推导级错误，不是崩溃）' "e=$ak10"
    $ak11 = Invoke-AkMr (Join-Path $AkD 'sqrtneg.json')
    Assert-True ($ak11 -eq 2 -and (($akOut -join ' ') -match 'sqrt of a negative')) `
        'AK11 负数开方→REJECTED exit 2 点名步骤' "e=$ak11"

    # --- AK12/AK13: DAG 校验与引用解析---
    $ak12 = Invoke-AkMr (Join-Path $AkD 'cycle.json')
    Assert-True ($ak12 -eq 3 -and (($akOut -join ' ') -match 'reference cycle')) `
        'AK12 循环引用→DAG 校验 exit 3 拒并点名（环上的图根本没法拓扑执行）' "e=$ak12"
    $ak13 = Invoke-AkMr (Join-Path $AkD 'ghost.json')
    Assert-True ($ak13 -eq 3 -and (($akOut -join ' ') -match 'neither a literal number nor a step output_id')) `
        'AK13 引用既不是字面量也不是任何 output_id→exit 3 拒（未解析的引用不猜）' "e=$ak13"

    # --- AK14/AK14b: 规模护栏边界（10000 过 / 10001 拒）---
    $ak14 = Invoke-AkMr (Join-Path $AkD 'big10001.json')
    Assert-True ($ak14 -eq 3 -and (($akOut -join ' ') -match 'scale guard')) `
        'AK14 10001 步→规模护栏 exit 3 拒（护栏只认数字，不认"差不多"）' "e=$ak14"
    $ak14b = Invoke-AkMr (Join-Path $AkD 'big10000.json')
    Assert-True ($ak14b -eq 0 -and (($akOut -join ' ') -match '10000 step\(s\) re-executed')) `
        'AK14b 恰好 10000 步→护栏边界内全量重执行→VERIFIED exit 0（0.2s 级，护栏不误伤真推导）' "e=$ak14b"

    # --- AK15/AK16: 精确 op 的容差带---
    $ak15 = Invoke-AkMr (Join-Path $AkD 'tolband.json')
    Assert-True ($ak15 -eq 0) `
        'AK15 精确 op 声明容差带：|3−3.0005|≤0.001→VERIFIED exit 0（容差是声明，不是引擎送的）' "e=$ak15"
    $ak16 = Invoke-AkMr (Join-Path $AkD 'tolband-bad.json')
    Assert-True ($ak16 -eq 2 -and (($akOut -join ' ') -match '\|diff\| 1/2000 > tol 1/10000')) `
        'AK16 容差带被超出→REJECTED exit 2 且给出精确差值（|diff| 1/2000 > tol 1/10000）' "e=$ak16"

    # --- AK17/AK18: UNVERIFIED 的两种触发---
    $ak17 = Invoke-AkMr (Join-Path $AkD 'nonote.json')
    Assert-True ($ak17 -eq 1 -and (($akOut -join ' ') -match 'no note')) `
        'AK17 步骤缺 note→UNVERIFIED exit 1（没有逐步注记的推导不是可审计的过程数据）' "e=$ak17"
    $ak18 = Invoke-AkMr (Join-Path $AkD 'nosteps.json')
    Assert-True ($ak18 -eq 1 -and (($akOut -join ' ') -match 'no derivation graph')) `
        'AK18 无 steps→UNVERIFIED exit 1（没查的东西不许当真）' "e=$ak18"

    # --- AK19: 坏 JSON fail-closed---
    $ak19 = Invoke-AkMr (Join-Path $AkD 'bad.json')
    Assert-True ($ak19 -eq 3 -and (($akOut -join ' ') -match 'unreadable')) `
        'AK19 坏 JSON→exit 3 拒（坏工件上一个数字都不信）' "e=$ak19"

    # --- AK20/AK21: claim_audit 审计模式（number+derivation 两条路径）---
    $ak20 = Invoke-AkCa @('-Claims', (Join-Path $Work 'ak-ca-ok.jsonl'), '-Root', $AkC)
    Assert-True ($ak20 -eq 0 -and (($akOut -join ' ') -match 'derivation re-executed: 2 step\(s\) match') `
        -and (($akOut -join "`n") -match 'summary: derivations verified=1 rejected=0')) `
        'AK20 number+推导文件：工件出处+推导重算双证据→VERIFIED exit 0，derivations 汇总行逐数上报' "e=$ak20"
    $ak21 = Invoke-AkCa @('-Claims', (Join-Path $Work 'ak-ca-bad.jsonl'), '-Root', $AkC)
    Assert-True ($ak21 -eq 2 -and (($akOut -join ' ') -match 'derivation REJECTED: step 2 \(s2\)')) `
        'AK21 数字在工件里但自带推导算不出它→FABRICATED exit 2，失配步点名（工件的哈希证据保留在案）' "e=$ak21"

    # --- AK22: second-opinion 的 number 第二证据路径升级---
    $ak22 = Invoke-AkCa @('second-opinion', '-Claims', (Join-Path $Work 'ak-ca-ok.jsonl'), '-Root', $AkC)
    Assert-True ($ak22 -eq 0 -and (($akOut -join ' ') -match 'dual-source') `
        -and (($akOut -join "`n") -match 'summary: dual-source=1') `
        -and (($akOut -join ' ') -match 'src2: derivation re-executed')) `
        'AK22 独立复核：src1=工件出处 + src2=推导重执行，两路一致→number 升 dual-source exit 0（单源孤儿缺口闭合，L121）' "e=$ak22"

    # --- AK23: 旧 number（无 derivation）行为不变回归---
    $ak23a = Invoke-AkCa @('-Claims', (Join-Path $Work 'ak-ca-legacy.jsonl'), '-Root', $AkC)
    $ak23b = Invoke-AkCa @('second-opinion', '-Claims', (Join-Path $Work 'ak-ca-legacy.jsonl'), '-Root', $AkC)
    Assert-True ($ak23a -eq 0 -and (($akOut -join ' ') -notmatch 'derivation re-executed') `
        -and (($akOut -join "`n") -notmatch 'summary: derivations') `
        -and (($akOut -join ' ') -match 'sha256:')) `
        'AK23 旧 number 回归（审计面）：无 derivation→只走工件出处（sha256 证据），无 derivations 汇总行，行为逐字不变' "e=$ak23a"
    Assert-True ($ak23b -eq 1 -and (($akOut -join ' ') -match 'no independent second path exists for kind number')) `
        'AK23b 旧 number 回归（复核面）：无 derivation 仍明说 number 没有第二证据路径、永不升级（旧语义原样）' "e=$ak23b"

    # --- AK24: 引用的推导文件缺失→FABRICATED---
    $ak24 = Invoke-AkCa @('-Claims', (Join-Path $Work 'ak-ca-missing.jsonl'), '-Root', $AkC)
    Assert-True ($ak24 -eq 2 -and (($akOut -join ' ') -match 'claimed derivation file does not exist')) `
        'AK24 声明引用的推导文件不存在→FABRICATED exit 2（拿不存在的证据自证等于伪造）' "e=$ak24"

    # --- AK25/AK25b: 内嵌 derivation（对象直嵌，不走文件）---
    $ak25 = Invoke-AkCa @('-Claims', (Join-Path $Work 'ak-ca-inline.jsonl'), '-Root', $AkC)
    Assert-True ($ak25 -eq 0 -and (($akOut -join ' ') -match 'derivation re-executed: 2 step\(s\) match') `
        -and (($akOut -join "`n") -match 'summary: derivations verified=1 rejected=0')) `
        'AK25 内嵌 derivation：审计模式重执行通过→VERIFIED exit 0（文件与内嵌同权）' "e=$ak25"
    $ak25b = Invoke-AkCa @('second-opinion', '-Claims', (Join-Path $Work 'ak-ca-inline.jsonl'), '-Root', $AkC)
    Assert-True ($ak25b -eq 0 -and (($akOut -join ' ') -match 'dual-source')) `
        'AK25b 内嵌 derivation 在独立复核下同样升 dual-source exit 0' "e=$ak25b"

    # --- AK26/AK26b: 编排 --derivations 回执字段（appended）---
    $akStore = Join-Path $Work 'ak-store'
    $ak26 = Invoke-AkAv $AkO $akStore (Join-Path $Work 'ak-rc1.json') `
        @('--claims-file', (Join-Path $AkO 'claims-ak-ok.jsonl'))
    $ak26doc = Read-AkReceipt (Join-Path $Work 'ak-rc1.json')
    $ak26steps = @($ak26doc.steps | ForEach-Object { $_.step })
    Assert-True ($ak26 -eq 0 -and $ak26doc.claims.derivations.verified -eq 1 `
        -and $ak26doc.claims.derivations.rejected -eq 0 -and ($ak26steps -contains 'run')) `
        'AK26 编排 --derivations（默认开）：重执行结果进回执 claims.derivations={verified,rejected}（appended 字段），链通过' "e=$ak26"
    $ak26b = Invoke-AkAv $AkO $akStore (Join-Path $Work 'ak-rc2.json') `
        @('--claims-file', (Join-Path $AkO 'claims-ak-legacy.jsonl'))
    $ak26bdoc = Read-AkReceipt (Join-Path $Work 'ak-rc2.json')
    Assert-True ($ak26b -eq 0 -and ($ak26bdoc.claims.PSObject.Properties.Name -notcontains 'derivations')) `
        'AK26b 旧声明集回归（编排面）：无 derivation→回执 claims 不出现 derivations 字段（追加不改建）' "e=$ak26b"

    # --- AK27: 推导 REJECTED 与 FABRICATED 同级拒绝全链---
    $ak27 = Invoke-AkAv $AkO $akStore (Join-Path $Work 'ak-rc3.json') `
        @('--claims-file', (Join-Path $AkO 'claims-ak-bad.jsonl'))
    $ak27doc = Read-AkReceipt (Join-Path $Work 'ak-rc3.json')
    $ak27steps = @($ak27doc.steps | ForEach-Object { $_.step })
    Assert-True ($ak27 -eq 1 -and $ak27doc.verdict -eq 'rejected: derivation mismatch' `
        -and $ak27doc.claims.derivations.rejected -eq 1 -and ($ak27steps -notcontains 'run')) `
        'AK27 编排：推导重执行 REJECTED→全链拒绝在 run 前，verdict=rejected: derivation mismatch（与 FABRICATED 同级）' "e=$ak27 v=$($ak27doc.verdict)"

    # --- AK27b: 独立复核的推导计数进回执 claims.second---
    $ak27b = Invoke-AkAv $AkO $akStore (Join-Path $Work 'ak-rc4.json') `
        @('--claims-file', (Join-Path $AkO 'claims-ak-so.jsonl'), '--second-opinion')
    $ak27bdoc = Read-AkReceipt (Join-Path $Work 'ak-rc4.json')
    Assert-True ($ak27b -eq 0 -and $ak27bdoc.claims.second.dual_source -eq 1 `
        -and $ak27bdoc.claims.second.derivations.verified -eq 1) `
        'AK27b 编排 --second-opinion：number 经工件+推导两路升 dual-source，claims.second.derivations 同形上报' "e=$ak27b"

    # --- AK28: 对抗自测——20 条"数字撒谎但附假推导"---
    $ak28 = Invoke-AkCa @('-Claims', (Join-Path $Work 'fool-claims.jsonl'), '-Root', $AkC)
    $ak28top = @($akOut | Where-Object { $_ -match '^\s*\[\d+\] FABRICATED' }).Count
    $ak28named = @($akOut | Where-Object { $_ -match 'derivation REJECTED: step 2 \(s2\)' }).Count
    Assert-True ($ak28 -eq 2 -and $ak28top -eq 20 -and $ak28named -eq 20 `
        -and (($akOut -join ' ') -match 'fabricated=20') `
        -and (($akOut -join ' ') -match 'summary: derivations verified=0 rejected=20')) `
        'AK28 对抗自测：20 条"数字在工件里、假推导背书"的声明 20/20 FABRICATED，逐条点名失配步 step 2 (s2)，derivations rejected=20' "e=$ak28 top=$ak28top named=$ak28named"
}


"selftest: passed=$passed failed=$failed skipped=$skipped  (work dir: $Work)"
Write-SelftestRecord -Root $repo -Passed $passed -Failed $failed -Skipped $skipped
if ($failed -gt 0) { exit 1 }
exit 0
