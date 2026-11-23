#!/usr/bin/env pwsh
# Rust trit-doctor 与 Python tools/doctor.py 的同机 A/B。
# 验收线（docs/engineering/DOCTOR-V2.md §4）：暖态中位 < 0.15 s，且除顶层 at 外 JSON 逐字节一致。
#
#   pwsh tools/doctor_ab.ps1 [-Runs 7] [-Rust <path>]
#
# 退出码：0 = 两项都过；1 = 超时或 JSON 不一致；2 = 构建/定位失败。

param(
    [int]$Runs = 7,
    [string]$Rust = ""
)

$ErrorActionPreference = "Stop"
$root = Split-Path -Parent $PSScriptRoot
Set-Location $root

# 本机 PATH 常缺 git/cargo；只在缺失时补，不覆盖用户已有配置。
$gitCmd = "C:\Program Files\Git\cmd"
$cargoBin = Join-Path $env:USERPROFILE ".cargo\bin"
if ((Test-Path -LiteralPath $gitCmd) -and -not (Get-Command git -ErrorAction SilentlyContinue)) {
    $env:Path = "$gitCmd;$env:Path"
}
if ((Test-Path -LiteralPath $cargoBin) -and -not (Get-Command cargo -ErrorAction SilentlyContinue)) {
    $env:Path = "$cargoBin;$env:Path"
}
$env:RUSTC_WRAPPER = ""

if (-not (Get-Command cargo -ErrorAction SilentlyContinue)) {
    Write-Output "doctor_ab: cargo not found"
    exit 2
}
if (-not (Get-Command python -ErrorAction SilentlyContinue)) {
    Write-Output "doctor_ab: python not found"
    exit 2
}

cargo build --release -p trit-doctor --quiet
if ($LASTEXITCODE -ne 0) {
    Write-Output "doctor_ab: cargo build failed"
    exit 2
}

if (-not $Rust) {
    $meta = cargo metadata --format-version 1 --no-deps | ConvertFrom-Json
    $Rust = Join-Path $meta.target_directory "release/trit-doctor.exe"
}
if (-not (Test-Path -LiteralPath $Rust)) {
    Write-Output "doctor_ab: rust binary not found: $Rust"
    exit 2
}

$pyCmd = @("python", "-I", "tools/doctor.py", "--json")
$rsCmd = @($Rust, "--root", ".", "--json")

function Invoke-Doctor([string[]]$Command) {
    $output = & $Command[0] $Command[1..($Command.Length - 1)] 2>&1
    if ($LASTEXITCODE -ne 0) {
        throw ("command failed (%d): %s" -f $LASTEXITCODE, ($Command -join " "))
    }
    return ($output -join "`n")
}

# 先各跑两遍，确保 gates 缓存与 git 缓存都进入暖态。
Invoke-Doctor $pyCmd > $null
Invoke-Doctor $pyCmd > $null
Invoke-Doctor $rsCmd > $null
Invoke-Doctor $rsCmd > $null

$pyJson = Invoke-Doctor $pyCmd
$rsJson = Invoke-Doctor $rsCmd
$pyNorm = $pyJson -replace '"at": "[^"]*"', '"at": "X"'
$rsNorm = $rsJson -replace '"at": "[^"]*"', '"at": "X"'
$jsonOk = ($pyNorm -ceq $rsNorm)

$pyTimes = @()
$rsTimes = @()
1..$Runs | ForEach-Object {
    $pyTimes += (Measure-Command { Invoke-Doctor $pyCmd > $null }).TotalSeconds
}
1..$Runs | ForEach-Object {
    $rsTimes += (Measure-Command { Invoke-Doctor $rsCmd > $null }).TotalSeconds
}
$pySorted = $pyTimes | Sort-Object
$rsSorted = $rsTimes | Sort-Object
$pyMedian = $pySorted[[math]::Floor($Runs / 2)]
$rsMedian = $rsSorted[[math]::Floor($Runs / 2)]
$threshold = 0.15
$speedOk = ($rsMedian -lt $threshold)

Write-Output ("doctor A/B: runs={0}  python_median={1:0.000}s  rust_median={2:0.000}s  speedup={3:N2}x" -f `
        $Runs, $pyMedian, $rsMedian, ($pyMedian / $rsMedian))
Write-Output ("JSON(除顶层 at): {0}" -f $(if ($jsonOk) { "byte-identical" } else { "DIFF" }))
Write-Output ("暖态验收 < {0:0.000}s: {1}" -f $threshold, $(if ($speedOk) { "PASS" } else { "FAIL" }))

if (-not $jsonOk -or -not $speedOk) {
    exit 1
}
exit 0
